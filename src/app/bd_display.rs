//! BetterDiscord 由来の同梱プラグインのうち、表示を変えるもの（名前の横のタグ・時刻・名前や色の書き換え）。
//! UI はここが返す文字列・色を並べるだけにする
use super::*;
use crate::color::Color32;

/// サーバーの情報（READY と GUILD_CREATE/UPDATE から。オーナー・メンバー数など）
#[derive(Clone, Debug, Default)]
pub struct GuildInfo {
    pub owner_id: Option<String>,
    pub member_count: Option<u64>,
    pub joined_at: Option<String>,
    pub description: Option<String>,
    pub nsfw_level: u8,
    pub premium_tier: u8,
    pub verification_level: u8,
}

/// ボイスの参加者の行に足すもの（EditUsers・MoreRoleColors・AllCallTimeCounter）
#[derive(Clone, Debug, Default)]
pub struct VoiceRowInfo {
    pub name: Option<String>,
    pub color: Option<Color32>,
    pub timer: Option<String>,
}

/// 名前の横に並べる小さなタグ
#[derive(Clone, Debug)]
pub struct Tag {
    pub text: String,
    pub color: Color32,
    /// 枠付きの札にする（ロール名など）
    pub pill: bool,
    pub tip: Option<String>,
}

fn tag(text: impl Into<String>, color: Color32, tip: Option<String>) -> Tag {
    Tag {
        text: text.into(),
        color,
        pill: false,
        tip,
    }
}

/// "a=b; c=d" を読む
pub fn parse_map(s: &str) -> Vec<(String, String)> {
    s.split(';')
        .filter_map(|item| {
            let (k, v) = item.split_once('=')?;
            let (k, v) = (k.trim(), v.trim());
            (!k.is_empty()).then(|| (k.to_string(), v.to_string()))
        })
        .collect()
}

/// "#rrggbb" を色にする
pub fn parse_color(s: &str) -> Option<Color32> {
    let h = s.trim().strip_prefix('#')?;
    if h.len() != 6 {
        return None;
    }
    let v = u32::from_str_radix(h, 16).ok()?;
    Some(Color32::from_rgb((v >> 16) as u8, (v >> 8) as u8, v as u8))
}

fn rgb(v: u32) -> Color32 {
    Color32::from_rgb((v >> 16) as u8, (v >> 8) as u8, v as u8)
}

/// "+9" "-5:30" "+0530" を秒にする
pub fn parse_offset(s: &str) -> Option<i64> {
    let s = s.trim().trim_start_matches("UTC").trim_start_matches("GMT");
    let (sign, rest) = match s.chars().next()? {
        '+' => (1, &s[1..]),
        '-' => (-1, &s[1..]),
        _ => (1, s),
    };
    let (h, m) = if let Some((h, m)) = rest.split_once(':') {
        (h.parse::<i64>().ok()?, m.parse::<i64>().ok()?)
    } else if rest.len() == 4 {
        (rest[..2].parse().ok()?, rest[2..].parse().ok()?)
    } else {
        let f: f64 = rest.parse().ok()?;
        return Some(sign * (f * 3600.0) as i64);
    };
    (h <= 14 && m < 60).then_some(sign * (h * 3600 + m * 60))
}

/// BetterChatNames: "general-chat_room" → "General Chat Room"
pub fn better_channel_name(name: &str) -> String {
    name.split(['-', '_'])
        .filter(|w| !w.is_empty())
        .map(|w| {
            let mut c = w.chars();
            match c.next() {
                Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// enhancecodeblocks: 本文の ``` ブロックの言語名（順番どおり。無ければ空）
pub fn code_langs(content: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = content;
    while let Some(open) = rest.find("```") {
        let Some(close) = rest[open + 3..].find("```").map(|i| open + 3 + i) else {
            break;
        };
        let inner = &rest[open + 3..close];
        let lang = inner
            .split_once('\n')
            .map(|(first, _)| first.trim())
            .filter(|f| {
                !f.is_empty()
                    && f.chars()
                        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '#' | '.' | '_'))
            })
            .unwrap_or("");
        out.push(lang.to_string());
        rest = &rest[close + 3..];
    }
    out
}

/// 公開バッジ（public_flags のビット, 印, 名前）
const BADGES: &[(u64, &str, crate::i18n::Tr)] = &[
    (1 << 0, "⚙", trc!("Discord スタッフ", "Discord Staff", "Discord 员工")),
    (1 << 1, "∞", trc!("パートナー", "Partnered Server Owner", "合作伙伴")),
    (1 << 2, "◆", trc!("HypeSquad イベント", "HypeSquad Events", "HypeSquad 活动")),
    (1 << 3, "🐛", trc!("バグハンター", "Bug Hunter", "漏洞猎人")),
    (1 << 6, "🟪", trc!("HypeSquad Bravery", "HypeSquad Bravery", "HypeSquad Bravery")),
    (1 << 7, "🟥", trc!("HypeSquad Brilliance", "HypeSquad Brilliance", "HypeSquad Brilliance")),
    (1 << 8, "🟩", trc!("HypeSquad Balance", "HypeSquad Balance", "HypeSquad Balance")),
    (1 << 9, "★", trc!("アーリーサポーター", "Early Supporter", "早期支持者")),
    (1 << 14, "🐞", trc!("バグハンター（ゴールド）", "Bug Hunter Gold", "金牌漏洞猎人")),
    (1 << 17, "⌘", trc!("認証済み Bot 開発者", "Verified Bot Developer", "认证机器人开发者")),
    (1 << 18, "🛡", trc!("モデレーター卒業生", "Moderator Alumni", "版主校友")),
    (1 << 22, "</>", trc!("アクティブ開発者", "Active Developer", "活跃开发者")),
];

impl App {
    /// RISCORD_GUILD_INFO（READY から）・GUILD_CREATE・GUILD_UPDATE・メンバーの出入り
    pub fn on_guild_info_event(&mut self, t: &str, d: &serde_json::Value) {
        let read = |g: &serde_json::Value, info: &mut GuildInfo| {
            if let Some(o) = g["owner_id"].as_str() {
                info.owner_id = Some(o.to_owned());
            }
            if let Some(n) = g["member_count"].as_u64() {
                info.member_count = Some(n);
            }
            if let Some(j) = g["joined_at"].as_str() {
                info.joined_at = Some(j.to_owned());
            }
            if let Some(s) = g["description"].as_str() {
                info.description = Some(s.to_owned());
            }
            for (key, slot) in [
                ("nsfw_level", &mut info.nsfw_level),
                ("premium_tier", &mut info.premium_tier),
                ("verification_level", &mut info.verification_level),
            ] {
                if let Some(v) = g[key].as_u64().or_else(|| g["properties"][key].as_u64()) {
                    *slot = v as u8;
                }
            }
        };
        match t {
            "RISCORD_GUILD_INFO" => {
                for g in d.as_array().into_iter().flatten() {
                    if let Some(id) = g["id"].as_str() {
                        read(g, self.guild_info.entry(id.to_owned()).or_default());
                    }
                }
            }
            "GUILD_CREATE" | "GUILD_UPDATE" => {
                if let Some(id) = d["id"].as_str() {
                    let info = self.guild_info.entry(id.to_owned()).or_default();
                    read(d, info);
                    if info.owner_id.is_none() {
                        if let Some(o) = d["properties"]["owner_id"].as_str() {
                            info.owner_id = Some(o.to_owned());
                        }
                    }
                }
            }
            "GUILD_MEMBER_ADD" | "GUILD_MEMBER_REMOVE" => {
                if let Some(info) = d["guild_id"].as_str().and_then(|g| self.guild_info.get_mut(g)) {
                    if let Some(n) = info.member_count.as_mut() {
                        if t == "GUILD_MEMBER_ADD" {
                            *n += 1;
                        } else {
                            *n = n.saturating_sub(1);
                        }
                    }
                }
            }
            _ => {}
        }
    }

    /// EditUsers: 手元だけの名前
    pub fn bd_user_name(&self, user_id: &str) -> Option<String> {
        if !self.plugins.on("edit-users") {
            return None;
        }
        let names = self.plugins.opt_str("edit-users", "names");
        parse_map(&names)
            .into_iter()
            .find(|(k, _)| k == user_id)
            .map(|(_, v)| v)
            .filter(|v| !v.is_empty())
    }

    /// EditUsers の色。MoreRoleColors は呼ぶ側でロールの色を使う
    pub fn bd_user_color(&self, user_id: &str) -> Option<Color32> {
        if !self.plugins.on("edit-users") {
            return None;
        }
        let colors = self.plugins.opt_str("edit-users", "colors");
        parse_map(&colors)
            .into_iter()
            .find(|(k, _)| k == user_id)
            .and_then(|(_, v)| parse_color(&v))
    }

    /// 表示する名前（EditUsers を反映）
    pub fn shown_name(&self, user_id: &str, default: &str) -> String {
        self.bd_user_name(user_id)
            .unwrap_or_else(|| default.to_owned())
    }

    /// EditRoles: ロールの名前と色（手元だけ）
    pub fn bd_role(&self, role: &Role) -> (String, u32) {
        if self.plugins.on("edit-roles") {
            let list = self.plugins.opt_str("edit-roles", "roles");
            if let Some((_, v)) = parse_map(&list).into_iter().find(|(k, _)| *k == role.id) {
                let (name, color) = match v.split_once('|') {
                    Some((n, c)) => (n.trim().to_owned(), c.trim().to_owned()),
                    None if v.starts_with('#') => (String::new(), v),
                    None => (v, String::new()),
                };
                let color = parse_color(&color)
                    .map(|c| ((c.r() as u32) << 16) | ((c.g() as u32) << 8) | c.b() as u32)
                    .unwrap_or(role.color);
                let name = if name.is_empty() { role.name.clone() } else { name };
                return (name, color);
            }
        }
        (role.name.clone(), role.color)
    }

    /// メンバーの一番上のロール（色が付いていなくても）
    fn top_role<'a>(&'a self, member: &Member, guild: &str) -> Option<&'a Role> {
        self.roles
            .get(guild)?
            .iter()
            .filter(|r| member.roles.contains(&r.id))
            .max_by_key(|r| r.position)
    }

    /// 名前の色（EditUsers → EditRoles → ロールの色）
    pub fn bd_name_color(&self, user_id: &str, member: Option<&Member>, guild: Option<&str>) -> Option<Color32> {
        if let Some(c) = self.bd_user_color(user_id) {
            return Some(c);
        }
        if self.plugins.on("edit-roles") {
            let (member, guild) = (member?, guild?);
            let role = self
                .roles
                .get(guild)?
                .iter()
                .filter(|r| member.roles.contains(&r.id) && self.bd_role(r).1 != 0)
                .max_by_key(|r| r.position)?;
            return Some(rgb(self.bd_role(role).1));
        }
        None
    }

    /// ユーザーの横に並べるタグ（メッセージとメンバー一覧で共通）
    pub fn bd_user_tags(&self, user: &User, member: Option<&Member>, guild: Option<&str>) -> Vec<Tag> {
        let p = &self.plugins;
        let mut tags = Vec::new();
        if p.on("status-everywhere") {
            let status = self
                .presences
                .get(&user.id)
                .map(|p| p.status.as_str())
                .unwrap_or("offline");
            let color = match status {
                "online" => rgb(0x23a55a),
                "idle" => rgb(0xf0b232),
                "dnd" => rgb(0xf23f43),
                _ => rgb(0x80848e),
            };
            tags.push(tag("●", color, Some(status.to_owned())));
        }
        if p.on("staff-tag") {
            if let Some(g) = guild {
                let owner = self
                    .guild_info
                    .get(g)
                    .and_then(|i| i.owner_id.as_deref())
                    == Some(user.id.as_str());
                if owner {
                    tags.push(tag(
                        "👑",
                        rgb(0xf0b232),
                        Some(tr!("サーバーのオーナー", "Server owner", "服务器所有者").into()),
                    ));
                } else if p.opt_bool("staff-tag", "admins") {
                    let admin = member.is_some_and(|m| {
                        self.roles.get(g).into_iter().flatten().any(|r| {
                            m.roles.contains(&r.id)
                                && r.permissions.parse::<u64>().is_ok_and(|v| v & 0x8 != 0)
                        })
                    });
                    if admin {
                        tags.push(tag(
                            "🛡",
                            rgb(0x5865f2),
                            Some(tr!("管理者", "Administrator", "管理员").into()),
                        ));
                    }
                }
            }
        }
        if p.on("show-badges-in-chat") && user.public_flags != 0 {
            for (bit, mark, name) in BADGES {
                if user.public_flags & bit != 0 {
                    tags.push(tag(*mark, rgb(0xb5bac1), Some(crate::i18n::pick(name.0, name.1, name.2).into())));
                }
            }
        }
        if p.on("platform-indicators") || p.on("mobile-for-all") {
            let bits = self.presences.get(&user.id).map(|p| p.platforms).unwrap_or(0);
            let mobile_first = p.on("mobile-for-all");
            for (bit, mark, name) in [
                (2u8, "📱", tr!("モバイル", "Mobile", "移动端")),
                (1, "🖥", tr!("デスクトップ", "Desktop", "桌面端")),
                (4, "🌐", tr!("ウェブ", "Web", "网页")),
            ] {
                if bits & bit != 0 {
                    tags.push(tag(mark, rgb(0x949ba4), Some(name.into())));
                    if mobile_first && bit == 2 {
                        break;
                    }
                }
                if !p.on("platform-indicators") && bit == 2 {
                    break;
                }
            }
        }
        if p.on("top-role-everywhere") {
            if let (Some(m), Some(g)) = (member, guild) {
                if let Some(role) = self.top_role(m, g) {
                    let (name, color) = self.bd_role(role);
                    let color = if color == 0 { 0xb5bac1 } else { color };
                    tags.push(Tag {
                        text: name,
                        color: rgb(color),
                        pill: true,
                        tip: None,
                    });
                }
            }
        }
        if p.on("user-tags") {
            let list = p.opt_str("user-tags", "tags");
            for (k, v) in parse_map(&list) {
                if k == user.id && !v.is_empty() {
                    for t in v.split(',').map(str::trim).filter(|t| !t.is_empty()) {
                        tags.push(Tag {
                            text: t.to_owned(),
                            color: rgb(0x00a8fc),
                            pill: true,
                            tip: None,
                        });
                    }
                }
            }
        }
        if p.on("cake-day") {
            if let Some(created) = user
                .id
                .parse::<u64>()
                .ok()
                .and_then(|i| chrono::DateTime::from_timestamp_millis(((i >> 22) + 1_420_070_400_000) as i64))
            {
                let created = created.with_timezone(&Local);
                let now = Local::now();
                use chrono::Datelike;
                if created.month() == now.month() && created.day() == now.day() && created.year() < now.year() {
                    tags.push(tag(
                        "🎂",
                        rgb(0xeb459e),
                        Some(tr!(
                            "アカウント作成 {} 周年",
                            "{} year account anniversary",
                            "账号创建 {} 周年",
                            now.year() - created.year()
                        )),
                    ));
                }
            }
        }
        if p.on("in-my-voice") {
            if let Some((_, mine, _)) = self.voice_channel() {
                let same = self
                    .voice_states
                    .get(&user.id)
                    .and_then(|v| v.channel_id.as_deref())
                    == Some(mine.as_str());
                if same && self.me.as_ref().is_none_or(|me| me.id != user.id) {
                    tags.push(tag(
                        "🔊",
                        rgb(0x23a55a),
                        Some(tr!("同じボイスチャンネルにいます", "In your voice channel", "在你的语音频道中").into()),
                    ));
                }
            }
        }
        if p.on("timezones") {
            let list = p.opt_str("timezones", "zones");
            if let Some(off) = parse_map(&list)
                .into_iter()
                .find(|(k, _)| *k == user.id)
                .and_then(|(_, v)| parse_offset(&v))
            {
                let t = chrono::Utc::now() + chrono::Duration::seconds(off);
                tags.push(tag(
                    format!("🕒{}", t.format("%H:%M")),
                    rgb(0x949ba4),
                    Some(tr!("相手の現地時刻", "Their local time", "对方的当地时间").into()),
                ));
            }
        }
        if p.on("show-me-your-name") {
            let shown = member
                .and_then(|m| m.nick.as_deref())
                .unwrap_or_else(|| user.display());
            if shown != user.username || p.opt_bool("show-me-your-name", "always") {
                tags.push(tag(format!("@{}", user.username), rgb(0x949ba4), None));
            }
        }
        tags
    }

    /// メッセージの投稿者の横に並べるタグ
    pub fn bd_tags(&self, m: &Message) -> Vec<Tag> {
        let guild = m.guild_id.as_deref().or(self.sel_guild.as_deref());
        self.bd_user_tags(&m.author, m.member.as_ref(), guild)
    }

    /// CompleteTimestamps: メッセージの時刻の書き方。None なら本体の既定
    pub fn bd_time(&self, ts: &str, compact: bool) -> Option<String> {
        if !self.plugins.on("complete-timestamps") {
            return None;
        }
        let key = if compact { "compact" } else { "format" };
        let mut fmt = self.plugins.opt_str("complete-timestamps", key);
        if fmt.is_empty() {
            fmt = if compact { "%H:%M:%S".into() } else { "%Y-%m-%d %H:%M:%S".into() };
        }
        let t = local_dt(ts)?;
        use std::fmt::Write;
        let mut out = String::new();
        write!(out, "{}", t.format(&fmt)).ok()?;
        Some(out)
    }

    /// CompleteTimestamps: まとめた行でも時刻を常に出す
    pub fn bd_always_time(&self) -> bool {
        self.plugins.opt_bool("complete-timestamps", "always")
    }

    /// BetterChatNames と EditChannels を反映したチャンネル名
    pub fn channel_label(&self, channel_id: &str, name: &str) -> String {
        if self.plugins.on("edit-channels") {
            let list = self.plugins.opt_str("edit-channels", "names");
            if let Some((_, v)) = parse_map(&list).into_iter().find(|(k, _)| k == channel_id) {
                if !v.is_empty() {
                    return v;
                }
            }
        }
        if self.plugins.on("better-chat-names") {
            return better_channel_name(name);
        }
        name.to_owned()
    }

    /// 一覧に出すアクティビティの文（SpotifyTitleDisplay を反映）
    pub fn bd_activity(&self, p: &crate::model::Presence) -> Option<String> {
        if self.plugins.on("spotify-title-display") {
            if let Some((song, artist)) = &p.spotify {
                return Some(if self.plugins.opt_bool("spotify-title-display", "both") {
                    format!("🎵 {song} — {artist}")
                } else {
                    format!("🎵 {song}")
                });
            }
        }
        p.activity.clone()
    }

    /// MentionFix（知らないユーザーのメンションを手元の名前で埋める）と RoleMentionIcons（ロールのメンションに印）
    pub fn bd_pre_mentions<'a>(&self, content: &'a str, m: &Message, roles: &[Role]) -> std::borrow::Cow<'a, str> {
        let fix = self.plugins.on("mention-fix");
        let icons = self.plugins.on("role-mention-icons");
        if (!fix && !icons) || !content.contains("<@") {
            return std::borrow::Cow::Borrowed(content);
        }
        let mut out = String::with_capacity(content.len());
        let mut rest = content;
        while let Some(i) = rest.find("<@") {
            out.push_str(&rest[..i]);
            let tail = &rest[i..];
            let Some(end) = tail.find('>') else {
                out.push_str(tail);
                rest = "";
                break;
            };
            let token = &tail[2..end];
            let replaced = if let Some(id) = token.strip_prefix('&') {
                icons
                    .then(|| roles.iter().find(|r| r.id == id))
                    .flatten()
                    .map(|r| format!("🏷<@&{}>", r.id))
            } else {
                let id = token.trim_start_matches('!');
                (fix && !m.mentions.iter().any(|u| u.id == id))
                    .then(|| self.user_name(id))
                    .flatten()
                    .map(|name| format!("@{name}"))
            };
            match replaced {
                Some(r) => out.push_str(&r),
                None => out.push_str(&tail[..=end]),
            }
            rest = &tail[end + 1..];
        }
        out.push_str(rest);
        std::borrow::Cow::Owned(out)
    }

    /// HideEmbedLink: 本文が URL 1 つだけで埋め込みがあるなら本文を隠す
    pub fn bd_hide_content(&self, m: &Message) -> bool {
        if !self.plugins.on("hide-embed-link") || !m.embeds.iter().any(|e| e.visible()) {
            return false;
        }
        let c = m.content.trim().trim_start_matches('<').trim_end_matches('>');
        (c.starts_with("https://") || c.starts_with("http://")) && !c.contains(char::is_whitespace)
    }

    /// NoMosaic の「画像を大きく」
    pub fn opt_large_images(&self) -> bool {
        self.plugins.opt_bool("no-mosaic", "large")
    }

    /// ボイスの参加者の行の名前・色・経過時間
    pub fn voice_row_info(&self, guild: Option<&str>, user_id: &str, _user: Option<&User>) -> VoiceRowInfo {
        let mut info = VoiceRowInfo {
            name: self.bd_user_name(user_id),
            ..Default::default()
        };
        if let Some(c) = self.bd_user_color(user_id) {
            info.color = Some(c);
        } else if self.plugins.on("more-role-colors") {
            if let Some(g) = guild {
                let member = self
                    .members
                    .get(g)
                    .into_iter()
                    .flatten()
                    .find(|m| m.user.as_ref().is_some_and(|u| u.id == user_id));
                if let Some(m) = member {
                    info.color = self
                        .bd_name_color(user_id, Some(m), Some(g))
                        .or_else(|| member_role_color(m, self.roles.get(g).map(Vec::as_slice).unwrap_or(&[])));
                }
            }
        }
        if self.plugins.on("all-call-time-counter") {
            if let Some(t) = self.voice_joined.get(user_id) {
                let s = t.elapsed().as_secs();
                info.timer = Some(if s >= 3600 {
                    format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
                } else {
                    format!("{:02}:{:02}", s / 60, s % 60)
                });
            }
        }
        info
    }

    /// EditServers: サーバー名（手元だけ）
    pub fn guild_label(&self, guild_id: &str, name: &str) -> String {
        if self.plugins.on("edit-servers") {
            let list = self.plugins.opt_str("edit-servers", "names");
            if let Some((_, v)) = parse_map(&list).into_iter().find(|(k, _)| k == guild_id) {
                if !v.is_empty() {
                    return v;
                }
            }
        }
        name.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn helpers() {
        assert_eq!(better_channel_name("general-chat_room"), "General Chat Room");
        assert_eq!(parse_offset("+9"), Some(9 * 3600));
        assert_eq!(parse_offset("-5:30"), Some(-(5 * 3600 + 30 * 60)));
        assert_eq!(parse_offset("UTC+0530"), Some(5 * 3600 + 30 * 60));
        assert_eq!(parse_map("1=a; 2 = b ;bad"), vec![("1".into(), "a".into()), ("2".into(), "b".into())]);
        assert!(parse_color("#ff0000").is_some());
        assert!(parse_color("red").is_none());
    }
}
