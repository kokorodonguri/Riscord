//! アプリの状態と、画面に依存しないロジック（通信イベントの処理・送信・メンバー一覧・音声・テーマの適用など）。
//! 描画は src/ui/ にある
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{mpsc::Receiver, Arc};

use chrono::{DateTime, Local};
use gpui::{Bounds, Pixels, RenderImage};
use std::sync::atomic::Ordering::Relaxed;
use tokio::sync::mpsc::UnboundedSender;

use crate::color::Color32;
use crate::login::{self, LoginEvent};
use crate::model::{
    self, Channel, Embed, Guild, Member, Message, Presence, ProfileData, Role, User, VoiceState,
};
use crate::net::{self, Cmd, Event, VoiceStatus};
use crate::wake::Waker;
use crate::{cache, clip, images, theme, voice};

pub const MESSAGE_RELOAD_ERROR: &str =
    "エラー: メッセージを更新できませんでした。再読み込みしてください。";

/// 選べるステータス（キー, 表示名）
pub const STATUSES: [(&str, &str); 4] = [
    ("online", "オンライン"),
    ("idle", "退席中"),
    ("dnd", "取り込み中"),
    ("invisible", "オフライン表示"),
];

pub fn status_label(s: &str) -> &'static str {
    match s {
        "online" => "オンライン",
        "idle" => "退席中",
        "dnd" => "取り込み中",
        "invisible" => "オフライン表示",
        _ => "オフライン",
    }
}

// ---- トークン ----
pub fn token_path(profile: Option<&str>) -> PathBuf {
    crate::profile::account_dir(profile).join("token")
}

/// 設定フォルダの親。テスト中は本物の設定（トークン・settings.json・テーマ）に触らないよう一時フォルダにする
pub fn config_root() -> PathBuf {
    if cfg!(test) {
        return std::env::temp_dir().join(format!("riscord-test-config-{}", std::process::id()));
    }
    if crate::demo::enabled() {
        // デモでは本物の設定・トークンを読み書きしない
        return std::env::temp_dir().join(format!("riscord-demo-config-{}", std::process::id()));
    }
    dirs::config_dir().unwrap_or_else(|| PathBuf::from("."))
}

/// 控え（起動用のスナップショット・アイコン）を置くフォルダの親。テスト・デモでは本物に触らない
pub fn cache_root() -> PathBuf {
    if cfg!(test) {
        return std::env::temp_dir().join(format!("riscord-test-cache-{}", std::process::id()));
    }
    if crate::demo::enabled() {
        return std::env::temp_dir().join(format!("riscord-demo-cache-{}", std::process::id()));
    }
    dirs::cache_dir().unwrap_or_else(config_root)
}

pub fn load_token(profile: Option<&str>) -> Option<String> {
    std::fs::read_to_string(token_path(profile))
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

pub fn save_token(profile: Option<&str>, token: &str) -> std::io::Result<()> {
    use std::io::Write;
    let p = token_path(profile);
    std::fs::create_dir_all(p.parent().unwrap())?;
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600); // 自分だけ読める
    }
    opts.open(&p)?.write_all(token.as_bytes())
}

// ---- 設定 ----
#[derive(Clone, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct Settings {
    pub enabled_plugins: Vec<String>,
    pub zoom: f32,
    pub font_size: f32,
    pub compact: bool,
    /// スクロールの速さ（既定に掛ける倍率）。マウスホイールとタッチパッドで別
    pub wheel_speed: f32,
    pub touchpad_speed: f32,
    /// テーマ: "builtin:black" などの組み込み、または themes/ のファイル名
    pub theme: String,
    /// 設定画面で色ごとに変えたもの（キー → #rrggbb）。テーマより優先
    pub color_overrides: std::collections::BTreeMap<String, String>,
    /// 設定画面で指定したチャット欄の背景画像（テーマより優先）
    pub background: theme::Background,
    pub status: String,
    /// アクティビティに「Riscord をプレイ中」を出す
    pub show_activity: bool,
    /// 次に開始する画面共有の画質
    pub stream_quality: crate::golive::media::QualityPreset,
    // ---- 音声 ----
    pub input_device: Option<String>,
    pub output_device: Option<String>,
    /// true = プッシュトゥトーク
    pub ptt: bool,
    pub ptt_key: Option<String>,
    /// 音声検出のしきい値 (dB)
    pub vad_db: f32,
    pub in_gain: f32,
    pub out_gain: f32,
    pub self_mute: bool,
    pub self_deaf: bool,
    /// サーバーレールの並び（ドラッグで並べ替えたもの）。ここにないサーバーは後ろに元の順で並ぶ。
    /// Riscord の中だけの並びで、Discord の設定には書かない
    pub guild_order: Vec<String>,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            enabled_plugins: Vec::new(),
            zoom: 1.0,
            font_size: 16.0,
            compact: false,
            // 1ノッチ 100px（ブラウザ版 Discord と同じくらい）
            wheel_speed: 2.5,
            touchpad_speed: 2.0,
            theme: "builtin:black".into(),
            color_overrides: Default::default(),
            background: Default::default(),
            status: "online".into(),
            show_activity: true,
            stream_quality: Default::default(),
            input_device: None,
            output_device: None,
            ptt: false,
            ptt_key: None,
            vad_db: -40.0,
            in_gain: 1.0,
            out_gain: 1.0,
            self_mute: false,
            self_deaf: false,
            guild_order: Vec::new(),
        }
    }
}

pub fn db_to_lin(db: f32) -> f32 {
    10f32.powf(db / 20.0)
}

pub fn lin_to_db(v: f32) -> f32 {
    20.0 * v.max(1e-6).log10()
}

pub fn settings_path(profile: Option<&str>) -> PathBuf {
    token_path(profile).with_file_name("settings.json")
}

pub fn load_settings(profile: Option<&str>) -> Settings {
    let p = settings_path(profile);
    let read = |path: &std::path::Path| {
        std::fs::read(path)
            .ok()
            .and_then(|s| serde_json::from_slice(&s).ok())
    };
    if let Some(settings) = read(&p) {
        return settings;
    }
    if p.exists() {
        eprintln!(
            "設定を読めませんでした。バックアップから復旧します: {}",
            p.display()
        );
    }
    read(&p.with_extension("json.bak")).unwrap_or_default()
}

pub fn save_settings(profile: Option<&str>, s: &Settings) -> std::io::Result<()> {
    let p = settings_path(profile);
    std::fs::create_dir_all(p.parent().unwrap())?;
    let bytes = serde_json::to_vec_pretty(s)?;
    let tmp = p.with_extension("json.tmp");
    crate::cache::write_private(&tmp, &bytes)?;
    if let Ok(old) = std::fs::read(&p) {
        if serde_json::from_slice::<Settings>(&old).is_ok() {
            let backup_tmp = p.with_extension("json.bak.tmp");
            crate::cache::write_private(&backup_tmp, &old)?;
            std::fs::rename(backup_tmp, p.with_extension("json.bak"))?;
        }
    }
    std::fs::rename(tmp, p)
}

pub fn themes_dir() -> PathBuf {
    // テーマはどのプロファイル（アカウント）でも共有する
    theme::themes_dir(&crate::profile::shared_dir())
}

/// themes/ の中身が変わったら知らせる（1秒ごとに更新時刻を見るだけの軽いスレッド）
pub fn watch_themes(ctx: Waker) -> Receiver<()> {
    let (tx, rx) = std::sync::mpsc::channel();
    let dir = themes_dir();
    let stamp = move || -> Vec<(PathBuf, std::time::SystemTime)> {
        let mut v: Vec<_> = std::fs::read_dir(&dir)
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|e| Some((e.path(), e.metadata().ok()?.modified().ok()?)))
            .collect();
        v.sort();
        v
    };
    let _ = std::thread::Builder::new()
        .name("theme-watch".into())
        .spawn(move || {
            let mut last = stamp();
            loop {
                std::thread::sleep(std::time::Duration::from_secs(1));
                let now = stamp();
                if now != last {
                    last = now;
                    if tx.send(()).is_err() {
                        return;
                    }
                    ctx.wake();
                }
            }
        });
    rx
}

/// OS のファイル選択ダイアログを別スレッドで開く（開いている間も画面を止めない）。選ばれたファイルが届く（取り消しなら空）
pub fn spawn_file_dialog(
    ctx: &Waker,
    title: &str,
    many: bool,
    filter: Option<(&str, &[&str])>,
) -> Receiver<Vec<PathBuf>> {
    let (tx, rx) = std::sync::mpsc::channel();
    let ctx = ctx.clone();
    let mut dialog = rfd::AsyncFileDialog::new().set_title(title);
    if let Some((name, exts)) = filter {
        dialog = dialog.add_filter(name, exts);
    }
    std::thread::spawn(move || {
        // Linux では xdg-desktop-portal と D-Bus でやり取りするので、その間だけ小さなランタイムを使う
        let Ok(rt) = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        else {
            return;
        };
        let paths: Vec<PathBuf> = if many {
            rt.block_on(dialog.pick_files())
                .unwrap_or_default()
                .into_iter()
                .map(|f| f.path().to_path_buf())
                .collect()
        } else {
            rt.block_on(dialog.pick_file())
                .into_iter()
                .map(|f| f.path().to_path_buf())
                .collect()
        };
        let _ = tx.send(paths);
        ctx.wake();
    });
    rx
}

/// フォルダをファイルマネージャで開く
pub fn open_folder(path: &std::path::Path) {
    let _ = std::fs::create_dir_all(path);
    #[cfg(windows)]
    let cmd = "explorer";
    #[cfg(target_os = "macos")]
    let cmd = "open";
    #[cfg(all(unix, not(target_os = "macos")))]
    let cmd = "xdg-open";
    let _ = std::process::Command::new(cmd).arg(path).spawn();
}

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
        Some(h) => format!(
            "https://cdn.discordapp.com/avatars/{}/{}.png?size=80",
            u.id, h
        ),
        None => {
            let n = u.id.parse::<u64>().map(|i| (i >> 22) % 6).unwrap_or(0);
            format!("https://cdn.discordapp.com/embed/avatars/{n}.png")
        }
    }
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

pub fn member_role_color(member: &Member, roles: &[Role]) -> Option<Color32> {
    let role = roles
        .iter()
        .filter(|r| member.roles.contains(&r.id))
        .max_by_key(|r| r.position)?;
    (role.color != 0).then(|| {
        Color32::from_rgb(
            (role.color >> 16) as u8,
            (role.color >> 8) as u8,
            role.color as u8,
        )
    })
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

pub const MEMBER_ROW_H: f32 = 42.0;
pub const MEMBER_HEAD_H: f32 = 40.0;

/// メンバーリストの1行。Member は members[gid] の添字
pub enum MemberRow {
    Header(String),
    Member(usize),
    /// Discord からまだ届いていない行
    Placeholder,
}

/// Discord が並べて送ってくるメンバー一覧（GUILD_MEMBER_LIST_UPDATE を当てたもの）
#[derive(Default)]
pub struct ServerMemberList {
    /// どのチャンネルの一覧として購読したか
    pub channel_id: String,
    /// 一覧の ID（見える人が同じチャンネルは同じ一覧）。最初の SYNC で決まり、違う ID の差分は無視する
    pub list_id: Option<String>,
    pub groups: Vec<(String, u32)>,
    /// 行ごとの中身。None はまだ届いていない
    pub items: Vec<Option<net::ListItem>>,
}

impl ServerMemberList {
    /// 差分を当てる。当てたら true
    pub fn apply(
        &mut self,
        list_id: &str,
        groups: Vec<(String, u32)>,
        ops: Vec<net::ListOp>,
    ) -> bool {
        match &self.list_id {
            Some(id) if id != list_id => return false,
            None if !ops.iter().any(|o| matches!(o, net::ListOp::Sync { .. })) => return false,
            _ => {}
        }
        self.list_id = Some(list_id.to_string());
        for op in ops {
            match op {
                net::ListOp::Sync { range, items } => {
                    let start = range[0] as usize;
                    if self.items.len() < start + items.len() {
                        self.items.resize(start + items.len(), None);
                    }
                    for (k, item) in items.into_iter().enumerate() {
                        self.items[start + k] = Some(item);
                    }
                }
                net::ListOp::Insert { index, item } => {
                    let i = index.min(self.items.len());
                    self.items.insert(i, Some(item));
                }
                net::ListOp::Update { index, item } => {
                    if index >= self.items.len() {
                        self.items.resize(index + 1, None);
                    }
                    self.items[index] = Some(item);
                }
                net::ListOp::Delete { index } => {
                    if index < self.items.len() {
                        self.items.remove(index);
                    }
                }
                net::ListOp::Invalidate { range } => {
                    let end = (range[1] as usize + 1).min(self.items.len());
                    for item in self.items.iter_mut().take(end).skip(range[0] as usize) {
                        *item = None;
                    }
                }
            }
        }
        // 全体の行数 = 各グループの見出し + 人数。まだ届いていない所は None のまま
        if !groups.is_empty() {
            let total: usize = groups
                .iter()
                .filter(|g| g.1 > 0)
                .map(|g| g.1 as usize + 1)
                .sum();
            self.items.resize(total, None);
            self.groups = groups;
        }
        true
    }
}

/// グループ分け・並べ替え済みのメンバーリスト（ver が変わるまで使い回す）
#[derive(Default)]
pub struct MemberListCache {
    pub gid: String,
    pub ver: u64,
    pub rows: Vec<MemberRow>,
    /// 各行の上端（仮想スクロール用の累積高さ）
    pub tops: Vec<f32>,
    pub height: f32,
}

impl MemberListCache {
    /// Discord が並べた一覧から作る（行の順番 = 一覧の順番。見えている行の番号をそのまま購読に使える）
    pub fn from_server(
        gid: &str,
        ver: u64,
        list: &ServerMemberList,
        members: &[Member],
        roles: &[Role],
    ) -> Self {
        let index: HashMap<&str, usize> = members
            .iter()
            .enumerate()
            .filter_map(|(i, m)| m.user.as_ref().map(|u| (u.id.as_str(), i)))
            .collect();
        let mut out = MemberListCache {
            gid: gid.to_string(),
            ver,
            ..Default::default()
        };
        for item in &list.items {
            out.tops.push(out.height);
            match item {
                Some(net::ListItem::Group(id, count)) => {
                    // 一覧の中の見出しの行には人数が入っていないことがある。人数は一緒に届く groups のほうが正しい
                    let count = list
                        .groups
                        .iter()
                        .find(|g| &g.0 == id)
                        .map_or(*count, |g| g.1);
                    let name = match id.as_str() {
                        "online" => "オンライン",
                        "offline" => "オフライン",
                        id => roles
                            .iter()
                            .find(|r| r.id == id)
                            .map_or("メンバー", |r| r.name.as_str()),
                    };
                    out.height += MEMBER_HEAD_H;
                    out.rows
                        .push(MemberRow::Header(format!("{name} — {count}")));
                }
                Some(net::ListItem::Member(uid)) => {
                    out.height += MEMBER_ROW_H;
                    out.rows.push(
                        index
                            .get(uid.as_str())
                            .map_or(MemberRow::Placeholder, |&i| MemberRow::Member(i)),
                    );
                }
                None => {
                    out.height += MEMBER_ROW_H;
                    out.rows.push(MemberRow::Placeholder);
                }
            }
        }
        out.height += 8.0;
        out
    }

    pub fn build(
        gid: &str,
        ver: u64,
        members: &[Member],
        roles: &[Role],
        presences: &HashMap<String, Presence>,
    ) -> Self {
        let mut hoisted: Vec<&Role> = roles.iter().filter(|r| r.hoist).collect();
        hoisted.sort_by_key(|r| std::cmp::Reverse(r.position));
        let online_index = hoisted.len();
        let mut groups: Vec<Vec<usize>> = vec![Vec::new(); online_index + 2];
        for (i, member) in members.iter().enumerate() {
            let Some(user) = &member.user else { continue };
            let online = presences
                .get(&user.id)
                .is_some_and(|p| p.status != "offline");
            let index = if online {
                hoisted
                    .iter()
                    .position(|r| member.roles.contains(&r.id))
                    .unwrap_or(online_index)
            } else {
                online_index + 1
            };
            groups[index].push(i);
        }
        let names = hoisted
            .iter()
            .map(|r| r.name.as_str())
            .chain(["オンライン", "オフライン"]);
        let mut out = MemberListCache {
            gid: gid.to_string(),
            ver,
            ..Default::default()
        };
        for (name, mut group) in names.zip(groups) {
            if group.is_empty() {
                continue;
            }
            group.sort_by_cached_key(|&i| members[i].display().to_lowercase());
            out.tops.push(out.height);
            out.height += MEMBER_HEAD_H;
            out.rows
                .push(MemberRow::Header(format!("{name} — {}", group.len())));
            for i in group {
                out.tops.push(out.height);
                out.height += MEMBER_ROW_H;
                out.rows.push(MemberRow::Member(i));
            }
        }
        out.height += 8.0;
        out
    }
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

pub enum ProfileState {
    Loading,
    Ready(ProfileData),
    Failed,
}

#[derive(Clone)]
pub struct ProfilePopup {
    pub user: User,
    pub guild_id: Option<String>,
    pub anchor: Bounds<Pixels>,
    /// 左下の自分のアバターから開いたときは上に出す
    pub above: bool,
}

/// 既読を送る最短の間隔。新着が続いても、この間隔で最新のものだけ送る
pub const ACK_INTERVAL: std::time::Duration = std::time::Duration::from_millis(1500);
const ACK_RETRY_DELAY: std::time::Duration = std::time::Duration::from_secs(30);

/// 過去ログの読み足し
#[derive(Default)]
pub struct History {
    /// 読み込み中の起点（このIDより前を頼んでいる）
    pub pending: Option<String>,
    /// チャンネルの始まりまで読んだ
    pub end: bool,
    /// 失敗したらしばらく待ってから取り直す
    pub retry_at: Option<std::time::Instant>,
    /// メッセージへ飛んで途中だけを読んだ: これより新しいメッセージがある（最新とつながっていない）
    pub newer: bool,
    /// 続きの読み込み中の起点（このIDより後を頼んでいる）
    pub pending_newer: Option<String>,
    pub newer_retry_at: Option<std::time::Instant>,
}

/// 上端からこの行数まで近づいたら古いメッセージを読み足す
pub const HISTORY_TRIGGER_ROWS: usize = 20;
/// 一覧の先頭に置く、読み込み中の表示・チャンネルの始まりの表示の高さ（どちらでも同じにして行がずれないようにする）
pub const HISTORY_HEAD_H: f32 = 72.0;
/// 表示中のチャンネルで遡って持てるメッセージ数の上限
pub const MAX_HISTORY: usize = 5000;
/// 1チャンネルで保持するメッセージ数の上限
pub const MAX_MSGS: usize = 1000;
/// メッセージを保持しておくチャンネル数の上限
pub const MAX_CHANNELS: usize = 30;
/// 1メッセージに添付できるファイル数（Discord の上限）
pub const MAX_UPLOADS: usize = 10;
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
/// 「コピーしました」などを出しておく時間
pub const TOAST_TTL: std::time::Duration = std::time::Duration::from_millis(1600);
pub const COMPOSE_NOTICE_TTL: std::time::Duration = std::time::Duration::from_secs(8);
/// 受信中のメンバー索引は、更新が止まってから短時間で破棄する。
pub const MEMBER_INDEX_TTL: std::time::Duration = std::time::Duration::from_secs(5);

pub struct RecentMemberIndex {
    pub guild_id: String,
    pub index: HashMap<String, usize>,
    pub updated: std::time::Instant,
}

// ---- アプリ本体 ----
pub struct Backend {
    pub cmd: UnboundedSender<Cmd>,
    pub ev: Receiver<Event>,
}

pub enum MessageChange {
    Create(Message),
    Update {
        id: String,
        content: Option<String>,
        embeds: Option<Vec<Embed>>,
        pinned: Option<bool>,
    },
    Delete(String),
    Reaction {
        id: String,
        op: model::ReactionOp,
    },
}

impl MessageChange {
    pub fn apply(self, messages: &mut Vec<Message>) {
        match self {
            Self::Create(message) => {
                if !messages.iter().any(|m| m.id == message.id) {
                    let at = messages.partition_point(|m| {
                        (m.id.len(), m.id.as_str()) < (message.id.len(), message.id.as_str())
                    });
                    messages.insert(at, message);
                }
            }
            Self::Update {
                id,
                content,
                embeds,
                pinned,
            } => {
                if let Some(message) = messages.iter_mut().find(|m| m.id == id) {
                    if let Some(content) = content {
                        if message.content != content {
                            message.content = content;
                            message.edited_timestamp = Some(String::new());
                        }
                    }
                    if let Some(embeds) = embeds {
                        message.embeds = embeds;
                    }
                    if let Some(p) = pinned {
                        message.pinned = p;
                    }
                }
            }
            Self::Delete(id) => messages.retain(|m| m.id != id),
            Self::Reaction { id, op } => {
                if let Some(message) = messages.iter_mut().find(|m| m.id == id) {
                    model::apply_reaction(&mut message.reactions, op);
                }
            }
        }
    }
}

pub struct ComposeSubmission {
    pub content: String,
    pub reply: Option<Message>,
    pub editing: Option<Message>,
    pub uploads: Vec<clip::Upload>,
    pub failed: bool,
    /// Discord に断られた理由（あれば確実に届いていない）
    pub rejected: Option<String>,
}

#[derive(Default)]
pub struct App {
    pub plugins: crate::plugins::Plugins,
    pub translator: crate::translator::Translator,
    /// MessageLogger の記録（削除・編集）
    pub message_log: crate::message_log::MessageLog,
    /// 別スレッドから画面を描き直させる口（描画側が作って渡す）
    pub waker: Waker,
    /// ウィンドウの幅（拡大率を除いた論理ピクセル）。狭い画面のときの並べ方に使う
    pub screen_w: f32,
    /// テーマ・フォントを適用し直してほしい（描画側で処理する）
    pub theme_dirty: bool,
    pub font_dirty: bool,
    /// 入力欄にフォーカスを移してほしい（描画側で処理する）
    pub focus_input: bool,
    pub backend: Option<Backend>,

    pub token_input: String,
    pub remember: bool,
    /// ログイン画面（QR コード・メールアドレス）の状態
    pub login: LoginState,
    pub login_error: Option<String>,

    pub me: Option<User>,
    pub guilds: Vec<Guild>,
    pub dms: Vec<Channel>,
    pub channels: HashMap<String, Vec<Channel>>,
    pub roles: HashMap<String, Vec<Role>>,
    pub members: HashMap<String, Vec<Member>>,
    pub member_index: Option<RecentMemberIndex>,
    /// サーバーごとのメンバー一覧（Discord が並べて送ってくるもの。購読中のチャンネルの分）
    pub member_lists: HashMap<String, ServerMemberList>,
    /// 最後に送った購読（サーバー, チャンネル, 範囲）
    pub member_sub: Option<(String, String, Vec<[u32; 2]>)>,
    /// メンバー一覧で見えている行（先頭, 末尾）。この範囲を購読する
    pub member_view: (usize, usize),
    pub show_members: bool,
    pub show_members_narrow: bool,
    pub compact_sidebar_closed: bool,
    pub messages: HashMap<String, Vec<Message>>,
    /// messages のどこかが変わるたびに進む番号（描画側はこれが同じなら行の目印を作り直さない）
    pub msg_rev: u64,
    /// チャンネルごとの過去ログ読み足しの状態
    pub history: HashMap<String, History>,
    /// 見る権限がなかったチャンネル（開くと 403）
    pub no_access: HashSet<String>,
    pub message_errors: HashSet<String>,
    pub message_status_channel: Option<String>,
    pub pending_message_loads: HashMap<String, Vec<MessageChange>>,
    /// 自分で押したリアクションの API 呼び出し（id → チャンネル・メッセージ・当てた操作）。失敗したら戻す
    pub pending_reactions: HashMap<u64, (String, String, model::ReactionOp)>,
    /// チャンネルごとに表示した添付・埋め込み画像の URI。チャンネルを捨てるときに画像キャッシュも解放する
    pub channel_images: HashMap<String, HashSet<String>>,
    /// 未読チャンネル → 所属サーバー（DMならNone）
    pub unread: HashMap<String, Option<String>>,
    /// DM 一覧の絞り込み
    pub dm_search: bool,
    pub dm_filter: String,
    /// 戻る/進むの履歴（サーバー, チャンネル）
    pub nav_back: Vec<(Option<String>, Option<String>)>,
    pub nav_fwd: Vec<(Option<String>, Option<String>)>,
    /// 返信先・編集中のメッセージ
    pub reply_to: Option<Message>,
    pub editing: Option<Message>,
    pub pending_delete: Option<Message>,
    pub compose_submissions: HashMap<String, ComposeSubmission>,
    /// Ctrl+K（検索文字, 選択中の行）
    pub switcher: Option<(String, usize)>,
    /// メッセージを読み込んだチャンネル（古い順）。上限を超えたら古いものから捨てる
    pub loaded_channels: std::collections::VecDeque<String>,
    /// DM のプロフィール欄を閉じたか
    pub hide_dm_profile: bool,
    /// メンバー・ロール・オンライン状態が変わるたびに増える（メンバーリストの作り直しの合図）
    pub member_ver: u64,
    pub member_cache: MemberListCache,
    pub sel_guild: Option<String>, // None = DM
    pub sel_channel: Option<String>,
    pub input: String,
    /// サーバーごとのカスタム絵文字（ピッカーと :名前: の変換に使う）
    pub guild_emojis: HashMap<String, Vec<crate::model::Emoji>>,
    /// guild_emojis が変わるたびに増える（ピッカーの作り直しの合図）
    pub emoji_rev: u64,
    /// 絵文字を REST で取りに行ったサーバー（READY に無かったときの補い。二重に頼まない）
    pub emoji_requested: HashSet<String>,
    /// ピッカーの検索欄
    pub emoji_query: String,
    /// 送信待ちの添付と、それを貼り付けたチャンネル（別のチャンネルに移ったら捨てる）
    pub uploads: Vec<clip::Upload>,
    /// 添付や文字数の上限についてのお知らせ（入力欄の上に出す）
    pub compose_notice: Option<(String, std::time::Instant)>,
    /// チャンネルごとに、最後に既読として Discord に送ったメッセージID
    pub acked: HashMap<String, String>,
    /// API エラー後に再試行できる時刻（チャンネルごと）
    pub ack_retry_at: HashMap<String, std::time::Instant>,
    /// 最後に既読を送った時刻（送りすぎて制限されないよう間を空ける）
    pub last_ack: Option<std::time::Instant>,
    /// 今使っている見た目（テーマ＋上書き）。変わったときだけ適用し直す
    pub theme_applied: Option<theme::Resolved>,
    pub theme_error: Option<String>,
    /// テーマファイルの読み飛ばした項目（知らない部品・読めない値）
    pub theme_warnings: Vec<String>,
    /// themes/ のファイルが変わったら届く（保存するとすぐ反映）
    pub theme_watch: Option<Receiver<()>>,
    /// 開いているファイル選択ダイアログの結果の受け口
    pub file_pick: Option<(String, Receiver<Vec<PathBuf>>)>,
    pub picked_files: Option<(String, Vec<PathBuf>)>,
    /// 背景画像を選ぶダイアログの結果の受け口
    pub bg_pick: Option<Receiver<Vec<PathBuf>>>,
    pub upload_channel: Option<String>,
    pub status: String,
    /// ゲートウェイから届いた接続の状態（接続中・再接続待ちなど）。ステータス帯のお知らせを閉じるとこれに戻す
    pub gateway_status: String,
    /// DM 一覧が一度でも届いたか（届く前は読み込み中、届いて 0 件なら空と出し分ける）
    pub dms_loaded: bool,
    /// 画面下に短く出す操作の結果（「コピーしました」など）
    pub toast: Option<(String, std::time::Instant)>,
    /// 次の描画でクリップボードに書くもの（gpui の cx が要るので画面側で書く）
    pub clipboard_out: Option<String>,
    /// サーバーごとの自分の通知設定（READY と USER_GUILD_SETTINGS_UPDATE から）
    pub guild_settings: HashMap<String, GuildSettings>,
    /// 未読のメンション数（チャンネル → (サーバー, 件数)。DM はサーバーが None で、メッセージごとに数える）
    pub mention_counts: HashMap<String, (Option<String>, u32)>,
    /// サーバーごとの自分のロール（ロールへのメンションを見分ける）
    pub my_roles: HashMap<String, Vec<String>>,
    /// ウィンドウにフォーカスがあるか（見ているチャンネルの通知は出さない）
    pub window_focused: bool,
    /// 押されたデスクトップ通知の行き先
    pub notify_clicked: crate::notify::Clicked,
    /// Cmd::Api に付ける番号
    pub next_api_id: u64,
    /// 脱退の確認を出しているサーバー
    pub confirm_leave: Option<String>,

    pub settings: Settings,
    pub stickers: HashMap<String, StickerState>,
    pub presences: HashMap<String, Presence>,
    pub profiles: HashMap<String, ProfileState>,
    pub popup: Option<ProfilePopup>,
    pub settings_open: bool,
    pub settings_tab: usize,
    /// 設定の左上の検索欄
    pub settings_query: String,
    /// ログアウトの確認を出している
    pub confirm_logout: bool,
    /// 設定 → プロフィールで編集中の内容（プロフィールを読み込んでから作る）
    pub profile_draft: Option<ProfileDraft>,
    pub profile_saving: bool,
    pub profile_error: Option<String>,
    pub profile_saved_at: Option<std::time::Instant>,
    /// 保存せずに閉じようとした時刻（保存バーを赤くして知らせる）
    pub profile_nag: Option<std::time::Instant>,
    pub avatar_pick: Option<Receiver<Vec<PathBuf>>>,

    // ---- 音声 ----
    pub voice: Arc<voice::VoiceCtl>,
    /// user_id → どのボイスチャンネルにいるか
    pub voice_states: HashMap<String, VoiceState>,
    pub voice_index: HashMap<String, Vec<String>>,
    /// 自分の接続状態（None = 未接続）
    pub voice_status: Option<VoiceStatus>,
    /// 接続確立後の経過時間。チャンネル変更・切断でリセットする。
    pub voice_call_started: Option<(Option<String>, String, std::time::Instant)>,
    /// DM通話: channel_id → 呼び出し中のユーザー
    pub calls: HashMap<String, Vec<String>>,
    pub mic_test: Option<voice::MicTest>,
    /// 自分の画面共有（None = していない）
    pub my_stream: Option<crate::golive::Phase>,
    /// 自分の配信を見ている人
    pub stream_viewers: Vec<String>,
    /// 見ている配信（配信の鍵, 状態）
    pub watching: Option<(String, crate::golive::Phase)>,
    pub audio_devs: Option<(Vec<String>, Vec<String>)>,
    pub binding_ptt: bool,
    /// 前回の控えから出しているだけで、まだ読み直していないチャンネル（開いたら取り直す・既読は送らない）
    pub stale_channels: HashSet<String>,
    /// サーバー → 知っているスレッド（参加中・開いたもの）
    pub threads: HashMap<String, Vec<Channel>>,
    /// チャット右側の枠
    pub side: SidePanel,
    pub pins: Option<PinsView>,
    pub search: Option<SearchView>,
    /// 参加中のスレッド
    pub joined_threads: HashSet<String>,
    /// 開いているフォーラムの投稿一覧
    pub forum: Option<ThreadsView>,
    /// チャンネルのスレッド一覧（右の枠）
    pub thread_panel: Option<ThreadsView>,
    /// フォーラムの新しい投稿（開いている間だけ）と、題・本文の欄の中身
    pub post_draft: Option<PostDraft>,
    pub post_title: String,
    pub post_body: String,
    /// メッセージからスレッドを作る画面と、名前の欄の中身
    pub thread_draft: Option<ThreadDraft>,
    pub thread_name: String,
    /// 画面に頼む: このチャンネルを開く（戻る・進むの履歴に残すため Root で開く）
    pub pending_open: Option<String>,
    /// ゲートウェイのセッション（スラッシュコマンドの実行で送る）
    pub session_id: Option<String>,
    /// 場所ごとのスラッシュコマンド（"g{サーバー}" / "c{DM}" / "u" = 自分のアカウントに入れたアプリ）
    pub command_index: HashMap<String, CommandIndex>,
    /// 入力中のコマンド
    pub cmd_draft: Option<CommandDraft>,
    /// 実行して返事待ちのコマンド（nonce → 中身）
    pub pending_interactions: HashMap<String, PendingInteraction>,
    /// 検索欄の中身
    pub search_query: String,
    /// 打つのを止めたあと、この時刻に検索する
    pub search_due: Option<std::time::Instant>,
    /// 飛ぶ先を探している途中
    pub jump: Option<Jump>,
    /// 画面に頼む: このチャンネルのこのメッセージまでスクロールする
    pub scroll_to_message: Option<(String, String)>,
    /// 飛んだ先の行（しばらく目立たせる）
    pub jump_flash: Option<(String, std::time::Instant)>,
    /// 同じく、控えから出しているサーバーのチャンネル一覧
    pub stale_guilds: HashSet<String>,
    /// 今のトークンの持ち主のユーザー ID（控えを書くときに付ける）
    pub token_owner: Option<String>,
    /// このアカウントのプロファイル（None は既定）。トークン・設定・控えの置き場所が決まる
    pub profile: Option<String>,
    /// 最後に控えを書いた時刻と中身の目印（変わっていなければ書かない）
    pub cache_saved: Option<(std::time::Instant, u64)>,
    /// 控えを出したまま接続できなかったとき、この時刻にもう一度つなぐ
    pub login_retry_at: Option<std::time::Instant>,
    /// 前回書いたあと、控えに入る中身が変わったかもしれない
    pub cache_dirty: bool,
}

/// 控えを書き直す間隔
pub const CACHE_SAVE_INTERVAL: std::time::Duration = std::time::Duration::from_secs(10);
/// 控えを出したまま接続できなかったとき、つなぎ直すまでの時間
pub const LOGIN_RETRY_DELAY: std::time::Duration = std::time::Duration::from_secs(5);

/// msg_rev の元。ログアウトで App を作り直しても前の値に戻らないよう、プロセスで1つ
static MSG_REV: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

impl App {
    /// messages を変えたら呼ぶ
    pub fn bump_msgs(&mut self) {
        self.msg_rev = MSG_REV.fetch_add(1, Relaxed) + 1;
    }

    /// profile = このアカウントのプロファイル（None は既定）。分割表示では画面ごとに違う
    pub fn new(waker: Waker, profile: Option<String>) -> Self {
        let settings = load_settings(profile.as_deref());
        let mut app = App {
            waker,
            screen_w: 1200.0,
            remember: true,
            settings,
            show_members: true,
            ..Default::default()
        };
        app.set_profile(profile);
        app.plugins.reload(&app.settings.enabled_plugins);
        app.theme_watch = Some(watch_themes(app.waker.clone()));
        app.apply_theme(false);
        if crate::demo::enabled() {
            app.voice.set_ctx(app.waker.clone());
            let (cmd, ev) = crate::demo::spawn(app.waker.clone());
            app.backend = Some(Backend { cmd, ev });
            // RISCORD_DEMO=guild なら最初からサーバーのチャンネルを、dm なら DM を開いておく（画面確認用）
            match std::env::var("RISCORD_DEMO").as_deref() {
                Ok("guild") => app.select(Some("100".into()), Some("10011".into())),
                Ok("dm") => app.select(None, Some("900".into())),
                _ => {}
            }
        } else if let Some(t) = app.load_token() {
            let snapshot = cache::load(app.profile.as_deref(), &t);
            std::thread::spawn(cache::prune_images);
            app.start(t);
            if let Some(s) = snapshot {
                app.restore_snapshot(s);
            }
        }
        app
    }

    /// 前回の控えを画面に出す。ログインは裏で続き、届いたものから入れ替わる。
    /// 開いていたチャンネルとサーバーは、今すぐ読み直しを頼んでおく（ログインが終わりしだい送られる）
    pub fn restore_snapshot(&mut self, s: cache::Snapshot) {
        self.me = s.me;
        self.guilds = s.guilds;
        self.apply_guild_order();
        self.dms = s.dms;
        self.dms_loaded = true;
        for (gid, chans) in s.channels {
            self.stale_guilds.insert(gid.clone());
            self.channels.insert(gid, chans);
        }
        self.roles.extend(s.roles);
        for (cid, list) in s.messages {
            self.stale_channels.insert(cid.clone());
            self.history.insert(cid.clone(), History::default());
            self.loaded_channels.push_back(cid.clone());
            self.messages.insert(cid, list);
        }
        self.bump_msgs();
        let guild_ok = s
            .sel_guild
            .as_ref()
            .is_none_or(|g| self.guilds.iter().any(|x| &x.id == g));
        if guild_ok {
            let channel = s.sel_channel.filter(|c| self.messages.contains_key(c));
            self.select(s.sel_guild, channel);
        }
    }

    /// 次の起動ですぐ出すための控えを作る。トークンを保存していないときは作らない
    pub fn snapshot(&self) -> Option<cache::Snapshot> {
        let owner = self.token_owner.as_ref()?;
        // 保存済みのトークンがこのアカウントのものであるときだけ（保存していないなら書かない）
        if self
            .load_token()
            .and_then(|t| cache::token_owner(&t))
            .as_ref()
            != Some(owner)
        {
            return None;
        }
        self.build_snapshot()
    }

    pub fn build_snapshot(&self) -> Option<cache::Snapshot> {
        let owner = self.token_owner.clone()?;
        let me = self.me.clone().filter(|m| m.id == owner)?;
        let messages = self
            .loaded_channels
            .iter()
            .filter(|c| !self.no_access.contains(*c) && !self.is_detached(c))
            .filter_map(|c| {
                self.messages
                    .get(c)
                    .filter(|l| !l.is_empty())
                    .map(|l| (c, l))
            })
            .rev()
            .take(cache::CHANNELS)
            .map(|(c, l)| {
                (
                    c.clone(),
                    l[l.len().saturating_sub(cache::MESSAGES_PER_CHANNEL)..].to_vec(),
                )
            })
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        Some(cache::Snapshot {
            version: cache::VERSION,
            owner,
            me: Some(me),
            guilds: self.guilds.clone(),
            dms: self.dms.clone(),
            channels: self
                .channels
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect(),
            roles: self
                .roles
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect(),
            messages,
            sel_guild: self.sel_guild.clone(),
            sel_channel: self.sel_channel.clone(),
        })
    }

    /// 控えを書く。now=false なら CACHE_SAVE_INTERVAL ごとに裏で、now=true（終了時）ならその場で書く
    pub fn save_snapshot(&mut self, now: bool) {
        if !now {
            if !self.cache_dirty {
                return;
            }
            if let Some(wait) = self
                .cache_saved
                .and_then(|(t, _)| CACHE_SAVE_INTERVAL.checked_sub(t.elapsed()))
            {
                // 描き直しがなくても、間隔が空いたら書けるように起こしてもらう
                self.repaint_after(wait);
                return;
            }
        }
        self.cache_dirty = false;
        let Some(s) = self.snapshot() else { return };
        let bytes = match serde_json::to_vec(&s) {
            Ok(b) => b,
            Err(e) => return eprintln!("起動用の控えを作れませんでした: {e}"),
        };
        let mark = {
            use std::hash::{Hash, Hasher};
            let mut h = std::collections::hash_map::DefaultHasher::new();
            bytes.hash(&mut h);
            h.finish()
        };
        let changed = self.cache_saved.is_none_or(|(_, m)| m != mark);
        self.cache_saved = Some((std::time::Instant::now(), mark));
        if !changed {
            return;
        }
        let profile = self.profile.clone();
        let generation = cache::generation(profile.as_deref());
        let write = move || {
            if let Err(e) = cache::write_if_current(profile.as_deref(), &bytes, generation) {
                eprintln!("起動用の控えを保存できませんでした: {e}");
            }
        };
        if now {
            write()
        } else {
            std::thread::spawn(write);
        }
    }

    /// 控えを出したまま接続できなかったあとのつなぎ直し。届かなかった読み込みを頼み直す
    fn retry_login(&mut self, token: String) {
        self.pending_message_loads.clear();
        self.start(token);
        if let Some(c) = self.sel_channel.clone() {
            if self.stale_channels.contains(&c) || self.message_errors.contains(&c) {
                self.load_messages(c);
            }
        }
        if let Some(g) = self.sel_guild.clone() {
            self.ensure_guild_channels(&g);
        }
    }

    /// サーバーのチャンネル一覧・ロールがまだない（または控えのまま）なら取りに行く
    pub fn ensure_guild_channels(&mut self, gid: &str) {
        if !self.channels.contains_key(gid)
            || !self.roles.contains_key(gid)
            || self.stale_guilds.contains(gid)
        {
            self.send(Cmd::LoadGuildChannels(gid.to_string()));
        }
    }

    pub fn reset(&mut self) {
        // 会話に紐づく画像と、読み込み中の要求を破棄する。
        for uri in self.channel_images.values().flatten() {
            images::forget(uri);
        }
        let waker = self.waker.clone();
        let screen_w = self.screen_w;
        let settings = std::mem::take(&mut self.settings);
        let (applied, watch) = (self.theme_applied.take(), self.theme_watch.take());
        let plugins = std::mem::take(&mut self.plugins);
        let message_log = std::mem::take(&mut self.message_log);
        let profile = self.profile.take();
        *self = App {
            waker,
            screen_w,
            remember: true,
            settings,
            show_members: true,
            theme_applied: applied,
            theme_watch: watch,
            plugins,
            message_log,
            profile,
            ..Default::default()
        };
    }

    /// プロファイルを決め、それに合わせて記録・プラグインのデータの置き場所を変える
    pub fn set_profile(&mut self, profile: Option<String>) {
        self.message_log.path = crate::message_log::path_for(profile.as_deref());
        self.plugins.data_root =
            crate::profile::account_dir(profile.as_deref()).join("plugin-data");
        self.profile = profile;
    }

    pub fn token_path(&self) -> PathBuf {
        token_path(self.profile.as_deref())
    }

    pub fn load_token(&self) -> Option<String> {
        load_token(self.profile.as_deref())
    }

    pub fn save_settings(&mut self) {
        if let Err(e) = save_settings(self.profile.as_deref(), &self.settings) {
            eprintln!("設定を保存できませんでした: {e}");
            self.show_toast(&format!("設定を保存できませんでした: {e}"));
        }
    }

    /// d 後に描き直す（期限のあるお知らせを消すときなど）
    pub fn repaint_after(&self, d: std::time::Duration) {
        self.waker.wake_after(d);
    }

    fn connect(&mut self, token: String) {
        self.voice.set_ctx(self.waker.clone());
        self.apply_voice_settings();
        let (cmd, ev) = net::spawn(
            token,
            net::MyPresence {
                status: self.settings.status.clone(),
                activity: self.settings.show_activity,
            },
            self.waker.clone(),
            self.voice.clone(),
        );
        self.backend = Some(Backend { cmd, ev });
        // ログイン中は「接続中」を出さない（ホーム画面をそのまま使ってもらう）。切断・エラーはゲートウェイから届く
        self.status.clear();
        self.gateway_status.clear();
    }

    /// ホーム画面を出すか（ログイン済み、またはつなぎ始めている）。つないでいる間も「ログイン中」画面は出さない
    pub fn in_home(&self) -> bool {
        self.me.is_some() || self.backend.is_some()
    }

    /// トークンでつなぐ（控えのハッシュ用に覚えておく）
    pub fn start(&mut self, token: String) {
        self.token_owner = cache::token_owner(&token);
        self.connect(token);
    }

    pub fn send(&self, c: Cmd) {
        if let Some(b) = &self.backend {
            let _ = b.cmd.send(c);
        }
    }

    fn set_guild_emojis(&mut self, gid: String, list: serde_json::Value) {
        if let Ok(list) = serde_json::from_value(list) {
            self.guild_emojis.insert(gid, list);
            self.emoji_rev += 1;
        }
    }

    /// 開いているサーバーの絵文字をまだ持っていなければ取りに行く（READY に入っていなかったとき）
    pub fn ensure_guild_emojis(&mut self) {
        let Some(gid) = self.sel_guild.clone() else {
            return;
        };
        if !self.guild_emojis.contains_key(&gid) && self.emoji_requested.insert(gid.clone()) {
            self.api(net::ApiCall::GetEmojis(gid));
        }
    }

    /// Nitro があれば、ほかのサーバーの絵文字とアニメ絵文字も使える
    pub fn has_nitro(&self) -> bool {
        self.me
            .as_ref()
            .and_then(|u| u.premium_type)
            .is_some_and(|t| t > 0)
    }

    /// いま送れるカスタム絵文字を、サーバーごとに（開いているサーバーが先頭、あとはサーバーの並び順）
    pub fn usable_emojis(&self) -> Vec<(&str, Vec<&crate::model::Emoji>)> {
        let nitro = self.has_nitro();
        let current = self.sel_guild.as_deref();
        let mut out = Vec::new();
        let ids = current.into_iter().chain(
            self.guilds
                .iter()
                .map(|g| g.id.as_str())
                .filter(|g| nitro && Some(*g) != current),
        );
        for gid in ids {
            let list: Vec<_> = self
                .guild_emojis
                .get(gid)
                .into_iter()
                .flatten()
                .filter(|e| e.available && (nitro || !e.animated))
                .collect();
            if !list.is_empty() {
                out.push((gid, list));
            }
        }
        out
    }

    /// 名前で絵文字を探す（補完とピッカーの検索）
    pub fn emoji_search(&self, query: &str, limit: usize) -> Vec<crate::emoji::Pick> {
        let usable = self.usable_emojis();
        crate::emoji::search(
            query,
            usable
                .iter()
                .flat_map(|(_, l)| l)
                .map(|e| (e.name.as_str(), e.id.as_str(), e.animated)),
            limit,
        )
    }

    /// 本文の :名前: を送れる形にする。known = 編集中のメッセージに元からあった絵文字（使えないサーバーのものでも保つ）
    pub fn expand_emoji_names(&self, content: &str, known: &[(String, String, bool)]) -> String {
        if !content.contains(':') {
            return content.to_string();
        }
        let usable = self.usable_emojis();
        crate::emoji::expand(content, |name| {
            if let Some((n, id, animated)) = known.iter().find(|k| k.0 == name) {
                return Some(crate::emoji::markup(n, id, *animated));
            }
            if let Some(e) = usable.iter().flat_map(|(_, l)| l).find(|e| e.name == name) {
                return Some(crate::emoji::markup(&e.name, &e.id, e.animated));
            }
            emojis::get_by_shortcode(name).map(|e| e.as_str().to_string())
        })
    }

    pub fn submit_compose(&mut self, channel_id: String, content: String) {
        if self.compose_submissions.contains_key(&channel_id) {
            return;
        }
        // 途中だけを出していたら、送ったものが見えるよう最新に戻す
        self.back_to_latest(&channel_id);
        if self.editing.is_none()
            && self.uploads.is_empty()
            && self.plugins.command(&content, &channel_id)
        {
            self.waker.wake();
            return;
        }
        let known = self
            .editing
            .as_ref()
            .map(|m| crate::emoji::shorten(&m.content).1)
            .unwrap_or_default();
        let mut content = self.expand_emoji_names(&content, &known);
        if self.editing.is_none() && self.plugins.translator_enabled() {
            let auto_target = match self.plugins.translation_outgoing() {
                Ok(target) => target,
                Err(error) => {
                    let reply = self.reply_to.take();
                    let uploads = std::mem::take(&mut self.uploads);
                    self.compose_submissions.insert(
                        channel_id,
                        ComposeSubmission {
                            content,
                            reply,
                            editing: None,
                            uploads,
                            failed: true,
                            rejected: Some(error),
                        },
                    );
                    return;
                }
            };
            if auto_target.is_some() && content.starts_with("!tr ") {
                content = content[4..].to_owned();
            } else {
                let request = crate::translator::prefix(&content)
                    .map(|(language, body)| (language, body.to_owned()))
                    .or_else(|| {
                        auto_target
                            .filter(|_| !content.trim().is_empty())
                            .map(|language| (language, content.clone()))
                    });
                if let Some((language, body)) = request {
                    self.translator.next_request_id =
                        self.translator.next_request_id.wrapping_add(1);
                    let request_id = self.translator.next_request_id;
                    let target = crate::translator::Target::Outgoing {
                        channel: channel_id.clone(),
                        request_id,
                        generation: self.plugins.generation,
                    };
                    let result =
                        self.translator
                            .request(target, body, language, self.waker.clone());
                    if result.is_ok() {
                        self.translator
                            .outgoing
                            .insert(channel_id.clone(), request_id);
                    }
                    let reply = self.reply_to.take();
                    let uploads = std::mem::take(&mut self.uploads);
                    let (failed, rejected) = match result {
                        Ok(()) => (false, None),
                        Err(e) => (true, Some(e)),
                    };
                    self.compose_submissions.insert(
                        channel_id,
                        ComposeSubmission {
                            content,
                            reply,
                            editing: None,
                            uploads,
                            failed,
                            rejected,
                        },
                    );
                    return;
                }
            }
        }
        if let Some(m) = self.editing.as_ref() {
            if content.trim().is_empty() {
                self.pending_delete = self.editing.take();
                return;
            }
            if content == m.content {
                self.editing = None;
                return;
            }
        }
        let content = if self.editing.is_none() {
            self.plugins.transform("on_send", &content, &channel_id)
        } else {
            content
        };
        if message_text_len(&content)
            > message_text_limit(self.me.as_ref().and_then(|u| u.premium_type))
        {
            self.input = content;
            self.compose_notice = Some((
                "プラグインによる変換後の本文が文字数制限を超えました".into(),
                std::time::Instant::now(),
            ));
            self.repaint_after(COMPOSE_NOTICE_TTL);
            return;
        }
        let editing = self.editing.take();
        let reply = self.reply_to.take();
        let uploads = if editing.is_none() {
            std::mem::take(&mut self.uploads)
        } else {
            Vec::new()
        };
        let cmd = if let Some(m) = &editing {
            Cmd::Edit {
                channel_id: channel_id.clone(),
                id: m.id.clone(),
                content: content.clone(),
            }
        } else {
            Cmd::Send {
                channel_id: channel_id.clone(),
                content: content.clone(),
                reply_to: reply.as_ref().map(|m| m.id.clone()),
                files: uploads
                    .iter()
                    .map(|u| (u.name.clone(), u.data.clone()))
                    .collect(),
            }
        };
        let failed = self
            .backend
            .as_ref()
            .is_none_or(|b| b.cmd.send(cmd).is_err());
        self.compose_submissions.insert(
            channel_id,
            ComposeSubmission {
                content,
                reply,
                editing,
                uploads,
                failed,
                rejected: None,
            },
        );
    }

    pub fn restore_compose(&mut self, channel_id: &str) -> bool {
        if !self.input.is_empty()
            || !self.uploads.is_empty()
            || self.editing.is_some()
            || self.reply_to.is_some()
        {
            return false;
        }
        if !self
            .compose_submissions
            .get(channel_id)
            .is_some_and(|s| s.failed)
        {
            return false;
        }
        let submission = self.compose_submissions.remove(channel_id).unwrap();
        self.input = submission.content;
        self.reply_to = submission.reply;
        self.editing = submission.editing;
        self.uploads = submission.uploads;
        self.upload_channel = Some(channel_id.to_string());
        true
    }

    pub fn logout(&mut self) {
        self.send(Cmd::LeaveVoice);
        let _ = std::fs::remove_file(self.token_path());
        cache::remove(self.profile.as_deref());
        self.reset();
    }

    pub fn poll(&mut self) {
        for item in self.translator.drain() {
            match item.target {
                crate::translator::Target::Message {
                    channel,
                    id,
                    original,
                    generation,
                } => {
                    if self.plugins.generation == generation
                        && self.plugins.translator_enabled()
                        && self.messages.get(&channel).is_some_and(|items| {
                            items.iter().any(|m| m.id == id && m.content == original)
                        })
                    {
                        match item.result {
                            Ok(result) => {
                                if self.translator.messages.len() >= 512 {
                                    self.translator.messages.clear();
                                }
                                self.translator
                                    .messages
                                    .insert((channel, id), (original, result));
                                self.plugins.revision = self.plugins.revision.wrapping_add(1);
                            }
                            Err(error) => self.show_toast(&error),
                        }
                    }
                }
                crate::translator::Target::Outgoing {
                    channel,
                    request_id,
                    generation,
                } => {
                    if self.translator.outgoing.get(&channel) != Some(&request_id) {
                        continue;
                    }
                    self.translator.outgoing.remove(&channel);
                    if let Some(sub) = self
                        .compose_submissions
                        .get_mut(&channel)
                        .filter(|s| !s.failed)
                    {
                        let result = if self.plugins.translator_enabled()
                            && self.plugins.generation == generation
                        {
                            item.result
                        } else {
                            Err("翻訳中にプラグインが無効になりました".into())
                        };
                        match result {
                            Ok(text)
                                if !text.trim().is_empty()
                                    && message_text_len(&text)
                                        <= message_text_limit(
                                            self.me.as_ref().and_then(|u| u.premium_type),
                                        ) =>
                            {
                                let cmd = Cmd::Send {
                                    channel_id: channel.clone(),
                                    content: text.clone(),
                                    reply_to: sub.reply.as_ref().map(|m| m.id.clone()),
                                    files: sub
                                        .uploads
                                        .iter()
                                        .map(|u| (u.name.clone(), u.data.clone()))
                                        .collect(),
                                };
                                sub.content = text;
                                if self
                                    .backend
                                    .as_ref()
                                    .is_none_or(|b| b.cmd.send(cmd).is_err())
                                {
                                    sub.failed = true;
                                    sub.rejected = Some("送信できませんでした".into());
                                }
                            }
                            Ok(_) => {
                                sub.failed = true;
                                sub.rejected =
                                    Some("翻訳結果が空か文字数制限を超えています".into());
                            }
                            Err(error) => {
                                sub.failed = true;
                                sub.rejected = Some(error);
                            }
                        }
                    }
                }
            }
        }
        if let Some(delay) = self.message_log.save_if_due() {
            self.repaint_after(delay);
        }
        let (actions, next) = self.plugins.tick();
        for action in actions {
            match action {
                crate::plugins::Action::Send(channel_id, content) => self.send(Cmd::Send {
                    channel_id,
                    content,
                    reply_to: None,
                    files: Vec::new(),
                }),
            }
        }
        if let Some(delay) = next {
            self.repaint_after(delay);
        }
        if self
            .login_retry_at
            .is_some_and(|t| t <= std::time::Instant::now())
        {
            self.login_retry_at = None;
            if let Some(t) = self.load_token() {
                self.retry_login(t);
            }
        }
        if let Some(cache) = &self.member_index {
            let elapsed = cache.updated.elapsed();
            if elapsed >= MEMBER_INDEX_TTL {
                self.member_index = None;
            } else {
                self.repaint_after(MEMBER_INDEX_TTL - elapsed);
            }
        }
        let Some(b) = &self.backend else { return };
        let events: Vec<Event> = b.ev.try_iter().collect();
        if !events.is_empty() {
            self.cache_dirty = true;
        }
        let mut voice_index_dirty = false;
        let mut sticker_cache_dirty = false;
        let mut member_indexes: HashMap<String, HashMap<String, usize>> = HashMap::new();
        let mut last_member_guild = None;
        if events
            .iter()
            .any(|event| matches!(event, Event::GuildMembers(..)))
        {
            if let Some(cache) = self.member_index.take() {
                member_indexes.insert(cache.guild_id, cache.index);
            }
        }
        for e in events {
            if matches!(
                e,
                Event::ComposeResult { .. }
                    | Event::Messages(..)
                    | Event::NoAccess(..)
                    | Event::OlderMessages { .. }
                    | Event::MessageCreate(..)
                    | Event::MessageUpdate { .. }
                    | Event::MessageDelete { .. }
            ) {
                self.bump_msgs();
            }
            match e {
                Event::Api { id, call, result } => self.on_api_result(id, call, result),
                Event::Stream(e) => self.on_stream_event(e),
                Event::Gateway { t, d } => self.on_gateway_event(&t, d),
                Event::GuildEmojis(list) => {
                    self.guild_emojis.extend(list);
                    self.emoji_rev += 1;
                }
                Event::ReadyExtra(d) => {
                    if let Some(sid) = d["session_id"].as_str() {
                        self.session_id = Some(sid.to_string());
                    }
                    if let Some(map) = d["threads"].as_object() {
                        for (gid, list) in map {
                            self.load_guild_threads(gid, list);
                        }
                    }
                    if let Some(map) = d["my_roles"].as_object() {
                        for (gid, roles) in map {
                            let roles = roles
                                .as_array()
                                .into_iter()
                                .flatten()
                                .filter_map(|r| r.as_str().map(String::from))
                                .collect();
                            self.my_roles.insert(gid.clone(), roles);
                        }
                    }
                    let rs = &d["read_state"];
                    if let Some(entries) = rs["entries"].as_array().or(rs.as_array()) {
                        let cg = &d["channel_guild"];
                        self.mention_counts.clear();
                        for e in entries {
                            // read_state_type が 0（チャンネル）以外は通知の既読などなので数えない
                            if e["read_state_type"].as_u64().unwrap_or(0) != 0 {
                                continue;
                            }
                            let (Some(cid), Some(n)) =
                                (e["id"].as_str(), e["mention_count"].as_u64())
                            else {
                                continue;
                            };
                            if n > 0 {
                                let gid = cg[cid].as_str().map(String::from);
                                self.mention_counts.insert(cid.to_string(), (gid, n as u32));
                            }
                        }
                    }
                    let ugs = &d["user_guild_settings"];
                    let entries = ugs["entries"].as_array().or(ugs.as_array());
                    for e in entries.into_iter().flatten() {
                        if let Some((gid, s)) = GuildSettings::parse(e) {
                            self.guild_settings.insert(gid, s);
                        }
                    }
                }
                Event::AckFailed {
                    channel_id,
                    message_id,
                } => {
                    // より新しい ACK を既に出していれば古い失敗は無視する。
                    if self.acked.get(&channel_id) == Some(&message_id) {
                        self.acked.remove(&channel_id);
                        self.ack_retry_at
                            .insert(channel_id, std::time::Instant::now() + ACK_RETRY_DELAY);
                        self.repaint_after(ACK_RETRY_DELAY);
                    }
                }
                Event::Me(u) => {
                    if self.remember && !self.token_input.is_empty() {
                        if let Err(e) = save_token(self.profile.as_deref(), self.token_input.trim())
                        {
                            eprintln!("トークン保存失敗: {e}");
                            self.status = "エラー: ログイン情報をこのPCに保存できませんでした。次回はもう一度ログインしてください。".into();
                        }
                    }
                    self.token_input.clear();
                    // 別アカウントの一覧に名前を出すため、プロファイルのフォルダに覚えておく
                    crate::profile::remember_account(self.profile.as_deref(), u.display());
                    self.me = Some(u);
                    self.status = self.gateway_status.clone();
                }
                Event::AuthFailed => {
                    cache::remove(self.profile.as_deref());
                    self.reset();
                    self.login_error =
                        Some("ログイン情報が無効です。もう一度ログインしてください。".into());
                    return;
                }
                Event::LoginFailed if self.me.is_some() => {
                    // 前回の控えを出している：画面はそのままにして、少し待ってつなぎ直す
                    self.backend = None;
                    for s in self.compose_submissions.values_mut() {
                        s.failed = true;
                    }
                    self.status = "接続できませんでした。もう一度つないでいます…".into();
                    self.gateway_status = self.status.clone();
                    self.login_retry_at = Some(std::time::Instant::now() + LOGIN_RETRY_DELAY);
                    self.repaint_after(LOGIN_RETRY_DELAY);
                    return;
                }
                Event::LoginFailed => {
                    self.backend = None;
                    self.login_error = Some(
                        "接続できませんでした。通信状態を確認して、もう一度お試しください。".into(),
                    );
                }
                Event::MessagesFailed(cid) => {
                    self.pending_message_loads.remove(&cid);
                    if self.loaded_channels.iter().any(|id| id == &cid) {
                        if self.messages.contains_key(&cid) {
                            self.status = MESSAGE_RELOAD_ERROR.into();
                            self.message_status_channel = Some(cid.clone());
                        }
                        self.message_errors.insert(cid);
                    }
                }
                Event::ProfileSaved(r) => {
                    self.profile_saving = false;
                    match r {
                        Ok(()) => {
                            // 次の描画で、保存後のプロフィールを読み直して編集を始め直す
                            self.profile_draft = None;
                            self.profile_error = None;
                            self.profile_saved_at = Some(std::time::Instant::now());
                            if let Some(me) = &self.me {
                                let prefix = format!("{}/", me.id);
                                self.profiles.retain(|k, _| !k.starts_with(&prefix));
                            }
                        }
                        Err(e) => self.profile_error = Some(e),
                    }
                }
                Event::ComposeRejected { channel_id, reason } => {
                    if let Some(submission) = self.compose_submissions.get_mut(&channel_id) {
                        submission.rejected = Some(reason);
                    }
                }
                Event::ComposeResult { channel_id, result } => {
                    if let Err(()) = result {
                        if let Some(submission) = self.compose_submissions.get_mut(&channel_id) {
                            submission.failed = true;
                        }
                    } else if let Some(submission) = self.compose_submissions.remove(&channel_id) {
                        if let Ok(Some(message)) = result {
                            if let Some(changes) = self.pending_message_loads.get_mut(&channel_id) {
                                if let Some(original) = &submission.editing {
                                    let already_changed =
                                        changes.iter().any(|change| match change {
                                            MessageChange::Update { id, .. }
                                            | MessageChange::Delete(id) => id == &original.id,
                                            _ => false,
                                        });
                                    if !already_changed && message.id == original.id {
                                        changes.push(MessageChange::Update {
                                            id: message.id.clone(),
                                            content: Some(message.content.clone()),
                                            embeds: None,
                                            pinned: None,
                                        });
                                    }
                                } else {
                                    changes.push(MessageChange::Create(message.clone()));
                                }
                            }
                            if let Some(list) = self.messages.get_mut(&channel_id) {
                                if let Some(original) = submission.editing {
                                    if let Some(current) =
                                        list.iter_mut().find(|m| m.id == original.id)
                                    {
                                        // ゲートウェイから新しい更新が先に届いていれば上書きしない。
                                        if current.content == original.content
                                            && message.id == original.id
                                        {
                                            current.content = message.content;
                                            current.edited_timestamp = message.edited_timestamp;
                                        }
                                    }
                                } else if !list.iter().any(|m| m.id == message.id) {
                                    // REST の応答が後続のゲートウェイ通知より遅れても時系列を保つ。
                                    let at = list.partition_point(|m| {
                                        (m.id.len(), m.id.as_str())
                                            < (message.id.len(), message.id.as_str())
                                    });
                                    list.insert(at, message);
                                    let cap = if self.sel_channel.as_deref() == Some(&channel_id) {
                                        MAX_HISTORY
                                    } else {
                                        MAX_MSGS
                                    };
                                    if list.len() > cap {
                                        let cut = list.len() - cap;
                                        if let Some(h) = self.history.get_mut(&channel_id) {
                                            h.end = false;
                                        }
                                        sticker_cache_dirty |=
                                            list[..cut].iter().any(|m| !m.sticker_items.is_empty());
                                        list.drain(..cut);
                                    }
                                }
                            }
                        } else {
                            self.status = "処理は完了しました。表示が更新されない場合は再読み込みしてください。".into();
                        }
                    }
                }
                Event::Guilds(g) => {
                    self.guilds = g;
                    self.apply_guild_order();
                }
                Event::Dms(mut d) => {
                    d.sort_by_cached_key(|c| std::cmp::Reverse(c.last_id()));
                    self.dms = d;
                    self.dms_loaded = true;
                }
                Event::GuildChannels(gid, mut c) => {
                    // 表示順（テキスト→ボイス、position順）に一度だけ並べておく
                    c.sort_by_key(|c| (c.is_voice(), c.position));
                    self.stale_guilds.remove(&gid);
                    self.channels.insert(gid, c);
                }
                Event::GuildRoles(gid, roles) => {
                    self.roles.insert(gid, roles);
                    self.member_ver += 1;
                }
                Event::MemberList {
                    guild_id,
                    list_id,
                    groups,
                    ops,
                } => {
                    if let Some(list) = self.member_lists.get_mut(&guild_id) {
                        if list.apply(&list_id, groups, ops) {
                            self.member_ver += 1;
                        }
                    }
                }
                Event::GatewayReady => {
                    // 一からつながったときは購読が消えているので、次の描画で送り直す
                    self.member_sub = None;
                }
                Event::GuildMembers(gid, members) => {
                    last_member_guild = Some(gid.clone());
                    let list = self.members.entry(gid.clone()).or_default();
                    // 同じ描画前に複数の塊が届いた場合、索引を共有する
                    let index = member_indexes.entry(gid).or_insert_with(|| {
                        list.iter()
                            .enumerate()
                            .filter_map(|(i, m)| m.user.as_ref().map(|u| (u.id.clone(), i)))
                            .collect()
                    });
                    for member in members {
                        let Some(id) = member.user.as_ref().map(|u| u.id.clone()) else {
                            continue;
                        };
                        match index.get(&id) {
                            Some(&i) => list[i] = member,
                            None => {
                                index.insert(id, list.len());
                                list.push(member);
                            }
                        }
                    }
                    self.member_ver += 1;
                }
                Event::Messages(cid, mut m) => {
                    let changes = self.pending_message_loads.remove(&cid).unwrap_or_default();
                    // 応答待ちの間に破棄されたチャンネルは復活させない
                    if !self.loaded_channels.iter().any(|id| id == &cid) {
                        continue;
                    }
                    let end = m.len() < net::HISTORY_PAGE;
                    for change in changes {
                        change.apply(&mut m);
                    }
                    let cap = if self.sel_channel.as_deref() == Some(&cid) {
                        MAX_HISTORY
                    } else {
                        MAX_MSGS
                    };
                    let trimmed = m.len() > cap;
                    if trimmed {
                        m.drain(..m.len() - cap);
                    }
                    if self.plugins.logger_enabled() {
                        for x in &m {
                            self.message_log.remember(x);
                        }
                        self.message_log.restore(&cid, &mut m, end && !trimmed);
                    }
                    self.history.insert(
                        cid.clone(),
                        History {
                            end: end && !trimmed,
                            ..Default::default()
                        },
                    );
                    self.stale_channels.remove(&cid);
                    self.no_access.remove(&cid);
                    self.clear_message_error_status(&cid);
                    self.message_errors.remove(&cid);
                    let jumping = self.jump.as_ref().is_some_and(|j| j.channel_id == cid);
                    if let Some(old) = self.messages.insert(cid, m) {
                        sticker_cache_dirty |= old.iter().any(|m| !m.sticker_items.is_empty());
                    }
                    if jumping {
                        self.resolve_jump();
                    }
                }
                Event::NoAccess(cid) => {
                    self.pending_message_loads.remove(&cid);
                    if !self.loaded_channels.iter().any(|id| id == &cid) {
                        continue;
                    }
                    self.history.insert(
                        cid.clone(),
                        History {
                            end: true,
                            ..Default::default()
                        },
                    );
                    self.clear_message_error_status(&cid);
                    self.message_errors.remove(&cid);
                    self.no_access.insert(cid.clone());
                    self.stale_channels.remove(&cid);
                    self.messages.insert(cid, Vec::new());
                }
                Event::OlderMessages {
                    channel_id,
                    before,
                    messages,
                } => {
                    let Some(h) = self.history.get_mut(&channel_id) else {
                        continue;
                    };
                    // 一覧を読み直した後などに届いた、今は待っていない応答は捨てる
                    if h.pending.as_deref() != Some(before.as_str()) {
                        continue;
                    }
                    h.pending = None;
                    let Some(older) = messages else {
                        h.retry_at =
                            Some(std::time::Instant::now() + std::time::Duration::from_secs(5));
                        continue;
                    };
                    h.end = older.len() < net::HISTORY_PAGE;
                    // 待っている間に一覧が入れ替わっていたら捨てる（次のスクロールで取り直す）
                    let Some(list) = self.messages.get_mut(&channel_id) else {
                        continue;
                    };
                    if list.first().map(|m| m.id.as_str()) != Some(before.as_str()) {
                        h.end = false;
                        continue;
                    }
                    let have: HashSet<&str> = list.iter().map(|m| m.id.as_str()).collect();
                    let older: Vec<Message> = older
                        .into_iter()
                        .filter(|m| !have.contains(m.id.as_str()))
                        .collect();
                    list.splice(0..0, older);
                    if self.plugins.logger_enabled() {
                        for x in list.iter().take(net::HISTORY_PAGE) {
                            self.message_log.remember(x);
                        }
                        self.message_log.restore(&channel_id, list, h.end);
                    }
                }
                Event::MessageCreate(m) => {
                    self.plugins.event(&m);
                    if self.plugins.logger_enabled() {
                        self.message_log.remember(&m);
                    }
                    self.waker.wake();
                    if let Some(changes) = self.pending_message_loads.get_mut(&m.channel_id) {
                        changes.push(MessageChange::Create(m.clone()));
                    }
                    let mine = self.me.as_ref().is_some_and(|u| u.id == m.author.id);
                    if !mine && self.sel_channel.as_deref() != Some(&m.channel_id) {
                        self.unread.insert(m.channel_id.clone(), m.guild_id.clone());
                    }
                    if !mine {
                        self.on_new_message(&m);
                    }
                    // 新着の DM は必ず最新なので、全体を並べ直さず先頭へ移すだけでよい
                    if let Some(i) = self.dms.iter().position(|c| c.id == m.channel_id) {
                        let mut dm = self.dms.remove(i);
                        dm.last_message_id = Some(m.id.clone());
                        self.dms.insert(0, dm);
                    }
                    // 途中だけを出している一覧には足さない（間が抜けるため。最新に戻れば読み直す）
                    let detached = self.is_detached(&m.channel_id);
                    if let Some(list) = self.messages.get_mut(&m.channel_id).filter(|_| !detached) {
                        // 重複は自分の送信の折り返しなので末尾側から探す
                        if !list.iter().rev().any(|x| x.id == m.id) {
                            let viewing =
                                self.sel_channel.as_deref() == Some(m.channel_id.as_str());
                            list.push(m);
                            // 表示中のチャンネルは遡って読んだ分を消さない（読んでいる行が消えるため）
                            let cap = if viewing { MAX_HISTORY } else { MAX_MSGS };
                            if list.len() > cap {
                                let cut = list.len() - cap;
                                if let Some(h) = self.history.get_mut(&list[0].channel_id) {
                                    h.end = false;
                                }
                                sticker_cache_dirty |=
                                    list[..cut].iter().any(|m| !m.sticker_items.is_empty());
                                list.drain(..cut);
                            }
                        }
                    }
                }
                Event::MessageUpdate {
                    channel_id,
                    id,
                    content,
                    embeds,
                    pinned,
                } => {
                    if let Some(changes) = self.pending_message_loads.get_mut(&channel_id) {
                        changes.push(MessageChange::Update {
                            id: id.clone(),
                            content: content.clone(),
                            embeds: embeds.clone(),
                            pinned,
                        });
                    }
                    if let Some(p) = pinned {
                        self.on_pin_changed(&channel_id, &id, p);
                    }
                    let logged = content
                        .as_deref()
                        .is_some_and(|c| self.on_message_edit(&channel_id, &id, c));
                    if let Some(m) = self
                        .messages
                        .get_mut(&channel_id)
                        .and_then(|l| l.iter_mut().find(|m| m.id == id))
                    {
                        if let Some(c) = content {
                            if c != m.content {
                                m.content = c;
                                m.edited_timestamp = Some(String::new());
                            }
                        }
                        if logged {
                            m.log = self.message_log.mark_for(&id);
                        }
                        if let Some(e) = embeds {
                            m.embeds = e;
                        }
                    }
                }
                Event::MessageDelete { channel_id, id } => {
                    sticker_cache_dirty |= self.on_message_delete(&channel_id, &id);
                }
                Event::Status(s) => {
                    self.gateway_status = s.clone();
                    self.status = s;
                }
                Event::Error(s) => {
                    eprintln!("操作失敗: {s}");
                    let action = if s.starts_with("サーバー一覧:") {
                        "サーバー一覧を取得できませんでした。"
                    } else if s.starts_with("DM一覧:") {
                        "DM一覧を取得できませんでした。"
                    } else if s.starts_with("呼び出し失敗:") {
                        "呼び出しに失敗しました。"
                    } else {
                        "操作を完了できませんでした。"
                    };
                    self.status =
                        format!("エラー: {action}通信状態を確認して、もう一度お試しください。");
                }
                Event::Profile { key, result } => {
                    let st = match result {
                        Ok(d) => ProfileState::Ready(d),
                        Err(e) => {
                            eprintln!("プロフィール取得失敗: {e}");
                            ProfileState::Failed
                        }
                    };
                    self.profiles.insert(key, st);
                }
                Event::VoiceStates(list) => {
                    for v in list {
                        let previous = self
                            .voice_states
                            .get(&v.user_id)
                            .and_then(|old| old.channel_id.as_deref());
                        voice_index_dirty |= previous != v.channel_id.as_deref();
                        if v.channel_id.is_some() {
                            self.voice_states.insert(v.user_id.clone(), v);
                        } else {
                            self.voice_states.remove(&v.user_id);
                        }
                    }
                }
                Event::Voice(st) => {
                    match st {
                        VoiceStatus::Disconnected => self.voice_status = None,
                        VoiceStatus::Error(e) => {
                            self.voice_status = None;
                            eprintln!("ボイス接続失敗: {e}");
                            self.status = "エラー: ボイスに接続できませんでした。通信状態と音声設定を確認してください。".into();
                        }
                        st => {
                            if let (VoiceStatus::Connected { channel_id, .. }, Some(me)) =
                                (&st, &self.me)
                            {
                                if let Some(r) = self.calls.get_mut(channel_id) {
                                    r.retain(|u| u != &me.id);
                                }
                            }
                            self.voice_status = Some(st);
                        }
                    }
                    self.update_voice_call_started();
                }
                Event::Call {
                    channel_id,
                    ringing,
                } => {
                    self.calls.insert(channel_id, ringing);
                }
                Event::CallEnded(cid) => {
                    self.calls.remove(&cid);
                    self.voice_states.retain(|_, v| {
                        v.channel_id.as_deref() != Some(&cid) || v.guild_id.is_some()
                    });
                    voice_index_dirty = true;
                }
                Event::OwnPresence(p) => {
                    if let Some(id) = self.me.as_ref().map(|u| u.id.clone()) {
                        self.presences.insert(id, p);
                        self.member_ver += 1;
                    }
                }
                Event::Presences(list) => {
                    // 並びが変わるのはオンライン/オフラインが入れ替わったときだけ
                    let flips = list.iter().any(|(id, p)| {
                        let was = self
                            .presences
                            .get(id)
                            .is_some_and(|o| o.status != "offline");
                        was != (p.status != "offline")
                    });
                    if flips {
                        self.member_ver += 1;
                    }
                    self.presences.extend(list);
                }
                Event::Sticker(id, r) => {
                    // チャンネル破棄後に到着した結果はテクスチャ化しない
                    if !matches!(self.stickers.get(&id), Some(StickerState::Loading)) {
                        continue;
                    }
                    let state = match r {
                        Ok(f) => {
                            let [w, h] = f.size;
                            let frames: Vec<image::Frame> = f
                                .frames
                                .into_iter()
                                .filter_map(|(rgba, ms)| {
                                    let img = image::RgbaImage::from_raw(w as u32, h as u32, rgba)?;
                                    Some(images::bgra_frame(
                                        img,
                                        std::time::Duration::from_millis(ms.max(10) as u64),
                                    ))
                                })
                                .collect();
                            if frames.is_empty() {
                                StickerState::Failed
                            } else {
                                StickerState::Ready(Arc::new(RenderImage::new(frames)))
                            }
                        }
                        Err(e) => {
                            eprintln!("スタンプ {id} の読み込み失敗: {e}");
                            StickerState::Failed
                        }
                    };
                    self.stickers.insert(id, state);
                }
            }
        }
        if voice_index_dirty {
            self.reindex_voice();
        }
        if let Some(guild_id) = last_member_guild {
            if let Some(index) = member_indexes.remove(&guild_id) {
                self.member_index = Some(RecentMemberIndex {
                    guild_id,
                    index,
                    updated: std::time::Instant::now(),
                });
                self.repaint_after(MEMBER_INDEX_TTL);
            }
        }
        if sticker_cache_dirty {
            self.prune_stickers();
        }
    }

    /// クリップボードの画像を添付に加える。画像がなければ false
    pub fn paste_image(&mut self) -> bool {
        match clip::clipboard_image() {
            Some(Ok(u)) => {
                self.add_uploads(vec![u], 0, Vec::new());
                true
            }
            Some(Err(e)) => {
                self.set_compose_notice(e);
                true
            }
            None => false,
        }
    }

    /// 貼り付けた文字がファイルのパス（ファイルマネージャでコピーしたもの）なら添付に回す。回したら true
    pub fn paste_text(&mut self, text: &str) -> bool {
        match clip::pasted_paths(text) {
            Some(paths) => {
                self.attach_paths(paths);
                true
            }
            None => false,
        }
    }

    /// ファイル選択ダイアログで選んだファイルを、ダイアログを開いたチャンネルに戻ったときだけ添付する
    pub fn take_picked_files(&mut self) {
        if self
            .picked_files
            .as_ref()
            .is_some_and(|(channel_id, _)| self.sel_channel.as_ref() == Some(channel_id))
        {
            let Some((_, picked)) = self.picked_files.take() else {
                return;
            };
            if matches!(
                self.status.as_str(),
                "選択したファイルは元のチャンネルに戻ると添付されます。"
                    | "先に開いたファイル選択を完了し、元のチャンネルで確認してください。"
            ) {
                self.status = self.gateway_status.clone();
            }
            self.attach_paths(picked);
        }
    }

    /// 貼り付け・ドロップ・ファイル選択で来たファイルを添付に加える
    pub fn attach_paths(&mut self, paths: Vec<PathBuf>) {
        // 上限を超える分は読み込まずに数だけ数える（大きいファイルを無駄に読まない）
        let room = MAX_UPLOADS.saturating_sub(self.uploads.len());
        let skipped = paths.len().saturating_sub(room);
        let (hard, _) = self.upload_limits();
        let mut loaded = Vec::new();
        let mut too_big = Vec::new();
        for p in paths.into_iter().take(room) {
            // 上限を超えるファイルは中身を読まない
            if let Some(len) = std::fs::metadata(&p)
                .ok()
                .map(|m| m.len())
                .filter(|&len| len > hard)
            {
                too_big.push((
                    p.file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default(),
                    len,
                ));
                continue;
            }
            match clip::load_file(&p) {
                Ok(u) => loaded.push(u),
                Err(e) => self.set_compose_notice(e),
            }
        }
        if !loaded.is_empty() || skipped > 0 || !too_big.is_empty() {
            self.add_uploads(loaded, skipped, too_big);
        }
    }

    /// 1ファイルの上限（送れないことが確実な大きさ, 超えると送れないかもしれない大きさ）。
    /// サーバーではブーストで上限が上がるが段階は分からないので、アカウントの上限を超えても注意だけにする
    pub fn upload_limits(&self) -> (u64, Option<u64>) {
        let account = account_upload_limit(self.me.as_ref().and_then(|u| u.premium_type));
        if self.sel_guild.is_some() && account < GUILD_MAX_UPLOAD {
            (GUILD_MAX_UPLOAD, Some(account))
        } else {
            (account, None)
        }
    }

    /// 添付に加える。Discord の上限（1メッセージ MAX_UPLOADS 件・1ファイルの大きさ）を超えるものは加えずにお知らせを出す。
    /// skipped = 件数の上限で読み込む前に除いた数、too_big = 大きすぎて読み込む前に除いたファイル（名前, 大きさ）
    pub fn add_uploads(
        &mut self,
        new: Vec<clip::Upload>,
        skipped: usize,
        mut too_big: Vec<(String, u64)>,
    ) {
        let (hard, soft) = self.upload_limits();
        let mut risky = Vec::new();
        let mut over = Vec::new();
        for u in new {
            let len = u.data.len() as u64;
            if len > hard {
                too_big.push((u.name.clone(), len));
            } else if self.uploads.len() >= MAX_UPLOADS {
                over.push(u.name.clone());
            } else {
                if soft.is_some_and(|s| len > s) {
                    risky.push((u.name.clone(), len));
                }
                self.uploads.push(u);
            }
        }
        let mut lines = Vec::new();
        let dropped = over.len() + skipped;
        if dropped > 0 {
            lines.push(match over.first() {
                Some(name) if dropped > 1 => format!("添付できるのは1メッセージ{MAX_UPLOADS}件までです。「{name}」ほか{}件は追加していません。", dropped - 1),
                Some(name) => format!("添付できるのは1メッセージ{MAX_UPLOADS}件までです。「{name}」は追加していません。"),
                None => format!("添付できるのは1メッセージ{MAX_UPLOADS}件までです。{dropped}件のファイルは追加していません。"),
            });
        }
        if let Some((name, len)) = too_big.first() {
            let more = if too_big.len() > 1 {
                format!("ほか{}件", too_big.len() - 1)
            } else {
                String::new()
            };
            lines.push(format!(
                "「{name}」（{}）{more}は大きすぎるため追加していません。1ファイル{}までです。",
                fmt_size(*len),
                fmt_size(hard)
            ));
        }
        if let (Some((name, len)), Some(s)) = (risky.first(), soft) {
            let more = if risky.len() > 1 {
                format!("ほか{}件", risky.len() - 1)
            } else {
                String::new()
            };
            lines.push(format!("「{name}」（{}）{more}はアカウントの上限 {} を超えています。サーバーのブーストが足りないと送信できません。", fmt_size(*len), fmt_size(s)));
        }
        if lines.is_empty() {
            return;
        }
        self.compose_notice = Some((lines.join("\n"), std::time::Instant::now()));
        self.repaint_after(COMPOSE_NOTICE_TTL);
    }

    /// 入力欄の上のお知らせを出す（しばらくすると消える）
    pub fn set_compose_notice(&mut self, text: String) {
        self.compose_notice = Some((text, std::time::Instant::now()));
        self.repaint_after(COMPOSE_NOTICE_TTL);
    }

    /// REST API をどれでも呼ぶ。結果は on_api_result に届く。返り値はこの呼び出しの番号
    pub fn api(&mut self, call: net::ApiCall) -> u64 {
        self.next_api_id += 1;
        let id = self.next_api_id;
        self.send(Cmd::Api { id, call });
        id
    }

    fn on_api_result(
        &mut self,
        id: u64,
        call: net::ApiCall,
        result: Result<serde_json::Value, String>,
    ) {
        use net::ApiCall as A;
        // 結果を自分で扱うもの（失敗の知らせ方もそれぞれ）
        let call = match call {
            A::GetChannelPins {
                channel_id, before, ..
            } => return self.on_pins_loaded(id, channel_id, before, result),
            A::Pin {
                channel_id,
                message_id,
            } => return self.on_pin_result(channel_id, message_id, true, result),
            A::Unpin {
                channel_id,
                message_id,
            } => return self.on_pin_result(channel_id, message_id, false, result),
            A::GetMessages {
                channel_id,
                around: Some(id),
                ..
            } => return self.on_around_loaded(channel_id, id, result),
            A::GetMessages {
                channel_id,
                after: Some(id),
                ..
            } => return self.on_newer_loaded(channel_id, id, result),
            A::SearchGuild { .. } | A::SearchChannel { .. } => {
                return self.on_search_result(id, result)
            }
            A::SearchThreads { .. } => return self.on_threads_loaded(id, result),
            A::CreateThreadFromMessage { .. } => return self.on_thread_created(true, result),
            A::CreateThread { .. } => return self.on_thread_created(false, result),
            A::JoinThread(t) => return self.on_thread_join_result(t, true, result),
            A::GetGuildCommands(g) => return self.on_commands_loaded(format!("g{g}"), result),
            A::GetChannelCommands(c) => return self.on_commands_loaded(format!("c{c}"), result),
            A::GetUserCommands => return self.on_commands_loaded("u".into(), result),
            A::Interaction(body) => return self.on_interaction_sent(body, result),
            A::LeaveThread(t) => return self.on_thread_join_result(t, false, result),
            A::AddReaction { .. } | A::RemoveReaction { .. } => {
                let pending = self.pending_reactions.remove(&id);
                if let (Err(e), Some((cid, mid, op))) = (result, pending) {
                    eprintln!("リアクションできませんでした: {e}");
                    if let Some(undo) = op.undo() {
                        self.apply_reaction_op(&cid, &mid, undo);
                    }
                    self.show_toast("リアクションできませんでした");
                }
                return;
            }
            call => call,
        };
        let v = match result {
            Ok(v) => v,
            Err(e) => {
                let what = match &call {
                    A::CreateInvite { .. } => "招待リンクを作れませんでした",
                    A::LeaveGuild(_) => "サーバーから脱退できませんでした",
                    A::ModifyGuildSettings { .. } | A::ModifyChannelSettings { .. } => {
                        "通知設定を変えられませんでした"
                    }
                    A::AckGuild(_) | A::AckBulk(_) => "既読にできませんでした",
                    _ => "Discord への操作に失敗しました",
                };
                eprintln!("{what}: {call:?}: {e}");
                self.show_toast(what);
                return;
            }
        };
        match call {
            A::CreateInvite { .. } => {
                if let Some(code) = v["code"].as_str() {
                    self.clipboard_out = Some(format!("https://discord.gg/{code}"));
                    self.show_toast("招待リンクをコピーしました");
                }
            }
            A::LeaveGuild(gid) => self.remove_guild(&gid),
            A::GetEmojis(gid) => self.set_guild_emojis(gid, v),
            A::ModifyGuildSettings { .. } | A::ModifyChannelSettings { .. } => {
                if let Some((gid, s)) = GuildSettings::parse(&v) {
                    self.guild_settings.insert(gid, s);
                }
            }
            _ => {}
        }
    }

    /// MESSAGE_REACTION_ADD / _ADD_MANY / _REMOVE / _REMOVE_ALL / _REMOVE_EMOJI
    fn on_reaction_event(&mut self, t: &str, d: &serde_json::Value) {
        let (Some(cid), Some(mid)) = (d["channel_id"].as_str(), d["message_id"].as_str()) else {
            return;
        };
        let my_id = self.me.as_ref().map(|u| u.id.clone());
        let is_me = |v: &serde_json::Value| my_id.is_some() && v.as_str() == my_id.as_deref();
        let emoji =
            |v: &serde_json::Value| serde_json::from_value::<model::ReactionEmoji>(v.clone()).ok();
        let ops: Vec<model::ReactionOp> = match t {
            "MESSAGE_REACTION_ADD" => emoji(&d["emoji"])
                .map(|emoji| model::ReactionOp::Add {
                    emoji,
                    me: is_me(&d["user_id"]),
                })
                .into_iter()
                .collect(),
            "MESSAGE_REACTION_REMOVE" => emoji(&d["emoji"])
                .map(|emoji| model::ReactionOp::Remove {
                    emoji,
                    me: is_me(&d["user_id"]),
                })
                .into_iter()
                .collect(),
            "MESSAGE_REACTION_REMOVE_ALL" => vec![model::ReactionOp::RemoveAll],
            "MESSAGE_REACTION_REMOVE_EMOJI" => emoji(&d["emoji"])
                .map(model::ReactionOp::RemoveEmoji)
                .into_iter()
                .collect(),
            // 大きいサーバーではまとめて届く: reactions: [{ emoji, users: [id…] }]
            "MESSAGE_REACTION_ADD_MANY" => d["reactions"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|r| Some((emoji(&r["emoji"])?, r["users"].as_array()?.clone())))
                .flat_map(|(e, users)| users.into_iter().map(move |u| (e.clone(), u)))
                .map(|(emoji, u)| model::ReactionOp::Add {
                    emoji,
                    me: is_me(&u),
                })
                .collect(),
            _ => return,
        };
        for op in ops {
            self.apply_reaction_op(cid, mid, op);
        }
    }

    /// 表示中の一覧（と読み込み中の差分）にリアクションの増減を当てる
    fn apply_reaction_op(&mut self, channel_id: &str, message_id: &str, op: model::ReactionOp) {
        if let Some(changes) = self.pending_message_loads.get_mut(channel_id) {
            changes.push(MessageChange::Reaction {
                id: message_id.to_string(),
                op: op.clone(),
            });
        }
        if let Some(m) = self
            .messages
            .get_mut(channel_id)
            .and_then(|l| l.iter_mut().find(|m| m.id == message_id))
        {
            if model::apply_reaction(&mut m.reactions, op) {
                self.bump_msgs();
            }
        }
    }

    /// 自分のリアクションを付け外しする。先に表示を変え、失敗したら戻す
    pub fn toggle_reaction(
        &mut self,
        channel_id: &str,
        message_id: &str,
        emoji: &model::ReactionEmoji,
    ) {
        let mine = self
            .messages
            .get(channel_id)
            .and_then(|l| l.iter().find(|m| m.id == message_id))
            .and_then(|m| m.reactions.iter().find(|r| r.emoji.key() == emoji.key()))
            .is_some_and(|r| r.me);
        let (op, call) = if mine {
            (
                model::ReactionOp::Remove {
                    emoji: emoji.clone(),
                    me: true,
                },
                net::ApiCall::RemoveReaction {
                    channel_id: channel_id.into(),
                    message_id: message_id.into(),
                    emoji: emoji.api_str(),
                },
            )
        } else {
            (
                model::ReactionOp::Add {
                    emoji: emoji.clone(),
                    me: true,
                },
                net::ApiCall::AddReaction {
                    channel_id: channel_id.into(),
                    message_id: message_id.into(),
                    emoji: emoji.api_str(),
                },
            )
        };
        self.apply_reaction_op(channel_id, message_id, op.clone());
        let id = self.api(call);
        self.pending_reactions
            .insert(id, (channel_id.into(), message_id.into(), op));
    }

    /// 編集を受け取ったとき（一覧を書き換える前）。MessageLogger が記録したら true
    fn on_message_edit(&mut self, channel_id: &str, id: &str, content: &str) -> bool {
        let old = self
            .messages
            .get(channel_id)
            .and_then(|l| l.iter().find(|m| m.id == id))
            .or_else(|| self.message_log.cached(id))
            .cloned();
        let Some(old) = old.filter(|o| o.content != content) else {
            return false;
        };
        self.plugins.message_hook("on_edit", &old, Some(content));
        let Some(opt) = self.plugins.logger_options() else {
            return false;
        };
        let me = self.me.as_ref().map(|u| u.id.as_str());
        if !crate::message_log::MessageLog::wanted(&old, me, opt) {
            return false;
        }
        self.message_log.on_edit(&old, content);
        true
    }

    /// 削除を受け取ったとき。MessageLogger が記録したら一覧に削除済みとして残し、そうでなければ消す。
    /// 消したメッセージにスタンプがあれば true（スタンプの画像を手放すため）
    fn on_message_delete(&mut self, channel_id: &str, id: &str) -> bool {
        if let Some(changes) = self.pending_message_loads.get_mut(channel_id) {
            changes.push(MessageChange::Delete(id.to_string()));
        }
        let old = self
            .messages
            .get(channel_id)
            .and_then(|l| l.iter().find(|m| m.id == id))
            .or_else(|| self.message_log.cached(id))
            .cloned();
        if old
            .as_ref()
            .and_then(|m| m.log.as_ref())
            .is_some_and(|log| log.deleted_at.is_some())
        {
            return false;
        }
        let mut keep = false;
        if let Some(old) = old.filter(|o| o.log.as_ref().is_none_or(|l| l.deleted_at.is_none())) {
            self.plugins.message_hook("on_delete", &old, None);
            if let Some(opt) = self.plugins.logger_options() {
                let me = self.me.as_ref().map(|u| u.id.as_str());
                if crate::message_log::MessageLog::wanted(&old, me, opt) {
                    self.message_log.on_delete(&old);
                    keep = true;
                }
            }
        }
        let Some(l) = self.messages.get_mut(channel_id) else {
            return false;
        };
        if keep {
            if let Some(m) = l.iter_mut().find(|m| m.id == id) {
                m.log = self.message_log.mark_for(id);
            }
            return false;
        }
        let stickers = l.iter().any(|m| m.id == id && !m.sticker_items.is_empty());
        l.retain(|m| m.id != id);
        stickers
    }

    fn on_gateway_event(&mut self, t: &str, mut d: serde_json::Value) {
        match t {
            "CHANNEL_PINS_UPDATE" => self.on_channel_pins_update(&d),
            t if t.starts_with("MESSAGE_REACTION_") => self.on_reaction_event(t, &d),
            "INTERACTION_CREATE"
            | "INTERACTION_SUCCESS"
            | "INTERACTION_FAILURE"
            | "APPLICATION_COMMAND_AUTOCOMPLETE_RESPONSE" => self.on_interaction_event(t, &d),
            "THREAD_CREATE"
            | "THREAD_UPDATE"
            | "THREAD_DELETE"
            | "THREAD_LIST_SYNC"
            | "THREAD_MEMBER_UPDATE"
            | "THREAD_MEMBERS_UPDATE" => self.on_thread_event(t, &d),
            "MESSAGE_DELETE_BULK" => {
                let Some(cid) = d["channel_id"].as_str().map(String::from) else {
                    return;
                };
                let ids: Vec<String> = d["ids"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect();
                let mut stickers = false;
                for id in ids {
                    stickers |= self.on_message_delete(&cid, &id);
                }
                self.bump_msgs();
                if stickers {
                    self.prune_stickers();
                }
            }
            "GUILD_EMOJIS_UPDATE" => {
                if let Some(gid) = d["guild_id"].as_str().map(String::from) {
                    let list = d
                        .get_mut("emojis")
                        .map(serde_json::Value::take)
                        .unwrap_or_default();
                    self.set_guild_emojis(gid, list);
                }
            }
            "GUILD_CREATE" => {
                if let Some(gid) = d["id"].as_str().map(String::from) {
                    if d["threads"].is_array() {
                        self.load_guild_threads(&gid, &d["threads"]);
                    }
                    if d["emojis"].is_array() {
                        let list = d
                            .get_mut("emojis")
                            .map(serde_json::Value::take)
                            .unwrap_or_default();
                        self.set_guild_emojis(gid, list);
                    }
                }
            }
            "USER_GUILD_SETTINGS_UPDATE" => {
                if let Some((gid, s)) = GuildSettings::parse(&d) {
                    self.guild_settings.insert(gid, s);
                }
            }
            // unavailable のときは障害で一時的に見えないだけなので残す
            "GUILD_DELETE" if d["unavailable"] != true => {
                if let Some(gid) = d["id"].as_str() {
                    self.remove_guild(gid);
                }
            }
            // 公式クライアントなど別の端末で既読にした
            "MESSAGE_ACK" => {
                if let Some(cid) = d["channel_id"].as_str() {
                    if d["manual"] != true {
                        self.unread.remove(cid);
                    }
                    match d["mention_count"].as_u64() {
                        Some(n) if n > 0 => {
                            if let Some(e) = self.mention_counts.get_mut(cid) {
                                e.1 = n as u32;
                            }
                        }
                        _ => {
                            self.mention_counts.remove(cid);
                        }
                    }
                }
            }
            // 自分のロールが変わった
            "GUILD_MEMBER_UPDATE" => {
                let me = self.me.as_ref().map(|u| u.id.as_str());
                if me.is_some() && d["user"]["id"].as_str() == me {
                    if let Some(gid) = d["guild_id"].as_str() {
                        let roles = d["roles"]
                            .as_array()
                            .into_iter()
                            .flatten()
                            .filter_map(|r| r.as_str().map(String::from))
                            .collect();
                        self.my_roles.insert(gid.to_string(), roles);
                    }
                }
            }
            _ => {}
        }
    }

    /// 保存してある並び（settings.guild_order）にサーバーを並べる
    pub fn apply_guild_order(&mut self) {
        let pos: HashMap<&str, usize> = self
            .settings
            .guild_order
            .iter()
            .enumerate()
            .map(|(i, id)| (id.as_str(), i))
            .collect();
        let mut keyed: Vec<(usize, Guild)> = std::mem::take(&mut self.guilds)
            .into_iter()
            .map(|g| (pos.get(g.id.as_str()).copied().unwrap_or(usize::MAX), g))
            .collect();
        keyed.sort_by_key(|(k, _)| *k);
        self.guilds = keyed.into_iter().map(|(_, g)| g).collect();
    }

    /// サーバー from を to の位置へ動かす（下へ動かしたら to の後ろ、上へなら前）。
    /// 並びを settings に写すので、呼んだ側で save_settings する。動いたら true
    pub fn move_guild(&mut self, from: &str, to: &str) -> bool {
        let (Some(fi), Some(ti)) = (
            self.guilds.iter().position(|g| g.id == from),
            self.guilds.iter().position(|g| g.id == to),
        ) else {
            return false;
        };
        if fi == ti {
            return false;
        }
        let g = self.guilds.remove(fi);
        self.guilds.insert(ti, g);
        // 抜けたサーバーの分も含めて覚えておく（一時的に一覧にないだけかもしれないので）
        let shown: std::collections::HashSet<&str> =
            self.guilds.iter().map(|g| g.id.as_str()).collect();
        let mut order: Vec<String> = self.guilds.iter().map(|g| g.id.clone()).collect();
        order.extend(
            self.settings
                .guild_order
                .iter()
                .filter(|id| !shown.contains(id.as_str()))
                .cloned(),
        );
        self.settings.guild_order = order;
        true
    }

    /// 抜けた・消えたサーバーを一覧から外す
    pub fn remove_guild(&mut self, gid: &str) {
        self.guilds.retain(|g| g.id != gid);
        self.channels.remove(gid);
        self.unread.retain(|_, g| g.as_deref() != Some(gid));
        self.mention_counts
            .retain(|_, (g, _)| g.as_deref() != Some(gid));
        self.my_roles.remove(gid);
        self.guild_settings.remove(gid);
        if self.guild_emojis.remove(gid).is_some() {
            self.emoji_rev += 1;
        }
        if self.sel_guild.as_deref() == Some(gid) {
            self.sel_guild = None;
            self.sel_channel = None;
        }
        self.cache_dirty = true;
    }

    /// サーバーのチャンネルをすべて既読にする
    pub fn mark_guild_read(&mut self, gid: &str) {
        self.unread.retain(|_, g| g.as_deref() != Some(gid));
        self.mention_counts
            .retain(|_, (g, _)| g.as_deref() != Some(gid));
        self.api(net::ApiCall::AckGuild(gid.to_string()));
    }

    /// 自分あてのメンションか（@自分・@everyone/@here・自分のロール）
    pub fn mentions_me(&self, m: &Message) -> bool {
        let Some(me) = &self.me else { return false };
        if m.mentions.iter().any(|u| u.id == me.id) || m.mention_everyone {
            return true;
        }
        let gid = m.guild_id.as_deref().unwrap_or("");
        self.my_roles
            .get(gid)
            .is_some_and(|mine| m.mention_roles.iter().any(|r| mine.contains(r)))
    }

    /// サーバー（None なら DM）の未読メンション数の合計
    pub fn mention_count(&self, gid: Option<&str>) -> u32 {
        self.mention_counts
            .values()
            .filter(|(g, _)| g.as_deref() == gid)
            .map(|(_, n)| n)
            .sum()
    }

    /// 未読のある DM と件数（新しい順）。レールに相手のアイコンを並べるのに使う
    pub fn unread_dms(&self) -> Vec<(&Channel, u32)> {
        self.dms
            .iter()
            .filter_map(|c| match self.mention_counts.get(&c.id) {
                Some((None, n)) if *n > 0 => Some((c, *n)),
                _ => None,
            })
            .collect()
    }

    /// 他の人の新着メッセージ: メンション数を数え、必要ならデスクトップ通知を出す
    fn on_new_message(&mut self, m: &Message) {
        let viewing =
            self.window_focused && self.sel_channel.as_deref() == Some(m.channel_id.as_str());
        let mentioned = self.mentions_me(m);
        // DM はすべてがメンション扱い（公式と同じ）
        if !viewing && (mentioned || m.guild_id.is_none()) {
            let e = self
                .mention_counts
                .entry(m.channel_id.clone())
                .or_insert((m.guild_id.clone(), 0));
            e.1 += 1;
        }
        let Some(gid) = m.guild_id.as_deref() else {
            return;
        };
        if viewing {
            return;
        }
        let set = self.guild_settings.get(gid).cloned().unwrap_or_default();
        let wanted = match set.notifications {
            _ if set.muted => false,
            2 => false,
            0 => true,
            // 1 = @メンションのみ、3 = サーバーの既定（多くのサーバーはメンションのみ）
            _ => mentioned,
        };
        if !wanted {
            return;
        }
        let author = m
            .member
            .as_ref()
            .and_then(|x| x.nick.as_deref())
            .unwrap_or_else(|| m.author.display())
            .to_string();
        let guild = self
            .guilds
            .iter()
            .find(|g| g.id == gid)
            .map(|g| g.name.as_str())
            .unwrap_or("");
        let channel = self
            .channels
            .get(gid)
            .and_then(|cs| cs.iter().find(|c| c.id == m.channel_id))
            .and_then(|c| c.name.as_deref())
            .unwrap_or("");
        let title = match (channel.is_empty(), guild.is_empty()) {
            (false, false) => format!("{author}（#{channel}・{guild}）"),
            (true, false) => format!("{author}（{guild}）"),
            _ => author,
        };
        let roles = self.roles.get(gid).map(Vec::as_slice).unwrap_or_default();
        let mut body =
            crate::notify::excerpt(&resolve_mentions(&m.content, m, &self.channels, roles), 200);
        if body.is_empty() {
            body = if m.is_call() {
                "通話を開始しました".into()
            } else if !m.attachments.is_empty() {
                "添付ファイルを送信しました".into()
            } else if !m.sticker_items.is_empty() {
                "スタンプを送信しました".into()
            } else {
                "新しいメッセージ".into()
            };
        }
        crate::notify::show(
            title,
            body,
            (m.guild_id.clone(), m.channel_id.clone()),
            self.notify_clicked.clone(),
            self.waker.clone(),
        );
    }

    /// サーバーの通知設定を変える（先に画面へ反映し、結果で上書きする）
    pub fn set_guild_settings(
        &mut self,
        gid: &str,
        muted: Option<bool>,
        notifications: Option<u8>,
    ) {
        let s = self.guild_settings.entry(gid.to_string()).or_default();
        let mut body = serde_json::Map::new();
        if let Some(m) = muted {
            s.muted = m;
            body.insert("muted".into(), m.into());
            if m {
                body.insert(
                    "mute_config".into(),
                    serde_json::json!({ "selected_time_window": -1, "end_time": null }),
                );
            }
        }
        if let Some(n) = notifications {
            s.notifications = n;
            body.insert("message_notifications".into(), n.into());
        }
        self.api(net::ApiCall::ModifyGuildSettings {
            guild_id: gid.to_string(),
            settings: serde_json::Value::Object(body),
        });
    }

    /// サーバーの招待リンクを作ってコピーする（見えている最初のテキストチャンネルで作る）
    pub fn create_guild_invite(&mut self, gid: &str) {
        let Some(cid) = self
            .channels
            .get(gid)
            .and_then(|cs| cs.iter().find(|c| c.is_text()).map(|c| c.id.clone()))
        else {
            self.ensure_guild_channels(gid);
            self.show_toast("チャンネルを読み込み中です。少し待ってからもう一度お試しください");
            return;
        };
        self.api(net::ApiCall::CreateInvite {
            channel_id: cid,
            body: serde_json::json!({ "max_age": 604800, "max_uses": 0, "temporary": false }),
        });
    }

    /// 画面下に短いお知らせを出す（「コピーしました」など）
    pub fn show_toast(&mut self, text: &str) {
        self.toast = Some((text.to_string(), std::time::Instant::now()));
        self.repaint_after(TOAST_TTL);
    }

    /// 結果は選択を開始したチャンネルに属する。画面が変わっても失わず、別の宛先に添付しない。
    pub fn poll_file_pick(&mut self) {
        let result = self
            .file_pick
            .as_ref()
            .map(|(channel_id, rx)| (channel_id.clone(), rx.try_recv()));
        match result {
            Some((channel_id, Ok(paths))) => {
                self.file_pick = None;
                if !paths.is_empty() {
                    if self.sel_channel.as_ref() != Some(&channel_id) {
                        self.status =
                            "選択したファイルは元のチャンネルに戻ると添付されます。".into();
                    }
                    self.picked_files = Some((channel_id, paths));
                }
            }
            Some((_, Err(std::sync::mpsc::TryRecvError::Disconnected))) => self.file_pick = None,
            _ => {}
        }
    }

    /// OS のファイル選択ダイアログを別スレッドで開く。
    pub fn pick_files(&mut self) {
        if self.file_pick.is_some() || self.picked_files.is_some() {
            self.status =
                "先に開いたファイル選択を完了し、元のチャンネルで確認してください。".into();
            return;
        }
        if let Some(channel_id) = self.sel_channel.clone() {
            self.file_pick = Some((
                channel_id,
                spawn_file_dialog(&self.waker, "ファイルをアップロード", true, None),
            ));
        }
    }

    /// 設定のテーマを読み直し、変わっていれば配色・フォント・背景画像を適用する。force = ファイルが変わった
    pub fn apply_theme(&mut self, force: bool) {
        let dir = themes_dir();
        let (r, err, warnings) = theme::resolve(
            &self.settings.theme,
            &dir,
            &self.settings.color_overrides,
            &self.settings.background,
        );
        self.theme_error = err;
        self.theme_warnings = warnings;
        let old = self.theme_applied.as_ref();
        if !force && old == Some(&r) {
            return;
        }
        let font_changed = old.map(|o| &o.font) != Some(&r.font);
        theme::set_colors(&r.colors);
        theme::set_parts(&r.parts);
        self.theme_dirty = true;
        if font_changed && (r.font.is_some() || old.is_some()) {
            self.font_dirty = true;
        }
        if let Some(theme::BackgroundImage { uri, .. }) = &r.background {
            if let Some(path) = uri.strip_prefix("file://") {
                // ファイルの背景は毎回読み直す（同じ名前のまま描き変えても反映させる）
                images::forget(uri);
                if let Err(e) = std::fs::metadata(path) {
                    self.theme_error = Some(format!("背景画像 {path}: {e}"));
                }
            }
        }
        self.theme_applied = Some(r);
        self.waker.wake();
    }

    /// 表示中のチャンネルの最新メッセージまでを Discord に既読として送る。
    /// ウィンドウにフォーカスがある（＝実際に見ている）ときだけ。新着が続くときは ACK_INTERVAL ごとにまとめる
    pub fn sync_read_state(&mut self, focused: bool) {
        if !focused {
            // フォーカス喪失時にはキーアップが届かないことがある。
            // PTT が押されたまま残って音声が送信され続けるのを防ぐ。
            self.voice.ptt_down.store(false, Relaxed);
            return;
        }
        let Some(cid) = self.sel_channel.clone() else {
            return;
        };
        // 控えのままの一覧は最新とは限らないので、既読を古い位置に戻さないよう送らない
        if self.stale_channels.contains(&cid) {
            return;
        }
        // 途中だけを出している一覧の末尾は最新ではないので、既読を戻さないよう送らない
        if self.is_detached(&cid) {
            return;
        }
        // MessageLogger が残した削除済みメッセージは既読の位置に使わない（もう存在しない ID のため）
        let Some(last) = self.messages.get(&cid).and_then(|l| {
            l.iter()
                .rev()
                .find(|m| m.log.as_ref().is_none_or(|x| x.deleted_at.is_none()))
        }) else {
            return;
        };
        if self.acked.get(&cid) == Some(&last.id) {
            // 最新まで既読にしてあるのに未読の印だけ残っていたら消す（届いたメッセージがすぐ消された、
            // つなぎ直しで件数だけ入り直した、など。残すと次のメッセージが来るまでレールのアイコンが出たままになる）
            self.unread.remove(&cid);
            self.mention_counts.remove(&cid);
            return;
        }
        if let Some(wait) = self
            .ack_retry_at
            .get(&cid)
            .and_then(|t| t.checked_duration_since(std::time::Instant::now()))
        {
            self.repaint_after(wait);
            return;
        }
        if let Some(wait) = self
            .last_ack
            .and_then(|t| ACK_INTERVAL.checked_sub(t.elapsed()))
        {
            self.repaint_after(wait);
            return;
        }
        let message_id = last.id.clone();
        let queued = self.backend.as_ref().is_some_and(|backend| {
            backend
                .cmd
                .send(Cmd::Ack {
                    channel_id: cid.clone(),
                    message_id: message_id.clone(),
                })
                .is_ok()
        });
        if !queued {
            // コマンドが届いていないときは既読扱いにせず、接続復帰後に再試行する。
            self.repaint_after(ACK_INTERVAL);
            return;
        }
        self.ack_retry_at.remove(&cid);
        self.acked.insert(cid.clone(), message_id);
        self.last_ack = Some(std::time::Instant::now());
        self.unread.remove(&cid);
        self.mention_counts.remove(&cid);
    }

    pub fn clear_message_error_status(&mut self, channel_id: &str) {
        if self.message_status_channel.as_deref() == Some(channel_id) {
            if self.status == MESSAGE_RELOAD_ERROR {
                self.status = self.gateway_status.clone();
            }
            self.message_status_channel = None;
        }
    }

    pub fn load_messages(&mut self, channel_id: String) {
        if self.pending_message_loads.contains_key(&channel_id) {
            return;
        }
        self.message_errors.remove(&channel_id);
        self.pending_message_loads
            .insert(channel_id.clone(), Vec::new());
        if self.backend.as_ref().is_none_or(|backend| {
            backend
                .cmd
                .send(Cmd::LoadMessages(channel_id.clone()))
                .is_err()
        }) {
            self.pending_message_loads.remove(&channel_id);
            self.message_errors.insert(channel_id);
        }
    }

    pub fn open_channel(&mut self, id: String) {
        if self.sel_channel.as_deref() != Some(&id) {
            if self.editing.take().is_some() {
                self.input.clear();
            }
            self.reply_to = None;
        }
        self.unread.remove(&id);
        self.mention_counts.remove(&id);
        // フォーラムにはメッセージが無い（投稿の一覧は ensure_forum が読む）
        let forum = self
            .sel_guild
            .as_deref()
            .and_then(|g| self.find_guild_channel(g, &id))
            .is_some_and(|c| c.is_forum());
        if !forum && (!self.messages.contains_key(&id) || self.stale_channels.contains(&id)) {
            self.load_messages(id.clone());
        }
        self.touch_channel(&id);
        self.sel_channel = Some(id);
        self.focus_input = true;
        self.cache_dirty = true;
        if self.screen_w < 700.0 {
            self.compact_sidebar_closed = true;
        }
    }

    /// 開いたチャンネルを「最近」に回し、MAX_CHANNELS を超えた古いチャンネルのメッセージと行の高さを捨てる。
    /// 1チャンネル最大 MAX_MSGS 件なので、巡回するほどメモリが増え続けるのを防ぐ
    pub fn touch_channel(&mut self, id: &str) {
        if let Some(i) = self.loaded_channels.iter().position(|c| c == id) {
            self.loaded_channels.remove(i);
        }
        self.loaded_channels.push_back(id.to_string());
        let mut evicted = false;
        while self.loaded_channels.len() > MAX_CHANNELS {
            let Some(old) = self.loaded_channels.pop_front() else {
                break;
            };
            self.messages.remove(&old);
            self.bump_msgs();
            self.history.remove(&old);
            self.stale_channels.remove(&old);
            self.no_access.remove(&old);
            self.message_errors.remove(&old);
            self.acked.remove(&old);
            self.ack_retry_at.remove(&old);
            evicted = true;
            // そのチャンネルでしか使っていない画像は、デコード済みの画像・テクスチャごと解放する
            if let Some(uris) = self.channel_images.remove(&old) {
                for uri in uris {
                    if !self.channel_images.values().any(|s| s.contains(&uri)) {
                        images::forget(&uri);
                    }
                }
            }
        }
        if evicted {
            self.prune_stickers();
        }
    }

    pub fn prune_stickers(&mut self) {
        let used: HashSet<&str> = self
            .messages
            .values()
            .flatten()
            .flat_map(|m| m.sticker_items.iter().map(|s| s.id.as_str()))
            .collect();
        self.stickers.retain(|id, _| used.contains(id.as_str()));
    }

    pub fn current_channel(&self) -> Option<&Channel> {
        let id = self.sel_channel.as_deref()?;
        match &self.sel_guild {
            None => self.dms.iter().find(|c| c.id == id),
            Some(g) => self.find_guild_channel(g, id),
        }
    }

    /// 未読チャンネルを持つサーバー（DM は None）。サーバーごとに未読全体をなめないよう1回で集める
    pub fn unread_guilds(&self) -> HashSet<Option<&str>> {
        self.unread.values().map(|g| g.as_deref()).collect()
    }

    /// 開いているテキストチャンネルのメンバー一覧を、見えている範囲だけ購読する（公式と同じく全員は取らない）。
    /// 先頭の 100 人は常に、スクロールしたらその辺りの 100 人ずつを足す
    pub fn subscribe_member_list(&mut self) {
        let (Some(gid), Some(cid)) = (self.sel_guild.clone(), self.sel_channel.clone()) else {
            return;
        };
        if !self.current_channel().is_some_and(|c| c.is_text()) {
            return;
        }
        let same_channel = self
            .member_sub
            .as_ref()
            .is_some_and(|(g, c, _)| *g == gid && *c == cid);
        let (first, last) = if same_channel {
            self.member_view
        } else {
            (0, 0)
        };
        let mut ranges = vec![[0u32, 99]];
        for chunk in [first / 100, last / 100] {
            let r = [chunk as u32 * 100, chunk as u32 * 100 + 99];
            if !ranges.contains(&r) {
                ranges.push(r);
            }
        }
        if self
            .member_sub
            .as_ref()
            .is_some_and(|(g, c, r)| *g == gid && *c == cid && *r == ranges)
        {
            return;
        }
        if !same_channel
            || !self
                .member_lists
                .get(&gid)
                .is_some_and(|l| l.channel_id == cid)
        {
            // 別のチャンネルの一覧は、新しい一覧が届くまで出さない
            self.member_lists.insert(
                gid.clone(),
                ServerMemberList {
                    channel_id: cid.clone(),
                    ..Default::default()
                },
            );
            self.member_ver += 1;
        }
        self.send(Cmd::SubscribeMembers {
            guild_id: gid.clone(),
            channel_id: cid.clone(),
            ranges: ranges.clone(),
        });
        self.member_sub = Some((gid, cid, ranges));
    }

    // ---- ログイン画面 ----
    /// ログインの通信（QR コード・メールアドレス・二段階認証）の結果を受け取る
    pub fn poll_login(&mut self) {
        let mut token = None;
        if let Some(rx) = &self.login.busy {
            match rx.try_recv() {
                Ok(e) => {
                    self.login.busy = None;
                    match e {
                        LoginEvent::Token(t) => token = Some(t),
                        LoginEvent::Mfa { ticket } => {
                            self.login.mfa_ticket = Some(ticket);
                            self.login.mfa_code.clear();
                            self.login_error = None;
                        }
                        LoginEvent::Error(e) => self.login_error = Some(e),
                        _ => {}
                    }
                }
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    self.login.busy = None;
                    self.login_error =
                        Some("ログインできませんでした。もう一度お試しください。".into());
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => {}
            }
        }
        if let Some(qr) = self.login.qr.as_mut() {
            while let Ok(e) = qr.rx.try_recv() {
                match e {
                    LoginEvent::QrUrl(url) => {
                        qr.modules = login::qr_modules(&url);
                        qr.scanned = None;
                    }
                    LoginEvent::QrScanned { name, avatar_url } => {
                        qr.scanned = Some((name, avatar_url))
                    }
                    LoginEvent::QrEnded(m) | LoginEvent::Error(m) => qr.ended = Some(m),
                    LoginEvent::Token(t) => token = Some(t),
                    LoginEvent::Mfa { .. } => {}
                }
            }
        }
        if let Some(t) = token {
            self.finish_login(t);
        }
    }

    /// 受け取ったトークンでつなぐ。保存は今までどおり、自分の情報が届いたとき（Event::Me）に token_input から行う
    pub fn finish_login(&mut self, token: String) {
        self.login.qr = None;
        self.login.password.clear();
        self.login.mfa_ticket = None;
        self.login_error = None;
        self.token_input = token.clone();
        self.start(token);
    }

    pub fn sidebar_order(&self) -> Vec<String> {
        match &self.sel_guild {
            None => self.dms.iter().map(|c| c.id.clone()).collect(),
            Some(g) => {
                let Some(chs) = self.channels.get(g) else {
                    return Vec::new();
                };
                let mut out: Vec<String> = chs
                    .iter()
                    .filter(|c| c.is_text() && c.parent_id.is_none())
                    .map(|c| c.id.clone())
                    .collect();
                let mut cats: Vec<&Channel> =
                    chs.iter().filter(|c| c.kind == Channel::CATEGORY).collect();
                cats.sort_by_key(|c| c.position);
                for cat in cats {
                    out.extend(
                        chs.iter()
                            .filter(|c| c.is_text() && c.parent_id.as_deref() == Some(&cat.id))
                            .map(|c| c.id.clone()),
                    );
                }
                out
            }
        }
    }

    pub fn select(&mut self, guild: Option<String>, channel: Option<String>) {
        if let Some(gid) = &guild {
            self.ensure_guild_channels(gid);
        }
        self.sel_guild = guild;
        self.sel_channel = None;
        self.cache_dirty = true;
        if let Some(c) = channel {
            self.open_channel(c);
        } else if self.screen_w < 700.0 {
            self.compact_sidebar_closed = false;
        }
    }

    /// (チャンネル名, サーバー名)
    pub fn voice_place(&self, guild_id: Option<&str>, channel_id: &str) -> (String, String) {
        match guild_id {
            Some(g) => {
                let ch = self
                    .channels
                    .get(g)
                    .and_then(|l| l.iter().find(|c| c.id == channel_id))
                    .and_then(|c| c.name.clone());
                let gn = self
                    .guilds
                    .iter()
                    .find(|x| x.id == g)
                    .map(|x| x.name.clone());
                (
                    ch.unwrap_or_else(|| "ボイス".into()),
                    gn.unwrap_or_default(),
                )
            }
            None => {
                let n = self
                    .dms
                    .iter()
                    .find(|c| c.id == channel_id)
                    .map(|c| c.dm_label())
                    .unwrap_or_default();
                (n, "ダイレクトメッセージ".into())
            }
        }
    }

    pub fn apply_voice_settings(&self) {
        let (v, s) = (&self.voice, &self.settings);
        v.mute.store(s.self_mute, Relaxed);
        v.deaf.store(s.self_deaf, Relaxed);
        if v.ptt.load(Relaxed) != s.ptt {
            // 入力モードの切り替えをまたいで古い押下状態を引き継がない。
            v.ptt_down.store(false, Relaxed);
            v.ptt.store(s.ptt, Relaxed);
        }
        v.set_vad_threshold(db_to_lin(s.vad_db));
        v.set_in_gain(s.in_gain);
        v.set_out_gain(s.out_gain);
        let mut changed = false;
        for (slot, val) in [
            (&v.input_device, &s.input_device),
            (&v.output_device, &s.output_device),
        ] {
            let mut g = slot.lock().unwrap();
            if *g != *val {
                *g = val.clone();
                changed = true;
            }
        }
        if changed {
            v.restart.store(true, Relaxed);
        }
    }

    pub fn voice_changed(&mut self) {
        self.apply_voice_settings();
        self.save_settings();
        self.send(Cmd::VoiceFlags);
    }

    pub fn toggle_mute(&mut self) {
        if self.settings.self_deaf {
            self.settings.self_deaf = false;
            self.settings.self_mute = false;
        } else {
            self.settings.self_mute = !self.settings.self_mute;
        }
        self.voice_changed();
    }

    pub fn toggle_deaf(&mut self) {
        self.settings.self_deaf = !self.settings.self_deaf;
        self.voice_changed();
    }

    pub fn mic_off(&self) -> bool {
        self.settings.self_mute || self.settings.self_deaf
    }

    pub fn join_voice(&mut self, guild_id: Option<String>, channel_id: String, ring: bool) {
        self.voice_status = Some(VoiceStatus::Connecting {
            guild_id: guild_id.clone(),
            channel_id: channel_id.clone(),
        });
        self.update_voice_call_started();
        self.send(Cmd::JoinVoice {
            guild_id,
            channel_id,
            ring,
        });
    }

    pub fn leave_voice(&mut self) {
        self.voice_status = None;
        self.my_stream = None;
        self.stream_viewers.clear();
        self.watching = None;
        self.update_voice_call_started();
        self.send(Cmd::LeaveVoice);
    }

    // ---- 画面共有 ----

    fn on_stream_event(&mut self, e: crate::golive::StreamEvent) {
        use crate::golive::{Phase, StreamEvent};
        match e {
            StreamEvent::Broadcast(Phase::Ended(err)) => {
                self.my_stream = None;
                self.stream_viewers.clear();
                if let Some(err) = err {
                    self.show_toast(&err);
                }
            }
            // やめた後に届いた古い通知で「共有中」に戻さない
            StreamEvent::Broadcast(p) => {
                if self.my_stream.is_some() {
                    self.my_stream = Some(p);
                }
            }
            StreamEvent::Viewers(v) => {
                if self.my_stream.is_some() {
                    self.stream_viewers = v;
                }
            }
            StreamEvent::Watch(key, Phase::Ended(err)) => {
                if self.watching.as_ref().is_some_and(|(k, _)| *k == key) {
                    self.watching = None;
                }
                if let Some(err) = err {
                    self.show_toast(&err);
                }
            }
            // 今見ようとしている配信の通知だけ反映する（やめた・切り替えた後の古い通知は捨てる）
            StreamEvent::Watch(key, p) => {
                if let Some((k, phase)) = &mut self.watching {
                    if *k == key {
                        *phase = p;
                    }
                }
            }
        }
    }

    /// 画面共有を始める・やめる
    pub fn toggle_stream(&mut self) {
        if self.my_stream.is_some() {
            self.my_stream = None;
            self.stream_viewers.clear();
            self.send(Cmd::StopStream);
        } else {
            self.my_stream = Some(crate::golive::Phase::Starting);
            self.send(Cmd::StartStream(self.settings.stream_quality));
        }
    }

    /// user_id の人の配信を見る（同じボイスチャンネルにいるとき）
    pub fn watch_stream(&mut self, user_id: &str) {
        let Some((guild, channel, true)) = self
            .voice_channel()
            .map(|(g, c, k)| (g.clone(), c.clone(), k))
        else {
            self.show_toast("配信を見るには、そのボイスチャンネルに接続してください");
            return;
        };
        if self.me.as_ref().is_some_and(|m| m.id == user_id) {
            return;
        }
        let key = crate::golive::stream_key(guild.as_deref(), &channel, user_id);
        if self.watching.as_ref().is_some_and(|(k, _)| *k == key) {
            return;
        }
        self.watching = Some((key.clone(), crate::golive::Phase::Starting));
        self.send(Cmd::WatchStream(key));
    }

    pub fn stop_watching(&mut self) {
        if self.watching.take().is_some() {
            self.send(Cmd::StopWatching);
        }
    }

    /// 配信中か（自分なら自分の配信の状態で判断する）
    pub fn is_streaming(&self, user_id: &str) -> bool {
        if self.me.as_ref().is_some_and(|m| m.id == user_id) {
            return matches!(self.my_stream, Some(crate::golive::Phase::Live));
        }
        self.voice_states
            .get(user_id)
            .is_some_and(|v| v.self_stream)
    }

    fn update_voice_call_started(&mut self) {
        match &self.voice_status {
            Some(VoiceStatus::Connected {
                guild_id,
                channel_id,
            }) => {
                if !self
                    .voice_call_started
                    .as_ref()
                    .is_some_and(|(g, c, _)| g == guild_id && c == channel_id)
                {
                    self.voice_call_started = Some((
                        guild_id.clone(),
                        channel_id.clone(),
                        std::time::Instant::now(),
                    ));
                }
            }
            _ => self.voice_call_started = None,
        }
    }

    /// 接続中（または接続しようとしている）チャンネル
    pub fn voice_channel(&self) -> Option<(&Option<String>, &String, bool)> {
        match self.voice_status.as_ref()? {
            VoiceStatus::Connecting {
                guild_id,
                channel_id,
            } => Some((guild_id, channel_id, false)),
            VoiceStatus::Connected {
                guild_id,
                channel_id,
            } => Some((guild_id, channel_id, true)),
            _ => None,
        }
    }

    /// channel_id → そこにいる user_id（並び順固定）。voice_states が変わったら作り直す
    pub fn reindex_voice(&mut self) {
        self.voice_index.clear();
        for v in self.voice_states.values() {
            if let Some(c) = &v.channel_id {
                self.voice_index
                    .entry(c.clone())
                    .or_default()
                    .push(v.user_id.clone());
            }
        }
        for l in self.voice_index.values_mut() {
            l.sort();
        }
    }

    pub fn find_user(&self, id: &str) -> Option<User> {
        if let Some(me) = self.me.as_ref().filter(|m| m.id == id) {
            return Some(me.clone());
        }
        if let Some(u) = self.voice_states.get(id).and_then(|v| v.user.clone()) {
            return Some(u);
        }
        self.dms
            .iter()
            .flat_map(|c| c.recipients.iter())
            .find(|u| u.id == id)
            .cloned()
    }

    pub fn is_speaking(&self, id: &str) -> bool {
        if self.me.as_ref().is_some_and(|m| m.id == id) {
            return self.voice.transmitting.load(Relaxed);
        }
        self.voice.speaking.lock().unwrap().contains(id)
    }

    /// あるボイスチャンネルにいる人
    pub fn voice_members<'a>(
        &'a self,
        channel_id: &str,
    ) -> impl Iterator<Item = &'a VoiceState> + 'a {
        self.voice_index
            .get(channel_id)
            .into_iter()
            .flatten()
            .filter_map(|id| self.voice_states.get(id))
    }

    pub fn set_status(&mut self, st: String) {
        self.send(Cmd::SetStatus(st.clone()));
        self.settings.status = st;
        self.save_settings();
    }

    pub fn open_profile(&mut self, user: User, anchor: Bounds<Pixels>, above: bool) {
        let guild_id = self.sel_guild.clone();
        let key = net::profile_key(&user.id, guild_id.as_deref());
        if !self.profiles.contains_key(&key) {
            self.profiles.insert(key, ProfileState::Loading);
            self.send(Cmd::LoadProfile {
                user_id: user.id.clone(),
                guild_id: guild_id.clone(),
            });
        }
        self.popup = Some(ProfilePopup {
            user,
            guild_id,
            anchor,
            above,
        });
    }

    pub fn save_profile(&mut self) {
        if self.profile_saving {
            return;
        }
        let Some(d) = &self.profile_draft else { return };
        let changed = |i: usize| {
            (d.fields[i].trim() != d.base[i].trim()).then(|| d.fields[i].trim().to_string())
        };
        let avatar = match &d.avatar {
            Some(AvatarEdit::Set { uri, .. }) => Some(Some(uri.clone())),
            Some(AvatarEdit::Remove) => Some(None),
            None => None,
        };
        self.send(Cmd::UpdateProfile {
            global_name: changed(0),
            pronouns: changed(1),
            bio: changed(2),
            avatar,
        });
        self.profile_saving = true;
        self.profile_error = None;
    }
}

// ---- ログイン画面の部品 ----
/// ログイン画面の状態（QR コード・メールアドレス・二段階認証）
#[derive(Default)]
pub struct LoginState {
    pub email: String,
    pub password: String,
    /// 二段階認証のコードを待っている（/auth/login が返した ticket）
    pub mfa_ticket: Option<String>,
    pub mfa_code: String,
    /// 送信中のメールアドレス・二段階認証の結果の受け口
    pub busy: Option<Receiver<LoginEvent>>,
    pub qr: Option<QrSession>,
}

/// QR コードでのログイン 1回分。捨てると通信も止まる
pub struct QrSession {
    pub rx: Receiver<LoginEvent>,
    pub cancel: Arc<std::sync::atomic::AtomicBool>,
    /// QR コードのマス目（1辺の数, 黒か）
    pub modules: Option<(usize, Vec<bool>)>,
    /// 読み取ったアカウント（名前, アバター）
    pub scanned: Option<(String, Option<String>)>,
    /// 期限切れ・取り消し（説明）
    pub ended: Option<String>,
}

impl QrSession {
    pub fn start(ctx: &Waker) -> Self {
        let (rx, cancel) = login::start_qr(ctx);
        QrSession {
            rx,
            cancel,
            modules: None,
            scanned: None,
            ended: None,
        }
    }
}

impl Drop for QrSession {
    fn drop(&mut self) {
        self.cancel.store(true, Relaxed);
    }
}

pub const PROFILE_NAME_MAX: usize = 32;
pub const PROFILE_PRONOUNS_MAX: usize = 40;
pub const PROFILE_BIO_MAX: usize = 190;
/// アバターは正方形に切り抜き、この大きさまで縮めて送る
pub const AVATAR_SIZE: u32 = 512;

pub enum AvatarEdit {
    /// 新しい画像（送る data URI と、プレビュー用の画像）
    Set {
        uri: String,
        tex: Arc<RenderImage>,
    },
    Remove,
}

// プレビューを差し替え・取消・保存したら GPUI の GPU キャッシュも解放する。
impl Drop for AvatarEdit {
    fn drop(&mut self) {
        if let Self::Set { tex, .. } = self {
            images::release(tex.clone());
        }
    }
}

/// 編集中のプロフィール。fields / base = [表示名, 代名詞, 自己紹介]
pub struct ProfileDraft {
    pub fields: [String; 3],
    pub base: [String; 3],
    pub avatar: Option<AvatarEdit>,
}

impl ProfileDraft {
    pub fn dirty(&self) -> bool {
        self.avatar.is_some()
            || self
                .fields
                .iter()
                .zip(&self.base)
                .any(|(a, b)| a.trim() != b.trim())
    }
}

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

mod commands;
mod jump;
mod pins;
#[cfg(test)]
mod regression_tests;
mod search;
mod threads;
pub use commands::*;
pub use jump::*;
pub use pins::*;
pub use search::*;
pub use threads::*;

/// チャット右側の枠に出すもの（メンバー一覧・DM のプロフィールの代わり）
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SidePanel {
    #[default]
    None,
    Pins,
    Search,
    /// チャンネルのスレッド一覧
    Threads,
}

/// サーバーごとの自分の通知設定
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GuildSettings {
    pub muted: bool,
    /// 0 = すべてのメッセージ、1 = @メンションのみ、2 = なし、3 = サーバーの既定
    pub notifications: u8,
}

impl GuildSettings {
    /// user_guild_settings の1件から（サーバーID, 設定）。DM の分（guild_id が null）は除く
    pub fn parse(v: &serde_json::Value) -> Option<(String, GuildSettings)> {
        let gid = v["guild_id"].as_str()?.to_string();
        let mut muted = v["muted"].as_bool().unwrap_or(false);
        // 期限つきのミュートは、期限を過ぎていれば解けている
        if let Some(end) = v["mute_config"]["end_time"]
            .as_str()
            .and_then(|t| DateTime::parse_from_rfc3339(t).ok())
        {
            if end < chrono::Utc::now() {
                muted = false;
            }
        }
        let notifications = v["message_notifications"].as_u64().unwrap_or(3).min(3) as u8;
        Some((
            gid,
            GuildSettings {
                muted,
                notifications,
            },
        ))
    }
}

#[cfg(test)]
mod settings_recovery_tests {
    use super::*;

    #[test]
    fn corrupt_settings_use_last_valid_backup() {
        let profile = Some("settings-recovery-test");
        let p = settings_path(profile);
        let _ = std::fs::remove_file(&p);
        let _ = std::fs::remove_file(p.with_extension("json.bak"));
        let first = Settings {
            font_size: 19.0,
            ..Default::default()
        };
        save_settings(profile, &first).unwrap();
        save_settings(profile, &Settings::default()).unwrap();
        std::fs::write(&p, b"{").unwrap();
        assert_eq!(load_settings(profile).font_size, 19.0);
        let _ = std::fs::remove_file(&p);
        let _ = std::fs::remove_file(p.with_extension("json.bak"));
    }
}

#[cfg(test)]
mod reaction_tests {
    use super::*;
    use crate::net::ApiCall;

    fn harness() -> (
        App,
        std::sync::mpsc::Sender<Event>,
        tokio::sync::mpsc::UnboundedReceiver<Cmd>,
    ) {
        let mut app = App::default();
        let (tx, ev) = std::sync::mpsc::channel();
        let (cmd, rx) = tokio::sync::mpsc::unbounded_channel();
        app.backend = Some(Backend { cmd, ev });
        app.me = Some(
            serde_json::from_value(serde_json::json!({ "id": "me", "username": "me" })).unwrap(),
        );
        let m = serde_json::json!({ "id": "1", "channel_id": "c", "author": { "id": "u", "username": "u" }, "timestamp": "2026-01-01T00:00:00+00:00" });
        app.messages
            .insert("c".into(), vec![serde_json::from_value(m).unwrap()]);
        (app, tx, rx)
    }

    fn reactions(app: &App) -> Vec<(String, u32, bool)> {
        app.messages["c"][0]
            .reactions
            .iter()
            .map(|r| (r.emoji.key().to_string(), r.count, r.me))
            .collect()
    }

    fn gateway(tx: &std::sync::mpsc::Sender<Event>, t: &str, user: &str) {
        let d = serde_json::json!({ "channel_id": "c", "message_id": "1", "user_id": user, "emoji": { "id": null, "name": "👍" } });
        tx.send(Event::Gateway { t: t.into(), d }).unwrap();
    }

    #[test]
    fn gateway_reactions_update_the_message() {
        let (mut app, tx, _rx) = harness();
        gateway(&tx, "MESSAGE_REACTION_ADD", "u");
        gateway(&tx, "MESSAGE_REACTION_ADD", "me");
        app.poll();
        assert_eq!(reactions(&app), vec![("👍".into(), 2, true)]);
        gateway(&tx, "MESSAGE_REACTION_REMOVE", "u");
        app.poll();
        assert_eq!(reactions(&app), vec![("👍".into(), 1, true)]);
        tx.send(Event::Gateway {
            t: "MESSAGE_REACTION_REMOVE_ALL".into(),
            d: serde_json::json!({ "channel_id": "c", "message_id": "1" }),
        })
        .unwrap();
        app.poll();
        assert!(reactions(&app).is_empty());
    }

    #[test]
    fn toggling_sends_the_call_and_ignores_the_echo() {
        let (mut app, tx, mut rx) = harness();
        let thumbs = model::ReactionEmoji {
            id: None,
            name: Some("👍".into()),
            animated: false,
        };
        app.toggle_reaction("c", "1", &thumbs);
        assert_eq!(reactions(&app), vec![("👍".into(), 1, true)]);
        let Cmd::Api {
            call: ApiCall::AddReaction { emoji, .. },
            ..
        } = rx.try_recv().unwrap()
        else {
            panic!("付けていない")
        };
        assert_eq!(emoji, "👍");
        gateway(&tx, "MESSAGE_REACTION_ADD", "me");
        app.poll();
        assert_eq!(reactions(&app), vec![("👍".into(), 1, true)]);
        app.toggle_reaction("c", "1", &thumbs);
        assert!(reactions(&app).is_empty());
        assert!(matches!(
            rx.try_recv().unwrap(),
            Cmd::Api {
                call: ApiCall::RemoveReaction { .. },
                ..
            }
        ));
    }

    #[test]
    fn failed_toggle_is_rolled_back() {
        let (mut app, tx, mut rx) = harness();
        let thumbs = model::ReactionEmoji {
            id: None,
            name: Some("👍".into()),
            animated: false,
        };
        app.toggle_reaction("c", "1", &thumbs);
        let Cmd::Api { id, call } = rx.try_recv().unwrap() else {
            panic!()
        };
        tx.send(Event::Api {
            id,
            call,
            result: Err("403".into()),
        })
        .unwrap();
        app.poll();
        assert!(reactions(&app).is_empty());
    }
}
