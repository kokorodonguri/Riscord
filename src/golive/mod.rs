//! 画面共有（Discord の Go Live）。自分の画面の配信と、ほかの人の配信の視聴。
//!
//! 流れ（配信）: ポータルで画面を選ぶ → ゲートウェイに STREAM_CREATE(op 18) →
//! STREAM_CREATE / STREAM_SERVER_UPDATE が届く → 配信用のボイス接続（conn.rs）を張って H264 を送る。
//! 視聴: STREAM_WATCH(op 20) → 同じく 2 つのイベント → 接続して受け取り、デコードして画面に出す。
//! 止めるときはどちらも STREAM_DELETE(op 19)。

mod conn;
pub mod media;
pub mod rtp;

use std::sync::{Arc, Mutex};

use serde_json::{json, Value};
use tokio::sync::{mpsc::UnboundedSender, oneshot};

use crate::wake::Waker;

/// 視聴中の映像の 1 枚（BGRA）
pub struct Frame {
    pub width: u32,
    pub height: u32,
    pub data: Vec<u8>,
}

/// 配信の鍵。サーバーなら guild:{guild}:{channel}:{user}、DM 通話なら call:{channel}:{user}
pub fn stream_key(guild_id: Option<&str>, channel_id: &str, user_id: &str) -> String {
    match guild_id {
        Some(g) => format!("guild:{g}:{channel_id}:{user_id}"),
        None => format!("call:{channel_id}:{user_id}"),
    }
}

/// 鍵から配信者の user_id を取り出す
pub fn key_user(key: &str) -> Option<&str> {
    key.rsplit(':')
        .next()
        .filter(|u| !u.is_empty() && u.bytes().all(|b| b.is_ascii_digit()))
}

/// 配信・視聴の段階
#[derive(Clone, Debug, PartialEq)]
pub enum Phase {
    /// 画面を選んでいる・つないでいる
    Starting,
    Live,
    /// 終わった。異常で終わったときは理由
    Ended(Option<String>),
}

/// UI に知らせること
#[derive(Clone, Debug, PartialEq)]
pub enum StreamEvent {
    /// 自分の配信
    Broadcast(Phase),
    /// 自分の配信を見ている人（user_id）
    Viewers(Vec<String>),
    /// 視聴（配信の鍵, 段階）
    Watch(String, Phase),
}

/// voice_task から渡すもの
pub enum Msg {
    Start(media::QualityPreset),
    Stop,
    Watch(String),
    Unwatch,
    /// STREAM_CREATE / STREAM_SERVER_UPDATE / STREAM_UPDATE / STREAM_DELETE
    Gateway(String, Value),
    Picked(u64, Result<media::Source, String>),
    Live(u64),
    Ended(u64, Option<String>),
    /// 接続情報（STREAM_CREATE / STREAM_SERVER_UPDATE）を待ちきれなかった
    Timeout(u64),
}

impl std::fmt::Debug for Msg {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Msg::Gateway(t, _) => write!(f, "Gateway({t})"),
            Msg::Picked(n, r) => write!(f, "Picked({n}, {})", if r.is_ok() { "ok" } else { "err" }),
            Msg::Live(n) => write!(f, "Live({n})"),
            Msg::Ended(n, e) => write!(f, "Ended({n}, {e:?})"),
            Msg::Timeout(n) => write!(f, "Timeout({n})"),
            Msg::Start(quality) => f.debug_tuple("Start").field(quality).finish(),
            Msg::Stop => f.write_str("Stop"),
            Msg::Watch(k) => write!(f, "Watch({k})"),
            Msg::Unwatch => f.write_str("Unwatch"),
        }
    }
}

/// 接続中のボイス（どこにいるか・session_id）
pub struct Place<'a> {
    pub guild_id: Option<&'a str>,
    pub channel_id: &'a str,
    pub session_id: &'a str,
}

struct Session {
    /// Ended などが古い接続のものでないか見分ける番号
    id: u64,
    key: String,
    rtc_server_id: Option<String>,
    server: Option<(String, String)>,
    /// 配信: 選んだ画面（接続を始めるまで持つ）
    source: Option<media::Source>,
    /// 配信: まだ画面を選んでいる
    picking: bool,
    quality: media::QualityPreset,
    stop: Option<oneshot::Sender<()>>,
    started: bool,
}

impl Session {
    fn new(id: u64, key: String) -> Self {
        Session {
            id,
            key,
            rtc_server_id: None,
            server: None,
            source: None,
            picking: false,
            quality: media::QualityPreset::default(),
            stop: None,
            started: false,
        }
    }
}

type Callback<T> = Arc<dyn Fn(T) + Send + Sync>;

/// 配信と視聴を 1 つずつ持つ
pub struct Manager {
    me: String,
    gw: UnboundedSender<Value>,
    emit: Callback<StreamEvent>,
    back: Callback<Msg>,
    frame: Arc<Mutex<Option<Frame>>>,
    waker: Waker,
    broadcast: Option<Session>,
    watch: Option<Session>,
    next_id: u64,
}

impl Manager {
    pub fn new(
        me: String,
        gw: UnboundedSender<Value>,
        emit: impl Fn(StreamEvent) + Send + Sync + 'static,
        back: impl Fn(Msg) + Send + Sync + 'static,
        frame: Arc<Mutex<Option<Frame>>>,
        waker: Waker,
    ) -> Self {
        Manager {
            me,
            gw,
            emit: Arc::new(emit),
            back: Arc::new(back),
            frame,
            waker,
            broadcast: None,
            watch: None,
            next_id: 1,
        }
    }

    fn id(&mut self) -> u64 {
        self.next_id += 1;
        self.next_id
    }

    pub fn handle(&mut self, msg: Msg, place: Option<Place<'_>>) {
        match msg {
            Msg::Start(quality) => self.start(place, quality),
            Msg::Stop => self.stop_broadcast(None),
            Msg::Watch(key) => self.start_watch(key, place),
            Msg::Unwatch => self.stop_watch(None),
            Msg::Picked(id, res) => self.picked(id, res, place),
            Msg::Live(id) => {
                if self.broadcast.as_ref().is_some_and(|s| s.id == id) {
                    (self.emit)(StreamEvent::Broadcast(Phase::Live));
                } else if let Some(s) = self.watch.as_ref().filter(|s| s.id == id) {
                    (self.emit)(StreamEvent::Watch(s.key.clone(), Phase::Live));
                }
            }
            Msg::Ended(id, err) => {
                if self.broadcast.as_ref().is_some_and(|s| s.id == id) {
                    self.stop_broadcast(Some(err.unwrap_or_else(|| {
                        tr!("配信が終了しました", "The stream ended", "直播已结束").into()
                    })));
                } else if self.watch.as_ref().is_some_and(|s| s.id == id) {
                    self.stop_watch(err);
                }
            }
            Msg::Timeout(id) => {
                let why = tr!(
                    "配信サーバーの情報が届きません。少し待ってからもう一度試してください",
                    "Stream server info didn't arrive. Wait a moment and try again",
                    "未收到直播服务器的信息。请稍候再试"
                );
                if self
                    .broadcast
                    .as_ref()
                    .is_some_and(|s| s.id == id && !s.started && !s.picking)
                {
                    conn::debug(format_args!(
                        "{}",
                        tr!(
                            "配信: 接続情報が来ない rtc={:?} server={}",
                            "Stream: no connection info rtc={:?} server={}",
                            "直播：未收到连接信息 rtc={:?} server={}",
                            self.broadcast
                                .as_ref()
                                .and_then(|s| s.rtc_server_id.clone()),
                            self.broadcast.as_ref().is_some_and(|s| s.server.is_some())
                        )
                    ));
                    self.stop_broadcast(Some(why.into()));
                } else if let Some(s) = self.watch.as_ref().filter(|s| s.id == id && !s.started) {
                    conn::debug(format_args!(
                        "{}",
                        tr!(
                            "視聴: 接続情報が来ない rtc={:?} server={}",
                            "Watch: no connection info rtc={:?} server={}",
                            "观看：未收到连接信息 rtc={:?} server={}",
                            s.rtc_server_id,
                            s.server.is_some()
                        )
                    ));
                    self.stop_watch(Some(why.into()));
                }
            }
            Msg::Gateway(t, d) => self.gateway(&t, &d, place),
        }
    }

    /// しばらくして接続を始められていなければ、あきらめる（「接続中」のまま止まらないように）
    fn arm_timeout(&self, id: u64) {
        let back = self.back.clone();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_secs(15)).await;
            back(Msg::Timeout(id));
        });
    }

    /// ボイスから抜けたとき
    pub fn stop_all(&mut self) {
        self.stop_broadcast(None);
        self.stop_watch(None);
    }

    fn start(&mut self, place: Option<Place<'_>>, quality: media::QualityPreset) {
        if self.broadcast.is_some() {
            return;
        }
        if place.is_none() {
            (self.emit)(StreamEvent::Broadcast(Phase::Ended(Some(
                tr!(
                    "ボイスチャンネルに接続してから画面共有してください",
                    "Join a voice channel before sharing your screen",
                    "请先连接语音频道再共享屏幕"
                )
                .into(),
            ))));
            return;
        }
        let id = self.id();
        let mut s = Session::new(id, String::new());
        s.picking = true;
        s.quality = quality;
        self.broadcast = Some(s);
        (self.emit)(StreamEvent::Broadcast(Phase::Starting));
        let back = self.back.clone();
        tokio::spawn(async move {
            let r = media::pick_source().await;
            back(Msg::Picked(id, r));
        });
    }

    fn picked(&mut self, id: u64, res: Result<media::Source, String>, place: Option<Place<'_>>) {
        let Some(s) = self.broadcast.as_mut().filter(|s| s.id == id && s.picking) else {
            return;
        };
        s.picking = false;
        let source = match res {
            Ok(src) => src,
            Err(e) => {
                self.broadcast = None;
                (self.emit)(StreamEvent::Broadcast(Phase::Ended(Some(e))));
                return;
            }
        };
        let Some(p) = place else {
            self.broadcast = None;
            (self.emit)(StreamEvent::Broadcast(Phase::Ended(Some(
                tr!(
                    "ボイスチャンネルから切断されました",
                    "Disconnected from the voice channel",
                    "已从语音频道断开"
                )
                .into(),
            ))));
            return;
        };
        s.key = stream_key(p.guild_id, p.channel_id, &self.me);
        s.source = Some(source);
        let kind = if p.guild_id.is_some() {
            "guild"
        } else {
            "call"
        };
        let _ = self.gw.send(json!({ "op": 18, "d": { "type": kind, "guild_id": p.guild_id, "channel_id": p.channel_id, "preferred_region": null } }));
        let _ = self
            .gw
            .send(json!({ "op": 22, "d": { "stream_key": s.key, "paused": false } }));
        self.arm_timeout(id);
    }

    fn start_watch(&mut self, key: String, place: Option<Place<'_>>) {
        if self.watch.as_ref().is_some_and(|s| s.key == key) {
            return;
        }
        self.stop_watch(None);
        if place.is_none() {
            (self.emit)(StreamEvent::Watch(
                key,
                Phase::Ended(Some(
                    tr!(
                        "ボイスチャンネルに接続してから視聴してください",
                        "Join a voice channel before watching",
                        "请先连接语音频道再观看"
                    )
                    .into(),
                )),
            ));
            return;
        }
        if key_user(&key) == Some(self.me.as_str()) {
            return;
        }
        let id = self.id();
        self.watch = Some(Session::new(id, key.clone()));
        let _ = self
            .gw
            .send(json!({ "op": 20, "d": { "stream_key": key } }));
        self.arm_timeout(id);
        (self.emit)(StreamEvent::Watch(key, Phase::Starting));
    }

    fn stop_broadcast(&mut self, err: Option<String>) {
        let Some(s) = self.broadcast.take() else {
            return;
        };
        if !s.key.is_empty() {
            let _ = self
                .gw
                .send(json!({ "op": 19, "d": { "stream_key": s.key } }));
        }
        drop(s);
        (self.emit)(StreamEvent::Broadcast(Phase::Ended(err)));
    }

    fn stop_watch(&mut self, err: Option<String>) {
        let Some(s) = self.watch.take() else { return };
        let _ = self
            .gw
            .send(json!({ "op": 19, "d": { "stream_key": s.key } }));
        *self.frame.lock().unwrap() = None;
        (self.emit)(StreamEvent::Watch(s.key.clone(), Phase::Ended(err)));
    }

    fn gateway(&mut self, t: &str, d: &Value, place: Option<Place<'_>>) {
        // token はログに出さない
        conn::debug(format_args!(
            "{t}: key={} rtc={} endpoint={} reason={} viewers={}",
            d["stream_key"], d["rtc_server_id"], d["endpoint"], d["reason"], d["viewer_ids"]
        ));
        let Some(key) = d["stream_key"].as_str() else {
            return;
        };
        let mine = self.broadcast.as_ref().is_some_and(|s| s.key == key);
        let watching = self.watch.as_ref().is_some_and(|s| s.key == key);
        if !mine && !watching {
            return;
        }
        match t {
            "STREAM_CREATE" | "STREAM_UPDATE" => {
                if mine {
                    let viewers = d["viewer_ids"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(|v| v.as_str().map(String::from))
                        .collect();
                    (self.emit)(StreamEvent::Viewers(viewers));
                }
                if let Some(rtc) = d["rtc_server_id"].as_str() {
                    let s = if mine {
                        self.broadcast.as_mut()
                    } else {
                        self.watch.as_mut()
                    };
                    if let Some(s) = s {
                        s.rtc_server_id = Some(rtc.to_string());
                    }
                }
            }
            "STREAM_SERVER_UPDATE" => {
                if let (Some(ep), Some(tok)) = (d["endpoint"].as_str(), d["token"].as_str()) {
                    let s = if mine {
                        self.broadcast.as_mut()
                    } else {
                        self.watch.as_mut()
                    };
                    if let Some(s) = s {
                        s.server = Some((ep.to_string(), tok.to_string()));
                    }
                }
            }
            "STREAM_DELETE" => {
                let reason = match d["reason"].as_str() {
                    Some("user_requested") | None => None,
                    Some("stream_full") => Some(
                        tr!(
                            "配信の視聴者が満員です",
                            "The stream is full",
                            "直播观众已满"
                        )
                        .to_string(),
                    ),
                    Some("unauthorized") => Some(
                        tr!(
                            "この配信を見る権限がありません",
                            "You don't have permission to watch this stream",
                            "你没有观看此直播的权限"
                        )
                        .to_string(),
                    ),
                    Some("stream_ended") => Some(
                        tr!("配信が終了しました", "The stream ended", "直播已结束").to_string(),
                    ),
                    Some(r) => Some(tr!(
                        "配信が終了しました（{r}）",
                        "The stream ended ({r})",
                        "直播已结束（{r}）",
                    )),
                };
                // 鍵を空にしてから止める（STREAM_DELETE を送り返さない）
                if mine {
                    if let Some(s) = &mut self.broadcast {
                        s.key.clear();
                    }
                    self.stop_broadcast(reason);
                } else {
                    let s = self.watch.take();
                    *self.frame.lock().unwrap() = None;
                    if let Some(s) = s {
                        (self.emit)(StreamEvent::Watch(s.key.clone(), Phase::Ended(reason)));
                    }
                }
                return;
            }
            _ => {}
        }
        self.try_connect(mine, place);
    }

    /// 情報がそろったら接続を始める
    fn try_connect(&mut self, mine: bool, place: Option<Place<'_>>) {
        let Some(place) = place else {
            conn::debug(format_args!(
                "{}",
                tr!(
                    "ボイスの session_id が無いので配信に接続できない",
                    "Can't connect to the stream without a voice session_id",
                    "没有语音的 session_id，无法连接直播",
                )
            ));
            return;
        };
        let me: u64 = self.me.parse().unwrap_or(0);
        let s = if mine {
            self.broadcast.as_mut()
        } else {
            self.watch.as_mut()
        };
        let Some(s) = s else { return };
        if s.started {
            return;
        }
        let (Some(rtc), Some((endpoint, token))) = (s.rtc_server_id.clone(), s.server.clone())
        else {
            return;
        };
        s.started = true;
        let params = conn::Params {
            endpoint,
            token,
            rtc_server_id: rtc,
            session_id: place.session_id.to_string(),
            user_id: me,
        };
        let role = if mine {
            let Some(source) = s.source.take() else {
                return;
            };
            let q = s.quality.fit(source.size);
            let (tx, rx) = tokio::sync::mpsc::channel(8);
            match media::Capture::start(source, q, tx) {
                Ok(capture) => conn::Role::Broadcast {
                    capture,
                    frames: rx,
                    quality: q,
                },
                Err(e) => {
                    self.stop_broadcast(Some(e));
                    return;
                }
            }
        } else {
            let streamer = key_user(&s.key).and_then(|u| u.parse().ok()).unwrap_or(0);
            match media::Decoder::new(self.frame.clone(), self.waker.clone()) {
                Ok(decoder) => conn::Role::Watch { streamer, decoder },
                Err(e) => {
                    self.stop_watch(Some(e));
                    return;
                }
            }
        };
        let (stop_tx, stop_rx) = oneshot::channel();
        s.stop = Some(stop_tx);
        let (id, back) = (s.id, self.back.clone());
        tokio::spawn(async move {
            let b2 = back.clone();
            let r = conn::run(
                params,
                role,
                move |conn::Report::Live| b2(Msg::Live(id)),
                stop_rx,
            )
            .await;
            back(Msg::Ended(id, r.err().map(|e| format!("{e:#}"))));
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stream_keys() {
        assert_eq!(stream_key(Some("1"), "2", "3"), "guild:1:2:3");
        assert_eq!(stream_key(None, "2", "3"), "call:2:3");
        assert_eq!(key_user("guild:1:2:3"), Some("3"));
        assert_eq!(key_user("call:2:44"), Some("44"));
        assert_eq!(key_user("call:2:"), None);
    }

    #[tokio::test]
    async fn watch_and_stop_send_gateway_ops() {
        let (gw, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let events = Arc::new(Mutex::new(Vec::new()));
        let ev = events.clone();
        let mut m = Manager::new(
            "9".into(),
            gw,
            move |e| ev.lock().unwrap().push(e),
            |_| {},
            Default::default(),
            Waker::default(),
        );
        let place = || {
            Some(Place {
                guild_id: Some("1"),
                channel_id: "2",
                session_id: "s",
            })
        };
        m.handle(Msg::Watch("guild:1:2:3".into()), place());
        assert_eq!(rx.try_recv().unwrap()["op"], 20);
        // 自分の配信は見ない
        m.handle(Msg::Unwatch, place());
        assert_eq!(rx.try_recv().unwrap()["op"], 19);
        m.handle(Msg::Watch("guild:1:2:9".into()), place());
        assert!(rx.try_recv().is_err());
        // 配信者が止めた
        m.handle(Msg::Watch("guild:1:2:3".into()), place());
        rx.try_recv().unwrap();
        m.handle(
            Msg::Gateway(
                "STREAM_DELETE".into(),
                json!({ "stream_key": "guild:1:2:3", "reason": "stream_ended" }),
            ),
            place(),
        );
        assert!(rx.try_recv().is_err(), "STREAM_DELETE を送り返さない");
        let ev = events.lock().unwrap();
        assert_eq!(
            ev.last(),
            Some(&StreamEvent::Watch(
                "guild:1:2:3".into(),
                Phase::Ended(Some("配信が終了しました".into()))
            ))
        );
    }

    #[test]
    fn quality_presets_control_size_and_frame_rate() {
        use media::QualityPreset::*;
        for (preset, expected) in [
            (P360_30, (640, 360, 30)),
            (P360_60, (640, 360, 60)),
            (P720_30, (1280, 720, 30)),
            (P720_60, (1280, 720, 60)),
            (P1080_30, (1920, 1080, 30)),
            (P1080_60, (1920, 1080, 60)),
        ] {
            let q = preset.fit(Some((3840, 2160)));
            assert_eq!((q.width, q.height, q.fps), expected);
        }
        let q = P1080_60.fit(Some((320, 240)));
        assert_eq!((q.width, q.height, q.fps), (320, 240, 60));
    }

    #[test]
    fn quality_settings_default_and_round_trip() {
        use media::QualityPreset;
        let old: crate::app::Settings = serde_json::from_str("{}").unwrap();
        assert_eq!(old.stream_quality, QualityPreset::P720_30);
        let unknown: crate::app::Settings =
            serde_json::from_str(r#"{"stream_quality":"future"}"#).unwrap();
        assert_eq!(unknown.stream_quality, QualityPreset::P720_30);
        let settings = crate::app::Settings {
            stream_quality: QualityPreset::P1080_60,
            ..Default::default()
        };
        let saved = serde_json::to_string(&settings).unwrap();
        let loaded: crate::app::Settings = serde_json::from_str(&saved).unwrap();
        assert_eq!(loaded.stream_quality, QualityPreset::P1080_60);
    }

    #[test]
    fn quality_keeps_aspect() {
        let q = media::Quality::fit(Some((2560, 1440)), 1280, 720, 30);
        assert_eq!((q.width, q.height), (1280, 720));
        let q = media::Quality::fit(Some((1000, 1000)), 1280, 720, 30);
        assert_eq!((q.width, q.height), (720, 720));
        let q = media::Quality::fit(Some((641, 361)), 1280, 720, 30);
        assert_eq!((q.width, q.height), (640, 360));
    }
}
