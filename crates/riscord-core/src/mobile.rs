//! Mobile transport. Owns credentials, reconnects the gateway, and reuses desktop REST routes.
use crate::{inflate::Inflater, login::LoginEvent, rest::ApiCall};
use anyhow::{bail, Result};
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::task::JoinHandle;
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
    "MESSAGE_REACTION_ADD",
    "MESSAGE_REACTION_REMOVE",
    "TYPING_START",
    "CHANNEL_CREATE",
    "CHANNEL_UPDATE",
    "CHANNEL_DELETE",
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
        f.write_str("ログインの有効期限が切れました。もう一度ログインしてください。")
    }
}
impl std::error::Error for AuthExpired {}

/// Gateway が close フレームで切った（コード付き）
#[derive(Debug)]
struct GatewayClosed(u16);
impl std::fmt::Display for GatewayClosed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "gateway close {}", self.0)
    }
}
impl std::error::Error for GatewayClosed {}
/// 再接続しても直らない close コード（認証失敗・intents 不正など）
fn fatal_close(code: u16) -> bool {
    code == 4004 || (4010..=4014).contains(&code)
}

#[derive(Default)]
struct GatewaySession {
    id: String,
    seq: Value,
    resume_url: Option<String>,
}

pub struct Client {
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
}
impl Client {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            #[cfg(target_os = "android")]
            voice_ctl: Arc::new(crate::voice::VoiceCtl::default()),
            #[cfg(target_os = "android")]
            voice_tx: Mutex::new(None),
            #[cfg(target_os = "android")]
            voice_task: Mutex::new(None),
            token: Mutex::new(String::new()),
            status: Mutex::new("online".into()),
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(60))
                .user_agent("Riscord/0.2.1 (Android)")
                .build()
                .expect("http"),
            events: Mutex::new(VecDeque::new()),
            notify: tokio::sync::Notify::new(),
            task: Mutex::new(None),
            gateway_tx: Mutex::new(None),
            login_cancel: Mutex::new(None),
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
        #[cfg(target_os = "android")]
        {
            self.voice_tx.lock().unwrap().take();
            if let Some(t) = self.voice_task.lock().unwrap().take() {
                t.abort();
            }
        }
        self.pause();
        if let Some(c) = self.login_cancel.lock().unwrap().take() {
            c.store(true, std::sync::atomic::Ordering::Relaxed);
        }
        self.token.lock().unwrap().clear();
        self.notify.notify_one();
    }
    /// Gateway だけを切る（トークンと通話は残す）。画面を閉じて常駐しないときに使う
    pub fn pause(&self) {
        if let Some(task) = self.task.lock().unwrap().take() {
            task.abort();
        }
        self.gateway_tx.lock().unwrap().take();
    }
    /// pause した Gateway をつなぎ直す
    pub fn resume(self: &Arc<Self>) -> Result<Value> {
        anyhow::ensure!(!self.token.lock().unwrap().is_empty(), "接続されていません");
        if self.task.lock().unwrap().is_none() {
            self.start_gateway();
        }
        Ok(Value::Null)
    }
    fn start_gateway(self: &Arc<Self>) {
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
    pub async fn connect(self: &Arc<Self>, token: String, status: Option<&str>) -> Result<Value> {
        self.stop();
        *self.token.lock().unwrap() = token;
        if let Some(status) = status.filter(|s| valid_status(s)) {
            *self.status.lock().unwrap() = status.into();
        }
        let me = self.api(ApiCall::GetMe).await?;
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
        let mut req = self
            .http
            .request(method, format!("{API}{path}"))
            .header("Authorization", token);
        if let Some(body) = body {
            req = req.json(&body);
        }
        response(req.send().await?).await
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
        let part = reqwest::multipart::Part::bytes(bytes).file_name(filename.to_owned());
        let payload = upload_payload(channel, filename, content, reply_to)?;
        let form = reqwest::multipart::Form::new()
            .text("payload_json", payload.to_string())
            .part("files[0]", part);
        let token = self.token.lock().unwrap().clone();
        response(
            self.http
                .post(format!("{API}/channels/{channel}/messages"))
                .header("Authorization", token)
                .multipart(form)
                .send()
                .await?,
        )
        .await
    }
    async fn gateway(self: Arc<Self>, mut commands: tokio::sync::mpsc::UnboundedReceiver<Value>) {
        let mut session = GatewaySession::default();
        let mut delay = 1u64;
        loop {
            self.push(json!({"t":"CONNECTION","d":{"status":"接続中"}}));
            let result = self
                .connection(&mut session, &mut commands, &mut delay)
                .await;
            if let Err(e) = &result {
                if let Some(GatewayClosed(code)) = e.downcast_ref::<GatewayClosed>() {
                    if fatal_close(*code) {
                        if *code == 4004 {
                            self.push(json!({"t":"AUTH_EXPIRED","d":{}}));
                        }
                        self.push(json!({"t":"ERROR","d":{"message":if *code==4004 {"認証に失敗しました。ログインし直してください。".to_string()} else {format!("接続が拒否されました（{code}）")}}}));
                        self.push(json!({"t":"CONNECTION","d":{"status":"未接続"}}));
                        break;
                    }
                }
            }
            self.push(json!({"t":"CONNECTION","d":{"status":"再接続待ち"}}));
            tokio::time::sleep(Duration::from_secs(delay)).await;
            delay = (delay * 2).min(30);
        }
    }
    async fn connection(
        &self,
        session: &mut GatewaySession,
        commands: &mut tokio::sync::mpsc::UnboundedReceiver<Value>,
        delay: &mut u64,
    ) -> Result<()> {
        let base = session
            .resume_url
            .as_deref()
            .filter(|_| !session.id.is_empty())
            .unwrap_or(GATEWAY)
            .trim_end_matches('/')
            .to_owned();
        let (mut ws, _) =
            tokio_tungstenite::connect_async(format!("{base}{GATEWAY_QUERY}")).await?;
        let mut inflater = Inflater::new();
        let mut hb = tokio::time::interval_at(
            tokio::time::Instant::now() + Duration::from_secs(40),
            Duration::from_secs(40),
        );
        let mut ack = true;
        // READY / RESUMED までは送らずに溜めておく（認証前に送ると 4003 で切られる）
        let mut ready = false;
        loop {
            tokio::select! {
                command = commands.recv(), if ready => {
                    let Some(command)=command else { bail!("終了しました") };
                    ws.send(Message::Text(command.to_string().into())).await?;
                },
                _ = hb.tick() => {
                    if !ack { bail!("heartbeat timeout"); }
                    ws.send(Message::Text(json!({"op":1,"d":session.seq}).to_string().into())).await?;
                    ack = false;
                },
                frame = ws.next() => {
                    let Some(frame) = frame else { bail!("切断されました") };
                    let v: Value = match frame? {
                        Message::Close(close) => return Err(GatewayClosed(close.map(|c| u16::from(c.code)).unwrap_or(1000)).into()),
                        Message::Text(text) => serde_json::from_str(text.as_str())?,
                        Message::Binary(data) => match inflater.push(&data)? { Some(text) => serde_json::from_str(text)?, None => continue },
                        _ => continue,
                    };
                    if !v["s"].is_null() { session.seq = v["s"].clone(); }
                    match v["op"].as_u64() {
                        Some(10) => {
                            let ms = v["d"]["heartbeat_interval"].as_u64().unwrap_or(41250).max(1000);
                            hb = tokio::time::interval_at(tokio::time::Instant::now()+Duration::from_millis(ms), Duration::from_millis(ms));
                            let token = self.token.lock().unwrap().clone();
                            let status = self.status.lock().unwrap().clone();
                            let identify = if session.id.is_empty() {
                                json!({"op":2,"d":{"token":token,"properties":{"os":"Android","browser":"Discord Client","device":""},"compress":false,"presence":{"status":status,"activities":[],"since":0,"afk":false}}})
                            } else { json!({"op":6,"d":{"token":token,"session_id":session.id,"seq":session.seq}}) };
                            ws.send(Message::Text(identify.to_string().into())).await?;
                        },
                        Some(11) => ack = true,
                        Some(1) => { ws.send(Message::Text(json!({"op":1,"d":session.seq}).to_string().into())).await?; },
                        Some(7) => bail!("reconnect requested"),
                        Some(9) => { if v["d"] != true { *session = GatewaySession::default(); } bail!("invalid session"); },
                        Some(0) => {
                            let t = v["t"].as_str().unwrap_or_default();
                            if t == "READY" {
                                session.id = v["d"]["session_id"].as_str().unwrap_or_default().into();
                                session.resume_url = v["d"]["resume_gateway_url"].as_str().map(str::to_owned);
                            }
                            if t == "READY" || t == "RESUMED" {
                                *delay = 1;
                                ready = true;
                                self.push(json!({"t":"CONNECTION","d":{"status":"接続済み"}}));
                            }
                            #[cfg(target_os = "android")]
                            match t {
                                "VOICE_STATE_UPDATE"=>{let _=self.voice(crate::mobile_voice::Command::State(v["d"].clone()));},
                                "VOICE_SERVER_UPDATE"=>{let _=self.voice(crate::mobile_voice::Command::Server(v["d"].clone()));},
                                _=>{}
                            }
                            if let Some(event) = slim_event(v) { self.push(event); }
                        },
                        _ => {},
                    }
                }
            }
        }
    }
}
/// 画面へ渡す形 {t, d} にする。使わない種類は None
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

fn slim_event(mut v: Value) -> Option<Value> {
    let t = v["t"].as_str()?.to_owned();
    let d = v.get_mut("d").map(Value::take).unwrap_or(Value::Null);
    if t == "READY" {
        let settings = &d["user_guild_settings"];
        let entries = settings["entries"]
            .as_array()
            .or(settings.as_array())
            .cloned()
            .unwrap_or_default();
        return Some(json!({"t":"READY","d":{"guild_settings":entries}}));
    }
    FORWARD.contains(&t.as_str()).then(|| json!({"t":t,"d":d}))
}
async fn response(r: reqwest::Response) -> Result<Value> {
    let status = r.status();
    let bytes = r.bytes().await?;
    let v: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    if !status.is_success() {
        match status.as_u16() {
            401 => return Err(AuthExpired.into()),
            413 => bail!("ファイルが大きすぎます（10MBまで）"),
            429 => bail!(
                "混雑しています。{}秒後に再試行してください。",
                v["retry_after"]
                    .as_f64()
                    .map(|s| s.ceil() as u64)
                    .unwrap_or(5)
            ),
            code => bail!(
                "Discord HTTP {code}: {}",
                v["message"].as_str().unwrap_or("操作に失敗しました")
            ),
        }
    }
    Ok(v)
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
        let ready = json!({"op":0,"s":1,"t":"READY","d":{"session_id":"s","guilds":[{"id":"1"}],"user_guild_settings":{"entries":[{"guild_id":"1","muted":true}]}}});
        assert_eq!(
            slim_event(ready),
            Some(json!({"t":"READY","d":{"guild_settings":[{"guild_id":"1","muted":true}]}}))
        );
        assert_eq!(
            slim_event(json!({"op":0,"t":"PRESENCE_UPDATE","d":{}})),
            None
        );
        assert_eq!(
            slim_event(json!({"op":0,"s":3,"t":"MESSAGE_CREATE","d":{"id":"5"}})),
            Some(json!({"t":"MESSAGE_CREATE","d":{"id":"5"}}))
        );
        assert!(fatal_close(4004) && fatal_close(4014) && !fatal_close(4000) && !fatal_close(1000));
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
