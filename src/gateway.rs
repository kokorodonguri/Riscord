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
    loop {
        tokio::select! {
            // 前のセッションで送れなかった VC の状態（op 4）。切断中のイベントの再送が終わる
            // READY / RESUMED のあとに送る。古すぎるものは今の状態と食い違いうるので捨てる
            _ = std::future::ready(()), if ready && session.unsent.is_some() => {
                if let Some((at, command)) = session.unsent.take() {
                    if at.elapsed() <= UNSENT_TTL {
                        if let Err(e) = socket.send(Message::Text(command.to_string().into())).await {
                            session.unsent = Some((at, command));
                            return Err(e.into());
                        }
                    }
                }
            }
            command = commands.recv(), if commands_open && (!wait_until_ready || ready) => {
                if let Some(command) = command {
                    // 新しい VC の状態を送るなら、持ち越していた古い分は要らない（順番の入れ替わりを防ぐ）
                    if is_voice_state(&command) {
                        session.unsent = None;
                    }
                    if let Err(e) = socket.send(Message::Text(command.to_string().into())).await {
                        session.keep_unsent(command);
                        return Err(e.into());
                    }
                } else if wait_until_ready {
                    bail!("Gateway command channel closed");
                } else {
                    commands_open = false;
                }
            }
            _ = heartbeat.tick() => {
                if !acknowledged { bail!("Gateway heartbeat timed out"); }
                acknowledged = false;
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
    /// 切断で送れなかった VC の状態（op 4）と、送ろうとした時刻。次のセッションで READY のあとに送る。
    /// ほかのコマンドは持ち越さない（op 3 は identify に今の状態が入り、購読は READY のあとに送り直すため）
    pub unsent: Option<(std::time::Instant, serde_json::Value)>,
}

/// 持ち越した op 4 を送ってよい時間。これより古いと、その間に VC から外されていても入り直してしまう
const UNSENT_TTL: Duration = Duration::from_secs(30);

fn is_voice_state(command: &serde_json::Value) -> bool {
    command["op"].as_u64() == Some(4)
}

impl GatewaySession {
    fn keep_unsent(&mut self, command: serde_json::Value) {
        if is_voice_state(&command) {
            self.unsent = Some((std::time::Instant::now(), command));
        }
    }

    /// 一からログインし直すときの状態。セッションは捨てるが、送れなかったコマンドは残す
    pub fn reset_keeping_unsent(&mut self) {
        let unsent = std::mem::take(&mut self.unsent);
        *self = GatewaySession {
            unsent,
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
    #[test]
    fn only_the_latest_voice_state_is_carried_over() {
        let mut s = GatewaySession {
            id: Some("x".into()),
            seq: Some(5),
            ..Default::default()
        };
        s.keep_unsent(serde_json::json!({"op": 3, "d": {}}));
        assert!(s.unsent.is_none(), "op 4 以外は持ち越さない");
        s.keep_unsent(serde_json::json!({"op": 4, "n": 1}));
        s.keep_unsent(serde_json::json!({"op": 4, "n": 2}));
        s.reset_keeping_unsent();
        assert!(s.resumable().is_none());
        assert_eq!(s.unsent.as_ref().unwrap().1["n"], 2, "最新の 1 件だけ");
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
