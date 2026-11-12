use std::time::Duration;

use anyhow::{bail, Result};
use reqwest::{header::AUTHORIZATION, StatusCode};
use serde::de::DeserializeOwned;
use serde_json::{json, Value};
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};

use crate::model::{self, Channel, Embed, Guild, Member, Message, Presence, ProfileData, Role, User, VoiceState};
use crate::voice::VoiceCtl;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

mod inflate;
mod rest;
mod gateway;
mod voice;
pub use rest::ApiCall;
#[allow(unused_imports)]
pub use rest::encode;
#[allow(unused_imports)]
pub use rest::{call_connect, request_guild_members, stream_create, stream_delete, stream_set_paused, stream_watch};

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

use gateway::*;
use voice::*;


pub enum Cmd {
    LoadGuildChannels(String),
    /// メンバー一覧のうち、見えている範囲だけを購読する（公式クライアントと同じ op 37）。
    /// ranges は [先頭, 末尾] の組（例: [[0, 99], [100, 199]]）
    SubscribeMembers { guild_id: String, channel_id: String, ranges: Vec<[u32; 2]> },
    LoadMessages(String),
    /// before より古いメッセージを読み足す（上へスクロールしたとき）
    LoadOlder { channel_id: String, before: String },
    /// reply_to = 返信先のメッセージID
    /// files = 添付ファイル（名前, 中身）
    Send { channel_id: String, content: String, reply_to: Option<String>, files: Vec<(String, Vec<u8>)> },
    Edit { channel_id: String, id: String, content: String },
    Delete { channel_id: String, id: String },
    LoadSticker { id: String, format: u8 },
    /// online / idle / dnd / invisible
    SetStatus(String),
    /// アクティビティに「Riscord をプレイ中」を出すか
    SetActivity(bool),
    LoadProfile { user_id: String, guild_id: Option<String> },
    /// 自分のプロフィールを変える。None は変えない。空の文字列は消す。avatar は Some(None) で削除、Some(Some(data URI)) で変更
    UpdateProfile { global_name: Option<String>, pronouns: Option<String>, bio: Option<String>, avatar: Option<Option<String>> },
    /// ボイスチャンネル / DM通話に入る。ring=true なら相手を呼び出す
    JoinVoice { guild_id: Option<String>, channel_id: String, ring: bool },
    LeaveVoice,
    /// ミュート・スピーカーミュートが変わったのを Discord に伝える
    VoiceFlags,
    /// 着信を拒否する
    StopRinging(String),
    /// message_id までを既読にする（公式クライアントなど他の端末にも反映される）
    Ack { channel_id: String, message_id: String },
    /// REST API をどれでも呼ぶ（src/net/rest.rs）。結果は Event::Api { id, .. } で返る
    Api { id: u64, call: ApiCall },
    /// ゲートウェイにそのまま送る（{"op": …, "d": …}。rest.rs の request_guild_members などで作る）
    Gateway(Value),
}

#[derive(Clone, Debug, PartialEq)]
pub enum VoiceStatus {
    Connecting { guild_id: Option<String>, channel_id: String },
    Connected { guild_id: Option<String>, channel_id: String },
    Disconnected,
    Error(String),
}

pub enum Event {
    Me(User),
    AuthFailed,
    LoginFailed,
    MessagesFailed(String),
    /// 既読通知が Discord に届かなかった。古い失敗が新しい通知を消さないよう ID を含める。
    AckFailed { channel_id: String, message_id: String },
    ComposeResult { channel_id: String, result: Result<Option<Message>, ()> },
    /// 送信・編集が Discord に断られた（確実に届いていない）。理由を添えて ComposeResult の直前に送る
    ComposeRejected { channel_id: String, reason: String },
    Guilds(Vec<Guild>),
    Dms(Vec<Channel>),
    GuildChannels(String, Vec<Channel>),
    GuildRoles(String, Vec<Role>),
    GuildMembers(String, Vec<Member>),
    /// メンバー一覧の差分（GUILD_MEMBER_LIST_UPDATE）。メンバーの中身は GuildMembers で別に送る
    MemberList { guild_id: String, list_id: String, groups: Vec<(String, u32)>, ops: Vec<ListOp> },
    /// 一からつながった（READY）。購読などはやり直しになる
    GatewayReady,
    Messages(String, Vec<Message>),
    /// このチャンネルを見る権限がない
    NoAccess(String),
    /// LoadOlder の結果（古い順）。失敗したら None
    OlderMessages { channel_id: String, before: String, messages: Option<Vec<Message>> },
    MessageCreate(Message),
    MessageUpdate { channel_id: String, id: String, content: Option<String>, embeds: Option<Vec<Embed>> },
    MessageDelete { channel_id: String, id: String },
    Status(String),
    Error(String),
    Sticker(String, Result<crate::sticker::Frames, String>),
    Presences(Vec<(String, Presence)>),
    /// 自分の見え方（Discord が全セッションをまとめたもの。アクティビティを含む）
    OwnPresence(Presence),
    Profile { key: String, result: Result<ProfileData, String> },
    /// UpdateProfile の結果。失敗したら利用者向けの理由
    ProfileSaved(Result<(), String>),
    VoiceStates(Vec<VoiceState>),
    Voice(VoiceStatus),
    /// DM通話の状態（呼び出し中のユーザー）
    Call { channel_id: String, ringing: Vec<String> },
    CallEnded(String),
    /// Cmd::Api の結果。失敗したら理由
    Api { id: u64, call: ApiCall, result: Result<Value, String> },
    /// 個別に扱っていないゲートウェイのイベント（rest::FORWARDED_EVENTS）。t は種類、d は中身
    Gateway { t: String, d: Value },
    /// READY のうち、自分の設定まわり（user_guild_settings / read_state / relationships / user_settings_proto）
    ReadyExtra(Value),
    /// サーバーごとのカスタム絵文字（READY）
    GuildEmojis(Vec<(String, Vec<model::Emoji>)>),
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
    Sync { range: [u32; 2], items: Vec<ListItem> },
    Insert { index: usize, item: ListItem },
    Update { index: usize, item: ListItem },
    Delete { index: usize },
    Invalidate { range: [u32; 2] },
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
        let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build().expect("tokio");
        rt.block_on(run(token, status, tx, cmd_rx, ctl));
    });
    (cmd_tx, ev_rx)
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
}

/// 1回に取るメッセージ数（API の上限は 100）。これより少なく返ってきたらチャンネルの始まり
pub const HISTORY_PAGE: usize = 50;

const ROLE_CACHE_TTL: Duration = Duration::from_secs(300);

impl Api {
    fn cached_roles(&self, guild_id: &str) -> Option<Vec<Role>> {
        let cache = self.roles.lock().unwrap();
        cache.get(guild_id).filter(|(at, _)| at.elapsed() < ROLE_CACHE_TTL).map(|(_, r)| r.clone())
    }

    fn store_roles(&self, guild_id: &str, roles: &[Role]) {
        self.roles.lock().unwrap().insert(guild_id.to_string(), (std::time::Instant::now(), roles.to_vec()));
    }
}

// サーバー指定の待機時間が不正なら、パニックや無限待機を避けて操作を失敗として返す。
fn retry_delay(value: &Value) -> Result<Duration> {
    let seconds = match value.get("retry_after") {
        None | Some(Value::Null) => 1.0,
        Some(v) => v.as_f64().ok_or_else(|| anyhow::anyhow!("invalid retry_after"))?,
    };
    if !seconds.is_finite() || !(0.0..=86_400.0).contains(&seconds) {
        bail!("invalid retry_after");
    }
    Ok(Duration::from_secs_f64(seconds.max(0.1)))
}

impl Api {
    async fn request(&self, method: reqwest::Method, path: &str, body: Option<&Value>) -> Result<reqwest::Response> {
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
        loop {
            #[cfg(test)]
            let base = self.base_url.as_deref().unwrap_or(API);
            #[cfg(not(test))]
            let base = API;
            let req = build(self.http.request(method.clone(), format!("{base}{path}")).header(AUTHORIZATION, &self.token));
            let r = req.send().await?;
            match r.status() {
                StatusCode::TOO_MANY_REQUESTS => {
                    let v: Value = r.json().await.unwrap_or_default();
                    tokio::time::sleep(retry_delay(&v)?).await;
                }
                StatusCode::UNAUTHORIZED => return Err(anyhow::Error::msg(UnauthorizedMarker)),
                s if !s.is_success() => {
                    let text = r.text().await.unwrap_or_default();
                    return Err(anyhow::Error::new(HttpError { status: s.as_u16(), text: format!("{method} {path}: {s} {text}") }));
                }
                _ => return Ok(r),
            }
        }
    }

    async fn get<T: DeserializeOwned>(&self, path: &str) -> Result<T> {
        Ok(self.request(reqwest::Method::GET, path, None).await?.json().await?)
    }

    async fn post(&self, path: &str, body: &Value) -> Result<()> {
        self.request(reqwest::Method::POST, path, Some(body)).await?;
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
        f.write_str("トークンが無効です (401)")
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
    let body: Value = h.text.find('{').and_then(|i| serde_json::from_str(&h.text[i..]).ok()).unwrap_or_default();
    let code = body["code"].as_u64().unwrap_or(0);
    let raw = body.to_string();
    Some(match (h.status, code) {
        (413, _) | (_, 40005) => "ファイルが大きすぎます".into(),
        (_, 50035) if raw.contains("BASE_TYPE_MAX_LENGTH") || raw.contains("\"content\"") => "本文が長すぎます".into(),
        (_, 50006) => "空のメッセージは送信できません".into(),
        (_, 20016) => "低速モード中です。しばらく待ってから送ってください".into(),
        (_, 50007) => "相手がDMを受け付けていません".into(),
        (403, _) | (_, 50013) | (_, 50001) => "このチャンネルで送信する権限がありません".into(),
        (s, _) if (400..500).contains(&s) && s != 429 => format!("Discord に拒否されました（{s}）"),
        _ => return None,
    })
}

fn is_forbidden(e: &anyhow::Error) -> bool {
    e.downcast_ref::<HttpError>().is_some_and(|h| h.status == 403)
}

fn is_unauthorized(e: &anyhow::Error) -> bool {
    e.downcast_ref::<UnauthorizedMarker>().is_some()
}

fn api_client(connect_timeout: Duration, read_timeout: Duration) -> Result<reqwest::Client> {
    Ok(reqwest::Client::builder()
        .user_agent("Mozilla/5.0 (X11; Linux x86_64) riscord/0.1")
        .connect_timeout(connect_timeout)
        // 全体の時間ではなく受信の無通信時間を制限し、大きな添付の送信を妨げない。
        .read_timeout(read_timeout)
        .build()?)
}

async fn run(token: String, status: MyPresence, tx: Tx, mut cmd_rx: UnboundedReceiver<Cmd>, ctl: Arc<VoiceCtl>) {
    let http = api_client(Duration::from_secs(15), Duration::from_secs(30)).expect("http client");
    let api = Api { #[cfg(test)] base_url: None, http, token: token.clone(), roles: Default::default(), guild_perms: Default::default() };

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
    tokio::spawn(gateway_loop(token, tx.clone(), gw_rx, status.clone(), v_tx.clone()));
    tokio::spawn(voice_task(me_id.clone(), api.clone(), tx.clone(), gw_tx.clone(), v_rx, ctl));

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
                Err(e) => tx.send(Event::Error(format!("サーバー一覧: {e:#}"))),
            }
            match api.get::<Vec<Channel>>("/users/@me/channels").await {
                Ok(c) => tx.send(Event::Dms(c)),
                Err(e) => tx.send(Event::Error(format!("DM一覧: {e:#}"))),
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
        if let Cmd::SubscribeMembers { guild_id, channel_id, ranges } = &cmd {
            let _ = gw_tx.send(member_subscription(guild_id, channel_id, ranges));
            continue;
        }
        let vmsg = match &cmd {
            Cmd::JoinVoice { guild_id, channel_id, ring } => {
                Some(VoiceMsg::Join { guild_id: guild_id.clone(), channel_id: channel_id.clone(), ring: *ring })
            }
            Cmd::LeaveVoice => Some(VoiceMsg::Leave),
            Cmd::VoiceFlags => Some(VoiceMsg::Flags),
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
                            let base = known.unwrap_or_else(|| model::base_permissions(&gid, roles, &member.roles));
                            Some((base, member.roles.clone()))
                        }
                        _ => None,
                    };
                    if let Ok(roles) = roles {
                        api.store_roles(&gid, &roles);
                        tx.send(Event::GuildRoles(gid.clone(), roles));
                    }
                    if let Ok(member) = member {
                        tx.send(Event::GuildMembers(gid.clone(), vec![member]));
                    }
                    channels.map(|c| {
                        let c = match filter {
                            Some((base, my_roles)) => model::visible_channels(c, &gid, base, &my_roles, &me),
                            None => c,
                        };
                        tx.send(Event::GuildChannels(gid, c))
                    })
                }
                Cmd::LoadMessages(cid) => match api.get::<Vec<Message>>(&format!("/channels/{cid}/messages?limit={HISTORY_PAGE}")).await {
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
                    let r = api.get::<Vec<Message>>(&format!("/channels/{channel_id}/messages?before={before}&limit={HISTORY_PAGE}")).await;
                    let messages = match &r {
                        Ok(m) => Some(m.iter().rev().cloned().collect()),
                        // 履歴を読む権限がない：取り直さず、ここまでとする
                        Err(e) if is_forbidden(e) => Some(Vec::new()),
                        Err(_) => None,
                    };
                    tx.send(Event::OlderMessages { channel_id, before, messages });
                    match r {
                        Err(e) if !is_forbidden(&e) => Err(e),
                        _ => Ok(()),
                    }
                }
                Cmd::SubscribeMembers { .. } | Cmd::SetStatus(_) | Cmd::SetActivity(_) | Cmd::JoinVoice { .. } | Cmd::LeaveVoice | Cmd::VoiceFlags | Cmd::Gateway(_) => Ok(()),
                Cmd::Api { id, call } => {
                    let result = api.call(&call).await.map_err(|e| format!("{e:#}"));
                    tx.send(Event::Api { id, call, result });
                    Ok(())
                }
                Cmd::Ack { channel_id, message_id } => {
                    send_ack(&api, &tx, channel_id, message_id).await
                }
                Cmd::StopRinging(cid) => {
                    api.post(&format!("/channels/{cid}/call/stop-ringing"), &json!({ "recipients": null })).await
                }
                Cmd::UpdateProfile { global_name, pronouns, bio, avatar } => {
                    let result = update_profile(&api, &tx, global_name, pronouns, bio, avatar).await;
                    tx.send(Event::ProfileSaved(result));
                    Ok(())
                }
                Cmd::LoadProfile { user_id, guild_id } => {
                    let key = profile_key(&user_id, guild_id.as_deref());
                    let result = load_profile(&api, &user_id, guild_id.as_deref()).await.map_err(|e| format!("{e:#}"));
                    tx.send(Event::Profile { key, result });
                    Ok(())
                }
                Cmd::LoadSticker { id, format } => {
                    let r = load_sticker(&api.http, &id, format).await.map_err(|e| format!("{e:#}"));
                    tx.send(Event::Sticker(id, r));
                    Ok(())
                }
                Cmd::Send { channel_id, content, reply_to, files } => {
                    let mut body = json!({ "content": content, "nonce": nonce(), "tts": false });
                    if let Some(id) = reply_to {
                        body["message_reference"] = json!({ "channel_id": channel_id, "message_id": id });
                    }
                    let path = format!("/channels/{channel_id}/messages");
                    let response = if files.is_empty() {
                        api.request(reqwest::Method::POST, &path, Some(&body)).await
                    } else {
                        body["attachments"] = files.iter().enumerate().map(|(i, (name, _))| json!({ "id": i, "filename": name })).collect();
                        let payload = body.to_string();
                        api.request_with(reqwest::Method::POST, &path, |req| req.multipart(upload_form(&payload, &files))).await
                    };
                    let result = match response {
                        Ok(response) => Ok(response.json::<Message>().await.ok()),
                        Err(e) => {
                            eprintln!("メッセージ送信失敗: {e:#}");
                            if let Some(reason) = compose_reject_reason(&e) { tx.send(Event::ComposeRejected { channel_id: channel_id.clone(), reason }); }
                            Err(())
                        }
                    };
                    tx.send(Event::ComposeResult { channel_id, result });
                    Ok(())
                }
                Cmd::Edit { channel_id, id, content } => {
                    let response = api.request(reqwest::Method::PATCH, &format!("/channels/{channel_id}/messages/{id}"), Some(&json!({ "content": content }))).await;
                    let result = match response {
                        Ok(response) => Ok(response.json::<Message>().await.ok()),
                        Err(e) => {
                            eprintln!("メッセージ編集失敗: {e:#}");
                            if let Some(reason) = compose_reject_reason(&e) { tx.send(Event::ComposeRejected { channel_id: channel_id.clone(), reason }); }
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
    api.delete(&format!("/channels/{channel_id}/messages/{id}")).await?;
    tx.send(Event::MessageDelete { channel_id, id });
    Ok(())
}

async fn send_ack(api: &Api, tx: &Tx, channel_id: String, message_id: String) -> Result<()> {
    let result = api.post(&format!("/channels/{channel_id}/messages/{message_id}/ack"), &json!({ "token": null })).await;
    if result.is_err() {
        tx.send(Event::AckFailed { channel_id, message_id });
    }
    result
}

async fn load_sticker(http: &reqwest::Client, id: &str, format: u8) -> Result<crate::sticker::Frames> {
    let r = http.get(crate::sticker::url(id, format)).send().await?.error_for_status()?;
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
        bio: str_of(&gp["bio"]).or_else(|| str_of(&up["bio"])).or_else(|| str_of(&v["user"]["bio"])),
        pronouns: str_of(&gp["pronouns"]).or_else(|| str_of(&up["pronouns"])),
        banner: str_of(&v["user"]["banner"]),
        accent_color: v["user"]["accent_color"].as_u64().or_else(|| up["accent_color"].as_u64()).map(|c| c as u32),
        nick: str_of(&gm["nick"]),
        joined_at: str_of(&gm["joined_at"]),
        roles: Vec::new(),
    };
    // ロール名と色はサーバーのロール一覧から引く（失敗しても無視）
    if let (Some(g), Some(ids)) = (guild_id, gm["roles"].as_array()) {
        let all = match api.cached_roles(g) {
            Some(r) => Ok(r),
            None => api.get::<Vec<Role>>(&format!("/guilds/{g}/roles")).await.inspect(|r| api.store_roles(g, r)),
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
        user.insert("global_name".into(), if n.is_empty() { Value::Null } else { Value::String(n) });
    }
    if let Some(a) = avatar {
        user.insert("avatar".into(), a.map_or(Value::Null, Value::String));
    }
    if !user.is_empty() {
        let r = api.request(reqwest::Method::PATCH, "/users/@me", Some(&Value::Object(user))).await.map_err(|e| profile_error_text(&e))?;
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
        api.request(reqwest::Method::PATCH, "/users/@me/profile", Some(&Value::Object(profile))).await.map_err(|e| profile_error_text(&e))?;
    }
    Ok(())
}

/// プロフィールの保存に失敗した理由（利用者向け）
pub fn profile_error_text(e: &anyhow::Error) -> String {
    eprintln!("プロフィール保存失敗: {e:#}");
    let Some(h) = e.downcast_ref::<HttpError>() else {
        return "保存できませんでした。通信状態を確認してください".into();
    };
    let t = &h.text;
    if t.contains("captcha_key") {
        "Discord が確認（CAPTCHA）を求めました。この変更は公式アプリで行ってください".into()
    } else if t.contains("AVATAR_RATE_LIMIT") || t.contains("too fast") {
        "アバターの変更が多すぎます。しばらく待ってから試してください".into()
    } else if t.contains("PASSWORD") {
        "パスワードの確認が必要な変更です。公式アプリで行ってください".into()
    } else if t.contains("50035") {
        "入力内容を受け付けてもらえませんでした。使えない言葉や長さを確認してください".into()
    } else {
        format!("保存できませんでした（{}）", h.status)
    }
}

fn upload_form(payload: &str, files: &[(String, Vec<u8>)]) -> reqwest::multipart::Form {
    let mut form = reqwest::multipart::Form::new().text("payload_json", payload.to_string());
    for (i, (name, data)) in files.iter().enumerate() {
        form = form.part(format!("files[{i}]"), reqwest::multipart::Part::bytes(data.clone()).file_name(name.clone()));
    }
    form
}

fn nonce() -> String {
    const EPOCH: u64 = 1_420_070_400_000;
    let ms = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as u64;
    ((ms - EPOCH) << 22).to_string()
}


#[cfg(test)]
mod tests;
