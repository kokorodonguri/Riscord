#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod anim_loader;
mod clip;
mod http_loader;
mod model;
mod theme;
mod net;
mod sticker;
mod voice;

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{mpsc::Receiver, Arc};

use chrono::{DateTime, Local};
use eframe::egui::{
    self, pos2, text::LayoutJob, vec2, Align, Align2, Color32, CornerRadius, FontId, Frame, Layout, Margin, Rect,
    RichText, Sense, Shape, Stroke, TextFormat,
};
use model::{Channel, Embed, Guild, Member, Message, Presence, ProfileData, Role, User, VoiceState};
use net::{Cmd, Event, VoiceStatus};
use theme::{
    BG, BLURPLE, BORDER, CALL_BG, CHAT, DIVIDER, EMBED_BAR, EMBED_BG, FAINT, GREEN, HOVER, INPUT, LINK, MSG_HOVER, MUTED, ORANGE, RAIL, RED,
    REPLY_LINE, SELECTED, SIDEBAR, ST_DND, ST_IDLE, ST_OFFLINE, ST_ONLINE, TEXT, USERBAR, WHITE, WIDGET, WIDGET_HOVER,
};
use std::sync::atomic::Ordering::Relaxed;
use tokio::sync::mpsc::UnboundedSender;

// ---- 配色 ----（色は theme.rs のパレット。テーマで差し替えられる）

/// 選べるステータス（キー, 表示名）
/// チャット入力欄の ID（文字を打ったらここへフォーカスする）
const INPUT_ID: &str = "chat_input";

const STATUSES: [(&str, &str); 4] =
    [("online", "オンライン"), ("idle", "退席中"), ("dnd", "取り込み中"), ("invisible", "オフライン表示")];

fn status_label(s: &str) -> &'static str {
    match s {
        "online" => "オンライン",
        "idle" => "退席中",
        "dnd" => "取り込み中",
        "invisible" => "オフライン表示",
        _ => "オフライン",
    }
}

/// アバター右下に状態マークを描く（ringは背景色。くり抜いたように見せる）
fn paint_status(p: &egui::Painter, avatar: Rect, status: &str, ring: Color32) {
    let r = (avatar.width() * 0.16).max(4.0);
    status_mark(p, avatar.max - vec2(r * 0.8, r * 0.8), r, status, ring);
}

/// 中心cに半径rの状態マークを描く
fn status_mark(p: &egui::Painter, c: egui::Pos2, r: f32, status: &str, ring: Color32) {
    p.circle_filled(c, r + (r * 0.5).max(2.0), ring);
    match status {
        "online" => {
            p.circle_filled(c, r, ST_ONLINE());
        }
        "idle" => {
            p.circle_filled(c, r, ST_IDLE());
            p.circle_filled(c - vec2(r * 0.45, r * 0.45), r * 0.62, ring);
        }
        "dnd" => {
            p.circle_filled(c, r, ST_DND());
            p.rect_filled(Rect::from_center_size(c, vec2(r * 1.2, r * 0.45)), r * 0.2, ring);
        }
        _ => {
            p.circle_filled(c, r, ST_OFFLINE());
            p.circle_filled(c, r * 0.45, ring);
        }
    }
}

// ---- トークン ----
fn token_path() -> PathBuf {
    dirs::config_dir().unwrap_or_else(|| PathBuf::from(".")).join("riscord").join("token")
}

fn load_token() -> Option<String> {
    if let Ok(t) = std::env::var("DISCORD_TOKEN") {
        if !t.trim().is_empty() {
            return Some(t.trim().to_string());
        }
    }
    std::fs::read_to_string(token_path()).ok().map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

fn save_token(token: &str) -> std::io::Result<()> {
    use std::io::Write;
    let p = token_path();
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
struct Settings {
    zoom: f32,
    font_size: f32,
    compact: bool,
    /// スクロールの速さ（egui の既定に掛ける倍率）。マウスホイール（1ノッチ = 40px × 倍率）とタッチパッドで別
    wheel_speed: f32,
    touchpad_speed: f32,
    /// テーマ: "builtin:black" などの組み込み、または themes/ のファイル名
    theme: String,
    /// 設定画面で色ごとに変えたもの（キー → #rrggbb）。テーマより優先
    color_overrides: std::collections::BTreeMap<String, String>,
    /// 設定画面で指定したチャット欄の背景画像（テーマより優先）
    background: theme::Background,
    status: String,
    // ---- 音声 ----
    input_device: Option<String>,
    output_device: Option<String>,
    /// true = プッシュトゥトーク
    ptt: bool,
    ptt_key: Option<String>,
    /// 音声検出のしきい値 (dB)
    vad_db: f32,
    in_gain: f32,
    out_gain: f32,
    self_mute: bool,
    self_deaf: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
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
            input_device: None,
            output_device: None,
            ptt: false,
            ptt_key: None,
            vad_db: -40.0,
            in_gain: 1.0,
            out_gain: 1.0,
            self_mute: false,
            self_deaf: false,
        }
    }
}

fn db_to_lin(db: f32) -> f32 {
    10f32.powf(db / 20.0)
}

fn lin_to_db(v: f32) -> f32 {
    20.0 * v.max(1e-6).log10()
}

fn settings_path() -> PathBuf {
    token_path().with_file_name("settings.json")
}

fn load_settings() -> Settings {
    std::fs::read_to_string(settings_path()).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
}

fn save_settings(s: &Settings) {
    let p = settings_path();
    let _ = std::fs::create_dir_all(p.parent().unwrap());
    let _ = std::fs::write(p, serde_json::to_string_pretty(s).unwrap_or_default());
}

// ---- 見た目の設定 ----
/// fc-match を起動だけしておく（1回 15ms ほどかかるので、通常と Medium の2回を並行させる）
#[cfg(all(unix, not(target_os = "macos")))]
fn spawn_fc_match(query: &str) -> Option<std::process::Child> {
    use std::process::Stdio;
    std::process::Command::new("fc-match")
        .args(["-f", "%{file}", query])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()
}

fn setup_fonts(ctx: &egui::Context, custom: Option<&std::path::Path>) {
    #[cfg(all(unix, not(target_os = "macos")))]
    let (fc_regular, fc_medium) = (spawn_fc_match("sans:lang=ja"), spawn_fc_match("sans:lang=ja:weight=100"));
    let mut fonts = egui::FontDefinitions::default();
    let mut candidates: Vec<PathBuf> = Vec::new();
    #[cfg(windows)]
    {
        let dir = PathBuf::from(std::env::var("WINDIR").unwrap_or_else(|_| r"C:\Windows".into())).join("Fonts");
        for f in ["YuGothM.ttc", "meiryo.ttc", "msgothic.ttc"] {
            candidates.push(dir.join(f));
        }
    }
    #[cfg(target_os = "macos")]
    candidates.push("/System/Library/Fonts/ヒラギノ角ゴシック W3.ttc".into());
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        if let Some(p) = fc_regular
            .and_then(|c| c.wait_with_output().ok())
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .filter(|s| !s.is_empty())
        {
            candidates.push(p.into());
        }
        candidates.push("/usr/share/fonts/noto-cjk/NotoSansCJK-Regular.ttc".into());
        candidates.push("/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc".into());
    }
    if let Some(bytes) = candidates.iter().find_map(|p| std::fs::read(p).ok()) {
        fonts.font_data.insert("cjk".into(), Arc::new(egui::FontData::from_owned(bytes)));
        for fam in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
            fonts.families.entry(fam).or_default().push("cjk".into());
        }
    }
    let mut medium_candidates: Vec<PathBuf> = Vec::new();
    #[cfg(windows)]
    medium_candidates.push(PathBuf::from(std::env::var("WINDIR").unwrap_or_else(|_| r"C:\Windows".into())).join("Fonts").join("YuGothM.ttc"));
    #[cfg(target_os = "macos")]
    medium_candidates.push("/System/Library/Fonts/ヒラギノ角ゴシック W5.ttc".into());
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        if let Some(p) = fc_medium
            .and_then(|c| c.wait_with_output().ok())
            .filter(|o| o.status.success())
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .filter(|s| !s.is_empty())
        {
            medium_candidates.push(p.into());
        }
        medium_candidates.push("/usr/share/fonts/noto-cjk/NotoSansCJK-Medium.ttc".into());
    }
    let medium_loaded = if let Some(bytes) = medium_candidates.iter().find_map(|p| std::fs::read(p).ok()) {
        fonts.font_data.insert("medium".into(), Arc::new(egui::FontData::from_owned(bytes)));
        let mut family = fonts.families.get(&egui::FontFamily::Proportional).cloned().unwrap_or_default();
        family.insert(0, "medium".into());
        fonts.families.insert(egui::FontFamily::Name("medium".into()), family);
        true
    } else {
        false
    };
    // テーマで指定したフォントを先頭に（無い文字は日本語フォントなどに任せる）
    if let Some(bytes) = custom.and_then(|p| std::fs::read(p).ok()) {
        fonts.font_data.insert("theme".into(), Arc::new(egui::FontData::from_owned(bytes)));
        for fam in [egui::FontFamily::Proportional, egui::FontFamily::Name("medium".into())] {
            if let Some(list) = fonts.families.get_mut(&fam) {
                list.insert(0, "theme".into());
            }
        }
    }
    ctx.data_mut(|data| data.insert_temp(egui::Id::new("medium_font_loaded"), medium_loaded));
    ctx.set_fonts(fonts);
}

fn medium_font(ctx: &egui::Context, size: f32) -> FontId {
    if ctx.data(|data| data.get_temp::<bool>(egui::Id::new("medium_font_loaded")).unwrap_or(false)) {
        FontId::new(size, egui::FontFamily::Name("medium".into()))
    } else {
        FontId::proportional(size)
    }
}

fn themes_dir() -> PathBuf {
    theme::themes_dir(token_path().parent().unwrap_or(std::path::Path::new(".")))
}

/// themes/ の中身が変わったら知らせる（1秒ごとに更新時刻を見るだけの軽いスレッド）
fn watch_themes(ctx: egui::Context) -> Receiver<()> {
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
    let _ = std::thread::Builder::new().name("theme-watch".into()).spawn(move || {
        let mut last = stamp();
        loop {
            std::thread::sleep(std::time::Duration::from_secs(1));
            let now = stamp();
            if now != last {
                last = now;
                if tx.send(()).is_err() {
                    return;
                }
                ctx.request_repaint();
            }
        }
    });
    rx
}

/// OS のファイル選択ダイアログを別スレッドで開く（開いている間も画面を止めない）。選ばれたファイルが届く（取り消しなら空）
fn spawn_file_dialog(ctx: &egui::Context, title: &str, many: bool, filter: Option<(&str, &[&str])>) -> Receiver<Vec<PathBuf>> {
    let (tx, rx) = std::sync::mpsc::channel();
    let ctx = ctx.clone();
    let mut dialog = rfd::AsyncFileDialog::new().set_title(title);
    if let Some((name, exts)) = filter {
        dialog = dialog.add_filter(name, exts);
    }
    std::thread::spawn(move || {
        // Linux では xdg-desktop-portal と D-Bus でやり取りするので、その間だけ小さなランタイムを使う
        let Ok(rt) = tokio::runtime::Builder::new_current_thread().enable_all().build() else { return };
        let paths: Vec<PathBuf> = if many {
            rt.block_on(dialog.pick_files()).unwrap_or_default().into_iter().map(|f| f.path().to_path_buf()).collect()
        } else {
            rt.block_on(dialog.pick_file()).into_iter().map(|f| f.path().to_path_buf()).collect()
        };
        let _ = tx.send(paths);
        ctx.request_repaint();
    });
    rx
}

/// フォルダをファイルマネージャで開く
fn open_folder(path: &std::path::Path) {
    let _ = std::fs::create_dir_all(path);
    #[cfg(windows)]
    let cmd = "explorer";
    #[cfg(target_os = "macos")]
    let cmd = "open";
    #[cfg(all(unix, not(target_os = "macos")))]
    let cmd = "xdg-open";
    let _ = std::process::Command::new(cmd).arg(path).spawn();
}

/// チャット欄の背景画像を、欄いっぱいに（縦横比を保って切り抜いて）描き、上から地の色を重ねて薄くする
fn paint_background(ui: &egui::Ui, rect: Rect, bg: &(String, f32)) {
    let (uri, opacity) = bg;
    let img = egui::Image::new(uri.clone());
    if let Ok(egui::load::TexturePoll::Ready { texture }) = img.load_for_size(ui.ctx(), rect.size()) {
        let s = texture.size;
        let k = (rect.width() / s.x).max(rect.height() / s.y);
        let shown = rect.size() / (s * k);
        let uv = Rect::from_center_size(pos2(0.5, 0.5), shown);
        ui.painter().image(texture.id, rect, uv, Color32::WHITE);
        ui.painter().rect_filled(rect, 0.0, CHAT().gamma_multiply(1.0 - opacity));
    }
}

fn setup_theme(ctx: &egui::Context) {
    use egui::TextStyle::*;
    let mut v = egui::Visuals::dark();
    v.panel_fill = CHAT();
    v.window_fill = SIDEBAR();
    v.extreme_bg_color = RAIL();
    v.faint_bg_color = SIDEBAR();
    v.hyperlink_color = LINK();
    v.selection.bg_fill = BLURPLE();
    v.selection.stroke = Stroke::new(1.0_f32, WHITE());
    v.window_corner_radius = CornerRadius::same(8);
    v.window_stroke = Stroke::new(1.0_f32, BORDER());
    v.widgets.noninteractive.fg_stroke.color = TEXT();
    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0_f32, BORDER());
    v.widgets.inactive.fg_stroke.color = TEXT();
    v.widgets.inactive.bg_fill = WIDGET();
    v.widgets.inactive.weak_bg_fill = WIDGET();
    v.widgets.hovered.fg_stroke.color = WHITE();
    v.widgets.hovered.bg_fill = WIDGET_HOVER();
    v.widgets.hovered.weak_bg_fill = WIDGET_HOVER();
    v.widgets.active.bg_fill = BLURPLE();
    v.widgets.active.weak_bg_fill = BLURPLE();
    for w in [
        &mut v.widgets.noninteractive,
        &mut v.widgets.inactive,
        &mut v.widgets.hovered,
        &mut v.widgets.active,
        &mut v.widgets.open,
    ] {
        w.corner_radius = CornerRadius::same(4);
    }
    ctx.set_theme(egui::Theme::Dark);
    ctx.all_styles_mut(|s| {
        s.visuals = v.clone();
        s.text_styles.insert(Body, FontId::proportional(16.0));
        s.text_styles.insert(Button, FontId::proportional(14.0));
        s.text_styles.insert(Small, FontId::proportional(12.0));
        s.text_styles.insert(Heading, FontId::proportional(22.0));
        s.text_styles.insert(Monospace, FontId::monospace(14.0));
        s.spacing.button_padding = vec2(10.0, 5.0);
        s.spacing.scroll.bar_width = 8.0;
    });
}

// ---- ヘルパー ----
fn local_dt(ts: &str) -> Option<DateTime<Local>> {
    DateTime::parse_from_rfc3339(ts).ok().map(|t| t.with_timezone(&Local))
}

fn fmt_time(ts: &str) -> String {
    let Some(t) = local_dt(ts) else { return String::new() };
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

fn name_color(id: &str) -> Color32 {
    let h = id.bytes().fold(0u32, |a, b| a.wrapping_mul(31).wrapping_add(b as u32));
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

fn initials(name: &str) -> String {
    let words: Vec<&str> = name.split_whitespace().collect();
    if words.len() > 1 {
        words.iter().filter_map(|w| w.chars().next()).take(3).collect()
    } else {
        name.chars().take(2).collect()
    }
}

fn user_avatar_url(u: &User) -> String {
    match &u.avatar {
        Some(h) => format!("https://cdn.discordapp.com/avatars/{}/{}.png?size=80", u.id, h),
        None => {
            let n = u.id.parse::<u64>().map(|i| (i >> 22) % 6).unwrap_or(0);
            format!("https://cdn.discordapp.com/embed/avatars/{n}.png")
        }
    }
}

fn guild_icon_url(g: &Guild) -> Option<String> {
    g.icon.as_ref().map(|h| format!("https://cdn.discordapp.com/icons/{}/{}.png?size=96", g.id, h))
}

fn dm_icon_url(c: &Channel) -> Option<String> {
    if let Some(h) = &c.icon {
        Some(format!("https://cdn.discordapp.com/channel-icons/{}/{}.png?size=80", c.id, h))
    } else if c.recipients.len() == 1 {
        Some(user_avatar_url(&c.recipients[0]))
    } else {
        None
    }
}

/// 画像が読めればそれを、無理なら頭文字入りの図形を描く
fn paint_icon(ui: &egui::Ui, rect: Rect, url: Option<&str>, fallback: &str, radius: f32, bg: Color32) {
    if let Some(url) = url {
        let img = egui::Image::new(url.to_string());
        if let Ok(egui::load::TexturePoll::Ready { texture }) = img.load_for_size(ui.ctx(), rect.size()) {
            egui::Image::from_texture(texture).corner_radius(radius).paint_at(ui, rect);
            return;
        }
    }
    ui.painter().rect_filled(rect, radius, bg);
    ui.painter().text(
        rect.center(),
        Align2::CENTER_CENTER,
        initials(fallback),
        FontId::proportional(rect.height() * 0.34),
        WHITE(),
    );
}

fn pill_radius() -> CornerRadius {
    CornerRadius { nw: 0, sw: 0, ne: 4, se: 4 }
}

/// 新しい Discord と同じく角丸の四角アイコン。DM ボタンは url=None・name="" でロゴを描く
fn rail_item(ui: &mut egui::Ui, selected: bool, unread: bool, name: &str, url: Option<&str>) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(72.0, 48.0), Sense::click());
    // スクロールで見えていない行は場所だけ取り、画像の照会や描画を省く
    if !ui.is_rect_visible(rect) {
        return resp;
    }
    let icon = Rect::from_center_size(rect.center(), vec2(40.0, 40.0));
    let active = selected || resp.hovered();
    let bg = if active { BLURPLE() } else { HOVER() };
    if name.is_empty() {
        ui.painter().rect_filled(icon, 12.0, bg);
        paint_logo(ui.painter(), icon.shrink(9.0), WHITE(), bg);
    } else {
        paint_icon(ui, icon, url, name, 12.0, bg);
    }
    let pill_h = if selected {
        40.0
    } else if resp.hovered() {
        20.0
    } else if unread {
        8.0
    } else {
        0.0
    };
    if pill_h > 0.0 {
        let r = Rect::from_min_size(pos2(rect.left(), rect.center().y - pill_h / 2.0), vec2(4.0, pill_h));
        ui.painter().rect_filled(r, pill_radius(), WHITE());
    }
    if name.is_empty() { resp.on_hover_text("ダイレクトメッセージ") } else { resp.on_hover_text(name) }
}

/// 吹き出しの顔（DM ボタン・タイトルバー用のロゴ）
fn paint_logo(p: &egui::Painter, rect: Rect, color: Color32, eye: Color32) {
    let c = rect.center();
    let s = rect.width().min(rect.height());
    let body = Rect::from_center_size(c + vec2(0.0, s * 0.04), vec2(s, s * 0.72));
    p.rect_filled(body, s * 0.3, color);
    for x in [-1.0_f32, 1.0] {
        let ear = c + vec2(x * s * 0.3, -s * 0.3);
        p.circle_filled(ear, s * 0.12, color);
    }
    for x in [-1.0_f32, 1.0] {
        p.add(Shape::ellipse_filled(c + vec2(x * s * 0.19, s * 0.02), vec2(s * 0.09, s * 0.11), eye));
    }
}

fn row_frame(ui: &mut egui::Ui, height: f32, selected: bool, unread: bool) -> (egui::Response, Rect, Color32) {
    let w = ui.available_width();
    let (rect, resp) = ui.allocate_exact_size(vec2(w, height), Sense::click());
    let r = rect.shrink2(vec2(8.0, 1.0));
    let fill = if selected {
        SELECTED()
    } else if resp.hovered() {
        HOVER()
    } else {
        Color32::TRANSPARENT
    };
    ui.painter().rect_filled(r, 8.0, fill);
    if unread && !selected {
        let p = Rect::from_min_size(pos2(rect.left(), rect.center().y - 4.0), vec2(4.0, 8.0));
        ui.painter().rect_filled(p, pill_radius(), WHITE());
    }
    let color = if selected || unread {
        WHITE()
    } else if resp.hovered() {
        TEXT()
    } else {
        MUTED()
    };
    (resp, r, color)
}

fn member_role_color(member: &Member, roles: &[Role]) -> Option<Color32> {
    let role = roles.iter().filter(|r| member.roles.contains(&r.id)).max_by_key(|r| r.position)?;
    (role.color != 0).then(|| Color32::from_rgb((role.color >> 16) as u8, (role.color >> 8) as u8, role.color as u8))
}

fn channel_row(ui: &mut egui::Ui, selected: bool, unread: bool, name: &str) -> egui::Response {
    let (resp, r, color) = row_frame(ui, 32.0, selected, unread);
    if !ui.is_rect_visible(r) {
        return resp;
    }
    let p = ui.painter().with_clip_rect(r);
    p.text(pos2(r.left() + 8.0, r.center().y), Align2::LEFT_CENTER, "#", FontId::proportional(20.0), FAINT());
    p.text(pos2(r.left() + 30.0, r.center().y), Align2::LEFT_CENTER, name, medium_font(ui.ctx(), 16.0), color);
    resp
}

fn dm_row(ui: &mut egui::Ui, selected: bool, unread: bool, c: &Channel, presence: Option<&Presence>) -> egui::Response {
    let (resp, r, color) = row_frame(ui, 42.0, selected, unread);
    // スクロールで見えていない行は場所だけ取り、名前・URL の組み立てや描画を省く
    if !ui.is_rect_visible(r) {
        return resp;
    }
    let label = c.dm_label();
    let av = Rect::from_min_size(pos2(r.left() + 8.0, r.center().y - 16.0), vec2(32.0, 32.0));
    paint_icon(ui, av, dm_icon_url(c).as_deref(), &label, 16.0, BLURPLE());
    // 1対1のDMだけ相手の状態を出す（情報が無ければオフライン扱い）
    let one_to_one = c.recipients.len() == 1;
    if one_to_one {
        let ring = if selected {
            SELECTED()
        } else if resp.hovered() {
            HOVER()
        } else {
            BG()
        };
        paint_status(ui.painter(), av, presence.map_or("offline", |p| p.status.as_str()), ring);
    }
    let p = ui.painter().with_clip_rect(r);
    let x = av.right() + 12.0;
    match presence.and_then(|p| p.activity.as_deref()).filter(|_| one_to_one) {
        Some(act) => {
            p.text(pos2(x, r.center().y - 8.0), Align2::LEFT_CENTER, label, FontId::proportional(16.0), color);
            p.text(pos2(x, r.center().y + 10.0), Align2::LEFT_CENTER, act, FontId::proportional(12.0), MUTED());
        }
        None => {
            p.text(pos2(x, r.center().y), Align2::LEFT_CENTER, label, FontId::proportional(16.0), color);
        }
    }
    if one_to_one {
        resp.on_hover_text(status_label(presence.map_or("offline", |p| p.status.as_str())))
    } else {
        resp
    }
}

fn category_header(ui: &mut egui::Ui, name: &str) {
    ui.add_space(14.0);
    let w = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(vec2(w, 20.0), Sense::hover());
    if !ui.is_rect_visible(rect) {
        return;
    }
    ui.painter().with_clip_rect(rect).text(
        pos2(rect.left() + 16.0, rect.center().y),
        Align2::LEFT_CENTER,
        name.to_uppercase(),
        FontId::proportional(12.0),
        FAINT(),
    );
}

fn date_divider(ui: &mut egui::Ui, d: DateTime<Local>) {
    ui.add_space(20.0);
    let w = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(vec2(w, 16.0), Sense::hover());
    let p = ui.painter();
    let galley = p.layout_no_wrap(d.format("%Y年%-m月%-d日").to_string(), FontId::proportional(12.0), FAINT());
    let c = rect.center();
    let half = galley.size().x / 2.0 + 8.0;
    let stroke = Stroke::new(1.0_f32, DIVIDER());
    p.hline((rect.left() + 16.0)..=(c.x - half), c.y, stroke);
    p.hline((c.x + half)..=(rect.right() - 16.0), c.y, stroke);
    p.galley(c - galley.size() / 2.0, galley, FAINT());
}

/// 幅・高さを最大サイズに収まるよう縮める
fn fit_size(w: Option<u32>, h: Option<u32>, max: egui::Vec2) -> Option<egui::Vec2> {
    let (w, h) = (w? as f32, h? as f32);
    if w <= 0.0 || h <= 0.0 {
        return None;
    }
    let scale = (max.x / w).min(max.y / h).min(1.0);
    Some(vec2(w * scale, h * scale))
}

/// Discordのメディアプロキシで縮小版を取る（GIFはアニメを残すため原寸）
fn resized(proxy: Option<&str>, url: &str, size: egui::Vec2) -> String {
    let base = proxy.unwrap_or(url);
    let path = base.split(['?', '#']).next().unwrap_or("").to_lowercase();
    let out = if base.contains("media.discordapp.net") && !path.ends_with(".gif") {
        let sep = if base.contains('?') { '&' } else { '?' };
        let ppp = 2.0; // 高DPI用に倍で取る
        // WebP は animated=true を付けないとプロキシが1枚目だけにしてしまう
        let anim = if path.ends_with(".webp") { "&animated=true" } else { "" };
        format!("{base}{sep}width={}&height={}{anim}", (size.x * ppp) as u32, (size.y * ppp) as u32)
    } else {
        base.to_string()
    };
    animated_hint(out, &path)
}

/// egui は URI の末尾が .gif / .webp のときだけアニメーションとして読む。
/// 署名やサイズ指定のクエリで終わる URL には `#.gif` / `#.webp` を足して知らせる（# 以降は送信されない）
fn animated_hint(uri: String, path: &str) -> String {
    let ext = [".gif", ".webp"].into_iter().find(|e| path.ends_with(e));
    match ext {
        Some(e) if !uri.ends_with(e) && !uri.contains('#') => format!("{uri}#{e}"),
        _ => uri,
    }
}

/// ウィンドウにフォーカスがない間は GIF・アニメ画像を止める（公式と同じ）。
/// 見えないウィンドウで描き続けると、Wayland のコンポジタが描画用バッファを返さず、画面の書き換えで
/// メインスレッドが止まって「応答なし」になるため。update で毎フレーム設定する
static ANIM_PAUSED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

fn anim_paused() -> bool {
    ANIM_PAUSED.load(Relaxed)
}

/// egui がアニメーションとして扱う URI か（egui 内部の判定と同じ）
fn is_anim_uri(u: &str) -> bool {
    u.ends_with(".gif") || u.contains(".gif#") || u.ends_with(".webp") || u.contains(".webp#")
}

/// 画像。止めている間はアニメーションの1フレーム目を静止画として出す（再描画を要求しない）
fn anim_image(ctx: &egui::Context, uri: String) -> egui::Image<'static> {
    if anim_paused() && is_anim_uri(&uri) {
        if let Ok(egui::load::TexturePoll::Ready { texture }) = ctx.try_load_texture(&format!("{uri}#0"), Default::default(), Default::default()) {
            return egui::Image::from_texture(texture);
        }
    }
    egui::Image::new(uri)
}

fn show_image(ui: &mut egui::Ui, src: String, open: &str, size: Option<egui::Vec2>, max: egui::Vec2) -> egui::Response {
    let mut img = anim_image(ui.ctx(), src).corner_radius(8).sense(Sense::click());
    img = match size {
        Some(s) => img.fit_to_exact_size(s),
        None => img.max_size(max),
    };
    let r = ui.add(img).on_hover_cursor(egui::CursorIcon::PointingHand);
    if r.clicked() {
        ui.ctx().open_url(egui::OpenUrl::new_tab(open));
    }
    ui.add_space(4.0);
    r
}

enum StickerState {
    Loading,
    Ready { frames: Vec<(egui::TextureHandle, u32)>, total: u32 },
    Failed,
}

/// メッセージ描画中に参照・収集するもの
struct MediaCx<'a> {
    settings: &'a Settings,
    stickers: &'a HashMap<String, StickerState>,
    roles: &'a [Role],
    want: Vec<(String, u8)>,
    /// このフレームで表示した添付・埋め込み画像の URI（チャンネル破棄時に画像キャッシュを解放するため）
    images: Vec<String>,
    /// アバターや名前がクリックされたユーザーと、その位置
    clicked_user: Option<(User, Rect)>,
    /// 自分のユーザーID（編集・削除ボタンを出すか）
    me: Option<&'a str>,
    /// 返信先・編集中として強調する行
    highlight: Option<&'a str>,
    /// 前のフレーム／今のフレームのツールバー（持ち主の行, 位置）
    tb_prev: Option<(String, Rect)>,
    tb_now: Option<(String, Rect)>,
    action: Option<MsgAction>,
}

fn show_sticker(ui: &mut egui::Ui, st: &model::StickerItem, cx: &mut MediaCx) {
    const BOX: f32 = 160.0;
    match cx.stickers.get(&st.id) {
        Some(StickerState::Ready { frames, total }) => {
            let (tex, wait) = if frames.len() == 1 || *total == 0 || anim_paused() {
                (&frames[0].0, None)
            } else {
                let now = (ui.input(|i| i.time) * 1000.0) as u64 % *total as u64;
                let mut acc = 0u64;
                let mut pick = (&frames[0].0, 0u64);
                for (t, ms) in frames {
                    acc += *ms as u64;
                    if now < acc {
                        pick = (t, acc - now);
                        break;
                    }
                }
                (pick.0, Some(pick.1))
            };
            let s = tex.size_vec2();
            let k = (BOX / s.x).min(BOX / s.y);
            let r = ui.add(egui::Image::new(egui::load::SizedTexture::new(tex.id(), s * k))).on_hover_text(&st.name);
            if let Some(ms) = wait {
                if ui.is_rect_visible(r.rect) {
                    ui.ctx().request_repaint_after(std::time::Duration::from_millis(ms.max(10)));
                }
            }
        }
        Some(StickerState::Failed) => {
            ui.label(RichText::new(format!("[スタンプ: {}]", st.name)).color(MUTED()));
        }
        state => {
            if state.is_none() {
                cx.want.push((st.id.clone(), st.format_type));
            }
            let (rect, _) = ui.allocate_exact_size(vec2(BOX, BOX), Sense::hover());
            egui::Spinner::new().paint_at(ui, Rect::from_center_size(rect.center(), vec2(24.0, 24.0)));
        }
    }
}

/// 添付画像・スタンプ・埋め込み画像
fn message_media(ui: &mut egui::Ui, m: &Message, cx: &mut MediaCx) {
    let max = vec2(ui.available_width().min(420.0), 320.0);
    for a in &m.attachments {
        if a.is_image() {
            let size = fit_size(a.width, a.height, max);
            let src = resized(a.proxy_url.as_deref(), &a.url, size.unwrap_or(max));
            cx.images.push(src.clone());
            show_image(ui, src, &a.url, size, max);
        } else {
            ui.hyperlink_to(RichText::new(format!("📎 {}", a.filename)).color(LINK()), &a.url);
        }
    }
    for st in &m.sticker_items {
        show_sticker(ui, st, cx);
    }
    for e in &m.embeds {
        if let Some(md) = e.media() {
            let size = fit_size(md.width, md.height, max);
            // GIF リンクは .gif 版を取って動かす。取れなかったら静止画に戻す
            let gif = e.animated_url().filter(|g| egui::Image::new(g.clone()).load_for_size(ui.ctx(), size.unwrap_or(max)).is_ok());
            let src = gif.unwrap_or_else(|| resized(md.proxy_url.as_deref(), &md.url, size.unwrap_or(max)));
            let open = e.url.as_deref().unwrap_or(&md.url);
            cx.images.push(src.clone());
            show_image(ui, src, open, size, max);
        } else if e.is_rich() {
            embed_card(ui, e, cx);
        }
    }
}

const EMBED_MAX_W: f32 = 432.0;
const EMBED_THUMB: f32 = 80.0;

/// 押せる1行の文字（url があれば押すとブラウザで開く）
fn embed_line(ui: &mut egui::Ui, text: RichText, url: Option<&str>) {
    let Some(url) = url else {
        ui.add(egui::Label::new(text).wrap());
        return;
    };
    let r = ui.add(egui::Label::new(text).wrap().sense(Sense::click())).on_hover_cursor(egui::CursorIcon::PointingHand);
    if r.clicked() {
        ui.ctx().open_url(egui::OpenUrl::new_tab(url));
    }
}

/// 埋め込みの本文。URL・[文字](URL) の伏せリンク・**太字** だけ解釈する
fn embed_text(ui: &mut egui::Ui, text: &str, size: f32, color: Color32) {
    let plain = TextFormat { font_id: FontId::proportional(size), color, ..Default::default() };
    let bold = TextFormat { font_id: medium_font(ui.ctx(), size), color: WHITE(), ..Default::default() };
    let mut job = LayoutJob::default();
    let mut links = Vec::new();
    for (i, part) in text.split("**").enumerate() {
        let fmt = if i % 2 == 1 { &bold } else { &plain };
        append_masked(&mut job, part, fmt, &mut links);
    }
    link_label(ui, job, &links);
}

/// [文字](https://...) を文字だけ青くして links に足す。それ以外は append_linked に任せる
fn append_masked(job: &mut LayoutJob, text: &str, fmt: &TextFormat, links: &mut Vec<(std::ops::Range<usize>, String)>) {
    let mut rest = text;
    while let Some(open) = rest.find('[') {
        let Some(close) = rest[open..].find("](").map(|i| open + i) else { break };
        let after = &rest[close + 2..];
        let Some(end) = after.find(')') else { break };
        let url = after[..end].trim().trim_start_matches('<').trim_end_matches('>');
        let label = &rest[open + 1..close];
        if label.contains('[') || !(url.starts_with("https://") || url.starts_with("http://")) {
            append_linked(job, &rest[..open + 1], fmt, links);
            rest = &rest[open + 1..];
            continue;
        }
        append_linked(job, &rest[..open], fmt, links);
        let start = job.text.chars().count();
        job.append(label, 0.0, TextFormat { color: LINK(), ..fmt.clone() });
        links.push((start..start + label.chars().count(), url.to_string()));
        rest = &after[end + 1..];
    }
    append_linked(job, rest, fmt, links);
}

fn embed_icon(ui: &mut egui::Ui, icon: Option<&str>, proxy: Option<&str>, size: f32, cx: &mut MediaCx) {
    if let Some(u) = proxy.or(icon) {
        let src = u.to_string();
        cx.images.push(src.clone());
        ui.add(anim_image(ui.ctx(), src).fit_to_exact_size(vec2(size, size)).corner_radius(size / 2.0));
    }
}

/// 大きく出す画像（画像、なければ動画のサムネ）
fn embed_big_media(e: &Embed) -> Option<&model::EmbedMedia> {
    e.image.as_ref().or_else(|| if e.kind.as_deref() == Some("video") || e.video.is_some() { e.thumbnail.as_ref() } else { None })
}

/// タイトル・説明・フィールドなどを持つ埋め込み（リンクのプレビューや bot の埋め込み）
fn embed_card(ui: &mut egui::Ui, e: &Embed, cx: &mut MediaCx) {
    let width = ui.available_width().min(EMBED_MAX_W);
    let bar = e.color.filter(|&c| c != 0).map(rgb).unwrap_or(EMBED_BAR());
    let big = embed_big_media(e);
    let small_thumb = if big.is_none() { e.thumbnail.as_ref() } else { None };
    let r = Frame::new()
        .fill(EMBED_BG())
        .corner_radius(4)
        .inner_margin(Margin { left: 16, right: 16, top: 8, bottom: 12 })
        .show(ui, |ui| {
            let inner = width - 32.0;
            ui.set_width(inner);
            ui.horizontal_top(|ui| {
                let text_w = if small_thumb.is_some() { inner - EMBED_THUMB - 16.0 } else { inner };
                ui.vertical(|ui| {
                    ui.set_width(text_w);
                    ui.spacing_mut().item_spacing.y = 6.0;
                    ui.add_space(2.0);
                    embed_body(ui, e, cx);
                });
                if let Some(t) = small_thumb {
                    let max = vec2(EMBED_THUMB, EMBED_THUMB);
                    let size = fit_size(t.width, t.height, max);
                    let src = resized(t.proxy_url.as_deref(), &t.url, size.unwrap_or(max));
                    cx.images.push(src.clone());
                    ui.add_space(16.0);
                    ui.vertical(|ui| {
                        ui.add_space(4.0);
                        show_image(ui, src, e.url.as_deref().unwrap_or(&t.url), size, max);
                    });
                }
            });
            if let Some(md) = big {
                ui.add_space(8.0);
                let max = vec2(inner, 300.0);
                let size = fit_size(md.width, md.height, max);
                let src = resized(md.proxy_url.as_deref(), &md.url, size.unwrap_or(max));
                cx.images.push(src.clone());
                let open = e.url.as_deref().unwrap_or(&md.url);
                let r = show_image(ui, src, open, size, max);
                if e.video.is_some() || e.kind.as_deref() == Some("video") {
                    // 動画はアプリ内で再生できないので、再生マークを重ねてブラウザで開く
                    let c = r.rect.center();
                    let p = ui.painter();
                    p.circle_filled(c, 24.0, Color32::from_black_alpha(170));
                    let h = 10.0;
                    p.add(Shape::convex_polygon(
                        vec![pos2(c.x - h * 0.7, c.y - h), pos2(c.x - h * 0.7, c.y + h), pos2(c.x + h, c.y)],
                        WHITE(),
                        Stroke::NONE,
                    ));
                }
            }
            embed_footer(ui, e, cx);
        });
    let rect = r.response.rect;
    ui.painter().rect_filled(
        Rect::from_min_max(rect.left_top(), pos2(rect.left() + 4.0, rect.bottom())),
        CornerRadius { nw: 4, sw: 4, ne: 0, se: 0 },
        bar,
    );
    ui.add_space(4.0);
}

fn embed_body(ui: &mut egui::Ui, e: &Embed, cx: &mut MediaCx) {
    if let Some(name) = e.provider.as_ref().and_then(|p| p.name.as_deref()) {
        embed_line(ui, RichText::new(name).size(12.0).color(MUTED()), e.provider.as_ref().and_then(|p| p.url.as_deref()));
    }
    if let Some(a) = &e.author {
        if let Some(name) = a.name.as_deref() {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                embed_icon(ui, a.icon_url.as_deref(), a.proxy_icon_url.as_deref(), 24.0, cx);
                embed_line(ui, RichText::new(name).font(medium_font(ui.ctx(), 14.0)).color(WHITE()), a.url.as_deref());
            });
        }
    }
    if let Some(t) = e.title.as_deref() {
        let color = if e.url.is_some() { LINK() } else { WHITE() };
        embed_line(ui, RichText::new(t).font(medium_font(ui.ctx(), 16.0)).color(color), e.url.as_deref());
    }
    if let Some(d) = e.description.as_deref().filter(|d| !d.is_empty()) {
        embed_text(ui, d, 14.0, TEXT());
    }
    // フィールド: inline は最大3つまで横に並べる
    let mut i = 0;
    while i < e.fields.len() {
        let mut n = 1;
        if e.fields[i].inline {
            while n < 3 && i + n < e.fields.len() && e.fields[i + n].inline {
                n += 1;
            }
        }
        let row = &e.fields[i..i + n];
        ui.columns(n, |cols| {
            for (col, f) in cols.iter_mut().zip(row) {
                col.spacing_mut().item_spacing.y = 2.0;
                embed_text(col, &f.name, 14.0, WHITE());
                embed_text(col, &f.value, 14.0, TEXT());
            }
        });
        i += n;
    }
}

fn embed_footer(ui: &mut egui::Ui, e: &Embed, cx: &mut MediaCx) {
    let time = e.timestamp.as_deref().and_then(local_dt).map(|t| t.format("%Y/%m/%d %H:%M").to_string());
    let text = e.footer.as_ref().map(|f| f.text.as_str()).filter(|t| !t.is_empty());
    let line = match (text, &time) {
        (Some(t), Some(d)) => format!("{t} • {d}"),
        (Some(t), None) => t.to_string(),
        (None, Some(d)) => d.clone(),
        (None, None) => return,
    };
    ui.add_space(8.0);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        if let Some(f) = &e.footer {
            embed_icon(ui, f.icon_url.as_deref(), f.proxy_icon_url.as_deref(), 20.0, cx);
        }
        ui.add(egui::Label::new(RichText::new(line).size(12.0).color(MUTED())).wrap());
    });
}

enum Seg<'a> {
    Text(&'a str),
    Emoji { name: &'a str, id: &'a str, animated: bool },
}

/// 本文を文字列とカスタム絵文字 `<:名前:id>` / `<a:名前:id>` に分ける
fn split_emoji(s: &str) -> Vec<Seg<'_>> {
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

fn parse_emoji(t: &str) -> Option<(usize, Seg<'_>)> {
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

fn emoji_url(id: &str, animated: bool) -> String {
    // GIFは末尾が .gif でないとアニメ再生されないので、サイズ指定を付けない
    if animated {
        format!("https://cdn.discordapp.com/emojis/{id}.gif")
    } else {
        format!("https://cdn.discordapp.com/emojis/{id}.png?size=64")
    }
}

/// 本文（先頭に `prefix` を付ける）。カスタム絵文字があれば画像を文中に並べる
/// images には表示したカスタム絵文字の URI を足す（チャンネル破棄時に画像キャッシュを解放するため）
/// 本文中の http(s) URL の位置（バイト範囲）。末尾の句読点や対応しない ) は含めない
fn find_links(s: &str) -> Vec<std::ops::Range<usize>> {
    let mut out = Vec::new();
    let mut from = 0;
    while let Some(i) = s[from..].find("http").map(|i| from + i) {
        let t = &s[i..];
        let boundary = i == 0 || !s[..i].chars().next_back().is_some_and(|c| c.is_alphanumeric());
        if !boundary || !(t.starts_with("https://") || t.starts_with("http://")) {
            from = i + 4;
            continue;
        }
        let mut end = t.find(|c: char| c.is_whitespace() || matches!(c, '<' | '>' | '"' | '`')).unwrap_or(t.len());
        while let Some(c) = t[..end].chars().next_back() {
            let unbalanced = c == ')' && t[..end].matches('(').count() < t[..end].matches(')').count();
            if matches!(c, '.' | ',' | ':' | ';' | '!' | '?' | '\'' | '*' | '_' | '~' | '|') || unbalanced {
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

/// text を job に足す。URL は色を変え、その文字位置（job 全体の文字番号）と URL を links に入れる。
/// 埋め込みを抑止する <URL> の山かっこは本家と同じく表示しない
fn append_linked(job: &mut LayoutJob, text: &str, fmt: &TextFormat, links: &mut Vec<(std::ops::Range<usize>, String)>) {
    let mut last = 0;
    for r in find_links(text) {
        let wrapped = text[..r.start].ends_with('<') && text[r.end..].starts_with('>');
        let before = if wrapped { r.start - 1 } else { r.start };
        job.append(&text[last..before], 0.0, fmt.clone());
        let start = job.text.chars().count();
        let url = &text[r.clone()];
        job.append(url, 0.0, TextFormat { color: LINK(), ..fmt.clone() });
        links.push((start..start + url.chars().count(), url.to_string()));
        last = if wrapped { r.end + 1 } else { r.end };
    }
    job.append(&text[last..], 0.0, fmt.clone());
}

/// リンクを押せるラベル。リンクの上ではカーソルを指にして URL を出し、クリックでブラウザで開く
fn link_label(ui: &mut egui::Ui, job: LayoutJob, links: &[(std::ops::Range<usize>, String)]) {
    if links.is_empty() {
        ui.add(egui::Label::new(job).wrap());
        return;
    }
    let (pos, galley, resp) = egui::Label::new(job).wrap().sense(Sense::click()).layout_in_ui(ui);
    let hit = resp.hover_pos().map(|p| p - pos).filter(|p| galley.rows.iter().any(|r| r.rect.contains(p.to_pos2()))).and_then(|p| {
        let c = galley.cursor_from_pos(p).ccursor.index;
        links.iter().find(|(r, _)| r.start <= c && c < r.end.max(r.start + 1)).map(|(_, u)| u.as_str())
    });
    if ui.is_rect_visible(resp.rect) {
        if ui.style().interaction.selectable_labels {
            egui::text_selection::LabelSelectionState::label_text_selection(ui, &resp, pos, galley, TEXT(), Stroke::NONE);
        } else {
            ui.painter().galley(pos, galley, TEXT());
        }
    }
    // 選択用の I ビームより後に設定して上書きする
    if let Some(url) = hit {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        if resp.clicked() {
            ui.ctx().open_url(egui::OpenUrl::new_tab(url));
        }
        resp.on_hover_text_at_pointer(url);
    }
}

fn inline_content(ui: &mut egui::Ui, m: &Message, s: &Settings, prefix: LayoutJob, images: &mut Vec<String>) {
    let body = TextFormat { font_id: FontId::proportional(s.font_size), color: TEXT(), ..Default::default() };
    let small = TextFormat { font_id: FontId::proportional(11.0), color: MUTED(), ..Default::default() };
    let segs = split_emoji(&m.content);

    if !segs.iter().any(|x| matches!(x, Seg::Emoji { .. })) {
        let mut job = prefix;
        let mut links = Vec::new();
        append_linked(&mut job, &m.content, &body, &mut links);
        if m.edited_timestamp.is_some() {
            job.append(" (編集済)", 0.0, small);
        }
        link_label(ui, job, &links);
        return;
    }

    // 絵文字だけのメッセージは大きく表示
    let only_emoji = segs.iter().all(|x| match x {
        Seg::Text(t) => t.trim().is_empty(),
        Seg::Emoji { .. } => true,
    });
    let size = if only_emoji { 48.0 } else { (s.font_size * 1.4).round() };

    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = vec2(0.0, 2.0);
        if !prefix.is_empty() {
            ui.label(prefix);
        }
        for seg in segs {
            match seg {
                Seg::Text(t) => {
                    if !(only_emoji && t.trim().is_empty()) {
                        let (mut job, mut links) = (LayoutJob::default(), Vec::new());
                        append_linked(&mut job, t, &body, &mut links);
                        link_label(ui, job, &links);
                    }
                }
                Seg::Emoji { name, id, animated } => {
                    let url = emoji_url(id, animated);
                    images.push(url.clone());
                    ui.add(anim_image(ui.ctx(), url).fit_to_exact_size(vec2(size, size)))
                        .on_hover_text(format!(":{name}:"));
                    if only_emoji {
                        ui.add_space(4.0);
                    }
                }
            }
        }
        if m.edited_timestamp.is_some() {
            ui.label(RichText::new(" (編集済)").size(11.0).color(MUTED()));
        }
    });
}

const MEMBER_ROW_H: f32 = 42.0;
const MEMBER_HEAD_H: f32 = 40.0;

/// メンバーリストの1行。Member は members[gid] の添字
enum MemberRow {
    Header(String),
    Member(usize),
}

/// グループ分け・並べ替え済みのメンバーリスト（ver が変わるまで使い回す）
#[derive(Default)]
struct MemberListCache {
    gid: String,
    ver: u64,
    rows: Vec<MemberRow>,
    /// 各行の上端（仮想スクロール用の累積高さ）
    tops: Vec<f32>,
    height: f32,
}

impl MemberListCache {
    fn build(gid: &str, ver: u64, members: &[Member], roles: &[Role], presences: &HashMap<String, Presence>) -> Self {
        let mut hoisted: Vec<&Role> = roles.iter().filter(|r| r.hoist).collect();
        hoisted.sort_by_key(|r| std::cmp::Reverse(r.position));
        let online_index = hoisted.len();
        let mut groups: Vec<Vec<usize>> = vec![Vec::new(); online_index + 2];
        for (i, member) in members.iter().enumerate() {
            let Some(user) = &member.user else { continue };
            let online = presences.get(&user.id).is_some_and(|p| p.status != "offline");
            let index = if online {
                hoisted.iter().position(|r| member.roles.contains(&r.id)).unwrap_or(online_index)
            } else {
                online_index + 1
            };
            groups[index].push(i);
        }
        let names = hoisted.iter().map(|r| r.name.as_str()).chain(["オンライン", "オフライン"]);
        let mut out = MemberListCache { gid: gid.to_string(), ver, ..Default::default() };
        for (name, mut group) in names.zip(groups) {
            if group.is_empty() {
                continue;
            }
            group.sort_by_cached_key(|&i| members[i].display().to_lowercase());
            out.tops.push(out.height);
            out.height += MEMBER_HEAD_H;
            out.rows.push(MemberRow::Header(format!("{name} — {}", group.len())));
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

/// メッセージにマウスを乗せたときのツールバーの操作
enum MsgAction {
    Reply(Message),
    Edit(Message),
    /// true = Shift を押しながら（確認なしで削除）
    Delete(Message, bool),
    Copy(String),
}

fn author_name(m: &Message) -> &str {
    m.member.as_ref().and_then(|member| member.nick.as_deref()).unwrap_or_else(|| m.author.display())
}

fn is_reply(m: &Message) -> bool {
    m.kind == 19 || m.referenced_message.is_some()
}

/// 行が「マウスで選ばれている」か。前のフレームのツールバーの上にいる間は、その持ち主の行だけを選ばれている扱いにする
fn row_hovered(ui: &egui::Ui, rect: Rect, m: &Message, cx: &MediaCx) -> bool {
    if let (Some((id, tr)), Some(p)) = (&cx.tb_prev, ui.ctx().pointer_hover_pos()) {
        if tr.contains(p) && ui.clip_rect().contains(p) {
            return id == &m.id;
        }
    }
    ui.rect_contains_pointer(rect)
}

/// 行の背景（ホバー・返信先/編集中の強調）を塗る。返り値はホバー中か
fn row_background(ui: &mut egui::Ui, bg: egui::layers::ShapeIdx, rect: Rect, m: &Message, cx: &mut MediaCx) -> bool {
    let hovered = row_hovered(ui, rect, m, cx);
    if cx.highlight == Some(m.id.as_str()) {
        ui.painter().set(bg, Shape::rect_filled(rect, 0.0, Color32::from_rgba_unmultiplied(0x58, 0x65, 0xf2, 14)));
        ui.painter().rect_filled(Rect::from_min_size(rect.min, vec2(2.0, rect.height())), 0.0, BLURPLE());
    } else if hovered {
        ui.painter().set(bg, Shape::rect_filled(rect, 0.0, MSG_HOVER()));
    }
    if hovered {
        message_toolbar(ui, m, rect, cx);
    }
    hovered
}

/// 右上に出る操作ボタン（返信・編集・コピー・削除）
fn message_toolbar(ui: &mut egui::Ui, m: &Message, row: Rect, cx: &mut MediaCx) {
    let mine = cx.me == Some(m.author.id.as_str());
    let mut items: Vec<(Icon, &str)> = vec![(Icon::Reply, "返信")];
    if mine {
        items.push((Icon::Pencil, "編集"));
    }
    if !m.content.is_empty() {
        items.push((Icon::Copy, "テキストをコピー"));
    }
    if mine {
        items.push((Icon::Trash, "メッセージを削除（Shift+クリックで確認なし）"));
    }
    let bw = 32.0;
    let w = bw * items.len() as f32 + 4.0;
    let tr = Rect::from_min_size(pos2(row.right() - 16.0 - w, row.top() - 16.0), vec2(w, 32.0));
    // 一番上の行ではスクロール領域からはみ出さないよう下げる
    let tr = tr.translate(vec2(0.0, (ui.clip_rect().top() + 2.0 - tr.top()).max(0.0)));
    cx.tb_now = Some((m.id.clone(), tr));
    ui.painter().rect_filled(tr, 8.0, SIDEBAR());
    ui.painter().rect_stroke(tr, 8.0, Stroke::new(1.0_f32, BORDER()), egui::StrokeKind::Inside);
    for (i, (icon, tip)) in items.into_iter().enumerate() {
        let r = Rect::from_min_size(pos2(tr.left() + 2.0 + i as f32 * bw, tr.top()), vec2(bw, 32.0)).shrink(3.0);
        let resp = ui.interact(r, ui.id().with(("msg_tb", &m.id, i)), Sense::click());
        if resp.hovered() {
            ui.painter().rect_filled(r, 6.0, SELECTED());
        }
        let color = match (icon, resp.hovered()) {
            (Icon::Trash, true) => RED(),
            (_, true) => WHITE(),
            _ => MUTED(),
        };
        paint_glyph(ui.painter(), r.shrink(5.0), icon, color);
        if resp.on_hover_text(tip).clicked() {
            let shift = ui.input(|i| i.modifiers.shift);
            cx.action = Some(match icon {
                Icon::Reply => MsgAction::Reply(m.clone()),
                Icon::Pencil => MsgAction::Edit(m.clone()),
                Icon::Copy => MsgAction::Copy(m.content.clone()),
                _ => MsgAction::Delete(m.clone(), shift),
            });
        }
    }
}

/// 返信の上に出る「╭ @相手 本文」の一行。avatar_x = 本文側アバターの中心
fn reply_preview(ui: &mut egui::Ui, m: &Message, avatar_x: f32, text_x: f32, cx: &mut MediaCx) {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 20.0), Sense::hover());
    let p = ui.painter().with_clip_rect(rect.intersect(ui.clip_rect()));
    // 左の曲がった線
    let y = rect.center().y;
    let x0 = rect.left() + avatar_x;
    let x1 = rect.left() + text_x - 6.0;
    let mut pts = vec![pos2(x0, rect.bottom() + 2.0)];
    pts.extend(arc_points(pos2(x0 + 6.0, y + 6.0), 6.0, std::f32::consts::PI, std::f32::consts::PI * 1.5));
    pts.push(pos2(x1, y));
    p.add(Shape::line(pts, Stroke::new(2.0_f32, REPLY_LINE())));
    let Some(r) = m.referenced_message.as_deref() else {
        p.text(pos2(rect.left() + text_x, y), Align2::LEFT_CENTER, "元のメッセージは削除されました", FontId::proportional(13.0), FAINT());
        return;
    };
    let av = Rect::from_center_size(pos2(rect.left() + text_x + 8.0, y), vec2(16.0, 16.0));
    paint_icon(ui, av, Some(&user_avatar_url(&r.author)), r.author.display(), 8.0, name_color(&r.author.id));
    let name_color = r.member.as_ref().and_then(|member| member_role_color(member, cx.roles)).unwrap_or(TEXT());
    let name = p.layout_no_wrap(format!("@{}", author_name(r)), medium_font(ui.ctx(), 13.0), name_color);
    let nx = av.right() + 4.0;
    let nw = name.size().x;
    p.galley(pos2(nx, y - name.size().y / 2.0), name, name_color);
    let body = if r.content.is_empty() { "添付ファイルを見るにはクリック".to_string() } else { r.content.replace('\n', " ") };
    let max = (rect.right() - (nx + nw + 6.0) - 16.0).max(0.0);
    let mut job = LayoutJob::single_section(body, TextFormat { font_id: FontId::proportional(13.0), color: MUTED(), ..Default::default() });
    job.wrap = egui::text::TextWrapping::truncate_at_width(max);
    let g = ui.fonts(|f| f.layout_job(job));
    p.galley(pos2(nx + nw + 6.0, y - g.size().y / 2.0), g, MUTED());
}

fn message_row_compact(ui: &mut egui::Ui, m: &Message, cx: &mut MediaCx) {
    let s = cx.settings;
    ui.add_space(2.0);
    let bg = ui.painter().add(Shape::Noop);
    let top = ui.cursor().top();
    if is_reply(m) {
        reply_preview(ui, m, 30.0, 56.0, cx);
    }
    ui.horizontal(|ui| {
        ui.add_space(16.0);
        let mut job = LayoutJob::default();
        let time = local_dt(&m.timestamp).map(|t| t.format("%H:%M ").to_string()).unwrap_or_default();
        job.append(&time, 0.0, TextFormat { font_id: FontId::proportional(11.0), color: FAINT(), ..Default::default() });
        job.append(
            &format!("{}  ", author_name(m)),
            0.0,
            TextFormat { font_id: medium_font(ui.ctx(), s.font_size), color: m.member.as_ref().and_then(|member| member_role_color(member, cx.roles)).unwrap_or(WHITE()), ..Default::default() },
        );
        inline_content(ui, m, s, job, &mut cx.images);
    });
    if !m.attachments.is_empty() || !m.sticker_items.is_empty() || m.embeds.iter().any(|e| e.visible()) {
        ui.horizontal(|ui| {
            ui.add_space(56.0);
            ui.vertical(|ui| message_media(ui, m, cx));
        });
    }
    let rect = Rect::from_min_max(pos2(ui.max_rect().left(), top - 1.0), pos2(ui.max_rect().right(), ui.cursor().top() + 1.0));
    row_background(ui, bg, rect, m, cx);
}

fn message_row(ui: &mut egui::Ui, m: &Message, prev: Option<&Message>, cx: &mut MediaCx) {
    let s = cx.settings;
    let t = local_dt(&m.timestamp);
    let pt = prev.and_then(|p| local_dt(&p.timestamp));
    let new_day = match (t, pt) {
        (Some(a), Some(b)) => a.date_naive() != b.date_naive(),
        (Some(_), None) => true,
        _ => false,
    };
    if new_day {
        date_divider(ui, t.unwrap());
    }
    if s.compact {
        if new_day {
            ui.add_space(6.0);
        }
        return message_row_compact(ui, m, cx);
    }
    let reply = is_reply(m);
    let grouped = !new_day
        && !reply
        && prev.is_some_and(|p| p.author.id == m.author.id)
        && matches!((t, pt), (Some(a), Some(b)) if (a - b).num_minutes() < 7);

    ui.add_space(if grouped { 0.0 } else { 17.0 });
    let bg = ui.painter().add(Shape::Noop);
    let top = ui.cursor().top();
    let full = ui.available_width();
    if reply {
        reply_preview(ui, m, 36.0, 72.0, cx);
        ui.add_space(2.0);
    }

    ui.with_layout(Layout::left_to_right(Align::TOP), |ui| {
        ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
        ui.add_space(16.0);
        let sense = if grouped { Sense::hover() } else { Sense::click() };
        let (av, av_resp) = ui.allocate_exact_size(vec2(40.0, if grouped { 22.0 } else { 40.0 }), sense);
        if !grouped {
            let url = user_avatar_url(&m.author);
            paint_icon(ui, av, Some(&url), m.author.display(), 20.0, name_color(&m.author.id));
            if av_resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                cx.clicked_user = Some((m.author.clone(), av));
            }
        }
        ui.add_space(16.0);
        ui.vertical(|ui| {
            ui.set_max_width(full - 72.0 - 16.0);
            ui.spacing_mut().item_spacing.y = 2.0;
            if !grouped {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 8.0;
                    let name = ui
                        .add(egui::Label::new(RichText::new(author_name(m)).font(medium_font(ui.ctx(), 16.0)).color(m.member.as_ref().and_then(|member| member_role_color(member, cx.roles)).unwrap_or(WHITE()))).sense(Sense::click()))
                        .on_hover_cursor(egui::CursorIcon::PointingHand);
                    if name.clicked() {
                        cx.clicked_user = Some((m.author.clone(), name.rect));
                    }
                    ui.label(RichText::new(fmt_time(&m.timestamp)).size(12.0).color(FAINT()));
                });
            }
            if !m.content.is_empty() {
                inline_content(ui, m, s, LayoutJob::default(), &mut cx.images);
            }
            message_media(ui, m, cx);
        });
    });

    let rect = Rect::from_min_max(
        pos2(ui.max_rect().left(), top - if grouped { 1.0 } else { 4.0 }),
        pos2(ui.max_rect().right(), ui.cursor().top() + 2.0),
    );
    if row_background(ui, bg, rect, m, cx) && grouped {
        if let Some(t) = t {
            ui.painter().text(
                pos2(rect.left() + 36.0, top + 11.0),
                Align2::CENTER_CENTER,
                t.format("%H:%M").to_string(),
                FontId::proportional(11.0),
                FAINT(),
            );
        }
    }
}

fn rgb(c: u32) -> Color32 {
    Color32::from_rgb((c >> 16) as u8, (c >> 8) as u8, c as u8)
}

fn darken(c: Color32) -> Color32 {
    Color32::from_rgb((c.r() as f32 * 0.55) as u8, (c.g() as f32 * 0.55) as u8, (c.b() as f32 * 0.55) as u8)
}

/// ユーザーIDからアカウント作成日を出す
fn snowflake_date(id: &str) -> String {
    let ms = id.parse::<u64>().map(|i| (i >> 22) + 1_420_070_400_000).unwrap_or(0);
    chrono::DateTime::from_timestamp_millis(ms as i64)
        .map(|t| t.with_timezone(&Local).format("%Y年%-m月%-d日").to_string())
        .unwrap_or_default()
}

enum ProfileState {
    Loading,
    Ready(ProfileData),
    Failed,
}

#[derive(Clone)]
struct ProfilePopup {
    user: User,
    guild_id: Option<String>,
    anchor: Rect,
    /// 左下の自分のアバターから開いたときは上に出す
    above: bool,
    opened: u64,
}

/// 1チャンネル分のメッセージ行の高さ。幅・文字サイズ・表示形式が変わったら作り直す
#[derive(Default)]
struct RowHeights {
    key: (u32, u32, bool),
    h: HashMap<String, f32>,
}

/// 既読を送る最短の間隔。新着が続いても、この間隔で最新のものだけ送る
const ACK_INTERVAL: std::time::Duration = std::time::Duration::from_millis(1500);

/// 描き続けるときの1秒あたりの最大回数（垂直同期の代わり）
const MAX_FPS: f64 = 120.0;

/// 過去ログの読み足し
#[derive(Default)]
struct History {
    /// 読み込み中の起点（このIDより前を頼んでいる）
    pending: Option<String>,
    /// チャンネルの始まりまで読んだ
    end: bool,
    /// 失敗したらしばらく待ってから取り直す
    retry_at: Option<std::time::Instant>,
}

/// 上端からこの距離まで近づいたら古いメッセージを読み足す
const HISTORY_TRIGGER: f32 = 800.0;
/// 一覧の先頭に置く、読み込み中の表示・チャンネルの始まりの表示の高さ（どちらでも同じにして行がずれないようにする）
const HISTORY_HEAD_H: f32 = 72.0;
/// 表示中のチャンネルで遡って持てるメッセージ数の上限
const MAX_HISTORY: usize = 5000;

/// 高さをまだ測っていない行の仮の高さ
const EST_ROW_H: f32 = 48.0;
/// 見えている範囲の上下にこれだけ余分に描く
const VIEW_MARGIN: f32 = 300.0;
/// 1チャンネルで保持するメッセージ数の上限
const MAX_MSGS: usize = 1000;
/// メッセージを保持しておくチャンネル数の上限
const MAX_CHANNELS: usize = 30;
/// 受信中のメンバー索引は、更新が止まってから短時間で破棄する。
const MEMBER_INDEX_TTL: std::time::Duration = std::time::Duration::from_secs(5);

struct RecentMemberIndex {
    guild_id: String,
    index: HashMap<String, usize>,
    updated: std::time::Instant,
}

// ---- アプリ本体 ----
struct Backend {
    cmd: UnboundedSender<Cmd>,
    ev: Receiver<Event>,
}

#[derive(Default)]
struct App {
    ctx: egui::Context,
    backend: Option<Backend>,

    token_input: String,
    remember: bool,
    login_error: Option<String>,

    me: Option<User>,
    guilds: Vec<Guild>,
    dms: Vec<Channel>,
    channels: HashMap<String, Vec<Channel>>,
    roles: HashMap<String, Vec<Role>>,
    members: HashMap<String, Vec<Member>>,
    member_index: Option<RecentMemberIndex>,
    members_requested: HashSet<String>,
    show_members: bool,
    messages: HashMap<String, Vec<Message>>,
    row_heights: HashMap<String, RowHeights>,
    /// チャンネルごとの過去ログ読み足しの状態
    history: HashMap<String, History>,
    /// 見る権限がなかったチャンネル（開くと 403）
    no_access: HashSet<String>,
    /// 先頭に読み足した行の仮の高さの合計。次の描画でスクロール位置をその分下げて、見ている行を動かさない
    prepended: HashMap<String, f32>,
    /// チャンネルごとに表示した添付・埋め込み画像の URI。チャンネルを捨てるときに画像キャッシュも解放する
    channel_images: HashMap<String, HashSet<String>>,
    /// 未読チャンネル → 所属サーバー（DMならNone）
    unread: HashMap<String, Option<String>>,

    last_title: String,
    /// DM 一覧の絞り込み
    dm_search: bool,
    dm_filter: String,
    /// 戻る/進むの履歴（サーバー, チャンネル）
    nav_back: Vec<(Option<String>, Option<String>)>,
    nav_fwd: Vec<(Option<String>, Option<String>)>,
    /// 返信先・編集中のメッセージ
    reply_to: Option<Message>,
    editing: Option<Message>,
    pending_delete: Option<Message>,
    /// 前のフレームでメッセージのツールバーを出した行と位置
    toolbar: Option<(String, Rect)>,
    /// Ctrl+K（検索文字, 選択中の行）
    switcher: Option<(String, usize)>,
    /// メッセージを読み込んだチャンネル（古い順）。上限を超えたら古いものから捨てる
    loaded_channels: std::collections::VecDeque<String>,
    /// DM のプロフィール欄を閉じたか
    hide_dm_profile: bool,
    /// メンバー・ロール・オンライン状態が変わるたびに増える（メンバーリストの作り直しの合図）
    member_ver: u64,
    member_cache: MemberListCache,
    sel_guild: Option<String>, // None = DM
    sel_channel: Option<String>,
    input: String,
    /// 送信待ちの添付と、それを貼り付けたチャンネル（別のチャンネルに移ったら捨てる）
    uploads: Vec<clip::Upload>,
    /// チャンネルごとに、最後に既読として Discord に送ったメッセージID
    acked: HashMap<String, String>,
    /// 最後に既読を送った時刻（送りすぎて制限されないよう間を空ける）
    last_ack: Option<std::time::Instant>,
    /// 今使っている見た目（テーマ＋上書き）。変わったときだけ適用し直す
    theme_applied: Option<theme::Resolved>,
    theme_error: Option<String>,
    /// themes/ のファイルが変わったら届く（保存するとすぐ反映）
    theme_watch: Option<Receiver<()>>,
    /// 前のフレームを描き始めた時刻（フレームレートを抑えるため）
    last_frame: Option<std::time::Instant>,
    /// 開いているファイル選択ダイアログの結果の受け口
    file_pick: Option<Receiver<Vec<PathBuf>>>,
    /// 背景画像を選ぶダイアログの結果の受け口
    bg_pick: Option<Receiver<Vec<PathBuf>>>,
    upload_channel: Option<String>,
    /// V キーが普通に押されているか（Ctrl+V の押下は egui に届かないので、離したときに判定する）
    v_down: bool,
    status: String,

    settings: Settings,
    stickers: HashMap<String, StickerState>,
    presences: HashMap<String, Presence>,
    profiles: HashMap<String, ProfileState>,
    popup: Option<ProfilePopup>,
    settings_open: bool,
    settings_tab: usize,

    // ---- 音声 ----
    voice: Arc<voice::VoiceCtl>,
    /// user_id → どのボイスチャンネルにいるか
    voice_states: HashMap<String, VoiceState>,
    voice_index: HashMap<String, Vec<String>>,
    /// 自分の接続状態（None = 未接続）
    voice_status: Option<VoiceStatus>,
    /// DM通話: channel_id → 呼び出し中のユーザー
    calls: HashMap<String, Vec<String>>,
    mic_test: Option<voice::MicTest>,
    audio_devs: Option<(Vec<String>, Vec<String>)>,
    binding_ptt: bool,
}

impl App {
    fn new(ctx: egui::Context) -> Self {
        let settings = load_settings();
        ctx.set_zoom_factor(settings.zoom);
        let mut app = App { ctx, remember: true, settings, show_members: true, ..Default::default() };
        app.theme_watch = Some(watch_themes(app.ctx.clone()));
        app.apply_theme(false);
        if let Some(t) = load_token() {
            app.start(t);
        }
        app
    }

    fn reset(&mut self) {
        let ctx = self.ctx.clone();
        let settings = std::mem::take(&mut self.settings);
        let (applied, watch) = (self.theme_applied.take(), self.theme_watch.take());
        *self = App { ctx, remember: true, settings, show_members: true, theme_applied: applied, theme_watch: watch, ..Default::default() };
    }

    fn start(&mut self, token: String) {
        self.voice.set_ctx(self.ctx.clone());
        self.apply_voice_settings();
        let (cmd, ev) = net::spawn(token, self.settings.status.clone(), self.ctx.clone(), self.voice.clone());
        self.backend = Some(Backend { cmd, ev });
        self.status = "接続中…".into();
    }

    fn send(&self, c: Cmd) {
        if let Some(b) = &self.backend {
            let _ = b.cmd.send(c);
        }
    }

    fn logout(&mut self) {
        self.send(Cmd::LeaveVoice);
        let _ = std::fs::remove_file(token_path());
        self.reset();
    }

    fn poll(&mut self) {
        if let Some(cache) = &self.member_index {
            let elapsed = cache.updated.elapsed();
            if elapsed >= MEMBER_INDEX_TTL {
                self.member_index = None;
            } else {
                self.ctx.request_repaint_after(MEMBER_INDEX_TTL - elapsed);
            }
        }
        let Some(b) = &self.backend else { return };
        let events: Vec<Event> = b.ev.try_iter().collect();
        let mut voice_index_dirty = false;
        let mut sticker_cache_dirty = false;
        let mut member_indexes: HashMap<String, HashMap<String, usize>> = HashMap::new();
        let mut last_member_guild = None;
        if events.iter().any(|event| matches!(event, Event::GuildMembers(..))) {
            if let Some(cache) = self.member_index.take() {
                member_indexes.insert(cache.guild_id, cache.index);
            }
        }
        for e in events {
            match e {
                Event::Me(u) => {
                    if self.remember && !self.token_input.is_empty() {
                        if let Err(e) = save_token(self.token_input.trim()) {
                            self.status = format!("トークン保存失敗: {e}");
                        }
                    }
                    self.token_input.clear();
                    self.me = Some(u);
                }
                Event::AuthFailed => {
                    self.reset();
                    self.login_error = Some("トークンが無効です".into());
                    return;
                }
                Event::Guilds(g) => self.guilds = g,
                Event::Dms(mut d) => {
                    d.sort_by_cached_key(|c| std::cmp::Reverse(c.last_id()));
                    self.dms = d;
                }
                Event::GuildChannels(gid, mut c) => {
                    // 表示順（テキスト→ボイス、position順）に一度だけ並べておく
                    c.sort_by_key(|c| (c.is_voice(), c.position));
                    self.channels.insert(gid, c);
                }
                Event::GuildRoles(gid, roles) => {
                    self.roles.insert(gid, roles);
                    self.member_ver += 1;
                }
                Event::GuildMembers(gid, members) => {
                    last_member_guild = Some(gid.clone());
                    let list = self.members.entry(gid.clone()).or_default();
                    // 同じ描画前に複数の塊が届いた場合、索引を共有する
                    let index = member_indexes.entry(gid).or_insert_with(||
                        list.iter().enumerate().filter_map(|(i, m)| m.user.as_ref().map(|u| (u.id.clone(), i))).collect());
                    for member in members {
                        let Some(id) = member.user.as_ref().map(|u| u.id.clone()) else { continue };
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
                Event::Messages(cid, m) => {
                    // 応答待ちの間に破棄されたチャンネルは復活させない
                    if !self.loaded_channels.iter().any(|id| id == &cid) {
                        continue;
                    }
                    if let Some(cache) = self.row_heights.get_mut(&cid) {
                        let ids: HashSet<&str> = m.iter().map(|message| message.id.as_str()).collect();
                        cache.h.retain(|id, _| ids.contains(id.as_str()));
                    }
                    self.history.insert(cid.clone(), History { end: m.len() < net::HISTORY_PAGE, ..Default::default() });
                    self.no_access.remove(&cid);
                    self.prepended.remove(&cid);
                    if let Some(old) = self.messages.insert(cid, m) {
                        sticker_cache_dirty |= old.iter().any(|m| !m.sticker_items.is_empty());
                    }
                }
                Event::NoAccess(cid) => {
                    if !self.loaded_channels.iter().any(|id| id == &cid) {
                        continue;
                    }
                    self.history.insert(cid.clone(), History { end: true, ..Default::default() });
                    self.no_access.insert(cid.clone());
                    self.messages.insert(cid, Vec::new());
                }
                Event::OlderMessages { channel_id, before, messages } => {
                    let Some(h) = self.history.get_mut(&channel_id) else { continue };
                    // 一覧を読み直した後などに届いた、今は待っていない応答は捨てる
                    if h.pending.as_deref() != Some(before.as_str()) {
                        continue;
                    }
                    h.pending = None;
                    let Some(older) = messages else {
                        h.retry_at = Some(std::time::Instant::now() + std::time::Duration::from_secs(5));
                        continue;
                    };
                    h.end = older.len() < net::HISTORY_PAGE;
                    // 待っている間に一覧が入れ替わっていたら捨てる（次のスクロールで取り直す）
                    let Some(list) = self.messages.get_mut(&channel_id) else { continue };
                    if list.first().map(|m| m.id.as_str()) != Some(before.as_str()) {
                        h.end = false;
                        continue;
                    }
                    let have: HashSet<&str> = list.iter().map(|m| m.id.as_str()).collect();
                    let older: Vec<Message> = older.into_iter().filter(|m| !have.contains(m.id.as_str())).collect();
                    let known = self.row_heights.get(&channel_id);
                    let added: f32 = older.iter().map(|m| known.and_then(|c| c.h.get(&m.id)).copied().unwrap_or(EST_ROW_H)).sum();
                    *self.prepended.entry(channel_id.clone()).or_default() += added;
                    list.splice(0..0, older);
                }
                Event::MessageCreate(m) => {
                    let mine = self.me.as_ref().is_some_and(|u| u.id == m.author.id);
                    if !mine && self.sel_channel.as_deref() != Some(&m.channel_id) {
                        self.unread.insert(m.channel_id.clone(), m.guild_id.clone());
                    }
                    // 新着の DM は必ず最新なので、全体を並べ直さず先頭へ移すだけでよい
                    if let Some(i) = self.dms.iter().position(|c| c.id == m.channel_id) {
                        let mut dm = self.dms.remove(i);
                        dm.last_message_id = Some(m.id.clone());
                        self.dms.insert(0, dm);
                    }
                    if let Some(list) = self.messages.get_mut(&m.channel_id) {
                        // 重複は自分の送信の折り返しなので末尾側から探す
                        if !list.iter().rev().any(|x| x.id == m.id) {
                            let viewing = self.sel_channel.as_deref() == Some(m.channel_id.as_str());
                            list.push(m);
                            // 表示中のチャンネルは遡って読んだ分を消さない（読んでいる行が消えるため）
                            let cap = if viewing { MAX_HISTORY } else { MAX_MSGS };
                            if list.len() > cap {
                                let cut = list.len() - cap;
                                if let Some(h) = self.history.get_mut(&list[0].channel_id) {
                                    h.end = false;
                                }
                                sticker_cache_dirty |= list[..cut].iter().any(|m| !m.sticker_items.is_empty());
                                if let Some(c) = self.row_heights.get_mut(&list[0].channel_id) {
                                    for old in &list[..cut] {
                                        c.h.remove(&old.id);
                                    }
                                }
                                list.drain(..cut);
                            }
                        }
                    }
                }
                Event::MessageUpdate { channel_id, id, content, embeds } => {
                    if let Some(m) = self.messages.get_mut(&channel_id).and_then(|l| l.iter_mut().find(|m| m.id == id)) {
                        if let Some(c) = content {
                            if c != m.content {
                                m.content = c;
                                m.edited_timestamp = Some(String::new());
                            }
                        }
                        if let Some(e) = embeds {
                            m.embeds = e;
                        }
                    }
                }
                Event::MessageDelete { channel_id, id } => {
                    if let Some(l) = self.messages.get_mut(&channel_id) {
                        sticker_cache_dirty |= l.iter().any(|m| m.id == id && !m.sticker_items.is_empty());
                        l.retain(|m| m.id != id);
                    }
                    if let Some(cache) = self.row_heights.get_mut(&channel_id) {
                        cache.h.remove(&id);
                    }
                }
                Event::Status(s) => self.status = s,
                Event::Error(s) => self.status = format!("エラー: {s}"),
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
                        let previous = self.voice_states.get(&v.user_id).and_then(|old| old.channel_id.as_deref());
                        voice_index_dirty |= previous != v.channel_id.as_deref();
                        if v.channel_id.is_some() {
                            self.voice_states.insert(v.user_id.clone(), v);
                        } else {
                            self.voice_states.remove(&v.user_id);
                        }
                    }
                }
                Event::Voice(st) => match st {
                    VoiceStatus::Disconnected => self.voice_status = None,
                    VoiceStatus::Error(e) => {
                        self.voice_status = None;
                        self.status = format!("エラー: ボイス接続失敗: {e}");
                    }
                    st => {
                        if let (VoiceStatus::Connected { channel_id, .. }, Some(me)) = (&st, &self.me) {
                            if let Some(r) = self.calls.get_mut(channel_id) {
                                r.retain(|u| u != &me.id);
                            }
                        }
                        self.voice_status = Some(st);
                    }
                },
                Event::Call { channel_id, ringing } => {
                    self.calls.insert(channel_id, ringing);
                }
                Event::CallEnded(cid) => {
                    self.calls.remove(&cid);
                    self.voice_states.retain(|_, v| v.channel_id.as_deref() != Some(&cid) || v.guild_id.is_some());
                    voice_index_dirty = true;
                }
                Event::Presences(list) => {
                    // 並びが変わるのはオンライン/オフラインが入れ替わったときだけ
                    let flips = list.iter().any(|(id, p)| {
                        let was = self.presences.get(id).is_some_and(|o| o.status != "offline");
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
                            let frames: Vec<(egui::TextureHandle, u32)> = f
                                .frames
                                .into_iter()
                                .enumerate()
                                .map(|(i, (rgba, ms))| {
                                    let img = egui::ColorImage::from_rgba_unmultiplied(f.size, &rgba);
                                    (self.ctx.load_texture(format!("sticker-{id}-{i}"), img, Default::default()), ms)
                                })
                                .collect();
                            let total = frames.iter().map(|(_, ms)| *ms).sum();
                            StickerState::Ready { frames, total }
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
                self.member_index = Some(RecentMemberIndex { guild_id, index, updated: std::time::Instant::now() });
                self.ctx.request_repaint_after(MEMBER_INDEX_TTL);
            }
        }
        if sticker_cache_dirty {
            self.prune_stickers();
        }
    }

    /// 入力欄に貼り付け・ドロップされた画像とファイルを添付に加える
    fn take_pasted(&mut self, ctx: &egui::Context) {
        let mut paths = Vec::new();
        let mut paste_key = false;
        ctx.input_mut(|i| {
            for e in &i.events {
                match e {
                    egui::Event::Key { key: egui::Key::V, pressed: true, .. } => self.v_down = true,
                    egui::Event::Key { key: egui::Key::V, pressed: false, modifiers, .. } => {
                        paste_key |= !self.v_down || modifiers.command;
                        self.v_down = false;
                    }
                    egui::Event::Key { key: egui::Key::Paste, pressed: false, .. } => paste_key = true,
                    _ => {}
                }
            }
            // コピーしたファイルは文字（パス）として届くので、入力欄に入る前に添付へ回す
            i.events.retain(|e| match e {
                egui::Event::Paste(t) => match clip::pasted_paths(t) {
                    Some(p) => {
                        paths.extend(p);
                        false
                    }
                    None => true,
                },
                _ => true,
            });
            paths.extend(i.raw.dropped_files.drain(..).filter_map(|f| f.path));
        });
        if paste_key {
            match clip::clipboard_image(ctx) {
                Some(Ok(u)) => self.uploads.push(u),
                Some(Err(e)) => self.status = format!("エラー: {e}"),
                None => {}
            }
        }
        // ファイル選択ダイアログで選ばれたもの
        if let Some(rx) = &self.file_pick {
            match rx.try_recv() {
                Ok(picked) => {
                    paths.extend(picked);
                    self.file_pick = None;
                }
                Err(std::sync::mpsc::TryRecvError::Disconnected) => self.file_pick = None,
                Err(std::sync::mpsc::TryRecvError::Empty) => {}
            }
        }
        for p in paths {
            match clip::load_file(ctx, &p) {
                Ok(u) => self.uploads.push(u),
                Err(e) => self.status = format!("エラー: {e}"),
            }
        }
        // Discord は1メッセージ10ファイルまで
        self.uploads.truncate(10);
    }

    /// OS のファイル選択ダイアログを別スレッドで開く（開いている間も画面は動かす）。選ばれたら take_pasted で添付に足す
    fn pick_files(&mut self, ctx: &egui::Context) {
        if self.file_pick.is_some() {
            return;
        }
        self.file_pick = Some(spawn_file_dialog(ctx, "ファイルをアップロード", true, None));
    }

    /// 設定のテーマを読み直し、変わっていれば配色・フォント・背景画像を適用する。force = ファイルが変わった
    fn apply_theme(&mut self, force: bool) {
        let dir = themes_dir();
        let (r, err) = theme::resolve(&self.settings.theme, &dir, &self.settings.color_overrides, &self.settings.background);
        self.theme_error = err;
        let old = self.theme_applied.as_ref();
        if !force && old == Some(&r) {
            return;
        }
        let font_changed = old.map(|o| &o.font) != Some(&r.font);
        theme::set_colors(&r.colors);
        setup_theme(&self.ctx);
        if font_changed && (r.font.is_some() || old.is_some()) {
            setup_fonts(&self.ctx, r.font.as_deref());
        }
        if let Some((uri, _)) = &r.background {
            if let Some(path) = uri.strip_prefix("file://") {
                // ファイルの背景は毎回読み直す（同じ名前のまま描き変えても反映させる）
                self.ctx.forget_image(uri);
                match std::fs::read(path) {
                    Ok(b) => self.ctx.include_bytes(uri.clone(), b),
                    Err(e) => self.theme_error = Some(format!("背景画像 {path}: {e}")),
                }
            }
        }
        self.theme_applied = Some(r);
        self.ctx.request_repaint();
    }

    /// 表示中のチャンネルの最新メッセージまでを Discord に既読として送る。
    /// ウィンドウにフォーカスがある（＝実際に見ている）ときだけ。新着が続くときは ACK_INTERVAL ごとにまとめる
    fn sync_read_state(&mut self, focused: bool) {
        if !focused {
            return;
        }
        let Some(cid) = self.sel_channel.clone() else { return };
        let Some(last) = self.messages.get(&cid).and_then(|l| l.last()) else { return };
        if self.acked.get(&cid) == Some(&last.id) {
            return;
        }
        if let Some(wait) = self.last_ack.and_then(|t| ACK_INTERVAL.checked_sub(t.elapsed())) {
            self.ctx.request_repaint_after(wait);
            return;
        }
        let message_id = last.id.clone();
        self.acked.insert(cid.clone(), message_id.clone());
        self.last_ack = Some(std::time::Instant::now());
        self.unread.remove(&cid);
        self.send(Cmd::Ack { channel_id: cid, message_id });
    }

    fn open_channel(&mut self, id: String) {
        if self.sel_channel.as_deref() != Some(&id) {
            if self.editing.take().is_some() {
                self.input.clear();
            }
            self.reply_to = None;
            self.toolbar = None;
        }
        self.unread.remove(&id);
        if !self.messages.contains_key(&id) {
            self.send(Cmd::LoadMessages(id.clone()));
        }
        self.touch_channel(&id);
        self.sel_channel = Some(id);
    }

    /// 開いたチャンネルを「最近」に回し、MAX_CHANNELS を超えた古いチャンネルのメッセージと行の高さを捨てる。
    /// 1チャンネル最大 MAX_MSGS 件なので、巡回するほどメモリが増え続けるのを防ぐ
    fn touch_channel(&mut self, id: &str) {
        if let Some(i) = self.loaded_channels.iter().position(|c| c == id) {
            self.loaded_channels.remove(i);
        }
        self.loaded_channels.push_back(id.to_string());
        let mut evicted = false;
        while self.loaded_channels.len() > MAX_CHANNELS {
            let Some(old) = self.loaded_channels.pop_front() else { break };
            self.messages.remove(&old);
            self.row_heights.remove(&old);
            self.history.remove(&old);
            self.prepended.remove(&old);
            self.no_access.remove(&old);
            self.acked.remove(&old);
            evicted = true;
            // そのチャンネルでしか使っていない画像は、デコード済みの画像・テクスチャごと解放する
            if let Some(uris) = self.channel_images.remove(&old) {
                for uri in uris {
                    if !self.channel_images.values().any(|s| s.contains(&uri)) {
                        // egui は GIF / WebP の各フレームを URI#番号 で別々に保持する
                        let frame_count = self.ctx.data(|data| data.get_temp::<egui::FrameDurations>(egui::Id::new(&uri)))
                            .map_or(0, |durations| durations.all().count());
                        for frame in 0..frame_count {
                            self.ctx.forget_image(&format!("{uri}#{frame}"));
                        }
                        self.ctx.forget_image(&uri);
                        self.ctx.data_mut(|data| data.remove::<egui::FrameDurations>(egui::Id::new(&uri)));
                    }
                }
            }
        }
        if evicted {
            self.prune_stickers();
        }
    }

    fn prune_stickers(&mut self) {
        let used: HashSet<&str> = self.messages.values().flatten()
            .flat_map(|m| m.sticker_items.iter().map(|s| s.id.as_str())).collect();
        self.stickers.retain(|id, _| used.contains(id.as_str()));
    }

    fn current_channel(&self) -> Option<&Channel> {
        let id = self.sel_channel.as_deref()?;
        match &self.sel_guild {
            None => self.dms.iter().find(|c| c.id == id),
            Some(g) => self.channels.get(g)?.iter().find(|c| c.id == id),
        }
    }

    /// 未読チャンネルを持つサーバー（DM は None）。サーバーごとに未読全体をなめないよう1回で集める
    fn unread_guilds(&self) -> HashSet<Option<&str>> {
        self.unread.values().map(|g| g.as_deref()).collect()
    }

    // ---- ログイン画面 ----
    fn login_ui(&mut self, ctx: &egui::Context) {
        egui::CentralPanel::default().frame(Frame::new().fill(RAIL())).show(ctx, |ui| {
            ui.vertical_centered(|ui| {
                ui.add_space((ui.available_height() / 2.0 - 160.0).max(24.0));
                Frame::new().fill(CHAT()).corner_radius(8).inner_margin(Margin::same(32)).show(ui, |ui| {
                    ui.set_width(480.0);
                    ui.vertical_centered(|ui| {
                        ui.label(RichText::new("Riscord にログイン").size(24.0).color(WHITE()));
                        ui.add_space(6.0);
                        ui.label(RichText::new("Discordのトークンを入力してください").color(MUTED()));
                    });
                    ui.add_space(20.0);
                    if self.backend.is_some() {
                        ui.vertical_centered(|ui| {
                            ui.spinner();
                            ui.label(RichText::new(&self.status).color(MUTED()));
                        });
                        return;
                    }
                    ui.label(RichText::new("トークン").size(12.0).color(MUTED()));
                    let r = Frame::new().fill(RAIL()).show(ui, |ui| {
                        ui.add(
                            egui::TextEdit::singleline(&mut self.token_input)
                                .frame(false)
                                .password(true)
                                .desired_width(f32::INFINITY)
                                .margin(Margin::symmetric(10, 10)),
                        )
                    }).inner;
                    ui.add_space(8.0);
                    ui.checkbox(&mut self.remember, RichText::new("このPCに保存する").color(MUTED()));
                    ui.add_space(16.0);
                    let enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                    let btn = egui::Button::new(RichText::new("ログイン").size(15.0).color(Color32::WHITE))
                        .fill(BLURPLE())
                        .min_size(vec2(ui.available_width(), 42.0));
                    if (ui.add(btn).clicked() || enter) && !self.token_input.trim().is_empty() {
                        self.login_error = None;
                        self.start(self.token_input.trim().to_string());
                    }
                    if let Some(e) = &self.login_error {
                        ui.add_space(10.0);
                        ui.colored_label(RED(), e);
                    }
                });
            });
        });
    }

    // ---- メイン画面 ----
    fn main_ui(&mut self, ctx: &egui::Context) {
        let mut open: Option<String> = None;
        let mut pick_guild: Option<Option<String>> = None;
        let mut open_settings = false;
        let mut my_profile: Option<(User, Rect)> = None;
        let mut reload = false;
        let mut to_send: Option<String> = None;
        let mut plus_action: Option<PlusAction> = None;
        let mut act_mute = false;
        let mut act_deaf = false;
        let mut act_leave = false;
        let mut act_join: Option<(Option<String>, String, bool)> = None;
        let mut act_decline: Option<String> = None;
        let mut clicked_user: Option<(User, Rect)> = None;
        let mut goto_voice: Option<(Option<String>, String)> = None;

        let before = (self.sel_guild.clone(), self.sel_channel.clone());
        // 戻る/進む: Some(true) = 戻る
        let mut nav: Option<bool> = ctx.input(|i| {
            if i.pointer.button_pressed(egui::PointerButton::Extra1) || (i.modifiers.alt && i.key_pressed(egui::Key::ArrowLeft)) {
                Some(true)
            } else if i.pointer.button_pressed(egui::PointerButton::Extra2) || (i.modifiers.alt && i.key_pressed(egui::Key::ArrowRight)) {
                Some(false)
            } else {
                None
            }
        });

        let mut edit_last = false;
        let mut cancel_compose = false;
        if ctx.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::K)) {
            self.switcher = if self.switcher.is_some() { None } else { Some((String::new(), 0)) };
        }
        // Alt+↑↓ でサイドバーの前後のチャンネルへ
        let alt_move = ctx.input(|i| match (i.modifiers.alt, i.key_pressed(egui::Key::ArrowUp), i.key_pressed(egui::Key::ArrowDown)) {
            (true, true, _) => -1,
            (true, _, true) => 1,
            _ => 0,
        });
        if alt_move != 0 && self.switcher.is_none() {
            let order = self.sidebar_order();
            if !order.is_empty() {
                let i = self.sel_channel.as_ref().and_then(|c| order.iter().position(|x| x == c));
                let n = match i {
                    Some(i) => (i as i32 + alt_move).clamp(0, order.len() as i32 - 1) as usize,
                    None => 0,
                };
                open = Some(order[n].clone());
            }
        }
        // どこにもフォーカスが無いときに文字を打ったら入力欄へ（そのフレームの入力ごと渡る）
        if self.sel_channel.is_some()
            && self.switcher.is_none()
            && self.pending_delete.is_none()
            && ctx.memory(|m| m.focused().is_none())
            && ctx.input(|i| !i.modifiers.command && i.events.iter().any(|e| matches!(e, egui::Event::Text(_))))
        {
            ctx.memory_mut(|m| m.request_focus(egui::Id::new(INPUT_ID)));
        }

        // タイトルバー（最上段に戻る/進むと現在地）
        egui::TopBottomPanel::top("titlebar")
            .exact_height(36.0)
            .show_separator_line(false)
            .frame(Frame::new().fill(BG()).inner_margin(Margin::symmetric(10, 0)))
            .show(ctx, |ui| {
                let full = ui.max_rect();
                let drag = ui.interact(full, ui.id().with("drag"), Sense::click_and_drag());
                if drag.drag_started() {
                    ui.ctx().send_viewport_cmd(egui::ViewportCommand::StartDrag);
                }
                if drag.double_clicked() {
                    let max = ui.ctx().input(|i| i.viewport().maximized.unwrap_or(false));
                    ui.ctx().send_viewport_cmd(egui::ViewportCommand::Maximized(!max));
                }
                ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                    ui.spacing_mut().item_spacing.x = 4.0;
                    for (back, icon, tip) in [(true, Icon::ArrowLeft, "戻る"), (false, Icon::ArrowRight, "進む")] {
                        let enabled = if back { !self.nav_back.is_empty() } else { !self.nav_fwd.is_empty() };
                        let (r, resp) = ui.allocate_exact_size(vec2(26.0, 26.0), if enabled { Sense::click() } else { Sense::hover() });
                        let color = if !enabled {
                            FAINT().gamma_multiply(0.6)
                        } else if resp.hovered() {
                            WHITE()
                        } else {
                            MUTED()
                        };
                        paint_glyph(ui.painter(), r.shrink(5.0), icon, color);
                        if resp.on_hover_text(tip).clicked() {
                            nav = Some(back);
                        }
                    }
                });
                // 中央に現在地（アイコン＋名前）
                let (label, url) = match &self.sel_guild {
                    None => ("ダイレクトメッセージ".to_string(), None),
                    Some(g) => self.guilds.iter().find(|x| &x.id == g).map(|x| (x.name.clone(), guild_icon_url(x))).unwrap_or_default(),
                };
                let galley = ui.painter().layout_no_wrap(label.clone(), medium_font(ui.ctx(), 14.0), TEXT());
                let w = 18.0 + 8.0 + galley.size().x;
                let ir = Rect::from_min_size(pos2(full.center().x - w / 2.0, full.center().y - 9.0), vec2(18.0, 18.0));
                if self.sel_guild.is_none() {
                    paint_logo(ui.painter(), ir.shrink(1.0), MUTED(), BG());
                } else {
                    paint_icon(ui, ir, url.as_deref(), &label, 5.0, HOVER());
                }
                ui.painter().galley(pos2(ir.right() + 8.0, full.center().y - galley.size().y / 2.0), galley, TEXT());
            });

        let in_voice = self.voice_channel().map(|(g, c, k)| (g.clone(), c.clone(), k));
        // 左列（サーバーレール＋チャンネル欄。下にユーザーパネルがまたがる）
        egui::SidePanel::left("left")
            .resizable(false)
            .exact_width(72.0 + 240.0)
            .show_separator_line(false)
            .frame(Frame::new().fill(BG()))
            .show(ctx, |ui| {
                egui::TopBottomPanel::bottom("userbar")
                    .show_separator_line(false)
                    .frame(Frame::new().fill(BG()).inner_margin(Margin { left: 8, right: 8, top: 6, bottom: 8 }))
                    .show_inside(ui, |ui| {
                        Frame::new()
                            .fill(USERBAR())
                            .stroke(Stroke::new(1.0_f32, DIVIDER()))
                            .corner_radius(8)
                            .inner_margin(Margin::symmetric(8, 6))
                            .show(ui, |ui| {
                                ui.set_width(ui.available_width());
                                ui.spacing_mut().item_spacing.y = 0.0;
                                // ボイス接続パネル（ユーザーパネルの上段）
                                if let Some((gid, cid, connected)) = &in_voice {
                                    let (cname, place) = self.voice_place(gid.as_deref(), cid);
                                    ui.allocate_ui_with_layout(vec2(ui.available_width(), 44.0), Layout::right_to_left(Align::Center), |ui| {
                                        let (r, resp) = ui.allocate_exact_size(vec2(32.0, 32.0), Sense::click());
                                        if resp.hovered() {
                                            ui.painter().rect_filled(r, 8.0, SELECTED());
                                        }
                                        paint_glyph(ui.painter(), r.shrink(6.0), Icon::Hangup, if resp.hovered() { WHITE() } else { TEXT() });
                                        if resp.on_hover_text("切断").clicked() {
                                            act_leave = true;
                                        }
                                        ui.with_layout(Layout::top_down(Align::Min), |ui| {
                                            ui.spacing_mut().item_spacing.y = 2.0;
                                            ui.add_space(4.0);
                                            let color = if *connected { GREEN() } else { ORANGE() };
                                            ui.horizontal(|ui| {
                                                let (g, _) = ui.allocate_exact_size(vec2(16.0, 16.0), Sense::hover());
                                                paint_glyph(ui.painter(), g, Icon::Signal, color);
                                                let t = if *connected { "ボイス接続済み" } else { "接続中…" };
                                                ui.label(RichText::new(t).size(14.0).strong().color(color));
                                            });
                                            let link = ui.add(
                                                egui::Label::new(RichText::new(format!("{cname} / {place}")).size(12.0).color(MUTED()))
                                                    .truncate()
                                                    .sense(Sense::click()),
                                            );
                                            if link.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                                                goto_voice = Some((gid.clone(), cid.clone()));
                                            }
                                        });
                                    });
                                    ui.add_space(6.0);
                                    let r = ui.max_rect();
                                    let y = ui.cursor().top();
                                    ui.painter().hline(r.x_range(), y, Stroke::new(1.0_f32, DIVIDER()));
                                    ui.add_space(6.0);
                                }
                                ui.allocate_ui_with_layout(vec2(ui.available_width(), 40.0), Layout::left_to_right(Align::Center), |ui| {
                                    if let Some(me) = &self.me {
                                        let cur = self.settings.status.as_str();
                                        let (av, av_resp) = ui.allocate_exact_size(vec2(32.0, 32.0), Sense::click());
                                        paint_icon(ui, av, Some(&user_avatar_url(me)), me.display(), 16.0, BLURPLE());
                                        paint_status(ui.painter(), av, cur, USERBAR());
                                        let av_resp = av_resp.on_hover_cursor(egui::CursorIcon::PointingHand);
                                        if av_resp.clicked() {
                                            my_profile = Some((me.clone(), av));
                                        }
                                        ui.add_space(8.0);
                                        let w = (ui.available_width() - (52.0 * 2.0 + 32.0 + 4.0)).max(16.0);
                                        ui.allocate_ui_with_layout(vec2(w, 40.0), Layout::top_down(Align::Min), |ui| {
                                            ui.spacing_mut().item_spacing.y = 0.0;
                                            ui.add_space(3.0);
                                            ui.add(egui::Label::new(RichText::new(me.display()).font(medium_font(ui.ctx(), 14.0)).color(WHITE())).truncate());
                                            ui.add(egui::Label::new(RichText::new(status_label(cur)).size(12.0).color(MUTED())).truncate());
                                        });
                                    }
                                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                        ui.spacing_mut().item_spacing.x = 0.0;
                                        if icon_button(ui, 32.0, Icon::Gear).on_hover_text("ユーザー設定").clicked() {
                                            open_settings = true;
                                        }
                                        let deaf = self.settings.self_deaf;
                                        let (hm, hc) = toggle_with_menu(ui, Icon::Headset, deaf);
                                        if hm.on_hover_text(if deaf { "スピーカーミュート解除" } else { "スピーカーミュート" }).clicked() {
                                            act_deaf = true;
                                        }
                                        let head_id = ui.make_persistent_id("head_menu");
                                        let hc = hc.on_hover_text("出力オプション");
                                        if hc.clicked() {
                                            ui.memory_mut(|m| m.toggle_popup(head_id));
                                        }
                                        let mic_off = self.mic_off();
                                        let (mm, mc) = toggle_with_menu(ui, Icon::Mic, mic_off);
                                        if mm.on_hover_text(if mic_off { "ミュート解除" } else { "ミュート" }).clicked() {
                                            act_mute = true;
                                        }
                                        let mic_id = ui.make_persistent_id("mic_menu");
                                        if mc.clicked() {
                                            ui.memory_mut(|m| m.toggle_popup(mic_id));
                                        }
                                        let mc = mc.on_hover_text("入力オプション");
                                        egui::popup_above_or_below_widget(
                                            ui,
                                            mic_id,
                                            &mc,
                                            egui::AboveOrBelow::Above,
                                            egui::PopupCloseBehavior::CloseOnClickOutside,
                                            |ui| self.audio_menu(ui, true),
                                        );
                                        egui::popup_above_or_below_widget(
                                            ui,
                                            head_id,
                                            &hc,
                                            egui::AboveOrBelow::Above,
                                            egui::PopupCloseBehavior::CloseOnClickOutside,
                                            |ui| self.audio_menu(ui, false),
                                        );
                                    });
                                });
                            });
                    });

                // サーバーレール
                egui::SidePanel::left("rail")
                    .resizable(false)
                    .exact_width(72.0)
                    .show_separator_line(false)
                    .frame(Frame::new().fill(BG()))
                    .show_inside(ui, |ui| {
                        egui::ScrollArea::vertical()
                            .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
                            .show(ui, |ui| {
                                ui.spacing_mut().item_spacing.y = 4.0;
                                ui.add_space(2.0);
                                let unread = self.unread_guilds();
                                if rail_item(ui, self.sel_guild.is_none(), unread.contains(&None), "", None).clicked() {
                                    pick_guild = Some(None);
                                }
                                let (r, _) = ui.allocate_exact_size(vec2(72.0, 6.0), Sense::hover());
                                ui.painter().hline((r.center().x - 16.0)..=(r.center().x + 16.0), r.center().y, Stroke::new(1.0_f32, DIVIDER()));
                                for g in &self.guilds {
                                    let sel = self.sel_guild.as_deref() == Some(&g.id);
                                    let url = guild_icon_url(g);
                                    if rail_item(ui, sel, unread.contains(&Some(g.id.as_str())), &g.name, url.as_deref()).clicked() {
                                        pick_guild = Some(Some(g.id.clone()));
                                    }
                                }
                                ui.add_space(8.0);
                            });
                    });

                // チャンネル欄
                egui::CentralPanel::default().frame(Frame::new().fill(BG())).show_inside(ui, |ui| {
                    let r = ui.max_rect();
                    // 左上だけ丸い内容エリアの縁（右側はチャットとの境目として画面下まで）
                    let stroke = Stroke::new(1.0_f32, DIVIDER());
                    let rad = 8.0;
                    let painter = ui.ctx().layer_painter(ui.layer_id());
                    let mut pts = vec![pos2(r.left() + 0.5, r.bottom())];
                    pts.extend(arc_points(pos2(r.left() + 0.5 + rad, r.top() + 0.5 + rad), rad, std::f32::consts::PI, std::f32::consts::PI * 1.5));
                    pts.push(pos2(r.right(), r.top() + 0.5));
                    painter.add(Shape::line(pts, stroke));
                    painter.vline(r.right() - 0.5, r.top()..=ui.ctx().screen_rect().bottom(), stroke);

                    egui::TopBottomPanel::top("ch_head")
                        .exact_height(48.0)
                        .show_separator_line(false)
                        .frame(Frame::new().inner_margin(Margin { left: 9, right: 8, top: 1, bottom: 0 }))
                        .show_inside(ui, |ui| {
                            let hr = ui.max_rect();
                            ui.painter().hline((hr.left() - 8.0)..=(hr.right() + 8.0), hr.bottom() - 0.5, stroke);
                            ui.with_layout(Layout::left_to_right(Align::Center), |ui| match &self.sel_guild {
                                None => {
                                    let search_id = egui::Id::new("dm_search");
                                    if self.dm_search {
                                        let resp = Frame::new()
                                            .fill(INPUT())
                                            .stroke(stroke)
                                            .corner_radius(8)
                                            .inner_margin(Margin::symmetric(10, 0))
                                            .show(ui, |ui| {
                                                ui.add(
                                                    egui::TextEdit::singleline(&mut self.dm_filter)
                                                        .id(search_id)
                                                        .frame(false)
                                                        .desired_width(f32::INFINITY)
                                                        .font(FontId::proportional(14.0))
                                                        .text_color(TEXT())
                                                        .margin(Margin::symmetric(0, 7))
                                                        .hint_text(RichText::new("会話を検索").color(FAINT())),
                                                )
                                            })
                                            .inner;
                                        let esc = ui.input(|i| i.key_pressed(egui::Key::Escape));
                                        if esc || (resp.lost_focus() && self.dm_filter.trim().is_empty()) {
                                            self.dm_search = false;
                                            self.dm_filter.clear();
                                        }
                                    } else {
                                        let (br, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 30.0), Sense::click());
                                        ui.painter().rect_filled(br, 8.0, if resp.hovered() { SELECTED() } else { HOVER() });
                                        ui.painter().text(br.center(), Align2::CENTER_CENTER, "会話に参加または作成する", FontId::proportional(14.0), if resp.hovered() { TEXT() } else { MUTED() });
                                        if resp.on_hover_cursor(egui::CursorIcon::Text).clicked() {
                                            self.dm_search = true;
                                            ui.memory_mut(|m| m.request_focus(search_id));
                                        }
                                    }
                                }
                                Some(g) => {
                                    let title = self.guilds.iter().find(|x| &x.id == g).map(|x| x.name.clone()).unwrap_or_default();
                                    ui.add_space(7.0);
                                    ui.add(egui::Label::new(RichText::new(title).font(medium_font(ui.ctx(), 16.0)).color(WHITE())).truncate());
                                }
                            });
                        });

                    egui::CentralPanel::default()
                        .frame(Frame::new().inner_margin(Margin { left: 1, top: 8, ..Default::default() }))
                        .show_inside(ui, |ui| {
                            egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
                                ui.spacing_mut().item_spacing.y = 1.0;
                                match &self.sel_guild {
                                    None => {
                                        let (hr, _) = ui.allocate_exact_size(vec2(ui.available_width(), 28.0), Sense::hover());
                                        ui.painter().text(pos2(hr.left() + 16.0, hr.center().y), Align2::LEFT_CENTER, "ダイレクトメッセージ", FontId::proportional(13.0), FAINT());
                                        let q = self.dm_filter.trim().to_lowercase();
                                        let mut shown = 0;
                                        for c in self.dms.iter().filter(|c| q.is_empty() || c.dm_label().to_lowercase().contains(&q)) {
                                            shown += 1;
                                            let sel = self.sel_channel.as_deref() == Some(&c.id);
                                            let pres = c.recipients.first().and_then(|u| self.presences.get(&u.id));
                                            if dm_row(ui, sel, self.unread.contains_key(&c.id), c, pres).clicked() {
                                                open = Some(c.id.clone());
                                            }
                                        }
                                        if shown == 0 && !q.is_empty() {
                                            ui.add_space(12.0);
                                            ui.vertical_centered(|ui| ui.label(RichText::new("見つかりませんでした").size(13.0).color(FAINT())));
                                        }
                                    }
                                    Some(gid) => match self.channels.get(gid) {
                                        None => {
                                            ui.vertical_centered(|ui| ui.spinner());
                                        }
                                        Some(chs) => {
                                            let cur_voice = self.voice_channel().map(|(_, c, _)| c.clone());
                                            let mut row = |ui: &mut egui::Ui, c: &Channel| {
                                                if c.is_voice() {
                                                    let here = cur_voice.as_deref() == Some(&c.id);
                                                    let name = c.name.as_deref().unwrap_or("");
                                                    if voice_channel_row(ui, here, name).on_hover_text("クリックで参加").clicked() && !here {
                                                        act_join = Some((Some(gid.clone()), c.id.clone(), false));
                                                    }
                                                    for v in self.voice_members(&c.id) {
                                                        let u = v.user.clone().or_else(|| self.find_user(&v.user_id));
                                                        let sp = self.is_speaking(&v.user_id);
                                                        voice_member_row(ui, u.as_ref(), &v.user_id, sp, v.self_mute, v.self_deaf);
                                                    }
                                                    return;
                                                }
                                                let sel = self.sel_channel.as_deref() == Some(&c.id);
                                                let name = c.name.as_deref().unwrap_or("");
                                                if channel_row(ui, sel, self.unread.contains_key(&c.id), name).clicked() {
                                                    open = Some(c.id.clone());
                                                }
                                            };
                                            // chs は受信時に表示順へ並べ済み。親ごとに1回の走査で振り分ける
                                            let mut top: Vec<&Channel> = Vec::new();
                                            let mut by_parent: HashMap<&str, Vec<&Channel>> = HashMap::new();
                                            let mut cats: Vec<&Channel> = Vec::new();
                                            for c in chs {
                                                if c.kind == Channel::CATEGORY {
                                                    cats.push(c);
                                                } else if c.is_text() || c.is_voice() {
                                                    match c.parent_id.as_deref() {
                                                        None => top.push(c),
                                                        Some(p) => by_parent.entry(p).or_default().push(c),
                                                    }
                                                }
                                            }
                                            for c in top {
                                                row(ui, c);
                                            }
                                            cats.sort_by_key(|c| c.position);
                                            for cat in cats {
                                                let Some(list) = by_parent.get(cat.id.as_str()) else { continue };
                                                category_header(ui, cat.name.as_deref().unwrap_or(""));
                                                for c in list {
                                                    row(ui, c);
                                                }
                                            }
                                        }
                                    },
                                }
                                ui.add_space(8.0);
                            });
                        });
                });
            });

        // ステータス帯（接続済みのときは出さない）
        if !self.status.is_empty() && self.status != "接続済み" {
            let color = if self.status.starts_with("エラー") { RED() } else { ORANGE() };
            egui::TopBottomPanel::top("status")
                .show_separator_line(false)
                .frame(Frame::new().fill(color).inner_margin(Margin::symmetric(12, 4)))
                .show(ctx, |ui| {
                    ui.add(egui::Label::new(RichText::new(&self.status).size(13.0).color(Color32::WHITE)).truncate());
                });
        }

        let cur = self.current_channel().cloned();
        let (prefix, cname) = match &cur {
            Some(c) if self.sel_guild.is_none() => ("@", c.dm_label()),
            Some(c) => ("#", c.name.clone().unwrap_or_default()),
            None => ("", String::new()),
        };

        let title = match (&cur, &self.sel_guild) {
            (Some(c), Some(gid)) => self.guilds.iter().find(|g| &g.id == gid)
                .map(|g| format!("#{} - {} - Discord", c.name.as_deref().unwrap_or_default(), g.name))
                .unwrap_or_else(|| "Discord".into()),
            (Some(c), None) => format!("@{} - Discord", c.dm_label()),
            (None, _) => "Discord".into(),
        };
        if self.last_title != title {
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(title.clone()));
            self.last_title = title;
        }

        if let Some(gid) = self.sel_guild.as_ref().filter(|_| self.show_members) {
            egui::SidePanel::right("members")
                .resizable(false).exact_width(240.0).show_separator_line(false)
                .frame(Frame::new().fill(BG()))
                .show(ctx, |ui| {
                    let r = ui.max_rect();
                    ui.painter().vline(r.left() + 0.5, r.y_range(), Stroke::new(1.0_f32, DIVIDER()));
                    ui.painter().hline(r.x_range(), r.top() + 0.5, Stroke::new(1.0_f32, DIVIDER()));
                    let members = self.members.get(gid).map(Vec::as_slice).unwrap_or(&[]);
                    let roles = self.roles.get(gid).map(Vec::as_slice).unwrap_or(&[]);
                    // グループ分けと並べ替えは、メンバー・ロール・オンライン状態が変わったときだけやり直す
                    if self.member_cache.gid != *gid || self.member_cache.ver != self.member_ver {
                        self.member_cache = MemberListCache::build(gid, self.member_ver, members, roles, &self.presences);
                    }
                    let cache = &self.member_cache;
                    egui::ScrollArea::vertical().id_salt("member_list").auto_shrink(false).show_viewport(ui, |ui, vp| {
                        // 全体の高さだけ確保し、見えている行だけ描く（仮想スクロール）
                        let w = ui.available_width();
                        let (content, _) = ui.allocate_exact_size(vec2(w, cache.height), Sense::hover());
                        let first = cache.tops.partition_point(|&t| t + MEMBER_ROW_H < vp.min.y);
                        for (i, row) in cache.rows.iter().enumerate().skip(first) {
                            let top = cache.tops[i];
                            if top > vp.max.y {
                                break;
                            }
                            match row {
                                MemberRow::Header(text) => {
                                    ui.painter().text(pos2(content.left() + 16.0, content.top() + top + 28.0), Align2::LEFT_CENTER, text, FontId::proportional(12.0), FAINT());
                                }
                                MemberRow::Member(mi) => {
                                    let member = &members[*mi];
                                    let user = member.user.as_ref().unwrap();
                                    let rect = Rect::from_min_size(pos2(content.left(), content.top() + top), vec2(w, MEMBER_ROW_H)).shrink2(vec2(8.0, 1.0));
                                    let resp = ui.interact(rect, ui.id().with(("member", *mi)), Sense::click());
                                    if resp.hovered() {
                                        ui.painter().rect_filled(rect, 8.0, HOVER());
                                    }
                                    let presence = self.presences.get(&user.id);
                                    let offline = presence.is_none_or(|p| p.status == "offline");
                                    let ring = if resp.hovered() { HOVER() } else { BG() };
                                    ui.scope(|ui| {
                                        if offline {
                                            ui.multiply_opacity(0.3);
                                        }
                                        let av = Rect::from_min_size(pos2(rect.left() + 8.0, rect.center().y - 16.0), vec2(32.0, 32.0));
                                        paint_icon(ui, av, Some(&user_avatar_url(user)), member.display(), 16.0, name_color(&user.id));
                                        paint_status(ui.painter(), av, presence.map_or("offline", |p| p.status.as_str()), ring);
                                        let color = member_role_color(member, roles).unwrap_or(TEXT());
                                        let p = ui.painter().with_clip_rect(rect);
                                        let x = av.right() + 12.0;
                                        if let Some(activity) = presence.and_then(|p| p.activity.as_deref()) {
                                            p.text(pos2(x, rect.center().y - 8.0), Align2::LEFT_CENTER, member.display(), medium_font(ui.ctx(), 16.0), color);
                                            p.text(pos2(x, rect.center().y + 10.0), Align2::LEFT_CENTER, activity, FontId::proportional(12.0), MUTED());
                                        } else {
                                            p.text(pos2(x, rect.center().y), Align2::LEFT_CENTER, member.display(), medium_font(ui.ctx(), 16.0), color);
                                        }
                                    });
                                    if resp.clicked() {
                                        clicked_user = Some((user.clone(), rect));
                                    }
                                }
                            }
                        }
                    });
                });
        }

        // DM 相手のプロフィール（Vesktop と同じく右側に常設。幅が狭いときは出さない）
        let dm_user = match (&self.sel_guild, &cur) {
            (None, Some(c)) if c.recipients.len() == 1 => Some(c.recipients[0].clone()),
            _ => None,
        };
        if let Some(u) = dm_user.as_ref().filter(|_| !self.hide_dm_profile && ctx.screen_rect().width() > 980.0) {
            let key = net::profile_key(&u.id, None);
            if !self.profiles.contains_key(&key) {
                self.profiles.insert(key.clone(), ProfileState::Loading);
                self.send(Cmd::LoadProfile { user_id: u.id.clone(), guild_id: None });
            }
            egui::SidePanel::right("dm_profile")
                .resizable(false)
                .exact_width(340.0)
                .show_separator_line(false)
                .frame(Frame::new().fill(BG()).inner_margin(Margin { left: 10, right: 10, top: 10, bottom: 10 }))
                .show(ctx, |ui| {
                    let r = ui.max_rect().expand(10.0);
                    ui.painter().vline(r.left() + 0.5, r.y_range(), Stroke::new(1.0_f32, DIVIDER()));
                    ui.painter().hline(r.x_range(), r.top() + 0.5, Stroke::new(1.0_f32, DIVIDER()));
                    // 毎フレーム ProfileData（自己紹介の文字列など）を複製しないよう参照で渡す
                    let data = match self.profiles.get(&key) {
                        Some(ProfileState::Ready(d)) => Some(d),
                        _ => None,
                    };
                    let presence = self.presences.get(&u.id);
                    if let Some(user) = self.dm_profile_card(ui, u, data, presence) {
                        clicked_user = Some(user);
                    }
                });
        }

        // チャットヘッダー
        egui::TopBottomPanel::top("chat_head")
            .exact_height(48.0)
            .show_separator_line(false)
            .frame(Frame::new().fill(CHAT()).inner_margin(Margin::symmetric(16, 0)))
            .show(ctx, |ui| {
                let r = ui.max_rect();
                ui.painter().hline((r.left() - 16.0)..=(r.right() + 16.0), r.top() + 0.5, (1.0, DIVIDER()));
                ui.painter().hline((r.left() - 16.0)..=(r.right() + 16.0), r.bottom() - 0.5, (1.0, DIVIDER()));
                if cur.is_none() {
                    return;
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.spacing_mut().item_spacing.x = 6.0;
                    // 右から: 一覧/プロフィール切替・再読込・ビデオ・通話（Vesktop の並び）
                    if self.sel_guild.is_some() {
                        if toggle_icon(ui, Icon::Members, self.show_members).on_hover_text(if self.show_members { "メンバーリストを非表示" } else { "メンバーリストを表示" }).clicked() {
                            self.show_members = !self.show_members;
                        }
                    } else if dm_user.is_some() {
                        let shown = !self.hide_dm_profile;
                        if toggle_icon(ui, Icon::Profile, shown).on_hover_text(if shown { "ユーザープロフィールを非表示" } else { "ユーザープロフィールを表示" }).clicked() {
                            self.hide_dm_profile = shown;
                        }
                    }
                    let (rr, rresp) = ui.allocate_exact_size(vec2(32.0, 32.0), Sense::click());
                    if rresp.hovered() {
                        ui.painter().rect_filled(rr, 8.0, SELECTED());
                    }
                    ui.painter().text(rr.center(), Align2::CENTER_CENTER, "⟳", FontId::proportional(18.0), if rresp.hovered() { WHITE() } else { MUTED() });
                    if rresp.on_hover_text("再読込").clicked() {
                        reload = true;
                    }
                    if let (None, Some(c)) = (&self.sel_guild, &cur) {
                        let (r, resp) = ui.allocate_exact_size(vec2(32.0, 32.0), Sense::hover());
                        paint_glyph(ui.painter(), r.shrink(5.0), Icon::Video, FAINT().gamma_multiply(0.5));
                        resp.on_hover_text("ビデオ通話（未対応）");
                        if icon_button(ui, 32.0, Icon::Phone).on_hover_text("音声通話を開始").clicked() {
                            let active = self.calls.contains_key(&c.id) || self.voice_members(&c.id).next().is_some();
                            if self.voice_channel().map(|(_, id, _)| id) != Some(&c.id) {
                                act_join = Some((None, c.id.clone(), !active));
                            }
                        }
                    }
                    ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                        match (&self.sel_guild, &cur) {
                            (None, Some(c)) => {
                                // DM は相手のアイコン＋名前
                                let (av, _) = ui.allocate_exact_size(vec2(24.0, 24.0), Sense::hover());
                                paint_icon(ui, av, dm_icon_url(c).as_deref(), &cname, 12.0, BLURPLE());
                                if c.recipients.len() == 1 {
                                    let st = self.presences.get(&c.recipients[0].id).map_or("offline", |p| p.status.as_str());
                                    paint_status(ui.painter(), av, st, CHAT());
                                }
                                ui.add_space(2.0);
                            }
                            _ => {
                                ui.label(RichText::new(prefix).size(22.0).color(FAINT()));
                            }
                        }
                        ui.label(RichText::new(&cname).font(medium_font(ui.ctx(), 16.0)).color(WHITE()));
                        if let Some(topic) = cur.as_ref().and_then(|c| c.topic.as_deref()).filter(|t| !t.is_empty()) {
                            ui.add_space(4.0);
                            ui.separator();
                            ui.add_space(4.0);
                            ui.add(egui::Label::new(RichText::new(topic.replace('\n', " ")).size(14.0).color(MUTED())).truncate());
                        }
                    });
                });
            });

        // DM通話エリア
        if let (None, Some(c)) = (&self.sel_guild, &cur) {
            let members: Vec<(String, Option<User>, bool, bool)> = self
                .voice_members(&c.id)
                .map(|v| (v.user_id.clone(), v.user.clone().or_else(|| self.find_user(&v.user_id)), v.self_mute || v.self_deaf, false))
                .chain(
                    self.calls
                        .get(&c.id)
                        .into_iter()
                        .flatten()
                        .filter(|id| !self.voice_states.get(*id).is_some_and(|v| v.channel_id.as_deref() == Some(&c.id)))
                        .map(|id| (id.clone(), self.find_user(id), false, true)),
                )
                .collect();
            let in_call = self.voice_channel().map(|(_, id, _)| id) == Some(&c.id);
            if !members.is_empty() || in_call || self.calls.contains_key(&c.id) {
                egui::TopBottomPanel::top("call_area")
                    .exact_height(230.0)
                    .show_separator_line(false)
                    .frame(Frame::new().fill(CALL_BG()))
                    .show(ctx, |ui| {
                        ui.vertical_centered(|ui| {
                            ui.add_space(36.0);
                            let (size, gap) = (80.0, 20.0);
                            let n = members.len().max(1) as f32;
                            let (row, _) = ui.allocate_exact_size(vec2(n * size + (n - 1.0) * gap, size), Sense::hover());
                            for (i, (id, u, muted, ringing)) in members.iter().enumerate() {
                                let r = Rect::from_min_size(row.min + vec2(i as f32 * (size + gap), 0.0), vec2(size, size));
                                speaking_avatar(ui, r, u.as_ref(), id, !ringing && self.is_speaking(id), CALL_BG());
                                if *ringing {
                                    // 呼び出し中は暗くして脈打たせる
                                    let t = ui.input(|i| i.time) as f32;
                                    let a = (0.45 + 0.25 * (t * 3.0).sin()) * 255.0;
                                    ui.painter().circle_filled(r.center(), size / 2.0, Color32::from_black_alpha(a as u8));
                                    ui.ctx().request_repaint_after(std::time::Duration::from_millis(50));
                                }
                                if *muted {
                                    let b = Rect::from_center_size(r.right_bottom() - vec2(12.0, 12.0), vec2(24.0, 24.0));
                                    ui.painter().circle_filled(b.center(), 12.0, RED());
                                    paint_glyph(ui.painter(), b.shrink(5.0), Icon::Mic, WHITE());
                                }
                                if let Some(u) = u {
                                    ui.painter().text(r.center_bottom() + vec2(0.0, 14.0), Align2::CENTER_CENTER, u.display(), FontId::proportional(12.0), MUTED());
                                }
                            }
                            ui.add_space(40.0);
                            if in_call {
                                ui.allocate_ui_with_layout(vec2(56.0 * 3.0 + 32.0, 56.0), Layout::left_to_right(Align::Center), |ui| {
                                    ui.spacing_mut().item_spacing.x = 16.0;
                                    let dark = SELECTED();
                                    let off = self.mic_off();
                                    let (bg, fg) = if off { (WHITE(), RED()) } else { (dark, WHITE()) };
                                    if round_button(ui, 56.0, Icon::Mic, bg, fg, off).on_hover_text(if off { "ミュート解除" } else { "ミュート" }).clicked() {
                                        act_mute = true;
                                    }
                                    let deaf = self.settings.self_deaf;
                                    let (bg, fg) = if deaf { (WHITE(), RED()) } else { (dark, WHITE()) };
                                    if round_button(ui, 56.0, Icon::Headset, bg, fg, deaf).on_hover_text("スピーカーミュート").clicked() {
                                        act_deaf = true;
                                    }
                                    if round_button(ui, 56.0, Icon::Hangup, RED(), WHITE(), false).on_hover_text("切断").clicked() {
                                        act_leave = true;
                                    }
                                });
                            } else if round_button(ui, 56.0, Icon::Phone, GREEN(), WHITE(), false).on_hover_text("通話に参加").clicked() {
                                act_join = Some((None, c.id.clone(), false));
                            }
                        });
                    });
            }
        }

        // 入力欄
        if cur.is_some() {
            if self.upload_channel != self.sel_channel {
                self.uploads.clear();
                self.upload_channel = self.sel_channel.clone();
            }
            let input_id = egui::Id::new(INPUT_ID);
            if self.editing.is_none() && ctx.memory(|m| m.focused().is_none_or(|f| f == input_id)) {
                self.take_pasted(ctx);
            }
            let hint = format!("{prefix}{cname} へメッセージを送信");
            egui::TopBottomPanel::bottom("input")
                .show_separator_line(false)
                .frame(Frame::new().fill(CHAT()).inner_margin(Margin { left: 10, right: 10, top: 2, bottom: 10 }))
                .show(ctx, |ui| {
                    ui.spacing_mut().item_spacing.y = 0.0;
                    // 返信中・編集中の帯（入力欄の上にくっつく）
                    let bar = match (&self.editing, &self.reply_to) {
                        (Some(_), _) => Some(("メッセージを編集中".to_string(), "Esc でキャンセル・Enter で保存".to_string())),
                        (None, Some(m)) => Some((format!("{} に返信", author_name(m)), "Esc でキャンセル".to_string())),
                        _ => None,
                    };
                    if let Some((label, sub)) = &bar {
                        Frame::new()
                            .fill(SIDEBAR())
                            .stroke(Stroke::new(1.0_f32, DIVIDER()))
                            .corner_radius(CornerRadius { nw: 8, ne: 8, sw: 0, se: 0 })
                            .inner_margin(Margin { left: 14, right: 8, top: 6, bottom: 6 })
                            .show(ui, |ui| {
                                ui.set_width(ui.available_width());
                                ui.horizontal(|ui| {
                                    ui.label(RichText::new(label).size(13.0).color(MUTED()));
                                    ui.label(RichText::new(sub).size(12.0).color(FAINT()));
                                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                        if icon_button(ui, 22.0, Icon::Close).on_hover_text("キャンセル").clicked() {
                                            cancel_compose = true;
                                        }
                                    });
                                });
                            });
                    }
                    let radius = if bar.is_some() { CornerRadius { nw: 0, ne: 0, sw: 8, se: 8 } } else { CornerRadius::same(8) };
                    Frame::new().fill(INPUT()).stroke(Stroke::new(1.0_f32, DIVIDER())).corner_radius(radius).inner_margin(Margin { left: 12, right: 16, top: 12, bottom: 12 }).show(ui, |ui| {
                        // 送信待ちの添付
                        if !self.uploads.is_empty() {
                            let mut remove = None;
                            egui::ScrollArea::horizontal().id_salt("uploads").show(ui, |ui| {
                                ui.horizontal(|ui| {
                                    ui.spacing_mut().item_spacing.x = 8.0;
                                    for (i, u) in self.uploads.iter().enumerate() {
                                        Frame::new().fill(SIDEBAR()).corner_radius(6).inner_margin(6).show(ui, |ui| {
                                            ui.vertical(|ui| {
                                                let (rect, _) = ui.allocate_exact_size(vec2(120.0, 90.0), Sense::hover());
                                                match &u.thumb {
                                                    Some(t) => {
                                                        let s = t.size_vec2();
                                                        let k = (rect.width() / s.x).min(rect.height() / s.y).min(1.0);
                                                        let r = Rect::from_center_size(rect.center(), s * k);
                                                        egui::Image::from_texture(t).corner_radius(4).paint_at(ui, r);
                                                    }
                                                    None => paint_glyph(ui.painter(), Rect::from_center_size(rect.center(), vec2(32.0, 32.0)), Icon::Plus, MUTED()),
                                                }
                                                let tb = Rect::from_min_size(rect.right_top() - vec2(22.0, 0.0), vec2(22.0, 22.0));
                                                if ui.put(tb, egui::Button::new(RichText::new("✕").size(12.0)).corner_radius(4)).on_hover_text("添付を外す").clicked() {
                                                    remove = Some(i);
                                                }
                                                ui.add_sized(vec2(120.0, 16.0), egui::Label::new(RichText::new(&u.name).size(12.0).color(MUTED())).truncate());
                                            });
                                        });
                                    }
                                });
                            });
                            if let Some(i) = remove {
                                self.uploads.remove(i);
                            }
                            ui.add_space(10.0);
                        }
                        ui.horizontal(|ui| {
                            let plus_id = egui::Id::new("plus_menu");
                            let open = ui.memory(|m| m.is_popup_open(plus_id));
                            let (pr, presp) = ui.allocate_exact_size(vec2(24.0, 22.0), Sense::click());
                            let lit = open || presp.hovered();
                            paint_glyph(ui.painter(), pr.shrink(2.0), Icon::Plus, if lit { WHITE() } else { MUTED() });
                            let presp = presp.on_hover_cursor(egui::CursorIcon::PointingHand);
                            if presp.clicked() {
                                ui.memory_mut(|m| m.toggle_popup(plus_id));
                            }
                            egui::popup_above_or_below_widget(
                                ui,
                                plus_id,
                                &presp,
                                egui::AboveOrBelow::Above,
                                egui::PopupCloseBehavior::CloseOnClickOutside,
                                |ui| {
                                    if let Some(a) = plus_menu(ui) {
                                        plus_action = Some(a);
                                        ui.memory_mut(|m| m.close_popup());
                                    }
                                },
                            );
                            ui.add_space(10.0);
                            let r = ui.add(
                                egui::TextEdit::singleline(&mut self.input)
                                    .id(egui::Id::new(INPUT_ID))
                                    .frame(false)
                                    .desired_width(f32::INFINITY)
                                    .font(FontId::proportional(16.0))
                                    .text_color(TEXT())
                                    .hint_text(RichText::new(hint).color(FAINT())),
                            );
                            let (enter, esc, up) = ui.input(|i| (i.key_pressed(egui::Key::Enter), i.key_pressed(egui::Key::Escape), i.key_pressed(egui::Key::ArrowUp)));
                            if r.lost_focus() && enter {
                                let content = std::mem::take(&mut self.input);
                                if !content.trim().is_empty() || self.editing.is_some() || !self.uploads.is_empty() {
                                    to_send = Some(content);
                                }
                                r.request_focus();
                            } else if esc && (r.lost_focus() || r.has_focus()) && bar.is_some() {
                                cancel_compose = true;
                                r.request_focus();
                            } else if up && r.has_focus() && self.input.is_empty() && self.editing.is_none() {
                                // ↑ で自分の直前のメッセージを編集（本家と同じ）
                                edit_last = true;
                            }
                        });
                    });
                });
        }

        // メッセージ一覧
        let mut want_stickers: Vec<(String, u8)> = Vec::new();
        let mut shown_images: Vec<String> = Vec::new();
        let mut msg_action: Option<MsgAction> = None;
        let mut load_older: Option<(String, String)> = None;
        egui::CentralPanel::default().frame(Frame::new().fill(CHAT())).show(ctx, |ui| {
            if let Some(bg) = self.theme_applied.as_ref().and_then(|t| t.background.as_ref()) {
                paint_background(ui, ui.max_rect(), bg);
            }
            let Some(cid) = &self.sel_channel else {
                ui.centered_and_justified(|ui| {
                    ui.label(RichText::new("左からチャンネルを選んでください").size(16.0).color(MUTED()));
                });
                return;
            };
            let Some(msgs) = self.messages.get(cid) else {
                ui.centered_and_justified(|ui| ui.spinner());
                return;
            };
            let hist = self.history.entry(cid.clone()).or_default();
            let no_access = self.no_access.contains(cid);
            // 先頭に読み足した分だけ下げる（見ている行をその場に留める）
            let mut shift = self.prepended.remove(cid).unwrap_or(0.0);
            let out = egui::ScrollArea::vertical()
                .id_salt(cid)
                .stick_to_bottom(true)
                .auto_shrink(false)
                .show_viewport(ui, |ui, viewport| {
                    ui.spacing_mut().item_spacing.y = 0.0;
                    // 先頭: 始まりまで読んだらその旨、まだなら読み込み中の印
                    let (head, _) = ui.allocate_exact_size(vec2(ui.available_width(), HISTORY_HEAD_H), Sense::hover());
                    if ui.is_rect_visible(head) {
                        if no_access {
                            ui.painter().text(pos2(head.left() + 16.0, head.bottom() - 12.0), Align2::LEFT_BOTTOM, "このチャンネルを見る権限がありません", FontId::proportional(20.0), MUTED());
                        } else if hist.end {
                            ui.painter().text(pos2(head.left() + 16.0, head.bottom() - 12.0), Align2::LEFT_BOTTOM, format!("{prefix}{cname} の始まりです"), FontId::proportional(20.0), WHITE());
                        } else {
                            egui::Spinner::new().paint_at(ui, Rect::from_center_size(head.center(), vec2(24.0, 24.0)));
                        }
                    }
                    let mut cx =
                        MediaCx { settings: &self.settings, stickers: &self.stickers, roles: self.sel_guild.as_ref().and_then(|gid| self.roles.get(gid)).map(Vec::as_slice).unwrap_or(&[]), want: Vec::new(), images: Vec::new(), clicked_user: None,
                            me: self.me.as_ref().map(|u| u.id.as_str()),
                            highlight: self.editing.as_ref().or(self.reply_to.as_ref()).map(|m| m.id.as_str()),
                            tb_prev: self.toolbar.take(),
                            tb_now: None,
                            action: None };
                    // 画面外の行は描かず、前回測った高さ分だけ空ける（仮想スクロール）
                    let key = (ui.available_width().round() as u32, self.settings.font_size.to_bits(), self.settings.compact);
                    let cache = self.row_heights.entry(cid.clone()).or_default();
                    if cache.key != key {
                        cache.key = key;
                        cache.h.clear();
                    }
                    let origin = ui.cursor().top();
                    let (lo, hi) = (viewport.min.y - VIEW_MARGIN, viewport.max.y + VIEW_MARGIN);
                    let mut y = 0.0;
                    let mut gap = 0.0;
                    for (i, m) in msgs.iter().enumerate() {
                        let known = cache.h.get(&m.id).copied();
                        let h = known.unwrap_or(EST_ROW_H);
                        // 高さ不明の行は、見える範囲に掛かりそうなときだけ描いて測る
                        if y + h < lo || y > hi {
                            gap += h;
                            y += h;
                            continue;
                        }
                        if gap > 0.0 {
                            ui.add_space(gap);
                            gap = 0.0;
                        }
                        let top = ui.cursor().top();
                        message_row(ui, m, i.checked_sub(1).map(|j| &msgs[j]), &mut cx);
                        let real = ui.cursor().top() - top;
                        // 見えている範囲より上の行の高さが仮の値から変わったら、その差だけスクロール位置を直す
                        if top - origin + h <= viewport.min.y {
                            shift += real - h;
                        }
                        cache.h.insert(m.id.clone(), real);
                        y = ui.cursor().top() - origin;
                    }
                    if gap > 0.0 {
                        ui.add_space(gap);
                    }
                    want_stickers = cx.want;
                    shown_images = cx.images;
                    msg_action = cx.action.take();
                    self.toolbar = cx.tb_now.take();
                    if cx.clicked_user.is_some() { clicked_user = cx.clicked_user; }
                    ui.add_space(16.0);
                });
            // 上端に近づいたら古いメッセージを読み足す。開いた直後のフレームはまだ下端に移る前なので、
            // 描画後（stick_to_bottom が効いた後）の位置で判断する
            let retry_ok = hist.retry_at.is_none_or(|t| std::time::Instant::now() >= t);
            if !hist.end && hist.pending.is_none() && retry_ok && out.state.offset.y < HISTORY_TRIGGER {
                if let Some(first) = msgs.first().filter(|_| msgs.len() < MAX_HISTORY) {
                    hist.pending = Some(first.id.clone());
                    hist.retry_at = None;
                    load_older = Some((cid.clone(), first.id.clone()));
                }
            }
            if hist.retry_at.is_some() {
                ui.ctx().request_repaint_after(std::time::Duration::from_secs(1));
            }
            // 一番下に張り付いているときは stick_to_bottom に任せる
            let at_bottom = out.state.offset.y + out.inner_rect.height() >= out.content_size.y - 2.0;
            if shift.abs() > 0.5 && !at_bottom {
                let mut st = out.state;
                st.offset.y = (st.offset.y + shift).max(0.0);
                st.store(ui.ctx(), out.id);
                // ずれた位置のまま一瞬表示されないよう、このフレームを描き直す
                ui.ctx().request_discard("履歴の行の高さが変わった");
            }
        });
        match plus_action {
            Some(PlusAction::Upload) => self.pick_files(ctx),
            Some(PlusAction::PasteImage) => match clip::clipboard_image(ctx) {
                Some(Ok(u)) => {
                    self.uploads.push(u);
                    self.uploads.truncate(10);
                }
                Some(Err(e)) => self.status = format!("エラー: {e}"),
                None => self.status = "クリップボードに画像がありません".into(),
            },
            None => {}
        }
        if let Some((channel_id, before)) = load_older {
            self.send(Cmd::LoadOlder { channel_id, before });
        }

        if let (Some(cid), false) = (&self.sel_channel, shown_images.is_empty()) {
            let set = self.channel_images.entry(cid.clone()).or_default();
            set.extend(shown_images);
        }
        for (id, format) in want_stickers {
            if !self.stickers.contains_key(&id) {
                self.stickers.insert(id.clone(), StickerState::Loading);
                self.send(Cmd::LoadSticker { id, format });
            }
        }
        if let Some(g) = pick_guild {
            if let Some(gid) = &g {
                if !self.channels.contains_key(gid) || !self.roles.contains_key(gid) {
                    self.send(Cmd::LoadGuildChannels(gid.clone()));
                }
                if self.members_requested.insert(gid.clone()) { self.send(Cmd::LoadMembers(gid.clone())); }
            }
            if self.sel_guild != g {
                self.sel_guild = g;
                self.sel_channel = None;
            }
        }
        if let Some(id) = open {
            self.open_channel(id);
        }
        if reload {
            if let Some(c) = &self.sel_channel {
                self.send(Cmd::LoadMessages(c.clone()));
            }
        }
        if let (Some(content), Some(cid)) = (to_send, self.sel_channel.clone()) {
            if let Some(m) = self.editing.take() {
                if content.trim().is_empty() {
                    // 空にして保存 = 削除（本家と同じく確認を出す）
                    self.pending_delete = Some(m);
                } else if content != m.content {
                    self.send(Cmd::Edit { channel_id: m.channel_id, id: m.id, content });
                }
            } else {
                let reply_to = self.reply_to.take().map(|m| m.id);
                let files = std::mem::take(&mut self.uploads).into_iter().map(|u| (u.name, u.data)).collect();
                self.send(Cmd::Send { channel_id: cid, content, reply_to, files });
            }
        }
        if cancel_compose {
            if self.editing.take().is_some() {
                self.input.clear();
            }
            self.reply_to = None;
        }
        if edit_last {
            let me = self.me.as_ref().map(|u| u.id.clone());
            let last = self.sel_channel.as_ref().and_then(|c| self.messages.get(c)).and_then(|l| l.iter().rev().find(|m| Some(&m.author.id) == me.as_ref())).cloned();
            if let Some(m) = last {
                self.input = m.content.clone();
                self.editing = Some(m);
                self.reply_to = None;
            }
        }
        match msg_action {
            Some(MsgAction::Reply(m)) => {
                if self.editing.take().is_some() {
                    self.input.clear();
                }
                self.reply_to = Some(m);
                ctx.memory_mut(|mem| mem.request_focus(egui::Id::new(INPUT_ID)));
            }
            Some(MsgAction::Edit(m)) => {
                self.input = m.content.clone();
                self.editing = Some(m);
                self.reply_to = None;
                ctx.memory_mut(|mem| mem.request_focus(egui::Id::new(INPUT_ID)));
            }
            Some(MsgAction::Copy(t)) => ctx.copy_text(t),
            Some(MsgAction::Delete(m, true)) => self.send(Cmd::Delete { channel_id: m.channel_id, id: m.id }),
            Some(MsgAction::Delete(m, false)) => self.pending_delete = Some(m),
            None => {}
        }
        if let Some(target) = self.switcher_ui(ctx) {
            self.select(target.0, target.1);
        }
        self.delete_confirm_ui(ctx);
        if open_settings {
            self.settings_open = true;
        }
        if let Some((u, r)) = my_profile {
            self.open_profile(u, r, true);
        }
        if let Some((u, r)) = clicked_user {
            self.open_profile(u, r, false);
        }
        self.incoming_call_ui(ctx, &mut act_join, &mut act_decline);
        if act_mute {
            self.toggle_mute();
        }
        if act_deaf {
            self.toggle_deaf();
        }
        if act_leave {
            self.leave_voice();
        }
        if let Some((g, c, ring)) = act_join {
            self.join_voice(g, c, ring);
        }
        if let Some(cid) = act_decline {
            if let (Some(r), Some(me)) = (self.calls.get_mut(&cid), &self.me) {
                r.retain(|u| u != &me.id);
            }
            self.send(Cmd::StopRinging(cid));
        }
        if let Some((g, c)) = goto_voice {
            if g != self.sel_guild {
                if let Some(gid) = &g {
                    if !self.channels.contains_key(gid) || !self.roles.contains_key(gid) {
                        self.send(Cmd::LoadGuildChannels(gid.clone()));
                    }
                    if self.members_requested.insert(gid.clone()) { self.send(Cmd::LoadMembers(gid.clone())); }
                }
                self.sel_guild = g.clone();
                self.sel_channel = None;
            }
            if g.is_none() {
                self.open_channel(c);
            }
        }
        if let Some(back) = nav {
            let (from, to) = if back { (&mut self.nav_back, &mut self.nav_fwd) } else { (&mut self.nav_fwd, &mut self.nav_back) };
            if let Some((g, c)) = from.pop() {
                to.push(before);
                self.select(g, c);
            }
        } else if (&self.sel_guild, &self.sel_channel) != (&before.0, &before.1) {
            self.nav_back.push(before);
            if self.nav_back.len() > 100 {
                self.nav_back.remove(0);
            }
            self.nav_fwd.clear();
        }
        // 別の場所へ移ったら、そこに属さない返信・編集はやめる
        let here = self.sel_channel.clone();
        if self.editing.as_ref().is_some_and(|m| Some(&m.channel_id) != here.as_ref()) {
            self.editing = None;
            self.input.clear();
        }
        if self.reply_to.as_ref().is_some_and(|m| Some(&m.channel_id) != here.as_ref()) {
            self.reply_to = None;
        }
        self.profile_popup_ui(ctx);
    }

    /// サイドバーに並んでいる順のチャンネル（Alt+↑↓ 用。テキストのみ）
    fn sidebar_order(&self) -> Vec<String> {
        match &self.sel_guild {
            None => self.dms.iter().map(|c| c.id.clone()).collect(),
            Some(g) => {
                let Some(chs) = self.channels.get(g) else { return Vec::new() };
                let mut out: Vec<String> = chs.iter().filter(|c| c.is_text() && c.parent_id.is_none()).map(|c| c.id.clone()).collect();
                let mut cats: Vec<&Channel> = chs.iter().filter(|c| c.kind == Channel::CATEGORY).collect();
                cats.sort_by_key(|c| c.position);
                for cat in cats {
                    out.extend(chs.iter().filter(|c| c.is_text() && c.parent_id.as_deref() == Some(&cat.id)).map(|c| c.id.clone()));
                }
                out
            }
        }
    }

    /// DM 右側のプロフィールカード。「プロフィール全体を表示」が押されたら (ユーザー, 位置) を返す
    fn dm_profile_card(&self, ui: &mut egui::Ui, u: &User, data: Option<&ProfileData>, presence: Option<&Presence>) -> Option<(User, Rect)> {
        let mut open = None;
        let accent = data.and_then(|d| d.accent_color).map(rgb).unwrap_or_else(|| darken(name_color(&u.id)));
        let status = presence.map_or("offline", |p| p.status.as_str());
        Frame::new().fill(SIDEBAR()).corner_radius(8).stroke(Stroke::new(1.0_f32, BORDER())).show(ui, |ui| {
            let w = ui.available_width();
            ui.set_width(w);
            ui.set_min_height(ui.available_height());
            ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
            let (banner, _) = ui.allocate_exact_size(vec2(w, 120.0), Sense::hover());
            let top = CornerRadius { nw: 8, ne: 8, sw: 0, se: 0 };
            ui.painter().rect_filled(banner, top, accent);
            if let Some(h) = data.and_then(|d| d.banner.as_ref()) {
                let url = format!("https://cdn.discordapp.com/banners/{}/{}.png?size=600", u.id, h);
                let img = egui::Image::new(url);
                if let Ok(egui::load::TexturePoll::Ready { texture }) = img.load_for_size(ui.ctx(), banner.size()) {
                    egui::Image::from_texture(texture).corner_radius(top).paint_at(ui, banner);
                }
            }
            let av = Rect::from_min_size(pos2(banner.left() + 16.0, banner.bottom() - 44.0), vec2(80.0, 80.0));
            ui.painter().circle_filled(av.center(), 46.0, SIDEBAR());
            paint_icon(ui, av, Some(&user_avatar_url(u)), u.display(), 40.0, name_color(&u.id));
            paint_status(ui.painter(), av, status, SIDEBAR());
            ui.add_space(48.0);
            Frame::new().inner_margin(Margin::symmetric(16, 0)).show(ui, |ui| {
                ui.set_width(w - 32.0);
                ui.spacing_mut().item_spacing.y = 4.0;
                ui.add(egui::Label::new(RichText::new(u.display()).font(medium_font(ui.ctx(), 20.0)).color(WHITE())).truncate());
                let mut line = u.username.clone();
                if let Some(p) = data.and_then(|d| d.pronouns.as_deref()).filter(|p| !p.is_empty()) {
                    line.push_str(&format!(" • {p}"));
                }
                ui.label(RichText::new(line).size(14.0).color(MUTED()));
                if let Some(a) = presence.and_then(|p| p.activity.as_deref()) {
                    ui.add_space(4.0);
                    ui.label(RichText::new(a).size(13.0).color(TEXT()));
                }
                if let Some(bio) = data.and_then(|d| d.bio.as_deref()).filter(|b| !b.is_empty()) {
                    ui.add_space(10.0);
                    // カスタム絵文字 <:name:id> は画像で出す
                    ui.horizontal_wrapped(|ui| {
                        ui.spacing_mut().item_spacing = vec2(0.0, 2.0);
                        for seg in split_emoji(bio) {
                            match seg {
                                Seg::Text(t) => {
                                    ui.label(RichText::new(t).size(14.0).color(TEXT()));
                                }
                                Seg::Emoji { name, id, animated } => {
                                    ui.add(anim_image(ui.ctx(), emoji_url(id, animated)).fit_to_exact_size(vec2(18.0, 18.0))).on_hover_text(format!(":{name}:"));
                                }
                            }
                        }
                    });
                }
                ui.add_space(14.0);
                ui.label(RichText::new("メンバー登録日").font(medium_font(ui.ctx(), 12.0)).color(MUTED()));
                ui.label(RichText::new(snowflake_date(&u.id)).size(14.0).color(TEXT()));
            });
            ui.with_layout(Layout::bottom_up(Align::Center), |ui| {
                ui.add_space(16.0);
                let btn = egui::Button::new(RichText::new("プロフィール全体を表示").size(14.0).color(WHITE()))
                    .fill(RAIL())
                    .corner_radius(8)
                    .min_size(vec2(w - 32.0, 38.0));
                let resp = ui.add(btn);
                if resp.clicked() {
                    open = Some((u.clone(), resp.rect));
                }
            });
        });
        open
    }

    /// Ctrl+K のクイックスイッチャー。選ばれた行き先 (サーバー, チャンネル) を返す
    fn switcher_ui(&mut self, ctx: &egui::Context) -> Option<(Option<String>, Option<String>)> {
        let (query, _) = self.switcher.clone()?;
        struct Hit {
            prefix: &'static str,
            label: String,
            sub: String,
            target: (Option<String>, Option<String>),
        }
        let q = query.trim().to_lowercase();
        let mut hits: Vec<(u8, Hit)> = Vec::new();
        let score = |name: &str| -> Option<u8> {
            let n = name.to_lowercase();
            if q.is_empty() {
                Some(2)
            } else if n.starts_with(&q) {
                Some(0)
            } else if n.contains(&q) {
                Some(1)
            } else {
                None
            }
        };
        for c in &self.dms {
            let label = c.dm_label();
            if let Some(s) = score(&label) {
                hits.push((s, Hit { prefix: "@", label, sub: String::new(), target: (None, Some(c.id.clone())) }));
            }
        }
        if !q.is_empty() {
            for g in &self.guilds {
                if let Some(s) = score(&g.name) {
                    hits.push((s, Hit { prefix: "", label: g.name.clone(), sub: "サーバー".into(), target: (Some(g.id.clone()), None) }));
                }
                for c in self.channels.get(&g.id).into_iter().flatten().filter(|c| c.is_text()) {
                    let name = c.name.clone().unwrap_or_default();
                    if let Some(s) = score(&name) {
                        hits.push((s, Hit { prefix: "#", label: name, sub: g.name.clone(), target: (Some(g.id.clone()), Some(c.id.clone())) }));
                    }
                }
            }
        }
        hits.sort_by_key(|(s, _)| *s);
        hits.truncate(10);

        let (down, up, enter, esc) = ctx.input_mut(|i| {
            (
                i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown),
                i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp),
                i.consume_key(egui::Modifiers::NONE, egui::Key::Enter),
                i.consume_key(egui::Modifiers::NONE, egui::Key::Escape),
            )
        });
        let n = hits.len();
        let sw = self.switcher.as_mut().unwrap();
        if n > 0 {
            if down {
                sw.1 = (sw.1 + 1) % n;
            }
            if up {
                sw.1 = (sw.1 + n - 1) % n;
            }
        }
        sw.1 = sw.1.min(n.saturating_sub(1));
        let mut picked: Option<usize> = (enter && n > 0).then_some(sw.1);
        let modal = egui::Modal::new(egui::Id::new("quick_switcher"))
            .backdrop_color(Color32::from_black_alpha(170))
            .frame(Frame::new().fill(SIDEBAR()).corner_radius(8).stroke(Stroke::new(1.0_f32, BORDER())).inner_margin(Margin::same(16)))
            .show(ctx, |ui| {
                ui.set_width(540.0);
                ui.label(RichText::new("どこに行きますか？").font(medium_font(ui.ctx(), 18.0)).color(WHITE()));
                ui.add_space(10.0);
                let sw = self.switcher.as_mut().unwrap();
                let before = sw.0.clone();
                Frame::new().fill(BG()).corner_radius(8).stroke(Stroke::new(1.0_f32, DIVIDER())).inner_margin(Margin::symmetric(12, 0)).show(ui, |ui| {
                    let r = ui.add(
                        egui::TextEdit::singleline(&mut sw.0)
                            .id(egui::Id::new("switcher_input"))
                            .frame(false)
                            .desired_width(f32::INFINITY)
                            .font(FontId::proportional(18.0))
                            .text_color(TEXT())
                            .margin(Margin::symmetric(0, 12))
                            .hint_text(RichText::new("会話・チャンネル・サーバーを検索").color(FAINT())),
                    );
                    r.request_focus();
                });
                if sw.0 != before {
                    sw.1 = 0;
                }
                let sel = sw.1;
                ui.add_space(10.0);
                ui.label(RichText::new(if q.is_empty() { "最近の会話" } else { "検索結果" }).size(12.0).color(FAINT()));
                ui.add_space(4.0);
                if hits.is_empty() {
                    ui.add_space(8.0);
                    ui.label(RichText::new("見つかりませんでした").size(14.0).color(MUTED()));
                }
                for (i, (_, h)) in hits.iter().enumerate() {
                    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 36.0), Sense::click());
                    if i == sel || resp.hovered() {
                        ui.painter().rect_filled(rect, 6.0, if i == sel { SELECTED() } else { HOVER() });
                    }
                    let p = ui.painter().with_clip_rect(rect);
                    p.text(pos2(rect.left() + 12.0, rect.center().y), Align2::LEFT_CENTER, h.prefix, FontId::proportional(18.0), FAINT());
                    p.text(pos2(rect.left() + 32.0, rect.center().y), Align2::LEFT_CENTER, &h.label, medium_font(ui.ctx(), 15.0), if i == sel { WHITE() } else { TEXT() });
                    p.text(pos2(rect.right() - 12.0, rect.center().y), Align2::RIGHT_CENTER, &h.sub, FontId::proportional(12.0), FAINT());
                    if resp.clicked() {
                        picked = Some(i);
                    }
                }
                ui.add_space(10.0);
                ui.label(RichText::new("↑↓ で選択 · Enter で移動 · Esc で閉じる").size(11.0).color(FAINT()));
            });
        if esc || modal.should_close() {
            self.switcher = None;
        }
        let target = picked.and_then(|i| hits.into_iter().nth(i)).map(|(_, h)| h.target);
        if target.is_some() {
            self.switcher = None;
        }
        target
    }

    /// 削除の確認
    fn delete_confirm_ui(&mut self, ctx: &egui::Context) {
        let Some(m) = self.pending_delete.clone() else { return };
        let mut confirm = ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Enter));
        let mut close = false;
        let modal = egui::Modal::new(egui::Id::new("delete_modal"))
            .backdrop_color(Color32::from_black_alpha(170))
            .frame(Frame::new().fill(SIDEBAR()).corner_radius(8).stroke(Stroke::new(1.0_f32, BORDER())).inner_margin(Margin::same(20)))
            .show(ctx, |ui| {
                ui.set_width(440.0);
                ui.label(RichText::new("メッセージを削除").font(medium_font(ui.ctx(), 20.0)).color(WHITE()));
                ui.add_space(8.0);
                ui.label(RichText::new("本当にこのメッセージを削除しますか？").size(15.0).color(TEXT()));
                ui.add_space(12.0);
                Frame::new().fill(BG()).corner_radius(8).stroke(Stroke::new(1.0_f32, DIVIDER())).inner_margin(Margin::same(12)).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.label(RichText::new(author_name(&m)).font(medium_font(ui.ctx(), 15.0)).color(WHITE()));
                    ui.add(egui::Label::new(RichText::new(&m.content).size(14.0).color(TEXT())).wrap());
                });
                ui.add_space(10.0);
                ui.label(RichText::new("ヒント: Shift を押しながら削除すると、この確認を省けます").size(12.0).color(FAINT()));
                ui.add_space(16.0);
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if ui.add(egui::Button::new(RichText::new("削除").size(14.0).color(Color32::WHITE)).fill(RED()).corner_radius(8).min_size(vec2(96.0, 38.0))).clicked() {
                        confirm = true;
                    }
                    ui.add_space(8.0);
                    if ui.add(egui::Button::new(RichText::new("キャンセル").size(14.0).color(TEXT())).frame(false).min_size(vec2(96.0, 38.0))).clicked() {
                        close = true;
                    }
                });
            });
        if confirm {
            self.send(Cmd::Delete { channel_id: m.channel_id.clone(), id: m.id.clone() });
            self.pending_delete = None;
        } else if close || modal.should_close() {
            self.pending_delete = None;
        }
    }

    /// 戻る/進むで場所を復元する
    fn select(&mut self, guild: Option<String>, channel: Option<String>) {
        if let Some(gid) = &guild {
            if !self.channels.contains_key(gid) || !self.roles.contains_key(gid) {
                self.send(Cmd::LoadGuildChannels(gid.clone()));
            }
            if self.members_requested.insert(gid.clone()) {
                self.send(Cmd::LoadMembers(gid.clone()));
            }
        }
        self.sel_guild = guild;
        self.sel_channel = None;
        if let Some(c) = channel {
            self.open_channel(c);
        }
    }

    /// (チャンネル名, サーバー名)
    fn voice_place(&self, guild_id: Option<&str>, channel_id: &str) -> (String, String) {
        match guild_id {
            Some(g) => {
                let ch = self.channels.get(g).and_then(|l| l.iter().find(|c| c.id == channel_id)).and_then(|c| c.name.clone());
                let gn = self.guilds.iter().find(|x| x.id == g).map(|x| x.name.clone());
                (ch.unwrap_or_else(|| "ボイス".into()), gn.unwrap_or_default())
            }
            None => {
                let n = self.dms.iter().find(|c| c.id == channel_id).map(|c| c.dm_label()).unwrap_or_default();
                (n, "ダイレクトメッセージ".into())
            }
        }
    }

    /// 左下のマイク／ヘッドホン横の ⌄ メニュー
    fn audio_menu(&mut self, ui: &mut egui::Ui, input: bool) {
        ui.set_min_width(240.0);
        if self.audio_devs.is_none() {
            self.audio_devs = Some((voice::input_devices(), voice::output_devices()));
        }
        let (ins, outs) = self.audio_devs.clone().unwrap_or_default();
        let mut changed = false;
        ui.label(RichText::new(if input { "入力デバイス" } else { "出力デバイス" }).size(12.0).color(MUTED()));
        let (slot, list) = if input { (&mut self.settings.input_device, ins) } else { (&mut self.settings.output_device, outs) };
        egui::ScrollArea::vertical().max_height(220.0).show(ui, |ui| {
            changed |= ui.radio_value(slot, None, "デフォルト").changed();
            for d in list {
                changed |= ui.radio_value(slot, Some(d.clone()), d).changed();
            }
        });
        ui.separator();
        if input {
            ui.label(RichText::new("入力モード").size(12.0).color(MUTED()));
            changed |= ui.radio_value(&mut self.settings.ptt, false, "音声検出").changed();
            changed |= ui.radio_value(&mut self.settings.ptt, true, "プッシュトゥトーク").changed();
            ui.label(RichText::new("入力音量").size(12.0).color(MUTED()));
            changed |= ui.add(egui::Slider::new(&mut self.settings.in_gain, 0.0..=2.0).custom_formatter(|v, _| format!("{:.0}%", v * 100.0))).changed();
        } else {
            ui.label(RichText::new("出力音量").size(12.0).color(MUTED()));
            changed |= ui.add(egui::Slider::new(&mut self.settings.out_gain, 0.0..=2.0).custom_formatter(|v, _| format!("{:.0}%", v * 100.0))).changed();
        }
        ui.separator();
        if ui.button("音声設定…").clicked() {
            self.settings_open = true;
            self.settings_tab = 2;
            ui.memory_mut(|m| m.close_popup());
        }
        if changed {
            self.apply_voice_settings();
        }
        if changed {
            save_settings(&self.settings);
        }
    }

    fn incoming_call_ui(&self, ctx: &egui::Context, join: &mut Option<(Option<String>, String, bool)>, decline: &mut Option<String>) {
        let Some(me) = &self.me else { return };
        let here = self.voice_channel().map(|(_, c, _)| c.clone());
        let Some((cid, _)) = self.calls.iter().find(|(cid, r)| r.contains(&me.id) && here.as_deref() != Some(cid.as_str())) else {
            return;
        };
        let ch = self.dms.iter().find(|c| &c.id == cid);
        let name = ch.map(|c| c.dm_label()).unwrap_or_else(|| "不明".into());
        egui::Area::new(egui::Id::new("incoming_call"))
            .anchor(Align2::CENTER_TOP, vec2(0.0, 72.0))
            .order(egui::Order::Foreground)
            .show(ctx, |ui| {
                Frame::new()
                    .fill(RAIL())
                    .corner_radius(12)
                    .inner_margin(Margin::same(20))
                    .shadow(egui::epaint::Shadow { offset: [0, 4], blur: 24, spread: 0, color: Color32::from_black_alpha(160) })
                    .show(ui, |ui| {
                        ui.set_width(220.0);
                        ui.vertical_centered(|ui| {
                            let (av, _) = ui.allocate_exact_size(vec2(80.0, 80.0), Sense::hover());
                            let t = ui.input(|i| i.time) as f32;
                            let pulse = 4.0 + 3.0 * (t * 4.0).sin().abs();
                            ui.painter().circle_stroke(av.center(), 40.0 + pulse, Stroke::new(2.0_f32, GREEN().gamma_multiply(0.6)));
                            paint_icon(ui, av, ch.and_then(dm_icon_url).as_deref(), &name, 40.0, BLURPLE());
                            ui.ctx().request_repaint_after(std::time::Duration::from_millis(50));
                            ui.add_space(12.0);
                            ui.label(RichText::new(&name).size(18.0).strong().color(WHITE()));
                            ui.label(RichText::new("着信中…").size(13.0).color(MUTED()));
                            ui.add_space(16.0);
                            ui.allocate_ui_with_layout(vec2(56.0 * 2.0 + 40.0, 56.0), Layout::left_to_right(Align::Center), |ui| {
                                ui.spacing_mut().item_spacing.x = 40.0;
                                if round_button(ui, 56.0, Icon::Hangup, RED(), WHITE(), false).on_hover_text("拒否").clicked() {
                                    *decline = Some(cid.clone());
                                }
                                if round_button(ui, 56.0, Icon::Phone, GREEN(), WHITE(), false).on_hover_text("応答").clicked() {
                                    *join = Some((None, cid.clone(), false));
                                }
                            });
                        });
                    });
            });
    }

    // ---- 音声 ----

    fn apply_voice_settings(&self) {
        let (v, s) = (&self.voice, &self.settings);
        v.mute.store(s.self_mute, Relaxed);
        v.deaf.store(s.self_deaf, Relaxed);
        v.ptt.store(s.ptt, Relaxed);
        v.set_vad_threshold(db_to_lin(s.vad_db));
        v.set_in_gain(s.in_gain);
        v.set_out_gain(s.out_gain);
        let mut changed = false;
        for (slot, val) in [(&v.input_device, &s.input_device), (&v.output_device, &s.output_device)] {
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

    fn voice_changed(&mut self) {
        self.apply_voice_settings();
        save_settings(&self.settings);
        self.send(Cmd::VoiceFlags);
    }

    fn toggle_mute(&mut self) {
        if self.settings.self_deaf {
            self.settings.self_deaf = false;
            self.settings.self_mute = false;
        } else {
            self.settings.self_mute = !self.settings.self_mute;
        }
        self.voice_changed();
    }

    fn toggle_deaf(&mut self) {
        self.settings.self_deaf = !self.settings.self_deaf;
        self.voice_changed();
    }

    fn mic_off(&self) -> bool {
        self.settings.self_mute || self.settings.self_deaf
    }

    fn join_voice(&mut self, guild_id: Option<String>, channel_id: String, ring: bool) {
        self.voice_status = Some(VoiceStatus::Connecting { guild_id: guild_id.clone(), channel_id: channel_id.clone() });
        self.send(Cmd::JoinVoice { guild_id, channel_id, ring });
    }

    fn leave_voice(&mut self) {
        self.voice_status = None;
        self.send(Cmd::LeaveVoice);
    }

    /// 接続中（または接続しようとしている）チャンネル
    fn voice_channel(&self) -> Option<(&Option<String>, &String, bool)> {
        match self.voice_status.as_ref()? {
            VoiceStatus::Connecting { guild_id, channel_id } => Some((guild_id, channel_id, false)),
            VoiceStatus::Connected { guild_id, channel_id } => Some((guild_id, channel_id, true)),
            _ => None,
        }
    }

    /// channel_id → そこにいる user_id（並び順固定）。voice_states が変わったら作り直す
    fn reindex_voice(&mut self) {
        self.voice_index.clear();
        for v in self.voice_states.values() {
            if let Some(c) = &v.channel_id {
                self.voice_index.entry(c.clone()).or_default().push(v.user_id.clone());
            }
        }
        for l in self.voice_index.values_mut() {
            l.sort();
        }
    }

    fn find_user(&self, id: &str) -> Option<User> {
        if let Some(me) = self.me.as_ref().filter(|m| m.id == id) {
            return Some(me.clone());
        }
        if let Some(u) = self.voice_states.get(id).and_then(|v| v.user.clone()) {
            return Some(u);
        }
        self.dms.iter().flat_map(|c| c.recipients.iter()).find(|u| u.id == id).cloned()
    }

    fn is_speaking(&self, id: &str) -> bool {
        if self.me.as_ref().is_some_and(|m| m.id == id) {
            return self.voice.transmitting.load(Relaxed);
        }
        self.voice.speaking.lock().unwrap().contains(id)
    }

    /// あるボイスチャンネルにいる人
    fn voice_members<'a>(&'a self, channel_id: &str) -> impl Iterator<Item = &'a VoiceState> + 'a {
        self.voice_index
            .get(channel_id)
            .into_iter()
            .flatten()
            .filter_map(|id| self.voice_states.get(id))
    }

    fn set_status(&mut self, st: String) {
        self.send(Cmd::SetStatus(st.clone()));
        self.settings.status = st;
        save_settings(&self.settings);
    }

    fn open_profile(&mut self, user: User, anchor: Rect, above: bool) {
        let guild_id = self.sel_guild.clone();
        let key = net::profile_key(&user.id, guild_id.as_deref());
        if !self.profiles.contains_key(&key) {
            self.profiles.insert(key, ProfileState::Loading);
            self.send(Cmd::LoadProfile { user_id: user.id.clone(), guild_id: guild_id.clone() });
        }
        let opened = self.ctx.cumulative_pass_nr();
        self.popup = Some(ProfilePopup { user, guild_id, anchor, above, opened });
    }

    // ---- プロフィールのポップアップ ----
    fn profile_popup_ui(&mut self, ctx: &egui::Context) {
        let Some(pp) = self.popup.clone() else { return };
        const W: f32 = 300.0;
        const BANNER_H: f32 = 120.0;
        let user = &pp.user;
        let key = net::profile_key(&user.id, pp.guild_id.as_deref());
        let state = self.profiles.get(&key);
        let data = match state {
            Some(ProfileState::Ready(d)) => Some(d),
            _ => None,
        };
        let is_me = self.me.as_ref().is_some_and(|m| m.id == user.id);
        let (status, activity) = if is_me {
            (self.settings.status.clone(), None)
        } else {
            match self.presences.get(&user.id) {
                Some(p) => (p.status.clone(), p.activity.clone()),
                None => ("offline".to_string(), None),
            }
        };
        let dm = self.dms.iter().find(|c| c.recipients.len() == 1 && c.recipients[0].id == user.id).map(|c| c.id.clone());

        let (pivot, pos) = if pp.above {
            (Align2::LEFT_BOTTOM, pos2(pp.anchor.left(), pp.anchor.top() - 10.0))
        } else {
            (Align2::LEFT_TOP, pos2(pp.anchor.right() + 12.0, pp.anchor.top()))
        };
        let mut new_status: Option<String> = None;
        let mut open_dm: Option<String> = None;

        let area = egui::Area::new(egui::Id::new("profile_popup"))
            .order(egui::Order::Foreground)
            .pivot(pivot)
            .fixed_pos(pos)
            .constrain(true)
            .show(ctx, |ui| {
                Frame::new()
                    .fill(SIDEBAR())
                    .corner_radius(8)
                    .stroke(Stroke::new(1.0_f32, BORDER()))
                    .shadow(egui::Shadow { offset: [0, 8], blur: 24, spread: 0, color: Color32::from_black_alpha(140) })
                    .show(ui, |ui| {
                        ui.set_width(W);
                        ui.spacing_mut().item_spacing = vec2(0.0, 0.0);

                        // バナー（画像 → 指定色 → 名前から決めた色の順）
                        let (banner, _) = ui.allocate_exact_size(vec2(W, BANNER_H), Sense::hover());
                        let top = CornerRadius { nw: 8, ne: 8, sw: 0, se: 0 };
                        let color = data.and_then(|d| d.accent_color).map(rgb).unwrap_or_else(|| darken(name_color(&user.id)));
                        ui.painter().rect_filled(banner, top, color);
                        if let Some(h) = data.and_then(|d| d.banner.as_ref()) {
                            let url = format!("https://cdn.discordapp.com/banners/{}/{}.png?size=600", user.id, h);
                            let img = egui::Image::new(url);
                            if let Ok(egui::load::TexturePoll::Ready { texture }) = img.load_for_size(ui.ctx(), banner.size()) {
                                egui::Image::from_texture(texture).corner_radius(top).paint_at(ui, banner);
                            }
                        }

                        // 大きいアバター（バナーにかぶせる）
                        let av = Rect::from_min_size(pos2(banner.left() + 16.0, banner.bottom() - 44.0), vec2(80.0, 80.0));
                        ui.painter().circle_filled(av.center(), 46.0, SIDEBAR());
                        paint_icon(ui, av, Some(&user_avatar_url(user)), user.display(), 40.0, BLURPLE());
                        paint_status(ui.painter(), av, &status, SIDEBAR());
                        ui.add_space(44.0);

                        Frame::new().inner_margin(Margin { left: 16, right: 16, top: 8, bottom: 16 }).show(ui, |ui| {
                            ui.set_width(W - 32.0);
                            ui.spacing_mut().item_spacing.y = 4.0;
                            let name = data.and_then(|d| d.nick.as_deref()).unwrap_or(user.display());
                            ui.label(RichText::new(name).size(20.0).color(WHITE()));
                            let mut sub = user.username.clone();
                            if let Some(pr) = data.and_then(|d| d.pronouns.as_deref()) {
                                sub += &format!(" • {pr}");
                            }
                            ui.label(RichText::new(sub).size(14.0).color(TEXT()));
                            ui.label(RichText::new(status_label(&status)).size(12.0).color(MUTED()));
                            if let Some(act) = &activity {
                                ui.add_space(4.0);
                                Frame::new().fill(RAIL()).corner_radius(8).inner_margin(Margin::symmetric(10, 6)).show(ui, |ui| {
                                    ui.add(egui::Label::new(RichText::new(act).size(13.0).color(TEXT())).wrap());
                                });
                            }
                            ui.add_space(10.0);

                            Frame::new().fill(RAIL()).corner_radius(8).inner_margin(Margin::same(12)).show(ui, |ui| {
                                ui.set_width(ui.available_width());
                                ui.spacing_mut().item_spacing.y = 4.0;
                                let heading = |ui: &mut egui::Ui, t: &str| {
                                    ui.label(RichText::new(t).size(12.0).color(WHITE()));
                                };
                                match state {
                                    Some(ProfileState::Loading) | None => {
                                        ui.vertical_centered(|ui| ui.spinner());
                                    }
                                    Some(ProfileState::Failed) => {
                                        ui.label(RichText::new("詳しいプロフィールを取得できませんでした").size(13.0).color(MUTED()));
                                    }
                                    Some(ProfileState::Ready(_)) => {}
                                }
                                if let Some(bio) = data.and_then(|d| d.bio.as_deref()) {
                                    heading(ui, "自己紹介");
                                    ui.add(egui::Label::new(RichText::new(bio).size(14.0).color(TEXT())).wrap());
                                    ui.add_space(8.0);
                                }
                                heading(ui, "Discordメンバー歴");
                                ui.label(RichText::new(snowflake_date(&user.id)).size(14.0).color(TEXT()));
                                if let Some(j) = data.and_then(|d| d.joined_at.as_deref()).and_then(local_dt) {
                                    ui.add_space(8.0);
                                    heading(ui, "サーバーメンバー歴");
                                    ui.label(RichText::new(j.format("%Y年%-m月%-d日").to_string()).size(14.0).color(TEXT()));
                                }
                                if let Some(roles) = data.map(|d| &d.roles).filter(|r| !r.is_empty()) {
                                    ui.add_space(8.0);
                                    heading(ui, "ロール");
                                    ui.horizontal_wrapped(|ui| {
                                        ui.spacing_mut().item_spacing = vec2(4.0, 4.0);
                                        for (name, c) in roles {
                                            let col = if *c == 0 { ST_OFFLINE() } else { rgb(*c) };
                                            Frame::new().fill(SELECTED()).corner_radius(4).inner_margin(Margin::symmetric(6, 3)).show(ui, |ui| {
                                                ui.horizontal(|ui| {
                                                    ui.spacing_mut().item_spacing.x = 5.0;
                                                    let (d, _) = ui.allocate_exact_size(vec2(10.0, 10.0), Sense::hover());
                                                    ui.painter().circle_filled(d.center(), 5.0, col);
                                                    ui.label(RichText::new(name).size(12.0).color(TEXT()));
                                                });
                                            });
                                        }
                                    });
                                }
                            });

                            if is_me {
                                ui.add_space(8.0);
                                Frame::new().fill(RAIL()).corner_radius(8).inner_margin(Margin::same(6)).show(ui, |ui| {
                                    ui.set_width(ui.available_width());
                                    ui.spacing_mut().item_spacing.y = 1.0;
                                    for (k, label) in STATUSES {
                                        let (resp, r, _) = row_frame(ui, 32.0, status == k, false);
                                        let ring = if status == k { SELECTED() } else if resp.hovered() { HOVER() } else { RAIL() };
                                        status_mark(ui.painter(), pos2(r.left() + 18.0, r.center().y), 5.0, k, ring);
                                        ui.painter().text(pos2(r.left() + 34.0, r.center().y), Align2::LEFT_CENTER, label, FontId::proportional(14.0), TEXT());
                                        if resp.clicked() {
                                            new_status = Some(k.to_string());
                                        }
                                    }
                                });
                            } else if let Some(cid) = &dm {
                                ui.add_space(10.0);
                                let btn = egui::Button::new(RichText::new("メッセージを送る").color(Color32::WHITE))
                                    .fill(BLURPLE())
                                    .min_size(vec2(ui.available_width(), 36.0));
                                if ui.add(btn).clicked() {
                                    open_dm = Some(cid.clone());
                                }
                            }
                        });
                    });
            });

        // 外側クリック・Esc・操作したら閉じる
        let pressed_outside = ctx.input(|i| {
            i.pointer.any_pressed() && i.pointer.interact_pos().is_some_and(|p| !area.response.rect.contains(p))
        });
        let esc = ctx.input(|i| i.key_pressed(egui::Key::Escape));
        if (pressed_outside && ctx.cumulative_pass_nr() > pp.opened) || esc || new_status.is_some() || open_dm.is_some() {
            self.popup = None;
        }
        if let Some(st) = new_status {
            self.set_status(st);
        }
        if let Some(cid) = open_dm {
            self.sel_guild = None;
            self.open_channel(cid);
        }
    }

    // ---- 設定画面 ----
    fn settings_ui(&mut self, ctx: &egui::Context) {
        let mut close = !self.binding_ptt && ctx.input(|i| i.key_pressed(egui::Key::Escape));
        let mut logout = false;
        let mut changed = false;

        egui::SidePanel::left("set_nav")
            .resizable(false)
            .exact_width(240.0)
            .show_separator_line(false)
            .frame(Frame::new().fill(SIDEBAR()).inner_margin(Margin { top: 56, ..Default::default() }))
            .show(ctx, |ui| {
                ui.spacing_mut().item_spacing.y = 2.0;
                ui.label(RichText::new("    ユーザー設定").size(12.0).color(MUTED()));
                ui.add_space(4.0);
                for (i, name) in ["マイアカウント", "表示", "音声", "テーマ"].into_iter().enumerate() {
                    if nav_row(ui, self.settings_tab == i, name, None).clicked() {
                        self.settings_tab = i;
                    }
                }
                ui.add_space(8.0);
                ui.separator();
                ui.add_space(8.0);
                if nav_row(ui, false, "ログアウト", Some(RED())).clicked() {
                    logout = true;
                }
            });

        egui::CentralPanel::default()
            .frame(Frame::new().fill(CHAT()).inner_margin(Margin { left: 40, right: 24, top: 56, bottom: 24 }))
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    let title = ["マイアカウント", "表示", "音声", "テーマ"][self.settings_tab];
                    ui.label(RichText::new(title).size(20.0).color(WHITE()));
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if icon_button(ui, 36.0, Icon::Close).on_hover_text("閉じる (Esc)").clicked()
                        {
                            close = true;
                        }
                    });
                });
                ui.add_space(20.0);
                ui.set_max_width(660.0);

                match self.settings_tab {
                    0 => {
                        if let Some(me) = &self.me {
                            Frame::new().fill(RAIL()).corner_radius(8).inner_margin(Margin::same(16)).show(ui, |ui| {
                                ui.set_width(ui.available_width());
                                ui.horizontal(|ui| {
                                    let (av, _) = ui.allocate_exact_size(vec2(80.0, 80.0), Sense::hover());
                                    paint_icon(ui, av, Some(&user_avatar_url(me)), me.display(), 40.0, BLURPLE());
                                    ui.add_space(12.0);
                                    ui.vertical(|ui| {
                                        ui.add_space(14.0);
                                        ui.label(RichText::new(me.display()).size(20.0).color(WHITE()));
                                        ui.label(RichText::new(&me.username).size(14.0).color(MUTED()));
                                    });
                                });
                                ui.add_space(12.0);
                                Frame::new().fill(SIDEBAR()).corner_radius(8).inner_margin(Margin::same(16)).show(ui, |ui| {
                                    ui.set_width(ui.available_width());
                                    for (k, v) in [("表示名", me.display()), ("ユーザー名", &me.username), ("ユーザーID", &me.id)] {
                                        ui.label(RichText::new(k).size(12.0).color(MUTED()));
                                        ui.label(RichText::new(v).color(TEXT()));
                                        ui.add_space(10.0);
                                    }
                                });
                            });
                        }
                        ui.add_space(24.0);
                        ui.label(RichText::new("トークン").size(12.0).color(MUTED()));
                        let saved = token_path().exists();
                        ui.label(
                            RichText::new(if saved { "このPCに保存されています (~/.config/riscord/token)" } else { "保存されていません" })
                                .color(TEXT()),
                        );
                        if saved {
                            ui.add_space(6.0);
                            if ui.button("保存したトークンを削除").clicked() {
                                let _ = std::fs::remove_file(token_path());
                            }
                        }
                    }
                    2 => self.voice_settings_ui(ui),
                    3 => changed |= self.theme_settings_ui(ui),
                    _ => {
                        ui.spacing_mut().slider_width = 320.0;
                        ui.label(RichText::new("UIの拡大率").size(12.0).color(MUTED()));
                        let mut zoom = self.settings.zoom;
                        let r = ui.add(
                            egui::Slider::new(&mut zoom, 0.7..=1.6)
                                .step_by(0.05)
                                .custom_formatter(|v, _| format!("{:.0}%", v * 100.0)),
                        );
                        self.settings.zoom = zoom;
                        if r.drag_stopped() || (r.changed() && !r.dragged()) {
                            ctx.set_zoom_factor(zoom);
                            changed = true;
                        }
                        ui.add_space(18.0);

                        ui.label(RichText::new("メッセージの文字サイズ").size(12.0).color(MUTED()));
                        if ui.add(egui::Slider::new(&mut self.settings.font_size, 12.0..=22.0).step_by(1.0).custom_formatter(|v, _| format!("{v:.0}px"))).changed() {
                            changed = true;
                        }
                        ui.add_space(18.0);

                        ui.label(RichText::new("スクロールの速さ（マウスホイール）").size(12.0).color(MUTED()));
                        if ui.add(egui::Slider::new(&mut self.settings.wheel_speed, 0.5..=6.0).step_by(0.25).custom_formatter(|v, _| format!("{v:.2}倍"))).changed() {
                            changed = true;
                        }
                        ui.add_space(10.0);
                        ui.label(RichText::new("スクロールの速さ（タッチパッド）").size(12.0).color(MUTED()));
                        if ui.add(egui::Slider::new(&mut self.settings.touchpad_speed, 0.5..=6.0).step_by(0.25).custom_formatter(|v, _| format!("{v:.2}倍"))).changed() {
                            changed = true;
                        }
                        ui.add_space(18.0);

                        ui.label(RichText::new("メッセージ表示").size(12.0).color(MUTED()));
                        changed |= ui.radio_value(&mut self.settings.compact, false, "標準（アバター付き）").changed();
                        changed |= ui.radio_value(&mut self.settings.compact, true, "コンパクト（1行表示）").changed();
                        ui.add_space(18.0);

                        if ui.button("初期設定に戻す").clicked() {
                            self.settings = Settings::default();
                            ctx.set_zoom_factor(1.0);
                            changed = true;
                        }
                    }
                }
            });

        if changed {
            save_settings(&self.settings);
        }
        // マイクテストは音声タブを開いている間だけ（接続中は本物の入力を使う）
        let want_test = !close && self.settings_tab == 2 && self.voice_channel().is_none();
        if want_test && self.mic_test.is_none() {
            self.mic_test = Some(voice::MicTest::start(self.voice.clone()));
        } else if !want_test {
            self.mic_test = None;
        }
        if close {
            self.settings_open = false;
            self.binding_ptt = false;
        }
        if logout {
            self.logout();
        }
    }

    /// 設定 → テーマ。変えたら true（保存する）
    fn theme_settings_ui(&mut self, ui: &mut egui::Ui) -> bool {
        let mut changed = false;
        if let Some(p) = self.bg_pick.as_ref().and_then(|rx| rx.try_recv().ok()) {
            self.bg_pick = None;
            if let Some(p) = p.first() {
                self.settings.background.image = Some(p.display().to_string());
                changed = true;
            }
        }
        let dir = themes_dir();
        let files = theme::list_files(&dir);
        egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
            ui.label(RichText::new("テーマ").size(12.0).color(MUTED()));
            let label = |key: &str| -> String {
                if let Some(b) = key.strip_prefix("builtin:") {
                    return theme::BUILTIN.iter().find(|t| t.0 == b).map_or(key.to_string(), |t| t.1.to_string());
                }
                match files.iter().find(|f| f.0 == key) {
                    Some((name, Some(f))) => match (&f.name, &f.author) {
                        (Some(n), Some(a)) => format!("{n}（{a}）"),
                        (Some(n), None) => n.clone(),
                        _ => name.clone(),
                    },
                    _ => key.to_string(),
                }
            };
            egui::ComboBox::from_id_salt("theme_pick").width(320.0).selected_text(label(&self.settings.theme)).show_ui(ui, |ui| {
                for (k, name, _) in theme::BUILTIN {
                    changed |= ui.selectable_value(&mut self.settings.theme, format!("builtin:{k}"), *name).changed();
                }
                if !files.is_empty() {
                    ui.separator();
                }
                for (f, _) in &files {
                    let text = label(f);
                    changed |= ui.selectable_value(&mut self.settings.theme, f.clone(), text).changed();
                }
            });
            if let Some(e) = &self.theme_error {
                ui.label(RichText::new(format!("テーマを読めませんでした: {e}")).size(12.0).color(RED()));
            }
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                if ui.button("テーマフォルダを開く").clicked() {
                    open_folder(&dir);
                }
                if ui.button("今の配色をテーマとして書き出す").on_hover_text("themes/ に my-theme.toml を作ります。編集して保存するとすぐ反映されます").clicked() {
                    let _ = std::fs::create_dir_all(&dir);
                    let mut path = dir.join("my-theme.toml");
                    let mut n = 2;
                    while path.exists() {
                        path = dir.join(format!("my-theme-{n}.toml"));
                        n += 1;
                    }
                    let text = theme::export("マイテーマ", &theme::current_colors(), &self.settings.background);
                    match std::fs::write(&path, text) {
                        Ok(()) => {
                            self.settings.theme = path.file_name().unwrap().to_string_lossy().into_owned();
                            // 書き出した色はファイルに入ったので、上書きは消す
                            self.settings.color_overrides.clear();
                            changed = true;
                        }
                        Err(e) => self.theme_error = Some(format!("{}: {e}", path.display())),
                    }
                }
            });
            ui.label(RichText::new(format!("テーマファイル（.toml）を {} に置くとここに出ます。保存するとすぐ反映されます", dir.display())).size(12.0).color(FAINT()));
            ui.add_space(18.0);

            ui.label(RichText::new("チャット欄の背景画像").size(12.0).color(MUTED()));
            ui.horizontal(|ui| {
                let mut img = self.settings.background.image.clone().unwrap_or_default();
                let r = ui.add(egui::TextEdit::singleline(&mut img).desired_width(360.0).hint_text("画像のパスか https:// の URL（空ならテーマの指定）"));
                if r.changed() {
                    self.settings.background.image = Some(img).filter(|s| !s.is_empty());
                }
                if r.lost_focus() {
                    changed = true;
                }
                if ui.add_enabled(self.bg_pick.is_none(), egui::Button::new("選ぶ…")).clicked() {
                    self.bg_pick = Some(spawn_file_dialog(ui.ctx(), "背景画像を選ぶ", false, Some(("画像", &["png", "jpg", "jpeg", "webp", "gif"]))));
                }
            });
            let mut op = self.settings.background.opacity.unwrap_or(0.35);
            if ui.add(egui::Slider::new(&mut op, 0.0..=1.0).custom_formatter(|v, _| format!("{:.0}%", v * 100.0)).text("画像の見え具合")).changed() {
                self.settings.background.opacity = Some(op);
                changed = true;
            }
            ui.add_space(18.0);

            ui.horizontal(|ui| {
                ui.label(RichText::new("色の調整（テーマより優先）").size(12.0).color(MUTED()));
                if !self.settings.color_overrides.is_empty() && ui.small_button("調整をすべて消す").clicked() {
                    self.settings.color_overrides.clear();
                    changed = true;
                }
            });
            ui.add_space(4.0);
            let current = theme::current_colors();
            egui::Grid::new("theme_colors").num_columns(3).spacing(vec2(12.0, 6.0)).show(ui, |ui| {
                for ((key, label, _), c) in theme::KEYS.iter().zip(current) {
                    let mut col = c;
                    if ui.color_edit_button_srgba(&mut col).changed() {
                        self.settings.color_overrides.insert(key.to_string(), theme::format_color(col));
                        changed = true;
                    }
                    ui.label(RichText::new(*label).color(TEXT()));
                    if self.settings.color_overrides.contains_key(*key) {
                        if ui.small_button("戻す").clicked() {
                            self.settings.color_overrides.remove(*key);
                            changed = true;
                        }
                    } else {
                        ui.label("");
                    }
                    ui.end_row();
                }
            });
        });
        if changed {
            self.apply_theme(false);
        }
        changed
    }

    fn voice_settings_ui(&mut self, ui: &mut egui::Ui) {
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(50));
        ui.spacing_mut().slider_width = 320.0;
        if self.audio_devs.is_none() {
            self.audio_devs = Some((voice::input_devices(), voice::output_devices()));
        }
        let (ins, outs) = self.audio_devs.clone().unwrap_or_default();
        let mut changed = false;

        ui.columns(2, |cols| {
            for (i, (title, slot, list)) in [
                ("入力デバイス", &mut self.settings.input_device, ins),
                ("出力デバイス", &mut self.settings.output_device, outs),
            ]
            .into_iter()
            .enumerate()
            {
                let ui = &mut cols[i];
                ui.label(RichText::new(title).size(12.0).color(MUTED()));
                egui::ComboBox::from_id_salt(title)
                    .width(ui.available_width() - 8.0)
                    .selected_text(slot.clone().unwrap_or_else(|| "デフォルト".into()))
                    .show_ui(ui, |ui| {
                        changed |= ui.selectable_value(slot, None, "デフォルト").changed();
                        for d in list {
                            changed |= ui.selectable_value(slot, Some(d.clone()), d).changed();
                        }
                    });
            }
        });
        if ui.small_button("デバイスを再検出").clicked() {
            self.audio_devs = None;
        }
        ui.add_space(18.0);

        ui.columns(2, |cols| {
            cols[0].label(RichText::new("入力音量").size(12.0).color(MUTED()));
            changed |= cols[0].add(egui::Slider::new(&mut self.settings.in_gain, 0.0..=2.0).custom_formatter(|v, _| format!("{:.0}%", v * 100.0))).changed();
            cols[1].label(RichText::new("出力音量").size(12.0).color(MUTED()));
            changed |= cols[1].add(egui::Slider::new(&mut self.settings.out_gain, 0.0..=2.0).custom_formatter(|v, _| format!("{:.0}%", v * 100.0))).changed();
        });
        ui.add_space(18.0);

        ui.label(RichText::new("入力モード").size(12.0).color(MUTED()));
        changed |= ui.radio_value(&mut self.settings.ptt, false, "音声検出").changed();
        changed |= ui.radio_value(&mut self.settings.ptt, true, "プッシュトゥトーク").changed();
        ui.add_space(18.0);

        if self.settings.ptt {
            ui.label(RichText::new("ショートカット").size(12.0).color(MUTED()));
            let text = if self.binding_ptt {
                "キーを押してください…（Escで取消）".to_string()
            } else {
                self.settings.ptt_key.clone().unwrap_or_else(|| "未設定".into())
            };
            if ui.add(egui::Button::new(RichText::new(text).size(15.0)).min_size(vec2(240.0, 36.0))).clicked() {
                self.binding_ptt = true;
            }
            if self.binding_ptt {
                let pressed = ui.input(|i| {
                    i.events.iter().find_map(|e| match e {
                        egui::Event::Key { key, pressed: true, repeat: false, .. } => Some(*key),
                        _ => None,
                    })
                });
                if let Some(k) = pressed {
                    if k != egui::Key::Escape {
                        self.settings.ptt_key = Some(k.name().to_string());
                        changed = true;
                    }
                    self.binding_ptt = false;
                }
            }
            ui.label(RichText::new("このウィンドウが前面にあるときだけ有効です").size(12.0).color(MUTED()));
            ui.add_space(18.0);
        }

        // 入力感度（レベルメーターとしきい値）
        ui.label(RichText::new(if self.settings.ptt { "入力レベル" } else { "入力感度" }).size(12.0).color(MUTED()));
        let level = self.voice.level();
        let thr = db_to_lin(self.settings.vad_db);
        let frac = |db: f32| ((db + 80.0) / 80.0).clamp(0.0, 1.0);
        let (r, _) = ui.allocate_exact_size(vec2(320.0, 10.0), Sense::hover());
        let p = ui.painter();
        p.rect_filled(r, 5.0, INPUT());
        let lv = Rect::from_min_size(r.min, vec2(r.width() * frac(lin_to_db(level)), r.height()));
        let color = if self.settings.ptt || level >= thr { GREEN() } else { ORANGE() };
        p.rect_filled(lv, 5.0, color);
        if !self.settings.ptt {
            let x = r.left() + r.width() * frac(self.settings.vad_db);
            p.vline(x, r.y_range().expand(3.0), Stroke::new(2.0_f32, WHITE()));
            ui.add_space(4.0);
            changed |= ui.add(egui::Slider::new(&mut self.settings.vad_db, -80.0..=0.0).step_by(1.0).custom_formatter(|v, _| format!("{v:.0} dB"))).changed();
            ui.label(RichText::new("話したときにバーが緑になれば送信されます").size(12.0).color(MUTED()));
        }

        if changed {
            self.apply_voice_settings();
        }
        if changed {
            save_settings(&self.settings);
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Icon {
    Gear,
    Members,
    Close,
    Mic,
    Headset,
    Phone,
    Hangup,
    Speaker,
    Video,
    Signal,
    Chevron,
    ArrowLeft,
    ArrowRight,
    Plus,
    Reply,
    Pencil,
    Copy,
    Trash,
    Profile,
    Upload,
    Clipboard,
}

/// 入力欄の左の＋で開くメニューの項目
enum PlusAction {
    Upload,
    PasteImage,
}

/// ＋メニュー。押された項目を返す
fn plus_menu(ui: &mut egui::Ui) -> Option<PlusAction> {
    let mut out = None;
    ui.spacing_mut().item_spacing.y = 2.0;
    for (icon, label, action) in [
        (Icon::Upload, "ファイルをアップロード", PlusAction::Upload),
        (Icon::Clipboard, "画像を貼り付け", PlusAction::PasteImage),
    ] {
        let (rect, resp) = ui.allocate_exact_size(vec2(240.0, 40.0), Sense::click());
        let hovered = resp.hovered();
        let p = ui.painter();
        if hovered {
            p.rect_filled(rect, 6.0, SELECTED());
        }
        let color = if hovered { WHITE() } else { TEXT() };
        paint_glyph(p, Rect::from_center_size(pos2(rect.left() + 22.0, rect.center().y), vec2(22.0, 22.0)), icon, color);
        p.text(pos2(rect.left() + 44.0, rect.center().y), Align2::LEFT_CENTER, label, medium_font(ui.ctx(), 15.0), color);
        if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
            out = Some(action);
        }
    }
    out
}

/// フォントに無い記号を使わず、図形で描くアイコンボタン
fn icon_button(ui: &mut egui::Ui, size: f32, icon: Icon) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(size, size), Sense::click());
    let hovered = resp.hovered();
    let color = if hovered { WHITE() } else { MUTED() };
    let p = ui.painter();
    if hovered && icon != Icon::Close {
        p.rect_filled(rect, 8.0, SELECTED());
    }
    paint_glyph(p, rect.shrink(size * 0.12), icon, color);
    resp
}

fn arc_points(c: egui::Pos2, r: f32, a0: f32, a1: f32) -> Vec<egui::Pos2> {
    let n = 16;
    (0..=n).map(|i| {
        let a = a0 + (a1 - a0) * i as f32 / n as f32;
        c + vec2(a.cos(), a.sin()) * r
    })
    .collect()
}

/// 受話器。angle = 回転（ラジアン）
fn paint_handset(p: &egui::Painter, c: egui::Pos2, s: f32, angle: f32, color: Color32) {
    use std::f32::consts::PI;
    let r = s * 0.30;
    let (a0, a1) = (angle + PI * 0.62, angle + PI * 1.38);
    let pts = arc_points(c, r, a0, a1);
    p.add(Shape::line(pts, Stroke::new(s * 0.13, color)));
    for a in [a0, a1] {
        let d = vec2(a.cos(), a.sin());
        let t = vec2(-d.y, d.x) * if a == a0 { -1.0 } else { 1.0 };
        let e = c + d * r;
        p.add(Shape::line(vec![e - t * s * 0.02, e + t * s * 0.14], Stroke::new(s * 0.17, color)));
    }
}

fn paint_glyph(p: &egui::Painter, rect: Rect, icon: Icon, color: Color32) {
    use std::f32::consts::{PI, TAU};
    let c = rect.center();
    let s = rect.width().min(rect.height());
    let thin = Stroke::new((s * 0.08).max(1.5), color);
    match icon {
        Icon::Members => {
            for x in [-0.2_f32, 0.2] {
                let head = c + vec2(s * x, -s * 0.18);
                p.circle_filled(head, s * 0.13, color);
                let body = Rect::from_center_size(c + vec2(s * x, s * 0.2), vec2(s * 0.32, s * 0.3));
                p.rect_filled(body, s * 0.14, color);
            }
        }
        Icon::Gear => {
            let r = s * 0.24;
            let stroke = Stroke::new(s * 0.1, color);
            for i in 0..8 {
                let a = i as f32 * TAU / 8.0;
                let d = vec2(a.cos(), a.sin());
                p.line_segment([c + d * r, c + d * r * 1.55], stroke);
            }
            p.circle_stroke(c, r, stroke);
            p.circle_filled(c, r * 0.35, color);
        }
        Icon::Close => {
            p.circle_stroke(c, s / 2.0 - 1.0, Stroke::new(2.0_f32, color));
            let h = s * 0.19;
            let stroke = Stroke::new(2.0_f32, color);
            p.line_segment([c + vec2(-h, -h), c + vec2(h, h)], stroke);
            p.line_segment([c + vec2(-h, h), c + vec2(h, -h)], stroke);
        }
        Icon::Mic => {
            let cap = Rect::from_center_size(c + vec2(0.0, -s * 0.12), vec2(s * 0.3, s * 0.48));
            p.rect_filled(cap, s * 0.15, color);
            let pts = arc_points(c + vec2(0.0, -s * 0.06), s * 0.27, 0.0, PI);
            p.add(Shape::line(pts, thin));
            p.line_segment([c + vec2(0.0, s * 0.21), c + vec2(0.0, s * 0.38)], thin);
        }
        Icon::Headset => {
            let r = s * 0.34;
            let cc = c + vec2(0.0, s * 0.04);
            p.add(Shape::line(arc_points(cc, r, PI, TAU), Stroke::new(s * 0.09, color)));
            for x in [-1.0f32, 1.0] {
                let edge = cc.x + x * r;
                let (l, rr) = if x < 0.0 { (edge - s * 0.05, edge + s * 0.14) } else { (edge - s * 0.14, edge + s * 0.05) };
                p.rect_filled(Rect::from_min_max(pos2(l, cc.y), pos2(rr, cc.y + s * 0.32)), s * 0.06, color);
            }
        }
        Icon::Phone => paint_handset(p, c, s, -PI / 4.0, color),
        Icon::Hangup => paint_handset(p, c + vec2(0.0, s * 0.18), s * 1.05, PI / 2.0, color),
        Icon::Speaker => {
            let body = Rect::from_center_size(c + vec2(-s * 0.26, 0.0), vec2(s * 0.16, s * 0.28));
            p.rect_filled(body, 1.0, color);
            p.add(Shape::convex_polygon(
                vec![body.right_top(), pos2(body.right() + s * 0.2, c.y - s * 0.3), pos2(body.right() + s * 0.2, c.y + s * 0.3), body.right_bottom()],
                color,
                Stroke::NONE,
            ));
            for (i, r) in [s * 0.2, s * 0.34].into_iter().enumerate() {
                let w = 0.75 - i as f32 * 0.1;
                p.add(Shape::line(arc_points(c + vec2(-s * 0.02, 0.0), r, -w, w), thin));
            }
        }
        Icon::Video => {
            let body = Rect::from_min_size(pos2(c.x - s * 0.4, c.y - s * 0.22), vec2(s * 0.55, s * 0.44));
            p.rect_filled(body, s * 0.08, color);
            p.add(Shape::convex_polygon(
                vec![pos2(body.right() + s * 0.04, c.y), pos2(c.x + s * 0.42, c.y - s * 0.2), pos2(c.x + s * 0.42, c.y + s * 0.2)],
                color,
                Stroke::NONE,
            ));
        }
        Icon::Signal => {
            for i in 0..3 {
                let h = s * (0.3 + i as f32 * 0.25);
                let x = c.x - s * 0.3 + i as f32 * s * 0.3;
                p.rect_filled(Rect::from_min_max(pos2(x - s * 0.08, c.y + s * 0.4 - h), pos2(x + s * 0.08, c.y + s * 0.4)), 1.0, color);
            }
        }
        Icon::ArrowLeft | Icon::ArrowRight => {
            let d = if icon == Icon::ArrowLeft { -1.0 } else { 1.0 };
            let h = s * 0.34;
            let st = Stroke::new((s * 0.1).max(1.5), color);
            p.line_segment([c - vec2(h * d, 0.0), c + vec2(h * d, 0.0)], st);
            let tip = c + vec2(h * d, 0.0);
            let back = tip - vec2(d * h * 0.75, 0.0);
            p.add(Shape::line(vec![back - vec2(0.0, h * 0.75), tip, back + vec2(0.0, h * 0.75)], st));
        }
        Icon::Reply => {
            // 左向きの矢印が右下へ曲がる
            let st = Stroke::new((s * 0.1).max(1.5), color);
            let tip = c + vec2(-s * 0.36, -s * 0.1);
            p.add(Shape::line(vec![tip + vec2(s * 0.22, -s * 0.22), tip, tip + vec2(s * 0.22, s * 0.22)], st));
            let mut pts = vec![tip];
            pts.extend(arc_points(c + vec2(s * 0.02, s * 0.28), s * 0.38, -std::f32::consts::FRAC_PI_2, 0.0));
            p.add(Shape::line(pts, st));
        }
        Icon::Pencil => {
            let st = Stroke::new((s * 0.16).max(2.0), color);
            let a = c + vec2(-s * 0.28, s * 0.28);
            let b = c + vec2(s * 0.3, -s * 0.3);
            p.line_segment([a + vec2(s * 0.06, -s * 0.06), b], st);
            p.add(Shape::convex_polygon(vec![a + vec2(-s * 0.05, s * 0.05), a + vec2(0.0, -s * 0.12), a + vec2(s * 0.12, 0.0)], color, Stroke::NONE));
        }
        Icon::Copy => {
            let st = Stroke::new((s * 0.08).max(1.5), color);
            let back = Rect::from_min_size(c + vec2(-s * 0.34, -s * 0.34), vec2(s * 0.46, s * 0.5));
            let front = back.translate(vec2(s * 0.2, s * 0.18));
            p.rect_stroke(back, s * 0.08, st, egui::StrokeKind::Middle);
            p.rect_filled(front.expand(1.0), s * 0.08, SIDEBAR());
            p.rect_stroke(front, s * 0.08, st, egui::StrokeKind::Middle);
        }
        Icon::Trash => {
            let st = Stroke::new((s * 0.08).max(1.5), color);
            p.line_segment([c + vec2(-s * 0.36, -s * 0.24), c + vec2(s * 0.36, -s * 0.24)], st);
            p.line_segment([c + vec2(-s * 0.1, -s * 0.36), c + vec2(s * 0.1, -s * 0.36)], st);
            let body = Rect::from_min_max(c + vec2(-s * 0.26, -s * 0.16), c + vec2(s * 0.26, s * 0.38));
            p.rect_stroke(body, s * 0.06, st, egui::StrokeKind::Middle);
            for x in [-0.09_f32, 0.09] {
                p.line_segment([c + vec2(s * x, -s * 0.04), c + vec2(s * x, s * 0.26)], st);
            }
        }
        Icon::Profile => {
            // 右側パネル付きの四角（プロフィール表示の切替）
            let st = Stroke::new((s * 0.08).max(1.5), color);
            let r = Rect::from_center_size(c, vec2(s * 0.8, s * 0.7));
            p.rect_stroke(r, s * 0.1, st, egui::StrokeKind::Middle);
            p.circle_filled(c + vec2(-s * 0.12, -s * 0.06), s * 0.1, color);
            p.rect_filled(Rect::from_center_size(c + vec2(-s * 0.12, s * 0.15), vec2(s * 0.26, s * 0.12)), s * 0.06, color);
            p.line_segment([pos2(c.x + s * 0.14, r.top()), pos2(c.x + s * 0.14, r.bottom())], st);
        }
        Icon::Upload => {
            // 角の折れた紙と、上向きの矢印
            let st = Stroke::new((s * 0.08).max(1.5), color);
            let (l, r, t, b) = (c.x - s * 0.3, c.x + s * 0.3, c.y - s * 0.4, c.y + s * 0.4);
            let fold = s * 0.18;
            p.add(Shape::closed_line(vec![pos2(l, t), pos2(r - fold, t), pos2(r, t + fold), pos2(r, b), pos2(l, b)], st));
            p.add(Shape::line(vec![pos2(r - fold, t), pos2(r - fold, t + fold), pos2(r, t + fold)], st));
            let (ay, by) = (c.y - s * 0.12, c.y + s * 0.24);
            p.line_segment([pos2(c.x, ay), pos2(c.x, by)], st);
            p.add(Shape::line(vec![pos2(c.x - s * 0.13, ay + s * 0.13), pos2(c.x, ay), pos2(c.x + s * 0.13, ay + s * 0.13)], st));
        }
        Icon::Clipboard => {
            // クリップボード（板と上の留め具）
            let st = Stroke::new((s * 0.08).max(1.5), color);
            let board = Rect::from_min_max(c + vec2(-s * 0.32, -s * 0.32), c + vec2(s * 0.32, s * 0.42));
            p.rect_stroke(board, s * 0.08, st, egui::StrokeKind::Middle);
            let clip = Rect::from_center_size(pos2(c.x, board.top()), vec2(s * 0.3, s * 0.16));
            p.rect_filled(clip, s * 0.04, color);
            for y in [0.02_f32, 0.2] {
                p.line_segment([c + vec2(-s * 0.16, s * y), c + vec2(s * 0.16, s * y)], st);
            }
        }
        Icon::Plus => {
            let h = s * 0.36;
            let st = Stroke::new((s * 0.1).max(1.5), color);
            p.line_segment([c - vec2(h, 0.0), c + vec2(h, 0.0)], st);
            p.line_segment([c - vec2(0.0, h), c + vec2(0.0, h)], st);
        }
        Icon::Chevron => {
            let h = s * 0.2;
            p.add(Shape::line(vec![c + vec2(-h, -h * 0.5), c + vec2(0.0, h * 0.5), c + vec2(h, -h * 0.5)], thin));
        }
    }
}

/// ヘッダーの表示切替アイコン（オンのときは白）
fn toggle_icon(ui: &mut egui::Ui, icon: Icon, on: bool) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(32.0, 32.0), Sense::click());
    if resp.hovered() {
        ui.painter().rect_filled(rect, 8.0, SELECTED());
    }
    paint_glyph(ui.painter(), rect.shrink(5.0), icon, if on || resp.hovered() { WHITE() } else { MUTED() });
    resp
}

/// 左下のマイク・ヘッドホンのような「アイコン＋⌄」の組。返り値は (本体, ⌄)
fn toggle_with_menu(ui: &mut egui::Ui, icon: Icon, off: bool) -> (egui::Response, egui::Response) {
    let (rect, _) = ui.allocate_exact_size(vec2(52.0, 32.0), Sense::hover());
    let main_r = Rect::from_min_size(rect.min, vec2(34.0, 32.0));
    let chev_r = Rect::from_min_max(pos2(main_r.right(), rect.top()), rect.max);
    let main = ui.interact(main_r, ui.id().with(("tgl", icon as u8)), Sense::click());
    let chev = ui.interact(chev_r, ui.id().with(("chev", icon as u8)), Sense::click());
    let p = ui.painter();
    let red_bg = Color32::from_rgba_unmultiplied(0xda, 0x37, 0x3c, 40);
    if off {
        p.rect_filled(rect, 6.0, red_bg);
    }
    for (r, resp) in [(main_r, &main), (chev_r, &chev)] {
        if resp.hovered() {
            p.rect_filled(r, 6.0, if off { Color32::from_rgba_unmultiplied(0xda, 0x37, 0x3c, 70) } else { SELECTED() });
        }
    }
    let color = if off { RED() } else if main.hovered() { WHITE() } else { TEXT() };
    let g = main_r.shrink(7.0);
    paint_glyph(p, g, icon, color);
    if off {
        p.line_segment([g.left_top() + vec2(1.0, 1.0), g.right_bottom() - vec2(1.0, 1.0)], Stroke::new(2.5_f32, USERBAR()));
        p.line_segment([g.left_top() + vec2(2.0, 0.0), g.right_bottom() - vec2(0.0, 2.0)], Stroke::new(2.0_f32, RED()));
    }
    paint_glyph(p, chev_r.shrink2(vec2(2.0, 9.0)), Icon::Chevron, if chev.hovered() { WHITE() } else { color });
    (main, chev)
}

/// 通話画面の丸いボタン
fn round_button(ui: &mut egui::Ui, size: f32, icon: Icon, bg: Color32, fg: Color32, slashed: bool) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(size, size), Sense::click());
    let bg = if resp.hovered() { bg.gamma_multiply(1.2) } else { bg };
    let p = ui.painter();
    p.circle_filled(rect.center(), size / 2.0, bg);
    let g = rect.shrink(size * 0.27);
    paint_glyph(p, g, icon, fg);
    if slashed {
        p.line_segment([g.left_top(), g.right_bottom()], Stroke::new(3.5_f32, bg));
        p.line_segment([g.left_top() + vec2(1.0, -1.0), g.right_bottom() + vec2(1.0, -1.0)], Stroke::new(2.0_f32, fg));
    }
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// 話しているときの緑の輪付きアバター
fn speaking_avatar(ui: &egui::Ui, rect: Rect, u: Option<&User>, fallback: &str, speaking: bool, bg: Color32) {
    let url = u.map(user_avatar_url);
    let name = u.map(|u| u.display()).unwrap_or(fallback);
    paint_icon(ui, rect, url.as_deref(), name, rect.width() / 2.0, BLURPLE());
    if speaking {
        let w = (rect.width() * 0.06).max(2.0);
        ui.painter().circle_stroke(rect.center(), rect.width() / 2.0 + w * 0.5, Stroke::new(w, GREEN()));
        ui.painter().circle_stroke(rect.center(), rect.width() / 2.0 + w * 1.5, Stroke::new(w, bg));
    }
}

/// ボイスチャンネルの行（スピーカー付き）
fn voice_channel_row(ui: &mut egui::Ui, selected: bool, name: &str) -> egui::Response {
    let (resp, r, color) = row_frame(ui, 34.0, selected, false);
    if !ui.is_rect_visible(r) {
        return resp;
    }
    let p = ui.painter().with_clip_rect(r);
    paint_glyph(&p, Rect::from_center_size(pos2(r.left() + 18.0, r.center().y), vec2(20.0, 20.0)), Icon::Speaker, MUTED());
    p.text(pos2(r.left() + 34.0, r.center().y), Align2::LEFT_CENTER, name, FontId::proportional(15.0), color);
    resp
}

/// ボイスチャンネル内の参加者の行
fn voice_member_row(ui: &mut egui::Ui, u: Option<&User>, id: &str, speaking: bool, mute: bool, deaf: bool) -> egui::Response {
    let w = ui.available_width();
    let (rect, resp) = ui.allocate_exact_size(vec2(w, 30.0), Sense::click());
    if !ui.is_rect_visible(rect) {
        return resp;
    }
    let r = Rect::from_min_max(pos2(rect.left() + 36.0, rect.top() + 1.0), pos2(rect.right() - 8.0, rect.bottom() - 1.0));
    if resp.hovered() {
        ui.painter().rect_filled(r, 4.0, HOVER());
    }
    let av = Rect::from_min_size(pos2(r.left() + 6.0, r.center().y - 12.0), vec2(24.0, 24.0));
    speaking_avatar(ui, av, u, id, speaking, if resp.hovered() { HOVER() } else { BG() });
    let name = u.map(|u| u.display()).unwrap_or(id);
    let p = ui.painter().with_clip_rect(r);
    p.text(pos2(av.right() + 8.0, r.center().y), Align2::LEFT_CENTER, name, FontId::proportional(14.0), if speaking { WHITE() } else { MUTED() });
    let mut x = r.right() - 10.0;
    for (on, icon) in [(deaf, Icon::Headset), (mute || deaf, Icon::Mic)] {
        if on {
            let g = Rect::from_center_size(pos2(x, r.center().y), vec2(14.0, 14.0));
            paint_glyph(&p, g, icon, MUTED());
            p.line_segment([g.left_top(), g.right_bottom()], Stroke::new(1.5_f32, RED()));
            x -= 18.0;
        }
    }
    resp
}

fn nav_row(ui: &mut egui::Ui, selected: bool, text: &str, color: Option<Color32>) -> egui::Response {
    let (resp, r, c) = row_frame(ui, 34.0, selected, false);
    ui.painter().text(
        pos2(r.left() + 10.0, r.center().y),
        Align2::LEFT_CENTER,
        text,
        FontId::proportional(15.0),
        color.unwrap_or(c),
    );
    resp
}

/// ホイール・タッチパッドのスクロール量に倍率を掛ける。
/// egui は 8pt 未満の動き（タッチパッド）は遅れなしでそのまま、それ以上（ホイール）は数フレームかけて滑らかに動かすので、
/// タッチパッドの動きは倍にした後も 8pt 未満に分けて送り、指への追従が遅れないようにする
fn scale_scroll(events: &mut Vec<egui::Event>, wheel: f32, touchpad: f32) {
    let mut out = Vec::with_capacity(events.len());
    for e in events.drain(..) {
        match e {
            egui::Event::MouseWheel { unit: egui::MouseWheelUnit::Line, delta, modifiers } if !modifiers.ctrl && !modifiers.command => {
                out.push(egui::Event::MouseWheel { unit: egui::MouseWheelUnit::Line, delta: delta * wheel, modifiers });
            }
            egui::Event::MouseWheel { unit: egui::MouseWheelUnit::Point, delta, modifiers } if !modifiers.ctrl && !modifiers.command => {
                let smooth = delta.length() < 8.0;
                let d = delta * if smooth { touchpad } else { wheel };
                let parts = if smooth { (d.length() / 7.5).ceil().max(1.0) } else { 1.0 };
                for _ in 0..parts as usize {
                    out.push(egui::Event::MouseWheel { unit: egui::MouseWheelUnit::Point, delta: d / parts, modifiers });
                }
            }
            e => out.push(e),
        }
    }
    *events = out;
}

impl eframe::App for App {
    fn raw_input_hook(&mut self, _ctx: &egui::Context, raw_input: &mut egui::RawInput) {
        scale_scroll(&mut raw_input.events, self.settings.wheel_speed, self.settings.touchpad_speed);
    }

    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // 垂直同期を切っているので、続けて描くときは 1 秒に MAX_FPS 回までにする（CPU・電池を食わないよう）
        let min = std::time::Duration::from_secs_f64(1.0 / MAX_FPS);
        if let Some(wait) = self.last_frame.and_then(|t| min.checked_sub(t.elapsed())) {
            std::thread::sleep(wait);
        }
        self.last_frame = Some(std::time::Instant::now());
        if self.theme_watch.as_ref().is_some_and(|rx| rx.try_iter().count() > 0) {
            self.apply_theme(true);
        }
        self.poll();
        let focused = ctx.input(|i| i.viewport().focused.unwrap_or(true));
        ANIM_PAUSED.store(!focused, Relaxed);
        self.sync_read_state(focused);
        // プッシュトゥトーク（ウィンドウにフォーカスがあるときだけ）
        let down = self.settings.ptt
            && !self.binding_ptt
            && self.settings.ptt_key.as_deref().and_then(egui::Key::from_name).is_some_and(|k| ctx.input(|i| i.key_down(k)));
        if self.voice.ptt_down.swap(down, Relaxed) != down {
            ctx.request_repaint();
        }
        if !self.settings_open {
            self.mic_test = None;
        }
        if self.me.is_none() {
            self.login_ui(ctx);
        } else if self.settings_open {
            self.settings_ui(ctx);
        } else {
            self.main_ui(ctx);
        }
    }
}

/// 画像ローダーを登録する。http(s) は接続を使い回す自前のローダーで取る
fn install_loaders(ctx: &egui::Context) {
    egui_extras::install_image_loaders(ctx);
    // GIF・アニメーション WebP は別スレッドでデコードする（後から足したローダーが先に使われる）
    if !ctx.is_loader_installed(anim_loader::AnimLoader::ID) {
        ctx.add_image_loader(Arc::new(anim_loader::AnimLoader::default()));
    }
    if !ctx.is_loader_installed(http_loader::HttpLoader::ID) {
        ctx.add_bytes_loader(Arc::new(http_loader::HttpLoader::default()));
    }
}

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1200.0, 760.0])
            .with_min_inner_size([700.0, 400.0])
            .with_app_id("riscord"),
        // 垂直同期を待つと、ウィンドウが見えていない間（別のワークスペース・最小化・他のウィンドウの裏）に
        // Wayland のコンポジタが描画の合図を送ってこないため、画面の書き換えでメインスレッドが止まったままになり
        // Hyprland などに「応答なし」と判定される（GIF のアニメで描き続けているとき）。待たずに描き、速さは update で抑える
        vsync: false,
        ..Default::default()
    };
    eframe::run_native(
        "Riscord",
        options,
        Box::new(|cc| {
            install_loaders(&cc.egui_ctx);
            setup_fonts(&cc.egui_ctx, None);
            setup_theme(&cc.egui_ctx);
            Ok(Box::new(App::new(cc.egui_ctx.clone())))
        }),
    )
}

#[cfg(test)]
mod animated_uri_tests {
    use super::*;

    #[test]
    fn webp_attachment_is_resized_and_marked_animated() {
        let u = resized(Some("https://media.discordapp.net/attachments/1/2/a.webp?ex=1&is=2&hm=3"), "x", vec2(100.0, 50.0));
        assert_eq!(u, "https://media.discordapp.net/attachments/1/2/a.webp?ex=1&is=2&hm=3&width=200&height=100&animated=true#.webp");
    }

    #[test]
    fn gif_with_query_is_marked_animated() {
        let u = resized(Some("https://media.discordapp.net/attachments/1/2/a.GIF?ex=1"), "x", vec2(100.0, 50.0));
        assert_eq!(u, "https://media.discordapp.net/attachments/1/2/a.GIF?ex=1#.gif");
    }

    /// 印付きの URI でも egui のローダーが動く WebP として読み、各フレームを出せる
    #[test]
    fn marked_webp_uri_loads_as_animation() {
        let ctx = egui::Context::default();
        egui_extras::install_image_loaders(&ctx);
        let uri = resized(Some("https://media.discordapp.net/attachments/1/2/a.webp?ex=1"), "x", vec2(10.0, 10.0));
        ctx.include_bytes(uri.clone(), &include_bytes!("../tests/fixtures/animated.webp")[..]);
        let img = egui::Image::new(uri.clone());
        let frame_uri = img.source(&ctx).uri().unwrap().to_string();
        assert_eq!(frame_uri, format!("{uri}#0"));
        for frame in 0..3 {
            assert!(matches!(ctx.try_load_texture(&format!("{uri}#{frame}"), Default::default(), Default::default()), Ok(egui::load::TexturePoll::Ready { .. })));
        }
        let durations: Option<egui::FrameDurations> = ctx.data(|d| d.get_temp(egui::Id::new(uri.as_str())));
        assert!(durations.is_some_and(|d| d.all().count() > 1));
    }

    #[test]
    fn plain_urls_are_unchanged() {
        assert_eq!(resized(None, "https://images-ext-1.discordapp.net/external/x/a.webp", vec2(1.0, 1.0)), "https://images-ext-1.discordapp.net/external/x/a.webp");
        let png = resized(Some("https://media.discordapp.net/a.png?ex=1"), "x", vec2(10.0, 10.0));
        assert!(!png.contains('#') && !png.contains("animated"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bench_app(n_msgs: usize) -> App {
        let ctx = egui::Context::default();
        setup_fonts(&ctx, None);
        setup_theme(&ctx);
        let mut app = App { ctx, ..Default::default() };
        let user = |i: usize| User { id: format!("{}", 1000 + i % 7), username: format!("user{}", i % 7), global_name: None, avatar: None };
        app.me = Some(user(0));
        let msgs: Vec<Message> = (0..n_msgs)
            .map(|i| {
                serde_json::from_value(serde_json::json!({
                    "id": format!("{}", 100000 + i),
                    "channel_id": "c",
                    "author": { "id": format!("{}", 1000 + (i / 3) % 7), "username": "u" },
                    "content": format!("メッセージ {i} です。少し長めの文章を入れて折り返しを発生させるためのテキスト <:pog:123> とか {}", "abc ".repeat(i % 20)),
                    "timestamp": format!("2026-11-0{}T1{}:00:00+00:00", 1 + i % 5, i % 10),
                }))
                .unwrap()
            })
            .collect();
        app.messages.insert("c".into(), msgs);
        app.touch_channel("c");
        app.dms.push(Channel {
            id: "c".into(), kind: 1, name: None, position: 0, parent_id: None,
            recipients: vec![user(1)], last_message_id: None, topic: None, icon: None, permission_overwrites: vec![],
        });
        app.sel_channel = Some("c".into());
        // 大きいサーバー: 40カテゴリ x 10チャンネル（半分ボイス）, 3000人がVCにいる
        let mut chs = Vec::new();
        for c in 0..40 {
            chs.push(Channel { id: format!("cat{c}"), kind: Channel::CATEGORY, name: Some(format!("cat{c}")), position: c, parent_id: None, recipients: vec![], last_message_id: None, topic: None, icon: None, permission_overwrites: vec![] });
            for j in 0..10 {
                let kind = if j % 2 == 0 { Channel::TEXT } else { Channel::VOICE };
                chs.push(Channel { id: format!("ch{c}_{j}"), kind, name: Some(format!("ch{j}")), position: j, parent_id: Some(format!("cat{c}")), recipients: vec![], last_message_id: None, topic: None, icon: None, permission_overwrites: vec![] });
            }
        }
        chs.sort_by_key(|c| (c.is_voice(), c.position));
        app.channels.insert("g".into(), chs);
        for u in 0..3000 {
            let gid = if u < 30 { "g".to_string() } else { format!("other{}", u % 50) };
            let cid = if u < 30 { format!("ch{}_{}", u % 40, 1) } else { format!("x{}", u % 300) };
            app.voice_states.insert(format!("u{u}"), VoiceState {
                user_id: format!("u{u}"), guild_id: Some(gid), channel_id: Some(cid), self_mute: false, self_deaf: false, user: None,
            });
        }
        app.reindex_voice();
        // 5000人・ロール20個（うち5個は別枠表示）、半分がオンライン
        let roles: Vec<Role> = (0..20)
            .map(|r| Role { id: format!("r{r}"), name: format!("role{r}"), color: 0x00ff00 + r as u32, position: r, hoist: r % 4 == 0, permissions: String::new() })
            .collect();
        app.roles.insert("g".into(), roles);
        let members: Vec<Member> = (0..5000)
            .map(|i| Member {
                user: Some(User { id: format!("m{i}"), username: format!("member{i}"), global_name: Some(format!("Member {i}")), avatar: None }),
                nick: (i % 5 == 0).then(|| format!("nick{i}")),
                roles: vec![format!("r{}", i % 20), format!("r{}", (i * 7) % 20)],
            })
            .collect();
        for (i, m) in members.iter().enumerate() {
            let status = if i % 2 == 0 { "online" } else { "offline" };
            app.presences.insert(m.user.as_ref().unwrap().id.clone(), Presence { status: status.into(), activity: None });
        }
        app.members.insert("g".into(), members);
        app.show_members = true;
        app
    }

    fn time_frames(app: &mut App, guild: Option<&str>, frames: usize) -> f64 {
        app.sel_guild = guild.map(String::from);
        let input = || egui::RawInput {
            screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(1200.0, 760.0))),
            ..Default::default()
        };
        let ctx = app.ctx.clone();
        for _ in 0..5 {
            let _ = ctx.run(input(), |ctx| app.main_ui(ctx));
        }
        let t = std::time::Instant::now();
        for _ in 0..frames {
            let _ = ctx.run(input(), |ctx| app.main_ui(ctx));
        }
        t.elapsed().as_secs_f64() * 1000.0 / frames as f64
    }

    #[test]
    fn finds_links_and_trims_punctuation() {
        let s = "見て https://example.com/a_(b). と <https://x.y/z> と(http://q.r/s) xhttps://no";
        let got: Vec<&str> = find_links(s).into_iter().map(|r| &s[r]).collect();
        assert_eq!(got, ["https://example.com/a_(b)", "https://x.y/z", "http://q.r/s"]);
        let mut job = LayoutJob::default();
        let mut links = Vec::new();
        append_linked(&mut job, "あ <https://x.y/z> い", &TextFormat::default(), &mut links);
        assert_eq!(job.text, "あ https://x.y/z い");
        assert_eq!(links, [(2..15, "https://x.y/z".to_string())]);
    }

    #[test]
    fn member_list_groups_and_sorts() {
        let app = bench_app(1);
        let gid = "g";
        let members = &app.members[gid];
        let c = MemberListCache::build(gid, 0, members, &app.roles[gid], &app.presences);
        assert_eq!(c.rows.len(), c.tops.len());
        // 全員がちょうど1回ずつ並ぶ
        let mut seen: Vec<usize> = c.rows.iter().filter_map(|r| if let MemberRow::Member(i) = r { Some(*i) } else { None }).collect();
        assert_eq!(seen.len(), members.len());
        seen.sort();
        seen.dedup();
        assert_eq!(seen.len(), members.len());
        // 最後のグループはオフライン、各グループ内は名前順
        let heads: Vec<&str> = c.rows.iter().filter_map(|r| if let MemberRow::Header(h) = r { Some(h.as_str()) } else { None }).collect();
        assert!(heads.last().unwrap().starts_with("オフライン — 2500"));
        let mut prev: Option<String> = None;
        for r in &c.rows {
            match r {
                MemberRow::Header(_) => prev = None,
                MemberRow::Member(i) => {
                    let n = members[*i].display().to_lowercase();
                    if let Some(p) = &prev {
                        assert!(p <= &n);
                    }
                    prev = Some(n);
                }
            }
        }
    }

    #[test]
    fn member_chunks_preserve_order_and_latest_updates() {
        let mut app = App::default();
        let (tx, ev) = std::sync::mpsc::channel();
        let (cmd, _rx) = tokio::sync::mpsc::unbounded_channel();
        app.backend = Some(Backend { cmd, ev });
        let member = |id: &str, name: &str| Member {
            user: Some(User { id: id.into(), username: name.into(), global_name: None, avatar: None }),
            nick: None, roles: Vec::new(),
        };
        tx.send(Event::GuildMembers("a".into(), vec![member("1", "old"), member("2", "two")])).unwrap();
        tx.send(Event::GuildMembers("b".into(), vec![member("1", "other")])).unwrap();
        tx.send(Event::GuildMembers("a".into(), vec![member("1", "new"), member("3", "three"), Member { user: None, nick: None, roles: vec![] }])).unwrap();
        app.poll();
        let a: Vec<_> = app.members["a"].iter().map(|m| (m.user.as_ref().unwrap().id.as_str(), m.display())).collect();
        assert_eq!(a, [("1", "new"), ("2", "two"), ("3", "three")]);
        assert_eq!(app.members["b"][0].display(), "other");
    }

    #[test]
    fn member_index_reuses_then_expires_and_handles_interleaved_guilds() {
        let mut app = App::default();
        let (tx, ev) = std::sync::mpsc::channel();
        let (cmd, _rx) = tokio::sync::mpsc::unbounded_channel();
        app.backend = Some(Backend { cmd, ev });
        let member = |id: &str, name: &str| Member {
            user: Some(User { id: id.into(), username: name.into(), global_name: None, avatar: None }),
            nick: None, roles: vec![],
        };
        tx.send(Event::GuildMembers("a".into(), vec![member("1", "old")])).unwrap();
        app.poll();
        assert_eq!(app.member_index.as_ref().unwrap().guild_id, "a");
        tx.send(Event::GuildMembers("a".into(), vec![member("1", "new"), member("2", "two")])).unwrap();
        app.poll();
        assert_eq!(app.members["a"].iter().map(Member::display).collect::<Vec<_>>(), ["new", "two"]);
        tx.send(Event::GuildMembers("b".into(), vec![member("1", "other")])).unwrap();
        app.poll();
        assert_eq!(app.member_index.as_ref().unwrap().guild_id, "b");
        tx.send(Event::GuildMembers("a".into(), vec![member("2", "updated")])).unwrap();
        app.poll();
        assert_eq!(app.members["a"].iter().map(Member::display).collect::<Vec<_>>(), ["new", "updated"]);
        app.member_index.as_mut().unwrap().updated -= MEMBER_INDEX_TTL;
        app.poll();
        assert!(app.member_index.is_none());
        assert_eq!(app.members["a"].len(), 2);
    }

    #[test]
    fn member_index_schedules_idle_expiration() {
        let ctx = egui::Context::default();
        let mut app = App { ctx: ctx.clone(), ..Default::default() };
        for _ in 0..3 { let _ = ctx.run(egui::RawInput::default(), |_| app.poll()); }
        app.member_index = Some(RecentMemberIndex {
            guild_id: "g".into(), index: HashMap::new(), updated: std::time::Instant::now(),
        });
        let output = ctx.run(egui::RawInput::default(), |_| app.poll());
        let delay = output.viewport_output[&egui::ViewportId::ROOT].repaint_delay;
        assert!(delay > std::time::Duration::ZERO && delay <= MEMBER_INDEX_TTL, "{delay:?}");
        app.member_index.as_mut().unwrap().updated -= MEMBER_INDEX_TTL;
        let _ = ctx.run(egui::RawInput::default(), |_| app.poll());
        assert!(app.member_index.is_none());
    }

    #[test]
    fn concurrent_member_producers_keep_index_consistent() {
        let mut app = App::default();
        let (tx, ev) = std::sync::mpsc::channel();
        let (cmd, _rx) = tokio::sync::mpsc::unbounded_channel();
        app.backend = Some(Backend { cmd, ev });
        let producers: Vec<_> = (0..4).map(|producer| {
            let tx = tx.clone();
            std::thread::spawn(move || {
                for i in 0..100 {
                    let id = format!("{producer}-{i}");
                    let member = Member {
                        user: Some(User { id: id.clone(), username: id, global_name: None, avatar: None }),
                        nick: None, roles: vec![],
                    };
                    tx.send(Event::GuildMembers("g".into(), vec![member])).unwrap();
                }
            })
        }).collect();
        while producers.iter().any(|thread| !thread.is_finished()) {
            app.poll();
            std::thread::yield_now();
        }
        for producer in producers { producer.join().unwrap(); }
        app.poll();
        let members = &app.members["g"];
        let index = &app.member_index.as_ref().unwrap().index;
        assert_eq!(members.len(), 400);
        assert_eq!(index.len(), 400);
        for (i, member) in members.iter().enumerate() {
            assert_eq!(index.get(&member.user.as_ref().unwrap().id), Some(&i));
        }
    }

    fn older_page(from: usize, n: usize) -> Vec<Message> {
        (from..from + n)
            .map(|i| serde_json::from_value(serde_json::json!({
                "id": format!("{}", 50000 + i), "channel_id": "c",
                "author": { "id": "1", "username": "u" }, "content": format!("old {i}"),
                "timestamp": "2026-10-01T10:00:00+00:00",
            })).unwrap())
            .collect()
    }

    /// 開いただけでは読み足さず、上へスクロールしたら最古のメッセージより前を取りに行き、届いたら先頭に足す
    #[test]
    fn scrolling_to_top_loads_older_history() {
        let mut app = bench_app(200);
        let (tx, ev) = std::sync::mpsc::channel();
        let (cmd, mut rx) = tokio::sync::mpsc::unbounded_channel();
        app.backend = Some(Backend { cmd, ev });
        let ctx = app.ctx.clone();
        let frame = |app: &mut App, events: Vec<egui::Event>| {
            let input = egui::RawInput { screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(1200.0, 760.0))), events, ..Default::default() };
            let _ = ctx.run(input, |ctx| { app.poll(); app.main_ui(ctx) });
        };
        let older_cmds = |rx: &mut tokio::sync::mpsc::UnboundedReceiver<Cmd>| {
            let mut v = Vec::new();
            while let Ok(c) = rx.try_recv() {
                if let Cmd::LoadOlder { channel_id, before } = c { v.push((channel_id, before)); }
            }
            v
        };
        for _ in 0..5 { frame(&mut app, vec![]); }
        assert!(older_cmds(&mut rx).is_empty(), "下端にいる間は読み足さない");

        let wheel = || vec![
            egui::Event::PointerMoved(pos2(700.0, 300.0)),
            egui::Event::MouseWheel { unit: egui::MouseWheelUnit::Point, delta: vec2(0.0, 1.0e6), modifiers: Default::default() },
        ];
        for _ in 0..30 { frame(&mut app, wheel()); }
        let sent = older_cmds(&mut rx);
        assert_eq!(sent, vec![("c".to_string(), "100000".to_string())], "読み込み中は重ねて頼まない");

        tx.send(Event::OlderMessages { channel_id: "c".into(), before: "100000".into(), messages: Some(older_page(0, net::HISTORY_PAGE)) }).unwrap();
        frame(&mut app, vec![]);
        let msgs = &app.messages["c"];
        assert_eq!(msgs.len(), 200 + net::HISTORY_PAGE);
        assert_eq!(msgs[0].id, "50000");
        assert_eq!(msgs[net::HISTORY_PAGE].id, "100000");
        assert!(!app.history["c"].end);

        // さらに上へ行けば、今の最古の行より前を（1回だけ）頼む
        for _ in 0..30 { frame(&mut app, wheel()); }
        // 待っていない起点への応答（重複など）は捨て、待っている依頼も取り消さない
        tx.send(Event::OlderMessages { channel_id: "c".into(), before: "100000".into(), messages: Some(older_page(100, 3)) }).unwrap();
        for _ in 0..5 { frame(&mut app, wheel()); }
        assert_eq!(app.messages["c"].len(), 200 + net::HISTORY_PAGE);
        assert_eq!(older_cmds(&mut rx), vec![("c".to_string(), "50000".to_string())]);

        // 少なく返ってきたら始まりに着いた
        tx.send(Event::OlderMessages { channel_id: "c".into(), before: "50000".into(), messages: Some(older_page(200, 3)) }).unwrap();
        frame(&mut app, vec![]);
        assert_eq!(app.messages["c"].len(), 203 + net::HISTORY_PAGE);
        assert!(app.history["c"].end);
        for _ in 0..10 { frame(&mut app, wheel()); }
        assert!(older_cmds(&mut rx).is_empty(), "始まりまで読んだらもう頼まない");
    }

    /// 権限のないチャンネルはスピナーのままにせず、その旨を出してエラー表示も履歴の読み足しもしない
    #[test]
    fn no_access_channel_shows_message() {
        let mut app = bench_app(1);
        let (tx, ev) = std::sync::mpsc::channel();
        let (cmd, mut rx) = tokio::sync::mpsc::unbounded_channel();
        app.backend = Some(Backend { cmd, ev });
        app.messages.remove("c");
        tx.send(Event::NoAccess("c".into())).unwrap();
        app.poll();
        assert!(app.messages["c"].is_empty());
        assert!(app.status.is_empty() || !app.status.starts_with("エラー"));
        let ctx = app.ctx.clone();
        let input = || egui::RawInput { screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(1200.0, 760.0))), ..Default::default() };
        let out = ctx.run(input(), |ctx| app.main_ui(ctx));
        let texts: Vec<String> = out.shapes.iter().filter_map(|c| match &c.shape { Shape::Text(t) => Some(t.galley.text().to_string()), _ => None }).collect();
        assert!(texts.iter().any(|t| t == "このチャンネルを見る権限がありません"), "{texts:?}");
        while let Ok(c) = rx.try_recv() { assert!(!matches!(c, Cmd::LoadOlder { .. })); }
        // 読み直して見えるようになったら消える
        tx.send(Event::Messages("c".into(), Vec::new())).unwrap();
        app.poll();
        assert!(!app.no_access.contains("c"));
    }

    /// 見ているチャンネルの最新メッセージを既読として送る。新着はまとめて、フォーカスがないときは送らない
    #[test]
    fn viewing_channel_acks_latest_message() {
        let mut app = bench_app(3);
        let (tx, ev) = std::sync::mpsc::channel();
        let (cmd, mut rx) = tokio::sync::mpsc::unbounded_channel();
        app.backend = Some(Backend { cmd, ev });
        let acks = |rx: &mut tokio::sync::mpsc::UnboundedReceiver<Cmd>| {
            let mut v = Vec::new();
            while let Ok(c) = rx.try_recv() {
                if let Cmd::Ack { channel_id, message_id } = c { v.push((channel_id, message_id)); }
            }
            v
        };
        app.sync_read_state(true);
        assert_eq!(acks(&mut rx), vec![("c".to_string(), "100002".to_string())]);
        app.sync_read_state(true);
        assert!(acks(&mut rx).is_empty(), "同じものは送り直さない");

        let mut m = app.messages["c"][0].clone();
        m.id = "100003".into();
        tx.send(Event::MessageCreate(m.clone())).unwrap();
        app.poll();
        app.sync_read_state(true);
        assert!(acks(&mut rx).is_empty(), "間隔を空ける");
        app.last_ack = Some(std::time::Instant::now() - ACK_INTERVAL);
        app.sync_read_state(false);
        assert!(acks(&mut rx).is_empty(), "見ていない（フォーカスがない）ときは送らない");
        app.sync_read_state(true);
        assert_eq!(acks(&mut rx), vec![("c".to_string(), "100003".to_string())]);
    }

    /// フォーカスがない間は、アニメ画像があっても再描画を要求し続けない
    #[test]
    fn unfocused_window_stops_animation_repaints() {
        let ctx = egui::Context::default();
        install_loaders(&ctx);
        let mut gif = Vec::new();
        {
            let mut enc = image::codecs::gif::GifEncoder::new(&mut gif);
            for i in 0..3u8 {
                let img = image::RgbaImage::from_pixel(8, 8, image::Rgba([i * 80, 0, 0, 255]));
                enc.encode_frame(image::Frame::from_parts(img, 0, 0, image::Delay::from_numer_denom_ms(100, 1))).unwrap();
            }
        }
        let uri = "bytes://anim.gif";
        ctx.include_bytes(uri, gif);
        let mut t = 0.0;
        let mut frame = |paused: bool| {
            ANIM_PAUSED.store(paused, Relaxed);
            t += 0.016;
            let out = ctx.run(egui::RawInput { time: Some(t), ..Default::default() }, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    ui.add(anim_image(ui.ctx(), uri.to_string()).fit_to_exact_size(vec2(8.0, 8.0)));
                });
            });
            ANIM_PAUSED.store(false, Relaxed);
            out.viewport_output.values().map(|v| v.repaint_delay).min().unwrap()
        };
        // デコードが終わるまで回す
        let start = std::time::Instant::now();
        while ctx.data(|d| d.get_temp::<egui::FrameDurations>(egui::Id::new(uri))).is_none() {
            frame(false);
            assert!(start.elapsed().as_secs() < 5);
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        for _ in 0..3 { frame(false); }
        assert!(frame(false) <= std::time::Duration::from_millis(100), "動いている間は次のフレームを頼む");
        frame(true);
        assert_eq!(frame(true), std::time::Duration::MAX, "止めている間は頼まない");
    }

    /// テーマを選ぶと配色が変わり、色の調整が優先され、背景画像も読み込まれて描かれる
    #[test]
    fn theme_applies_colors_and_background() {
        let mut app = bench_app(3);
        install_loaders(&app.ctx);
        app.settings.theme = "builtin:midnight".into();
        app.apply_theme(false);
        assert_eq!(BG(), theme::parse_color("#0b0e17").unwrap());
        assert_eq!(app.ctx.style().visuals.panel_fill, CHAT(), "egui の部品の色も変わる");
        app.settings.color_overrides.insert("accent".into(), "#ff0000".into());
        app.apply_theme(false);
        assert_eq!(BLURPLE(), Color32::from_rgb(255, 0, 0));

        // 背景画像（ファイル）
        let path = std::env::temp_dir().join(format!("riscord-bg-{}.png", std::process::id()));
        image::RgbaImage::from_pixel(4, 4, image::Rgba([0, 255, 0, 255])).save(&path).unwrap();
        app.settings.background = theme::Background { image: Some(path.display().to_string()), opacity: Some(1.0) };
        app.apply_theme(false);
        assert!(app.theme_error.is_none(), "{:?}", app.theme_error);
        let ctx = app.ctx.clone();
        let input = || egui::RawInput { screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(1200.0, 760.0))), ..Default::default() };
        let mut painted = false;
        for _ in 0..3 {
            let out = ctx.run(input(), |ctx| app.main_ui(ctx));
            painted |= out.shapes.iter().any(|c| matches!(&c.shape, Shape::Mesh(m) if m.texture_id != egui::TextureId::default() && m.vertices.len() == 4 && m.vertices[0].pos.x > 300.0));
        }
        assert!(painted, "チャット欄に背景画像を描く");
        std::fs::remove_file(&path).ok();

        // 既定に戻す（色は全体で共有なので、他のテストに影響しないように）
        app.settings = Settings::default();
        app.apply_theme(false);
        assert_eq!(BG(), theme::KEYS[0].2);
    }

    #[test]
    fn failed_history_load_waits_before_retry() {
        let mut app = bench_app(10);
        let (tx, ev) = std::sync::mpsc::channel();
        let (cmd, _rx) = tokio::sync::mpsc::unbounded_channel();
        app.backend = Some(Backend { cmd, ev });
        app.history.insert("c".into(), History { pending: Some("100000".into()), ..Default::default() });
        tx.send(Event::OlderMessages { channel_id: "c".into(), before: "100000".into(), messages: None }).unwrap();
        app.poll();
        let h = &app.history["c"];
        assert!(h.pending.is_none() && !h.end && h.retry_at.is_some());
    }

    /// ＋メニューは項目を押すとその操作を返し、＋ボタンを押すと開く
    #[test]
    fn plus_menu_items_are_clickable() {
        let ctx = egui::Context::default();
        let rect = std::cell::Cell::new(Rect::NOTHING);
        let run = |events: Vec<egui::Event>| {
            let mut got = None;
            let input = egui::RawInput { screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(400.0, 300.0))), events, ..Default::default() };
            let _ = ctx.run(input, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    let top = ui.cursor().min;
                    got = plus_menu(ui);
                    rect.set(Rect::from_min_size(top, vec2(240.0, 40.0)));
                });
            });
            got
        };
        assert!(run(vec![]).is_none());
        let click = |p: egui::Pos2| vec![
            egui::Event::PointerMoved(p),
            egui::Event::PointerButton { pos: p, button: egui::PointerButton::Primary, pressed: true, modifiers: Default::default() },
            egui::Event::PointerButton { pos: p, button: egui::PointerButton::Primary, pressed: false, modifiers: Default::default() },
        ];
        let first = rect.get().center();
        assert!(matches!(run(click(first)), Some(PlusAction::Upload)));
        assert!(matches!(run(click(first + vec2(0.0, 42.0))), Some(PlusAction::PasteImage)));
    }

    #[test]
    fn plus_button_opens_menu() {
        let mut app = bench_app(5);
        let ctx = app.ctx.clone();
        let input = |events: Vec<egui::Event>| egui::RawInput { screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(1200.0, 760.0))), events, ..Default::default() };
        let _ = ctx.run(input(vec![]), |ctx| app.main_ui(ctx));
        let id = egui::Id::new("plus_menu");
        assert!(!ctx.memory(|m| m.is_popup_open(id)));
        ctx.memory_mut(|m| m.open_popup(id));
        // 開いた最初のフレームは大きさを測るだけで描かれない
        let _ = ctx.run(input(vec![]), |ctx| app.main_ui(ctx));
        let out = ctx.run(input(vec![]), |ctx| app.main_ui(ctx));
        assert!(ctx.memory(|m| m.is_popup_open(id)));
        let texts: Vec<String> = out.shapes.iter().filter_map(|c| match &c.shape {
            Shape::Text(t) => Some(t.galley.text().to_string()),
            _ => None,
        }).collect();
        assert!(texts.iter().any(|t| t == "ファイルをアップロード"), "{texts:?}");
        assert!(texts.iter().any(|t| t == "画像を貼り付け"));
    }

    #[test]
    fn old_channels_are_evicted() {
        let mut app = App::default();
        for i in 0..(MAX_CHANNELS + 5) {
            let id = format!("c{i}");
            app.messages.insert(id.clone(), Vec::new());
            app.touch_channel(&id);
        }
        assert_eq!(app.messages.len(), MAX_CHANNELS);
        assert!(!app.messages.contains_key("c0"));
        // 開き直したチャンネルは最新扱いになり残る
        app.touch_channel("c5");
        app.messages.insert("new".into(), Vec::new());
        app.touch_channel("new");
        assert!(app.messages.contains_key("c5"));
        assert!(!app.messages.contains_key("c6"));
    }

    #[test]
    fn late_message_response_does_not_restore_evicted_channel() {
        let mut app = bench_app(1);
        let old_messages = app.messages.remove("c").unwrap();
        let (tx, ev) = std::sync::mpsc::channel();
        let (cmd, _rx) = tokio::sync::mpsc::unbounded_channel();
        app.backend = Some(Backend { cmd, ev });
        app.touch_channel("c");
        for i in 0..MAX_CHANNELS {
            let id = format!("c{i}");
            app.messages.insert(id.clone(), Vec::new());
            app.touch_channel(&id);
        }
        tx.send(Event::Messages("c".into(), old_messages.clone())).unwrap();
        app.poll();
        assert!(!app.messages.contains_key("c"));
        assert_eq!(app.messages.len(), MAX_CHANNELS);
        app.touch_channel("c");
        tx.send(Event::Messages("c".into(), old_messages)).unwrap();
        app.poll();
        assert_eq!(app.messages["c"].len(), 1);
        assert_eq!(app.messages.len(), MAX_CHANNELS);
    }

    #[test]
    fn evicted_channel_releases_unreferenced_sticker_textures() {
        let mut app = bench_app(2);
        let mut old = app.messages.remove("c").unwrap();
        for (m, id) in old.iter_mut().zip(["old", "shared"]) {
            m.sticker_items.push(model::StickerItem { id: id.into(), name: id.into(), format_type: 1 });
        }
        let shared = old[1].clone();
        app.messages.insert("old-channel".into(), old);
        app.touch_channel("old-channel");
        app.messages.insert("keep".into(), vec![shared]);
        app.touch_channel("keep");
        for id in ["old", "shared"] {
            let tex = app.ctx.load_texture(id, egui::ColorImage::new([200, 200], Color32::WHITE), Default::default());
            app.stickers.insert(id.into(), StickerState::Ready { frames: vec![(tex, 0)], total: 0 });
        }
        let before = app.ctx.tex_manager().read().num_allocated();
        for i in 0..(MAX_CHANNELS - 1) { app.touch_channel(&format!("c{i}")); }
        let after = app.ctx.tex_manager().read().num_allocated();
        assert!(!app.stickers.contains_key("old"));
        assert!(app.stickers.contains_key("shared"));
        assert_eq!(before - after, 1);
        let (tx, ev) = std::sync::mpsc::channel();
        let (cmd, _rx) = tokio::sync::mpsc::unbounded_channel();
        app.backend = Some(Backend { cmd, ev });
        tx.send(Event::Sticker("old".into(), Ok(sticker::Frames { size: [1, 1], frames: vec![(vec![255; 4], 0)] }))).unwrap();
        app.poll();
        assert!(!app.stickers.contains_key("old"));
        assert_eq!(app.ctx.tex_manager().read().num_allocated(), after);
    }

    #[test]
    fn deleted_messages_release_only_unreferenced_stickers() {
        let mut app = bench_app(3);
        for (m, id) in app.messages.get_mut("c").unwrap().iter_mut().zip(["old", "shared", "shared"]) {
            m.sticker_items.push(model::StickerItem { id: id.into(), name: id.into(), format_type: 1 });
        }
        for id in ["old", "shared"] {
            let tex = app.ctx.load_texture(id, egui::ColorImage::new([200, 200], Color32::WHITE), Default::default());
            app.stickers.insert(id.into(), StickerState::Ready { frames: vec![(tex, 0)], total: 0 });
        }
        let (tx, ev) = std::sync::mpsc::channel();
        let (cmd, _rx) = tokio::sync::mpsc::unbounded_channel();
        app.backend = Some(Backend { cmd, ev });
        let before = app.ctx.tex_manager().read().num_allocated();
        for m in &app.messages["c"][..2] {
            tx.send(Event::MessageDelete { channel_id: "c".into(), id: m.id.clone() }).unwrap();
        }
        app.poll();
        assert!(!app.stickers.contains_key("old"));
        assert!(app.stickers.contains_key("shared"));
        assert_eq!(before - app.ctx.tex_manager().read().num_allocated(), 1);
    }

    #[test]
    fn replaced_and_trimmed_messages_release_stickers() {
        for replace in [true, false] {
            let mut app = bench_app(MAX_MSGS);
            let messages = app.messages.get_mut("c").unwrap();
            for (m, id) in messages.iter_mut().take(2).zip(["old", "shared"]) {
                m.sticker_items.push(model::StickerItem { id: id.into(), name: id.into(), format_type: 1 });
            }
            app.stickers.insert("old".into(), StickerState::Loading);
            app.stickers.insert("shared".into(), StickerState::Loading);
            let (tx, ev) = std::sync::mpsc::channel();
            let (cmd, _rx) = tokio::sync::mpsc::unbounded_channel();
            app.backend = Some(Backend { cmd, ev });
            if replace {
                tx.send(Event::Messages("c".into(), app.messages["c"][1..].to_vec())).unwrap();
            } else {
                // 表示中のチャンネルは遡った履歴を消さないので、別のチャンネルを見ている状態にする
                app.sel_channel = None;
                let mut new = app.messages["c"][2].clone();
                new.id = "new".into();
                tx.send(Event::MessageCreate(new)).unwrap();
            }
            app.poll();
            assert!(!app.stickers.contains_key("old"), "replace={replace}");
            assert!(app.stickers.contains_key("shared"), "replace={replace}");
        }
    }

    #[test]
    #[ignore]
    fn bench_prune_stickers() {
        let mut app = bench_app(MAX_MSGS);
        let mut messages = app.messages["c"].clone();
        for (i, m) in messages.iter_mut().enumerate().step_by(100) {
            m.sticker_items.push(model::StickerItem { id: format!("s{i}"), name: String::new(), format_type: 1 });
        }
        for i in 0..30 { app.messages.insert(format!("channel{i}"), messages.clone()); }
        let start = std::time::Instant::now();
        for _ in 0..100 { app.prune_stickers(); }
        println!("sticker cache scan (31000 messages): {:.3} ms", start.elapsed().as_secs_f64() * 10.0);
    }

    #[test]
    fn voice_members_follow_index_and_skip_missing_states() {
        let mut app = bench_app(0);
        let expected = app.voice_index["ch1_1"].clone();
        let actual: Vec<_> = app.voice_members("ch1_1").map(|v| v.user_id.clone()).collect();
        assert_eq!(actual, expected);
        assert!(app.voice_members("missing").next().is_none());
        app.voice_states.remove(&expected[0]);
        let actual: Vec<_> = app.voice_members("ch1_1").map(|v| v.user_id.clone()).collect();
        assert_eq!(actual, expected[1..]);
    }

    #[test]
    fn voice_event_burst_keeps_final_index_consistent() {
        let mut app = bench_app(0);
        let (tx, ev) = std::sync::mpsc::channel();
        let (cmd, _rx) = tokio::sync::mpsc::unbounded_channel();
        app.backend = Some(Backend { cmd, ev });
        let mut state = app.voice_states["u1"].clone();
        state.guild_id = None;
        state.channel_id = Some("call".into());
        tx.send(Event::VoiceStates(vec![state.clone()])).unwrap();
        tx.send(Event::CallEnded("call".into())).unwrap();
        state.channel_id = Some("new".into());
        tx.send(Event::VoiceStates(vec![state])).unwrap();
        app.poll();
        assert_eq!(app.voice_members("new").map(|v| v.user_id.as_str()).collect::<Vec<_>>(), ["u1"]);
        assert!(app.voice_members("call").next().is_none());
        assert!(!app.voice_members("ch1_1").any(|v| v.user_id == "u1"));
        let index = app.voice_index.clone();
        app.reindex_voice();
        assert_eq!(app.voice_index, index);
    }

    #[test]
    fn voice_flag_update_preserves_index_and_updates_state() {
        let mut app = bench_app(0);
        let (tx, ev) = std::sync::mpsc::channel();
        let (cmd, _rx) = tokio::sync::mpsc::unbounded_channel();
        app.backend = Some(Backend { cmd, ev });
        let before = app.voice_index.clone();
        let mut state = app.voice_states["u1"].clone();
        state.self_mute = !state.self_mute;
        let expected = state.self_mute;
        tx.send(Event::VoiceStates(vec![state])).unwrap();
        app.poll();
        assert_eq!(app.voice_index, before);
        assert_eq!(app.voice_states["u1"].self_mute, expected);
        app.reindex_voice();
        assert_eq!(app.voice_index, before);
    }

    #[test]
    fn voice_index_matches_states_across_mixed_updates() {
        let mut app = App::default();
        let (tx, ev) = std::sync::mpsc::channel();
        let (cmd, _rx) = tokio::sync::mpsc::unbounded_channel();
        app.backend = Some(Backend { cmd, ev });
        for batch in 0..100 {
            let mut updates = Vec::new();
            for i in 0..12 {
                let n = (batch * 7 + i * 3) % 29;
                updates.push(VoiceState {
                    user_id: format!("u{n}"), guild_id: Some("g".into()),
                    channel_id: ((batch + i) % 5 != 0).then(|| format!("c{}", (batch + n) % 4)),
                    self_mute: (batch + i) % 2 == 0, self_deaf: i % 3 == 0, user: None,
                });
            }
            tx.send(Event::VoiceStates(updates)).unwrap();
            app.poll();
            let mut expected: HashMap<String, Vec<String>> = HashMap::new();
            for (id, state) in &app.voice_states {
                if let Some(channel) = &state.channel_id {
                    expected.entry(channel.clone()).or_default().push(id.clone());
                }
            }
            for ids in expected.values_mut() { ids.sort(); }
            assert_eq!(app.voice_index, expected, "batch {batch}");
        }
    }

    #[test]
    fn deleted_messages_release_cached_heights() {
        let mut app = bench_app(1000);
        let (tx, ev) = std::sync::mpsc::channel();
        let (cmd, _rx) = tokio::sync::mpsc::unbounded_channel();
        app.backend = Some(Backend { cmd, ev });
        let heights = app.row_heights.entry("c".into()).or_default();
        for message in &app.messages["c"] {
            heights.h.insert(message.id.clone(), 48.0);
            tx.send(Event::MessageDelete { channel_id: "c".into(), id: message.id.clone() }).unwrap();
        }
        app.poll();
        assert!(app.messages["c"].is_empty());
        assert!(app.row_heights["c"].h.is_empty());
    }

    #[test]
    fn refreshed_messages_release_obsolete_cached_heights() {
        let mut app = bench_app(1000);
        let (tx, ev) = std::sync::mpsc::channel();
        let (cmd, _rx) = tokio::sync::mpsc::unbounded_channel();
        app.backend = Some(Backend { cmd, ev });
        let cache = app.row_heights.entry("c".into()).or_default();
        for message in &app.messages["c"] { cache.h.insert(message.id.clone(), 48.0); }
        let remaining = app.messages["c"][950..].to_vec();
        tx.send(Event::Messages("c".into(), remaining)).unwrap();
        app.poll();
        assert_eq!(app.row_heights["c"].h.len(), 50);
        assert!(app.messages["c"].iter().all(|m| app.row_heights["c"].h.contains_key(&m.id)));
    }

    #[test]
    #[ignore]
    fn bench_member_event_burst() {
        let mut app = bench_app(0);
        let initial = app.members["g"].clone();
        let chunks: Vec<Vec<Member>> = (0..20).map(|chunk| (0..1000).map(|i| Member {
            user: Some(User { id: format!("new{}", chunk * 1000 + i), username: "user".into(), global_name: None, avatar: None }),
            nick: None, roles: Vec::new(),
        }).collect()).collect();
        let (tx, ev) = std::sync::mpsc::channel();
        let (cmd, _rx) = tokio::sync::mpsc::unbounded_channel();
        app.backend = Some(Backend { cmd, ev });
        for separate in [false, true] {
            let mut elapsed = std::time::Duration::ZERO;
            for _ in 0..5 {
                app.members.insert("g".into(), initial.clone());
                app.member_index = None;
                for chunk in &chunks {
                    tx.send(Event::GuildMembers("g".into(), chunk.clone())).unwrap();
                    if separate {
                        let start = std::time::Instant::now();
                        app.poll();
                        elapsed += start.elapsed();
                    }
                }
                if !separate {
                    let start = std::time::Instant::now();
                    app.poll();
                    elapsed += start.elapsed();
                }
                assert_eq!(app.members["g"].len(), 25_000);
            }
            println!("20 member chunks with 5000 initial ({}): {:.3} ms", if separate { "separate polls" } else { "one poll" }, elapsed.as_secs_f64() * 200.0);
        }
    }

    #[test]
    #[ignore]
    fn bench_member_multi_guild() {
        let mut app = bench_app(0);
        let initial = app.members.remove("g").unwrap();
        for i in 0..6 { app.members.insert(format!("g{i}"), initial.clone()); }
        let (tx, ev) = std::sync::mpsc::channel();
        let (cmd, _rx) = tokio::sync::mpsc::unbounded_channel();
        app.backend = Some(Backend { cmd, ev });
        for chunk in 0..4 {
            for guild in 0..6 {
                let members = (0..1000).map(|i| Member {
                    user: Some(User { id: format!("new{}", chunk * 1000 + i), username: "user".into(), global_name: None, avatar: None }),
                    nick: None, roles: vec![],
                }).collect();
                tx.send(Event::GuildMembers(format!("g{guild}"), members)).unwrap();
            }
        }
        let start = std::time::Instant::now();
        app.poll();
        println!("24 chunks across 6 guilds: {:.3} ms", start.elapsed().as_secs_f64() * 1000.0);
        assert!((0..6).all(|i| app.members[&format!("g{i}")].len() == 9000));
    }

    #[test]
    #[ignore]
    fn bench_member_index_idle_poll() {
        let mut app = App::default();
        let (_tx, ev) = std::sync::mpsc::channel();
        let (cmd, _rx) = tokio::sync::mpsc::unbounded_channel();
        app.backend = Some(Backend { cmd, ev });
        for cached in [false, true, true, false] {
            app.member_index = cached.then(|| RecentMemberIndex {
                guild_id: "g".into(), index: HashMap::new(), updated: std::time::Instant::now(),
            });
            let start = std::time::Instant::now();
            for _ in 0..50_000 { app.poll(); }
            println!("idle poll {}: {:.1} ns", if cached { "with index" } else { "without index" }, start.elapsed().as_secs_f64() * 20_000.0);
        }
    }

    #[test]
    #[ignore]
    fn bench_voice_event_burst() {
        let mut app = bench_app(0);
        let (tx, ev) = std::sync::mpsc::channel();
        let (cmd, _rx) = tokio::sync::mpsc::unbounded_channel();
        app.backend = Some(Backend { cmd, ev });
        let mut elapsed = std::time::Duration::ZERO;
        for _ in 0..50 {
            for i in 0..30 {
                let state = app.voice_states[&format!("u{i}")].clone();
                tx.send(Event::VoiceStates(vec![state])).unwrap();
            }
            let start = std::time::Instant::now();
            app.poll();
            elapsed += start.elapsed();
        }
        println!("30 voice events, 3000 states: {:.3} ms/burst", elapsed.as_secs_f64() * 1000.0 / 50.0);
    }

    /// cargo test --release bench_frames -- --ignored --nocapture
    #[test]
    #[ignore]
    fn bench_frames() {
        for n in [200, 2000, 10000] {
            let mut app = bench_app(n);
            let ms = time_frames(&mut app, None, 30);
            println!("DM {n:>5} messages: {ms:.2} ms/frame");
        }
        let mut app = bench_app(10);
        let ms = time_frames(&mut app, Some("g"), 30);
        println!("guild 400ch + 3000 voice states + 5000 members: {ms:.2} ms/frame");
        app.show_members = false;
        let ms = time_frames(&mut app, Some("g"), 30);
        println!("guild (members hidden): {ms:.2} ms/frame");
        let t = std::time::Instant::now();
        for _ in 0..20 {
            std::hint::black_box(MemberListCache::build("g", 0, &app.members["g"], &app.roles["g"], &app.presences));
        }
        println!("member list rebuild (5000): {:.2} ms", t.elapsed().as_secs_f64() * 1000.0 / 20.0);
        let t = std::time::Instant::now();
        for _ in 0..100_000 {
            std::hint::black_box(app.voice_members("ch1_1").count());
        }
        println!("voice member lookup (100000): {:.2} ms", t.elapsed().as_secs_f64() * 1000.0);
    }

    /// 上へスクロールし続けるとき（まだ高さを測っていない行が次々に入ってくる）の1フレームの時間
    /// cargo test --release bench_scroll -- --ignored --nocapture
    #[test]
    #[ignore]
    fn bench_scroll() {
        let mut app = bench_app(2000);
        let ctx = app.ctx.clone();
        let input = |events: Vec<egui::Event>, t: f64| egui::RawInput {
            screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(1200.0, 760.0))),
            time: Some(t),
            events,
            ..Default::default()
        };
        for i in 0..5 { let _ = ctx.run(input(vec![], i as f64 / 60.0), |ctx| app.main_ui(ctx)); }
        let mut worst = 0.0_f64;
        let n = 300;
        let start = std::time::Instant::now();
        for i in 0..n {
            let ev = vec![
                egui::Event::PointerMoved(pos2(700.0, 300.0)),
                egui::Event::MouseWheel { unit: egui::MouseWheelUnit::Line, delta: vec2(0.0, 3.0), modifiers: Default::default() },
            ];
            let t = std::time::Instant::now();
            let _ = ctx.run(input(ev, (i + 5) as f64 / 60.0), |ctx| app.main_ui(ctx));
            worst = worst.max(t.elapsed().as_secs_f64() * 1000.0);
        }
        let avg = start.elapsed().as_secs_f64() * 1000.0 / n as f64;
        println!("scrolling up through 2000 messages: avg {avg:.2} ms/frame, worst {worst:.2} ms, measured rows {}", app.row_heights["c"].h.len());
    }

    #[test]
    fn scroll_is_scaled_and_touchpad_stays_unsmoothed() {
        let wheel = |unit, d: f32, ctrl| egui::Event::MouseWheel { unit, delta: vec2(0.0, d), modifiers: egui::Modifiers { ctrl, command: ctrl, ..Default::default() } };
        let ys = |ev: &[egui::Event]| ev.iter().map(|e| match e { egui::Event::MouseWheel { delta, .. } => delta.y, _ => f32::NAN }).collect::<Vec<_>>();
        let mut ev = vec![wheel(egui::MouseWheelUnit::Line, 1.0, false)];
        scale_scroll(&mut ev, 2.5, 2.0);
        assert_eq!(ys(&ev), vec![2.5]);
        // タッチパッド 6pt × 2 = 12pt → 8pt 未満の2つに分ける（egui が平滑化して遅らせないように）
        let mut ev = vec![wheel(egui::MouseWheelUnit::Point, 6.0, false)];
        scale_scroll(&mut ev, 2.5, 2.0);
        assert_eq!(ys(&ev), vec![6.0, 6.0]);
        // 大きい Point（ホイール扱い）はホイールの倍率
        let mut ev = vec![wheel(egui::MouseWheelUnit::Point, 40.0, false)];
        scale_scroll(&mut ev, 2.5, 2.0);
        assert_eq!(ys(&ev), vec![100.0]);
        // Ctrl+ホイール（拡大縮小）は変えない
        let mut ev = vec![wheel(egui::MouseWheelUnit::Line, 1.0, true)];
        scale_scroll(&mut ev, 2.5, 2.0);
        assert_eq!(ys(&ev), vec![1.0]);
    }

    /// ホイールで回した量だけ画面上の内容が動くか（行の高さの補正がスクロールを打ち消していないか）
    #[test]
    fn wheel_moves_content_by_wheel_amount() {
        let mut app = bench_app(2000);
        let ctx = app.ctx.clone();
        let mut t = 0.0;
        let mut frame = |app: &mut App, events: Vec<egui::Event>| {
            t += 1.0 / 60.0;
            let input = egui::RawInput { screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(1200.0, 760.0))), time: Some(t), events, ..Default::default() };
            ctx.run(input, |ctx| app.main_ui(ctx))
        };
        // 画面上の「メッセージ N です」の位置（N → y）
        let rows = |out: &egui::FullOutput| -> HashMap<usize, f32> {
            out.shapes.iter().filter_map(|c| match &c.shape {
                Shape::Text(tx) => tx.galley.text().strip_prefix("メッセージ ").and_then(|r| r.split(' ').next()?.parse().ok()).map(|n| (n, tx.pos.y)),
                _ => None,
            }).collect()
        };
        for _ in 0..10 { frame(&mut app, vec![]); }
        let wheel = |pts: f32| vec![
            egui::Event::PointerMoved(pos2(700.0, 300.0)),
            egui::Event::MouseWheel { unit: egui::MouseWheelUnit::Point, delta: vec2(0.0, pts), modifiers: Default::default() },
        ];
        let mut total = 0.0;
        for _ in 0..40 {
            let before = rows(&frame(&mut app, vec![]));
            frame(&mut app, wheel(5.0));
            let after = rows(&frame(&mut app, vec![]));
            total += 5.0;
            let moved: Vec<f32> = before.iter().filter_map(|(n, y)| after.get(n).map(|y2| y2 - y)).collect();
            assert!(!moved.is_empty());
            let d = moved[0];
            assert!((d - 5.0).abs() < 0.6, "5pt 回して {d}pt 動いた（{total}pt 目）");
        }
    }

    /// DM・サーバーが多いアカウントでのサイドバー描画
    /// cargo test --release bench_sidebar -- --ignored --nocapture
    #[test]
    #[ignore]
    fn bench_sidebar() {
        let mut app = bench_app(50);
        let user = |i: usize| User { id: format!("{}", 5000 + i), username: format!("dmuser{i}"), global_name: Some(format!("DM相手 {i}")), avatar: Some(format!("hash{i}")) };
        for i in 0..1000 {
            app.dms.push(Channel {
                id: format!("dm{i}"), kind: 1, name: None, position: 0, parent_id: None,
                recipients: vec![user(i)], last_message_id: None, topic: None, icon: None, permission_overwrites: vec![],
            });
            app.presences.insert(format!("{}", 5000 + i), Presence { status: "online".into(), activity: (i % 3 == 0).then(|| format!("プレイ中: game{i}")) });
        }
        for i in 0..200 {
            app.guilds.push(Guild { id: format!("g{i}"), name: format!("サーバー {i}"), icon: Some(format!("icon{i}")), permissions: None });
        }
        let ms = time_frames(&mut app, None, 50);
        println!("DM list 1000 + rail 200 guilds: {ms:.3} ms/frame");
        // 多くのサーバーで未読が溜まっている状態
        for i in 0..3000 {
            app.unread.insert(format!("unread{i}"), Some(format!("g{}", i % 150)));
        }
        let ms = time_frames(&mut app, None, 50);
        println!("  + 3000 unread channels: {ms:.3} ms/frame");
    }

    /// スライダーの変更はドラッグ中も保存する
    #[test]
    fn slider_drag_saves_each_change() {
        let ctx = egui::Context::default();
        let mut value = 16.0_f32;
        let mut saves = 0;
        let mut changes = 0;
        let mut slider_rect = Rect::NOTHING;
        let frame = |ev: Vec<egui::Event>, value: &mut f32, saves: &mut i32, changes: &mut i32, rect: &mut Rect| {
            let input = egui::RawInput { screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(800.0, 600.0))), events: ev, ..Default::default() };
            let _ = ctx.run(input, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    ui.spacing_mut().slider_width = 300.0;
                    let r = ui.add(egui::Slider::new(value, 12.0..=22.0).step_by(1.0));
                    *rect = r.rect;
                    let changed = r.changed();
                    *changes += changed as i32;
                    *saves += changed as i32;
                });
            });
        };
        frame(vec![], &mut value, &mut saves, &mut changes, &mut slider_rect);
        let y = slider_rect.center().y;
        let x0 = slider_rect.left() + 10.0;
        let press = |x: f32, pressed: bool| egui::Event::PointerButton { pos: pos2(x, y), button: egui::PointerButton::Primary, pressed, modifiers: Default::default() };
        frame(vec![egui::Event::PointerMoved(pos2(x0, y)), press(x0, true)], &mut value, &mut saves, &mut changes, &mut slider_rect);
        for i in 1..=10 {
            let x = x0 + i as f32 * 25.0;
            frame(vec![egui::Event::PointerMoved(pos2(x, y))], &mut value, &mut saves, &mut changes, &mut slider_rect);
        }
        let x_end = x0 + 250.0;
        frame(vec![press(x_end, false)], &mut value, &mut saves, &mut changes, &mut slider_rect);
        frame(vec![], &mut value, &mut saves, &mut changes, &mut slider_rect);
        assert!(changes >= 5, "ドラッグ中に値が反映されていない: {changes}");
        assert!(value > 16.0);
        assert_eq!(saves, changes, "値の変更ごとに保存する");
    }

    #[test]
    fn slider_change_is_saved_if_widget_disappears_during_drag() {
        let ctx = egui::Context::default();
        let mut value = 0.5_f32;
        let mut saves = 0;
        let rect = std::cell::Cell::new(Rect::NOTHING);
        let mut render = |events: Vec<egui::Event>, visible: bool| {
            let input = egui::RawInput { screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(800.0, 600.0))), events, ..Default::default() };
            let _ = ctx.run(input, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    if visible {
                        let response = ui.add(egui::Slider::new(&mut value, 0.0..=1.0));
                        rect.set(response.rect);
                        let changed = response.changed();
                        saves += changed as i32;
                    }
                });
            });
        };
        render(vec![], true);
        let start = rect.get().center();
        render(vec![egui::Event::PointerMoved(start), egui::Event::PointerButton { pos: start, button: egui::PointerButton::Primary, pressed: true, modifiers: Default::default() }], true);
        render(vec![egui::Event::PointerMoved(start + vec2(30.0, 0.0))], true);
        render(vec![], false);
        drop(render);
        assert_ne!(value, 0.5);
        assert!(saves > 0, "変更後、スライダーが消えた場合も保存される必要がある");
    }

    /// 捨てたチャンネルだけで使っていた画像は、デコード済みの画像キャッシュから消える。他で使っている画像は残す
    #[test]
    fn evicted_channel_releases_image_cache() {
        let ctx = egui::Context::default();
        egui_extras::install_image_loaders(&ctx);
        let mut png = Vec::new();
        image::RgbaImage::from_pixel(64, 64, image::Rgba([200, 10, 10, 255]))
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();
        // アバターもメモリから返し、テストでネットワークに出ないようにする
        let avatar = |id: &str| user_avatar_url(&User { id: id.into(), username: String::new(), global_name: None, avatar: None });
        for u in ["bytes://only.png".to_string(), "bytes://shared.png".to_string(), avatar("1"), avatar("2"), emoji_url("123", false)] {
            ctx.include_bytes(u, png.clone());
        }
        let mut app = App { ctx: ctx.clone(), me: Some(User { id: "1".into(), username: "me".into(), global_name: None, avatar: None }), ..Default::default() };
        let msg = |cid: &str, id: &str, url: &str| -> Message {
            serde_json::from_value(serde_json::json!({
                "id": id, "channel_id": cid, "author": { "id": "2", "username": "u" }, "content": "",
                "timestamp": "2026-11-05T10:00:00+00:00",
                "attachments": [{ "filename": "a.png", "url": url, "content_type": "image/png", "width": 64, "height": 64 }],
            })).unwrap()
        };
        let input = || egui::RawInput { screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(1200.0, 760.0))), ..Default::default() };
        let image_bytes = |ctx: &egui::Context| -> usize { ctx.loaders().image.lock().iter().map(|l| l.byte_size()).sum() };
        let show = |app: &mut App, cid: &str, msgs: Vec<Message>| {
            app.messages.insert(cid.into(), msgs);
            app.open_channel(cid.into());
            for _ in 0..50 {
                let _ = ctx.run(input(), |ctx| app.main_ui(ctx));
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
        };
        let mut with_emoji = msg("c0", "13", "bytes://only.png");
        with_emoji.content = "見て <:pog:123>".into();
        show(&mut app, "c0", vec![msg("c0", "10", "bytes://only.png"), msg("c0", "11", "bytes://shared.png"), with_emoji]);
        show(&mut app, "c1", vec![msg("c1", "12", "bytes://shared.png")]);
        let loaded = image_bytes(&ctx);
        assert!(loaded > 0, "画像が読み込まれていない");
        assert!(app.channel_images["c0"].contains("bytes://only.png"));
        assert!(app.channel_images["c0"].contains(&emoji_url("123", false)), "絵文字も追跡する");
        // c0 が最も古くなるまで他のチャンネルを開く（c1 は途中で開き直して残す）
        for i in 2..(MAX_CHANNELS + 1) {
            app.messages.insert(format!("c{i}"), Vec::new());
            app.touch_channel(&format!("c{i}"));
            if i == 5 { app.touch_channel("c1"); }
        }
        assert!(!app.messages.contains_key("c0") && app.messages.contains_key("c1"));
        assert!(!app.channel_images.contains_key("c0"));
        let after = image_bytes(&ctx);
        assert!(after < loaded && after > 0, "only.png だけが解放されるはず: {loaded} -> {after}");
        println!("IMAGE CACHE {loaded} -> {after} bytes");
    }

    #[test]
    fn evicted_channel_releases_animated_image_frame_textures() {
        let ctx = egui::Context::default();
        egui_extras::install_image_loaders(&ctx);
        let mut gif = Vec::new();
        {
            let mut encoder = image::codecs::gif::GifEncoder::new(&mut gif);
            for color in [[255, 0, 0, 255], [0, 0, 255, 255]] {
                let image = image::RgbaImage::from_pixel(16, 16, image::Rgba(color));
                encoder.encode_frame(image::Frame::from_parts(image, 0, 0, image::Delay::from_numer_denom_ms(40, 1))).unwrap();
            }
        }
        let uri = "bytes://animated.gif";
        ctx.include_bytes(uri, gif.clone());
        for i in 0..2 {
            assert!(matches!(ctx.try_load_texture(&format!("{uri}#{i}"), Default::default(), Default::default()), Ok(egui::load::TexturePoll::Ready { .. })));
        }
        let before = ctx.tex_manager().read().num_allocated();
        let mut app = App { ctx: ctx.clone(), ..Default::default() };
        app.channel_images.entry("old".into()).or_default().insert(uri.into());
        app.touch_channel("old");
        app.channel_images.entry("shared".into()).or_default().insert(uri.into());
        app.touch_channel("shared");
        for i in 0..(MAX_CHANNELS - 1) { app.touch_channel(&format!("c{i}")); }
        assert_eq!(ctx.tex_manager().read().num_allocated(), before);
        app.touch_channel("last");
        assert_eq!(ctx.tex_manager().read().num_allocated() + 2, before);
        assert!(ctx.data(|data| data.get_temp::<egui::FrameDurations>(egui::Id::new(uri))).is_none());
        ctx.include_bytes(uri, gif);
        assert!(matches!(ctx.try_load_texture(&format!("{uri}#0"), Default::default(), Default::default()), Ok(egui::load::TexturePoll::Ready { .. })));
    }

    #[test]
    fn evicted_channel_releases_animated_webp_textures() {
        let ctx = egui::Context::default();
        egui_extras::install_image_loaders(&ctx);
        let uri = "bytes://animated.webp";
        ctx.include_bytes(uri, &include_bytes!("../tests/fixtures/animated.webp")[..]);
        for frame in 0..3 {
            assert!(matches!(ctx.try_load_texture(&format!("{uri}#{frame}"), Default::default(), Default::default()), Ok(egui::load::TexturePoll::Ready { .. })));
        }
        let before = ctx.tex_manager().read().num_allocated();
        let mut app = App { ctx: ctx.clone(), ..Default::default() };
        app.channel_images.entry("old".into()).or_default().insert(uri.into());
        app.touch_channel("old");
        for i in 0..MAX_CHANNELS { app.touch_channel(&format!("c{i}")); }
        assert_eq!(ctx.tex_manager().read().num_allocated() + 3, before);
    }

    /// 本物の CDN（認証不要のデフォルトアバター）から画像を読み込む時間・CPU 時間・スレッド数。
    /// 一斉（起動直後に多数のアイコンが見える状態）と1枚ずつ（スクロールで新しいアイコンが出る状態）の2通り
    /// cargo test --release bench_http_images -- --ignored --nocapture --test-threads=1
    #[test]
    #[ignore]
    fn bench_http_images() {
        fn cpu_ms() -> f64 {
            // /proc/self/stat の utime + stime（clock tick = 10ms）
            let s = std::fs::read_to_string("/proc/self/stat").unwrap();
            let f: Vec<&str> = s.rsplit(')').next().unwrap().split_whitespace().collect();
            (f[11].parse::<f64>().unwrap() + f[12].parse::<f64>().unwrap()) * 10.0
        }
        fn threads() -> usize {
            std::fs::read_to_string("/proc/self/status").unwrap().lines()
                .find_map(|l| l.strip_prefix("Threads:")).unwrap().trim().parse().unwrap()
        }
        let run = std::process::id();
        let ctx = egui::Context::default();
        install_loaders(&ctx);
        let wait = |uris: &[String]| -> (Vec<f64>, usize) {
            let t0 = std::time::Instant::now();
            let mut done = vec![None; uris.len()];
            let mut peak = 0;
            while done.iter().any(Option::is_none) {
                for (i, u) in uris.iter().enumerate() {
                    if done[i].is_none() {
                        match ctx.try_load_bytes(u) {
                            Ok(egui::load::BytesPoll::Ready { .. }) => done[i] = Some(t0.elapsed().as_secs_f64() * 1000.0),
                            Ok(egui::load::BytesPoll::Pending { .. }) => {}
                            Err(e) => panic!("{u}: {e}"),
                        }
                    }
                }
                peak = peak.max(threads());
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
            (done.into_iter().map(Option::unwrap).collect(), peak)
        };
        let stat = |v: &mut Vec<f64>| {
            v.sort_by(f64::total_cmp);
            let n = v.len();
            format!("平均 {:6.1} p50 {:6.1} p95 {:6.1} p99 {:6.1} ms", v.iter().sum::<f64>() / n as f64, v[n / 2], v[(n * 95 / 100).min(n - 1)], v[(n * 99 / 100).min(n - 1)])
        };
        let url = |tag: &str, i: usize| format!("https://cdn.discordapp.com/embed/avatars/{}.png?size=80&bench={run}-{tag}-{i}", i % 6);
        // 一斉に 40 枚
        let uris: Vec<String> = (0..40).map(|i| url("burst", i)).collect();
        let (c0, t0) = (cpu_ms(), std::time::Instant::now());
        let (mut v, peak) = wait(&uris);
        println!("IMG burst 40: 全体 {:.1} ms, CPU {:.0} ms, 最大スレッド数 {peak}, 各画像 {}", t0.elapsed().as_secs_f64() * 1000.0, cpu_ms() - c0, stat(&mut v));
        // 1枚ずつ 30 枚
        let c0 = cpu_ms();
        let mut seq = Vec::new();
        for i in 0..30 {
            let (v, _) = wait(&[url("seq", i)]);
            seq.push(v[0]);
        }
        println!("IMG sequential 30: CPU {:.0} ms, 1枚 {}", cpu_ms() - c0, stat(&mut seq));
    }

    #[test]
    #[ignore]
    fn bench_animated_image_eviction() {
        let ctx = egui::Context::default();
        egui_extras::install_image_loaders(&ctx);
        let mut gif = Vec::new();
        {
            let mut encoder = image::codecs::gif::GifEncoder::new(&mut gif);
            for i in 0..100 {
                let pixels = image::RgbaImage::from_pixel(8, 8, image::Rgba([i, 0, 0, 255]));
                encoder.encode_frame(image::Frame::from_parts(pixels, 0, 0, image::Delay::from_numer_denom_ms(40, 1))).unwrap();
            }
        }
        let mut app = App { ctx: ctx.clone(), ..Default::default() };
        for image in 0..10 {
            let uri = format!("bytes://bench{image}.gif");
            ctx.include_bytes(uri.clone(), gif.clone());
            for frame in 0..100 {
                ctx.try_load_texture(&format!("{uri}#{frame}"), Default::default(), Default::default()).unwrap();
            }
            app.channel_images.entry(format!("c{image}")).or_default().insert(uri);
            app.touch_channel(&format!("c{image}"));
        }
        for i in 10..MAX_CHANNELS { app.touch_channel(&format!("c{i}")); }
        let before = ctx.tex_manager().read().num_allocated();
        let start = std::time::Instant::now();
        app.touch_channel("new");
        println!("100 animated frames eviction among 1000: {:.3} ms", start.elapsed().as_secs_f64() * 1000.0);
        assert_eq!(ctx.tex_manager().read().num_allocated() + 100, before);
    }

    #[test]
    fn unread_guilds_matches_per_guild_scan() {
        let mut app = App::default();
        app.unread.insert("a".into(), Some("g1".into()));
        app.unread.insert("b".into(), None);
        app.unread.insert("c".into(), Some("g1".into()));
        let set = app.unread_guilds();
        assert!(set.contains(&Some("g1")) && set.contains(&None) && !set.contains(&Some("g2")));
        app.unread.remove("b");
        assert!(!app.unread_guilds().contains(&None));
    }

    #[test]
    fn emoji_split() {
        let v = split_emoji("hi <:pog:123> and <a:dance:456>!<bad> <@789>");
        let k: Vec<String> = v
            .iter()
            .map(|x| match x {
                Seg::Text(t) => format!("T({t})"),
                Seg::Emoji { name, id, animated } => format!("E({name},{id},{animated})"),
            })
            .collect();
        assert_eq!(k, ["T(hi )", "E(pog,123,false)", "T( and )", "E(dance,456,true)", "T(!<bad> <@789>)"]);
    }
}

#[cfg(test)]
mod startup_bench {
    use super::*;
    /// 起動時の setup_fonts（fc-match＋フォント読み込み）と最初のフレーム（フォントアトラス作成）の時間
    #[test]
    #[ignore]
    fn bench_startup() {
        let t = std::time::Instant::now();
        let ctx = egui::Context::default();
        setup_fonts(&ctx, None);
        let fonts = t.elapsed().as_secs_f64() * 1000.0;
        setup_theme(&ctx);
        let mut app = App { ctx: ctx.clone(), remember: true, show_members: true, ..Default::default() };
        let t2 = std::time::Instant::now();
        let input = egui::RawInput { screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(1200.0, 760.0))), ..Default::default() };
        let _ = ctx.run(input, |ctx| app.login_ui(ctx));
        println!("STARTUP setup_fonts {fonts:.1} ms, first frame {:.1} ms", t2.elapsed().as_secs_f64() * 1000.0);
    }
}

#[cfg(test)]
mod anim_loader_tests {
    use super::*;

    fn gif(frames: usize, w: u32, h: u32) -> Vec<u8> {
        let mut out = Vec::new();
        {
            let mut enc = image::codecs::gif::GifEncoder::new(&mut out);
            for i in 0..frames {
                let img = image::RgbaImage::from_pixel(w, h, image::Rgba([(i * 40) as u8, 0, 0, 255]));
                enc.encode_frame(image::Frame::from_parts(img, 0, 0, image::Delay::from_numer_denom_ms(50, 1))).unwrap();
            }
        }
        out
    }

    fn ctx() -> egui::Context {
        let ctx = egui::Context::default();
        install_loaders(&ctx);
        ctx
    }

    /// デコードは別スレッドで行い、描画側はその間「読み込み中」を受け取るだけで待たされない
    #[test]
    fn gif_decodes_off_the_ui_thread() {
        let ctx = ctx();
        let uri = "bytes://a.gif";
        ctx.include_bytes(uri, gif(4, 30, 20));
        let first = ctx.try_load_image(&format!("{uri}#0"), Default::default());
        assert!(matches!(first, Ok(egui::load::ImagePoll::Pending { .. })), "最初は読み込み中");
        let t = std::time::Instant::now();
        loop {
            if let Ok(egui::load::ImagePoll::Ready { image }) = ctx.try_load_image(&format!("{uri}#2"), Default::default()) {
                assert_eq!(image.size, [30, 20]);
                assert_eq!(image.pixels[0].r(), 80);
                break;
            }
            assert!(t.elapsed().as_secs() < 5);
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        let d: egui::FrameDurations = ctx.data(|d| d.get_temp(egui::Id::new(uri))).unwrap();
        assert_eq!(d.all().count(), 4);
        // チャンネルを捨てたときと同じ手順で解放できる
        ctx.forget_image(&format!("{uri}#0"));
        assert!(matches!(ctx.try_load_image(&format!("{uri}#0"), Default::default()), Ok(egui::load::ImagePoll::Pending { .. })));
    }

    #[test]
    fn huge_animation_is_thinned_and_shrunk() {
        // 600×600 × 80 フレーム（そのままだと 115MB）→ 64MB 以内に間引く
        let (frames, durations) = anim_loader::decode(&gif(80, 600, 600), true).unwrap();
        let bytes: usize = frames.iter().map(|f| f.pixels.len() * 4).sum();
        assert!(bytes <= 64 * 1024 * 1024, "{bytes}");
        assert!(frames.len() >= 20 && frames.len() < 80, "{}", frames.len());
        // 間引いても全体の長さ（80 × 50ms）は変わらない
        let total: std::time::Duration = durations.iter().sum();
        assert_eq!(total, std::time::Duration::from_millis(4000));
        // 大きいフレームは縮める
        let (big, _) = anim_loader::decode(&gif(2, 1200, 1000), true).unwrap();
        assert!(big[0].size[0] * big[0].size[1] <= 840 * 640);
        assert!(big[0].size[0] > big[0].size[1]);
    }

    #[test]
    fn animated_webp_fixture_decodes() {
        let (frames, durations) = anim_loader::decode(include_bytes!("../tests/fixtures/animated.webp"), false).unwrap();
        assert!(frames.len() > 1);
        assert_eq!(frames.len(), durations.len());
    }
}
