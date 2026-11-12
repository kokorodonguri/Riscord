//! 日時・色・URL・絵文字・メンション・アバター変換などの汎用ヘルパー。
use std::collections::HashMap;
use std::sync::Arc;

use chrono::{DateTime, Local};
use gpui::RenderImage;

use super::{AvatarEdit, AVATAR_SIZE};
use crate::color::Color32;
use crate::images;
use crate::model::{self, Channel, Embed, Guild, Member, Message, Role, User};

// ---- ヘルパー ----
pub fn local_dt(ts: &str) -> Option<DateTime<Local>> {
    DateTime::parse_from_rfc3339(ts)
        .ok()
        .map(|t| t.with_timezone(&Local))
}

pub fn fmt_time(ts: &str) -> String {
    let Some(t) = local_dt(ts) else {
        return String::new();
    };
    let today = Local::now().date_naive();
    let d = t.date_naive();
    if d == today {
        format!("今日 {}", t.format("%H:%M"))
    } else if today.pred_opt() == Some(d) {
        format!("昨日 {}", t.format("%H:%M"))
    } else {
        t.format("%Y/%m/%d %H:%M").to_string()
    }
}

pub fn name_color(id: &str) -> Color32 {
    let h = id
        .bytes()
        .fold(0u32, |a, b| a.wrapping_mul(31).wrapping_add(b as u32));
    let palette = [
        Color32::from_rgb(0x5a, 0xb4, 0xf0),
        Color32::from_rgb(0xf0, 0x8c, 0x5a),
        Color32::from_rgb(0x7c, 0xd0, 0x7a),
        Color32::from_rgb(0xd0, 0x7a, 0xc8),
        Color32::from_rgb(0xe8, 0xc8, 0x50),
        Color32::from_rgb(0x8a, 0x9c, 0xf5),
    ];
    palette[(h as usize) % palette.len()]
}

pub fn initials(name: &str) -> String {
    let words: Vec<&str> = name.split_whitespace().collect();
    if words.len() > 1 {
        words
            .iter()
            .filter_map(|w| w.chars().next())
            .take(3)
            .collect()
    } else {
        name.chars().take(2).collect()
    }
}

pub fn user_avatar_url(u: &User) -> String {
    match &u.avatar {
        Some(h) => {
            let ext = if h.starts_with("a_") { "gif" } else { "png" };
            format!(
                "https://cdn.discordapp.com/avatars/{}/{h}.{ext}?size=80",
                u.id
            )
        }
        None => {
            let n = u.id.parse::<u64>().map(|i| (i >> 22) % 6).unwrap_or(0);
            format!("https://cdn.discordapp.com/embed/avatars/{n}.png")
        }
    }
}

pub fn member_avatar_url(m: &Member, guild_id: &str) -> Option<String> {
    if let Some(h) = m.avatar.as_deref().filter(|s| !s.is_empty()) {
        let uid = &m.user.as_ref()?.id;
        let ext = if h.starts_with("a_") { "gif" } else { "png" };
        return Some(format!(
            "https://cdn.discordapp.com/guilds/{guild_id}/users/{uid}/avatars/{h}.{ext}?size=80"
        ));
    }
    m.user.as_ref().map(user_avatar_url)
}

pub fn decoration_url(asset: &str) -> String {
    format!("https://cdn.discordapp.com/avatar-decoration-presets/{asset}.png?size=96&passthrough=true")
}

pub fn user_decoration_url(u: &User) -> Option<String> {
    u.decoration_asset().map(decoration_url)
}

pub fn member_decoration_url(m: &Member) -> Option<String> {
    m.decoration_asset().map(decoration_url)
}

pub fn guild_icon_url(g: &Guild) -> Option<String> {
    g.icon.as_ref().map(|h| {
        format!(
            "https://cdn.discordapp.com/icons/{}/{}.png?size=96",
            g.id, h
        )
    })
}

pub fn dm_icon_url(c: &Channel) -> Option<String> {
    if let Some(h) = &c.icon {
        Some(format!(
            "https://cdn.discordapp.com/channel-icons/{}/{}.png?size=80",
            c.id, h
        ))
    } else if c.recipients.len() == 1 {
        Some(user_avatar_url(&c.recipients[0]))
    } else {
        None
    }
}

pub fn role_color(role_ids: &[String], roles: &[Role]) -> Option<Color32> {
    let role = roles
        .iter()
        .filter(|r| r.color != 0 && role_ids.contains(&r.id))
        .max_by_key(|r| r.position)?;
    Some(Color32::from_rgb(
        (role.color >> 16) as u8,
        (role.color >> 8) as u8,
        role.color as u8,
    ))
}

pub fn member_role_color(member: &Member, roles: &[Role]) -> Option<Color32> {
    role_color(&member.roles, roles)
}

/// 幅・高さを最大サイズに収まるよう縮める
pub fn fit_size(w: Option<u32>, h: Option<u32>, max: (f32, f32)) -> Option<(f32, f32)> {
    let (w, h) = (w? as f32, h? as f32);
    if w <= 0.0 || h <= 0.0 {
        return None;
    }
    let scale = (max.0 / w).min(max.1 / h).min(1.0);
    Some((w * scale, h * scale))
}

/// Discordのメディアプロキシで縮小版を取る（GIFはアニメを残すため原寸）
pub fn resized(proxy: Option<&str>, url: &str, size: (f32, f32)) -> String {
    let base = proxy.unwrap_or(url);
    let path = base.split(['?', '#']).next().unwrap_or("").to_lowercase();
    if base.contains("media.discordapp.net") && !path.ends_with(".gif") {
        let sep = if base.contains('?') { '&' } else { '?' };
        let ppp = 2.0; // 高DPI用に倍で取る
                       // WebP は animated=true を付けないとプロキシが1枚目だけにしてしまう
        let anim = if path.ends_with(".webp") {
            "&animated=true"
        } else {
            ""
        };
        format!(
            "{base}{sep}width={}&height={}{anim}",
            (size.0 * ppp) as u32,
            (size.1 * ppp) as u32
        )
    } else {
        base.to_string()
    }
}

/// スタンプ（Lottie・APNG・GIF をデコードしたもの。アニメは gpui が進める）
pub enum StickerState {
    Loading,
    Ready(Arc<RenderImage>),
    Failed,
}

impl Drop for StickerState {
    fn drop(&mut self) {
        if let Self::Ready(image) = self {
            images::release(image.clone());
        }
    }
}

pub fn embed_big_media(e: &Embed) -> Option<&model::EmbedMedia> {
    e.image.as_ref().or_else(|| {
        if e.kind.as_deref() == Some("video") || e.video.is_some() {
            e.thumbnail.as_ref()
        } else {
            None
        }
    })
}

pub enum Seg<'a> {
    Text(&'a str),
    Emoji {
        name: &'a str,
        id: &'a str,
        animated: bool,
    },
}

/// 本文を文字列とカスタム絵文字 `<:名前:id>` / `<a:名前:id>` に分ける
pub fn split_emoji(s: &str) -> Vec<Seg<'_>> {
    let mut out = Vec::new();
    let mut rest = s;
    let mut text_start = 0usize;
    let mut pos = 0usize;
    while let Some(i) = rest.find('<') {
        let at = pos + i;
        let tail = &s[at..];
        if let Some((len, seg)) = parse_emoji(tail) {
            if text_start < at {
                out.push(Seg::Text(&s[text_start..at]));
            }
            out.push(seg);
            pos = at + len;
            text_start = pos;
        } else {
            pos = at + 1;
        }
        rest = &s[pos..];
    }
    if text_start < s.len() {
        out.push(Seg::Text(&s[text_start..]));
    }
    out
}

pub fn parse_emoji(t: &str) -> Option<(usize, Seg<'_>)> {
    let end = t.find('>')?;
    let inner = &t[1..end];
    let (animated, inner) = match inner.strip_prefix("a:") {
        Some(r) => (true, r),
        None => (false, inner.strip_prefix(':')?),
    };
    let (name, id) = inner.split_once(':')?;
    let ok_name = !name.is_empty() && name.chars().all(|c| c.is_alphanumeric() || c == '_');
    let ok_id = !id.is_empty() && id.bytes().all(|b| b.is_ascii_digit());
    (ok_name && ok_id).then_some((end + 1, Seg::Emoji { name, id, animated }))
}

pub fn emoji_url(id: &str, animated: bool) -> String {
    // GIFは末尾が .gif でないとアニメ再生されないので、サイズ指定を付けない
    if animated {
        format!("https://cdn.discordapp.com/emojis/{id}.gif")
    } else {
        format!("https://cdn.discordapp.com/emojis/{id}.png?size=64")
    }
}

/// 本文の <#id>・<@id>・<@!id>・<@&id> を本家と同じく「#チャンネル名」「@名前」「@ロール名」にする（ID のまま見せない）
pub fn resolve_mentions<'a>(
    content: &'a str,
    m: &Message,
    channels: &HashMap<String, Vec<Channel>>,
    roles: &[Role],
) -> std::borrow::Cow<'a, str> {
    if !content.contains("<#") && !content.contains("<@") {
        return std::borrow::Cow::Borrowed(content);
    }
    let mut out = String::with_capacity(content.len());
    let mut rest = content;
    while let Some(i) = rest.find('<') {
        out.push_str(&rest[..i]);
        let tail = &rest[i..];
        let end = tail.find('>');
        let token = end.map(|e| &tail[1..e]);
        let name = token.and_then(|t| {
            let (kind, id) = if let Some(id) = t.strip_prefix("@&") {
                ('&', id)
            } else if let Some(id) = t.strip_prefix("@!") {
                ('@', id)
            } else if let Some(id) = t.strip_prefix('@') {
                ('@', id)
            } else if let Some(id) = t.strip_prefix('#') {
                ('#', id)
            } else {
                return None;
            };
            if id.is_empty() || !id.bytes().all(|b| b.is_ascii_digit()) {
                return None;
            }
            Some(match kind {
                '#' => format!(
                    "#{}",
                    channels
                        .values()
                        .flatten()
                        .find(|c| c.id == id)
                        .and_then(|c| c.name.clone())
                        .unwrap_or_else(|| "不明なチャンネル".into())
                ),
                '&' => format!(
                    "@{}",
                    roles
                        .iter()
                        .find(|r| r.id == id)
                        .map_or("不明なロール", |r| r.name.as_str())
                ),
                _ => format!(
                    "@{}",
                    m.mentions
                        .iter()
                        .find(|u| u.id == id)
                        .map_or("不明なユーザー", |u| u.display())
                ),
            })
        });
        match (name, end) {
            (Some(name), Some(e)) => {
                out.push_str(&name);
                rest = &tail[e + 1..];
            }
            _ => {
                out.push('<');
                rest = &tail[1..];
            }
        }
    }
    out.push_str(rest);
    std::borrow::Cow::Owned(out)
}

/// 本文中の http(s) URL の位置（バイト範囲）。末尾の句読点や対応しない ) は含めない
pub fn find_links(s: &str) -> Vec<std::ops::Range<usize>> {
    let mut out = Vec::new();
    let mut from = 0;
    while let Some(i) = s[from..].find("http").map(|i| from + i) {
        let t = &s[i..];
        let boundary = i == 0
            || !s[..i]
                .chars()
                .next_back()
                .is_some_and(|c| c.is_alphanumeric());
        if !boundary || !(t.starts_with("https://") || t.starts_with("http://")) {
            from = i + 4;
            continue;
        }
        let mut end = t
            .find(|c: char| c.is_whitespace() || matches!(c, '<' | '>' | '"' | '`'))
            .unwrap_or(t.len());
        while let Some(c) = t[..end].chars().next_back() {
            let unbalanced =
                c == ')' && t[..end].matches('(').count() < t[..end].matches(')').count();
            if matches!(
                c,
                '.' | ',' | ':' | ';' | '!' | '?' | '\'' | '*' | '_' | '~' | '|'
            ) || unbalanced
            {
                end -= c.len_utf8();
            } else {
                break;
            }
        }
        if end > t.find("://").unwrap() + 3 {
            out.push(i..i + end);
        }
        from = i + end.max(4);
    }
    out
}

pub fn author_name(m: &Message) -> &str {
    m.member
        .as_ref()
        .and_then(|member| member.nick.as_deref())
        .unwrap_or_else(|| m.author.display())
}

pub fn is_reply(m: &Message) -> bool {
    m.kind == 19 || m.referenced_message.is_some()
}

pub fn rgb(c: u32) -> Color32 {
    Color32::from_rgb((c >> 16) as u8, (c >> 8) as u8, c as u8)
}

pub fn darken(c: Color32) -> Color32 {
    Color32::from_rgb(
        (c.r() as f32 * 0.55) as u8,
        (c.g() as f32 * 0.55) as u8,
        (c.b() as f32 * 0.55) as u8,
    )
}

/// ユーザーIDからアカウント作成日を出す
pub fn snowflake_date(id: &str) -> String {
    let ms = id
        .parse::<u64>()
        .map(|i| (i >> 22) + 1_420_070_400_000)
        .unwrap_or(0);
    chrono::DateTime::from_timestamp_millis(ms as i64)
        .map(|t| t.with_timezone(&Local).format("%Y年%-m月%-d日").to_string())
        .unwrap_or_default()
}

pub const MIB: u64 = 1024 * 1024;
/// サーバーのブーストを最大にしても送れないファイルの大きさ
pub const GUILD_MAX_UPLOAD: u64 = 100 * MIB;

/// アカウントの種類ごとの、1ファイルの上限（0/なし = 無料、1 = Nitro Classic、2 = Nitro、3 = Nitro Basic）
pub fn account_upload_limit(premium_type: Option<u8>) -> u64 {
    match premium_type {
        Some(2) => 500 * MIB,
        Some(1) | Some(3) => 50 * MIB,
        _ => 10 * MIB,
    }
}

/// 1メッセージの文字数の上限（Nitro は 4000）
pub fn message_text_limit(premium_type: Option<u8>) -> usize {
    if premium_type == Some(2) {
        4000
    } else {
        2000
    }
}

/// Discord と同じ数え方（JavaScript の文字列の長さ = UTF-16 の単位数）
pub fn message_text_len(s: &str) -> usize {
    s.encode_utf16().count()
}

pub fn fmt_size(bytes: u64) -> String {
    if bytes >= MIB {
        format!("{:.1}MB", bytes as f64 / MIB as f64)
    } else {
        format!("{}KB", bytes.div_ceil(1024))
    }
}
/// 入力欄の上のお知らせ（添付や文字数の上限）を出しておく時間

pub fn make_avatar(path: &std::path::Path) -> Result<AvatarEdit, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("画像を読めません: {e}"))?;
    let img = image::load_from_memory(&bytes).map_err(|_| {
        "画像として読み込めませんでした（PNG・JPEG・GIF・WebP が使えます）".to_string()
    })?;
    let s = img.width().min(img.height());
    if s == 0 {
        return Err("画像が空です".into());
    }
    let mut sq = img.crop_imm((img.width() - s) / 2, (img.height() - s) / 2, s, s);
    if s > AVATAR_SIZE {
        sq = sq.resize_exact(
            AVATAR_SIZE,
            AVATAR_SIZE,
            image::imageops::FilterType::Lanczos3,
        );
    }
    let rgba = sq.to_rgba8();
    let mut png = Vec::new();
    image::DynamicImage::ImageRgba8(rgba.clone())
        .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .map_err(|e| format!("画像を変換できませんでした: {e}"))?;
    let tex = images::render_image(rgba);
    Ok(AvatarEdit::Set {
        uri: format!("data:image/png;base64,{}", base64_encode(&png)),
        tex,
    })
}

pub fn base64_encode(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for c in data.chunks(3) {
        let n = (c[0] as u32) << 16
            | (*c.get(1).unwrap_or(&0) as u32) << 8
            | *c.get(2).unwrap_or(&0) as u32;
        for i in 0..4 {
            out.push(if i <= c.len() {
                T[(n >> (18 - 6 * i) & 63) as usize] as char
            } else {
                '='
            });
        }
    }
    out
}
