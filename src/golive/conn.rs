//! 配信用のボイス接続（Go Live）。音声の songbird とは別に、映像を送受信する接続を自前で張る。
//! 手順: WS で Identify → Ready/Hello → UDP の IP 探索 → Select Protocol → Session Description → 送受信。
//! DAVE（エンドツーエンド暗号）は davey に任せ、MLS のやり取りだけここで中継する。

use std::collections::{HashMap, HashSet};
use std::net::IpAddr;
use std::num::NonZeroU16;
use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use davey::{Codec, DaveSession, MediaType, ProposalsOperationType};
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio::net::UdpSocket;
use tokio::sync::{mpsc, oneshot};
use tokio::time::{interval, sleep, Instant, MissedTickBehavior};
use tokio_tungstenite::tungstenite::Message as WsMsg;

use super::media::{Capture, Decoder, Encoded, Quality};
use super::rtp::{self, Cipher, Packetizer, Push};

/// 接続に必要な情報（STREAM_CREATE / STREAM_SERVER_UPDATE と、ボイスの session_id）
#[derive(Clone, Debug)]
pub struct Params {
    pub endpoint: String,
    pub token: String,
    pub rtc_server_id: String,
    pub session_id: String,
    pub user_id: u64,
}

pub enum Role {
    Broadcast {
        capture: Capture,
        frames: mpsc::Receiver<Encoded>,
        quality: Quality,
    },
    Watch {
        streamer: u64,
        decoder: Decoder,
    },
}

/// 接続の様子を UI に知らせる
pub enum Report {
    /// 映像を流し始めた / 受け取り始めた
    Live,
}

type Ws =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

struct Dave {
    session: Option<DaveSession>,
    version: u16,
    pending: HashMap<u16, u16>,
    users: HashSet<u64>,
    user_id: u64,
    channel_id: u64,
}

impl Dave {
    /// DAVE の鍵パッケージを作り直す（必要なら送るべきバイナリを返す）
    fn reinit(&mut self) -> Option<Vec<u8>> {
        match NonZeroU16::new(self.version) {
            Some(v) => {
                let r = match &mut self.session {
                    Some(s) => s
                        .reinit(v, self.user_id, self.channel_id, None)
                        .map_err(|e| format!("{e:?}")),
                    None => DaveSession::new(v, self.user_id, self.channel_id, None)
                        .map(|s| self.session = Some(s))
                        .map_err(|e| format!("{e:?}")),
                };
                if let Err(e) = r {
                    eprintln!("DAVE を初期化できません: {e}");
                    return None;
                }
                let kp = self.session.as_mut()?.create_key_package().ok()?;
                Some(binary(26, &kp))
            }
            None => {
                if let Some(s) = &mut self.session {
                    let _ = s.reset();
                    s.set_passthrough_mode(true, Some(10));
                }
                None
            }
        }
    }

    fn execute(&mut self, id: u16) {
        let Some(new) = self.pending.remove(&id) else {
            return;
        };
        let old = self.version;
        self.version = new;
        if id > 0 && old == 0 && new != 0 {
            if let Some(s) = &mut self.session {
                s.set_passthrough_mode(true, Some(10));
            }
        }
    }

    fn encrypt_ready(&self) -> bool {
        self.version != 0 && self.session.as_ref().is_some_and(|s| s.is_ready())
    }
}

fn binary(op: u8, data: &[u8]) -> Vec<u8> {
    let mut v = Vec::with_capacity(data.len() + 1);
    v.push(op);
    v.extend_from_slice(data);
    v
}

async fn send_json(ws: &mut Ws, op: u8, d: Value) -> Result<()> {
    ws.send(WsMsg::Text(json!({ "op": op, "d": d }).to_string().into()))
        .await?;
    Ok(())
}

fn codecs() -> Value {
    // 受け取れるのは H264 だけなので、映像は H264 だけを名乗る（相手も H264 で送ってくる）
    json!([
        { "name": "opus", "type": "audio", "priority": 1000, "payload_type": rtp::PT_OPUS },
        { "name": "H264", "type": "video", "priority": 1000, "payload_type": rtp::PT_H264, "rtx_payload_type": rtp::PT_H264_RTX, "encode": true, "decode": true },
    ])
}

/// 接続して、stop が来るまで送受信を続ける
pub async fn run(
    p: Params,
    mut role: Role,
    report: impl Fn(Report) + Send,
    mut stop: oneshot::Receiver<()>,
) -> Result<()> {
    let url = format!(
        "wss://{}/?v=8",
        p.endpoint
            .trim_start_matches("wss://")
            .trim_end_matches('/')
    );
    let (mut ws, _) = tokio::time::timeout(
        Duration::from_secs(10),
        tokio_tungstenite::connect_async(&url),
    )
    .await
    .context(tr!(
        "配信サーバーへの接続がタイムアウトしました",
        "Connection to the stream server timed out"
    ))??;
    send_json(
        &mut ws,
        0,
        json!({
            "server_id": p.rtc_server_id,
            "user_id": p.user_id.to_string(),
            "session_id": p.session_id,
            "token": p.token,
            "video": true,
            "streams": [{ "type": "screen", "rid": "100", "quality": 100 }],
            "max_dave_protocol_version": davey::DAVE_PROTOCOL_VERSION,
        }),
    )
    .await?;

    let dave_channel = p
        .rtc_server_id
        .parse::<u64>()
        .map_err(|_| anyhow!("{}", tr!("不正な rtc_server_id", "Invalid rtc_server_id",)))?
        .saturating_sub(1);
    let mut dave = Dave {
        session: None,
        version: 0,
        pending: HashMap::new(),
        users: HashSet::from([p.user_id]),
        user_id: p.user_id,
        channel_id: dave_channel,
    };
    let mut seq_ack: i64 = -1;
    let mut early: Vec<Value> = Vec::new();

    // ---- Hello と Ready を待つ ----
    let mut hello: Option<f64> = None;
    let mut ready: Option<Value> = None;
    while hello.is_none() || ready.is_none() {
        let msg = tokio::time::timeout(Duration::from_secs(10), ws.next())
            .await
            .context(tr!(
                "配信サーバーの応答がありません",
                "No response from the stream server"
            ))?
            .ok_or_else(|| {
                anyhow!(
                    "{}",
                    tr!(
                        "配信サーバーが切断しました",
                        "The stream server disconnected",
                    )
                )
            })??;
        match msg {
            WsMsg::Text(t) => {
                let v: Value = serde_json::from_str(&t)?;
                if let Some(s) = v["seq"].as_i64() {
                    seq_ack = s;
                }
                match v["op"].as_u64() {
                    Some(8) => hello = v["d"]["heartbeat_interval"].as_f64(),
                    Some(2) => ready = Some(v["d"].clone()),
                    _ => early.push(v),
                }
            }
            WsMsg::Close(f) => bail!(
                "{}",
                tr!(
                    "配信サーバーに断られました: {f:?}",
                    "Rejected by the stream server: {f:?}",
                )
            ),
            _ => {}
        }
    }
    let ready = ready.unwrap();
    debug(format_args!("Ready: {}", ready));
    let hb_every = Duration::from_secs_f64(hello.unwrap().max(1000.0) / 1000.0);
    let ssrc = ready["ssrc"]
        .as_u64()
        .ok_or_else(|| anyhow!("{}", tr!("ssrc がありません", "Missing ssrc",)))?
        as u32;
    let stream = &ready["streams"][0];
    let video_ssrc = stream["ssrc"]
        .as_u64()
        .map(|v| v as u32)
        .unwrap_or(ssrc + 1);
    let rtx_ssrc = stream["rtx_ssrc"]
        .as_u64()
        .map(|v| v as u32)
        .unwrap_or(ssrc + 2);
    let ip: IpAddr = ready["ip"]
        .as_str()
        .ok_or_else(|| anyhow!("{}", tr!("ip がありません", "Missing ip",)))?
        .parse()?;
    let port = ready["port"]
        .as_u64()
        .ok_or_else(|| anyhow!("{}", tr!("port がありません", "Missing port",)))?
        as u16;
    let modes: Vec<String> = ready["modes"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|m| m.as_str().map(String::from))
        .collect();
    let mode = rtp::choose_mode(&modes).ok_or_else(|| {
        anyhow!(
            "{}",
            tr!(
                "対応している暗号方式がありません: {modes:?}",
                "No supported encryption mode: {modes:?}",
            )
        )
    })?;

    // ---- UDP と IP 探索 ----
    let udp = UdpSocket::bind(if ip.is_ipv4() { "0.0.0.0:0" } else { "[::]:0" }).await?;
    udp.connect((ip, port)).await?;
    let mut disc = [0u8; 74];
    disc[0..2].copy_from_slice(&1u16.to_be_bytes());
    disc[2..4].copy_from_slice(&70u16.to_be_bytes());
    disc[4..8].copy_from_slice(&ssrc.to_be_bytes());
    let mut buf = vec![0u8; 2048];
    let (my_ip, my_port) = 'found: {
        for _ in 0..5 {
            udp.send(&disc).await?;
            if let Ok(Ok(n)) =
                tokio::time::timeout(Duration::from_secs(1), udp.recv(&mut buf)).await
            {
                if n >= 74 && u16::from_be_bytes([buf[0], buf[1]]) == 2 {
                    let raw = &buf[8..72];
                    let end = raw.iter().position(|&b| b == 0).unwrap_or(raw.len());
                    let addr = std::str::from_utf8(&raw[..end])?.to_string();
                    break 'found (addr, u16::from_be_bytes([buf[72], buf[73]]));
                }
            }
        }
        bail!(
            "{}",
            tr!(
                "配信サーバーと UDP でつながりません",
                "Can't reach the stream server over UDP",
            )
        );
    };
    send_json(&mut ws, 1, json!({ "protocol": "udp", "data": { "address": my_ip, "port": my_port, "mode": mode }, "codecs": codecs() })).await?;

    // ---- Session Description を待つ ----
    let cipher = loop {
        let msg = tokio::time::timeout(Duration::from_secs(10), ws.next())
            .await
            .context(tr!(
                "暗号鍵が届きません",
                "The encryption key didn't arrive"
            ))?
            .ok_or_else(|| {
                anyhow!(
                    "{}",
                    tr!(
                        "配信サーバーが切断しました",
                        "The stream server disconnected",
                    )
                )
            })??;
        match msg {
            WsMsg::Text(t) => {
                let v: Value = serde_json::from_str(&t)?;
                if let Some(s) = v["seq"].as_i64() {
                    seq_ack = s;
                }
                if v["op"].as_u64() == Some(4) {
                    let d = &v["d"];
                    debug(format_args!(
                        "Session Description: mode={} video_codec={} dave={}",
                        d["mode"], d["video_codec"], d["dave_protocol_version"]
                    ));
                    let key: Vec<u8> = d["secret_key"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(|b| b.as_u64().map(|b| b as u8))
                        .collect();
                    let m = d["mode"].as_str().unwrap_or(mode);
                    if let Some(c) = d["video_codec"].as_str() {
                        if !c.eq_ignore_ascii_case("H264") {
                            eprintln!("配信の映像コーデックが H264 ではありません: {c}");
                        }
                    }
                    dave.version = d["dave_protocol_version"].as_u64().unwrap_or(0) as u16;
                    if let Some(kp) = dave.reinit() {
                        ws.send(WsMsg::Binary(kp.into())).await?;
                    }
                    break Cipher::new(m, &key).ok_or_else(|| {
                        anyhow!("{}", tr!("暗号鍵が不正です", "Invalid encryption key",))
                    })?;
                }
                early.push(v);
            }
            WsMsg::Binary(b) => handle_binary(&mut ws, &mut dave, &b, &mut seq_ack).await?,
            WsMsg::Close(f) => bail!(
                "{}",
                tr!(
                    "配信サーバーに断られました: {f:?}",
                    "Rejected by the stream server: {f:?}",
                )
            ),
            _ => {}
        }
    };

    // ---- 送受信の準備 ----
    let mut packetizer = Packetizer::new(video_ssrc);
    let mut rtcp_out = Packetizer::new(ssrc);
    let mut depack = rtp::Depacketizer::default();
    let mut streamer_video: Option<u32> = None;
    let mut got_key = false;
    let mut last_pli = Instant::now() - Duration::from_secs(10);
    let mut live = false;
    let mut stats = WatchStats::default();
    match &mut role {
        Role::Broadcast {
            quality: q,
            capture,
            ..
        } => {
            send_json(
                &mut ws,
                12,
                json!({
                    "audio_ssrc": ssrc, "video_ssrc": video_ssrc, "rtx_ssrc": rtx_ssrc,
                    "streams": [{
                        "type": "video", "rid": "100", "ssrc": video_ssrc, "active": true, "quality": 100,
                        "rtx_ssrc": rtx_ssrc, "max_bitrate": q.bitrate, "max_framerate": q.fps,
                        "max_resolution": { "type": "fixed", "width": q.width, "height": q.height },
                    }],
                }),
            )
            .await?;
            send_json(
                &mut ws,
                5,
                json!({ "speaking": 2, "delay": 0, "ssrc": ssrc }),
            )
            .await?;
            capture.force_keyframe();
            // 視聴者がいない間は DAVE の鍵がそろわず何も送らないが、配信としては始まっている
            live = true;
            report(Report::Live);
        }
        Role::Watch { .. } => {
            send_json(
                &mut ws,
                12,
                json!({ "audio_ssrc": ssrc, "video_ssrc": 0, "rtx_ssrc": 0, "streams": [] }),
            )
            .await?;
            send_json(&mut ws, 15, json!({ "any": 100 })).await?;
        }
    }
    for v in early {
        handle_json(&mut ws, &mut dave, &v, &mut role, &mut streamer_video).await?;
    }

    let mut heartbeat = interval(hb_every);
    heartbeat.set_missed_tick_behavior(MissedTickBehavior::Delay);
    let mut tick = interval(Duration::from_millis(500));
    tick.set_missed_tick_behavior(MissedTickBehavior::Skip);
    let mut keepalive = Instant::now();
    let started = Instant::now();

    loop {
        tokio::select! {
            _ = &mut stop => break,
            _ = heartbeat.tick() => {
                let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_millis() as u64;
                send_json(&mut ws, 3, json!({ "t": t, "seq_ack": seq_ack })).await?;
            }
            _ = tick.tick() => {
                if keepalive.elapsed() > Duration::from_secs(5) {
                    keepalive = Instant::now();
                    let mut k = [0u8; 8];
                    k[..4].copy_from_slice(&ssrc.to_be_bytes());
                    let _ = udp.send(&k).await;
                }
                match &role {
                    Role::Broadcast { capture, .. } => {
                        if let Some(e) = capture.error() {
                            bail!(e);
                        }
                    }
                    Role::Watch { decoder, .. } => {
                        if let Some(e) = decoder.error() {
                            bail!(e);
                        }
                        // キーフレームが来るまで頼み続ける
                        if !got_key && last_pli.elapsed() > Duration::from_secs(1) {
                            if let Some(v) = streamer_video {
                                last_pli = Instant::now();
                                if let Some(p) = rtcp_out.seal_rtcp(&cipher, &rtp::pli(ssrc, v)) {
                                    let _ = udp.send(&p).await;
                                }
                            }
                        }
                        // 何が足りないのかを伝えて終える（「接続中」のまま待ち続けない）
                        if !live && started.elapsed() > Duration::from_secs(20) {
                            debug(format_args!("{}", tr!("視聴が始まらない: {stats:?} streamer_video={streamer_video:?} dave={}", "Viewing didn't start: {stats:?} streamer_video={streamer_video:?} dave={}", dave.version)));
                            bail!(stats.stall_reason(dave.version));
                        }
                    }
                }
            }
            msg = ws.next() => {
                let msg = match msg {
                    Some(Ok(m)) => m,
                    Some(Err(e)) => return Err(e.into()),
                    None => bail!("{}", tr!("配信サーバーが切断しました", "The stream server disconnected",)),
                };
                match msg {
                    WsMsg::Text(t) => {
                        let v: Value = serde_json::from_str(&t)?;
                        if let Some(s) = v["seq"].as_i64() {
                            seq_ack = s;
                        }
                        handle_json(&mut ws, &mut dave, &v, &mut role, &mut streamer_video).await?;
                    }
                    WsMsg::Binary(b) => handle_binary(&mut ws, &mut dave, &b, &mut seq_ack).await?,
                    WsMsg::Close(f) => {
                        // 4014 = 配信が終わった・追い出された
                        bail!("{}", tr!("配信が終了しました{}", "The stream ended{}", f.map(|f| tr!("（{}）", " ({})", f.code)).unwrap_or_default()));
                    }
                    _ => {}
                }
            }
            n = udp.recv(&mut buf) => {
                let Ok(n) = n else { continue };
                let pkt = &buf[..n];
                match &mut role {
                    Role::Broadcast { capture, .. } => {
                        if rtp::is_rtcp(pkt) && rtp::rtcp_wants_keyframe(&cipher, pkt) && last_pli.elapsed() > Duration::from_millis(500) {
                            last_pli = Instant::now();
                            capture.force_keyframe();
                        }
                    }
                    Role::Watch { streamer, decoder } => {
                        let Some(r) = rtp::parse_rtp(pkt) else { continue };
                        if r.pt != rtp::PT_H264 {
                            // 音声と再送は映像の代わりにならないので数えない
                            if r.pt != rtp::PT_OPUS && r.pt != rtp::PT_H264_RTX && !stats.other_pt.contains(&r.pt) && stats.other_pt.len() < 8 {
                                stats.other_pt.push(r.pt);
                            }
                            continue;
                        }
                        if streamer_video.is_some_and(|v| v != r.ssrc) {
                            continue;
                        }
                        streamer_video.get_or_insert(r.ssrc);
                        stats.rtp += 1;
                        let Some(payload) = r.decrypt(&cipher) else {
                            stats.transport_fail += 1;
                            continue;
                        };
                        match depack.push(r.seq, r.timestamp, r.marker, &payload) {
                            Push::Frame(au) => {
                                stats.frames += 1;
                                let data = if dave.version != 0 {
                                    match dave.session.as_mut().map(|s| s.decrypt(*streamer, MediaType::VIDEO, &au.data)) {
                                        Some(Ok(d)) => d,
                                        // 暗号化されていないフレーム（DAVE への切り替え中）はそのまま使う
                                        _ if !au.data.ends_with(&[0xFA, 0xFA]) => au.data,
                                        _ => {
                                            stats.dave_fail += 1;
                                            continue;
                                        }
                                    }
                                } else {
                                    au.data
                                };
                                if !got_key {
                                    if !au.keyframe {
                                        stats.no_key += 1;
                                        continue;
                                    }
                                    got_key = true;
                                }
                                decoder.push(data);
                                if !live {
                                    live = true;
                                    report(Report::Live);
                                }
                            }
                            Push::Lost => {
                                // 欠けたら次のキーフレームまで待つ（崩れた画を出さない）
                                got_key = false;
                            }
                            Push::None => {}
                        }
                    }
                }
            }
            e = recv_frame(&mut role) => {
                let Some(e) = e else { bail!("{}", tr!("画面の取り込みが止まりました", "Screen capture stopped",)) };
                let data = if dave.version != 0 {
                    if !dave.encrypt_ready() {
                        // まだ鍵がそろっていない（視聴者がいない間など）
                        continue;
                    }
                    match dave.session.as_mut().unwrap().encrypt(MediaType::VIDEO, Codec::H264, &e.data) {
                        Ok(d) => d.into_owned(),
                        Err(_) => continue,
                    }
                } else {
                    e.data
                };
                for p in packetizer.packetize(&cipher, &data, e.rtp_ts) {
                    let _ = udp.send(&p).await;
                }
                if !live {
                    live = true;
                    report(Report::Live);
                }
                // 大きなキーフレームのあとは少し間をあける（受け手のバッファあふれを防ぐ）
                if e.keyframe && data.len() > 100_000 {
                    sleep(Duration::from_millis(2)).await;
                }
            }
        }
    }
    let _ = ws.close(None).await;
    Ok(())
}

/// RISCORD_DEBUG_STREAM を付けて起動したときだけ、視聴・配信の様子を標準エラーに出す
pub(super) fn debug(args: std::fmt::Arguments<'_>) {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    if *ON.get_or_init(|| std::env::var_os("RISCORD_DEBUG_STREAM").is_some()) {
        eprintln!("[golive] {args}");
    }
}

/// 視聴で受け取ったものの数（映らないときに、どこで止まっているかを知らせる）
#[derive(Debug, Default)]
struct WatchStats {
    /// 配信者の H264 の RTP
    rtp: u32,
    /// H264 以外の payload type
    other_pt: Vec<u8>,
    /// 通信の暗号を解けなかった RTP
    transport_fail: u32,
    /// 組み上がったフレーム
    frames: u32,
    /// DAVE を解けなかったフレーム
    dave_fail: u32,
    /// キーフレームを待つ間に捨てたフレーム
    no_key: u32,
}

impl WatchStats {
    fn stall_reason(&self, dave_version: u16) -> String {
        if self.rtp == 0 {
            return if self.other_pt.is_empty() {
                tr!(
                    "配信の映像が届きません（配信サーバーから映像が来ていません）",
                    "No stream video is arriving (the stream server isn't sending video)"
                )
                .into()
            } else {
                tr!(
                    "配信の映像が H264 ではないため表示できません（payload type {:?}）",
                    "The stream video isn't H264, so it can't be shown (payload type {:?})",
                    self.other_pt
                )
            };
        }
        if self.transport_fail >= self.rtp {
            return tr!(
                "配信の映像の暗号を解けません",
                "Can't decrypt the stream video"
            )
            .into();
        }
        if self.frames == 0 {
            return tr!(
                "配信の映像を組み立てられません（パケットが欠けています）",
                "Can't assemble the stream video (packets are missing)"
            )
            .into();
        }
        if dave_version != 0 && self.dave_fail >= self.frames {
            return tr!("エンドツーエンド暗号（DAVE）の鍵がそろわず、配信の映像を解けません", "End-to-end encryption (DAVE) keys are incomplete, so the stream video can't be decrypted").into();
        }
        tr!(
            "配信のキーフレームが届きません",
            "No stream keyframe is arriving"
        )
        .into()
    }
}

async fn recv_frame(role: &mut Role) -> Option<Encoded> {
    match role {
        Role::Broadcast { frames, .. } => frames.recv().await,
        Role::Watch { .. } => std::future::pending().await,
    }
}

async fn handle_json(
    ws: &mut Ws,
    dave: &mut Dave,
    v: &Value,
    role: &mut Role,
    streamer_video: &mut Option<u32>,
) -> Result<()> {
    let d = &v["d"];
    debug(format_args!("op {}: {}", v["op"], d));
    match v["op"].as_u64() {
        // CLIENTS_CONNECT
        Some(11) => {
            dave.users.extend(
                d["user_ids"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|u| u.as_str()?.parse::<u64>().ok()),
            );
            if let Role::Broadcast { capture, .. } = role {
                // 視聴者が来たらすぐ映るようにキーフレームを出す
                capture.force_keyframe();
            }
        }
        // VIDEO（配信者の SSRC）
        Some(12) => {
            if let Role::Watch { streamer, .. } = role {
                if d["user_id"].as_str().and_then(|u| u.parse::<u64>().ok()) == Some(*streamer) {
                    let v = d["video_ssrc"].as_u64().unwrap_or(0) as u32;
                    if v != 0 {
                        *streamer_video = Some(v);
                    }
                }
            }
        }
        // CLIENT_DISCONNECT
        Some(13) => {
            if let Some(u) = d["user_id"].as_str().and_then(|u| u.parse::<u64>().ok()) {
                dave.users.remove(&u);
            }
        }
        // DAVE_PREPARE_TRANSITION
        Some(21) => {
            let id = d["transition_id"].as_u64().unwrap_or(0) as u16;
            let ver = d["protocol_version"].as_u64().unwrap_or(0) as u16;
            dave.pending.insert(id, ver);
            if id == 0 {
                dave.execute(0);
            } else {
                if ver == 0 {
                    if let Some(s) = &mut dave.session {
                        s.set_passthrough_mode(true, Some(120));
                    }
                }
                send_json(ws, 23, json!({ "transition_id": id })).await?;
            }
        }
        // DAVE_EXECUTE_TRANSITION
        Some(22) => dave.execute(d["transition_id"].as_u64().unwrap_or(0) as u16),
        // DAVE_PREPARE_EPOCH
        Some(24) if d["epoch"].as_u64() == Some(1) => {
            dave.version = d["protocol_version"].as_u64().unwrap_or(0) as u16;
            if let Some(kp) = dave.reinit() {
                ws.send(WsMsg::Binary(kp.into())).await?;
            }
        }
        _ => {}
    }
    Ok(())
}

async fn handle_binary(ws: &mut Ws, dave: &mut Dave, b: &[u8], seq_ack: &mut i64) -> Result<()> {
    if b.len() < 3 {
        return Ok(());
    }
    *seq_ack = u16::from_be_bytes([b[0], b[1]]) as i64;
    let op = b[2];
    let body = &b[3..];
    debug(format_args!("binary op {op} ({} bytes)", body.len()));
    let transition = |body: &[u8]| {
        if body.len() >= 2 {
            u16::from_be_bytes([body[0], body[1]])
        } else {
            0
        }
    };
    match op {
        // MLS_EXTERNAL_SENDER
        25 => {
            if let Some(s) = &mut dave.session {
                if let Err(e) = s.set_external_sender(body) {
                    eprintln!("DAVE: external sender を設定できません: {e:?}");
                }
            }
        }
        // MLS_PROPOSALS
        27 if !body.is_empty() => {
            let op_type = if body[0] == 0 {
                ProposalsOperationType::APPEND
            } else {
                ProposalsOperationType::REVOKE
            };
            let users: Vec<u64> = dave.users.iter().copied().collect();
            let res = dave
                .session
                .as_mut()
                .map(|s| s.process_proposals(op_type, &body[1..], Some(&users)));
            match res {
                Some(Ok(Some(cw))) => {
                    let mut data = cw.commit;
                    if let Some(w) = cw.welcome {
                        data.extend_from_slice(&w);
                    }
                    ws.send(WsMsg::Binary(binary(28, &data).into())).await?;
                }
                Some(Err(e)) => eprintln!("DAVE: proposals を処理できません: {e:?}"),
                _ => {}
            }
        }
        // MLS_ANNOUNCE_COMMIT_TRANSITION / MLS_WELCOME
        29 | 30 if body.len() >= 2 => {
            let id = transition(body);
            let ok = match dave.session.as_mut() {
                Some(s) if op == 29 => s.process_commit(&body[2..]).map_err(|e| format!("{e:?}")),
                Some(s) => s.process_welcome(&body[2..]).map_err(|e| format!("{e:?}")),
                None => return Ok(()),
            };
            match ok {
                Ok(()) => {
                    if id != 0 {
                        dave.pending.insert(id, dave.version);
                        send_json(ws, 23, json!({ "transition_id": id })).await?;
                    }
                }
                Err(e) => {
                    eprintln!("DAVE: commit / welcome を処理できません: {e}");
                    send_json(ws, 31, json!({ "transition_id": id })).await?;
                    if let Some(kp) = dave.reinit() {
                        ws.send(WsMsg::Binary(kp.into())).await?;
                    }
                }
            }
        }
        _ => {}
    }
    Ok(())
}
