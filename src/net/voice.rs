//! ボイス通話タスクとボイス状態ペイロード。
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};

use super::{Api, Event, Tx, VoiceStatus};
use crate::voice::{VoiceCtl, VoiceSession};

pub(super) enum VoiceMsg {
    Join { guild_id: Option<String>, channel_id: String, ring: bool },
    Leave,
    Flags,
    State(Value),
    Server(Value),
}


// ---- ボイス ----

pub(super) fn voice_state_payload(guild_id: Option<&str>, channel_id: Option<&str>, ctl: &VoiceCtl) -> Value {
    use std::sync::atomic::Ordering::Relaxed;
    json!({ "op": 4, "d": {
        "guild_id": guild_id,
        "channel_id": channel_id,
        "self_mute": ctl.mute.load(Relaxed) || ctl.deaf.load(Relaxed),
        "self_deaf": ctl.deaf.load(Relaxed),
        "self_video": false,
    }})
}

/// 接続先と、そろうのを待っている情報
struct Target {
    guild_id: Option<String>,
    channel_id: String,
    ring: bool,
    connected: bool,
}

pub(super) async fn voice_task(
    me_id: String,
    api: Api,
    tx: Tx,
    gw_tx: UnboundedSender<Value>,
    mut rx: UnboundedReceiver<VoiceMsg>,
    ctl: Arc<VoiceCtl>,
) {
    let mut session: Option<VoiceSession> = None;
    let mut session_id: Option<String> = None;
    let mut target: Option<Target> = None;
    let mut tick = tokio::time::interval(Duration::from_millis(20));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        let msg = tokio::select! {
            m = rx.recv() => match m { Some(m) => m, None => return },
            _ = tick.tick() => {
                if let Some(s) = &mut session { s.tick(); }
                continue;
            }
        };
        match msg {
            VoiceMsg::Join { guild_id, channel_id, ring } => {
                if let Some(s) = &mut session {
                    s.leave();
                }
                let _ = gw_tx.send(voice_state_payload(guild_id.as_deref(), Some(&channel_id), &ctl));
                tx.send(Event::Voice(VoiceStatus::Connecting { guild_id: guild_id.clone(), channel_id: channel_id.clone() }));
                target = Some(Target { guild_id, channel_id, ring, connected: false });
            }
            VoiceMsg::Leave => {
                if let Some(t) = target.take() {
                    let _ = gw_tx.send(voice_state_payload(t.guild_id.as_deref(), None, &ctl));
                }
                if let Some(s) = &mut session {
                    s.leave();
                }
                tx.send(Event::Voice(VoiceStatus::Disconnected));
            }
            VoiceMsg::Flags => {
                if let Some(t) = &target {
                    let _ = gw_tx.send(voice_state_payload(t.guild_id.as_deref(), Some(&t.channel_id), &ctl));
                }
            }
            VoiceMsg::State(d) => {
                if d["user_id"].as_str() != Some(me_id.as_str()) {
                    continue;
                }
                session_id = d["session_id"].as_str().map(String::from);
                match d["channel_id"].as_str() {
                    // 他の端末で抜けた・切断された
                    None => {
                        if target.take().is_some() {
                            if let Some(s) = &mut session {
                                s.leave();
                            }
                            tx.send(Event::Voice(VoiceStatus::Disconnected));
                        }
                    }
                    // 別のチャンネルに移動させられた
                    Some(cid) => {
                        if let Some(t) = &mut target {
                            if t.channel_id != cid {
                                t.channel_id = cid.to_string();
                                if t.connected {
                                    tx.send(Event::Voice(VoiceStatus::Connected {
                                        guild_id: t.guild_id.clone(),
                                        channel_id: t.channel_id.clone(),
                                    }));
                                }
                            }
                        }
                    }
                }
            }
            VoiceMsg::Server(d) => {
                let Some(t) = &mut target else { continue };
                let (Some(endpoint), Some(token), Some(sid)) = (d["endpoint"].as_str(), d["token"].as_str(), &session_id) else {
                    continue;
                };
                let s = session.get_or_insert_with(|| VoiceSession::new(ctl.clone()));
                match s.connect(t.guild_id.as_deref(), &t.channel_id, endpoint, sid, token, &me_id).await {
                    Ok(()) => {
                        t.connected = true;
                        tx.send(Event::Voice(VoiceStatus::Connected { guild_id: t.guild_id.clone(), channel_id: t.channel_id.clone() }));
                        if std::mem::take(&mut t.ring) {
                            let path = format!("/channels/{}/call/ring", t.channel_id);
                            if let Err(e) = api.post(&path, &json!({ "recipients": null })).await {
                                tx.send(Event::Error(format!("呼び出し失敗: {e:#}")));
                            }
                        }
                    }
                    Err(e) => {
                        tx.send(Event::Voice(VoiceStatus::Error(format!("{e:#}"))));
                        let _ = gw_tx.send(voice_state_payload(t.guild_id.as_deref(), None, &ctl));
                        target = None;
                    }
                }
            }
        }
    }
}

