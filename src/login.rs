//! トークンを手で貼らずにログインする方法。
//! - QR コード: 公式デスクトップ版と同じ Remote Auth。スマホの Discord アプリで読み取って承認する（パスワードを扱わない）
//! - メールアドレスとパスワード: /auth/login。二段階認証（認証アプリのコード）にも対応する
//!
//! どちらも最後に受け取るのは今までと同じユーザートークンで、それを LoginEvent::Token で渡す。

use std::sync::atomic::{AtomicBool, Ordering::Relaxed};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, Result};
use crate::wake::Waker;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use base64::Engine;
use futures_util::{SinkExt, StreamExt};
use rsa::pkcs8::EncodePublicKey;
use rsa::{Oaep, RsaPrivateKey};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::Message as WsMsg;

const API: &str = "https://discord.com/api/v9";
const REMOTE_AUTH: &str = "wss://remote-auth-gateway.discord.gg/?v=2";
const USER_AGENT: &str = "Mozilla/5.0 (X11; Linux x86_64) riscord/0.1";

pub enum LoginEvent {
    /// QR コードにする URL（https://discord.com/ra/…）
    QrUrl(String),
    /// スマホで読み取られた。スマホ側で承認されるのを待っている
    QrScanned { name: String, avatar_url: Option<String> },
    /// QR コードの期限切れ・スマホ側での取り消し。説明を添える（作り直せる）
    QrEnded(String),
    /// ログインできた
    Token(String),
    /// 二段階認証のコードが要る（ticket を MFA の送信に使う）
    Mfa { ticket: String },
    /// 失敗した理由（利用者向け）
    Error(String),
}

#[derive(Clone)]
struct Tx {
    tx: Sender<LoginEvent>,
    ctx: Waker,
}

impl Tx {
    fn send(&self, e: LoginEvent) -> bool {
        let ok = self.tx.send(e).is_ok();
        self.ctx.wake();
        ok
    }
}

fn spawn<F>(ctx: &Waker, f: impl FnOnce(Tx) -> F + Send + 'static) -> Receiver<LoginEvent>
where
    F: std::future::Future<Output = ()>,
{
    let (tx, rx) = channel();
    let tx = Tx { tx, ctx: ctx.clone() };
    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().expect("tokio");
        rt.block_on(f(tx));
    });
    rx
}

fn http() -> reqwest::Client {
    reqwest::Client::builder().user_agent(USER_AGENT).connect_timeout(Duration::from_secs(10)).timeout(Duration::from_secs(30)).build().expect("http")
}

// ---- QR コード ----

/// QR コードでのログインを始める。cancel を true にすると止まる（画面を離れたとき・ログインできたとき）
pub fn start_qr(ctx: &Waker) -> (Receiver<LoginEvent>, Arc<AtomicBool>) {
    let cancel = Arc::new(AtomicBool::new(false));
    let c = cancel.clone();
    let rx = spawn(ctx, move |tx| async move {
        if let Err(e) = remote_auth(&tx, &c).await {
            if !c.load(Relaxed) {
                eprintln!("QR ログイン失敗: {e:#}");
                tx.send(LoginEvent::QrEnded("QR コードを用意できませんでした。通信状態を確認して、作り直してください。".into()));
            }
        }
    });
    (rx, cancel)
}

async fn remote_auth(tx: &Tx, cancel: &AtomicBool) -> Result<()> {
    // 鍵の生成は少し重いので、接続より先に済ませる
    let key = tokio::task::spawn_blocking(|| RsaPrivateKey::new(&mut rand::rngs::OsRng, 2048)).await??;
    let public = STANDARD.encode(key.to_public_key().to_public_key_der()?.as_bytes());
    let decrypt = |b64: &str| -> Result<Vec<u8>> { Ok(key.decrypt(Oaep::new::<Sha256>(), &STANDARD.decode(b64)?)?) };

    let mut req = REMOTE_AUTH.into_client_request()?;
    // Origin が discord.com でないと接続を断られる
    req.headers_mut().insert("Origin", "https://discord.com".parse()?);
    let (ws, _) = tokio_tungstenite::connect_async(req).await?;
    let (mut write, mut read) = ws.split();
    let mut heartbeat = tokio::time::interval(Duration::from_secs(40));
    heartbeat.tick().await;

    loop {
        if cancel.load(Relaxed) {
            return Ok(());
        }
        let msg = tokio::select! {
            _ = heartbeat.tick() => {
                write.send(WsMsg::Text(json!({ "op": "heartbeat" }).to_string().into())).await?;
                continue;
            }
            // 取り消しに気づけるよう、何も届かなくても時々見に行く
            _ = tokio::time::sleep(Duration::from_millis(500)) => continue,
            m = read.next() => m,
        };
        let text = match msg {
            Some(Ok(WsMsg::Text(t))) => t.to_string(),
            Some(Ok(WsMsg::Close(_))) | None => {
                tx.send(LoginEvent::QrEnded("QR コードの期限が切れました。作り直してください。".into()));
                return Ok(());
            }
            Some(Ok(_)) => continue,
            Some(Err(e)) => return Err(e.into()),
        };
        let v: Value = serde_json::from_str(&text)?;
        match v["op"].as_str().unwrap_or("") {
            "hello" => {
                if let Some(ms) = v["heartbeat_interval"].as_u64() {
                    heartbeat = tokio::time::interval(Duration::from_millis(ms));
                    heartbeat.tick().await;
                }
                write.send(WsMsg::Text(json!({ "op": "init", "encoded_public_key": public }).to_string().into())).await?;
            }
            "nonce_proof" => {
                let nonce = decrypt(v["encrypted_nonce"].as_str().ok_or_else(|| anyhow!("nonce がない"))?)?;
                let proof = URL_SAFE_NO_PAD.encode(Sha256::digest(&nonce));
                write.send(WsMsg::Text(json!({ "op": "nonce_proof", "proof": proof }).to_string().into())).await?;
            }
            "pending_remote_init" => {
                let fp = v["fingerprint"].as_str().ok_or_else(|| anyhow!("fingerprint がない"))?;
                if !tx.send(LoginEvent::QrUrl(format!("https://discord.com/ra/{fp}"))) {
                    return Ok(());
                }
            }
            "pending_ticket" => {
                // 「ユーザーID:識別番号:アバター:ユーザー名」
                let payload = String::from_utf8(decrypt(v["encrypted_user_payload"].as_str().unwrap_or(""))?)?;
                let parts: Vec<&str> = payload.splitn(4, ':').collect();
                let (id, avatar, name) = (parts.first().copied().unwrap_or(""), parts.get(2).copied().unwrap_or(""), parts.get(3).copied().unwrap_or(""));
                let avatar_url = (!avatar.is_empty() && avatar != "0").then(|| format!("https://cdn.discordapp.com/avatars/{id}/{avatar}.png?size=128"));
                tx.send(LoginEvent::QrScanned { name: name.to_string(), avatar_url });
            }
            "pending_login" => {
                let ticket = v["ticket"].as_str().ok_or_else(|| anyhow!("ticket がない"))?;
                let r: Value = http().post(format!("{API}/users/@me/remote-auth/login")).json(&json!({ "ticket": ticket })).send().await?.error_for_status()?.json().await?;
                let token = String::from_utf8(decrypt(r["encrypted_token"].as_str().ok_or_else(|| anyhow!("token がない"))?)?)?;
                tx.send(LoginEvent::Token(token));
                return Ok(());
            }
            // 古い手順（トークンが直接届く）
            "pending_finish" | "finish" if v["encrypted_token"].is_string() => {
                let token = String::from_utf8(decrypt(v["encrypted_token"].as_str().unwrap())?)?;
                tx.send(LoginEvent::Token(token));
                return Ok(());
            }
            "cancel" => {
                tx.send(LoginEvent::QrEnded("スマホでログインが取り消されました。もう一度読み取るには作り直してください。".into()));
                return Ok(());
            }
            _ => {}
        }
    }
}

/// QR コードの白黒のマス目（true = 黒）。1辺のマス数と、行ごとに並べた値
pub fn qr_modules(url: &str) -> Option<(usize, Vec<bool>)> {
    let code = qrcode::QrCode::new(url.as_bytes()).ok()?;
    let colors = code.to_colors().into_iter().map(|c| c == qrcode::Color::Dark).collect();
    Some((code.width(), colors))
}

// ---- メールアドレスとパスワード ----

pub fn password(ctx: &Waker, login: String, password: String) -> Receiver<LoginEvent> {
    spawn(ctx, move |tx| async move {
        let body = json!({ "login": login, "password": password, "undelete": false, "login_source": null, "gift_code_sku_id": null });
        tx.send(match post(&format!("{API}/auth/login"), &body).await {
            Ok(v) => match (v["token"].as_str(), v["ticket"].as_str()) {
                (Some(t), _) => LoginEvent::Token(t.to_string()),
                (None, Some(ticket)) if v["mfa"].as_bool() == Some(true) => {
                    if v["totp"].as_bool() == Some(false) {
                        LoginEvent::Error("このアカウントの二段階認証（SMS・セキュリティキー）には対応していません。QR コードでログインしてください。".into())
                    } else {
                        LoginEvent::Mfa { ticket: ticket.to_string() }
                    }
                }
                _ => LoginEvent::Error("ログインできませんでした。QR コードでのログインを試してください。".into()),
            },
            Err(e) => LoginEvent::Error(e),
        });
    })
}

/// 二段階認証のコード（認証アプリの6桁、またはバックアップコード）を送る
pub fn mfa(ctx: &Waker, ticket: String, code: String) -> Receiver<LoginEvent> {
    spawn(ctx, move |tx| async move {
        let code: String = code.chars().filter(|c| !c.is_whitespace() && *c != '-').collect();
        let body = json!({ "code": code, "ticket": ticket, "login_source": null, "gift_code_sku_id": null });
        tx.send(match post(&format!("{API}/auth/mfa/totp"), &body).await {
            Ok(v) => match v["token"].as_str() {
                Some(t) => LoginEvent::Token(t.to_string()),
                None => LoginEvent::Error("ログインできませんでした。最初からやり直してください。".into()),
            },
            Err(e) => LoginEvent::Error(e),
        });
    })
}

/// JSON を送り、成功なら応答を返す。失敗なら利用者向けの理由
async fn post(url: &str, body: &Value) -> std::result::Result<Value, String> {
    let r = http().post(url).json(body).send().await.map_err(|e| {
        eprintln!("ログイン通信失敗: {e:#}");
        "Discord に接続できませんでした。通信状態を確認してください。".to_string()
    })?;
    let status = r.status().as_u16();
    let v: Value = r.json().await.unwrap_or_default();
    if (200..300).contains(&status) {
        return Ok(v);
    }
    eprintln!("ログイン失敗: {status} {v}");
    Err(login_error_text(status, &v))
}

/// ログインの失敗を利用者向けの言葉にする
pub fn login_error_text(status: u16, v: &Value) -> String {
    let raw = v.to_string();
    let code = v["code"].as_u64().unwrap_or(0);
    if v.get("captcha_key").is_some() || raw.contains("captcha") {
        "Discord が確認（CAPTCHA）を求めています。Riscord では表示できないため、QR コードでログインしてください。".into()
    } else if status == 429 {
        "試行回数が多すぎます。しばらく待ってからもう一度お試しください。".into()
    } else if raw.contains("INVALID_LOGIN") || raw.contains("INVALID_PASSWORD") || raw.contains("PASSWORD_DOES_NOT_MATCH") {
        "メールアドレスまたはパスワードが違います。".into()
    } else if code == 60008 || raw.contains("INVALID_MFA") {
        "認証コードが違います。".into()
    } else if code == 60006 || raw.contains("ticket") {
        "時間が経ちすぎました。最初からログインし直してください。".into()
    } else if raw.contains("VERIFICATION") || raw.contains("NEW_LOGIN_LOCATION") || code == 70007 {
        "新しい場所からのログインです。Discord から届いたメールで承認してから、もう一度ログインしてください。".into()
    } else if code == 20013 || raw.contains("ACCOUNT_DISABLED") {
        "このアカウントは無効になっています。".into()
    } else {
        format!("ログインできませんでした（{status}）。QR コードでのログインも試してください。")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn login_errors_are_explained() {
        assert!(login_error_text(400, &json!({ "captcha_key": ["captcha-required"] })).contains("QR コード"));
        assert!(login_error_text(400, &json!({ "code": 50035, "errors": { "login": { "_errors": [{ "code": "INVALID_LOGIN" }] } } })).contains("違います"));
        assert!(login_error_text(400, &json!({ "code": 60008 })).contains("認証コード"));
        assert!(login_error_text(429, &json!({ "retry_after": 3 })).contains("多すぎます"));
        assert!(login_error_text(500, &json!({})).contains("500"));
    }

    #[test]
    fn qr_url_becomes_modules() {
        let (w, cells) = qr_modules("https://discord.com/ra/abcdefghijklmnopqrstuvwxyz0123456789").unwrap();
        assert_eq!(cells.len(), w * w);
        assert!(w >= 21 && cells.iter().any(|&c| c) && cells.iter().any(|&c| !c));
    }

    #[test]
    fn rsa_round_trip_matches_remote_auth() {
        // サーバーと同じ手順（公開鍵で暗号化 → 秘密鍵で復号）が通ること
        let key = RsaPrivateKey::new(&mut rand::rngs::OsRng, 1024).unwrap();
        let enc = key.to_public_key().encrypt(&mut rand::rngs::OsRng, Oaep::new::<Sha256>(), b"nonce").unwrap();
        assert_eq!(key.decrypt(Oaep::new::<Sha256>(), &enc).unwrap(), b"nonce");
        assert!(key.to_public_key().to_public_key_der().is_ok());
        let proof = URL_SAFE_NO_PAD.encode(Sha256::digest(b"nonce"));
        assert!(!proof.contains('=') && !proof.contains('+'));
    }
}
