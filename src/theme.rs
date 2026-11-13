//! テーマ（配色・背景画像・フォント）。BetterDiscord のテーマのように、ファイルで見た目を変えられる。
//!
//! テーマファイルは設定フォルダの themes/ に置く TOML。例:
//! ```toml
//! name = "夜空"
//! author = "me"
//! font = "/usr/share/fonts/TTF/MyFont.ttf"   # 省略可
//!
//! [colors]          # 書いた色だけ上書き（#rrggbb または #rrggbbaa）
//! bg = "#0b1020"
//! accent = "#7aa2f7"
//!
//! [background]      # 背景画像（省略可）
//! image = "~/Pictures/wall.png"   # ファイルのパスか https:// の URL
//! opacity = 0.35                  # 画像の見え具合 0〜1
//! area = "window"                 # "chat"（チャット欄だけ・既定）か "window"（ウィンドウ全体。
//!                                 #  各面の色を #rrggbbaa で透かすと壁紙が見える）
//!
//! [parts.profile.card]   # 部品ごとの上書き（省略可）。部品の名前は PARTS、書ける項目は PART_PROPS
//! bg = "#0e0e1080"
//! radius = 16
//! ```
//!
//! [colors] は画面全体の配色、[parts] は名前の付いた部品1つだけの見た目。部品を増やすときは parts! に
//! 1行足し、UI 側でその部品に `.part(Part::名前)` を付ける（src/ui/kit.rs の Themed）。
use crate::color::Color32;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering::Relaxed};
use std::sync::RwLock;

fn pack(c: Color32) -> u32 {
    u32::from_le_bytes(c.rgba)
}

fn unpack(v: u32) -> Color32 {
    Color32 {
        rgba: v.to_le_bytes(),
    }
}

macro_rules! palette {
    ($($idx:literal $name:ident $key:literal $label:literal $en:literal [$r:literal, $g:literal, $b:literal];)*) => {
        /// (キー, 表示名, 既定の色)
        pub const KEYS: &[(&str, crate::i18n::Tr, Color32)] = &[$(($key, crate::i18n::Tr($label, $en), Color32::from_rgb($r, $g, $b))),*];
        static COLORS: [AtomicU32; KEYS.len()] = [$(AtomicU32::new(u32::from_le_bytes([$r, $g, $b, 255]))),*];
        $(
            #[allow(non_snake_case, dead_code)]
            #[inline]
            pub fn $name() -> Color32 {
                unpack(COLORS[$idx].load(Relaxed))
            }
        )*
    };
}

// 既定は今までの黒テーマ（Vesktop の黒に合わせたもの）
palette! {
    0 BG "bg" "画面の地" "Base background" [0x00, 0x00, 0x00];
    1 RAIL "rail" "サーバー一覧・カード" "Server list and cards" [0x18, 0x18, 0x1b];
    2 SIDEBAR "sidebar" "ポップアップ・浮いた面" "Popups and floating surfaces" [0x0e, 0x0e, 0x10];
    3 CHAT "chat" "チャット欄" "Chat area" [0x00, 0x00, 0x00];
    4 INPUT "input" "入力欄" "Input box" [0x10, 0x10, 0x13];
    5 USERBAR "userbar" "左下のユーザー欄" "User panel (bottom left)" [0x00, 0x00, 0x00];
    6 BLURPLE "accent" "アクセント" "Accent" [0x58, 0x65, 0xf2];
    7 TEXT "text" "本文" "Body text" [0xdb, 0xdb, 0xde];
    8 MUTED "muted" "控えめな文字" "Muted text" [0x94, 0x95, 0x9c];
    9 FAINT "faint" "薄い文字（時刻など）" "Faint text (timestamps, etc.)" [0x6e, 0x6f, 0x77];
    10 WHITE "strong" "強調する文字（名前など）" "Emphasized text (names, etc.)" [0xf2, 0xf2, 0xf3];
    11 HOVER "hover" "マウスを乗せた項目" "Hovered item" [0x12, 0x12, 0x14];
    12 SELECTED "selected" "選択中の項目" "Selected item" [0x1d, 0x1d, 0x20];
    13 MSG_HOVER "msg_hover" "マウスを乗せたメッセージ" "Hovered message" [0x0e, 0x0e, 0x10];
    14 DIVIDER "divider" "区切り線" "Dividers" [0x1e, 0x1e, 0x20];
    15 BORDER "border" "枠線" "Borders" [0x2a, 0x2a, 0x2e];
    16 LINK "link" "リンク" "Links" [0x00, 0xa8, 0xfc];
    17 RED "danger" "危険（削除・切断）" "Danger (delete, disconnect)" [0xda, 0x37, 0x3c];
    18 ORANGE "warning" "注意" "Warning" [0xc2, 0x7c, 0x0e];
    19 GREEN "success" "成功・通話" "Success and calls" [0x23, 0xa5, 0x5a];
    20 ST_ONLINE "online" "オンライン" "Online" [0x23, 0xa5, 0x5a];
    21 ST_IDLE "idle" "退席中" "Idle" [0xf0, 0xb2, 0x32];
    22 ST_DND "dnd" "取り込み中" "Do Not Disturb" [0xf2, 0x3f, 0x43];
    23 ST_OFFLINE "offline" "オフライン" "Offline" [0x80, 0x84, 0x8e];
    24 CALL_BG "call_bg" "通話画面の地" "Call screen background" [0x00, 0x00, 0x00];
    25 EMBED_BG "embed_bg" "埋め込みの地" "Embed background" [0x12, 0x12, 0x15];
    26 EMBED_BAR "embed_bar" "埋め込みの縦線（色指定なし）" "Embed side bar (no color set)" [0x2b, 0x2d, 0x31];
    27 WIDGET "widget" "ボタン・スライダー" "Buttons and sliders" [0x24, 0x24, 0x28];
    28 WIDGET_HOVER "widget_hover" "ボタン（マウスを乗せたとき）" "Buttons (hovered)" [0x30, 0x30, 0x35];
    29 REPLY_LINE "reply_line" "返信の線" "Reply line" [0x4e, 0x4f, 0x56];
    30 SETTINGS_BG "settings_bg" "設定画面の地" "Settings background" [0x00, 0x00, 0x00];
    31 SETTINGS_NAV "settings_nav" "設定画面のメニュー" "Settings menu" [0x00, 0x00, 0x00];
}

macro_rules! parts {
    ($($name:ident $key:literal $label:literal;)*) => {
        /// テーマで見た目を上書きできる部品
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        pub enum Part { $($name),* }
        /// (テーマファイルでの名前, 説明)。並びは Part と同じ
        pub const PARTS: &[(&str, &str)] = &[$(($key, $label)),*];
    };
}

// 名前は「画面.部品」。テーマファイルでは [parts.画面.部品] と書く
parts! {
    Titlebar "window.titlebar" "最上段のタイトルバー";
    SidebarPane "sidebar.pane" "左列全体（サーバー一覧＋チャンネル欄＋ユーザー欄）の地";
    SidebarRail "sidebar.rail" "サーバーアイコンの列";
    SidebarServer "sidebar.server" "サーバーアイコン（radius だけ。20 で丸になる）";
    SidebarChannels "sidebar.channels" "チャンネル・DM の一覧の欄";
    SidebarHeader "sidebar.header" "チャンネル欄の上のサーバー名・DM 検索の段";
    SidebarCategory "sidebar.category" "カテゴリー名・「ダイレクトメッセージ」の見出し（text）";
    SidebarRow "sidebar.row" "チャンネル・DM の行（text はふだんの文字色、hover_bg はマウスを乗せたとき）";
    SidebarRowSelected "sidebar.row_selected" "選択中のチャンネル・DM の行";
    SidebarUserbar "sidebar.userbar" "左下のユーザー欄";
    ChatHeader "chat.header" "チャット上部のチャンネル名の段";
    ChatInput "chat.input" "メッセージの入力欄";
    MessageText "message.text" "メッセージの本文（text）";
    MessageTime "message.time" "メッセージの時刻（text）";
    MessageDate "message.date" "日付の区切りの文字（text）";
    MessageCode "message.code" "コードブロック（bg・text・border・radius）";
    MessageEmbed "message.embed" "埋め込みのカード（bg・radius・opacity。左の縦線は埋め込みごとの色）";
    MessageToolbar "message.toolbar" "メッセージにマウスを乗せると出る操作ボタンの枠";
    MembersPanel "members.panel" "メンバー一覧の欄";
    MembersHeading "members.heading" "メンバー一覧の見出し（text）";
    MembersRow "members.row" "メンバー一覧の行（hover_bg はマウスを乗せたとき）";
    MenuBox "menu.box" "右クリックメニューなどの浮いた箱";
    MenuRow "menu.row" "メニューの行（hover_bg はマウスを乗せたとき）";
    DialogBox "dialog.box" "確認ダイアログ・チャンネル検索の箱";
    ProfilePanel "profile.panel" "DM 右側のプロフィール欄の地";
    ProfileCard "profile.card" "DM 右側のプロフィールカード";
    ProfilePopup "profile.popup" "名前やアバターを押して出るプロフィール";
    ProfileAvatar "profile.avatar" "プロフィールのアバターの縁（bg）";
    ProfileName "profile.name" "プロフィールの表示名（text）";
    ProfileText "profile.text" "プロフィールの本文（text）";
    ProfileMuted "profile.muted" "プロフィールの控えめな文字と見出し（text）";
    ProfileBox "profile.box" "プロフィールの中の囲み（アクティビティ・自己紹介・メニュー）";
    ProfileButton "profile.button" "プロフィールのボタン";
}

/// 部品に書ける項目（名前, 説明）
pub const PART_PROPS: &[(&str, crate::i18n::Tr)] = &[
    ("bg", trc!("地の色", "Background color")),
    ("text", trc!("文字の色", "Text color")),
    ("border", trc!("枠線の色", "Border color")),
    ("border_width", trc!("枠線の太さ", "Border width")),
    ("radius", trc!("角の丸み", "Corner radius")),
    ("opacity", trc!("中身ごとの見え具合（0〜1）", "Content opacity (0–1)")),
    (
        "hover_bg",
        trc!("マウスを乗せたときの地の色（行・ボタンなど、乗せると色が変わる部品だけ）", "Background color on hover (only for parts that change on hover, like rows and buttons)"),
    ),
];

/// 部品1つの上書き。None の項目は元の見た目のまま
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PartStyle {
    pub bg: Option<Color32>,
    pub text: Option<Color32>,
    pub border: Option<Color32>,
    pub border_width: Option<f32>,
    pub radius: Option<f32>,
    pub opacity: Option<f32>,
    pub hover_bg: Option<Color32>,
}

static PART_STYLES: RwLock<Vec<PartStyle>> = RwLock::new(Vec::new());

pub fn set_parts(parts: &[PartStyle]) {
    *PART_STYLES.write().unwrap() = parts.to_vec();
}

/// 今のテーマでの、その部品の上書き
pub fn part_style(p: Part) -> PartStyle {
    PART_STYLES
        .read()
        .unwrap()
        .get(p as usize)
        .copied()
        .unwrap_or_default()
}

/// テーマの [parts] を読む。知らない部品・項目や読めない値は飛ばして、warnings に理由を足す
fn resolve_parts(table: &toml::Table, warnings: &mut Vec<String>) -> Vec<PartStyle> {
    let mut out = vec![PartStyle::default(); PARTS.len()];
    for (group, v) in table {
        let Some(members) = v.as_table() else {
            warnings.push(tr!(
                "parts.{group} は [parts.{group}.部品] の形で書いてください",
                "Write parts.{group} as [parts.{group}.part]",
            ));
            continue;
        };
        for (name, v) in members {
            let key = format!("{group}.{name}");
            let Some(props) = v.as_table() else {
                warnings.push(tr!(
                    "parts.{key} は読めません。[parts.画面.部品] の下に項目を書いてください",
                    "Can't read parts.{key}. Write items under [parts.screen.part]",
                ));
                continue;
            };
            let Some(i) = PARTS.iter().position(|p| p.0 == key) else {
                warnings.push(tr!(
                    "{key} という部品はありません",
                    "There is no part named {key}",
                ));
                continue;
            };
            for (prop, v) in props {
                let color = || v.as_str().and_then(parse_color);
                let number = || {
                    v.as_float()
                        .or_else(|| v.as_integer().map(|n| n as f64))
                        .map(|n| n as f32)
                        .filter(|n| n.is_finite() && *n >= 0.0)
                };
                let st = &mut out[i];
                let ok = match prop.as_str() {
                    "bg" => color().map(|c| st.bg = Some(c)).is_some(),
                    "text" => color().map(|c| st.text = Some(c)).is_some(),
                    "border" => color().map(|c| st.border = Some(c)).is_some(),
                    "hover_bg" => color().map(|c| st.hover_bg = Some(c)).is_some(),
                    "border_width" => number().map(|n| st.border_width = Some(n)).is_some(),
                    "radius" => number().map(|n| st.radius = Some(n)).is_some(),
                    "opacity" => number().map(|n| st.opacity = Some(n.min(1.0))).is_some(),
                    _ => {
                        warnings.push(tr!(
                            "{key} に {prop} という項目はありません",
                            "{key} has no property named {prop}",
                        ));
                        continue;
                    }
                };
                if !ok {
                    let shown = match v {
                        toml::Value::String(s) => format!("\"{s}\""),
                        toml::Value::Integer(n) => n.to_string(),
                        toml::Value::Float(n) => n.to_string(),
                        toml::Value::Boolean(b) => b.to_string(),
                        _ => tr!("表や配列", "a table or array").into(),
                    };
                    warnings.push(tr!(
                        "{key} の {prop} を読めません（{shown}）",
                        "Can't read {prop} of {key} ({shown})",
                    ));
                }
            }
        }
    }
    out
}

pub fn set_colors(colors: &[Color32]) {
    for (slot, c) in COLORS.iter().zip(colors) {
        slot.store(pack(*c), Relaxed);
    }
}

pub fn current_colors() -> Vec<Color32> {
    COLORS.iter().map(|c| unpack(c.load(Relaxed))).collect()
}

/// "#rrggbb" / "#rrggbbaa" / "rrggbb"
pub fn parse_color(s: &str) -> Option<Color32> {
    let h = s.trim().trim_start_matches('#');
    let n = u32::from_str_radix(h, 16).ok()?;
    match h.len() {
        6 => Some(Color32::from_rgb((n >> 16) as u8, (n >> 8) as u8, n as u8)),
        8 => Some(Color32::from_rgba_unmultiplied(
            (n >> 24) as u8,
            (n >> 16) as u8,
            (n >> 8) as u8,
            n as u8,
        )),
        _ => None,
    }
}

pub fn format_color(c: Color32) -> String {
    let [r, g, b, a] = c.to_srgba_unmultiplied();
    if a == 255 {
        format!("#{r:02x}{g:02x}{b:02x}")
    } else {
        format!("#{r:02x}{g:02x}{b:02x}{a:02x}")
    }
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(default)]
pub struct Background {
    pub image: Option<String>,
    pub opacity: Option<f32>,
    /// "chat"（チャット欄だけ）/ "window"（ウィンドウ全体）
    pub area: Option<String>,
}

/// 背景画像の敷き方
#[derive(Clone, Debug, PartialEq)]
pub struct BackgroundImage {
    pub uri: String,
    /// 画像の見え具合 0〜1（残りを地の色で覆う）
    pub opacity: f32,
    /// ウィンドウ全体に敷く
    pub window: bool,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(default)]
pub struct ThemeFile {
    pub name: Option<String>,
    pub author: Option<String>,
    /// 設定画面のカードに出す説明
    pub description: Option<String>,
    pub font: Option<String>,
    pub colors: BTreeMap<String, String>,
    pub background: Background,
    /// 部品ごとの上書き（[parts.画面.部品]）。中身は resolve_parts で確かめる
    pub parts: toml::Table,
}

/// 実際に使う見た目（テーマ＋設定画面での上書きを合わせたもの）
#[derive(Clone, Debug, PartialEq)]
pub struct Resolved {
    pub colors: Vec<Color32>,
    /// 部品ごとの上書き（並びは PARTS と同じ）
    pub parts: Vec<PartStyle>,
    /// 背景画像の URI（file は bytes:// で読み込む）と見え具合
    pub background: Option<BackgroundImage>,
    pub font: Option<PathBuf>,
}

/// 組み込みのテーマ（キー, 表示名, 上書きする色）
pub const BUILTIN: &[(&str, crate::i18n::Tr, &[(&str, &str)])] = &[
    ("black", trc!("黒（既定）", "Black (default)"), &[]),
    (
        "dark",
        trc!("ダーク（Discord 風）", "Dark (Discord-like)"),
        &[
            ("bg", "#1e1f22"),
            ("rail", "#1e1f22"),
            ("sidebar", "#2b2d31"),
            ("chat", "#313338"),
            ("input", "#383a40"),
            ("userbar", "#232428"),
            ("hover", "#35373c"),
            ("selected", "#404249"),
            ("msg_hover", "#2e3035"),
            ("divider", "#3f4147"),
            ("border", "#1e1f22"),
            ("call_bg", "#000000"),
            ("embed_bg", "#2b2d31"),
            ("embed_bar", "#1e1f22"),
            ("widget", "#4e5058"),
            ("widget_hover", "#6d6f78"),
        ],
    ),
    (
        "midnight",
        trc!("ミッドナイト（青）", "Midnight (blue)"),
        &[
            ("bg", "#0b0e17"),
            ("rail", "#121726"),
            ("sidebar", "#0f1320"),
            ("chat", "#0b0e17"),
            ("input", "#141a2b"),
            ("userbar", "#0b0e17"),
            ("accent", "#7aa2f7"),
            ("text", "#c0caf5"),
            ("muted", "#8089b3"),
            ("faint", "#565f89"),
            ("strong", "#e0e6ff"),
            ("hover", "#151b2e"),
            ("selected", "#1f2740"),
            ("msg_hover", "#111627"),
            ("divider", "#1a2035"),
            ("border", "#283252"),
            ("link", "#7dcfff"),
            ("embed_bg", "#121726"),
            ("embed_bar", "#283252"),
            ("widget", "#1f2740"),
            ("widget_hover", "#2a3557"),
            ("reply_line", "#3b4261"),
        ],
    ),
    (
        "sakura",
        trc!("さくら（ピンク）", "Sakura (pink)"),
        &[
            ("bg", "#1a1216"),
            ("rail", "#241a1f"),
            ("sidebar", "#1f161b"),
            ("chat", "#1a1216"),
            ("input", "#2a1e24"),
            ("userbar", "#1a1216"),
            ("accent", "#f4a7c3"),
            ("text", "#f2dde5"),
            ("muted", "#b8949f"),
            ("faint", "#8a6b75"),
            ("strong", "#fff0f5"),
            ("hover", "#271c22"),
            ("selected", "#35252d"),
            ("msg_hover", "#21171c"),
            ("divider", "#2e2128"),
            ("border", "#43303a"),
            ("link", "#ff9ecb"),
            ("embed_bg", "#241a1f"),
            ("embed_bar", "#43303a"),
            ("widget", "#35252d"),
            ("widget_hover", "#4a333e"),
            ("reply_line", "#5a4250"),
        ],
    ),
];

/// 組み込みテーマの説明（設定画面のカード用）
pub fn builtin_description(key: &str) -> &'static str {
    match key {
        "black" => tr!(
            "真っ黒な地に白い文字。Vesktop の黒テーマに合わせた既定の配色。",
            "White text on pure black. The default colors, matching Vesktop's black theme."
        ),
        "dark" => tr!(
            "公式 Discord のダークテーマに近い灰色の配色。",
            "Gray colors close to the official Discord dark theme."
        ),
        "midnight" => tr!(
            "夜空のような紺色に、青いアクセント。",
            "Navy like the night sky, with a blue accent."
        ),
        "sakura" => tr!(
            "暗いピンクの地に、桜色のアクセント。",
            "A dark pink background with a cherry-blossom accent."
        ),
        _ => "",
    }
}

pub fn themes_dir(config_dir: &Path) -> PathBuf {
    config_dir.join("themes")
}

/// themes/ にある *.toml（ファイル名, 中身）。読めないものは名前だけ返して中身は None
pub fn list_files(dir: &Path) -> Vec<(String, Option<ThemeFile>)> {
    let mut out: Vec<(String, Option<ThemeFile>)> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "toml"))
        .map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            let file = std::fs::read_to_string(e.path())
                .ok()
                .and_then(|s| toml::from_str(&s).ok());
            (name, file)
        })
        .collect();
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

pub fn load_file(path: &Path) -> Result<ThemeFile, String> {
    let s = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    toml::from_str(&s).map_err(|e| format!("{}: {e}", path.display()))
}

fn expand_home(p: &str) -> PathBuf {
    match p.strip_prefix("~/") {
        Some(rest) => dirs::home_dir().unwrap_or_default().join(rest),
        None => PathBuf::from(p),
    }
}

/// 画像の指定を URI にする（ファイルは呼び出し側で bytes:// として読み込む）
pub fn image_uri(spec: &str, base: &Path) -> String {
    if spec.starts_with("https://") || spec.starts_with("http://") {
        return spec.to_string();
    }
    let p = expand_home(spec);
    let p = if p.is_relative() { base.join(p) } else { p };
    format!("file://{}", p.display())
}

/// 選んだテーマ（"builtin:black" か themes/ のファイル名）と設定画面での上書きから、使う見た目を決める。
/// 返すのは（見た目, ファイルを読めなかった理由, 読み飛ばした項目の一覧）
pub fn resolve(
    selected: &str,
    dir: &Path,
    overrides: &BTreeMap<String, String>,
    bg_override: &Background,
) -> (Resolved, Option<String>, Vec<String>) {
    let mut colors: Vec<Color32> = KEYS.iter().map(|k| k.2).collect();
    let mut set = |key: &str, val: &str| {
        if let (Some(i), Some(c)) = (KEYS.iter().position(|k| k.0 == key), parse_color(val)) {
            colors[i] = c;
        }
    };
    let mut error = None;
    let mut file = ThemeFile::default();
    if let Some(b) = selected.strip_prefix("builtin:") {
        if let Some((_, _, list)) = BUILTIN.iter().find(|t| t.0 == b) {
            for (k, v) in list.iter() {
                set(k, v);
            }
        }
    } else if !selected.is_empty() {
        match load_file(&dir.join(selected)) {
            Ok(f) => file = f,
            Err(e) => error = Some(e),
        }
    }
    let mut warnings = Vec::new();
    for (k, v) in &file.colors {
        if !KEYS.iter().any(|x| x.0 == k) {
            warnings.push(tr!(
                "colors に {k} という色はありません",
                "colors has no color named {k}",
            ));
        } else if parse_color(v).is_none() {
            warnings.push(tr!(
                "colors の {k} を読めません（\"{v}\"）",
                "Can't read {k} in colors (\"{v}\")",
            ));
        }
        set(k, v);
    }
    let parts = resolve_parts(&file.parts, &mut warnings);
    for (k, v) in overrides {
        set(k, v);
    }
    // 旧テーマでは設定画面の色をチャット欄から引き継ぐ。
    let specified = |key: &str| {
        [overrides.get(key), file.colors.get(key)]
            .into_iter()
            .flatten()
            .any(|v| parse_color(v).is_some())
    };
    if !specified("settings_bg") {
        colors[30] = colors[3];
    }
    if !specified("settings_nav") {
        colors[31] = colors[30];
    }
    let image = bg_override
        .image
        .clone()
        .filter(|s| !s.trim().is_empty())
        .or(file.background.image.clone());
    let opacity = bg_override
        .opacity
        .or(file.background.opacity)
        .unwrap_or(0.35)
        .clamp(0.0, 1.0);
    let window = bg_override
        .area
        .as_deref()
        .or(file.background.area.as_deref())
        == Some("window");
    let background = image
        .filter(|s| !s.trim().is_empty())
        .map(|s| BackgroundImage {
            uri: image_uri(&s, dir),
            opacity,
            window,
        });
    let font =
        file.font
            .as_deref()
            .map(expand_home)
            .map(|p| if p.is_relative() { dir.join(p) } else { p });
    (
        Resolved {
            colors,
            parts,
            background,
            font,
        },
        error,
        warnings,
    )
}

/// TOML の文字列にする（" と \ と制御文字だけ \uXXXX などで書き、それ以外の文字はそのまま）
fn toml_str(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if c.is_control() => out.push_str(&format!("\\u{:04X}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// 今の配色をテーマファイルにする（自分のテーマを作る出発点）
pub fn export(name: &str, colors: &[Color32], background: &Background) -> String {
    let mut s = format!(
        "name = {}\n# font = \"/path/to/font.ttf\"\n\n[colors]\n",
        toml_str(name)
    );
    for ((key, label, _), c) in KEYS.iter().zip(colors) {
        s.push_str(&format!(
            "{key} = \"{}\"   # {}\n",
            format_color(*c),
            label.s()
        ));
    }
    s.push_str("\n[background]\n");
    match &background.image {
        Some(i) if !i.is_empty() => s.push_str(&format!("image = {}\n", toml_str(i))),
        _ => s.push_str("# image = \"~/Pictures/wallpaper.png\"\n"),
    }
    s.push_str(&format!(
        "opacity = {}\n",
        background.opacity.unwrap_or(0.35)
    ));
    s.push_str(&tr!(
        "area = {}   # \"chat\" か \"window\"\n",
        "area = {}   # \"chat\" or \"window\"\n",
        toml_str(background.area.as_deref().unwrap_or("chat"))
    ));
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colors_parse_and_format() {
        assert_eq!(
            parse_color("#5865f2"),
            Some(Color32::from_rgb(0x58, 0x65, 0xf2))
        );
        assert_eq!(
            parse_color("ff000080").map(|c| c.to_srgba_unmultiplied()[3]),
            Some(0x80)
        );
        assert_eq!(parse_color("#12"), None);
        assert_eq!(format_color(Color32::from_rgb(1, 2, 3)), "#010203");
    }

    #[test]
    fn settings_colors_inherit_and_can_be_overridden() {
        let mut overrides = BTreeMap::new();
        overrides.insert("chat".into(), "#123456".into());
        let resolve_colors = |overrides: &BTreeMap<String, String>| {
            resolve(
                "builtin:black",
                Path::new("/nonexistent"),
                overrides,
                &Background::default(),
            )
            .0
            .colors
        };
        let colors = resolve_colors(&overrides);
        assert_eq!(colors[30], colors[3]);
        assert_eq!(colors[31], colors[3]);
        overrides.insert("settings_bg".into(), "#abcdef80".into());
        let colors = resolve_colors(&overrides);
        assert_eq!(colors[30], parse_color("#abcdef80").unwrap());
        assert_eq!(colors[31], colors[30]);
        overrides.insert("settings_nav".into(), "#112233".into());
        assert_eq!(
            resolve_colors(&overrides)[31],
            parse_color("#112233").unwrap()
        );
    }

    #[test]
    fn theme_file_then_overrides_win() {
        let dir = std::env::temp_dir().join(format!("riscord-theme-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("t.toml"),
            "name = \"t\"\n[colors]\nbg = \"#112233\"\naccent = \"#445566\"\nnope = \"#000000\"\n[background]\nimage = \"wall.png\"\nopacity = 0.5\n",
        )
        .unwrap();
        let mut ov = BTreeMap::new();
        ov.insert("accent".to_string(), "#abcdef".to_string());
        let (r, err, warnings) = resolve("t.toml", &dir, &ov, &Background::default());
        assert!(err.is_none());
        assert_eq!(warnings, ["colors に nope という色はありません"]);
        let at = |k: &str| r.colors[KEYS.iter().position(|x| x.0 == k).unwrap()];
        assert_eq!(at("bg"), Color32::from_rgb(0x11, 0x22, 0x33));
        assert_eq!(at("accent"), Color32::from_rgb(0xab, 0xcd, 0xef));
        assert_eq!(at("text"), KEYS[7].2, "書いていない色は既定のまま");
        let b = r.background.unwrap();
        assert_eq!(b.uri, format!("file://{}", dir.join("wall.png").display()));
        assert_eq!(b.opacity, 0.5);
        assert!(!b.window, "既定はチャット欄だけ");
        // 壊れたファイルは既定に戻してエラーを返す
        std::fs::write(dir.join("bad.toml"), "[colors\n").unwrap();
        let (r, err, _) = resolve("bad.toml", &dir, &BTreeMap::new(), &Background::default());
        assert!(err.is_some());
        assert_eq!(r.colors[0], KEYS[0].2);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// 濁点が分かれた名前（か + ゛）や " \ を含むパスも、読み直せる形で書き出す
    #[test]
    fn export_escapes_strings_for_toml() {
        let path = "/home/u/Downloads/いつもありか\u{3099}とう \"x\" \\y.jpg";
        let bg = Background {
            image: Some(path.into()),
            opacity: Some(0.3),
            area: None,
        };
        let text = export("名前\"", &current_colors(), &bg);
        let f: ThemeFile = toml::from_str(&text).unwrap();
        assert_eq!(f.background.image.as_deref(), Some(path));
        assert_eq!(f.name.as_deref(), Some("名前\""));
    }

    #[test]
    fn export_round_trips() {
        let (r, _, _) = resolve(
            "builtin:midnight",
            Path::new("/nonexistent"),
            &BTreeMap::new(),
            &Background::default(),
        );
        let text = export("x", &r.colors, &Background::default());
        let f: ThemeFile = toml::from_str(&text).unwrap();
        let dir = std::env::temp_dir().join(format!("riscord-theme-rt-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("x.toml"), &text).unwrap();
        let (back, _, _) = resolve("x.toml", &dir, &BTreeMap::new(), &Background::default());
        assert_eq!(back.colors, r.colors);
        assert_eq!(f.name.as_deref(), Some("x"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn template_covers_default_palette() {
        let f: ThemeFile = toml::from_str(include_str!("../themes/template.toml")).unwrap();
        assert_eq!(f.colors.len(), KEYS.len());
        for (key, _, default) in KEYS {
            assert_eq!(
                f.colors.get(*key).and_then(|v| parse_color(v)),
                Some(*default),
                "{key}"
            );
        }
        assert_eq!(f.background.opacity, Some(0.35));
        assert_eq!(f.background.area.as_deref(), Some("chat"));
    }

    #[test]
    fn parts_override_and_report_mistakes() {
        let f: ThemeFile = toml::from_str(
            r##"
            [parts.profile.card]
            bg = "#0e0e1080"
            radius = 16
            opacity = 0.5
            border_width = 2.5
            shadow = "big"
            text = "red"
            [parts.profile.nope]
            bg = "#000000"
            [parts.oops]
            bg = "#000000"
            "##,
        )
        .unwrap();
        let mut warnings = Vec::new();
        let parts = resolve_parts(&f.parts, &mut warnings);
        let card = parts[Part::ProfileCard as usize];
        assert_eq!(card.bg, parse_color("#0e0e1080"));
        assert_eq!(
            (card.radius, card.opacity, card.border_width, card.text),
            (Some(16.0), Some(0.5), Some(2.5), None)
        );
        assert_eq!(
            parts[Part::ProfileButton as usize],
            PartStyle::default(),
            "書いていない部品はそのまま"
        );
        warnings.sort();
        assert_eq!(
            warnings,
            [
                "parts.oops.bg は読めません。[parts.画面.部品] の下に項目を書いてください",
                "profile.card に shadow という項目はありません",
                "profile.card の text を読めません（\"red\"）",
                "profile.nope という部品はありません",
            ]
        );
    }

    #[test]
    fn part_names_are_unique_and_two_levels() {
        for (i, (key, label)) in PARTS.iter().enumerate() {
            assert_eq!(key.split('.').filter(|s| !s.is_empty()).count(), 2, "{key}");
            assert!(!label.is_empty(), "{key}");
            assert!(!PARTS[..i].iter().any(|p| p.0 == *key), "{key} が重複");
        }
    }

    /// テンプレートには全部の部品と項目が（コメントで）載っている
    #[test]
    fn template_lists_every_part() {
        let t = include_str!("../themes/template.toml");
        for (key, _) in PARTS {
            assert!(t.contains(&format!("[parts.{key}]")), "{key}");
        }
        for (prop, _) in PART_PROPS {
            assert!(t.contains(&format!("# {prop} = ")), "{prop}");
        }
        // 部品の見出しのコメントを外しても、警告なしで読めること
        let open: String = t
            .lines()
            .map(|l| {
                l.strip_prefix("# [parts.")
                    .map_or(l.to_string(), |r| format!("[parts.{r}"))
                    + "\n"
            })
            .collect();
        let f: ThemeFile = toml::from_str(&open).unwrap();
        let mut warnings = Vec::new();
        resolve_parts(&f.parts, &mut warnings);
        assert_eq!(warnings, Vec::<String>::new());
    }

    #[test]
    fn sample_theme_is_valid() {
        let f: ThemeFile = toml::from_str(include_str!("../themes/sample.toml")).unwrap();
        for (k, v) in &f.colors {
            assert!(KEYS.iter().any(|x| x.0 == k), "{k}");
            assert!(parse_color(v).is_some(), "{v}");
        }
    }

    #[test]
    fn builtin_keys_exist() {
        for (_, _, list) in BUILTIN {
            for (k, v) in list.iter() {
                assert!(KEYS.iter().any(|x| x.0 == *k), "{k}");
                assert!(parse_color(v).is_some(), "{v}");
            }
        }
    }
}
