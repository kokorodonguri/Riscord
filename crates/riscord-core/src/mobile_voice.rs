use crate::{
    mobile::Client,
    voice::{VoiceCtl, VoiceSession},
};
use serde_json::{json, Value};
use std::{
    sync::{atomic::Ordering::Relaxed, Arc, Weak},
    time::Duration,
};
use tokio::sync::mpsc::UnboundedReceiver;

pub enum Command {
    Join {
        guild: Option<String>,
        channel: String,
        ring: bool,
    },
    Leave,
    Flags,
    /// Gateway の READY。中身はこのクライアントのセッションの session_id
    Ready(Option<String>),
    State(Value),
    Server(Value),
}
/// 別のタスクで待っていたボイスサーバーへの接続の結果。gen が今の試行と違えば捨てる
struct Connected {
    gen: u64,
    result: Result<(), String>,
}
struct Session(VoiceSession);
impl Drop for Session {
    fn drop(&mut self) {
        self.0.leave();
    }
}
struct Target {
    guild: Option<String>,
    channel: String,
    ring: bool,
    /// ボイスサーバーにつながった
    connected: bool,
}
fn payload(target: &Target, channel: Option<&str>, ctl: &VoiceCtl) -> Value {
    // スピーカーミュート中はマイクも切れている（デスクトップと同じ）
    let deaf = ctl.deaf.load(Relaxed);
    json!({"op":4,"d":{"guild_id":target.guild,"channel_id":channel,"self_mute":ctl.mute.load(Relaxed)||deaf,"self_deaf":deaf,"self_video":false}})
}
/// 自分の VOICE_STATE_UPDATE が、このクライアントのセッションのものか（デスクトップの voice_state_is_ours と同じ）。
/// READY の session_id をまだ知らない・届いた状態に session_id が無いときは、自分のものとして扱う
fn voice_state_is_ours(state_session: Option<&str>, gateway_session: Option<&str>) -> bool {
    match (state_session, gateway_session) {
        (Some(state), Some(gateway)) => state == gateway,
        _ => true,
    }
}
// drop_call! のあとにすぐ入れ直す所（Join）があるので
#[allow(unused_assignments)]
pub async fn run(
    client: Weak<Client>,
    me: String,
    ctl: Arc<VoiceCtl>,
    mut rx: UnboundedReceiver<Command>,
) {
    let mut target: Option<Target> = None;
    // ボイスサーバーとの接続。入り直すときも作り直さずに使い回す（songbird の Driver は接続し直せる）
    let mut session: Option<Session> = None;
    let mut sid: Option<String> = None;
    let mut server: Option<Value> = None;
    // READY で教わった、このクライアントのゲートウェイの session_id
    let mut gateway_session: Option<String> = None;
    // ボイスサーバーへの接続の試行番号。入り直し・抜けたら進め、古い試行の結果を捨てる
    let mut connect_gen: u64 = 0;
    let mut ringing: Option<tokio::task::JoinHandle<()>> = None;
    let (done_tx, mut done_rx) = tokio::sync::mpsc::unbounded_channel::<Connected>();
    let mut tick = tokio::time::interval(Duration::from_millis(20));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    // 手元の通話をやめる（op 4 は送らない）
    macro_rules! drop_call {
        () => {{
            if let Some(task) = ringing.take() {
                task.abort();
            }
            connect_gen += 1;
            if let Some(s) = &mut session {
                s.0.leave();
            }
            target = None;
            sid = None;
            server = None;
        }};
    }
    loop {
        let command = tokio::select! {
            command = rx.recv() => match command { Some(c) => c, None => break },
            Some(done) = done_rx.recv() => {
                let Some(c) = client.upgrade() else { break };
                if done.gen != connect_gen {
                    continue;
                }
                let (Some(t), Some(s)) = (&mut target, &mut session) else { continue };
                match done.result {
                    Ok(()) => {
                        s.0.finish_connect();
                        t.connected = true;
                        c.push(json!({"t":"VOICE","d":{"status":"通話中","channel_id":t.channel}}));
                        if std::mem::take(&mut t.ring) {
                            // 呼び出しの送信で音声の処理（20ms ごとの送信の切り替え）を止めない
                            let path = format!("/channels/{}/call/ring", t.channel);
                            if let Some(task) = ringing.take() {
                                task.abort();
                            }
                            let api = client.clone();
                            ringing = Some(tokio::spawn(async move {
                                let Some(api) = api.upgrade() else { return };
                                let _ = api
                                    .request(reqwest::Method::POST, &path, Some(json!({"recipients":null})))
                                    .await;
                            }));
                        }
                    }
                    Err(e) => {
                        let _ = c.send_gateway(payload(t, None, &ctl));
                        c.push(json!({"t":"VOICE","d":{"status":"未接続","error":format!("通話に接続できませんでした: {e}")}}));
                        drop_call!();
                    }
                }
                continue;
            }
            _ = tick.tick(), if session.is_some() => {
                if let Some(s) = &mut session {
                    s.0.tick();
                }
                continue;
            }
        };
        let Some(c) = client.upgrade() else { break };
        match command {
            Command::Join {
                guild,
                channel,
                ring,
            } => {
                drop_call!();
                let next = Target {
                    guild,
                    channel,
                    ring,
                    connected: false,
                };
                let _ = c.send_gateway(payload(&next, Some(&next.channel), &ctl));
                c.push(json!({"t":"VOICE","d":{"status":"接続中","channel_id":next.channel}}));
                target = Some(next);
            }
            Command::Leave => {
                if let Some(t) = &target {
                    let _ = c.send_gateway(payload(t, None, &ctl));
                }
                drop_call!();
                c.push(json!({"t":"VOICE","d":{"status":"未接続"}}));
            }
            Command::Flags => {
                if let Some(t) = &target {
                    let _ = c.send_gateway(payload(t, Some(&t.channel), &ctl));
                }
            }
            Command::Ready(session_id) => {
                gateway_session = session_id;
                let Some(t) = &target else { continue };
                if t.connected {
                    // 前のゲートウェイのセッションごと VC の接続は切れている。入り直しの op 4 は送らない
                    // （その間にほかの端末で入っていたら奪ってしまう）
                    drop_call!();
                    c.push(json!({"t":"VOICE","d":{"status":"未接続"}}));
                } else {
                    // 入ろうとしている途中: 自分で押した操作なので、新しいセッションで頼み直す
                    sid = None;
                    server = None;
                    let _ = c.send_gateway(payload(t, Some(&t.channel), &ctl));
                }
            }
            Command::State(d) => {
                if d["user_id"].as_str() != Some(&me) {
                    continue;
                }
                let state_sid = d["session_id"].as_str();
                if !voice_state_is_ours(state_sid, gateway_session.as_deref()) {
                    // ほかの端末の VC の状態。入ったのなら VC はそちらに移ったので、手元だけ抜ける
                    // （op 4 で抜けるとほかの端末を追い出してしまう）
                    if d["channel_id"].is_string() && target.is_some() {
                        drop_call!();
                        c.push(json!({"t":"VOICE","d":{"status":"未接続"}}));
                    }
                    continue;
                }
                if d["channel_id"].is_null() {
                    drop_call!();
                    c.push(json!({"t":"VOICE","d":{"status":"未接続"}}));
                } else if let Some(t) = &mut target {
                    if let Some(channel) = d["channel_id"].as_str() {
                        if t.channel != channel {
                            t.channel = channel.to_owned();
                            if t.connected {
                                c.push(json!({"t":"VOICE","d":{"status":"通話中","channel_id":t.channel}}));
                            }
                        }
                    }
                    sid = state_sid.map(str::to_owned);
                }
            }
            Command::Server(d) => {
                let Some(t) = &target else { continue };
                if t.guild.as_deref() != d["guild_id"].as_str() {
                    continue;
                }
                server = Some(d);
            }
        }
        // session_id とボイスサーバーがそろったら、つなぎ始める（つながるのは別のタスクで待つ）
        if let (Some(t), Some(sid_value)) = (&target, &sid) {
            let Some(server_value) = server.take() else {
                continue;
            };
            let (Some(endpoint), Some(token)) = (
                server_value["endpoint"].as_str(),
                server_value["token"].as_str(),
            ) else {
                continue;
            };
            let s = session.get_or_insert_with(|| Session(VoiceSession::new(ctl.clone())));
            connect_gen += 1;
            let gen = connect_gen;
            let result = match s.0.begin_connect(
                t.guild.as_deref(),
                &t.channel,
                endpoint,
                sid_value,
                token,
                &me,
            ) {
                Ok(connecting) => {
                    let done_tx = done_tx.clone();
                    tokio::spawn(async move {
                        let result = connecting.await.map_err(|e| format!("{e:#}"));
                        let _ = done_tx.send(Connected { gen, result });
                    });
                    continue;
                }
                Err(e) => Err(format!("{e:#}")),
            };
            let _ = done_tx.send(Connected { gen, result });
        }
    }
    if let Some(task) = ringing.take() {
        task.abort();
    }
}
