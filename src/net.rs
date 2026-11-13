use std::time::Duration;

use anyhow::{bail, Result};
use futures_util::{SinkExt, StreamExt};
use reqwest::{header::AUTHORIZATION, StatusCode};
use serde::de::DeserializeOwned;
use serde_json::{json, Value};
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};
use tokio_tungstenite::tungstenite::Message as WsMsg;

use crate::model::{
    self, Channel, Embed, Guild, Member, Message, Presence, ProfileData, Role, User, VoiceState,
};
use crate::voice::{VoiceCtl, VoiceSession};
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{Arc, Mutex};

mod inflate;
mod rest;
use inflate::Inflater;
#[allow(unused_imports)]
pub use rest::encode;
pub use rest::ApiCall;
#[allow(unused_imports)]
pub use rest::{
    call_connect, request_guild_members, stream_create, stream_delete, stream_set_paused,
    stream_watch,
};

impl Api {
    /// ApiCall を実行して、返ってきた JSON（本文がなければ Null）を返す
    async fn call(&self, call: &ApiCall) -> anyhow::Result<Value> {
        let (method, path, body) = call.route();
        let r = self.request(method, &path, body.as_ref()).await?;
        let bytes = r.bytes().await?;
        if bytes.is_empty() {
            return Ok(Value::Null);
        }
        Ok(serde_json::from_slice(&bytes).unwrap_or(Value::Null))
    }
}

const API: &str = "https://discord.com/api/v10";
/// zlib-stream: READY などの大きいペイロードを圧縮して受け取る（展開は Inflater）
const GATEWAY: &str = "wss://gateway.discord.gg/?v=10&encoding=json&compress=zlib-stream";

/// テキスト・圧縮バイナリのどちらで届いても JSON にする。まだ途中なら None
fn decode_frame(msg: &WsMsg, inflater: &mut Inflater) -> Result<Option<Value>> {
    decode_frame_as(msg, inflater)
}

/// ゲートウェイの1件。op・s・t は従来どおり Value で読み、d は文字列のまま持って
/// 使う種類のときだけ解析する（入力中・リアクションなど使わない種類は木を作らない）
#[derive(serde::Deserialize)]
struct GatewayFrame {
    #[serde(default)]
    op: Value,
    #[serde(default)]
    s: Value,
    #[serde(default)]
    t: Value,
    d: Option<Box<serde_json::value::RawValue>>,
}

fn decode_frame_as<T: DeserializeOwned>(msg: &WsMsg, inflater: &mut Inflater) -> Result<Option<T>> {
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

pub enum Cmd {
    LoadGuildChannels(String),
    /// メンバー一覧のうち、見えている範囲だけを購読する（公式クライアントと同じ op 37）。
    /// ranges は [先頭, 末尾] の組（例: [[0, 99], [100, 199]]）
    SubscribeMembers {
        guild_id: String,
        channel_id: String,
        ranges: Vec<[u32; 2]>,
    },
    LoadMessages(String),
    /// before より古いメッセージを読み足す（上へスクロールしたとき）
    LoadOlder {
        channel_id: String,
        before: String,
    },
    /// reply_to = 返信先のメッセージID
    /// files = 添付ファイル（名前, 中身）
    Send {
        channel_id: String,
        content: String,
        reply_to: Option<String>,
        files: Vec<(String, Vec<u8>)>,
    },
    Edit {
        channel_id: String,
        id: String,
        content: String,
    },
    Delete {
        channel_id: String,
        id: String,
    },
    LoadSticker {
        id: String,
        format: u8,
    },
    /// online / idle / dnd / invisible
    SetStatus(String),
    /// アクティビティに「Riscord をプレイ中」を出すか
    SetActivity(bool),
    LoadProfile {
        user_id: String,
        guild_id: Option<String>,
    },
    /// 自分のプロフィールを変える。None は変えない。空の文字列は消す。avatar は Some(None) で削除、Some(Some(data URI)) で変更
    UpdateProfile {
        global_name: Option<String>,
        pronouns: Option<String>,
        bio: Option<String>,
        avatar: Option<Option<String>>,
    },
    /// ボイスチャンネル / DM通話に入る。ring=true なら相手を呼び出す
    JoinVoice {
        guild_id: Option<String>,
        channel_id: String,
        ring: bool,
    },
    LeaveVoice,
    /// ミュート・スピーカーミュートが変わったのを Discord に伝える
    VoiceFlags,
    /// 着信を拒否する
    StopRinging(String),
    /// サウンドボードの音を手元で鳴らす（CDN から取って復号し、受信の音声に混ぜる）
    PlaySound {
        sound_id: String,
        volume: f32,
    },
    /// 画面共有を始める（共有する画面をポータルで選んでもらう）/ やめる
    StartStream(crate::golive::media::QualityPreset),
    StopStream,
    /// 配信を見る（配信の鍵。golive::stream_key）/ 見るのをやめる
    WatchStream(String),
    StopWatching,
    /// message_id までを既読にする（公式クライアントなど他の端末にも反映される）
    Ack {
        channel_id: String,
        message_id: String,
    },
    /// REST API をどれでも呼ぶ（src/net/rest.rs）。結果は Event::Api { id, .. } で返る
    Api {
        id: u64,
        call: ApiCall,
    },
    /// ゲートウェイにそのまま送る（{"op": …, "d": …}。rest.rs の request_guild_members などで作る）
    Gateway(Value),
}

#[derive(Clone, Debug, PartialEq)]
pub enum VoiceStatus {
    Connecting {
        guild_id: Option<String>,
        channel_id: String,
    },
    Connected {
        guild_id: Option<String>,
        channel_id: String,
    },
    Disconnected,
    Error(String),
}

pub enum Event {
    Me(User),
    AuthFailed,
    LoginFailed,
    MessagesFailed(String),
    /// 既読通知が Discord に届かなかった。古い失敗が新しい通知を消さないよう ID を含める。
    AckFailed {
        channel_id: String,
        message_id: String,
    },
    ComposeResult {
        channel_id: String,
        result: Result<Option<Message>, ()>,
    },
    /// 送信・編集が Discord に断られた（確実に届いていない）。理由を添えて ComposeResult の直前に送る
    ComposeRejected {
        channel_id: String,
        reason: String,
    },
    Guilds(Vec<Guild>),
    Dms(Vec<Channel>),
    GuildChannels(String, Vec<Channel>),
    GuildRoles(String, Vec<Role>),
    GuildMembers(String, Vec<Member>),
    /// メンバー一覧の差分（GUILD_MEMBER_LIST_UPDATE）。メンバーの中身は GuildMembers で別に送る
    MemberList {
        guild_id: String,
        list_id: String,
        groups: Vec<(String, u32)>,
        ops: Vec<ListOp>,
    },
    /// 一からつながった（READY）。購読などはやり直しになる
    GatewayReady,
    Messages(String, Vec<Message>),
    /// このチャンネルを見る権限がない
    NoAccess(String),
    /// LoadOlder の結果（古い順）。失敗したら None
    OlderMessages {
        channel_id: String,
        before: String,
        messages: Option<Vec<Message>>,
    },
    MessageCreate(Message),
    MessageUpdate {
        channel_id: String,
        id: String,
        content: Option<String>,
        embeds: Option<Vec<Embed>>,
        pinned: Option<bool>,
    },
    MessageDelete {
        channel_id: String,
        id: String,
    },
    Status(String),
    Error(String),
    Sticker(String, Result<crate::sticker::Frames, String>),
    Presences(Vec<(String, Presence)>),
    /// 自分の見え方（Discord が全セッションをまとめたもの。アクティビティを含む）
    OwnPresence(Presence),
    Profile {
        key: String,
        result: Result<ProfileData, String>,
    },
    /// UpdateProfile の結果。失敗したら利用者向けの理由
    ProfileSaved(Result<(), String>),
    VoiceStates(Vec<VoiceState>),
    Voice(VoiceStatus),
    /// 画面共有（配信・視聴）の状態
    Stream(crate::golive::StreamEvent),
    /// DM通話の状態（呼び出し中のユーザー）
    Call {
        channel_id: String,
        ringing: Vec<String>,
    },
    CallEnded(String),
    /// Cmd::Api の結果。失敗したら理由
    Api {
        id: u64,
        call: ApiCall,
        result: Result<Value, String>,
    },
    /// 個別に扱っていないゲートウェイのイベント（rest::FORWARDED_EVENTS）。t は種類、d は中身
    Gateway {
        t: String,
        d: Value,
    },
    /// READY のうち、自分の設定まわり（user_guild_settings / read_state / relationships / user_settings_proto）
    ReadyExtra(Value),
    /// サーバーごとのカスタム絵文字（READY）
    GuildEmojis(Vec<(String, Vec<model::Emoji>)>),
}

enum VoiceMsg {
    Join {
        guild_id: Option<String>,
        channel_id: String,
        ring: bool,
    },
    Leave,
    Flags,
    State(Value),
    Server(Value),
    Stream(crate::golive::Msg),
}

#[derive(Clone)]
struct Tx {
    tx: std::sync::mpsc::Sender<Event>,
    ctx: crate::wake::Waker,
}

impl Tx {
    fn send(&self, e: Event) {
        let _ = self.tx.send(e);
        self.ctx.wake();
    }

    /// 急がない更新（オンライン状態など）。大きいサーバーでは毎秒大量に届くので、
    /// 1件ごとに描き直さず 250ms 以内にまとめて1回にする
    fn send_lazy(&self, e: Event) {
        let _ = self.tx.send(e);
        self.ctx.wake_after(std::time::Duration::from_millis(250));
    }
}

/// メンバー一覧の1行
#[derive(Clone, Debug, PartialEq)]
pub enum ListItem {
    /// グループの見出し（"online" / "offline" / ロールID, 人数）
    Group(String, u32),
    /// メンバー（ユーザーID）
    Member(String),
}

/// メンバー一覧への変更（Discord が送ってくる順に当てる）
#[derive(Clone, Debug, PartialEq)]
pub enum ListOp {
    Sync {
        range: [u32; 2],
        items: Vec<ListItem>,
    },
    Insert {
        index: usize,
        item: ListItem,
    },
    Update {
        index: usize,
        item: ListItem,
    },
    Delete {
        index: usize,
    },
    Invalidate {
        range: [u32; 2],
    },
}

fn member_subscription(guild_id: &str, channel_id: &str, ranges: &[[u32; 2]]) -> Value {
    let mut channels = serde_json::Map::new();
    channels.insert(channel_id.to_string(), json!(ranges));
    let mut subs = serde_json::Map::new();
    subs.insert(
        guild_id.to_string(),
        json!({ "typing": true, "activities": true, "threads": true, "channels": channels }),
    );
    json!({ "op": 37, "d": { "subscriptions": subs } })
}

/// 自分の見え方（ステータスと、アクティビティに Riscord を出すか）。接続するたびと変えたときに Discord へ送る
#[derive(Clone)]
pub struct MyPresence {
    pub status: String,
    pub activity: bool,
}

pub fn spawn(
    token: String,
    status: MyPresence,
    ctx: crate::wake::Waker,
    ctl: Arc<VoiceCtl>,
) -> (UnboundedSender<Cmd>, std::sync::mpsc::Receiver<Event>) {
    let (cmd_tx, cmd_rx) = unbounded_channel();
    let (ev_tx, ev_rx) = std::sync::mpsc::channel();
    let tx = Tx { tx: ev_tx, ctx };
    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(io_workers())
            .thread_name("net")
            .enable_all()
            .build()
            .expect("tokio");
        rt.block_on(run(token, status, tx, cmd_rx, ctl));
    });
    (cmd_tx, ev_rx)
}

/// 通信用ランタイムのワーカー数。仕事はほぼ通信の待ちで、重いデコードは spawn_blocking に逃がしているので、
/// コア数に合わせて増やしても速くならない（実測で CPU 時間は 2〜8 で差なし）。
/// 一方ワーカー 1 つごとに RSS が約 2MB 増え、アカウント（区画）ごとに積み上がるので 2〜4 に抑える
pub fn io_workers() -> usize {
    std::thread::available_parallelism().map_or(2, |n| n.get().clamp(2, 4))
}

#[derive(Clone)]
struct Api {
    #[cfg(test)]
    base_url: Option<String>,
    http: reqwest::Client,
    token: String,
    /// サーバーごとのロール一覧と取得時刻。プロフィールを開くたびの再取得（1往復 約180ms）を省く。
    /// 古い名前を出し続けないよう ROLE_CACHE_TTL を過ぎたら取り直す（ユーザー判断で採用）
    roles: Arc<Mutex<HashMap<String, (std::time::Instant, Vec<Role>)>>>,
    /// /users/@me/guilds が返した、サーバーごとの自分の権限（見えるチャンネルを絞るのに使う）
    guild_perms: Arc<Mutex<HashMap<String, u64>>>,
    limits: Arc<ApiLimits>,
}

struct ApiLimits {
    concurrent: tokio::sync::Semaphore,
    cooldown: tokio::sync::Mutex<tokio::time::Instant>,
}

impl Default for ApiLimits {
    fn default() -> Self {
        Self {
            concurrent: tokio::sync::Semaphore::new(8),
            cooldown: tokio::sync::Mutex::new(tokio::time::Instant::now()),
        }
    }
}

/// 1回に取るメッセージ数（API の上限は 100）。これより少なく返ってきたらチャンネルの始まり
pub const HISTORY_PAGE: usize = 50;

const ROLE_CACHE_TTL: Duration = Duration::from_secs(300);

impl Api {
    fn cached_roles(&self, guild_id: &str) -> Option<Vec<Role>> {
        let cache = self.roles.lock().unwrap();
        cache
            .get(guild_id)
            .filter(|(at, _)| at.elapsed() < ROLE_CACHE_TTL)
            .map(|(_, r)| r.clone())
    }

    fn store_roles(&self, guild_id: &str, roles: &[Role]) {
        self.roles.lock().unwrap().insert(
            guild_id.to_string(),
            (std::time::Instant::now(), roles.to_vec()),
        );
    }
}

// サーバー指定の待機時間が不正なら、パニックや無限待機を避けて操作を失敗として返す。
fn retry_delay(value: &Value) -> Result<Duration> {
    let seconds = match value.get("retry_after") {
        None | Some(Value::Null) => 1.0,
        Some(v) => v
            .as_f64()
            .ok_or_else(|| anyhow::anyhow!("invalid retry_after"))?,
    };
    if !seconds.is_finite() || !(0.0..=86_400.0).contains(&seconds) {
        bail!("invalid retry_after");
    }
    Ok(Duration::from_secs_f64(seconds.max(0.1)))
}

impl Api {
    async fn request(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<&Value>,
    ) -> Result<reqwest::Response> {
        self.request_with(method, path, |req| match body {
            Some(b) => req.json(b),
            None => req,
        })
        .await
    }

    /// build で本文を付ける（429 で再送するたびに作り直す。multipart は使い回せないため）
    async fn request_with(
        &self,
        method: reqwest::Method,
        path: &str,
        build: impl Fn(reqwest::RequestBuilder) -> reqwest::RequestBuilder,
    ) -> Result<reqwest::Response> {
        const MAX_RETRIES: usize = 3;
        let _permit =
            tokio::time::timeout(Duration::from_secs(120), self.limits.concurrent.acquire())
                .await??;
        for attempt in 0..=MAX_RETRIES {
            tokio::time::timeout(Duration::from_secs(120), async {
                loop {
                    let until = *self.limits.cooldown.lock().await;
                    if until <= tokio::time::Instant::now() {
                        break;
                    }
                    tokio::time::sleep_until(until).await;
                }
            })
            .await?;
            #[cfg(test)]
            let base = self.base_url.as_deref().unwrap_or(API);
            #[cfg(not(test))]
            let base = API;
            let req = build(
                self.http
                    .request(method.clone(), format!("{base}{path}"))
                    .header(AUTHORIZATION, &self.token),
            );
            let r = req.send().await?;
            match r.status() {
                StatusCode::TOO_MANY_REQUESTS => {
                    if attempt == MAX_RETRIES {
                        bail!(
                            "{}",
                            tr!(
                                "Discord のレート制限: 再試行回数を超えました",
                                "Discord rate limit: too many retries",
                            )
                        );
                    }
                    let v: Value = r.json().await.unwrap_or_default();
                    let delay = retry_delay(&v)?;
                    if delay > Duration::from_secs(120) {
                        bail!(
                            "{}",
                            tr!(
                                "Discord のレート制限: 待機時間が長すぎます",
                                "Discord rate limit: the wait is too long",
                            )
                        );
                    }
                    let mut until = self.limits.cooldown.lock().await;
                    *until = (*until).max(tokio::time::Instant::now() + delay);
                }
                StatusCode::UNAUTHORIZED => return Err(anyhow::Error::msg(UnauthorizedMarker)),
                s if !s.is_success() => {
                    let text = r.text().await.unwrap_or_default();
                    return Err(anyhow::Error::new(HttpError {
                        status: s.as_u16(),
                        text: format!("{method} {path}: {s} {text}"),
                    }));
                }
                _ => return Ok(r),
            }
        }
        unreachable!()
    }

    async fn get<T: DeserializeOwned>(&self, path: &str) -> Result<T> {
        Ok(self
            .request(reqwest::Method::GET, path, None)
            .await?
            .json()
            .await?)
    }

    async fn post(&self, path: &str, body: &Value) -> Result<()> {
        self.request(reqwest::Method::POST, path, Some(body))
            .await?;
        Ok(())
    }

    async fn delete(&self, path: &str) -> Result<()> {
        self.request(reqwest::Method::DELETE, path, None).await?;
        Ok(())
    }
}

#[derive(Debug)]
struct UnauthorizedMarker;
impl std::fmt::Display for UnauthorizedMarker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(tr!("トークンが無効です (401)", "Invalid token (401)"))
    }
}

/// 401 以外の失敗。表示は今までどおり「GET /path: 403 Forbidden {本文}」
#[derive(Debug)]
struct HttpError {
    status: u16,
    text: String,
}
impl std::fmt::Display for HttpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.text)
    }
}
impl std::error::Error for HttpError {}

/// 送信・編集の失敗が「Discord に断られた（届いていない）」ものなら、その理由を利用者向けの言葉で返す。
/// 通信が途中で切れたなど、届いたか分からないときは None
pub fn compose_reject_reason(e: &anyhow::Error) -> Option<String> {
    let h = e.downcast_ref::<HttpError>()?;
    let body: Value = h
        .text
        .find('{')
        .and_then(|i| serde_json::from_str(&h.text[i..]).ok())
        .unwrap_or_default();
    let code = body["code"].as_u64().unwrap_or(0);
    let raw = body.to_string();
    Some(match (h.status, code) {
        (413, _) | (_, 40005) => tr!("ファイルが大きすぎます", "The file is too large").into(),
        (_, 50035) if raw.contains("BASE_TYPE_MAX_LENGTH") || raw.contains("\"content\"") => {
            tr!("本文が長すぎます", "The message is too long").into()
        }
        (_, 50006) => tr!(
            "空のメッセージは送信できません",
            "Empty messages can't be sent"
        )
        .into(),
        (_, 20016) => tr!(
            "低速モード中です。しばらく待ってから送ってください",
            "Slow mode is on. Wait a moment before sending"
        )
        .into(),
        (_, 50007) => tr!(
            "相手がDMを受け付けていません",
            "This user isn't accepting DMs"
        )
        .into(),
        (403, _) | (_, 50013) | (_, 50001) => tr!(
            "このチャンネルで送信する権限がありません",
            "You don't have permission to send messages in this channel"
        )
        .into(),
        (s, _) if (400..500).contains(&s) && s != 429 => tr!(
            "Discord に拒否されました（{s}）",
            "Rejected by Discord ({s})",
        ),
        _ => return None,
    })
}

fn is_forbidden(e: &anyhow::Error) -> bool {
    e.downcast_ref::<HttpError>()
        .is_some_and(|h| h.status == 403)
}

fn is_unauthorized(e: &anyhow::Error) -> bool {
    e.downcast_ref::<UnauthorizedMarker>().is_some()
}

/// サウンドボードの音の取得と、復号済みの音の置き場（同じ音を何度も取りに行かない）
#[derive(Clone)]
struct SoundCache {
    ctl: Arc<VoiceCtl>,
    /// CDN には認証情報を送らないので、API とは別の接続を使う
    http: reqwest::Client,
    decoded: Arc<Mutex<VecDeque<DecodedSound>>>,
    /// 取得中の音（連打で同じ音を何本も取りに行かない）
    loading: Arc<Mutex<HashSet<String>>>,
}

/// 音の ID と、復号した 48 kHz ステレオ
type DecodedSound = (String, Arc<Vec<f32>>);

/// 覚えておく音の数
const SOUND_CACHE: usize = 32;
/// 取りに行く音の大きさの上限（Discord の上限は 512 KB）
const SOUND_MAX_BYTES: usize = 1024 * 1024;

impl SoundCache {
    fn new(ctl: Arc<VoiceCtl>) -> Self {
        Self {
            ctl,
            http: reqwest::Client::builder()
                .connect_timeout(Duration::from_secs(10))
                .timeout(Duration::from_secs(20))
                .build()
                .unwrap_or_default(),
            decoded: Default::default(),
            loading: Default::default(),
        }
    }

    fn cached(&self, id: &str) -> Option<Arc<Vec<f32>>> {
        let mut q = self.decoded.lock().unwrap();
        let i = q.iter().position(|(k, _)| k == id)?;
        let e = q.remove(i)?;
        let pcm = e.1.clone();
        q.push_back(e);
        Some(pcm)
    }

    fn play(&self, sound_id: String, volume: f32, tx: Tx) {
        if sound_id.is_empty()
            || sound_id.len() > 20
            || !sound_id.bytes().all(|b| b.is_ascii_digit())
        {
            return;
        }
        if let Some(pcm) = self.cached(&sound_id) {
            self.ctl.play_effect(&pcm, volume);
            return;
        }
        if !self.loading.lock().unwrap().insert(sound_id.clone()) {
            return;
        }
        let this = self.clone();
        tokio::spawn(async move {
            let r = this.fetch(&sound_id).await;
            this.loading.lock().unwrap().remove(&sound_id);
            match r {
                Ok(pcm) => {
                    let pcm = Arc::new(pcm);
                    this.ctl.play_effect(&pcm, volume);
                    let mut q = this.decoded.lock().unwrap();
                    q.push_back((sound_id, pcm));
                    while q.len() > SOUND_CACHE {
                        q.pop_front();
                    }
                }
                Err(e) => {
                    eprintln!("サウンドボード {sound_id}: {e:#}");
                    tx.send(Event::Error(
                        tr!(
                            "サウンドボードの音を再生できませんでした",
                            "Couldn't play the soundboard sound"
                        )
                        .into(),
                    ));
                }
            }
        });
    }

    async fn fetch(&self, sound_id: &str) -> Result<Vec<f32>> {
        let mut res = self
            .http
            .get(format!(
                "https://cdn.discordapp.com/soundboard-sounds/{sound_id}"
            ))
            .send()
            .await?
            .error_for_status()?;
        if res
            .content_length()
            .is_some_and(|n| n as usize > SOUND_MAX_BYTES)
        {
            anyhow::bail!("{}", tr!("大きすぎます", "Too large",));
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = res.chunk().await? {
            bytes.extend_from_slice(&chunk);
            if bytes.len() > SOUND_MAX_BYTES {
                anyhow::bail!("{}", tr!("大きすぎます", "Too large",));
            }
        }
        tokio::task::spawn_blocking(move || crate::voice::decode_sound(bytes)).await?
    }
}

fn api_client(connect_timeout: Duration, read_timeout: Duration) -> Result<reqwest::Client> {
    Ok(reqwest::Client::builder()
        .user_agent("Mozilla/5.0 (X11; Linux x86_64) riscord/0.1")
        .connect_timeout(connect_timeout)
        // 全体の時間ではなく受信の無通信時間を制限し、大きな添付の送信を妨げない。
        .read_timeout(read_timeout)
        .build()?)
}

async fn run(
    token: String,
    status: MyPresence,
    tx: Tx,
    mut cmd_rx: UnboundedReceiver<Cmd>,
    ctl: Arc<VoiceCtl>,
) {
    let http = api_client(Duration::from_secs(15), Duration::from_secs(30)).expect("http client");
    let api = Api {
        #[cfg(test)]
        base_url: None,
        http,
        token: token.clone(),
        roles: Default::default(),
        guild_perms: Default::default(),
        limits: Default::default(),
    };

    let me_id;
    match api.get::<User>("/users/@me").await {
        Ok(me) => {
            me_id = me.id.clone();
            tx.send(Event::Me(me))
        }
        Err(e) if is_unauthorized(&e) => return tx.send(Event::AuthFailed),
        Err(e) => {
            eprintln!("ログイン失敗: {e:#}");
            return tx.send(Event::LoginFailed);
        }
    }

    let status = Arc::new(Mutex::new(status));
    let (gw_tx, gw_rx) = unbounded_channel::<Value>();
    let (v_tx, v_rx) = unbounded_channel::<VoiceMsg>();
    tokio::spawn(gateway_loop(
        token,
        tx.clone(),
        gw_rx,
        status.clone(),
        v_tx.clone(),
    ));
    let sounds = SoundCache::new(ctl.clone());
    tokio::spawn(voice_task(
        me_id.clone(),
        api.clone(),
        tx.clone(),
        gw_tx.clone(),
        v_tx.clone(),
        v_rx,
        ctl,
    ));

    {
        let (api, tx) = (api.clone(), tx.clone());
        tokio::spawn(async move {
            match api.get::<Vec<Guild>>("/users/@me/guilds").await {
                Ok(g) => {
                    let mut perms = api.guild_perms.lock().unwrap();
                    for guild in &g {
                        if let Some(p) = guild.permissions.as_deref().and_then(|p| p.parse().ok()) {
                            perms.insert(guild.id.clone(), p);
                        }
                    }
                    drop(perms);
                    tx.send(Event::Guilds(g))
                }
                Err(e) => tx.send(Event::Error(tr!(
                    "サーバー一覧: {e:#}",
                    "Server list: {e:#}",
                ))),
            }
            match api.get::<Vec<Channel>>("/users/@me/channels").await {
                Ok(c) => tx.send(Event::Dms(c)),
                Err(e) => tx.send(Event::Error(tr!("DM一覧: {e:#}", "DM list: {e:#}",))),
            }
        });
    }

    while let Some(cmd) = cmd_rx.recv().await {
        if let Cmd::SetActivity(on) = &cmd {
            let payload = {
                let mut p = status.lock().unwrap();
                p.activity = *on;
                presence_payload(&p)
            };
            let _ = gw_tx.send(payload);
            continue;
        }
        if let Cmd::SetStatus(s) = &cmd {
            let payload = {
                let mut p = status.lock().unwrap();
                p.status = s.clone();
                presence_payload(&p)
            };
            let _ = gw_tx.send(payload);
            continue;
        }
        if let Cmd::Gateway(v) = &cmd {
            let _ = gw_tx.send(v.clone());
            continue;
        }
        if let Cmd::PlaySound { sound_id, volume } = &cmd {
            sounds.play(sound_id.clone(), *volume, tx.clone());
            continue;
        }
        if let Cmd::SubscribeMembers {
            guild_id,
            channel_id,
            ranges,
        } = &cmd
        {
            let _ = gw_tx.send(member_subscription(guild_id, channel_id, ranges));
            continue;
        }
        let vmsg = match &cmd {
            Cmd::JoinVoice {
                guild_id,
                channel_id,
                ring,
            } => Some(VoiceMsg::Join {
                guild_id: guild_id.clone(),
                channel_id: channel_id.clone(),
                ring: *ring,
            }),
            Cmd::LeaveVoice => Some(VoiceMsg::Leave),
            Cmd::VoiceFlags => Some(VoiceMsg::Flags),
            Cmd::StartStream(quality) => {
                Some(VoiceMsg::Stream(crate::golive::Msg::Start(*quality)))
            }
            Cmd::StopStream => Some(VoiceMsg::Stream(crate::golive::Msg::Stop)),
            Cmd::WatchStream(key) => Some(VoiceMsg::Stream(crate::golive::Msg::Watch(key.clone()))),
            Cmd::StopWatching => Some(VoiceMsg::Stream(crate::golive::Msg::Unwatch)),
            _ => None,
        };
        if let Some(m) = vmsg {
            let _ = v_tx.send(m);
            continue;
        }
        let (api, tx, me) = (api.clone(), tx.clone(), me_id.clone());
        tokio::spawn(async move {
            let res = match cmd {
                Cmd::LoadGuildChannels(gid) => {
                    let channel_path = format!("/guilds/{gid}/channels");
                    let role_path = format!("/guilds/{gid}/roles");
                    let member_path = format!("/guilds/{gid}/members/{me}");
                    let (channels, roles, member) = tokio::join!(
                        api.get::<Vec<Channel>>(&channel_path),
                        api.get::<Vec<Role>>(&role_path),
                        api.get::<Member>(&member_path),
                    );
                    // 見る権限のないチャンネルは本家と同じく出さない（開くと 403 になる）。
                    // 自分のロールが分からないときは絞らない（見えるはずのものを隠さないため）
                    let filter = match (&roles, &member) {
                        (Ok(roles), Ok(member)) => {
                            let known = api.guild_perms.lock().unwrap().get(&gid).copied();
                            let base = known.unwrap_or_else(|| {
                                model::base_permissions(&gid, roles, &member.roles)
                            });
                            Some((base, member.roles.clone()))
                        }
                        _ => None,
                    };
                    if let Ok(roles) = roles {
                        api.store_roles(&gid, &roles);
                        tx.send(Event::GuildRoles(gid.clone(), roles));
                    }
                    channels.map(|c| {
                        let c = match filter {
                            Some((base, my_roles)) => {
                                model::visible_channels(c, &gid, base, &my_roles, &me)
                            }
                            None => c,
                        };
                        tx.send(Event::GuildChannels(gid, c))
                    })
                }
                Cmd::LoadMessages(cid) => match api
                    .get::<Vec<Message>>(&format!("/channels/{cid}/messages?limit={HISTORY_PAGE}"))
                    .await
                {
                    Ok(mut m) => {
                        m.reverse();
                        tx.send(Event::Messages(cid, m));
                        Ok(())
                    }
                    // 権限がないチャンネル：エラーではなく、そのチャンネルの画面にその旨を出す
                    Err(e) if is_forbidden(&e) => {
                        tx.send(Event::NoAccess(cid));
                        Ok(())
                    }
                    Err(e) => {
                        eprintln!("メッセージ取得失敗: {e:#}");
                        tx.send(Event::MessagesFailed(cid));
                        Ok(())
                    }
                },
                Cmd::LoadOlder { channel_id, before } => {
                    let r = api
                        .get::<Vec<Message>>(&format!(
                            "/channels/{channel_id}/messages?before={before}&limit={HISTORY_PAGE}"
                        ))
                        .await;
                    let messages = match &r {
                        Ok(m) => Some(m.iter().rev().cloned().collect()),
                        // 履歴を読む権限がない：取り直さず、ここまでとする
                        Err(e) if is_forbidden(e) => Some(Vec::new()),
                        Err(_) => None,
                    };
                    tx.send(Event::OlderMessages {
                        channel_id,
                        before,
                        messages,
                    });
                    match r {
                        Err(e) if !is_forbidden(&e) => Err(e),
                        _ => Ok(()),
                    }
                }
                Cmd::SubscribeMembers { .. }
                | Cmd::SetStatus(_)
                | Cmd::SetActivity(_)
                | Cmd::JoinVoice { .. }
                | Cmd::LeaveVoice
                | Cmd::VoiceFlags
                | Cmd::Gateway(_)
                | Cmd::StartStream(_)
                | Cmd::StopStream
                | Cmd::WatchStream(_)
                | Cmd::StopWatching
                | Cmd::PlaySound { .. } => Ok(()),
                Cmd::Api { id, call } => {
                    let result = api.call(&call).await.map_err(|e| format!("{e:#}"));
                    tx.send(Event::Api { id, call, result });
                    Ok(())
                }
                Cmd::Ack {
                    channel_id,
                    message_id,
                } => send_ack(&api, &tx, channel_id, message_id).await,
                Cmd::StopRinging(cid) => {
                    api.post(
                        &format!("/channels/{cid}/call/stop-ringing"),
                        &json!({ "recipients": null }),
                    )
                    .await
                }
                Cmd::UpdateProfile {
                    global_name,
                    pronouns,
                    bio,
                    avatar,
                } => {
                    let result =
                        update_profile(&api, &tx, global_name, pronouns, bio, avatar).await;
                    tx.send(Event::ProfileSaved(result));
                    Ok(())
                }
                Cmd::LoadProfile { user_id, guild_id } => {
                    let key = profile_key(&user_id, guild_id.as_deref());
                    let result = load_profile(&api, &user_id, guild_id.as_deref())
                        .await
                        .map_err(|e| format!("{e:#}"));
                    tx.send(Event::Profile { key, result });
                    Ok(())
                }
                Cmd::LoadSticker { id, format } => {
                    let r = load_sticker(&api.http, &id, format)
                        .await
                        .map_err(|e| format!("{e:#}"));
                    tx.send(Event::Sticker(id, r));
                    Ok(())
                }
                Cmd::Send {
                    channel_id,
                    content,
                    reply_to,
                    files,
                } => {
                    let mut body = json!({ "content": content, "nonce": nonce(), "tts": false });
                    if let Some(id) = reply_to {
                        body["message_reference"] =
                            json!({ "channel_id": channel_id, "message_id": id });
                    }
                    let path = format!("/channels/{channel_id}/messages");
                    let response = if files.is_empty() {
                        api.request(reqwest::Method::POST, &path, Some(&body)).await
                    } else {
                        body["attachments"] = files
                            .iter()
                            .enumerate()
                            .map(|(i, (name, _))| json!({ "id": i, "filename": name }))
                            .collect();
                        let payload = body.to_string();
                        api.request_with(reqwest::Method::POST, &path, |req| {
                            req.multipart(upload_form(&payload, &files))
                        })
                        .await
                    };
                    let result = match response {
                        Ok(response) => Ok(response.json::<Message>().await.ok()),
                        Err(e) => {
                            eprintln!("メッセージ送信失敗: {e:#}");
                            if let Some(reason) = compose_reject_reason(&e) {
                                tx.send(Event::ComposeRejected {
                                    channel_id: channel_id.clone(),
                                    reason,
                                });
                            }
                            Err(())
                        }
                    };
                    tx.send(Event::ComposeResult { channel_id, result });
                    Ok(())
                }
                Cmd::Edit {
                    channel_id,
                    id,
                    content,
                } => {
                    let response = api
                        .request(
                            reqwest::Method::PATCH,
                            &format!("/channels/{channel_id}/messages/{id}"),
                            Some(&json!({ "content": content })),
                        )
                        .await;
                    let result = match response {
                        Ok(response) => Ok(response.json::<Message>().await.ok()),
                        Err(e) => {
                            eprintln!("メッセージ編集失敗: {e:#}");
                            if let Some(reason) = compose_reject_reason(&e) {
                                tx.send(Event::ComposeRejected {
                                    channel_id: channel_id.clone(),
                                    reason,
                                });
                            }
                            Err(())
                        }
                    };
                    tx.send(Event::ComposeResult { channel_id, result });
                    Ok(())
                }
                Cmd::Delete { channel_id, id } => delete_message(&api, &tx, channel_id, id).await,
            };
            if let Err(e) = res {
                tx.send(Event::Error(format!("{e:#}")));
            }
        });
    }
}

async fn delete_message(api: &Api, tx: &Tx, channel_id: String, id: String) -> Result<()> {
    api.delete(&format!("/channels/{channel_id}/messages/{id}"))
        .await?;
    tx.send(Event::MessageDelete { channel_id, id });
    Ok(())
}

async fn send_ack(api: &Api, tx: &Tx, channel_id: String, message_id: String) -> Result<()> {
    let result = api
        .post(
            &format!("/channels/{channel_id}/messages/{message_id}/ack"),
            &json!({ "token": null }),
        )
        .await;
    if result.is_err() {
        tx.send(Event::AckFailed {
            channel_id,
            message_id,
        });
    }
    result
}

async fn load_sticker(
    http: &reqwest::Client,
    id: &str,
    format: u8,
) -> Result<crate::sticker::Frames> {
    let r = http
        .get(crate::sticker::url(id, format))
        .send()
        .await?
        .error_for_status()?;
    let bytes = r.bytes().await?;
    Ok(tokio::task::spawn_blocking(move || crate::sticker::decode(format, &bytes)).await??)
}

pub fn profile_key(user_id: &str, guild_id: Option<&str>) -> String {
    format!("{user_id}/{}", guild_id.unwrap_or("-"))
}

async fn load_profile(api: &Api, user_id: &str, guild_id: Option<&str>) -> Result<ProfileData> {
    let mut path = format!("/users/{user_id}/profile?with_mutual_guilds=false");
    if let Some(g) = guild_id {
        path += &format!("&guild_id={g}");
    }
    let v: Value = api.get(&path).await?;
    let str_of = |x: &Value| x.as_str().filter(|s| !s.is_empty()).map(String::from);
    let up = &v["user_profile"];
    let gp = &v["guild_member_profile"];
    let gm = &v["guild_member"];
    let mut p = ProfileData {
        bio: str_of(&gp["bio"])
            .or_else(|| str_of(&up["bio"]))
            .or_else(|| str_of(&v["user"]["bio"])),
        pronouns: str_of(&gp["pronouns"]).or_else(|| str_of(&up["pronouns"])),
        banner: str_of(&v["user"]["banner"]),
        accent_color: v["user"]["accent_color"]
            .as_u64()
            .or_else(|| up["accent_color"].as_u64())
            .map(|c| c as u32),
        nick: str_of(&gm["nick"]),
        joined_at: str_of(&gm["joined_at"]),
        roles: Vec::new(),
    };
    // ロール名と色はサーバーのロール一覧から引く（失敗しても無視）
    if let (Some(g), Some(ids)) = (guild_id, gm["roles"].as_array()) {
        let all = match api.cached_roles(g) {
            Some(r) => Ok(r),
            None => api
                .get::<Vec<Role>>(&format!("/guilds/{g}/roles"))
                .await
                .inspect(|r| api.store_roles(g, r)),
        };
        if let Ok(all) = all {
            let mut roles: Vec<(i64, String, u32)> = all
                .into_iter()
                .filter(|r| ids.iter().any(|id| id.as_str() == Some(r.id.as_str())))
                .map(|r| (r.position, r.name, r.color))
                .collect();
            roles.sort_by_key(|r| std::cmp::Reverse(r.0));
            p.roles = roles.into_iter().map(|(_, n, c)| (n, c)).collect();
        }
    }
    Ok(p)
}

/// 表示名・アバターは /users/@me、代名詞・自己紹介は /users/@me/profile で変える。
/// 表示名かアバターを変えたら、新しい自分の情報を Event::Me で送る
async fn update_profile(
    api: &Api,
    tx: &Tx,
    global_name: Option<String>,
    pronouns: Option<String>,
    bio: Option<String>,
    avatar: Option<Option<String>>,
) -> std::result::Result<(), String> {
    let mut user = serde_json::Map::new();
    if let Some(n) = global_name {
        user.insert(
            "global_name".into(),
            if n.is_empty() {
                Value::Null
            } else {
                Value::String(n)
            },
        );
    }
    if let Some(a) = avatar {
        user.insert("avatar".into(), a.map_or(Value::Null, Value::String));
    }
    if !user.is_empty() {
        let r = api
            .request(
                reqwest::Method::PATCH,
                "/users/@me",
                Some(&Value::Object(user)),
            )
            .await
            .map_err(|e| profile_error_text(&e))?;
        if let Ok(u) = r.json::<User>().await {
            tx.send(Event::Me(u));
        }
    }
    let mut profile = serde_json::Map::new();
    if let Some(p) = pronouns {
        profile.insert("pronouns".into(), Value::String(p));
    }
    if let Some(b) = bio {
        profile.insert("bio".into(), Value::String(b));
    }
    if !profile.is_empty() {
        api.request(
            reqwest::Method::PATCH,
            "/users/@me/profile",
            Some(&Value::Object(profile)),
        )
        .await
        .map_err(|e| profile_error_text(&e))?;
    }
    Ok(())
}

/// プロフィールの保存に失敗した理由（利用者向け）
pub fn profile_error_text(e: &anyhow::Error) -> String {
    eprintln!("プロフィール保存失敗: {e:#}");
    let Some(h) = e.downcast_ref::<HttpError>() else {
        return tr!(
            "保存できませんでした。通信状態を確認してください",
            "Couldn't save. Check your connection"
        )
        .into();
    };
    let t = &h.text;
    if t.contains("captcha_key") {
        tr!(
            "Discord が確認（CAPTCHA）を求めました。この変更は公式アプリで行ってください",
            "Discord asked for verification (CAPTCHA). Please make this change in the official app"
        )
        .into()
    } else if t.contains("AVATAR_RATE_LIMIT") || t.contains("too fast") {
        tr!(
            "アバターの変更が多すぎます。しばらく待ってから試してください",
            "Too many avatar changes. Wait a while and try again"
        )
        .into()
    } else if t.contains("PASSWORD") {
        tr!(
            "パスワードの確認が必要な変更です。公式アプリで行ってください",
            "This change requires password confirmation. Please make it in the official app"
        )
        .into()
    } else if t.contains("50035") {
        tr!(
            "入力内容を受け付けてもらえませんでした。使えない言葉や長さを確認してください",
            "The input was rejected. Check for disallowed words or length"
        )
        .into()
    } else {
        tr!("保存できませんでした（{}）", "Couldn't save ({})", h.status)
    }
}

fn upload_form(payload: &str, files: &[(String, Vec<u8>)]) -> reqwest::multipart::Form {
    let mut form = reqwest::multipart::Form::new().text("payload_json", payload.to_string());
    for (i, (name, data)) in files.iter().enumerate() {
        form = form.part(
            format!("files[{i}]"),
            reqwest::multipart::Part::bytes(data.clone()).file_name(name.clone()),
        );
    }
    form
}

fn nonce() -> String {
    const EPOCH: u64 = 1_420_070_400_000;
    let ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64;
    ((ms - EPOCH) << 22).to_string()
}

fn presence_payload(p: &MyPresence) -> Value {
    json!({ "op": 3, "d": presence_data(p) })
}

/// 接続時（identify）と変更時（op 3）に送る自分の見え方
fn presence_data(p: &MyPresence) -> Value {
    // 経過時間が「〜分間プレイ中」と出るよう、起動した時刻を開始時刻にする（再接続しても数え直さない）
    static STARTED: std::sync::OnceLock<u64> = std::sync::OnceLock::new();
    let start = *STARTED.get_or_init(|| {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64
    });
    let activities = if p.activity {
        json!([{ "name": "Riscord", "type": 0, "timestamps": { "start": start } }])
    } else {
        json!([])
    };
    json!({ "status": p.status, "since": 0, "activities": activities, "afk": false })
}

async fn gateway_loop(
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
        let end = gateway_once(
            &token,
            &tx,
            &mut out,
            &status,
            &v_tx,
            &mut session,
            &mut ready,
        )
        .await
        .unwrap_or_else(|e| {
            eprintln!("ゲートウェイ切断: {e:#}");
            GatewayEnd::Resume
        });
        // つながった（READY / RESUMED まで行った）なら、待ち時間を最初に戻す
        if ready {
            failures = 0;
        }
        let wait = match end {
            GatewayEnd::Fatal(msg) => {
                tx.send(Status(tr!("エラー: {msg}", "Error: {msg}",)));
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
        tx.send(Status(tr!(
            "接続が切れました。{}秒後に再接続します…",
            "Disconnected. Reconnecting in {} seconds…",
            wait.as_secs().max(1)
        )));
        tokio::time::sleep(wait).await;
    }
}

/// 再開（RESUME）に使う、前の接続の情報
#[derive(Default)]
struct GatewaySession {
    id: Option<String>,
    seq: Option<i64>,
    /// READY の resume_gateway_url（再開はここにつなぐ）
    resume_url: Option<String>,
}

/// 1回の接続がどう終わったか
#[derive(Debug, PartialEq)]
enum GatewayEnd {
    /// 通信が切れた・Discord から再接続を頼まれた: 途中から再開する
    Resume,
    /// 再開できない: 一からログインし直す
    Reidentify,
    /// つなぎ直しても無駄（トークン無効など）。理由（利用者向け）
    Fatal(String),
}

/// 再接続までの待ち時間。1, 2, 4 … 秒と延ばして 60 秒で止め、同時に大勢がつなぎ直さないよう少しずらす
fn reconnect_delay(failures: u32) -> Duration {
    let secs = (1u64 << failures.min(6)).min(60);
    Duration::from_millis(secs * 1000 + rand::random::<u64>() % 1000)
}

/// 切断コードごとの扱い（https://discord.com/developers/docs/topics/opcodes-and-status-codes）
fn close_action(code: u16) -> GatewayEnd {
    match code {
        4004 => GatewayEnd::Fatal(
            tr!(
                "トークンが無効です。ログインし直してください。",
                "Your token is invalid. Please log in again."
            )
            .into(),
        ),
        4010..=4014 => GatewayEnd::Fatal(tr!(
            "Discord に接続を断られました（{code}）。再接続を止めました。",
            "Discord refused the connection ({code}). Stopped reconnecting.",
        )),
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
async fn gateway_once(
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
async fn gateway_once_at(
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
        (Some(_), Some(base)) => format!(
            "{}/?{}",
            base.trim_end_matches('/'),
            gateway.split_once('?').map_or("", |q| q.1)
        ),
        _ => gateway.to_string(),
    };
    let (ws, _) = tokio::time::timeout(
        Duration::from_secs(30),
        tokio_tungstenite::connect_async(url.as_str()),
    )
    .await
    .map_err(|_| {
        anyhow::anyhow!(
            "{}",
            tr!(
                "Gateway 接続がタイムアウトしました",
                "Gateway connection timed out",
            )
        )
    })??;
    let (mut write, mut read) = ws.split();
    let mut inflater = Inflater::new();

    let hello = tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            match read.next().await {
                Some(Ok(m @ (WsMsg::Text(_) | WsMsg::Binary(_)))) => {
                    match decode_frame(&m, &mut inflater)? {
                        Some(v) => break Ok(v),
                        None => continue,
                    }
                }
                Some(Ok(_)) => continue,
                Some(Err(e)) => return Err(e.into()),
                None => bail!(
                    "{}",
                    tr!(
                        "helloを受信する前に切断",
                        "Disconnected before receiving hello",
                    )
                ),
            }
        }
    })
    .await
    .map_err(|_| {
        anyhow::anyhow!(
            "{}",
            tr!(
                "Gateway Hello の受信がタイムアウトしました",
                "Timed out waiting for Gateway Hello",
            )
        )
    })??;
    let interval_ms = hello["d"]["heartbeat_interval"].as_u64().unwrap_or(41_250);
    if interval_ms == 0 {
        bail!(
            "{}",
            tr!("無効な heartbeat interval", "Invalid heartbeat interval",)
        );
    }

    let first = match &resume {
        // 途中から再開する（切れていた間のイベントは Discord が送り直す）
        Some((id, seq)) => {
            json!({ "op": 6, "d": { "token": token, "session_id": id, "seq": seq } })
        }
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
                if !acked { bail!("{}", tr!("heartbeat ACKなし", "No heartbeat ACK",)); }
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
                    None => bail!("{}", tr!("切断", "Disconnected",)),
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
fn split_payload(mut v: Value) -> (String, Value) {
    let d = v.get_mut("d").map(Value::take).unwrap_or_default();
    (v["t"].as_str().unwrap_or("").to_string(), d)
}

/// dispatch が扱う種類。dispatch に種類を足したらここにも足す
fn handles(t: &str) -> bool {
    matches!(
        t,
        "READY"
            | "READY_SUPPLEMENTAL"
            | "SESSIONS_REPLACE"
            | "GUILD_MEMBERS_CHUNK"
            | "GUILD_MEMBER_LIST_UPDATE"
            | "PRESENCE_UPDATE"
            | "VOICE_STATE_UPDATE"
            | "VOICE_SERVER_UPDATE"
            | "STREAM_CREATE"
            | "STREAM_UPDATE"
            | "STREAM_SERVER_UPDATE"
            | "STREAM_DELETE"
            | "CALL_CREATE"
            | "CALL_UPDATE"
            | "CALL_DELETE"
            | "MESSAGE_CREATE"
            | "MESSAGE_UPDATE"
            | "MESSAGE_DELETE"
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
    /// 参加中のスレッド（中身ごと。画面のスレッド一覧に使う）
    #[serde(default)]
    threads: Vec<Value>,
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
    user_settings_proto: Value,
    #[serde(default)]
    user: Option<IdOnly>,
    /// guilds と同じ順に、そのサーバーでの自分のメンバー情報
    #[serde(default)]
    merged_members: Vec<Vec<ReadyMember>>,
    /// スラッシュコマンドの実行で送る（結果のイベントがこのセッションに届く）
    #[serde(default)]
    session_id: Value,
}

/// 文字列のままの d を、使う種類のときだけ解析して dispatch に渡す
fn dispatch_raw(
    t: &str,
    d: Option<&serde_json::value::RawValue>,
    tx: &Tx,
    v_tx: &UnboundedSender<VoiceMsg>,
) {
    if !handles(t) {
        return;
    }
    let Some(raw) = d else {
        return dispatch(t, Value::Null, tx, v_tx);
    };
    if t == "READY" {
        // READY はサーバーのチャンネル・ロールなどで大きいので、使う所だけの小さい形にしてから渡す。
        // 想定外の形なら従来どおり全体を解析する
        if let Ok(r) = serde_json::from_str::<Ready>(raw.get()) {
            let me = r.user.map(|u| u.id).unwrap_or_default();
            // 自分のメンバー情報だけ残す（dispatch がロールを読む）
            let member = |m: &ReadyMember| json!({ "user_id": me, "roles": m.roles });
            let merged: Vec<Value> = r
                .merged_members
                .iter()
                .map(|ms| Value::Array(ms.iter().filter(|m| m.is(&me)).map(member).collect()))
                .collect();
            let ids = |v: &[IdOnly]| v.iter().map(|c| json!({ "id": c.id })).collect::<Vec<_>>();
            let guilds = r
                .guilds
                .into_iter()
                .map(|g| {
                    let members: Vec<Value> = g.members.iter().filter(|m| m.is(&me)).map(member).collect();
                    json!({ "id": g.id, "voice_states": g.voice_states, "emojis": g.emojis, "channels": ids(&g.channels), "threads": g.threads, "members": members })
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
                    "user_settings_proto": r.user_settings_proto,
                    "user": { "id": me },
                    "merged_members": merged,
                    "session_id": r.session_id,
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
    let id = v["user_id"]
        .as_str()
        .or_else(|| v["user"]["id"].as_str())?
        .to_string();
    let status = v["status"].as_str().unwrap_or("offline").to_string();
    Some((
        id,
        Presence {
            status,
            activity: activity_text(&v["activities"]),
        },
    ))
}

/// カスタムステータスを優先し、なければ最初のアクティビティを文にする
fn activity_text(acts: &Value) -> Option<String> {
    let acts = acts.as_array()?;
    if let Some(c) = acts.iter().find(|a| a["type"] == 4) {
        let emoji = c["emoji"]["name"]
            .as_str()
            .filter(|_| c["emoji"]["id"].is_null())
            .unwrap_or("");
        let state = c["state"].as_str().unwrap_or("");
        let text = format!("{emoji} {state}").trim().to_string();
        if !text.is_empty() {
            return Some(text);
        }
    }
    let a = acts.iter().find(|a| a["type"] != 4)?;
    let name = a["name"].as_str()?;
    Some(match a["type"].as_u64() {
        Some(1) => tr!("配信中: {name}", "Streaming: {name}",),
        Some(2) => match a["details"].as_str() {
            Some(song) => tr!("{name} を再生中: {song}", "Listening to {name}: {song}",),
            None => tr!("{name} を再生中", "Listening to {name}",),
        },
        Some(3) => tr!("視聴中: {name}", "Watching: {name}",),
        Some(5) => tr!("参戦中: {name}", "Competing in: {name}",),
        _ => tr!("プレイ中: {name}", "Playing: {name}",),
    })
}

fn dispatch(t: &str, mut d: Value, tx: &Tx, v_tx: &UnboundedSender<VoiceMsg>) {
    match t {
        // 自分のセッション一覧。"all" が Discord がまとめた自分の見え方（ほかの人に見えている状態とアクティビティ）
        "SESSIONS_REPLACE" => {
            let list = d.as_array().map(Vec::as_slice).unwrap_or_default();
            if let Some(s) = list
                .iter()
                .find(|s| s["session_id"] == "all")
                .or(list.first())
            {
                let status = s["status"].as_str().unwrap_or("online").to_string();
                tx.send(Event::OwnPresence(Presence {
                    status,
                    activity: activity_text(&s["activities"]),
                }));
            }
        }
        "READY" => {
            tx.send(Status("接続済み".into()));
            let mut vs = Vec::new();
            for g in d["guilds"].as_array().into_iter().flatten() {
                let gid = g["id"].as_str();
                vs.extend(
                    g["voice_states"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(|v| VoiceState::parse(v, gid)),
                );
            }
            if !vs.is_empty() {
                tx.send(Event::VoiceStates(vs));
            }
            // カスタム絵文字（ピッカーと :名前: の変換に使う）
            let mut emojis = Vec::new();
            for g in d
                .get_mut("guilds")
                .and_then(Value::as_array_mut)
                .into_iter()
                .flatten()
            {
                let Some(gid) = g["id"].as_str().map(String::from) else {
                    continue;
                };
                let Some(list) = g.get_mut("emojis").map(Value::take) else {
                    continue;
                };
                if let Ok(list) = serde_json::from_value::<Vec<model::Emoji>>(list) {
                    emojis.push((gid, list));
                }
            }
            if emojis.iter().any(|(_, l)| !l.is_empty()) {
                tx.send(Event::GuildEmojis(emojis));
            }
            let list: Vec<_> = d["presences"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(parse_presence)
                .collect();
            if !list.is_empty() {
                tx.send(Event::Presences(list));
            }
            let mut extra = serde_json::Map::new();
            let me = &d["user"]["id"];
            let is_me =
                |m: &Value| !me.is_null() && (&m["user_id"] == me || &m["user"]["id"] == me);
            let (mut my_roles, mut channel_guild, mut threads) = (
                serde_json::Map::new(),
                serde_json::Map::new(),
                serde_json::Map::new(),
            );
            for (i, g) in d["guilds"].as_array().into_iter().flatten().enumerate() {
                let Some(gid) = g["id"].as_str() else {
                    continue;
                };
                let merged = d["merged_members"][i].as_array().into_iter().flatten();
                if let Some(m) = merged
                    .chain(g["members"].as_array().into_iter().flatten())
                    .find(|m| is_me(m))
                {
                    my_roles.insert(gid.into(), m["roles"].clone());
                }
                for c in g["channels"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .chain(g["threads"].as_array().into_iter().flatten())
                {
                    if let Some(cid) = c["id"].as_str() {
                        channel_guild.insert(cid.into(), gid.into());
                    }
                }
                if g["threads"].as_array().is_some_and(|t| !t.is_empty()) {
                    threads.insert(gid.into(), g["threads"].clone());
                }
            }
            if !my_roles.is_empty() {
                extra.insert("my_roles".into(), Value::Object(my_roles));
            }
            if !channel_guild.is_empty() {
                extra.insert("channel_guild".into(), Value::Object(channel_guild));
            }
            if !threads.is_empty() {
                extra.insert("threads".into(), Value::Object(threads));
            }
            if let Some(sid) = d["session_id"].as_str() {
                extra.insert("session_id".into(), sid.into());
            }
            for k in [
                "user_guild_settings",
                "read_state",
                "relationships",
                "user_settings_proto",
            ] {
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
            let guilds = mp["guilds"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|g| g.as_array())
                .flatten();
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
                    tx.send_lazy(Event::Presences(
                        presences.iter().filter_map(parse_presence).collect(),
                    ));
                }
            }
        }
        "GUILD_MEMBER_LIST_UPDATE" => {
            let Some(guild_id) = d["guild_id"].as_str().map(String::from) else {
                return;
            };
            let list_id = d["id"].as_str().unwrap_or("").to_string();
            let groups = d["groups"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|g| {
                    Some((
                        g["id"].as_str()?.to_string(),
                        g["count"].as_u64().unwrap_or(0) as u32,
                    ))
                })
                .collect();
            let mut members = Vec::new();
            let mut presences = Vec::new();
            let mut item = |v: &mut Value| -> Option<ListItem> {
                if let Some(g) = v.get("group") {
                    return Some(ListItem::Group(
                        g["id"].as_str()?.to_string(),
                        g["count"].as_u64().unwrap_or(0) as u32,
                    ));
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
            let range = |v: &Value| -> [u32; 2] {
                [
                    v[0].as_u64().unwrap_or(0) as u32,
                    v[1].as_u64().unwrap_or(0) as u32,
                ]
            };
            let mut ops = Vec::new();
            for op in d["ops"].as_array_mut().into_iter().flatten() {
                let index = op["index"].as_u64().unwrap_or(0) as usize;
                let parsed = match op["op"].as_str().unwrap_or("") {
                    "SYNC" => Some(ListOp::Sync {
                        range: range(&op["range"]),
                        items: op["items"]
                            .as_array_mut()
                            .into_iter()
                            .flatten()
                            .filter_map(&mut item)
                            .collect(),
                    }),
                    "INSERT" => item(&mut op["item"]).map(|item| ListOp::Insert { index, item }),
                    "UPDATE" => item(&mut op["item"]).map(|item| ListOp::Update { index, item }),
                    "DELETE" => Some(ListOp::Delete { index }),
                    "INVALIDATE" => Some(ListOp::Invalidate {
                        range: range(&op["range"]),
                    }),
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
            tx.send(Event::MemberList {
                guild_id,
                list_id,
                groups,
                ops,
            });
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
        "STREAM_CREATE" | "STREAM_UPDATE" | "STREAM_SERVER_UPDATE" | "STREAM_DELETE" => {
            let _ = v_tx.send(VoiceMsg::Stream(crate::golive::Msg::Gateway(
                t.to_string(),
                d,
            )));
        }
        "CALL_CREATE" | "CALL_UPDATE" => {
            let Some(cid) = d["channel_id"].as_str() else {
                return;
            };
            let ringing = d["ringing"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|x| x.as_str().map(String::from))
                .collect();
            tx.send(Event::Call {
                channel_id: cid.into(),
                ringing,
            });
            let vs: Vec<_> = d["voice_states"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|v| VoiceState::parse(v, None))
                .collect();
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
            if let (Some(cid), Some(id)) = (
                d["channel_id"].as_str().map(String::from),
                d["id"].as_str().map(String::from),
            ) {
                let content = d["content"].as_str().map(String::from);
                let embeds = d
                    .get_mut("embeds")
                    .and_then(|e| serde_json::from_value(e.take()).ok());
                // ピン留め・解除でも MESSAGE_UPDATE が届く（本文は付かない）
                let pinned = d["pinned"].as_bool();
                tx.send(Event::MessageUpdate {
                    channel_id: cid,
                    id,
                    content,
                    embeds,
                    pinned,
                });
            }
        }
        "MESSAGE_DELETE" => {
            if let (Some(cid), Some(id)) = (d["channel_id"].as_str(), d["id"].as_str()) {
                tx.send(Event::MessageDelete {
                    channel_id: cid.into(),
                    id: id.into(),
                });
            }
        }
        // ここに種類を足したら handles() にも足す（足さないと dispatch_raw が解析せずに捨てる）
        t if rest::FORWARDED_EVENTS.contains(&t) => {
            // 画面ではまだ使っていないものも多いので、急がず届ける
            tx.send_lazy(Event::Gateway {
                t: t.to_string(),
                d,
            });
        }
        _ => {}
    }
}

// ---- ボイス ----

fn voice_state_payload(guild_id: Option<&str>, channel_id: Option<&str>, ctl: &VoiceCtl) -> Value {
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

async fn voice_task(
    me_id: String,
    api: Api,
    tx: Tx,
    gw_tx: UnboundedSender<Value>,
    v_tx: UnboundedSender<VoiceMsg>,
    mut rx: UnboundedReceiver<VoiceMsg>,
    ctl: Arc<VoiceCtl>,
) {
    let mut session: Option<VoiceSession> = None;
    let mut stream = {
        let (tx, ctl2) = (tx.clone(), ctl.clone());
        let waker = crate::wake::Waker::new(move |d| match d {
            None => ctl2.waker().wake(),
            Some(d) => ctl2.waker().wake_after(d),
        });
        crate::golive::Manager::new(
            me_id.clone(),
            gw_tx.clone(),
            move |e| tx.send(Event::Stream(e)),
            move |m| {
                let _ = v_tx.send(VoiceMsg::Stream(m));
            },
            ctl.screen.clone(),
            waker,
        )
    };
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
            VoiceMsg::Stream(m) => {
                let place = match (&target, &session_id) {
                    (Some(t), Some(sid)) if t.connected => Some(crate::golive::Place {
                        guild_id: t.guild_id.as_deref(),
                        channel_id: &t.channel_id,
                        session_id: sid,
                    }),
                    _ => None,
                };
                stream.handle(m, place);
            }
            VoiceMsg::Join {
                guild_id,
                channel_id,
                ring,
            } => {
                stream.stop_all();
                if let Some(s) = &mut session {
                    s.leave();
                }
                let _ = gw_tx.send(voice_state_payload(
                    guild_id.as_deref(),
                    Some(&channel_id),
                    &ctl,
                ));
                tx.send(Event::Voice(VoiceStatus::Connecting {
                    guild_id: guild_id.clone(),
                    channel_id: channel_id.clone(),
                }));
                target = Some(Target {
                    guild_id,
                    channel_id,
                    ring,
                    connected: false,
                });
            }
            VoiceMsg::Leave => {
                stream.stop_all();
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
                    let _ = gw_tx.send(voice_state_payload(
                        t.guild_id.as_deref(),
                        Some(&t.channel_id),
                        &ctl,
                    ));
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
                        stream.stop_all();
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
                                // 配信は元のチャンネルのものなので止める
                                stream.stop_all();
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
                let (Some(endpoint), Some(token), Some(sid)) =
                    (d["endpoint"].as_str(), d["token"].as_str(), &session_id)
                else {
                    continue;
                };
                let s = session.get_or_insert_with(|| VoiceSession::new(ctl.clone()));
                match s
                    .connect(
                        t.guild_id.as_deref(),
                        &t.channel_id,
                        endpoint,
                        sid,
                        token,
                        &me_id,
                    )
                    .await
                {
                    Ok(()) => {
                        t.connected = true;
                        tx.send(Event::Voice(VoiceStatus::Connected {
                            guild_id: t.guild_id.clone(),
                            channel_id: t.channel_id.clone(),
                        }));
                        if std::mem::take(&mut t.ring) {
                            let path = format!("/channels/{}/call/ring", t.channel_id);
                            if let Err(e) = api.post(&path, &json!({ "recipients": null })).await {
                                tx.send(Event::Error(tr!(
                                    "呼び出し失敗: {e:#}",
                                    "Request failed: {e:#}",
                                )));
                            }
                        }
                    }
                    Err(e) => {
                        stream.stop_all();
                        tx.send(Event::Voice(VoiceStatus::Error(format!("{e:#}"))));
                        let _ = gw_tx.send(voice_state_payload(t.guild_id.as_deref(), None, &ctl));
                        target = None;
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn delete_notifies_only_after_http_success() {
        use std::io::{Read, Write};
        for (status, success) in [
            ("204 No Content", true),
            ("500 Internal Server Error", false),
        ] {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            let addr = listener.local_addr().unwrap();
            let server = std::thread::spawn(move || {
                let (mut socket, _) = listener.accept().unwrap();
                socket
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                let mut request = [0; 2048];
                let n = socket.read(&mut request).unwrap();
                assert!(String::from_utf8_lossy(&request[..n])
                    .starts_with("DELETE /channels/c/messages/m HTTP/1.1\r\n"));
                write!(
                    socket,
                    "HTTP/1.1 {status}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                )
                .unwrap();
            });
            let api = Api {
                base_url: Some(format!("http://{addr}")),
                http: reqwest::Client::builder().no_proxy().build().unwrap(),
                token: "test".into(),
                roles: Default::default(),
                guild_perms: Default::default(),
                limits: Default::default(),
            };
            let (sender, events) = std::sync::mpsc::channel();
            let tx = Tx {
                tx: sender,
                ctx: crate::wake::Waker::default(),
            };
            let result = delete_message(&api, &tx, "c".into(), "m".into()).await;
            server.join().unwrap();
            assert_eq!(result.is_ok(), success);
            if success {
                assert!(
                    matches!(events.try_recv(), Ok(Event::MessageDelete { channel_id, id }) if channel_id == "c" && id == "m")
                );
            } else {
                assert!(events.try_recv().is_err());
            }
        }
    }

    #[tokio::test]
    async fn ack_http_error_notifies_backend_and_success_does_not() {
        use std::io::{Read, Write};
        for (status, fails) in [
            ("500 Internal Server Error", true),
            ("204 No Content", false),
        ] {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            let addr = listener.local_addr().unwrap();
            let server = std::thread::spawn(move || {
                let (mut socket, _) = listener.accept().unwrap();
                socket
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                let mut request = [0; 2048];
                let n = socket.read(&mut request).unwrap();
                let request = String::from_utf8_lossy(&request[..n]);
                assert!(
                    request.starts_with("POST /channels/c/messages/m/ack HTTP/1.1\r\n"),
                    "{request}"
                );
                write!(
                    socket,
                    "HTTP/1.1 {status}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                )
                .unwrap();
            });
            let api = Api {
                base_url: Some(format!("http://{addr}")),
                http: reqwest::Client::builder().no_proxy().build().unwrap(),
                token: "test".into(),
                roles: Default::default(),
                guild_perms: Default::default(),
                limits: Default::default(),
            };
            let (sender, events) = std::sync::mpsc::channel();
            let tx = Tx {
                tx: sender,
                ctx: crate::wake::Waker::default(),
            };
            let result = send_ack(&api, &tx, "c".into(), "m".into()).await;
            server.join().unwrap();
            assert_eq!(result.is_err(), fails);
            if fails {
                assert!(
                    matches!(events.try_recv(), Ok(Event::AckFailed { channel_id, message_id }) if channel_id == "c" && message_id == "m")
                );
            } else {
                assert!(events.try_recv().is_err());
            }
        }
    }

    #[tokio::test]
    async fn api_read_timeout_releases_a_stalled_request() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let (release, done) = std::sync::mpsc::channel::<()>();
        let server = std::thread::spawn(move || {
            let (_socket, _) = listener.accept().unwrap();
            let _ = done.recv_timeout(Duration::from_secs(5));
        });
        let client = api_client(Duration::from_secs(1), Duration::from_millis(100)).unwrap();
        let result = client.get(format!("http://{addr}/")).send().await;
        let _ = release.send(());
        server.join().unwrap();
        assert!(result.unwrap_err().is_timeout());
    }

    #[test]
    fn rate_limit_delay_validates_untrusted_values() {
        assert_eq!(retry_delay(&json!({})).unwrap(), Duration::from_secs(1));
        assert_eq!(
            retry_delay(&json!({ "retry_after": 1.5 })).unwrap(),
            Duration::from_millis(1500)
        );
        assert_eq!(
            retry_delay(&json!({ "retry_after": 0 })).unwrap(),
            Duration::from_millis(100)
        );
        for invalid in [json!(-1), json!(1e100), json!("bad"), json!([])] {
            assert!(retry_delay(&json!({ "retry_after": invalid })).is_err());
        }
    }

    /// 大きいサーバーの GUILD_MEMBERS_CHUNK（1000人＋オンライン状態）相当
    fn members_chunk() -> Value {
        let members: Vec<Value> = (0..1000)
            .map(|i| json!({ "user": { "id": format!("{}", 10_000 + i), "username": format!("user{i}"), "global_name": format!("ユーザー {i}"), "avatar": "0123456789abcdef0123456789abcdef" }, "nick": null, "roles": ["1", "2", "3"], "joined_at": "2024-01-01T00:00:00+00:00" }))
            .collect();
        let presences: Vec<Value> = (0..1000)
            .map(|i| json!({ "user": { "id": format!("{}", 10_000 + i) }, "status": "online", "activities": [{ "type": 0, "name": "game" }] }))
            .collect();
        json!({ "op": 0, "s": 5, "t": "GUILD_MEMBERS_CHUNK", "d": { "guild_id": "42", "members": members, "presences": presences } })
    }

    struct Sink {
        tx: Tx,
        rx: std::sync::mpsc::Receiver<Event>,
        v_tx: UnboundedSender<VoiceMsg>,
    }

    /// 自分のセッション一覧からは、Discord がまとめた "all" の見え方を使う
    #[test]
    fn sessions_replace_reports_merged_own_presence() {
        let s = sink();
        let d = json!([
            { "session_id": "abcd", "status": "idle", "activities": [] },
            { "session_id": "all", "status": "online", "activities": [{ "type": 0, "name": "Riscord" }] },
        ]);
        dispatch("SESSIONS_REPLACE", d, &s.tx, &s.v_tx);
        let events: Vec<Event> = s.rx.try_iter().collect();
        assert!(
            matches!(&events[..], [Event::OwnPresence(p)] if p.status == "online" && p.activity.as_deref() == Some("プレイ中: Riscord"))
        );
    }

    fn sink() -> Sink {
        let (tx, rx) = std::sync::mpsc::channel();
        let (v_tx, _) = unbounded_channel();
        Sink {
            tx: Tx {
                tx,
                ctx: crate::wake::Waker::default(),
            },
            rx,
            v_tx,
        }
    }

    /// Discord と同じく、1接続で1つの圧縮文脈から Sync フラッシュで区切って送る
    fn zlib_stream(msgs: &[String]) -> Vec<Vec<u8>> {
        let mut z = flate2::Compress::new(flate2::Compression::default(), true);
        msgs.iter()
            .map(|m| {
                let mut out = Vec::with_capacity(m.len() + 1024);
                let mut input = m.as_bytes();
                loop {
                    if out.len() == out.capacity() {
                        out.reserve(64 * 1024);
                    }
                    let before = z.total_in();
                    z.compress_vec(input, &mut out, flate2::FlushCompress::Sync)
                        .unwrap();
                    input = &input[(z.total_in() - before) as usize..];
                    if input.is_empty() && out.len() < out.capacity() {
                        break;
                    }
                }
                out
            })
            .collect()
    }

    #[test]
    fn zlib_stream_roundtrip_with_fragments() {
        let big = members_chunk().to_string();
        let msgs = vec![
            json!({"op": 10, "d": {"heartbeat_interval": 41250}}).to_string(),
            big.clone(),
            json!({"op": 11}).to_string(),
            big,
            json!({"op": 0, "t": "MESSAGE_CREATE", "d": {"content": "日本語 ✓"}}).to_string(),
        ];
        let frames = zlib_stream(&msgs);
        assert!(frames.iter().all(|f| f.ends_with(&Inflater::SUFFIX)));
        let mut inf = Inflater::new();
        for (i, (f, m)) in frames.iter().zip(&msgs).enumerate() {
            // 分割受信: 途中までは None、最後の断片で1件そろう
            let parts: Vec<&[u8]> = if i % 2 == 1 {
                f.chunks(f.len() / 3 + 1).collect()
            } else {
                vec![&f[..]]
            };
            for (k, part) in parts.iter().enumerate() {
                let r = inf.push(part).unwrap().map(String::from);
                if k + 1 < parts.len() {
                    assert!(r.is_none());
                } else {
                    assert_eq!(r.as_deref(), Some(m.as_str()));
                }
            }
        }
        // 大きいメッセージの後は作業領域を取り直している
        assert!(inf.buf.capacity() <= Inflater::KEEP && inf.out.capacity() <= Inflater::KEEP * 4);
        // 壊れたデータはエラー（呼び出し側で再接続する）
        let mut inf = Inflater::new();
        assert!(inf.push(&[1, 2, 3, 4, 0, 0, 0xff, 0xff]).is_err());
    }

    #[test]
    fn decode_frame_text_and_binary() {
        let mut inf = Inflater::new();
        let v = decode_frame(&WsMsg::Text(r#"{"op":11}"#.into()), &mut inf)
            .unwrap()
            .unwrap();
        assert_eq!(v["op"], 11);
        let f = zlib_stream(&[r#"{"op":10,"d":{"heartbeat_interval":5}}"#.to_string()]).remove(0);
        let v = decode_frame(&WsMsg::Binary(f.into()), &mut inf)
            .unwrap()
            .unwrap();
        assert_eq!(v["d"]["heartbeat_interval"], 5);
        assert!(decode_frame(&WsMsg::Ping(Vec::new().into()), &mut inf)
            .unwrap()
            .is_none());
    }

    #[test]
    fn decode_frame_releases_large_output_and_keeps_stream_context() {
        let text = json!({"op": 0, "d": "x".repeat(2_000_000)}).to_string();
        let frames = zlib_stream(&[text, r#"{"op":11}"#.into()]);
        let mut inflater = Inflater::new();
        let first = decode_frame(&WsMsg::Binary(frames[0].clone().into()), &mut inflater)
            .unwrap()
            .unwrap();
        assert_eq!(first["d"].as_str().unwrap().len(), 2_000_000);
        assert_eq!(inflater.out.capacity(), 0);
        let next = decode_frame(&WsMsg::Binary(frames[1].clone().into()), &mut inflater)
            .unwrap()
            .unwrap();
        assert_eq!(next["op"], 11);
    }

    #[test]
    fn decode_frame_releases_large_invalid_json_and_continues() {
        let frames = zlib_stream(&["x".repeat(2_000_000), r#"{"op":11}"#.into()]);
        let mut inflater = Inflater::new();
        assert!(
            decode_frame(&WsMsg::Binary(frames[0].clone().into()), &mut inflater)
                .unwrap()
                .is_none()
        );
        assert_eq!(inflater.out.capacity(), 0);
        let next = decode_frame(&WsMsg::Binary(frames[1].clone().into()), &mut inflater)
            .unwrap()
            .unwrap();
        assert_eq!(next["op"], 11);
    }

    #[test]
    fn zlib_stream_decodes_independent_python_vectors() {
        // Python zlib.compressobj(), Z_SYNC_FLUSH; second packet reuses the first context.
        let first = [
            120, 156, 170, 86, 202, 47, 80, 178, 50, 52, 208, 81, 74, 81, 178, 170, 86, 202, 72,
            77, 44, 42, 73, 74, 77, 44, 137, 207, 204, 43, 73, 45, 42, 75, 204, 81, 178, 50, 49,
            53, 48, 48, 168, 173, 5, 0, 0, 0, 255, 255,
        ];
        let second = [170, 134, 168, 52, 172, 5, 0, 0, 0, 255, 255];
        for _ in 0..2 {
            let mut inf = Inflater::new();
            for (packet, expected) in [
                (&first[..], r#"{"op":10,"d":{"heartbeat_interval":45000}}"#),
                (&second[..], r#"{"op":11}"#),
            ] {
                for (i, byte) in packet.iter().enumerate() {
                    let got = inf.push(std::slice::from_ref(byte)).unwrap();
                    if i + 1 == packet.len() {
                        assert_eq!(got, Some(expected));
                    } else {
                        assert!(got.is_none());
                    }
                }
            }
        }
    }

    #[test]
    fn compressed_websocket_frames_work_after_reconnect() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async {
            for _ in 0..2 {
                let (client_io, server_io) = tokio::io::duplex(4096);
                let mut client = tokio_tungstenite::WebSocketStream::from_raw_socket(
                    client_io, tokio_tungstenite::tungstenite::protocol::Role::Client, None,
                ).await;
                let mut server = tokio_tungstenite::WebSocketStream::from_raw_socket(
                    server_io, tokio_tungstenite::tungstenite::protocol::Role::Server, None,
                ).await;
                let frames = zlib_stream(&[
                    r#"{"op":10,"d":{"heartbeat_interval":45000}}"#.into(),
                    r#"{"op":0,"t":"MESSAGE_UPDATE","d":{"channel_id":"1","id":"2","content":"test"}}"#.into(),
                ]);
                let split = frames[0].len() - 2;
                server.send(WsMsg::Binary(frames[0][..split].to_vec().into())).await.unwrap();
                server.send(WsMsg::Binary(frames[0][split..].to_vec().into())).await.unwrap();
                server.send(WsMsg::Binary(frames[1].clone().into())).await.unwrap();
                let mut inf = Inflater::new();
                assert!(decode_frame(&client.next().await.unwrap().unwrap(), &mut inf).unwrap().is_none());
                let hello = decode_frame(&client.next().await.unwrap().unwrap(), &mut inf).unwrap().unwrap();
                assert_eq!(hello["d"]["heartbeat_interval"], 45000);
                let event = decode_frame(&client.next().await.unwrap().unwrap(), &mut inf).unwrap().unwrap();
                let (kind, data) = split_payload(event);
                let s = sink();
                dispatch(&kind, data, &s.tx, &s.v_tx);
                assert!(matches!(s.rx.try_recv().unwrap(), Event::MessageUpdate { content: Some(c), .. } if c == "test"));
            }
        });
    }

    #[test]
    fn zlib_stream_enforces_compressed_and_expanded_limits() {
        let text = "a".repeat(128);
        let packet = zlib_stream(&[text.clone()]).remove(0);
        let mut exact = Inflater::new();
        assert_eq!(
            exact.push_with_limit(&packet, 128).unwrap(),
            Some(text.as_str())
        );
        let mut expanded = Inflater::new();
        assert!(expanded.push_with_limit(&packet, 127).is_err());
        let mut compressed = Inflater::new();
        assert!(compressed.push_with_limit(&[1; 8], 10).unwrap().is_none());
        assert!(compressed.push_with_limit(&[2; 3], 10).is_err());
    }

    #[test]
    fn zlib_stream_continues_after_output_buffer_boundaries() {
        let messages: Vec<String> = [65_535, 65_536, 65_537, 131_072, 1]
            .into_iter()
            .map(|len| "x".repeat(len))
            .collect();
        let packets = zlib_stream(&messages);
        let mut inflater = Inflater::new();
        for (packet, expected) in packets.iter().zip(&messages) {
            assert_eq!(inflater.push(packet).unwrap(), Some(expected.as_str()));
        }
    }

    #[test]
    fn zlib_stream_releases_large_input_buffer_after_decoding() {
        let mut seed = 0x12345678u32;
        let text: String = (0..2_500_000)
            .map(|_| {
                seed ^= seed << 13;
                seed ^= seed >> 17;
                seed ^= seed << 5;
                char::from(b'!' + (seed % 90) as u8)
            })
            .collect();
        let packet = zlib_stream(std::slice::from_ref(&text)).remove(0);
        assert!(packet.len() > Inflater::KEEP);
        let mut inflater = Inflater::new();
        assert_eq!(inflater.push(&packet).unwrap(), Some(text.as_str()));
        assert_eq!(inflater.buf.capacity(), 0);
    }

    #[test]
    #[ignore]
    fn zlib_stream_actual_size_limit() {
        let limit = Inflater::MAX_MESSAGE;
        let exact = "x".repeat(limit);
        let packets = zlib_stream(&[exact, "next".into()]);
        let mut inflater = Inflater::new();
        assert_eq!(inflater.push(&packets[0]).unwrap().unwrap().len(), limit);
        assert_eq!(inflater.push(&packets[1]).unwrap(), Some("next"));
        drop(inflater);
        drop(packets);
        let over = "x".repeat(limit + 1);
        let packet = zlib_stream(std::slice::from_ref(&over)).remove(0);
        let mut inflater = Inflater::new();
        assert!(inflater.push(&packet).is_err());
    }

    #[test]
    #[ignore]
    fn bench_zlib_limit_allocation() {
        let limit = 2 << 20;
        let packet = zlib_stream(&["a".repeat(limit + 1)]).remove(0);
        let mut inf = Inflater::new();
        assert!(inf.push_with_limit(&packet, limit).is_err());
        println!("limit={limit}, output_capacity={}", inf.out.capacity());
    }

    /// 本物のゲートウェイに（トークン無しで）つなぎ、圧縮された Hello を展開できるか確かめる。identify は送らない
    /// cargo test --release live_gateway_hello -- --ignored --nocapture
    #[test]
    #[ignore]
    fn live_gateway_hello() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async {
            let (ws, _) = tokio_tungstenite::connect_async(GATEWAY).await.unwrap();
            let (mut write, mut read) = ws.split();
            let mut inf = Inflater::new();
            let mut binary = false;
            let hello = loop {
                let m = read.next().await.unwrap().unwrap();
                binary |= matches!(m, WsMsg::Binary(_));
                if let Some(v) = decode_frame(&m, &mut inf).unwrap() {
                    break v;
                }
            };
            let _ = write.send(WsMsg::Close(None)).await;
            println!("LIVE binary={binary} hello={hello}");
            assert!(
                binary && hello["op"] == 10 && hello["d"]["heartbeat_interval"].as_u64().is_some()
            );
        });
    }

    /// 圧縮率と展開にかかる時間
    #[test]
    #[ignore]
    fn bench_zlib_stream() {
        let text = members_chunk().to_string();
        let frames = zlib_stream(&vec![text.clone(); 20]);
        let comp: usize = frames.iter().map(Vec::len).sum();
        let mut inf = Inflater::new();
        let t = std::time::Instant::now();
        for f in &frames {
            assert_eq!(inf.push(f).unwrap().unwrap().len(), text.len());
        }
        println!(
            "ZLIB {} KB -> {} KB/件 ({:.1}%), 展開 {:.3} ms/件",
            text.len() / 1024,
            comp / 20 / 1024,
            comp as f64 / 20.0 / text.len() as f64 * 100.0,
            t.elapsed().as_secs_f64() * 1000.0 / 20.0
        );
    }

    #[test]
    #[ignore]
    fn bench_large_decode_buffer_release() {
        let large = json!({"op": 0, "d": "x".repeat(2_000_000)}).to_string();
        let messages: Vec<String> = (0..20)
            .flat_map(|_| [large.clone(), r#"{"op":11}"#.into()])
            .collect();
        let frames: Vec<WsMsg> = zlib_stream(&messages)
            .into_iter()
            .map(|f| WsMsg::Binary(f.into()))
            .collect();
        for early_release in [false, true, true, false] {
            let mut inf = Inflater::new();
            let start = std::time::Instant::now();
            let mut retained = 0;
            for (i, frame) in frames.iter().enumerate() {
                let value: Value = if early_release {
                    decode_frame(frame, &mut inf).unwrap().unwrap()
                } else if let WsMsg::Binary(bytes) = frame {
                    serde_json::from_str(inf.push(bytes).unwrap().unwrap()).unwrap()
                } else {
                    unreachable!()
                };
                std::hint::black_box(value);
                if i % 2 == 0 {
                    retained = inf.out.capacity();
                }
            }
            println!(
                "{}: {:.3} ms/pair, retained after large={} bytes",
                if early_release { "early" } else { "previous" },
                start.elapsed().as_secs_f64() * 50.0,
                retained
            );
        }
    }

    #[test]
    #[ignore]
    fn bench_gzip_json() {
        use std::io::{Read, Write};
        let input = members_chunk().to_string();
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        encoder.write_all(input.as_bytes()).unwrap();
        let gzip = encoder.finish().unwrap();
        let plain_start = std::time::Instant::now();
        for _ in 0..200 {
            std::hint::black_box(serde_json::from_slice::<Value>(input.as_bytes()).unwrap());
        }
        let plain_ms = plain_start.elapsed().as_secs_f64() * 5.0;
        let start = std::time::Instant::now();
        for _ in 0..200 {
            let mut output = Vec::new();
            flate2::read::GzDecoder::new(gzip.as_slice())
                .read_to_end(&mut output)
                .unwrap();
            assert_eq!(output, input.as_bytes());
            std::hint::black_box(serde_json::from_slice::<Value>(&output).unwrap());
        }
        println!(
            "gzip JSON: {} -> {} bytes, plain parse {:.3} ms, decode+parse {:.3} ms",
            input.len(),
            gzip.len(),
            plain_ms,
            start.elapsed().as_secs_f64() * 5.0
        );
    }

    /// 保存済みトークンで実際の REST を測る。gzip なし／ありのクライアントを交互に呼んで同じ条件で比べる
    /// cargo test --release live_rest_bench -- --ignored --nocapture
    #[test]
    #[ignore]
    fn live_rest_bench() {
        let token =
            std::fs::read_to_string(dirs::config_dir().unwrap().join("riscord").join("token"))
                .unwrap()
                .trim()
                .to_string();
        let ua = "Mozilla/5.0 (X11; Linux x86_64) riscord/0.1";
        let plain = Api {
            base_url: None,
            http: reqwest::Client::builder()
                .user_agent(ua)
                .no_gzip()
                .build()
                .unwrap(),
            token: token.clone(),
            roles: Default::default(),
            guild_perms: Default::default(),
            limits: Default::default(),
        };
        let gzip = Api {
            base_url: None,
            http: reqwest::Client::builder().user_agent(ua).build().unwrap(),
            token,
            roles: Default::default(),
            guild_perms: Default::default(),
            limits: Default::default(),
        };
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async {
            let guilds: Vec<Guild> = gzip.get("/users/@me/guilds").await.unwrap();
            let dms: Vec<Channel> = gzip.get("/users/@me/channels").await.unwrap();
            let g = &guilds[0].id;
            let dm = dms.iter().max_by_key(|c| c.last_id()).unwrap().id.clone();
            let paths = [
                ("guilds", "/users/@me/guilds".to_string()),
                ("dms", "/users/@me/channels".to_string()),
                ("messages(50)", format!("/channels/{dm}/messages?limit=50")),
                ("guild channels", format!("/guilds/{g}/channels")),
            ];
            let stat = |v: &mut Vec<f64>| {
                v.sort_by(|a, b| a.partial_cmp(b).unwrap());
                let n = v.len();
                format!(
                    "平均 {:6.1} p50 {:6.1} p95 {:6.1} p99 {:6.1}",
                    v.iter().sum::<f64>() / n as f64,
                    v[n / 2],
                    v[(n * 95 / 100).min(n - 1)],
                    v[(n * 99 / 100).min(n - 1)]
                )
            };
            let (mut all_p, mut all_g) = (Vec::new(), Vec::new());
            for (name, path) in &paths {
                let (mut vp, mut vg) = (Vec::new(), Vec::new());
                for i in 0..20 {
                    let order: [(&Api, bool); 2] = if i % 2 == 0 {
                        [(&plain, false), (&gzip, true)]
                    } else {
                        [(&gzip, true), (&plain, false)]
                    };
                    for (api, gz) in order {
                        let t = std::time::Instant::now();
                        let r = api.request(reqwest::Method::GET, path, None).await.unwrap();
                        let _ = r.bytes().await.unwrap();
                        let ms = t.elapsed().as_secs_f64() * 1000.0;
                        if gz {
                            vg.push(ms)
                        } else {
                            vp.push(ms)
                        }
                        tokio::time::sleep(Duration::from_millis(300)).await;
                    }
                }
                all_p.extend(vp.iter().copied());
                all_g.extend(vg.iter().copied());
                println!(
                    "REST {name:<15} gzipなし {}\n     {:<15} gzipあり {}",
                    stat(&mut vp),
                    "",
                    stat(&mut vg)
                );
            }
            println!("REST 全体 gzipなし n={} {}", all_p.len(), stat(&mut all_p));
            println!("REST 全体 gzipあり n={} {}", all_g.len(), stat(&mut all_g));
        });
    }

    /// 保存済みトークンで identify し、READY までの時間と受信バイト数を測る（オフライン表示で接続）
    async fn ready_once(url: &str, token: &str) -> (f64, usize, usize, f64) {
        let t0 = std::time::Instant::now();
        let (ws, _) = tokio_tungstenite::connect_async(url).await.unwrap();
        let (mut write, mut read) = ws.split();
        let mut inf = Inflater::new();
        let (mut wire, mut json_bytes, mut parse_ms) = (0usize, 0usize, 0.0);
        let mut identified = false;
        loop {
            let m = read.next().await.unwrap().unwrap();
            wire += m.len();
            let tp = std::time::Instant::now();
            let Some(v) = decode_frame(&m, &mut inf).unwrap() else {
                continue;
            };
            parse_ms += tp.elapsed().as_secs_f64() * 1000.0;
            json_bytes += match &m {
                WsMsg::Text(t) => t.len(),
                _ => inf.out.len(),
            };
            if !identified {
                let identify = json!({ "op": 2, "d": { "token": token, "properties": { "os": "Linux", "browser": "Discord Client", "device": "" },
                    "presence": { "status": "invisible", "since": 0, "activities": [], "afk": false }, "compress": false } });
                write
                    .send(WsMsg::Text(identify.to_string().into()))
                    .await
                    .unwrap();
                identified = true;
            }
            if v["t"] == "READY" {
                let ms = t0.elapsed().as_secs_f64() * 1000.0;
                let _ = write.send(WsMsg::Close(None)).await;
                return (ms, wire, json_bytes, parse_ms);
            }
        }
    }

    /// cargo test --release live_gateway_ready -- --ignored --nocapture
    #[test]
    #[ignore]
    fn live_gateway_ready() {
        let token =
            std::fs::read_to_string(dirs::config_dir().unwrap().join("riscord").join("token"))
                .unwrap()
                .trim()
                .to_string();
        let plain_url = "wss://gateway.discord.gg/?v=10&encoding=json";
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async {
            for i in 0..2 {
                for (name, url) in if i == 0 { [("圧縮なし", plain_url), ("zlib-stream", GATEWAY)] } else { [("zlib-stream", GATEWAY), ("圧縮なし", plain_url)] } {
                    let (ms, wire, json_bytes, parse) = ready_once(url, &token).await;
                    println!("READY {name:<11} 接続〜READY {ms:7.1} ms  受信 {:6} KB (JSON {:6} KB)  展開+解析 {parse:.1} ms", wire / 1024, json_bytes / 1024);
                    tokio::time::sleep(Duration::from_secs(6)).await;
                }
            }
        });
    }

    #[test]
    fn role_cache_expires() {
        let api = Api {
            base_url: None,
            http: reqwest::Client::new(),
            token: String::new(),
            roles: Default::default(),
            guild_perms: Default::default(),
            limits: Default::default(),
        };
        let role = Role {
            id: "1".into(),
            name: "mod".into(),
            color: 0xff0000,
            position: 3,
            hoist: false,
            managed: Some(false),
            permissions: String::new(),
        };
        assert!(api.cached_roles("g").is_none());
        api.store_roles("g", std::slice::from_ref(&role));
        assert_eq!(api.cached_roles("g").unwrap()[0].name, "mod");
        // 期限切れは使わない
        api.roles.lock().unwrap().get_mut("g").unwrap().0 -=
            ROLE_CACHE_TTL + Duration::from_secs(1);
        assert!(api.cached_roles("g").is_none());
    }

    #[test]
    fn role_cache_keeps_guilds_separate_and_replaces_entries() {
        let api = Api {
            base_url: None,
            http: reqwest::Client::new(),
            token: String::new(),
            roles: Default::default(),
            guild_perms: Default::default(),
            limits: Default::default(),
        };
        let role = |name: &str| Role {
            id: "1".into(),
            name: name.into(),
            color: 0,
            position: 0,
            hoist: false,
            managed: Some(false),
            permissions: String::new(),
        };
        api.store_roles("a", &[role("old")]);
        api.store_roles("b", &[role("other")]);
        api.store_roles("a", &[role("new")]);
        assert_eq!(api.cached_roles("a").unwrap()[0].name, "new");
        assert_eq!(api.cached_roles("b").unwrap()[0].name, "other");
        assert!(api.cached_roles("missing").is_none());
    }

    #[test]
    fn members_chunk_dispatch() {
        let s = sink();
        let v = members_chunk();
        let (t, d) = split_payload(v);
        dispatch(&t, d, &s.tx, &s.v_tx);
        let evs: Vec<Event> = s.rx.try_iter().collect();
        assert!(
            matches!(&evs[0], Event::GuildMembers(g, m) if g == "42" && m.len() == 1000 && m[999].user.as_ref().unwrap().id == "10999")
        );
        assert!(
            matches!(&evs[1], Event::Presences(p) if p.len() == 1000 && p[0].1.activity.as_deref() == Some("プレイ中: game"))
        );
    }

    #[test]
    fn message_update_dispatch() {
        let s = sink();
        let v = json!({ "op": 0, "t": "MESSAGE_UPDATE", "d": { "channel_id": "1", "id": "2", "content": "x", "embeds": [{ "type": "image", "url": "u" }] } });
        let (t, d) = split_payload(v);
        dispatch(&t, d, &s.tx, &s.v_tx);
        match s.rx.try_recv().unwrap() {
            Event::MessageUpdate {
                channel_id,
                id,
                content,
                embeds,
                ..
            } => {
                assert_eq!(
                    (channel_id.as_str(), id.as_str(), content.as_deref()),
                    ("1", "2", Some("x"))
                );
                assert_eq!(embeds.unwrap()[0].url.as_deref(), Some("u"));
            }
            _ => panic!(),
        }
        // 配列など想定外の形でも落ちない
        let (t, d) = split_payload(json!([1, 2]));
        assert!(t.is_empty() && d.is_null());
    }

    #[test]
    fn split_payload_matches_previous_behavior() {
        for value in [
            json!({"t": "READY", "d": {"guilds": [1, 2]}}),
            json!({"t": 123, "d": null}),
            json!({"t": "X"}),
            json!({"d": [1, 2]}),
            json!(null),
            json!([1, 2]),
        ] {
            let expected = (
                value["t"].as_str().unwrap_or("").to_string(),
                value["d"].clone(),
            );
            assert_eq!(split_payload(value), expected);
        }
    }

    /// cargo test --release bench_dispatch -- --ignored --nocapture
    #[test]
    #[ignore]
    fn bench_dispatch() {
        let s = sink();
        let text = members_chunk().to_string();
        let n = 200;
        let mut samples = Vec::with_capacity(n);
        let t0 = std::time::Instant::now();
        for _ in 0..n {
            let start = std::time::Instant::now();
            let v: Value = serde_json::from_str(&text).unwrap();
            let (t, d) = split_payload(v);
            dispatch(&t, d, &s.tx, &s.v_tx);
            s.rx.try_iter().for_each(drop);
            samples.push(start.elapsed().as_secs_f64() * 1000.0);
        }
        let elapsed = t0.elapsed().as_secs_f64();
        samples.sort_by(f64::total_cmp);
        println!("GUILD_MEMBERS_CHUNK ({} KB) parse+dispatch: mean {:.3} ms, p95 {:.3} ms, p99 {:.3} ms, {:.1} events/s", text.len() / 1024, elapsed * 1000.0 / n as f64, samples[n * 95 / 100 - 1], samples[n * 99 / 100 - 1], n as f64 / elapsed);
    }

    /// 比べるために、dispatch が出すイベントと音声タスクへの転送を文字列にする
    #[test]
    fn member_list_subscribes_by_range_and_parses_updates() {
        let sub = member_subscription("10", "20", &[[0, 99], [100, 199]]);
        assert_eq!(sub["op"], 37, "全員を取る op 8 ではなく、範囲の購読");
        assert_eq!(
            sub["d"]["subscriptions"]["10"]["channels"]["20"],
            json!([[0, 99], [100, 199]])
        );
        let s = sink();
        let d = json!({
            "guild_id": "10", "id": "everyone", "member_count": 3, "online_count": 2,
            "groups": [{ "id": "online", "count": 2 }, { "id": "offline", "count": 1 }],
            "ops": [
                { "op": "SYNC", "range": [0, 99], "items": [
                    { "group": { "id": "online", "count": 2 } },
                    { "member": { "user": { "id": "1", "username": "a" }, "roles": [], "presence": { "user": { "id": "1" }, "status": "online", "activities": [] } } },
                    { "member": { "user": { "id": "2", "username": "b" }, "roles": [] } },
                    { "group": { "id": "offline", "count": 1 } },
                ] },
                { "op": "DELETE", "index": 2 },
                { "op": "INVALIDATE", "range": [100, 199] },
            ]
        });
        dispatch("GUILD_MEMBER_LIST_UPDATE", d, &s.tx, &s.v_tx);
        let events: Vec<Event> = s.rx.try_iter().collect();
        assert!(events
            .iter()
            .any(|e| matches!(e, Event::GuildMembers(g, m) if g == "10" && m.len() == 2)));
        assert!(events
            .iter()
            .any(|e| matches!(e, Event::Presences(p) if p.len() == 1)));
        let Some(Event::MemberList {
            guild_id,
            list_id,
            groups,
            ops,
        }) = events
            .into_iter()
            .find(|e| matches!(e, Event::MemberList { .. }))
        else {
            panic!("一覧の差分")
        };
        assert_eq!((guild_id.as_str(), list_id.as_str()), ("10", "everyone"));
        assert_eq!(
            groups,
            vec![("online".to_string(), 2), ("offline".to_string(), 1)]
        );
        assert_eq!(
            ops[0],
            ListOp::Sync {
                range: [0, 99],
                items: vec![
                    ListItem::Group("online".into(), 2),
                    ListItem::Member("1".into()),
                    ListItem::Member("2".into()),
                    ListItem::Group("offline".into(), 1),
                ]
            }
        );
        assert_eq!(ops[1], ListOp::Delete { index: 2 });
        assert_eq!(ops[2], ListOp::Invalidate { range: [100, 199] });
    }

    #[test]
    fn reconnect_backs_off_and_stops_on_fatal_codes() {
        // 1, 2, 4 … 秒と延び、60 秒で止まる（揺らぎは 1 秒未満）
        for (n, secs) in [(0, 1), (1, 2), (2, 4), (5, 32), (6, 60), (20, 60)] {
            let d = reconnect_delay(n).as_millis() as u64;
            assert!(
                d >= secs * 1000 && d < secs * 1000 + 1000,
                "{n} 回目: {d}ms"
            );
        }
        assert!(
            matches!(close_action(4004), GatewayEnd::Fatal(_)),
            "トークン無効はやめる"
        );
        assert!(matches!(close_action(4014), GatewayEnd::Fatal(_)));
        assert_eq!(
            close_action(4009),
            GatewayEnd::Reidentify,
            "セッション期限切れは一から"
        );
        assert_eq!(close_action(4007), GatewayEnd::Reidentify);
        assert_eq!(close_action(4000), GatewayEnd::Resume, "その他は再開を試す");
        assert_eq!(close_action(1006), GatewayEnd::Resume);
    }

    #[test]
    fn gateway_rejects_zero_heartbeat_interval() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let base = format!("ws://{}", listener.local_addr().unwrap());
            let server = tokio::spawn(async move {
                let (sock, _) = listener.accept().await.unwrap();
                let mut ws = tokio_tungstenite::accept_async(sock).await.unwrap();
                ws.send(WsMsg::Text(
                    r#"{"op":10,"d":{"heartbeat_interval":0}}"#.into(),
                ))
                .await
                .unwrap();
            });
            let s = sink();
            let (_out_tx, mut out) = unbounded_channel();
            let status = Mutex::new(MyPresence {
                status: "online".into(),
                activity: true,
            });
            let mut session = GatewaySession::default();
            let mut ready = false;
            let result = gateway_once_at(
                &base,
                "tok",
                &s.tx,
                &mut out,
                &status,
                &s.v_tx,
                &mut session,
                &mut ready,
            )
            .await;
            assert!(result.is_err());
            assert!(!ready);
            server.await.unwrap();
        });
    }

    #[test]
    fn gateway_resumes_session_and_handles_invalid_session() {
        // 本物の代わりに手元で WebSocket サーバーを立て、READY → 切断 → RESUME の流れを確かめる
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let base = format!("ws://{}", listener.local_addr().unwrap());
            let server = tokio::spawn(async move {
                let mut firsts = Vec::new();
                for step in 0..2 {
                    let (sock, _) = listener.accept().await.unwrap();
                    let mut ws = tokio_tungstenite::accept_async(sock).await.unwrap();
                    ws.send(WsMsg::Text(r#"{"op":10,"d":{"heartbeat_interval":45000}}"#.into())).await.unwrap();
                    let first: Value = match ws.next().await.unwrap().unwrap() { WsMsg::Text(t) => serde_json::from_str(&t).unwrap(), m => panic!("{m:?}") };
                    firsts.push(first);
                    if step == 0 {
                        let ready = json!({ "op": 0, "s": 5, "t": "READY", "d": { "session_id": "sess", "resume_gateway_url": "ws://unused", "guilds": [] } });
                        ws.send(WsMsg::Text(ready.to_string().into())).await.unwrap();
                        // Discord から再接続を頼む
                        ws.send(WsMsg::Text(r#"{"op":7,"d":null}"#.into())).await.unwrap();
                    } else {
                        ws.send(WsMsg::Text(r#"{"op":0,"s":6,"t":"RESUMED","d":{}}"#.into())).await.unwrap();
                        ws.send(WsMsg::Text(r#"{"op":9,"d":false}"#.into())).await.unwrap();
                    }
                    let _ = ws.next().await;
                }
                firsts
            });
            let s = sink();
            let (_out_tx, mut out) = unbounded_channel();
            let status = Mutex::new(MyPresence { status: "online".into(), activity: true });
            let mut session = GatewaySession::default();
            // 1回目: identify して READY を受け、op 7 で「再開」
            let mut ready = false;
            let end = gateway_once_at(&base, "tok", &s.tx, &mut out, &status, &s.v_tx, &mut session, &mut ready).await.unwrap();
            assert_eq!(end, GatewayEnd::Resume);
            assert!(ready);
            assert_eq!((session.id.as_deref(), session.seq), (Some("sess"), Some(5)));
            // 2回目: RESUME を送り、op 9 (false) で「一から」
            let mut ready = false;
            session.resume_url = Some(base.clone());
            let end = gateway_once_at(&base, "tok", &s.tx, &mut out, &status, &s.v_tx, &mut session, &mut ready).await.unwrap();
            assert_eq!(end, GatewayEnd::Reidentify);
            assert!(ready, "RESUMED でつながった扱い");
            let firsts = server.await.unwrap();
            assert_eq!(firsts[0]["op"], 2, "最初は identify");
            assert_eq!(firsts[0]["d"]["presence"]["activities"][0]["name"], "Riscord");
            assert_eq!(firsts[1]["op"], 6, "再接続は RESUME");
            assert_eq!(firsts[1]["d"]["session_id"], "sess");
            assert_eq!(firsts[1]["d"]["seq"], 5);
        });
    }

    #[test]
    fn presence_shows_riscord_activity_only_when_enabled() {
        let on = presence_payload(&MyPresence {
            status: "idle".into(),
            activity: true,
        });
        assert_eq!(on["op"], 3);
        assert_eq!(on["d"]["status"], "idle");
        assert_eq!(on["d"]["activities"][0]["name"], "Riscord");
        assert_eq!(on["d"]["activities"][0]["type"], 0, "プレイ中");
        assert!(
            on["d"]["activities"][0]["timestamps"]["start"]
                .as_u64()
                .unwrap()
                > 0
        );
        let again = presence_payload(&MyPresence {
            status: "online".into(),
            activity: true,
        });
        assert_eq!(
            again["d"]["activities"][0]["timestamps"], on["d"]["activities"][0]["timestamps"],
            "開始時刻は変わらない"
        );
        let off = presence_payload(&MyPresence {
            status: "online".into(),
            activity: false,
        });
        assert_eq!(off["d"]["activities"], json!([]));
    }

    #[test]
    fn profile_errors_are_explained() {
        let err = |status: u16, body: &str| {
            anyhow::Error::new(HttpError {
                status,
                text: format!("PATCH /users/@me: {status} {body}"),
            })
        };
        assert!(
            profile_error_text(&err(400, r#"{"captcha_key": ["captcha-required"]}"#))
                .contains("CAPTCHA")
        );
        assert!(profile_error_text(&err(
            400,
            r#"{"code": 50035, "errors": {"avatar": {"_errors": [{"code": "AVATAR_RATE_LIMIT"}]}}}"#
        ))
        .contains("アバターの変更が多すぎます"));
        assert!(
            profile_error_text(&err(400, r#"{"code": 50035, "errors": {"bio": {}}}"#))
                .contains("受け付けてもらえません")
        );
        assert!(profile_error_text(&anyhow::anyhow!("timeout")).contains("通信状態"));
    }

    #[test]
    fn compose_rejections_are_explained() {
        let err = |status: u16, body: &str| {
            anyhow::Error::new(HttpError {
                status,
                text: format!("POST /channels/1/messages: {status} {body}"),
            })
        };
        assert_eq!(
            compose_reject_reason(&err(413, "")).as_deref(),
            Some("ファイルが大きすぎます")
        );
        assert_eq!(compose_reject_reason(&err(400, r#"{"code": 50035, "errors": {"content": {"_errors": [{"code": "BASE_TYPE_MAX_LENGTH"}]}}}"#)).as_deref(), Some("本文が長すぎます"));
        assert_eq!(
            compose_reject_reason(&err(403, r#"{"code": 50013}"#)).as_deref(),
            Some("このチャンネルで送信する権限がありません")
        );
        assert!(
            compose_reject_reason(&err(500, "")).is_none(),
            "サーバー側の失敗は届いたか分からない"
        );
        assert!(compose_reject_reason(&anyhow::anyhow!("timeout")).is_none());
    }

    fn describe(e: &Event) -> String {
        match e {
            Event::Status(s) => format!("Status({s})"),
            Event::VoiceStates(v) => format!("VoiceStates({v:?})"),
            Event::Presences(p) => format!("Presences({p:?})"),
            Event::GuildMembers(g, m) => format!("GuildMembers({g}, {m:?})"),
            Event::Call {
                channel_id,
                ringing,
            } => format!("Call({channel_id}, {ringing:?})"),
            Event::CallEnded(c) => format!("CallEnded({c})"),
            Event::MessageCreate(m) => format!("MessageCreate({m:?})"),
            Event::MessageUpdate {
                channel_id,
                id,
                content,
                embeds,
                ..
            } => format!("MessageUpdate({channel_id}, {id}, {content:?}, {embeds:?})"),
            Event::MessageDelete { channel_id, id } => format!("MessageDelete({channel_id}, {id})"),
            Event::AuthFailed => "AuthFailed".into(),
            _ => "other".into(),
        }
    }

    /// 1件のフレーム文字列を、従来（全体を Value にして dispatch）と dispatch_raw の両方で処理した結果
    #[test]
    fn ready_extracts_my_roles_and_channel_guilds() {
        let (tx, rx) = std::sync::mpsc::channel();
        let (v_tx, _v_rx) = unbounded_channel();
        let tx = Tx {
            tx,
            ctx: crate::wake::Waker::default(),
        };
        let ready = json!({
            "user": { "id": "me" },
            "guilds": [
                { "id": "g1", "channels": [{ "id": "c1", "name": "x" }], "threads": [{ "id": "t1" }] },
                { "id": "g2", "channels": [], "members": [{ "user": { "id": "me" }, "roles": ["r2"] }] },
            ],
            "merged_members": [[{ "user_id": "me", "roles": ["r1"] }], []],
            "read_state": { "entries": [{ "id": "c1", "mention_count": 1 }] },
        });
        let raw = serde_json::value::RawValue::from_string(ready.to_string()).unwrap();
        dispatch_raw("READY", Some(&raw), &tx, &v_tx);
        let extra = rx
            .try_iter()
            .find_map(|e| match e {
                Event::ReadyExtra(v) => Some(v),
                _ => None,
            })
            .unwrap();
        assert_eq!(extra["my_roles"], json!({ "g1": ["r1"], "g2": ["r2"] }));
        assert_eq!(extra["channel_guild"], json!({ "c1": "g1", "t1": "g1" }));
        assert_eq!(extra["read_state"]["entries"][0]["mention_count"], 1);
    }

    fn both_paths(text: &str) -> [(Option<i64>, Option<u64>, Vec<String>, Vec<String>); 2] {
        let run = |raw: bool| {
            let (tx, rx) = std::sync::mpsc::channel();
            let (v_tx, mut v_rx) = unbounded_channel();
            let tx = Tx {
                tx,
                ctx: crate::wake::Waker::default(),
            };
            let (seq, op) = if raw {
                let f: GatewayFrame = serde_json::from_str(text).unwrap();
                if f.op.as_u64() == Some(0) {
                    dispatch_raw(f.t.as_str().unwrap_or(""), f.d.as_deref(), &tx, &v_tx);
                }
                (f.s.as_i64(), f.op.as_u64())
            } else {
                let v: Value = serde_json::from_str(text).unwrap();
                let (seq, op) = (v["s"].as_i64(), v["op"].as_u64());
                if op == Some(0) {
                    let (t, d) = split_payload(v);
                    dispatch(&t, d, &tx, &v_tx);
                }
                (seq, op)
            };
            let events = rx.try_iter().map(|e| describe(&e)).collect();
            let mut voice = Vec::new();
            while let Ok(m) = v_rx.try_recv() {
                voice.push(match m {
                    VoiceMsg::State(v) => format!("State({v})"),
                    VoiceMsg::Server(v) => format!("Server({v})"),
                    _ => "other".into(),
                });
            }
            (seq, op, events, voice)
        };
        [run(false), run(true)]
    }

    /// dispatch_raw は従来の「全体を Value にしてから dispatch」と同じイベントを出す
    #[test]
    fn dispatch_raw_matches_value_dispatch() {
        let vs = json!({ "user_id": "7", "channel_id": "300", "session_id": "x", "self_mute": true, "deaf": false, "member": { "user": { "id": "7", "username": "seven" } } });
        let presence = json!({ "user": { "id": "7" }, "status": "idle", "activities": [{ "type": 4, "state": "休憩中", "emoji": { "name": "☕" } }, { "type": 0, "name": "game" }] });
        let guild = |id: Value, voice: Option<Value>| {
            let mut g = json!({ "id": id, "name": "g", "channels": [{ "id": "1", "name": "general", "type": 0, "permission_overwrites": [] }], "roles": [{ "id": "1", "name": "@everyone" }], "emojis": [], "member_count": 5 });
            if let Some(v) = voice {
                g["voice_states"] = v;
            }
            g
        };
        let frames = [
            json!({ "op": 0, "s": 1, "t": "READY", "d": { "v": 10, "user": { "id": "1" }, "guilds": [guild(json!("10"), Some(json!([vs]))), guild(json!("11"), None), guild(Value::Null, Some(json!([vs])))], "presences": [presence], "private_channels": [], "session_id": "s" } }),
            json!({ "op": 0, "s": 2, "t": "READY", "d": { "guilds": [guild(json!("10"), Some(json!([vs])))] } }),
            json!({ "op": 0, "s": 2, "t": "READY", "d": { "presences": [presence] } }),
            // 想定外の形は全体の解析に戻る
            json!({ "op": 0, "s": 3, "t": "READY", "d": { "guilds": null, "presences": [presence] } }),
            json!({ "op": 0, "s": 3, "t": "READY", "d": { "guilds": [null, guild(json!("12"), Some(json!([vs])))], "presences": "x" } }),
            json!({ "op": 0, "s": 3, "t": "READY", "d": { "guilds": [guild(json!(5), Some(json!({ "a": 1 })))] } }),
            json!({ "op": 0, "s": 3, "t": "READY", "d": [1, 2] }),
            json!({ "op": 0, "s": 4, "t": "READY_SUPPLEMENTAL", "d": { "merged_presences": { "friends": [presence], "guilds": [[presence], []] } } }),
            members_chunk(),
            json!({ "op": 0, "s": 5, "t": "PRESENCE_UPDATE", "d": presence }),
            json!({ "op": 0, "s": 6, "t": "VOICE_STATE_UPDATE", "d": vs }),
            json!({ "op": 0, "s": 7, "t": "VOICE_SERVER_UPDATE", "d": { "token": "t", "guild_id": "10", "endpoint": "e" } }),
            json!({ "op": 0, "s": 8, "t": "CALL_CREATE", "d": { "channel_id": "300", "ringing": ["1", "2"], "voice_states": [vs] } }),
            json!({ "op": 0, "s": 9, "t": "CALL_UPDATE", "d": { "channel_id": "300", "ringing": [] } }),
            json!({ "op": 0, "s": 10, "t": "CALL_DELETE", "d": { "channel_id": "300" } }),
            json!({ "op": 0, "s": 11, "t": "MESSAGE_CREATE", "d": { "id": "5", "channel_id": "300", "author": { "id": "7", "username": "seven" }, "content": "やあ <:pog:1>", "timestamp": "2026-11-05T10:00:00+00:00", "attachments": [], "embeds": [], "mentions": [] } }),
            json!({ "op": 0, "s": 12, "t": "MESSAGE_UPDATE", "d": { "channel_id": "300", "id": "5", "content": "x", "embeds": [{ "type": "image", "url": "u" }] } }),
            json!({ "op": 0, "s": 13, "t": "MESSAGE_DELETE", "d": { "channel_id": "300", "id": "5" } }),
            json!({ "op": 0, "s": 14, "t": "MESSAGE_DELETE", "d": null }),
            json!({ "op": 0, "s": 15, "t": "MESSAGE_CREATE" }),
            json!({ "op": 0, "s": 16, "t": "MESSAGE_CREATE", "d": { "id": 5 } }),
            json!({ "op": 0, "s": 17, "t": "TYPING_START", "d": { "channel_id": "300", "user_id": "7" } }),
            json!({ "op": 0, "s": 18, "t": "MESSAGE_REACTION_ADD", "d": { "emoji": { "name": "👍" } } }),
            json!({ "op": 0, "s": 19, "t": null, "d": {} }),
            json!({ "op": 11 }),
            json!({ "op": 10, "d": { "heartbeat_interval": 41250 } }),
            json!({ "op": "0", "s": "20", "t": "MESSAGE_DELETE", "d": { "channel_id": "300", "id": "5" } }),
        ];
        for f in frames {
            let text = f.to_string();
            let [old, new] = both_paths(&text);
            assert_eq!(old, new, "{}", &text[..text.len().min(200)]);
        }
        // 確認用: READY から実際にイベントが出ている
        let [old, _] = both_paths(&json!({ "op": 0, "s": 1, "t": "READY", "d": { "guilds": [guild(json!("10"), Some(json!([vs])))], "presences": [presence] } }).to_string());
        // Status・VoiceStates・Presences と、チャンネル→サーバーの対応（ReadyExtra）
        assert_eq!(old.2.len(), 4, "{:?}", old.2);
    }

    /// 大きい READY（多数のサーバー・チャンネル・ロール・絵文字）と、使わない種類の処理時間
    /// cargo test --release bench_dispatch_raw -- --ignored --nocapture
    #[test]
    #[ignore]
    fn bench_dispatch_raw() {
        let guilds: Vec<Value> = (0..100).map(|g| json!({
            "id": format!("{g}"), "name": format!("サーバー {g}"), "member_count": 1000,
            "channels": (0..50).map(|c| json!({ "id": format!("{g}{c}"), "name": format!("チャンネル{c}"), "type": c % 3, "position": c, "topic": "説明文".repeat(5), "permission_overwrites": [{ "id": "1", "type": 0, "allow": "0", "deny": "1024" }] })).collect::<Vec<_>>(),
            "roles": (0..30).map(|r| json!({ "id": format!("{g}{r}"), "name": format!("role{r}"), "color": r, "position": r, "permissions": "123456" })).collect::<Vec<_>>(),
            "emojis": (0..20).map(|e| json!({ "id": format!("{g}{e}"), "name": format!("emoji{e}"), "animated": false })).collect::<Vec<_>>(),
            "voice_states": [{ "user_id": format!("{g}"), "channel_id": format!("{g}1"), "self_mute": false }],
        })).collect();
        let ready = json!({ "op": 0, "s": 1, "t": "READY", "d": { "guilds": guilds, "presences": [], "private_channels": [] } }).to_string();
        let typing = json!({ "op": 0, "s": 2, "t": "TYPING_START", "d": { "channel_id": "300", "guild_id": "1", "user_id": "7", "timestamp": 1, "member": { "user": { "id": "7", "username": "seven", "avatar": "0123456789abcdef" }, "roles": ["1", "2"], "nick": null, "joined_at": "2024-01-01T00:00:00+00:00" } } }).to_string();
        let chunk = members_chunk().to_string();
        let s = sink();
        for (name, text, n) in [
            ("READY", &ready, 20),
            ("TYPING_START", &typing, 20000),
            ("GUILD_MEMBERS_CHUNK", &chunk, 100),
        ] {
            for raw in [false, true, false, true] {
                let start = std::time::Instant::now();
                for _ in 0..n {
                    if raw {
                        let f: GatewayFrame = serde_json::from_str(text).unwrap();
                        dispatch_raw(f.t.as_str().unwrap_or(""), f.d.as_deref(), &s.tx, &s.v_tx);
                    } else {
                        let v: Value = serde_json::from_str(text).unwrap();
                        let (t, d) = split_payload(v);
                        dispatch(&t, d, &s.tx, &s.v_tx);
                    }
                    s.rx.try_iter().for_each(drop);
                }
                println!(
                    "{name:<20} ({:5} KB) {}: {:.4} ms/件",
                    text.len() / 1024,
                    if raw { "dispatch_raw" } else { "Value全体  " },
                    start.elapsed().as_secs_f64() * 1000.0 / n as f64
                );
            }
        }
    }

    /// 保存済みトークンで（オフライン表示で）つなぎ、届くイベントの種類・量と、受信処理の時間を
    /// 従来（全体を Value に）と dispatch_raw で比べる
    /// LIVE_SECS=60 cargo test --release live_gateway_event_mix -- --ignored --nocapture
    #[test]
    #[ignore]
    fn live_gateway_event_mix() {
        let token =
            std::fs::read_to_string(dirs::config_dir().unwrap().join("riscord").join("token"))
                .unwrap()
                .trim()
                .to_string();
        let secs: u64 = std::env::var("LIVE_SECS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(60);
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let texts: Vec<String> = rt.block_on(async {
            let (ws, _) = tokio_tungstenite::connect_async(GATEWAY).await.unwrap();
            let (mut write, mut read) = ws.split();
            let mut inf = Inflater::new();
            let mut texts = Vec::new();
            let end = tokio::time::Instant::now() + Duration::from_secs(secs);
            let mut identified = false;
            let mut hb = tokio::time::interval(Duration::from_secs(40));
            loop {
                tokio::select! {
                    _ = tokio::time::sleep_until(end) => break,
                    _ = hb.tick(), if identified => { write.send(WsMsg::Text(json!({"op": 1, "d": null}).to_string().into())).await.unwrap(); }
                    m = read.next() => {
                        let m = m.unwrap().unwrap();
                        let text = match &m {
                            WsMsg::Text(t) => t.to_string(),
                            WsMsg::Binary(b) => match inf.push(b).unwrap() { Some(t) => t.to_string(), None => continue },
                            _ => continue,
                        };
                        texts.push(text);
                        if !identified {
                            let identify = json!({ "op": 2, "d": { "token": token, "properties": { "os": "Linux", "browser": "Discord Client", "device": "" },
                                "presence": { "status": "invisible", "since": 0, "activities": [], "afk": false }, "compress": false } });
                            write.send(WsMsg::Text(identify.to_string().into())).await.unwrap();
                            identified = true;
                        }
                    }
                }
            }
            let _ = write.send(WsMsg::Close(None)).await;
            texts
        });
        let mut kinds: std::collections::BTreeMap<String, (usize, usize)> = Default::default();
        for t in &texts {
            let v: Value = serde_json::from_str(t).unwrap();
            let k = v["t"]
                .as_str()
                .map(String::from)
                .unwrap_or_else(|| format!("op{}", v["op"]));
            let e = kinds.entry(k).or_default();
            e.0 += 1;
            e.1 += t.len();
        }
        println!(
            "LIVE {secs} 秒で {} 件・{} KB",
            texts.len(),
            texts.iter().map(String::len).sum::<usize>() / 1024
        );
        for (k, (n, b)) in &kinds {
            println!("  {k:<28} {n:6} 件 {:8} KB 使う={}", b / 1024, handles(k));
        }
        let s = sink();
        for raw in [false, true, false, true] {
            let start = std::time::Instant::now();
            for t in &texts {
                if raw {
                    let f: GatewayFrame = serde_json::from_str(t).unwrap();
                    if f.op.as_u64() == Some(0) {
                        dispatch_raw(f.t.as_str().unwrap_or(""), f.d.as_deref(), &s.tx, &s.v_tx);
                    }
                } else {
                    let v: Value = serde_json::from_str(t).unwrap();
                    if v["op"].as_u64() == Some(0) {
                        let (t, d) = split_payload(v);
                        dispatch(&t, d, &s.tx, &s.v_tx);
                    }
                }
                s.rx.try_iter().for_each(drop);
            }
            println!(
                "LIVE 全件の解析+dispatch {}: {:.2} ms",
                if raw { "dispatch_raw" } else { "Value全体  " },
                start.elapsed().as_secs_f64() * 1000.0
            );
        }
    }
}
