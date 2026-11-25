//! Mobile transport. Owns credentials, reconnects the gateway, and reuses desktop REST routes.
use crate::{login::LoginEvent, rest::ApiCall};
use anyhow::{bail, Result};
use futures_util::SinkExt;
use serde_json::{json, Value};
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::task::JoinHandle;

/// これだけつながっていたら、次の再接続の待ち時間を 1 秒に戻す
const STABLE_CONNECTION: Duration = Duration::from_secs(60);
use tokio_tungstenite::tungstenite::Message;

const API: &str = "https://discord.com/api/v10";
const GATEWAY: &str = "wss://gateway.discord.gg";
const GATEWAY_QUERY: &str = "/?v=10&encoding=json&compress=zlib-stream";
/// 無料アカウントで送れる添付の上限
pub const UPLOAD_LIMIT: u64 = 10 * 1024 * 1024;
/// 画面側で使う種類だけを渡す（READY などの大きいものは必要な部分だけに絞る）
const FORWARD: &[&str] = &[
    "MESSAGE_CREATE",
    "MESSAGE_UPDATE",
    "MESSAGE_DELETE",
    "MESSAGE_DELETE_BULK",
    "MESSAGE_POLL_VOTE_ADD",
    "MESSAGE_POLL_VOTE_REMOVE",
    "MESSAGE_REACTION_ADD",
    "MESSAGE_REACTION_REMOVE",
    "TYPING_START",
    "CHANNEL_CREATE",
    "CHANNEL_UPDATE",
    "CHANNEL_DELETE",
    "GUILD_CREATE",
    "GUILD_DELETE",
    "CALL_CREATE",
    "CALL_UPDATE",
    "CALL_DELETE",
    "RESUMED",
    "USER_GUILD_SETTINGS_UPDATE",
];

/// トークンが無効になった（ログインし直しが必要）
#[derive(Debug)]
pub struct AuthExpired;
impl std::fmt::Display for AuthExpired {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(tr!(
            "ログインの有効期限が切れました。もう一度ログインしてください。",
            "Your login has expired. Please log in again.",
            "登录已过期。请重新登录。"
        ))
    }
}
impl std::error::Error for AuthExpired {}

use crate::gateway::{close_action, CloseAction, GatewayClosed, GatewaySession};

pub struct Client {
    lifecycle: Mutex<()>,
    token: Mutex<String>,
    status: Mutex<String>,
    http: reqwest::Client,
    events: Mutex<VecDeque<Value>>,
    notify: tokio::sync::Notify,
    task: Mutex<Option<JoinHandle<()>>>,
    gateway_tx: Mutex<Option<tokio::sync::mpsc::UnboundedSender<Value>>>,
    #[cfg(target_os = "android")]
    pub(crate) voice_ctl: Arc<crate::voice::VoiceCtl>,
    #[cfg(target_os = "android")]
    voice_tx: Mutex<Option<tokio::sync::mpsc::UnboundedSender<crate::mobile_voice::Command>>>,
    #[cfg(target_os = "android")]
    voice_task: Mutex<Option<JoinHandle<()>>>,
    login_cancel: Mutex<Option<Arc<std::sync::atomic::AtomicBool>>>,
    /// connect / stop のたびに進める。古い connect が終わってから Gateway や通話を始めないように
    connect_gen: std::sync::atomic::AtomicU64,
    /// 画面側が付けた、いまの connect の番号（disconnect で古い接続だけを止めるため）
    attempt: std::sync::atomic::AtomicI64,
    cancelled_through: std::sync::atomic::AtomicI64,
}
impl Client {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            lifecycle: Mutex::new(()),
            #[cfg(target_os = "android")]
            voice_ctl: Arc::new(crate::voice::VoiceCtl::default()),
            #[cfg(target_os = "android")]
            voice_tx: Mutex::new(None),
            #[cfg(target_os = "android")]
            voice_task: Mutex::new(None),
            token: Mutex::new(String::new()),
            status: Mutex::new("online".into()),
            http: reqwest::Client::builder()
                // つながるまでと、届くのが止まってからの待ちを分ける（大きい添付の送信は全体では時間がかかる）
                .connect_timeout(Duration::from_secs(15))
                .read_timeout(Duration::from_secs(30))
                .user_agent(concat!("Riscord/", env!("CARGO_PKG_VERSION"), " (Android)"))
                .build()
                .expect("http"),
            events: Mutex::new(VecDeque::new()),
            notify: tokio::sync::Notify::new(),
            task: Mutex::new(None),
            gateway_tx: Mutex::new(None),
            login_cancel: Mutex::new(None),
            connect_gen: std::sync::atomic::AtomicU64::new(0),
            attempt: std::sync::atomic::AtomicI64::new(0),
            cancelled_through: std::sync::atomic::AtomicI64::new(-1),
        })
    }
    pub fn push(&self, event: Value) {
        {
            let mut q = self.events.lock().unwrap();
            if q.len() >= 1024 {
                q.clear();
                q.push_back(json!({"t":"RESYNC","d":{}}));
            }
            q.push_back(event);
        }
        self.notify.notify_one();
    }
    pub fn drain(&self) -> Value {
        Value::Array(self.events.lock().unwrap().drain(..).collect())
    }
    /// イベントが届くまで最大 wait だけ待ってから取り出す（届いていればすぐ返す）
    pub async fn wait_events(&self, wait: Duration) -> Value {
        let deadline = tokio::time::Instant::now() + wait;
        loop {
            if !self.events.lock().unwrap().is_empty() {
                return self.drain();
            }
            if tokio::time::timeout_at(deadline, self.notify.notified())
                .await
                .is_err()
            {
                return self.drain();
            }
            // 起こされたが空なら（終了・古い通知）呼び出し側へ返して状態を見直させる
            if self.events.lock().unwrap().is_empty() {
                return json!([]);
            }
        }
    }
    pub fn stop(&self) {
        let _guard = self.lifecycle.lock().unwrap();
        if let Some(c) = self.login_cancel.lock().unwrap().take() {
            c.store(true, std::sync::atomic::Ordering::Relaxed);
        }
        self.stop_connection();
    }
    /// 接続（Gateway・通話・トークン）だけを止める。進めているログインはそのまま
    fn stop_connection(&self) {
        self.connect_gen
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        #[cfg(target_os = "android")]
        {
            self.voice_tx.lock().unwrap().take();
            if let Some(t) = self.voice_task.lock().unwrap().take() {
                t.abort();
            }
        }
        self.pause_inner();
        self.token.lock().unwrap().clear();
        self.notify.notify_one();
    }
    /// Gateway だけを切る（トークンと通話は残す）。画面を閉じて常駐しないときに使う
    pub fn pause(&self) {
        let _guard = self.lifecycle.lock().unwrap();
        self.pause_inner();
    }
    fn pause_inner(&self) {
        if let Some(task) = self.task.lock().unwrap().take() {
            task.abort();
        }
        self.gateway_tx.lock().unwrap().take();
    }
    /// pause した Gateway をつなぎ直す
    pub fn resume(self: &Arc<Self>) -> Result<Value> {
        let _guard = self.lifecycle.lock().unwrap();
        anyhow::ensure!(!self.token.lock().unwrap().is_empty(), "接続されていません");
        // 致命的な切断で Gateway のタスクが終わっていても、つなぎ直せるようにする
        let needs_start = self
            .task
            .lock()
            .unwrap()
            .as_ref()
            .is_none_or(|t| t.is_finished());
        if needs_start {
            self.start_gateway();
        }
        Ok(Value::Null)
    }
    fn start_gateway(self: &Arc<Self>) {
        // 前のタスクが残っていれば止めてから（2 本の Gateway が同じトークンでつながらないように）
        self.pause_inner();
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        *self.gateway_tx.lock().unwrap() = Some(tx);
        let client = self.clone();
        *self.task.lock().unwrap() = Some(tokio::spawn(async move {
            client.gateway(rx).await;
        }));
    }
    pub fn login(self: &Arc<Self>, mode: &str, args: &Value) {
        if let Some(c) = self.login_cancel.lock().unwrap().take() {
            c.store(true, std::sync::atomic::Ordering::Relaxed);
        }
        let waker = crate::wake::Waker::default();
        let rx = match mode {
            "qr" => {
                let (rx, cancel) = crate::login::start_qr(&waker);
                *self.login_cancel.lock().unwrap() = Some(cancel);
                rx
            }
            "mfa" => crate::login::mfa(&waker, text(args, "ticket"), text(args, "code")),
            _ => crate::login::password(&waker, text(args, "email"), text(args, "password")),
        };
        let weak = Arc::downgrade(self);
        std::thread::spawn(move || {
            while let Ok(event) = rx.recv() {
                let Some(client) = weak.upgrade() else { break };
                client.push(match event {
                    LoginEvent::Token(token) => json!({"t":"LOGIN_TOKEN","d":{"token":token}}),
                    LoginEvent::Mfa { ticket } => json!({"t":"LOGIN_MFA","d":{"ticket":ticket}}),
                    LoginEvent::QrUrl(url) => {
                        let modules = crate::login::qr_modules(&url)
                            .map(|(size, bits)| json!({"size":size,"bits":bits}));
                        json!({"t":"LOGIN_QR","d":{"url":url,"modules":modules}})
                    }
                    LoginEvent::QrScanned { name, .. } => {
                        json!({"t":"LOGIN_SCANNED","d":{"name":name}})
                    }
                    LoginEvent::QrEnded(error) | LoginEvent::Error(error) => {
                        json!({"t":"ERROR","d":{"message":error}})
                    }
                });
            }
        });
    }
    /// attempt は画面側が付ける番号。disconnect で、この connect の結果だけを取り消せる
    pub async fn connect(
        self: &Arc<Self>,
        token: String,
        status: Option<&str>,
        attempt: i64,
    ) -> Result<Value> {
        use std::sync::atomic::Ordering::SeqCst;
        let gen = {
            let _guard = self.lifecycle.lock().unwrap();
            anyhow::ensure!(
                attempt >= self.attempt.load(SeqCst)
                    && attempt > self.cancelled_through.load(SeqCst),
                "接続は取り消されました"
            );
            if let Some(c) = self.login_cancel.lock().unwrap().take() {
                c.store(true, std::sync::atomic::Ordering::Relaxed);
            }
            self.stop_connection();
            let gen = self.connect_gen.load(SeqCst);
            self.attempt.store(attempt, SeqCst);
            *self.token.lock().unwrap() = token;
            if let Some(status) = status.filter(|s| valid_status(s)) {
                *self.status.lock().unwrap() = status.into();
            }
            gen
        };
        let me = self.api(ApiCall::GetMe).await?;
        // 待っている間に stop・新しい connect が来た: 古いトークンで Gateway や通話を始めない
        let _guard = self.lifecycle.lock().unwrap();
        anyhow::ensure!(
            self.connect_gen.load(SeqCst) == gen,
            "接続は取り消されました"
        );
        #[cfg(target_os = "android")]
        {
            let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
            *self.voice_tx.lock().unwrap() = Some(tx);
            let (weak, ctl, me) = (
                Arc::downgrade(self),
                self.voice_ctl.clone(),
                me["id"].as_str().unwrap_or_default().to_owned(),
            );
            *self.voice_task.lock().unwrap() =
                Some(tokio::spawn(crate::mobile_voice::run(weak, me, ctl, rx)));
        }
        self.start_gateway();
        Ok(me)
    }
    /// attempt の connect がいまの接続なら止める（画面側で取り消した connect が、あとから終わったとき）。
    /// 新しい connect が始まっていれば何もしない。そのあとに始めたログイン（QR など）は止めない
    pub fn disconnect(&self, attempt: i64) {
        let _guard = self.lifecycle.lock().unwrap();
        self.cancelled_through
            .fetch_max(attempt, std::sync::atomic::Ordering::SeqCst);
        if self.attempt.load(std::sync::atomic::Ordering::SeqCst) == attempt {
            self.stop_connection();
        }
    }
    pub(crate) fn send_gateway(&self, value: Value) -> Result<()> {
        self.gateway_tx
            .lock()
            .unwrap()
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("接続されていません"))?
            .send(value)?;
        Ok(())
    }
    #[cfg(target_os = "android")]
    pub(crate) fn voice(&self, command: crate::mobile_voice::Command) -> Result<Value> {
        self.voice_tx
            .lock()
            .unwrap()
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("接続されていません"))?
            .send(command)?;
        Ok(Value::Null)
    }
    pub fn subscribe_typing(&self, guild: &str, channel: &str) -> Result<Value> {
        anyhow::ensure!(
            !guild.is_empty()
                && guild.bytes().all(|b| b.is_ascii_digit())
                && !channel.is_empty()
                && channel.bytes().all(|b| b.is_ascii_digit()),
            "無効なチャンネル"
        );
        self.send_gateway(typing_subscription(guild, channel))?;
        Ok(Value::Null)
    }
    pub fn presence(&self, status: &str) -> Result<Value> {
        anyhow::ensure!(valid_status(status), "無効なステータス");
        *self.status.lock().unwrap() = status.into();
        // つながっていなくても、次の identify で使う
        let _ = self.send_gateway(
            json!({"op":3,"d":{"status":status,"since":0,"afk":false,"activities":[]}}),
        );
        Ok(Value::Null)
    }
    pub async fn api(&self, call: ApiCall) -> Result<Value> {
        let (method, path, body) = call.route();
        self.request(method, &path, body).await
    }
    pub async fn request(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<Value>,
    ) -> Result<Value> {
        anyhow::ensure!(
            path.starts_with('/') && !path.starts_with("//"),
            "無効なAPIパス"
        );
        let token = self.token.lock().unwrap().clone();
        let url = format!("{API}{path}");
        send_retrying(|| {
            let req = self
                .http
                .request(method.clone(), &url)
                .header("Authorization", &token);
            match &body {
                Some(body) => req.json(body),
                None => req,
            }
        })
        .await
    }
    /// 添付を送る。file は呼び出し側から所有権を受け取り、上限まで読む
    pub async fn upload(
        &self,
        channel: &str,
        filename: &str,
        file: std::fs::File,
        content: &str,
        reply_to: Option<&str>,
    ) -> Result<Value> {
        anyhow::ensure!(
            !channel.is_empty() && channel.bytes().all(|b| b.is_ascii_digit()),
            "無効なチャンネル"
        );
        let bytes = tokio::task::spawn_blocking(move || -> Result<Vec<u8>> {
            use std::io::Read;
            let mut bytes = Vec::new();
            file.take(UPLOAD_LIMIT + 1).read_to_end(&mut bytes)?;
            Ok(bytes)
        })
        .await??;
        anyhow::ensure!(bytes.len() as u64 <= UPLOAD_LIMIT, "添付は10MBまでです");
        let payload = upload_payload(channel, filename, content, reply_to)?.to_string();
        let token = self.token.lock().unwrap().clone();
        let url = format!("{API}/channels/{channel}/messages");
        send_retrying(|| {
            // 送り直すときは作り直す（multipart の本文は使い回せない）
            let part =
                reqwest::multipart::Part::bytes(bytes.clone()).file_name(filename.to_owned());
            let form = reqwest::multipart::Form::new()
                .text("payload_json", payload.clone())
                .part("files[0]", part);
            self.http
                .post(&url)
                .header("Authorization", &token)
                .multipart(form)
        })
        .await
    }
    async fn gateway(self: Arc<Self>, mut commands: tokio::sync::mpsc::UnboundedReceiver<Value>) {
        let mut session = GatewaySession::default();
        // つながらなかった回数（Hello が届いたら数え直す）と、identify が READY まで行かなかった回数は別に数える
        // （デスクトップの src/net.rs と同じ考え方）
        let mut conn_failures: u32 = 0;
        let mut identify_failures: u32 = 0;
        loop {
            self.push(json!({"t":"CONNECTION","d":{"status":"接続中"}}));
            let mut ready_at = None;
            let mut hello = false;
            // 今回は再開ではなく identify を送る接続か
            let identifying = session.resumable().is_none();
            let end = self
                .connection(&mut session, &mut commands, &mut ready_at, &mut hello)
                .await
                .unwrap_or(ConnEnd::Resume);
            let ready = ready_at.is_some();
            if hello {
                conn_failures = 0;
            }
            // しばらく安定してつながっていたときだけ identify の待ち時間を戻す。
            // READY の直後に切られ続ける状態で、数秒おきに identify し続けないように
            if ready_at.is_some_and(|t: std::time::Instant| t.elapsed() >= STABLE_CONNECTION) {
                identify_failures = 0;
            }
            let identify_failed = identifying && hello && !ready;
            let wait = match end {
                ConnEnd::Stopped => return,
                ConnEnd::Fatal(code) => {
                    if code == 4004 {
                        self.push(json!({"t":"AUTH_EXPIRED","d":{}}));
                    }
                    self.push(json!({"t":"ERROR","d":{"message":if code==4004 {tr!("認証に失敗しました。ログインし直してください。", "Authentication failed. Please log in again.", "认证失败。请重新登录。").to_string()} else {tr!("接続が拒否されました（{code}）", "Connection refused ({code})", "连接被拒绝（{code}）",)}}}));
                    self.push(json!({"t":"CONNECTION","d":{"status":"未接続"}}));
                    return;
                }
                // 再開できない: 一からログインし直す。最低 1〜5 秒、失敗が続けば最長 5 分まで延ばす
                ConnEnd::Reidentify => {
                    session.reset_keeping_unsent();
                    identify_failures = identify_failures.saturating_add(1);
                    reidentify_delay(identify_failures - 1)
                }
                ConnEnd::Resume if identify_failed => {
                    identify_failures = identify_failures.saturating_add(1);
                    reidentify_delay(identify_failures - 1)
                }
                ConnEnd::Resume => reconnect_delay(conn_failures),
            };
            conn_failures = conn_failures.saturating_add(1);
            self.push(json!({"t":"CONNECTION","d":{"status":"再接続待ち"}}));
            // 待っている間に届いたコマンドも受け取った時刻つきで溜める（古い VC の参加を後から送らないように）
            if session
                .queue_while(&mut commands, tokio::time::sleep(wait))
                .await
                .is_none()
            {
                return;
            }
        }
    }
    async fn connection(
        &self,
        session: &mut GatewaySession,
        commands: &mut tokio::sync::mpsc::UnboundedReceiver<Value>,
        ready_at: &mut Option<std::time::Instant>,
        hello: &mut bool,
    ) -> Result<ConnEnd> {
        let base = session
            .resume_url
            .as_deref()
            .filter(|_| session.resumable().is_some())
            // Discord 以外の接続先へはトークンを送らない
            .filter(|u| crate::gateway::is_discord_gateway_url(u))
            .unwrap_or(GATEWAY)
            .trim_end_matches('/')
            .to_owned();
        // つながるまで（最長で 1 分ほど）の間に届いたコマンドも、受け取った時刻つきで溜めておく
        let Some(connection) = session
            .queue_while(
                commands,
                crate::gateway::connect_and_hello(&format!("{base}{GATEWAY_QUERY}")),
            )
            .await
        else {
            return Ok(ConnEnd::Stopped);
        };
        let (mut ws, inflater, period) = match connection {
            Ok(connection) => connection,
            Err(error) => {
                if let Some(GatewayClosed(code)) = error.downcast_ref::<GatewayClosed>() {
                    return Ok(ConnEnd::from_close(*code));
                }
                return Err(error);
            }
        };
        *hello = true;
        let token = self.token.lock().unwrap().clone();
        let status = self.status.lock().unwrap().clone();
        let identify = if session.resumable().is_none() {
            json!({"op":2,"d":{"token":token,"properties":{"os":"Android","browser":"Discord Client","device":""},"compress":false,"presence":{"status":status,"activities":[],"since":0,"afk":false}}})
        } else {
            json!({"op":6,"d":{"token":token,"session_id":session.id,"seq":session.seq}})
        };
        ws.send(Message::Text(identify.to_string().into())).await?;
        let end =
            crate::gateway::run_session(ws, inflater, period, session, commands, true, |frame| {
                let t = frame.t.as_str().unwrap_or_default();
                let raw = frame.d.as_deref().map(|d| d.get());
                if t == "READY" || t == "RESUMED" {
                    *ready_at = Some(std::time::Instant::now());
                    self.push(json!({"t":"CONNECTION","d":{"status":"接続済み"}}));
                }
                #[cfg(target_os = "android")]
                match t {
                    "READY" => {
                        // 新しいセッション。前のセッションの VC は切れている
                        let sid = raw
                            .and_then(|d| serde_json::from_str::<ReadySession>(d).ok())
                            .and_then(|r| r.session_id);
                        let _ = self.voice(crate::mobile_voice::Command::Ready(sid));
                    }
                    "VOICE_STATE_UPDATE" | "VOICE_SERVER_UPDATE" => {
                        if let Some(data) = raw.and_then(|d| serde_json::from_str::<Value>(d).ok())
                        {
                            let _ = self.voice(if t == "VOICE_STATE_UPDATE" {
                                crate::mobile_voice::Command::State(data)
                            } else {
                                crate::mobile_voice::Command::Server(data)
                            });
                        }
                    }
                    _ => {}
                }
                if let Some(event) = ui_event(t, raw) {
                    self.push(event);
                }
            })
            .await?;
        Ok(match end {
            crate::gateway::SessionEnd::Resume => ConnEnd::Resume,
            crate::gateway::SessionEnd::Reidentify => ConnEnd::Reidentify,
            crate::gateway::SessionEnd::Closed(code) => ConnEnd::from_close(code),
        })
    }
}

/// 1 回の接続がどう終わったか
#[derive(Debug, PartialEq)]
enum ConnEnd {
    /// 通信が切れた・再接続を頼まれた: 途中から再開する
    Resume,
    /// 再開できない: 一からログインし直す
    Reidentify,
    /// つなぎ直しても無駄（トークン無効など）
    Fatal(u16),
    /// 閉じるよう頼まれた
    Stopped,
}
impl ConnEnd {
    fn from_close(code: u16) -> Self {
        match close_action(code) {
            CloseAction::Fatal => ConnEnd::Fatal(code),
            CloseAction::Reidentify => ConnEnd::Reidentify,
            CloseAction::Resume => ConnEnd::Resume,
        }
    }
}

/// 再接続までの待ち時間。1, 2, 4 … 秒と延ばして 60 秒で止め、同時に大勢がつなぎ直さないよう少しずらす
fn reconnect_delay(failures: u32) -> Duration {
    let secs = (1u64 << failures.min(6)).min(60);
    Duration::from_millis(secs * 1000 + rand::random::<u64>() % 1000)
}

/// 一からログインし直すまでの待ち時間（デスクトップの reidentify_delay と同じ）。最低 1〜5 秒待ち、
/// READY まで届かない失敗が続くほど長く待つ（identify は 1 日 1000 回を超えるとトークンが無効にされる）。
/// 最長 5 分なので、失敗が続いても 1 日 300 回ほどに収まる
fn reidentify_delay(failures: u32) -> Duration {
    let floor = 1000 + rand::random::<u64>() % 4000;
    let backoff = (1u64 << failures.min(9)).min(300) * 1000 + rand::random::<u64>() % 1000;
    Duration::from_millis(floor.max(backoff))
}

/// READY のうち使う部分だけ（大きい READY 全体を Value にしない）
#[cfg(target_os = "android")]
#[derive(serde::Deserialize)]
struct ReadySession {
    #[serde(default)]
    session_id: Option<String>,
}
/// GUILD_CREATE / GUILD_DELETE のうち、サーバー一覧に要る部分
#[derive(serde::Deserialize, serde::Serialize)]
struct GuildSlim {
    id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    icon: Option<String>,
    /// 障害で一時的に使えないだけ（抜けたのではない）
    #[serde(default)]
    unavailable: bool,
}
#[derive(serde::Deserialize)]
struct ReadySlim {
    /// { entries: [...] } か、古い形式では配列そのもの
    #[serde(default)]
    user_guild_settings: Option<Value>,
}
fn upload_payload(
    channel: &str,
    filename: &str,
    content: &str,
    reply_to: Option<&str>,
) -> Result<Value> {
    let mut payload = json!({"content":content,"attachments":[{"id":"0","filename":filename}]});
    if let Some(id) = reply_to {
        anyhow::ensure!(
            !id.is_empty() && id.bytes().all(|b| b.is_ascii_digit()),
            "無効な返信先"
        );
        payload["message_reference"] = json!({"message_id":id,"channel_id":channel});
    }
    Ok(payload)
}

fn typing_subscription(guild: &str, channel: &str) -> Value {
    json!({"op":37,"d":{"subscriptions":{(guild):{"typing":true,"activities":false,"threads":false,"channels":{(channel):[[0,99]]}}}}})
}

/// 画面へ渡す形 {t, d} にする。使わない種類は None（中身も読まない）。READY は必要な部分だけ型で読む
fn ui_event(t: &str, raw: Option<&str>) -> Option<Value> {
    if t == "READY" {
        let settings = raw
            .and_then(|d| serde_json::from_str::<ReadySlim>(d).ok())
            .and_then(|r| r.user_guild_settings)
            .unwrap_or(Value::Null);
        let entries = match settings {
            Value::Array(list) => list,
            Value::Object(mut map) => match map.remove("entries") {
                Some(Value::Array(list)) => list,
                _ => Vec::new(),
            },
            _ => Vec::new(),
        };
        return Some(json!({"t":"READY","d":{"guild_settings":entries}}));
    }
    if !FORWARD.contains(&t) {
        return None;
    }
    // GUILD_CREATE はメンバーやチャンネルまで入っていて大きい。一覧に要る部分だけ渡す
    if t == "GUILD_CREATE" || t == "GUILD_DELETE" {
        let g = raw.and_then(|d| serde_json::from_str::<GuildSlim>(d).ok())?;
        return Some(json!({"t":t,"d":g}));
    }
    let d = raw
        .and_then(|d| serde_json::from_str::<Value>(d).ok())
        .unwrap_or(Value::Null);
    Some(json!({"t":t,"d":d}))
}
/// 429 で送り直す回数と、送り直すまで待ってよい長さ（これより長く待てと言われたら、そのまま利用者に伝える）
const RATE_LIMIT_RETRIES: usize = 2;
const RATE_LIMIT_MAX_WAIT: Duration = Duration::from_secs(10);

/// 送って結果を読む。429 なら Retry-After だけ待って送り直す（上限つき）
async fn send_retrying(build: impl Fn() -> reqwest::RequestBuilder) -> Result<Value> {
    let mut retries = 0;
    loop {
        let r = build().send().await?;
        let status = r.status();
        let header_wait = retry_after_header(r.headers());
        let bytes = r.bytes().await?;
        let v: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
        if status.as_u16() == 429 && retries < RATE_LIMIT_RETRIES {
            if let Some(wait) = retry_wait(&v, header_wait).filter(|w| *w <= RATE_LIMIT_MAX_WAIT) {
                retries += 1;
                tokio::time::sleep(wait).await;
                continue;
            }
        }
        return response(status, &v, header_wait);
    }
}

fn retry_after_header(headers: &reqwest::header::HeaderMap) -> Option<Duration> {
    headers
        .get(reqwest::header::RETRY_AFTER)?
        .to_str()
        .ok()?
        .trim()
        .parse::<f64>()
        .ok()
        .filter(|s| s.is_finite() && *s >= 0.0)
        .and_then(|seconds| Duration::try_from_secs_f64(seconds).ok())
}

/// 本文の retry_after（秒・小数あり）を優先し、無ければ Retry-After ヘッダー
fn retry_wait(v: &Value, header: Option<Duration>) -> Option<Duration> {
    v["retry_after"]
        .as_f64()
        .filter(|s| s.is_finite() && *s >= 0.0)
        .and_then(|seconds| Duration::try_from_secs_f64(seconds).ok())
        .or(header)
}

fn response(
    status: reqwest::StatusCode,
    v: &Value,
    header_wait: Option<Duration>,
) -> Result<Value> {
    if !status.is_success() {
        match status.as_u16() {
            401 => return Err(AuthExpired.into()),
            413 => bail!("ファイルが大きすぎます（10MBまで）"),
            429 => bail!(
                "混雑しています。{}秒後に再試行してください。",
                retry_wait(v, header_wait)
                    .map(|d| d.as_secs_f64().ceil() as u64)
                    .unwrap_or(5)
            ),
            code => bail!(
                "Discord HTTP {code}: {}",
                v["message"].as_str().unwrap_or("操作に失敗しました")
            ),
        }
    }
    Ok(v.clone())
}
fn valid_status(status: &str) -> bool {
    ["online", "idle", "dnd", "invisible"].contains(&status)
}
fn text(v: &Value, k: &str) -> String {
    v[k].as_str().unwrap_or_default().into()
}

#[cfg(test)]
mod tests {
    #[test]
    fn upload_preserves_the_reply_target_and_rejects_invalid_ids() {
        let reply = super::upload_payload("100", "image.png", "hello", Some("200")).unwrap();
        assert_eq!(reply["message_reference"]["channel_id"], "100");
        assert_eq!(reply["message_reference"]["message_id"], "200");
        assert!(super::upload_payload("100", "x", "", None)
            .unwrap()
            .get("message_reference")
            .is_none());
        assert!(super::upload_payload("100", "x", "", Some("bad/id")).is_err());
    }

    #[test]
    fn typing_subscription_targets_the_selected_guild_and_channel() {
        let value = super::typing_subscription("42", "99");
        assert_eq!(value["op"], 37);
        assert_eq!(value["d"]["subscriptions"]["42"]["typing"], true);
        assert_eq!(
            value["d"]["subscriptions"]["42"]["channels"]["99"],
            serde_json::json!([[0, 99]])
        );
    }

    use super::*;
    #[tokio::test]
    async fn stale_disconnect_cannot_stop_newer_connection_and_cancelled_attempt_cannot_start() {
        use std::sync::atomic::Ordering::SeqCst;
        let client = Client::new();
        client.attempt.store(2, SeqCst);
        *client.token.lock().unwrap() = "new-session".into();
        client.disconnect(1);
        assert_eq!(&*client.token.lock().unwrap(), "new-session");
        assert!(client.connect("stale".into(), None, 1).await.is_err());
        assert_eq!(&*client.token.lock().unwrap(), "new-session");
        client.disconnect(3);
        assert!(client.connect("cancelled".into(), None, 3).await.is_err());
        assert_eq!(&*client.token.lock().unwrap(), "new-session");
    }
    #[test]
    fn event_overflow_requests_resync_and_drain_consumes_events() {
        let c = Client::new();
        for i in 0..1025 {
            c.push(json!({"t":"MESSAGE_CREATE","d":{"id":i}}));
        }
        let events = c.drain();
        assert_eq!(events[0]["t"], "RESYNC");
        assert_eq!(events[1]["d"]["id"], 1024);
        assert_eq!(c.drain(), json!([]));
    }
    #[tokio::test]
    async fn rejects_external_api_urls_before_sending_credentials() {
        let c = Client::new();
        assert!(c
            .request(reqwest::Method::GET, "https://example.com", None)
            .await
            .is_err());
        assert!(c
            .request(reqwest::Method::GET, "//example.com", None)
            .await
            .is_err());
    }
    #[tokio::test]
    async fn wait_events_returns_when_an_event_arrives_or_times_out() {
        let c = Client::new();
        let started = std::time::Instant::now();
        assert_eq!(c.wait_events(Duration::from_millis(50)).await, json!([]));
        assert!(started.elapsed() >= Duration::from_millis(50));
        let pusher = c.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(20)).await;
            pusher.push(json!({"t":"X"}));
        });
        let started = std::time::Instant::now();
        assert_eq!(c.wait_events(Duration::from_secs(5)).await[0]["t"], "X");
        assert!(started.elapsed() < Duration::from_secs(2));
    }
    #[test]
    fn gateway_events_are_slimmed_for_the_ui() {
        let ready = r#"{"session_id":"s","guilds":[{"id":"1"}],"user_guild_settings":{"entries":[{"guild_id":"1","muted":true}]}}"#;
        assert_eq!(
            ui_event("READY", Some(ready)),
            Some(json!({"t":"READY","d":{"guild_settings":[{"guild_id":"1","muted":true}]}}))
        );
        // 古い形式（配列そのもの）・無いときも READY は渡す
        assert_eq!(
            ui_event(
                "READY",
                Some(r#"{"user_guild_settings":[{"guild_id":"2"}]}"#)
            ),
            Some(json!({"t":"READY","d":{"guild_settings":[{"guild_id":"2"}]}}))
        );
        assert_eq!(
            ui_event("READY", Some("{}")),
            Some(json!({"t":"READY","d":{"guild_settings":[]}}))
        );
        assert_eq!(ui_event("PRESENCE_UPDATE", Some("{}")), None);
        assert_eq!(
            ui_event("MESSAGE_CREATE", Some(r#"{"id":"5"}"#)),
            Some(json!({"t":"MESSAGE_CREATE","d":{"id":"5"}}))
        );
        assert_eq!(
            ui_event(
                "MESSAGE_DELETE_BULK",
                Some(r#"{"ids":["1"],"channel_id":"2"}"#)
            ),
            Some(json!({"t":"MESSAGE_DELETE_BULK","d":{"ids":["1"],"channel_id":"2"}}))
        );
        assert_eq!(
            ui_event(
                "GUILD_CREATE",
                Some(r#"{"id":"9","name":"g","icon":null,"members":[{"user":{}}],"channels":[]}"#)
            ),
            Some(json!({"t":"GUILD_CREATE","d":{"id":"9","name":"g","unavailable":false}}))
        );
        assert_eq!(
            ui_event("GUILD_DELETE", Some(r#"{"id":"9","unavailable":true}"#)),
            Some(json!({"t":"GUILD_DELETE","d":{"id":"9","unavailable":true}}))
        );
        assert_eq!(close_action(4004), CloseAction::Fatal);
        assert_eq!(close_action(4014), CloseAction::Fatal);
        assert_eq!(close_action(4000), CloseAction::Resume);
        assert_eq!(close_action(1000), CloseAction::Resume);
    }
    #[test]
    fn rate_limit_wait_prefers_the_body_and_falls_back_to_the_header() {
        let header = Some(Duration::from_secs(3));
        assert_eq!(
            retry_wait(&json!({"retry_after":0.5}), header),
            Some(Duration::from_millis(500))
        );
        assert_eq!(retry_wait(&Value::Null, header), header);
        assert_eq!(retry_wait(&json!({"retry_after":-1.0}), None), None);
        assert_eq!(retry_wait(&json!({"retry_after":1e100}), header), header);
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(reqwest::header::RETRY_AFTER, "2".parse().unwrap());
        assert_eq!(retry_after_header(&headers), Some(Duration::from_secs(2)));
        headers.insert(reqwest::header::RETRY_AFTER, "1e100".parse().unwrap());
        assert_eq!(retry_after_header(&headers), None);
        let err =
            response(reqwest::StatusCode::TOO_MANY_REQUESTS, &Value::Null, header).unwrap_err();
        assert!(err.to_string().contains('3'), "{err}");
    }
    #[test]
    fn reidentify_waits_grow_to_five_minutes() {
        assert!(reidentify_delay(0) >= Duration::from_secs(1));
        assert!(reidentify_delay(0) <= Duration::from_secs(6));
        let longest = reidentify_delay(30);
        assert!(longest >= Duration::from_secs(300) && longest < Duration::from_secs(302));
        assert!(reconnect_delay(30) < Duration::from_secs(62));
    }
    #[tokio::test]
    async fn upload_rejects_files_over_the_limit_before_sending() {
        let path = std::env::temp_dir().join(format!("riscord-upload-{}", std::process::id()));
        std::fs::write(&path, vec![0u8; (UPLOAD_LIMIT + 1) as usize]).unwrap();
        let file = std::fs::File::open(&path).unwrap();
        std::fs::remove_file(&path).unwrap();
        let err = Client::new()
            .upload("1", "big.bin", file, "", None)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("10MB"), "{err}");
    }
}
