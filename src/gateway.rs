//! Gateway session and reconnect rules shared by desktop and Android.
use std::time::Duration;

use anyhow::{bail, Context, Result};
use futures_util::{SinkExt, StreamExt};
use tokio_tungstenite::{tungstenite::Message, MaybeTlsStream, WebSocketStream};

pub(crate) type Socket = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;

#[derive(Debug)]
pub(crate) struct GatewayClosed(pub u16);
impl std::fmt::Display for GatewayClosed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "gateway close {}", self.0)
    }
}
impl std::error::Error for GatewayClosed {}

/// Decode one complete gateway message while retaining the zlib stream context.
pub(crate) fn decode_frame_as<T: serde::de::DeserializeOwned>(
    frame: &Message,
    inflater: &mut crate::inflate::Inflater,
) -> Result<Option<T>> {
    let text = match frame {
        Message::Text(text) => text.as_str(),
        Message::Binary(bytes) => match inflater.push(bytes)? {
            Some(text) => text,
            None => return Ok(None),
        },
        _ => return Ok(None),
    };
    let parsed = serde_json::from_str(text).ok();
    if inflater.out.capacity() > crate::inflate::Inflater::KEEP {
        inflater.out = Vec::new();
    }
    Ok(parsed)
}

/// Establish the websocket and receive Hello with a shared decompression context.
pub(crate) async fn connect_and_hello(
    url: &str,
) -> Result<(Socket, crate::inflate::Inflater, Duration)> {
    let (mut socket, _) = tokio::time::timeout(
        Duration::from_secs(30),
        tokio_tungstenite::connect_async(url),
    )
    .await
    .context("Gateway connection timed out")??;
    let mut inflater = crate::inflate::Inflater::new();
    let period = tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            let frame = socket
                .next()
                .await
                .context("Gateway disconnected before Hello")??;
            if let Message::Close(close) = frame {
                return Err(GatewayClosed(close.map(|c| u16::from(c.code)).unwrap_or(1000)).into());
            }
            let Some(hello): Option<serde_json::Value> = decode_frame_as(&frame, &mut inflater)?
            else {
                continue;
            };
            if opcode(hello["op"].as_u64()) != Opcode::Hello {
                bail!("Unexpected Gateway message before Hello");
            }
            break heartbeat_period(hello["d"]["heartbeat_interval"].as_u64())
                .context("Invalid heartbeat interval");
        }
    })
    .await
    .context("Gateway Hello timed out")??;
    if inflater.out.capacity() > crate::inflate::Inflater::KEEP {
        inflater.out = Vec::new();
    }
    Ok((socket, inflater, period))
}

/// Gateway envelope. The payload stays unparsed until the consumer needs it.
#[derive(serde::Deserialize)]
pub(crate) struct GatewayFrame {
    #[serde(default)]
    pub op: serde_json::Value,
    #[serde(default)]
    pub s: serde_json::Value,
    #[serde(default)]
    pub t: serde_json::Value,
    pub d: Option<Box<serde_json::value::RawValue>>,
}

#[derive(serde::Deserialize)]
struct ReadySession {
    session_id: Option<String>,
    resume_gateway_url: Option<String>,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum SessionEnd {
    Resume,
    Reidentify,
    Closed(u16),
}

/// Drive outbound messages, heartbeats and gateway frames for either client.
pub(crate) async fn run_session(
    mut socket: Socket,
    mut inflater: crate::inflate::Inflater,
    period: Duration,
    session: &mut GatewaySession,
    commands: &mut tokio::sync::mpsc::UnboundedReceiver<serde_json::Value>,
    wait_until_ready: bool,
    mut dispatch: impl FnMut(&GatewayFrame),
) -> Result<SessionEnd> {
    let mut heartbeat = tokio::time::interval_at(tokio::time::Instant::now() + period, period);
    // スリープから戻ったときに、取りこぼした分をまとめて打たない（ACK を待たずに切断扱いになるため）
    heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut acknowledged = true;
    let mut ready = false;
    let mut commands_open = true;
    // この接続で送った回数（identify / resume の 1 回を含む）
    let mut limit = SendLimit::default();
    limit.record();
    loop {
        // 閉じるよう頼まれていて、送れるものを送り終えた（送れない状態なら待たない）
        if session.closing && (session.pending.is_empty() || !(ready || !wait_until_ready)) {
            let _ = socket.close(None).await;
            return Ok(SessionEnd::Closed(1000));
        }
        // 溜まったコマンドは READY / RESUMED のあとに、1 分あたりの上限を超えないように送る
        let send_at =
            ((ready || !wait_until_ready) && !session.pending.is_empty()).then(|| limit.next_at());
        tokio::select! {
            _ = tokio::time::sleep_until(send_at.unwrap_or_else(tokio::time::Instant::now)), if send_at.is_some() => {
                let Some((at, command)) = session.pending.pop_front() else { continue };
                // 古すぎる VC の状態は、その間に VC から外されていても入り直してしまうので捨てる
                if is_voice_state(&command) && at.elapsed() > UNSENT_TTL {
                    continue;
                }
                limit.record();
                if let Err(e) = socket.send(Message::Text(command.to_string().into())).await {
                    session.keep_unsent(at, command);
                    return Err(e.into());
                }
            }
            command = commands.recv(), if commands_open => {
                match command {
                    Some(command) => session.queue(command),
                    None if wait_until_ready => bail!("Gateway command channel closed"),
                    None => commands_open = false,
                }
            }
            _ = heartbeat.tick() => {
                if !acknowledged { bail!("Gateway heartbeat timed out"); }
                acknowledged = false;
                limit.record();
                socket.send(Message::Text(serde_json::json!({"op":1,"d":session.seq}).to_string().into())).await?;
            }
            frame = socket.next() => {
                let frame = frame.context("Gateway disconnected")??;
                if let Message::Close(close) = frame {
                    return Ok(SessionEnd::Closed(close.map(|c| u16::from(c.code)).unwrap_or(1000)));
                }
                let Some(frame): Option<GatewayFrame> = decode_frame_as(&frame, &mut inflater)? else { continue };
                if let Some(seq) = frame.s.as_i64() { session.seq = Some(seq); }
                match opcode(frame.op.as_u64()) {
                    Opcode::Dispatch => {
                        match frame.t.as_str() {
                            Some("READY") => {
                                if let Some(data) = frame.d.as_deref().and_then(|d| serde_json::from_str::<ReadySession>(d.get()).ok()) {
                                    session.id = data.session_id;
                                    // Discord 以外の接続先は信用しない（その場合は既定の接続先で再開する）
                                    session.resume_url = data.resume_gateway_url.filter(|u| is_discord_gateway_url(u));
                                }
                                ready = true;
                            }
                            Some("RESUMED") => ready = true,
                            _ => {}
                        }
                        dispatch(&frame);
                    }
                    Opcode::Heartbeat => socket.send(Message::Text(serde_json::json!({"op":1,"d":session.seq}).to_string().into())).await?,
                    Opcode::Reconnect => return Ok(SessionEnd::Resume),
                    Opcode::InvalidSession => return Ok(if frame.d.as_deref().is_some_and(|d| d.get().trim() == "true") { SessionEnd::Resume } else { SessionEnd::Reidentify }),
                    Opcode::HeartbeatAck => acknowledged = true,
                    _ => {}
                }
            }
        }
    }
}

/// 再開用に教わった接続先が Discord のゲートウェイか（wss:// で、ホストが discord.gg かその下）。
/// 違う所へトークンを送らないよう、それ以外は使わない
pub(crate) fn is_discord_gateway_url(url: &str) -> bool {
    let Some(rest) = url.strip_prefix("wss://") else {
        return false;
    };
    // 後ろに付く「?」以下は自分で付け直すので、パスより先は受け付けない
    if rest.contains(['?', '#', '@', '\\']) {
        return false;
    }
    let authority = rest.split('/').next().unwrap_or("");
    let host = match authority.rsplit_once(':') {
        Some((host, port)) if !port.is_empty() && port.bytes().all(|b| b.is_ascii_digit()) => host,
        Some(_) => return false,
        None => authority,
    };
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    host == "discord.gg"
        || host
            .strip_suffix(".discord.gg")
            .is_some_and(|sub| !sub.is_empty() && sub.split('.').all(|l| !l.is_empty()))
}

#[derive(Default)]
pub(crate) struct GatewaySession {
    pub id: Option<String>,
    pub seq: Option<i64>,
    pub resume_url: Option<String>,
    /// まだ送っていないコマンドと、受け取った時刻。READY / RESUMED のあとに古い順に送る。
    /// 送るのに失敗したものは VC の状態（op 4）だけ戻す（op 3 は identify に今の状態が入り、
    /// 購読は READY のあとに送り直すため）
    pub pending: std::collections::VecDeque<(tokio::time::Instant, serde_json::Value)>,
    /// close_command を受け取った（ログアウト）。つなぎ直さない
    pub closing: bool,
}

/// 溜めておいた op 4 を送ってよい時間。これより古いと、その間に VC から外されていても入り直してしまう
const UNSENT_TTL: Duration = Duration::from_secs(30);

/// 1 接続で 60 秒あたりに送ってよい回数。Discord の上限は 120 で、超えると 4008 で切られる。
/// ハートビートはこの数に入れたうえで止めないので、少し余らせておく
const SEND_LIMIT: usize = 110;
const SEND_WINDOW: Duration = Duration::from_secs(60);

/// 直近 60 秒に送った時刻
#[derive(Default)]
struct SendLimit(std::collections::VecDeque<tokio::time::Instant>);

impl SendLimit {
    fn record(&mut self) {
        let now = tokio::time::Instant::now();
        while self
            .0
            .front()
            .is_some_and(|t| now.duration_since(*t) >= SEND_WINDOW)
        {
            self.0.pop_front();
        }
        self.0.push_back(now);
    }
    /// 次のコマンドを送ってよい時刻
    fn next_at(&self) -> tokio::time::Instant {
        let now = tokio::time::Instant::now();
        let recent: Vec<_> = self
            .0
            .iter()
            .filter(|t| now.duration_since(**t) < SEND_WINDOW)
            .collect();
        match recent.len().checked_sub(SEND_LIMIT) {
            // 上限まで送った: 超えた分の最初の 1 回が窓から外れるまで待つ
            Some(over) => *recent[over] + SEND_WINDOW,
            None => now,
        }
    }
}

fn is_voice_state(command: &serde_json::Value) -> bool {
    command["op"].as_u64() == Some(4)
}

/// ゲートウェイを閉じてほしいときにコマンドの代わりに送るもの（Discord には送らない）。
/// 溜まっている分（VC から抜ける op 4 など）を送れる状態なら送ってから閉じる
pub(crate) fn close_command() -> serde_json::Value {
    serde_json::json!({ "riscord": "close" })
}

fn is_close_command(command: &serde_json::Value) -> bool {
    command["riscord"] == "close"
}

/// op 37 の購読先のサーバー
fn subscribed_guilds(
    command: &serde_json::Value,
) -> Option<&serde_json::Map<String, serde_json::Value>> {
    (command["op"].as_u64() == Some(37))
        .then(|| command["d"]["subscriptions"].as_object())
        .flatten()
}

impl GatewaySession {
    /// 送れなかったコマンドを、つなぎ直したあとに送れるよう先頭へ戻す。
    /// VC の状態・自分の見え方（presence）・購読も持ち越す。ただし同じ意味の新しいものが
    /// もう溜まっていれば、古い方は戻さない（queue と同じ考え方）
    fn keep_unsent(&mut self, at: tokio::time::Instant, mut command: serde_json::Value) {
        let op = command["op"].as_u64();
        if matches!(op, Some(3 | 4)) && self.pending.iter().any(|(_, c)| c["op"].as_u64() == op) {
            return;
        }
        if subscribed_guilds(&command).is_some() {
            let newer: std::collections::HashSet<String> = self
                .pending
                .iter()
                .filter_map(|(_, c)| subscribed_guilds(c))
                .flat_map(|g| g.keys().cloned())
                .collect();
            if let Some(subs) = command["d"]["subscriptions"].as_object_mut() {
                subs.retain(|g, _| !newer.contains(g));
                if subs.is_empty() {
                    return;
                }
            }
        }
        self.pending.push_front((at, command));
    }

    /// 送るコマンドを溜める。後から来たものと同じ意味の古いものは捨てる:
    /// VC の状態（op 4）と自分の見え方（op 3）は最新の 1 件、購読（op 37）はサーバーごとに最新のもの
    pub fn queue(&mut self, command: serde_json::Value) {
        if is_close_command(&command) {
            self.closing = true;
            return;
        }
        let op = command["op"].as_u64();
        if matches!(op, Some(3 | 4)) {
            self.pending.retain(|(_, c)| c["op"].as_u64() != op);
        }
        if let Some(guilds) = subscribed_guilds(&command) {
            let guilds: Vec<String> = guilds.keys().cloned().collect();
            self.pending.retain_mut(|(_, c)| {
                if c["op"] != 37 {
                    return true;
                }
                let Some(subs) = c["d"]["subscriptions"].as_object_mut() else {
                    return true;
                };
                for g in &guilds {
                    subs.remove(g);
                }
                !subs.is_empty()
            });
        }
        self.pending
            .push_back((tokio::time::Instant::now(), command));
    }

    /// fut が終わるまで、届いたコマンドを溜めながら待つ（つなぎ直しを待つ間・接続中も、受け取った時刻を残す）。
    /// 閉じるよう頼まれたら待つのをやめて None
    pub async fn queue_while<F: std::future::Future>(
        &mut self,
        commands: &mut tokio::sync::mpsc::UnboundedReceiver<serde_json::Value>,
        fut: F,
    ) -> Option<F::Output> {
        tokio::pin!(fut);
        let mut open = true;
        loop {
            if self.closing {
                return None;
            }
            tokio::select! {
                out = &mut fut => return Some(out),
                command = commands.recv(), if open => match command {
                    Some(command) => self.queue(command),
                    None => open = false,
                },
            }
        }
    }

    /// 一からログインし直すときの状態。セッションは捨てるが、送れなかったコマンドは残す
    pub fn reset_keeping_unsent(&mut self) {
        let pending = std::mem::take(&mut self.pending);
        let closing = self.closing;
        *self = GatewaySession {
            pending,
            closing,
            ..Default::default()
        };
    }

    pub fn resumable(&self) -> Option<(&str, i64)> {
        Some((self.id.as_deref().filter(|id| !id.is_empty())?, self.seq?))
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum CloseAction {
    Resume,
    Reidentify,
    Fatal,
}

pub(crate) fn close_action(code: u16) -> CloseAction {
    match code {
        4004 | 4010..=4014 => CloseAction::Fatal,
        4007 | 4009 => CloseAction::Reidentify,
        _ => CloseAction::Resume,
    }
}

/// Protocol opcodes interpreted identically by both clients.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Opcode {
    Dispatch,
    Heartbeat,
    Reconnect,
    InvalidSession,
    Hello,
    HeartbeatAck,
    Other,
}

pub(crate) fn opcode(value: Option<u64>) -> Opcode {
    match value {
        Some(0) => Opcode::Dispatch,
        Some(1) => Opcode::Heartbeat,
        Some(7) => Opcode::Reconnect,
        Some(9) => Opcode::InvalidSession,
        Some(10) => Opcode::Hello,
        Some(11) => Opcode::HeartbeatAck,
        _ => Opcode::Other,
    }
}

pub(crate) fn heartbeat_period(ms: Option<u64>) -> Option<Duration> {
    let ms = ms.unwrap_or(41_250);
    (1..=300_000)
        .contains(&ms)
        .then(|| Duration::from_millis(ms))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn only_the_latest_voice_state_is_carried_over() {
        let mut s = GatewaySession {
            id: Some("x".into()),
            seq: Some(5),
            ..Default::default()
        };
        let now = tokio::time::Instant::now();
        s.queue(serde_json::json!({"op": 4, "n": 1}));
        s.queue(serde_json::json!({"op": 4, "n": 2}));
        s.keep_unsent(now, serde_json::json!({"op": 4, "n": 0}));
        s.reset_keeping_unsent();
        assert!(s.resumable().is_none());
        assert_eq!(s.pending.len(), 1);
        assert_eq!(s.pending[0].1["n"], 2, "最新の 1 件だけ");
    }

    /// 送れなかった presence と購読も持ち越す。新しいものが溜まっていればそちらを優先する
    #[tokio::test]
    async fn unsent_presence_and_subscriptions_are_carried_over() {
        let mut s = GatewaySession::default();
        let now = tokio::time::Instant::now();
        s.keep_unsent(now, serde_json::json!({"op": 3, "d": {"status": "idle"}}));
        assert_eq!(s.pending.len(), 1);
        s.queue(serde_json::json!({"op": 3, "d": {"status": "dnd"}}));
        s.keep_unsent(now, serde_json::json!({"op": 3, "d": {"status": "online"}}));
        let statuses: Vec<_> = s.pending.iter().map(|(_, c)| c["d"]["status"].clone()).collect();
        assert_eq!(statuses, ["dnd"], "新しい presence があれば古いものは戻さない");

        let mut s = GatewaySession::default();
        s.queue(serde_json::json!({"op": 37, "d": {"subscriptions": {"a": {"n": 2}}}}));
        s.keep_unsent(
            now,
            serde_json::json!({"op": 37, "d": {"subscriptions": {"a": {"n": 1}, "b": {"n": 1}}}}),
        );
        assert_eq!(s.pending.len(), 2);
        let kept = &s.pending[0].1["d"]["subscriptions"];
        assert!(kept.get("a").is_none(), "新しい購読があるサーバーは戻さない");
        assert_eq!(kept["b"]["n"], 1);
    }

    #[tokio::test]
    async fn queued_commands_are_coalesced() {
        let mut s = GatewaySession::default();
        let sub = |g: &str, n: u32| {
            let mut subs = serde_json::Map::new();
            subs.insert(g.into(), serde_json::json!({ "n": n }));
            serde_json::json!({ "op": 37, "d": { "subscriptions": subs } })
        };
        s.queue(sub("a", 1));
        s.queue(serde_json::json!({ "op": 3, "d": { "status": "idle" } }));
        s.queue(sub("b", 1));
        s.queue(serde_json::json!({ "op": 14, "d": {} }));
        s.queue(sub("a", 2));
        s.queue(serde_json::json!({ "op": 3, "d": { "status": "dnd" } }));
        let left: Vec<_> = s.pending.iter().map(|(_, c)| c.clone()).collect();
        assert_eq!(
            left,
            [
                sub("b", 1),
                serde_json::json!({ "op": 14, "d": {} }),
                sub("a", 2),
                serde_json::json!({ "op": 3, "d": { "status": "dnd" } }),
            ]
        );
        assert!(!s.closing);
        s.queue(close_command());
        assert!(
            s.closing,
            "閉じる合図は Discord へ送るコマンドとして積まない"
        );
        assert_eq!(s.pending.len(), 4);
        s.reset_keeping_unsent();
        assert!(s.closing, "一からつなぎ直すときも閉じる合図は残す");
    }

    #[tokio::test(start_paused = true)]
    async fn send_limit_waits_for_the_window() {
        let mut limit = SendLimit::default();
        let start = tokio::time::Instant::now();
        for _ in 0..SEND_LIMIT {
            assert_eq!(limit.next_at(), tokio::time::Instant::now());
            limit.record();
            tokio::time::advance(Duration::from_millis(10)).await;
        }
        assert_eq!(
            limit.next_at(),
            start + SEND_WINDOW,
            "最初の 1 回が外れるまで待つ"
        );
        tokio::time::advance(SEND_WINDOW).await;
        assert_eq!(limit.next_at(), tokio::time::Instant::now());
    }

    #[tokio::test]
    async fn queue_while_keeps_commands_and_stops_on_close() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let mut s = GatewaySession::default();
        tx.send(serde_json::json!({ "op": 4, "d": {} })).unwrap();
        let waited = s
            .queue_while(&mut rx, tokio::time::sleep(Duration::from_millis(20)))
            .await;
        assert!(waited.is_some());
        assert_eq!(
            s.pending.len(),
            1,
            "待っている間に届いたものを時刻つきで溜める"
        );
        tx.send(close_command()).unwrap();
        let waited = tokio::time::timeout(
            Duration::from_secs(5),
            s.queue_while(&mut rx, std::future::pending::<()>()),
        )
        .await
        .expect("閉じる合図で待つのをやめる");
        assert!(waited.is_none());
    }

    #[tokio::test]
    async fn close_command_flushes_then_closes() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut ws = tokio_tungstenite::accept_async(stream).await.unwrap();
            ws.send(Message::Text(
                r#"{"op":10,"d":{"heartbeat_interval":45000}}"#.into(),
            ))
            .await
            .unwrap();
            ws.send(Message::Text(
                r#"{"op":0,"s":1,"t":"READY","d":{"session_id":"abc"}}"#.into(),
            ))
            .await
            .unwrap();
            let sent = ws.next().await.unwrap().unwrap();
            assert_eq!(
                serde_json::from_str::<serde_json::Value>(sent.to_text().unwrap()).unwrap()["op"],
                4,
                "閉じる前に溜まっていた op 4 を送る"
            );
            assert!(matches!(ws.next().await, Some(Ok(Message::Close(_)))));
        });
        let (ws, inflater, period) = connect_and_hello(&format!("ws://{addr}")).await.unwrap();
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        tx.send(serde_json::json!({"op": 4, "d": {"channel_id": null}}))
            .unwrap();
        let mut session = GatewaySession::default();
        // READY のあとに閉じるよう頼む（READY の前なら送れないので待たずに閉じる）
        let end = tokio::time::timeout(
            Duration::from_secs(5),
            run_session(ws, inflater, period, &mut session, &mut rx, true, |_| {
                tx.send(close_command()).unwrap();
            }),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(end, SessionEnd::Closed(1000));
        assert!(session.closing);
        server.await.unwrap();
    }

    #[tokio::test]
    async fn shared_handshake_reads_hello_and_preserves_close_code() {
        use futures_util::SinkExt;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut ws = tokio_tungstenite::accept_async(stream).await.unwrap();
            ws.send(Message::Text(
                r#"{"op":10,"d":{"heartbeat_interval":45000}}"#.into(),
            ))
            .await
            .unwrap();
        });
        let (_, _, period) = connect_and_hello(&format!("ws://{addr}")).await.unwrap();
        assert_eq!(period, Duration::from_millis(45_000));
        server.await.unwrap();

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut ws = tokio_tungstenite::accept_async(stream).await.unwrap();
            ws.send(Message::Close(Some(
                tokio_tungstenite::tungstenite::protocol::CloseFrame {
                    code: 4004.into(),
                    reason: "invalid token".into(),
                },
            )))
            .await
            .unwrap();
        });
        let error = match connect_and_hello(&format!("ws://{addr}")).await {
            Ok(_) => panic!("expected close"),
            Err(error) => error,
        };
        assert_eq!(
            error.downcast_ref::<GatewayClosed>().map(|e| e.0),
            Some(4004)
        );
        server.await.unwrap();
    }
    #[tokio::test]
    async fn shared_loop_waits_for_ready_then_sends_commands() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut ws = tokio_tungstenite::accept_async(stream).await.unwrap();
            ws.send(Message::Text(
                r#"{"op":10,"d":{"heartbeat_interval":45000}}"#.into(),
            ))
            .await
            .unwrap();
            assert!(tokio::time::timeout(Duration::from_millis(50), ws.next())
                .await
                .is_err());
            ws.send(Message::Text(r#"{"op":0,"s":5,"t":"READY","d":{"session_id":"abc","resume_gateway_url":"wss://example.invalid"}}"#.into())).await.unwrap();
            let sent = ws.next().await.unwrap().unwrap();
            assert_eq!(
                serde_json::from_str::<serde_json::Value>(sent.to_text().unwrap()).unwrap()["op"],
                42
            );
            ws.send(Message::Text(r#"{"op":9,"d":false}"#.into()))
                .await
                .unwrap();
        });
        let (ws, inflater, period) = connect_and_hello(&format!("ws://{addr}")).await.unwrap();
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        tx.send(serde_json::json!({"op":42})).unwrap();
        let mut session = GatewaySession::default();
        let mut seen = Vec::new();
        let end = run_session(ws, inflater, period, &mut session, &mut rx, true, |frame| {
            seen.push(frame.t.as_str().unwrap_or_default().to_owned());
        })
        .await
        .unwrap();
        assert_eq!(end, SessionEnd::Reidentify);
        assert_eq!(session.resumable(), Some(("abc", 5)));
        assert_eq!(seen, ["READY"]);
        server.await.unwrap();
    }

    #[tokio::test]
    async fn shared_loop_heartbeats_and_handles_reconnect() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut ws = tokio_tungstenite::accept_async(stream).await.unwrap();
            ws.send(Message::Text(
                r#"{"op":10,"d":{"heartbeat_interval":20}}"#.into(),
            ))
            .await
            .unwrap();
            let heartbeat = ws.next().await.unwrap().unwrap();
            let heartbeat: serde_json::Value =
                serde_json::from_str(heartbeat.to_text().unwrap()).unwrap();
            assert_eq!(heartbeat["op"], 1);
            assert_eq!(heartbeat["d"], 7);
            ws.send(Message::Text(r#"{"op":11,"d":null}"#.into()))
                .await
                .unwrap();
            ws.send(Message::Text(r#"{"op":7,"d":null}"#.into()))
                .await
                .unwrap();
        });
        let (ws, inflater, period) = connect_and_hello(&format!("ws://{addr}")).await.unwrap();
        let (_tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let mut session = GatewaySession {
            seq: Some(7),
            ..Default::default()
        };
        assert_eq!(
            run_session(ws, inflater, period, &mut session, &mut rx, false, |_| {})
                .await
                .unwrap(),
            SessionEnd::Resume
        );
        server.await.unwrap();
    }

    #[test]
    fn resume_requires_id_and_sequence() {
        let mut s = GatewaySession::default();
        s.id = Some("session".into());
        assert!(s.resumable().is_none());
        s.seq = Some(42);
        assert_eq!(s.resumable(), Some(("session", 42)));
        s.id = Some(String::new());
        assert!(s.resumable().is_none());
    }
    #[test]
    fn accepts_only_discord_resume_urls() {
        assert!(is_discord_gateway_url(
            "wss://gateway-us-east1-b.discord.gg"
        ));
        assert!(is_discord_gateway_url("wss://gateway.discord.gg/"));
        assert!(is_discord_gateway_url("wss://discord.gg:443"));
        assert!(!is_discord_gateway_url("ws://gateway.discord.gg"));
        assert!(!is_discord_gateway_url("wss://evil.example"));
        assert!(!is_discord_gateway_url("wss://evildiscord.gg"));
        assert!(!is_discord_gateway_url("wss://discord.gg.evil.example"));
        assert!(!is_discord_gateway_url(
            "wss://user@evil.example/.discord.gg"
        ));
        assert!(!is_discord_gateway_url("wss://gateway.discord.gg?x=1"));
        assert!(!is_discord_gateway_url("wss://.discord.gg"));
    }
    #[test]
    fn classifies_gateway_opcodes() {
        assert_eq!(opcode(Some(0)), Opcode::Dispatch);
        assert_eq!(opcode(Some(1)), Opcode::Heartbeat);
        assert_eq!(opcode(Some(7)), Opcode::Reconnect);
        assert_eq!(opcode(Some(9)), Opcode::InvalidSession);
        assert_eq!(opcode(Some(10)), Opcode::Hello);
        assert_eq!(opcode(Some(11)), Opcode::HeartbeatAck);
        assert_eq!(opcode(None), Opcode::Other);
    }
    #[test]
    fn classifies_close_and_rejects_invalid_heartbeat() {
        assert_eq!(close_action(4004), CloseAction::Fatal);
        assert_eq!(close_action(4007), CloseAction::Reidentify);
        assert_eq!(close_action(4000), CloseAction::Resume);
        assert!(heartbeat_period(Some(0)).is_none());
        assert!(heartbeat_period(Some(u64::MAX)).is_none());
        assert_eq!(heartbeat_period(None), Some(Duration::from_millis(41_250)));
    }
}
