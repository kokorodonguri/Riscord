//! Discord ゲートウェイ接続（WebSocket・ハートビート・RESUME・READY/ディスパッチ処理）。
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{bail, Result};
use futures_util::{SinkExt, StreamExt};
use serde::de::DeserializeOwned;
use serde_json::{json, Value};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use tokio_tungstenite::tungstenite::Message as WsMsg;

use super::inflate::Inflater;
use super::rest;
use super::{Event, ListItem, ListOp, MyPresence, Tx, VoiceMsg};
use crate::model::{self, Member, Message, Presence, VoiceState};

pub(super) const API: &str = "https://discord.com/api/v10";
/// zlib-stream: READY などの大きいペイロードを圧縮して受け取る（展開は Inflater）
pub(super) const GATEWAY: &str = "wss://gateway.discord.gg/?v=10&encoding=json&compress=zlib-stream";

/// テキスト・圧縮バイナリのどちらで届いても JSON にする。まだ途中なら None
pub(super) fn decode_frame(msg: &WsMsg, inflater: &mut Inflater) -> Result<Option<Value>> {
    decode_frame_as(msg, inflater)
}

/// ゲートウェイの1件。op・s・t は従来どおり Value で読み、d は文字列のまま持って
/// 使う種類のときだけ解析する（入力中・リアクションなど使わない種類は木を作らない）
#[derive(serde::Deserialize)]
pub(super) struct GatewayFrame {
    #[serde(default)]
    pub(super) op: Value,
    #[serde(default)]
    pub(super) s: Value,
    #[serde(default)]
    pub(super) t: Value,
    pub(super) d: Option<Box<serde_json::value::RawValue>>,
}

pub(super) fn decode_frame_as<T: DeserializeOwned>(msg: &WsMsg, inflater: &mut Inflater) -> Result<Option<T>> {
    let text = match msg {
        WsMsg::Text(t) => t.as_str(),
        WsMsg::Binary(b) => match inflater.push(b)? {
            Some(t) => t,
            None => return Ok(None),
        },
        _ => return Ok(None),
    };
    let parsed = serde_json::from_str(text).ok();
    if inflater.out.capacity() > Inflater::KEEP {
        inflater.out = Vec::new();
    }
    Ok(parsed)
}

pub(super) fn member_subscription(guild_id: &str, channel_id: &str, ranges: &[[u32; 2]]) -> Value {
    let mut channels = serde_json::Map::new();
    channels.insert(channel_id.to_string(), json!(ranges));
    let mut subs = serde_json::Map::new();
    subs.insert(guild_id.to_string(), json!({ "typing": true, "activities": true, "threads": true, "channels": channels }));
    json!({ "op": 37, "d": { "subscriptions": subs } })
}


pub(super) fn presence_payload(p: &MyPresence) -> Value {
    json!({ "op": 3, "d": presence_data(p) })
}

/// 接続時（identify）と変更時（op 3）に送る自分の見え方
pub(super) fn presence_data(p: &MyPresence) -> Value {
    // 経過時間が「〜分間プレイ中」と出るよう、起動した時刻を開始時刻にする（再接続しても数え直さない）
    static STARTED: std::sync::OnceLock<u64> = std::sync::OnceLock::new();
    let start = *STARTED.get_or_init(|| std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as u64);
    let activities = if p.activity { json!([{ "name": "Riscord", "type": 0, "timestamps": { "start": start } }]) } else { json!([]) };
    json!({ "status": p.status, "since": 0, "activities": activities, "afk": false })
}

pub(super) async fn gateway_loop(
    token: String,
    tx: Tx,
    mut out: UnboundedReceiver<Value>,
    status: Arc<Mutex<MyPresence>>,
    v_tx: UnboundedSender<VoiceMsg>,
) {
    let mut session = GatewaySession::default();
    let mut failures: u32 = 0;
    loop {
        let mut ready = false;
        let end = gateway_once(&token, &tx, &mut out, &status, &v_tx, &mut session, &mut ready).await.unwrap_or_else(|e| {
            eprintln!("ゲートウェイ切断: {e:#}");
            GatewayEnd::Resume
        });
        // つながった（READY / RESUMED まで行った）なら、待ち時間を最初に戻す
        if ready {
            failures = 0;
        }
        let wait = match end {
            GatewayEnd::Fatal(msg) => {
                tx.send(Status(format!("エラー: {msg}")));
                return;
            }
            // 再開できないと言われたら一からログインし直す。Discord の推奨どおり 1〜5 秒待つ
            GatewayEnd::Reidentify => {
                session = GatewaySession::default();
                Duration::from_millis(1000 + rand::random::<u64>() % 4000)
            }
            GatewayEnd::Resume => reconnect_delay(failures),
        };
        failures = failures.saturating_add(1);
        tx.send(Status(format!("接続が切れました。{}秒後に再接続します…", wait.as_secs().max(1))));
        tokio::time::sleep(wait).await;
    }
}

/// 再開（RESUME）に使う、前の接続の情報
#[derive(Default)]
pub(super) struct GatewaySession {
    pub(super) id: Option<String>,
    pub(super) seq: Option<i64>,
    /// READY の resume_gateway_url（再開はここにつなぐ）
    pub(super) resume_url: Option<String>,
}

/// 1回の接続がどう終わったか
#[derive(Debug, PartialEq)]
pub(super) enum GatewayEnd {
    /// 通信が切れた・Discord から再接続を頼まれた: 途中から再開する
    Resume,
    /// 再開できない: 一からログインし直す
    Reidentify,
    /// つなぎ直しても無駄（トークン無効など）。理由（利用者向け）
    Fatal(String),
}

/// 再接続までの待ち時間。1, 2, 4 … 秒と延ばして 60 秒で止め、同時に大勢がつなぎ直さないよう少しずらす
pub(super) fn reconnect_delay(failures: u32) -> Duration {
    let secs = (1u64 << failures.min(6)).min(60);
    Duration::from_millis(secs * 1000 + rand::random::<u64>() % 1000)
}

/// 切断コードごとの扱い（https://discord.com/developers/docs/topics/opcodes-and-status-codes）
pub(super) fn close_action(code: u16) -> GatewayEnd {
    match code {
        4004 => GatewayEnd::Fatal("トークンが無効です。ログインし直してください。".into()),
        4010..=4014 => GatewayEnd::Fatal(format!("Discord に接続を断られました（{code}）。再接続を止めました。")),
        // 連番が合わない・セッションの期限切れ: 再開はできない
        4007 | 4009 => GatewayEnd::Reidentify,
        _ => GatewayEnd::Resume,
    }
}

#[derive(serde::Deserialize)]
struct ReadySession {
    #[serde(default)]
    session_id: Option<String>,
    #[serde(default)]
    resume_gateway_url: Option<String>,
}

use Event::Status;

#[allow(clippy::too_many_arguments)]
pub(super) async fn gateway_once(
    token: &str,
    tx: &Tx,
    out: &mut UnboundedReceiver<Value>,
    status: &Mutex<MyPresence>,
    v_tx: &UnboundedSender<VoiceMsg>,
    session: &mut GatewaySession,
    ready: &mut bool,
) -> Result<GatewayEnd> {
    gateway_once_at(GATEWAY, token, tx, out, status, v_tx, session, ready).await
}

/// gateway = 最初につなぐ所（テストでは手元のサーバー）
#[allow(clippy::too_many_arguments)]
pub(super) async fn gateway_once_at(
    gateway: &str,
    token: &str,
    tx: &Tx,
    out: &mut UnboundedReceiver<Value>,
    status: &Mutex<MyPresence>,
    v_tx: &UnboundedSender<VoiceMsg>,
    session: &mut GatewaySession,
    ready: &mut bool,
) -> Result<GatewayEnd> {
    // 再開できるときは READY で教わった接続先へ（同じ接続の設定を付ける）
    let resume = session.id.clone().zip(session.seq);
    let url = match (&resume, &session.resume_url) {
        (Some(_), Some(base)) => format!("{}/?{}", base.trim_end_matches('/'), gateway.split_once('?').map_or("", |q| q.1)),
        _ => gateway.to_string(),
    };
    let (ws, _) = tokio_tungstenite::connect_async(url.as_str()).await?;
    let (mut write, mut read) = ws.split();
    let mut inflater = Inflater::new();

    let hello = loop {
        match read.next().await {
            Some(Ok(m @ (WsMsg::Text(_) | WsMsg::Binary(_)))) => match decode_frame(&m, &mut inflater)? {
                Some(v) => break v,
                None => continue,
            },
            Some(Ok(_)) => continue,
            Some(Err(e)) => return Err(e.into()),
            None => bail!("helloを受信する前に切断"),
        }
    };
    let interval_ms = hello["d"]["heartbeat_interval"].as_u64().unwrap_or(41_250);
    if interval_ms == 0 {
        bail!("無効な heartbeat interval");
    }

    let first = match &resume {
        // 途中から再開する（切れていた間のイベントは Discord が送り直す）
        Some((id, seq)) => json!({ "op": 6, "d": { "token": token, "session_id": id, "seq": seq } }),
        None => json!({
            "op": 2,
            "d": {
                "token": token,
                "properties": { "os": "Linux", "browser": "Discord Client", "device": "" },
                "presence": presence_data(&status.lock().unwrap()),
                "compress": false
            }
        }),
    };
    write.send(WsMsg::Text(first.to_string().into())).await?;

    let period = Duration::from_millis(interval_ms);
    let mut hb = tokio::time::interval_at(tokio::time::Instant::now() + period, period);
    let mut acked = true;

    loop {
        tokio::select! {
            Some(v) = out.recv() => {
                write.send(WsMsg::Text(v.to_string().into())).await?;
            }
            _ = hb.tick() => {
                if !acked { bail!("heartbeat ACKなし"); }
                acked = false;
                write.send(WsMsg::Text(json!({"op": 1, "d": session.seq}).to_string().into())).await?;
            }
            msg = read.next() => {
                let m = match msg {
                    Some(Ok(m @ (WsMsg::Text(_) | WsMsg::Binary(_)))) => m,
                    Some(Ok(WsMsg::Close(f))) => {
                        if let Some(f) = &f {
                            let code: u16 = f.code.into();
                            if code == 4004 { tx.send(Event::AuthFailed); }
                            eprintln!("ゲートウェイが閉じられた: {code} {}", f.reason);
                            return Ok(close_action(code));
                        }
                        return Ok(GatewayEnd::Resume);
                    }
                    Some(Ok(_)) => continue,
                    Some(Err(e)) => return Err(e.into()),
                    None => bail!("切断"),
                };
                let Some(f) = decode_frame_as::<GatewayFrame>(&m, &mut inflater)? else { continue };
                if let Some(s) = f.s.as_i64() { session.seq = Some(s); }
                match f.op.as_u64() {
                    Some(0) => {
                        let t = f.t.as_str().unwrap_or("");
                        match t {
                            "READY" => {
                                if let Some(r) = f.d.as_deref().and_then(|raw| serde_json::from_str::<ReadySession>(raw.get()).ok()) {
                                    session.id = r.session_id;
                                    session.resume_url = r.resume_gateway_url;
                                }
                                *ready = true;
                                tx.send(Event::GatewayReady);
                            }
                            "RESUMED" => {
                                *ready = true;
                                tx.send(Status("接続済み".into()));
                            }
                            _ => {}
                        }
                        dispatch_raw(t, f.d.as_deref(), tx, v_tx);
                    }
                    Some(1) => write.send(WsMsg::Text(json!({"op": 1, "d": session.seq}).to_string().into())).await?,
                    // 再接続を頼まれた: 再開する
                    Some(7) => return Ok(GatewayEnd::Resume),
                    // セッションが無効: d が true なら再開でき、false なら一からログインし直す
                    Some(9) => {
                        let resumable = f.d.as_deref().is_some_and(|d| d.get().trim() == "true");
                        return Ok(if resumable { GatewayEnd::Resume } else { GatewayEnd::Reidentify });
                    }
                    Some(11) => acked = true,
                    _ => {}
                }
            }
        }
    }
}

/// ディスパッチ (op 0) を種類 t と本体 d に分ける。
/// READY などは数MBになるので、d は複製せず取り出す（受信時は dispatch_raw を使う。テストで Value から呼ぶ用）
#[cfg(test)]
pub(super) fn split_payload(mut v: Value) -> (String, Value) {
    let d = v.get_mut("d").map(Value::take).unwrap_or_default();
    (v["t"].as_str().unwrap_or("").to_string(), d)
}

/// dispatch が扱う種類。dispatch に種類を足したらここにも足す
pub(super) fn handles(t: &str) -> bool {
    matches!(
        t,
        "READY" | "READY_SUPPLEMENTAL" | "SESSIONS_REPLACE" | "GUILD_MEMBERS_CHUNK" | "GUILD_MEMBER_LIST_UPDATE" | "PRESENCE_UPDATE" | "VOICE_STATE_UPDATE" | "VOICE_SERVER_UPDATE"
            | "CALL_CREATE" | "CALL_UPDATE" | "CALL_DELETE" | "MESSAGE_CREATE" | "MESSAGE_UPDATE" | "MESSAGE_DELETE"
    ) || rest::FORWARDED_EVENTS.contains(&t)
}

/// READY のうち dispatch が読む所（サーバーの id・voice_states と presences）。残りは読み飛ばす
#[derive(serde::Deserialize)]
struct ReadyGuild {
    #[serde(default)]
    id: Value,
    #[serde(default)]
    voice_states: Value,
    #[serde(default)]
    emojis: Value,
    /// チャンネル・スレッドの ID だけ（read_state のチャンネルがどのサーバーのものか知るため）
    #[serde(default)]
    channels: Vec<IdOnly>,
    #[serde(default)]
    threads: Vec<IdOnly>,
    /// 古い形の READY では、自分のメンバー情報がここに入る
    #[serde(default)]
    members: Vec<ReadyMember>,
}

#[derive(serde::Deserialize)]
struct IdOnly {
    #[serde(default)]
    id: Value,
}

/// 自分のメンバー情報（ロールでのメンションを見分けるため）
#[derive(serde::Deserialize)]
struct ReadyMember {
    #[serde(default)]
    user_id: Value,
    #[serde(default)]
    user: Option<IdOnly>,
    #[serde(default)]
    roles: Vec<Value>,
}

impl ReadyMember {
    fn is(&self, me: &Value) -> bool {
        !me.is_null() && (&self.user_id == me || self.user.as_ref().is_some_and(|u| &u.id == me))
    }
}

#[derive(serde::Deserialize)]
struct Ready {
    #[serde(default)]
    guilds: Vec<ReadyGuild>,
    #[serde(default)]
    presences: Value,
    #[serde(default)]
    user_guild_settings: Value,
    #[serde(default)]
    read_state: Value,
    #[serde(default)]
    relationships: Value,
    #[serde(default)]
    user_settings: Value,
    #[serde(default)]
    user_settings_proto: Value,
    #[serde(default)]
    user: Option<IdOnly>,
    /// guilds と同じ順に、そのサーバーでの自分のメンバー情報
    #[serde(default)]
    merged_members: Vec<Vec<ReadyMember>>,
}

/// 文字列のままの d を、使う種類のときだけ解析して dispatch に渡す
pub(super) fn dispatch_raw(t: &str, d: Option<&serde_json::value::RawValue>, tx: &Tx, v_tx: &UnboundedSender<VoiceMsg>) {
    if !handles(t) {
        return;
    }
    let Some(raw) = d else { return dispatch(t, Value::Null, tx, v_tx) };
    if t == "READY" {
        // READY はサーバーのチャンネル・ロールなどで大きいので、使う所だけの小さい形にしてから渡す。
        // 想定外の形なら従来どおり全体を解析する
        if let Ok(r) = serde_json::from_str::<Ready>(raw.get()) {
            let me = r.user.map(|u| u.id).unwrap_or_default();
            // 自分のメンバー情報だけ残す（dispatch がロールを読む）
            let member = |m: &ReadyMember| json!({ "user_id": me, "roles": m.roles });
            let merged: Vec<Value> = r.merged_members.iter().map(|ms| Value::Array(ms.iter().filter(|m| m.is(&me)).map(member).collect())).collect();
            let ids = |v: &[IdOnly]| v.iter().map(|c| json!({ "id": c.id })).collect::<Vec<_>>();
            let guilds = r
                .guilds
                .into_iter()
                .map(|g| {
                    let members: Vec<Value> = g.members.iter().filter(|m| m.is(&me)).map(member).collect();
                    json!({ "id": g.id, "voice_states": g.voice_states, "emojis": g.emojis, "channels": ids(&g.channels), "threads": ids(&g.threads), "members": members })
                })
                .collect::<Vec<_>>();
            return dispatch(
                t,
                json!({
                    "guilds": guilds,
                    "presences": r.presences,
                    "user_guild_settings": r.user_guild_settings,
                    "read_state": r.read_state,
                    "relationships": r.relationships,
                    "user_settings": r.user_settings,
                    "user_settings_proto": r.user_settings_proto,
                    "user": { "id": me },
                    "merged_members": merged,
                }),
                tx,
                v_tx,
            );
        }
    }
    if let Ok(d) = serde_json::from_str::<Value>(raw.get()) {
        dispatch(t, d, tx, v_tx);
    }
}

fn parse_presence(v: &Value) -> Option<(String, Presence)> {
    let id = v["user_id"].as_str().or_else(|| v["user"]["id"].as_str())?.to_string();
    let status = v["status"].as_str().unwrap_or("offline").to_string();
    Some((id, Presence { status, activity: activity_text(&v["activities"]) }))
}

/// カスタムステータスを優先し、なければ最初のアクティビティを文にする
fn activity_text(acts: &Value) -> Option<String> {
    let acts = acts.as_array()?;
    if let Some(c) = acts.iter().find(|a| a["type"] == 4) {
        let emoji = c["emoji"]["name"].as_str().filter(|_| c["emoji"]["id"].is_null()).unwrap_or("");
        let state = c["state"].as_str().unwrap_or("");
        let text = format!("{emoji} {state}").trim().to_string();
        if !text.is_empty() {
            return Some(text);
        }
    }
    let a = acts.iter().find(|a| a["type"] != 4)?;
    let name = a["name"].as_str()?;
    Some(match a["type"].as_u64() {
        Some(1) => format!("配信中: {name}"),
        Some(2) => match a["details"].as_str() {
            Some(song) => format!("{name} を再生中: {song}"),
            None => format!("{name} を再生中"),
        },
        Some(3) => format!("視聴中: {name}"),
        Some(5) => format!("参戦中: {name}"),
        _ => format!("プレイ中: {name}"),
    })
}

pub(super) fn dispatch(t: &str, mut d: Value, tx: &Tx, v_tx: &UnboundedSender<VoiceMsg>) {
    match t {
        // 自分のセッション一覧。"all" が Discord がまとめた自分の見え方（ほかの人に見えている状態とアクティビティ）
        "SESSIONS_REPLACE" => {
            let list = d.as_array().map(Vec::as_slice).unwrap_or_default();
            if let Some(s) = list.iter().find(|s| s["session_id"] == "all").or(list.first()) {
                let status = s["status"].as_str().unwrap_or("online").to_string();
                tx.send(Event::OwnPresence(Presence { status, activity: activity_text(&s["activities"]) }));
            }
        }
        "READY" => {
            tx.send(Status("接続済み".into()));
            let mut vs = Vec::new();
            for g in d["guilds"].as_array().into_iter().flatten() {
                let gid = g["id"].as_str();
                vs.extend(g["voice_states"].as_array().into_iter().flatten().filter_map(|v| VoiceState::parse(v, gid)));
            }
            if !vs.is_empty() {
                tx.send(Event::VoiceStates(vs));
            }
            // カスタム絵文字（ピッカーと :名前: の変換に使う）
            let mut emojis = Vec::new();
            for g in d.get_mut("guilds").and_then(Value::as_array_mut).into_iter().flatten() {
                let Some(gid) = g["id"].as_str().map(String::from) else { continue };
                let Some(list) = g.get_mut("emojis").map(Value::take) else { continue };
                if let Ok(list) = serde_json::from_value::<Vec<model::Emoji>>(list) {
                    emojis.push((gid, list));
                }
            }
            if emojis.iter().any(|(_, l)| !l.is_empty()) {
                tx.send(Event::GuildEmojis(emojis));
            }
            let list: Vec<_> = d["presences"].as_array().into_iter().flatten().filter_map(parse_presence).collect();
            if !list.is_empty() {
                tx.send(Event::Presences(list));
            }
            let mut extra = serde_json::Map::new();
            let me = &d["user"]["id"];
            let is_me = |m: &Value| !me.is_null() && (&m["user_id"] == me || &m["user"]["id"] == me);
            let (mut my_roles, mut channel_guild) = (serde_json::Map::new(), serde_json::Map::new());
            for (i, g) in d["guilds"].as_array().into_iter().flatten().enumerate() {
                let Some(gid) = g["id"].as_str() else { continue };
                let merged = d["merged_members"][i].as_array().into_iter().flatten();
                if let Some(m) = merged.chain(g["members"].as_array().into_iter().flatten()).find(|m| is_me(m)) {
                    my_roles.insert(gid.into(), m["roles"].clone());
                }
                for c in g["channels"].as_array().into_iter().flatten().chain(g["threads"].as_array().into_iter().flatten()) {
                    if let Some(cid) = c["id"].as_str() {
                        channel_guild.insert(cid.into(), gid.into());
                    }
                }
            }
            if !my_roles.is_empty() {
                extra.insert("my_roles".into(), Value::Object(my_roles));
            }
            if !channel_guild.is_empty() {
                extra.insert("channel_guild".into(), Value::Object(channel_guild));
            }
            for k in ["user_guild_settings", "read_state", "relationships", "user_settings", "user_settings_proto"] {
                if let Some(v) = d.get_mut(k).map(Value::take).filter(|v| !v.is_null()) {
                    extra.insert(k.into(), v);
                }
            }
            if !extra.is_empty() {
                tx.send(Event::ReadyExtra(Value::Object(extra)));
            }
        }
        "READY_SUPPLEMENTAL" => {
            let mp = &d["merged_presences"];
            let friends = mp["friends"].as_array().into_iter().flatten();
            let guilds = mp["guilds"].as_array().into_iter().flatten().filter_map(|g| g.as_array()).flatten();
            let list: Vec<_> = friends.chain(guilds).filter_map(parse_presence).collect();
            tx.send(Event::Presences(list));
        }
        "GUILD_MEMBERS_CHUNK" => {
            if let Some(gid) = d["guild_id"].as_str().map(String::from) {
                // 1000人分の塊を複製せずにそのまま変換する
                let members = d.get_mut("members").map(Value::take).unwrap_or_default();
                if let Ok(members) = serde_json::from_value::<Vec<Member>>(members) {
                    tx.send(Event::GuildMembers(gid, members));
                }
                if let Some(presences) = d["presences"].as_array() {
                    tx.send_lazy(Event::Presences(presences.iter().filter_map(parse_presence).collect()));
                }
            }
        }
        "GUILD_MEMBER_LIST_UPDATE" => {
            let Some(guild_id) = d["guild_id"].as_str().map(String::from) else { return };
            let list_id = d["id"].as_str().unwrap_or("").to_string();
            let groups = d["groups"].as_array().into_iter().flatten()
                .filter_map(|g| Some((g["id"].as_str()?.to_string(), g["count"].as_u64().unwrap_or(0) as u32)))
                .collect();
            let mut members = Vec::new();
            let mut presences = Vec::new();
            let mut item = |v: &mut Value| -> Option<ListItem> {
                if let Some(g) = v.get("group") {
                    return Some(ListItem::Group(g["id"].as_str()?.to_string(), g["count"].as_u64().unwrap_or(0) as u32));
                }
                let m = v.get_mut("member")?;
                if let Some(p) = m.get("presence").and_then(parse_presence) {
                    presences.push(p);
                }
                let member: Member = serde_json::from_value(m.take()).ok()?;
                let id = member.user.as_ref()?.id.clone();
                members.push(member);
                Some(ListItem::Member(id))
            };
            let range = |v: &Value| -> [u32; 2] { [v[0].as_u64().unwrap_or(0) as u32, v[1].as_u64().unwrap_or(0) as u32] };
            let mut ops = Vec::new();
            for op in d["ops"].as_array_mut().into_iter().flatten() {
                let index = op["index"].as_u64().unwrap_or(0) as usize;
                let parsed = match op["op"].as_str().unwrap_or("") {
                    "SYNC" => Some(ListOp::Sync {
                        range: range(&op["range"]),
                        items: op["items"].as_array_mut().into_iter().flatten().filter_map(&mut item).collect(),
                    }),
                    "INSERT" => item(&mut op["item"]).map(|item| ListOp::Insert { index, item }),
                    "UPDATE" => item(&mut op["item"]).map(|item| ListOp::Update { index, item }),
                    "DELETE" => Some(ListOp::Delete { index }),
                    "INVALIDATE" => Some(ListOp::Invalidate { range: range(&op["range"]) }),
                    _ => None,
                };
                ops.extend(parsed);
            }
            if !members.is_empty() {
                tx.send(Event::GuildMembers(guild_id.clone(), members));
            }
            if !presences.is_empty() {
                tx.send_lazy(Event::Presences(presences));
            }
            tx.send(Event::MemberList { guild_id, list_id, groups, ops });
        }
        "PRESENCE_UPDATE" => {
            if let Some(p) = parse_presence(&d) {
                tx.send_lazy(Event::Presences(vec![p]));
            }
        }
        "VOICE_STATE_UPDATE" => {
            if let Some(v) = VoiceState::parse(&d, None) {
                tx.send(Event::VoiceStates(vec![v]));
            }
            let _ = v_tx.send(VoiceMsg::State(d));
        }
        "VOICE_SERVER_UPDATE" => {
            let _ = v_tx.send(VoiceMsg::Server(d));
        }
        "CALL_CREATE" | "CALL_UPDATE" => {
            let Some(cid) = d["channel_id"].as_str() else { return };
            let ringing = d["ringing"].as_array().into_iter().flatten().filter_map(|x| x.as_str().map(String::from)).collect();
            tx.send(Event::Call { channel_id: cid.into(), ringing });
            let vs: Vec<_> = d["voice_states"].as_array().into_iter().flatten().filter_map(|v| VoiceState::parse(v, None)).collect();
            if !vs.is_empty() {
                tx.send(Event::VoiceStates(vs));
            }
        }
        "CALL_DELETE" => {
            if let Some(cid) = d["channel_id"].as_str() {
                tx.send(Event::CallEnded(cid.into()));
            }
        }
        "MESSAGE_CREATE" => {
            if let Ok(m) = serde_json::from_value::<Message>(d) {
                tx.send(Event::MessageCreate(m));
            }
        }
        "MESSAGE_UPDATE" => {
            if let (Some(cid), Some(id)) = (d["channel_id"].as_str().map(String::from), d["id"].as_str().map(String::from)) {
                let content = d["content"].as_str().map(String::from);
                let embeds = d.get_mut("embeds").and_then(|e| serde_json::from_value(e.take()).ok());
                tx.send(Event::MessageUpdate { channel_id: cid, id, content, embeds });
            }
        }
        "MESSAGE_DELETE" => {
            if let (Some(cid), Some(id)) = (d["channel_id"].as_str(), d["id"].as_str()) {
                tx.send(Event::MessageDelete { channel_id: cid.into(), id: id.into() });
            }
        }
        // ここに種類を足したら handles() にも足す（足さないと dispatch_raw が解析せずに捨てる）
        t if rest::FORWARDED_EVENTS.contains(&t) => {
            // 画面ではまだ使っていないものも多いので、急がず届ける
            tx.send_lazy(Event::Gateway { t: t.to_string(), d });
        }
        _ => {}
    }
}

