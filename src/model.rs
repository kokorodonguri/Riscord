use serde::{Deserialize, Serialize};

/// オンライン状態と、表示用のアクティビティ文（「プレイ中: 〜」やカスタムステータス）
#[derive(Clone, Debug, Default)]
pub struct Presence {
    pub status: String,
    pub activity: Option<String>,
}

/// プロフィールのポップアップ用（/users/{id}/profile とサーバーのロール）
#[derive(Clone, Debug, Default)]
pub struct ProfileData {
    pub bio: Option<String>,
    pub pronouns: Option<String>,
    pub banner: Option<String>,
    pub accent_color: Option<u32>,
    pub nick: Option<String>,
    pub joined_at: Option<String>,
    /// (ロール名, 色) 上位のロール順
    pub roles: Vec<(String, u32)>,
}

#[derive(Clone, Debug, Deserialize, Serialize, Hash)]
pub struct User {
    pub id: String,
    pub username: String,
    #[serde(default)]
    pub global_name: Option<String>,
    #[serde(default)]
    pub avatar: Option<String>,
    /// Nitro の種類（0/なし = 無料、1 = Classic、2 = Nitro、3 = Basic）。自分の情報にだけ入る
    #[serde(default)]
    pub premium_type: Option<u8>,
    #[serde(default)]
    pub bot: bool,
}

impl User {
    pub fn display(&self) -> &str {
        self.global_name.as_deref().unwrap_or(&self.username)
    }
}

/// サーバーのカスタム絵文字
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Emoji {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub animated: bool,
    /// サーバーのブーストが切れるなどして使えなくなった絵文字は false
    #[serde(default = "yes")]
    pub available: bool,
}

fn yes() -> bool {
    true
}

/// サーバーのロール
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Role {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub color: u32,
    #[serde(default)]
    pub position: i64,
    #[serde(default)]
    pub hoist: bool,
    /// 権限ビット（10進の文字列）
    #[serde(default)]
    pub permissions: String,
}

/// サーバーメンバー
#[derive(Clone, Debug, Deserialize, Serialize, Hash)]
pub struct Member {
    #[serde(default)]
    pub user: Option<User>,
    #[serde(default)]
    pub nick: Option<String>,
    #[serde(default)]
    pub roles: Vec<String>,
}

impl Member {
    pub fn display(&self) -> &str {
        self.nick.as_deref().or_else(|| self.user.as_ref().map(User::display)).unwrap_or("")
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Guild {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub icon: Option<String>,
    /// /users/@me/guilds が返す、このサーバーでの自分の権限（ロール・オーナーを反映済み。10進の文字列）
    #[serde(default)]
    pub permissions: Option<String>,
    /// 自分がオーナーか（オーナーは脱退できない）
    #[serde(default)]
    pub owner: bool,
}

/// チャンネルごとの権限の上書き
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Overwrite {
    pub id: String,
    /// 0 / "role" = ロール、1 / "member" = メンバー
    #[serde(rename = "type", default)]
    pub kind: serde_json::Value,
    #[serde(default)]
    pub allow: String,
    #[serde(default)]
    pub deny: String,
}

impl Overwrite {
    fn is_member(&self) -> bool {
        self.kind == 1 || self.kind == "member"
    }
}

pub const ADMINISTRATOR: u64 = 1 << 3;
pub const VIEW_CHANNEL: u64 = 1 << 10;

fn bits(s: &str) -> u64 {
    s.parse().unwrap_or(0)
}

/// サーバー全体での自分の権限を @everyone と自分のロールから出す
pub fn base_permissions(guild_id: &str, roles: &[Role], my_roles: &[String]) -> u64 {
    roles.iter().filter(|r| r.id == guild_id || my_roles.contains(&r.id)).fold(0, |p, r| p | bits(&r.permissions))
}

/// このチャンネルが見えるか（Discord の権限計算: @everyone → ロール → メンバー の順に上書き）
pub fn can_view(ch: &Channel, guild_id: &str, base: u64, my_roles: &[String], me: &str) -> bool {
    if base & ADMINISTRATOR != 0 {
        return true;
    }
    let mut p = base;
    let ow = &ch.permission_overwrites;
    if let Some(o) = ow.iter().find(|o| o.id == guild_id) {
        p = (p & !bits(&o.deny)) | bits(&o.allow);
    }
    let (mut allow, mut deny) = (0, 0);
    for o in ow.iter().filter(|o| !o.is_member() && my_roles.contains(&o.id)) {
        allow |= bits(&o.allow);
        deny |= bits(&o.deny);
    }
    p = (p & !deny) | allow;
    if let Some(o) = ow.iter().find(|o| o.is_member() && o.id == me) {
        p = (p & !bits(&o.deny)) | bits(&o.allow);
    }
    p & VIEW_CHANNEL != 0
}

/// 見えるチャンネルだけ残す。カテゴリは中に見えるチャンネルがあるとき（空のカテゴリは自分が見えるとき）だけ残す
pub fn visible_channels(channels: Vec<Channel>, guild_id: &str, base: u64, my_roles: &[String], me: &str) -> Vec<Channel> {
    let ok: Vec<bool> = channels.iter().map(|c| can_view(c, guild_id, base, my_roles, me)).collect();
    let keep: Vec<bool> = channels
        .iter()
        .zip(&ok)
        .map(|(c, &v)| {
            if c.kind != Channel::CATEGORY {
                return v;
            }
            let mut children = channels.iter().zip(&ok).filter(|(x, _)| x.parent_id.as_deref() == Some(c.id.as_str())).peekable();
            if children.peek().is_none() {
                v
            } else {
                children.any(|(_, &cv)| cv)
            }
        })
        .collect();
    channels.into_iter().zip(keep).filter_map(|(c, k)| k.then_some(c)).collect()
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Channel {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: u8,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub position: i64,
    #[serde(default)]
    pub parent_id: Option<String>,
    #[serde(default)]
    pub recipients: Vec<User>,
    #[serde(default)]
    pub last_message_id: Option<String>,
    #[serde(default)]
    pub topic: Option<String>,
    #[serde(default)]
    pub icon: Option<String>,
    #[serde(default)]
    pub permission_overwrites: Vec<Overwrite>,
}

impl Channel {
    pub const TEXT: u8 = 0;
    pub const CATEGORY: u8 = 4;
    pub const NEWS: u8 = 5;
    pub const VOICE: u8 = 2;

    pub fn is_text(&self) -> bool {
        matches!(self.kind, Self::TEXT | Self::NEWS)
    }

    pub fn is_voice(&self) -> bool {
        self.kind == Self::VOICE
    }

    pub fn dm_label(&self) -> String {
        if let Some(n) = self.name.as_deref().filter(|n| !n.is_empty()) {
            return n.to_string();
        }
        let names: Vec<&str> = self.recipients.iter().map(|u| u.display()).collect();
        if names.is_empty() { "(不明)".into() } else { names.join(", ") }
    }

    pub fn last_id(&self) -> u64 {
        self.last_message_id.as_deref().and_then(|s| s.parse().ok()).unwrap_or(0)
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, Hash)]
pub struct Attachment {
    pub filename: String,
    pub url: String,
    #[serde(default)]
    pub proxy_url: Option<String>,
    #[serde(default)]
    pub content_type: Option<String>,
    #[serde(default)]
    pub width: Option<u32>,
    #[serde(default)]
    pub height: Option<u32>,
}

impl Attachment {
    pub fn is_video(&self) -> bool {
        if let Some(content_type) = &self.content_type {
            if content_type.starts_with("video/") { return true; }
            if content_type != "application/octet-stream" { return false; }
        }
        let name = self.filename.to_ascii_lowercase();
        [".mp4", ".webm", ".mov", ".m4v", ".mkv"].iter().any(|ext| name.ends_with(ext))
    }

    pub fn is_image(&self) -> bool {
        if let Some(ct) = &self.content_type {
            return ct.starts_with("image/");
        }
        let f = self.filename.to_lowercase();
        [".png", ".jpg", ".jpeg", ".gif", ".webp"].iter().any(|e| f.ends_with(e))
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, Hash)]
pub struct StickerItem {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub format_type: u8,
}

#[derive(Clone, Debug, Deserialize, Serialize, Hash)]
pub struct EmbedMedia {
    pub url: String,
    #[serde(default)]
    pub proxy_url: Option<String>,
    #[serde(default)]
    pub width: Option<u32>,
    #[serde(default)]
    pub height: Option<u32>,
}

/// 埋め込みの作者・提供元（名前とリンク、作者はアイコンも）
#[derive(Clone, Debug, Default, Deserialize, Serialize, Hash)]
pub struct EmbedName {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub icon_url: Option<String>,
    #[serde(default)]
    pub proxy_icon_url: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, Hash)]
pub struct EmbedFooter {
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub icon_url: Option<String>,
    #[serde(default)]
    pub proxy_icon_url: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, Hash)]
pub struct EmbedField {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub value: String,
    #[serde(default)]
    pub inline: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize, Hash)]
pub struct Embed {
    #[serde(default, rename = "type")]
    pub kind: Option<String>,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    /// 左の縦線の色（0xRRGGBB）
    #[serde(default)]
    pub color: Option<u32>,
    #[serde(default)]
    pub timestamp: Option<String>,
    #[serde(default)]
    pub image: Option<EmbedMedia>,
    #[serde(default)]
    pub thumbnail: Option<EmbedMedia>,
    #[serde(default)]
    pub video: Option<EmbedMedia>,
    #[serde(default)]
    pub provider: Option<EmbedName>,
    #[serde(default)]
    pub author: Option<EmbedName>,
    #[serde(default)]
    pub footer: Option<EmbedFooter>,
    #[serde(default)]
    pub fields: Vec<EmbedField>,
}

impl Embed {
    /// 枠なしで画像だけ出す埋め込み（画像・GIF のリンクを貼ったとき）
    pub fn media(&self) -> Option<&EmbedMedia> {
        if !self.is_bare_media() {
            return None;
        }
        self.image.as_ref().or(self.thumbnail.as_ref())
    }

    fn is_bare_media(&self) -> bool {
        matches!(self.kind.as_deref(), Some("image" | "gifv")) && self.title.is_none() && self.description.is_none()
    }

    /// 文字の入ったカードとして出す埋め込みか
    pub fn is_rich(&self) -> bool {
        !self.is_bare_media()
            && (self.title.is_some()
                || self.description.is_some()
                || self.author.as_ref().is_some_and(|a| a.name.is_some())
                || self.provider.as_ref().is_some_and(|a| a.name.is_some())
                || self.footer.is_some()
                || !self.fields.is_empty()
                || self.image.is_some()
                || self.thumbnail.is_some())
    }

    /// GIF リンク（type=gifv）は mp4 とその静止画しか来ないので、同じ GIF の .gif 版の URL を作る。
    /// 作れなければ None（静止画のまま出す）
    pub fn animated_url(&self) -> Option<String> {
        if self.kind.as_deref() != Some("gifv") {
            return None;
        }
        [self.video.as_ref(), self.thumbnail.as_ref()].into_iter().flatten().find_map(|m| gif_url(&m.url))
    }

    /// 何か表示するものがあるか
    pub fn visible(&self) -> bool {
        self.media().is_some() || self.is_rich()
    }
}

/// Tenor / Giphy の mp4・静止画 URL を、アニメーションする GIF の URL にする
fn gif_url(url: &str) -> Option<String> {
    let rest = url.strip_prefix("https://").or_else(|| url.strip_prefix("http://"))?;
    let (host, path) = rest.split_once('/')?;
    let path = path.split(['?', '#']).next()?;
    if host.ends_with("tenor.com") {
        // https://media.tenor.com/<11文字のID><5文字の形式>/<名前>.<拡張子>
        // 形式: AAAPo=mp4, AAAAe=静止画, AAAAC=GIF原寸, AAAAd=GIF中サイズ（表示には十分で軽い）
        let mut parts: Vec<&str> = path.split('/').collect();
        let file = parts.pop()?;
        let code = parts.pop()?;
        if code.len() <= 5 || !code.is_ascii() {
            return None;
        }
        let name = file.rsplit_once('.').map_or(file, |(n, _)| n);
        let id = &code[..code.len() - 5];
        return Some(format!("https://media.tenor.com/{id}AAAAd/{name}.gif"));
    }
    if host.ends_with("giphy.com") {
        let stem = path.strip_suffix(".mp4").or_else(|| path.strip_suffix(".webp"))?;
        return Some(format!("https://{host}/{stem}.gif"));
    }
    None
}

#[derive(Clone, Debug, Deserialize, Serialize, Hash)]
pub struct Message {
    pub id: String,
    pub channel_id: String,
    #[serde(default)]
    pub guild_id: Option<String>,
    pub author: User,
    #[serde(default)]
    pub member: Option<Member>,
    #[serde(default)]
    pub content: String,
    /// 本文で @ されたユーザー（<@id> を名前で出すため）
    #[serde(default)]
    pub mentions: Vec<User>,
    /// @everyone / @here
    #[serde(default)]
    pub mention_everyone: bool,
    /// @ されたロールの ID
    #[serde(default)]
    pub mention_roles: Vec<String>,
    pub timestamp: String,
    #[serde(default)]
    pub edited_timestamp: Option<String>,
    #[serde(default)]
    pub attachments: Vec<Attachment>,
    #[serde(default)]
    pub sticker_items: Vec<StickerItem>,
    #[serde(default)]
    pub embeds: Vec<Embed>,
    /// 返信先（削除済みなら null）
    #[serde(default)]
    pub referenced_message: Option<Box<Message>>,
    /// 3 = 通話の記録、19 = 返信
    #[serde(default, rename = "type")]
    pub kind: u8,
    /// 通話の記録（type 3 のときだけ付く）
    #[serde(default)]
    pub call: Option<CallInfo>,
    /// MessageLogger の記録（削除済み・編集前の本文）。Discord からは来ない
    #[serde(skip)]
    pub log: Option<Box<LogMark>>,
}

/// MessageLogger が付ける印
#[derive(Clone, Debug, Default, Deserialize, Serialize, Hash, PartialEq)]
pub struct LogMark {
    /// 削除を受け取った時刻（RFC 3339）
    #[serde(default)]
    pub deleted_at: Option<String>,
    /// 編集前の本文（古い順）
    #[serde(default)]
    pub edits: Vec<PastEdit>,
}

/// 編集で置き換わる前の本文と、編集を受け取った時刻
#[derive(Clone, Debug, Deserialize, Serialize, Hash, PartialEq)]
pub struct PastEdit {
    pub content: String,
    pub at: String,
}

/// 通話の記録。参加した人と、終わった時刻（続いている間は無い）
#[derive(Clone, Debug, Default, Deserialize, Serialize, Hash)]
pub struct CallInfo {
    #[serde(default)]
    pub participants: Vec<String>,
    #[serde(default)]
    pub ended_timestamp: Option<String>,
}

impl Message {
    pub const CALL: u8 = 3;

    pub fn is_call(&self) -> bool {
        self.kind == Self::CALL
    }
}

/// ボイスチャンネルに誰がいるか
#[derive(Clone, Debug)]
pub struct VoiceState {
    pub user_id: String,
    pub guild_id: Option<String>,
    pub channel_id: Option<String>,
    pub self_mute: bool,
    pub self_deaf: bool,
    pub user: Option<User>,
}

impl VoiceState {
    pub fn parse(v: &serde_json::Value, guild_id: Option<&str>) -> Option<VoiceState> {
        let user_id = v["user_id"].as_str()?.to_string();
        let user = serde_json::from_value::<User>(v["member"]["user"].clone()).ok();
        Some(VoiceState {
            user_id,
            guild_id: v["guild_id"].as_str().or(guild_id).map(String::from),
            channel_id: v["channel_id"].as_str().map(String::from),
            self_mute: v["self_mute"].as_bool().unwrap_or(false) || v["mute"].as_bool().unwrap_or(false),
            self_deaf: v["self_deaf"].as_bool().unwrap_or(false) || v["deaf"].as_bool().unwrap_or(false),
            user,
        })
    }
}

#[cfg(test)]
mod embed_tests {
    use super::Embed;

    fn parse(v: serde_json::Value) -> Embed {
        serde_json::from_value(v).unwrap()
    }

    #[test]
    fn rich_embed_fields_are_read() {
        let e = parse(serde_json::json!({
            "type": "rich", "title": "t", "description": "d", "color": 0x5865f2,
            "author": { "name": "a", "icon_url": "https://x/a.png" },
            "footer": { "text": "f" },
            "fields": [{ "name": "n", "value": "v", "inline": true }],
            "thumbnail": { "url": "https://x/t.png", "width": 10, "height": 10 }
        }));
        assert!(e.is_rich() && e.visible());
        assert!(e.media().is_none());
        assert_eq!(e.color, Some(0x5865f2));
        assert_eq!(e.fields[0].value, "v");
    }

    #[test]
    fn link_preview_and_video_are_cards() {
        let yt = parse(serde_json::json!({
            "type": "video", "url": "https://youtu.be/x", "title": "動画",
            "provider": { "name": "YouTube" },
            "thumbnail": { "url": "https://i.ytimg.com/x.jpg", "width": 1280, "height": 720 },
            "video": { "url": "https://www.youtube.com/embed/x", "width": 1280, "height": 720 }
        }));
        assert!(yt.is_rich());
        assert!(yt.media().is_none());
    }

    #[test]
    fn bare_image_stays_image() {
        let img = parse(serde_json::json!({ "type": "image", "url": "u", "thumbnail": { "url": "https://x/i.png" } }));
        assert!(img.media().is_some());
        assert!(!img.is_rich());
        let gif = parse(serde_json::json!({ "type": "gifv", "url": "u", "thumbnail": { "url": "https://x/g.png" }, "video": { "url": "https://x/g.mp4" } }));
        assert!(gif.media().is_some() && !gif.is_rich());
    }

    #[test]
    fn gifv_gets_animated_gif_url() {
        let tenor = parse(serde_json::json!({
            "type": "gifv", "url": "https://tenor.com/view/x-123",
            "thumbnail": { "url": "https://media.tenor.com/SPmJqrVn0PEAAAAe/steven-universe-happy.png" },
            "video": { "url": "https://media.tenor.com/SPmJqrVn0PEAAAPo/steven-universe-happy.mp4" }
        }));
        assert_eq!(tenor.animated_url().as_deref(), Some("https://media.tenor.com/SPmJqrVn0PEAAAAd/steven-universe-happy.gif"));
        let giphy = parse(serde_json::json!({
            "type": "gifv", "url": "https://giphy.com/gifs/x",
            "video": { "url": "https://media.giphy.com/media/abc/giphy.mp4" }
        }));
        assert_eq!(giphy.animated_url().as_deref(), Some("https://media.giphy.com/media/abc/giphy.gif"));
        let other = parse(serde_json::json!({ "type": "gifv", "video": { "url": "https://example.com/a.mp4" } }));
        assert!(other.animated_url().is_none());
        let img = parse(serde_json::json!({ "type": "image", "thumbnail": { "url": "https://media.tenor.com/SPmJqrVn0PEAAAAe/a.png" } }));
        assert!(img.animated_url().is_none());
    }
}

#[cfg(test)]
mod permission_tests {
    use super::*;

    fn ch(id: &str, kind: u8, parent: Option<&str>, ow: serde_json::Value) -> Channel {
        serde_json::from_value(serde_json::json!({ "id": id, "type": kind, "parent_id": parent, "permission_overwrites": ow })).unwrap()
    }

    const G: &str = "100";
    const ME: &str = "7";

    #[test]
    fn overwrites_apply_in_order() {
        let base = VIEW_CHANNEL;
        let roles = vec!["r1".to_string()];
        // @everyone で見えなくされたチャンネル
        let hidden = ch("1", 0, None, serde_json::json!([{ "id": G, "type": 0, "allow": "0", "deny": "1024" }]));
        assert!(!can_view(&hidden, G, base, &roles, ME));
        // 自分のロールで許可されていれば見える
        let by_role = ch("2", 0, None, serde_json::json!([{ "id": G, "type": 0, "allow": "0", "deny": "1024" }, { "id": "r1", "type": 0, "allow": "1024", "deny": "0" }]));
        assert!(can_view(&by_role, G, base, &roles, ME));
        // 他人のロールの許可は関係ない
        assert!(!can_view(&by_role, G, base, &[], ME));
        // メンバー個別の拒否はロールの許可より強い（古い文字列の type も読む）
        let member_deny = ch("3", 0, None, serde_json::json!([{ "id": "r1", "type": "role", "allow": "1024", "deny": "0" }, { "id": ME, "type": "member", "allow": "0", "deny": "1024" }]));
        assert!(!can_view(&member_deny, G, base, &roles, ME));
        // 管理者は全部見える
        assert!(can_view(&hidden, G, ADMINISTRATOR, &[], ME));
        // 元々 VIEW_CHANNEL が無くても上書きで許可されれば見える
        let allowed = ch("4", 0, None, serde_json::json!([{ "id": ME, "type": 1, "allow": "1024", "deny": "0" }]));
        assert!(can_view(&allowed, G, 0, &[], ME));
    }

    #[test]
    fn categories_without_visible_children_are_dropped() {
        let deny = serde_json::json!([{ "id": G, "type": 0, "allow": "0", "deny": "1024" }]);
        let list = vec![
            ch("c1", Channel::CATEGORY, None, serde_json::json!([])),
            ch("a", 0, Some("c1"), deny.clone()),
            ch("c2", Channel::CATEGORY, None, serde_json::json!([])),
            ch("b", 0, Some("c2"), serde_json::json!([])),
            ch("c3", Channel::CATEGORY, None, serde_json::json!([])),
        ];
        let ids: Vec<String> = visible_channels(list, G, VIEW_CHANNEL, &[], ME).into_iter().map(|c| c.id).collect();
        assert_eq!(ids, vec!["c2", "b", "c3"]);
    }

    #[test]
    fn base_permissions_combine_everyone_and_my_roles() {
        let role = |id: &str, p: u64| Role { id: id.into(), name: String::new(), color: 0, position: 0, hoist: false, permissions: p.to_string() };
        let roles = vec![role(G, VIEW_CHANNEL), role("r1", 1 << 11), role("r2", ADMINISTRATOR)];
        assert_eq!(base_permissions(G, &roles, &["r1".into()]), VIEW_CHANNEL | 1 << 11);
    }
}

#[cfg(test)]
mod layout_hash_tests {
    use super::*;
    use std::hash::{Hash, Hasher};

    fn hash(message: &Message) -> u64 {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        message.hash(&mut hasher);
        hasher.finish()
    }

    #[test]
    fn message_hash_changes_for_embed_text_and_media_dimensions() {
        let message: Message = serde_json::from_value(serde_json::json!({
            "id": "1", "channel_id": "c", "timestamp": "2026-11-01T10:00:00Z",
            "author": {"id": "u", "username": "name"},
            "embeds": [{"fields": [{"name": "title", "value": "short"}],
                        "image": {"url": "https://example.com/image", "width": 100, "height": 100}}]
        })).unwrap();
        let original = hash(&message);
        let mut updated = message.clone();
        updated.embeds[0].fields[0].value = "a much longer field".into();
        assert_ne!(hash(&updated), original);
        let mut updated = message.clone();
        updated.embeds[0].image.as_mut().unwrap().height = Some(400);
        assert_ne!(hash(&updated), original);
        let mut updated = message.clone();
        updated.author.username = "another author".into();
        assert_ne!(hash(&updated), original);
    }
}
