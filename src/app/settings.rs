use std::path::PathBuf;
use std::sync::mpsc::Receiver;

use chrono::DateTime;

use crate::theme;
use crate::wake::Waker;

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
    /// 公式クライアントのサーバーフォルダと並び順（user_settings_proto / user_settings から取得）
    pub guild_folders: Vec<GuildFolder>,
    /// 開いているサーバーフォルダの ID 一覧
    pub open_folders: Vec<i64>,
    /// サーバーごとに最後に開いたチャンネル（guild_id → channel_id。DM は "" → channel_id）
    pub last_channels: std::collections::BTreeMap<String, String>,
    /// チャンネル一覧ペインの横幅（論理ピクセル、サーバーレール 72px を除く）
    pub channel_pane_width: f32,
    /// メンバー一覧（ユーザー一覧）ペインの横幅（論理ピクセル）
    pub member_pane_width: f32,
    /// 閉じているチャンネルカテゴリの ID 一覧
    pub collapsed_categories: std::collections::BTreeSet<String>,
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
            guild_folders: Vec::new(),
            open_folders: Vec::new(),
            last_channels: Default::default(),
            channel_pane_width: 240.0,
            member_pane_width: 240.0,
            collapsed_categories: Default::default(),
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
    std::fs::read_to_string(settings_path(profile))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

pub fn save_settings(profile: Option<&str>, s: &Settings) {
    let p = settings_path(profile);
    let _ = std::fs::create_dir_all(p.parent().unwrap());
    let _ = std::fs::write(p, serde_json::to_string_pretty(s).unwrap_or_default());
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

/// 公式クライアントのサーバーフォルダ（id が None で guild_ids が 1 件のものは単体のサーバー）
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct GuildFolder {
    pub id: Option<i64>,
    pub name: Option<String>,
    pub color: Option<u32>,
    pub guild_ids: Vec<String>,
}

/// サーバーレールに並べる1項目（単体サーバーまたはフォルダ）
#[derive(Clone, Debug)]
pub enum RailItem<'a> {
    Guild(&'a crate::model::Guild),
    Folder {
        id: i64,
        name: String,
        color: Option<u32>,
        open: bool,
        guilds: Vec<&'a crate::model::Guild>,
    },
}

fn read_varint(buf: &mut &[u8]) -> Option<u64> {
    let mut val = 0u64;
    let mut shift = 0u32;
    while let Some((&b, rest)) = buf.split_first() {
        *buf = rest;
        val |= ((b & 0x7f) as u64) << shift;
        if b & 0x80 == 0 {
            return Some(val);
        }
        shift += 7;
        if shift >= 64 {
            return None;
        }
    }
    None
}

fn read_bytes<'a>(buf: &mut &'a [u8]) -> Option<&'a [u8]> {
    let len = read_varint(buf)? as usize;
    if buf.len() < len {
        return None;
    }
    let (head, tail) = buf.split_at(len);
    *buf = tail;
    Some(head)
}

fn skip_field(buf: &mut &[u8], wire_type: u64) -> Option<()> {
    match wire_type {
        0 => read_varint(buf).map(|_| ()),
        1 if buf.len() >= 8 => {
            *buf = &buf[8..];
            Some(())
        }
        2 => read_bytes(buf).map(|_| ()),
        5 if buf.len() >= 4 => {
            *buf = &buf[4..];
            Some(())
        }
        _ => None,
    }
}

fn read_wrapped_u64(mut buf: &[u8]) -> Option<u64> {
    let mut out = 0u64;
    while !buf.is_empty() {
        let tag = read_varint(&mut buf)?;
        match (tag >> 3, tag & 7) {
            (1, 0) => out = read_varint(&mut buf)?,
            (1, 1) if buf.len() >= 8 => {
                out = u64::from_le_bytes(buf[..8].try_into().ok()?);
                buf = &buf[8..];
            }
            (_, wt) => skip_field(&mut buf, wt)?,
        }
    }
    Some(out)
}

fn read_wrapped_string(mut buf: &[u8]) -> Option<String> {
    let mut out = String::new();
    while !buf.is_empty() {
        let tag = read_varint(&mut buf)?;
        match (tag >> 3, tag & 7) {
            (1, 2) => out = std::str::from_utf8(read_bytes(&mut buf)?).ok()?.to_string(),
            (_, wt) => skip_field(&mut buf, wt)?,
        }
    }
    Some(out)
}

fn read_fixed64_list(buf: &mut &[u8], wire_type: u64, out: &mut Vec<String>) -> Option<()> {
    match wire_type {
        2 => {
            let mut b = read_bytes(buf)?;
            while b.len() >= 8 {
                let id = u64::from_le_bytes(b[..8].try_into().ok()?);
                out.push(id.to_string());
                b = &b[8..];
            }
            Some(())
        }
        1 if buf.len() >= 8 => {
            let id = u64::from_le_bytes(buf[..8].try_into().ok()?);
            *buf = &buf[8..];
            out.push(id.to_string());
            Some(())
        }
        0 => {
            let id = read_varint(buf)?;
            out.push(id.to_string());
            Some(())
        }
        _ => skip_field(buf, wire_type),
    }
}

fn parse_proto_folder(mut buf: &[u8]) -> Option<GuildFolder> {
    let mut folder = GuildFolder::default();
    while !buf.is_empty() {
        let tag = read_varint(&mut buf)?;
        match (tag >> 3, tag & 7) {
            (1, wt) => read_fixed64_list(&mut buf, wt, &mut folder.guild_ids)?,
            (2, 2) => folder.id = Some(read_wrapped_u64(read_bytes(&mut buf)?)? as i64),
            (2, 0) => folder.id = Some(read_varint(&mut buf)? as i64),
            (3, 2) => {
                let s = read_wrapped_string(read_bytes(&mut buf)?)?;
                if !s.trim().is_empty() {
                    folder.name = Some(s);
                }
            }
            (4, 2) => folder.color = Some(read_wrapped_u64(read_bytes(&mut buf)?)? as u32),
            (4, 0) => folder.color = Some(read_varint(&mut buf)? as u32),
            (_, wt) => skip_field(&mut buf, wt)?,
        }
    }
    Some(folder)
}

fn parse_proto_guild_folders(mut buf: &[u8]) -> Option<Vec<GuildFolder>> {
    let mut folders = Vec::new();
    let mut positions = Vec::new();
    while !buf.is_empty() {
        let tag = read_varint(&mut buf)?;
        match (tag >> 3, tag & 7) {
            (1, 2) => {
                let f = parse_proto_folder(read_bytes(&mut buf)?)?;
                if !f.guild_ids.is_empty() {
                    folders.push(f);
                }
            }
            (2, wt) => read_fixed64_list(&mut buf, wt, &mut positions)?,
            (_, wt) => skip_field(&mut buf, wt)?,
        }
    }
    if !folders.is_empty() {
        Some(folders)
    } else if !positions.is_empty() {
        Some(
            positions
                .into_iter()
                .map(|id| GuildFolder { guild_ids: vec![id], ..Default::default() })
                .collect(),
        )
    } else {
        None
    }
}

impl GuildFolder {
    /// PreloadedUserSettings (type 1) の base64 文字列から guild_folders (field 14) を取り出す
    pub fn from_proto_base64(b64: &str) -> Option<Vec<GuildFolder>> {
        use base64::engine::general_purpose::{STANDARD, STANDARD_NO_PAD, URL_SAFE, URL_SAFE_NO_PAD};
        use base64::Engine as _;
        let raw = b64.trim();
        if raw.is_empty() {
            return None;
        }
        let bytes = STANDARD
            .decode(raw)
            .or_else(|_| STANDARD_NO_PAD.decode(raw.trim_end_matches('=')))
            .or_else(|_| URL_SAFE.decode(raw))
            .or_else(|_| URL_SAFE_NO_PAD.decode(raw.trim_end_matches('=')))
            .ok()?;
        let mut buf = bytes.as_slice();
        let mut result = None;
        while !buf.is_empty() {
            let tag = read_varint(&mut buf)?;
            match (tag >> 3, tag & 7) {
                (14, 2) => {
                    let body = read_bytes(&mut buf)?;
                    if let Some(folders) = parse_proto_guild_folders(body) {
                        result = Some(folders);
                    }
                }
                (_, wt) => skip_field(&mut buf, wt)?,
            }
        }
        result
    }

    /// 旧形式の user_settings JSON（guild_folders / guild_positions）から取り出す
    pub fn from_user_settings_json(us: &serde_json::Value) -> Option<Vec<GuildFolder>> {
        if let Some(arr) = us["guild_folders"].as_array() {
            let mut folders = Vec::new();
            for e in arr {
                let guild_ids: Vec<String> = e["guild_ids"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|v| v.as_str().map(String::from).or_else(|| v.as_u64().map(|n| n.to_string())))
                    .collect();
                if guild_ids.is_empty() {
                    continue;
                }
                let id = e["id"].as_i64().or_else(|| e["id"].as_str()?.parse().ok());
                let name = e["name"].as_str().map(str::trim).filter(|s| !s.is_empty()).map(String::from);
                let color = e["color"].as_u64().map(|c| c as u32);
                folders.push(GuildFolder { id, name, color, guild_ids });
            }
            if !folders.is_empty() {
                return Some(folders);
            }
        }
        if let Some(arr) = us["guild_positions"].as_array() {
            let folders: Vec<GuildFolder> = arr
                .iter()
                .filter_map(|v| v.as_str().map(String::from).or_else(|| v.as_u64().map(|n| n.to_string())))
                .map(|id| GuildFolder { guild_ids: vec![id], ..Default::default() })
                .collect();
            if !folders.is_empty() {
                return Some(folders);
            }
        }
        None
    }

    /// ReadyExtra の JSON から user_settings_proto または user_settings を優先順で取り出す
    pub fn from_ready_extra(d: &serde_json::Value) -> Option<Vec<GuildFolder>> {
        d["user_settings_proto"]
            .as_str()
            .and_then(Self::from_proto_base64)
            .or_else(|| Self::from_user_settings_json(&d["user_settings"]))
    }
}
