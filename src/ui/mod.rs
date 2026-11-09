//! 画面（gpui）。状態とロジックは crate::app にあり、ここではそれを描いて操作をつなぐ
mod chat;
mod dialogs;
mod kit;
mod login;
mod members;
mod message;
mod popup;
mod settings;
mod sidebar;

use std::cell::Cell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::atomic::Ordering::Relaxed;
use std::time::{Duration, Instant};

use futures_util::StreamExt as _;
use gpui::component::input::{InputEvent, InputState, TextareaState};
use gpui::component::slider::SliderState;
use gpui::prelude::*;
use gpui::*;

use crate::app::{self as st, App as AppState};
use crate::color::Color32;
use crate::theme::*;
use crate::wake::Waker;
pub use kit::*;

gpui::assets::icon_assets!(
    ExtraIcons,
    [
        ArrowLeft, ArrowRight, AtSign, ChevronDown, ChevronRight, CirclePlus, CircleX, ClipboardPaste, Copy, FileUp, Folder,
        Hash, HeadphoneOff, Headphones, Menu, Mic, MicOff, Palette, PanelRight, Pencil, Phone, PhoneOff, Play, Plus, RefreshCw,
        Reply, RotateCw, Search, Settings, Signal, Trash, Upload, User, UserRound, Users, Video, Volume2, X, Check, IdCard,
        LogOut, Info, TriangleAlert, Paperclip, SignalHigh, File
    ]
);

/// 同梱のアイコン（gpui-component が使う分）と、このアプリで使う分を合わせたもの
struct AppAssets;

impl AssetSource for AppAssets {
    fn load(&self, path: &str) -> Result<Option<std::borrow::Cow<'static, [u8]>>> {
        if let Some(b) = ExtraIcons.load(path)? {
            return Ok(Some(b));
        }
        gpui::assets::Assets.load(path)
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        let mut v = ExtraIcons.list(path)?;
        v.extend(gpui::assets::Assets.list(path)?);
        Ok(v)
    }
}

pub fn run() {
    gpui::application().with_assets(AppAssets).run(|cx: &mut App| {
        gpui::init(cx);
        let bounds = Bounds::centered(None, size(px(1200.0), px(760.0)), cx);
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            window_min_size: Some(size(px(320.0), px(400.0))),
            app_id: Some("riscord".into()),
            titlebar: Some(TitlebarOptions { title: Some("Riscord".into()), ..Default::default() }),
            ..Default::default()
        };
        let opened = gpui::open_window(options, cx, |window, cx| cx.new(|cx| Root::new(window, cx)));
        if let Err(e) = opened {
            eprintln!("ウィンドウを開けませんでした: {e}");
            cx.quit();
        }
    });
}

/// 1チャンネル分のメッセージ一覧（gpui の list が高さを測り、スクロール位置を保つ）
pub struct MsgList {
    pub state: ListState,
    /// 前に渡した行の中身の目印（変わった行だけ測り直す）
    pub keys: Vec<u64>,
    /// keys を作ったときの一覧の状態（AppState::msg_rev など）。同じなら keys は作り直さない
    pub stamp: u64,
}

/// 入力欄（gpui-component）。中身は AppState の文字列と同期する
pub struct Inputs {
    pub compose: Entity<TextareaState>,
    pub dm_filter: Entity<InputState>,
    pub switcher: Entity<InputState>,
    pub email: Entity<InputState>,
    pub password: Entity<InputState>,
    pub token: Entity<InputState>,
    pub mfa: Entity<InputState>,
    pub settings_query: Entity<InputState>,
    pub name: Entity<InputState>,
    pub pronouns: Entity<InputState>,
    pub bio: Entity<TextareaState>,
    pub bg_image: Entity<InputState>,
}

/// ポップアップ（開いている間だけ出す小さなメニュー）
#[derive(Clone, Copy, PartialEq)]
pub enum PopMenu {
    Plus,
    Mic,
    Headset,
}

pub struct Root {
    pub app: AppState,
    pub this: WeakEntity<Root>,
    pub focus: FocusHandle,
    pub inp: Inputs,
    pub lists: HashMap<String, MsgList>,
    /// 行の高さに効く設定（変わったら全部測り直す）
    pub list_key: (u32, u32, bool),
    pub member_list: ListState,
    pub member_key: (String, u64),
    /// マウスが乗っているメッセージ（ツールバーを出す）
    pub hover_msg: Option<String>,
    pub menu: Option<(PopMenu, Point<Pixels>)>,
    /// プロフィールのポップアップで「ステータス」のサブメニューを開いている
    pub status_sub: bool,
    pub sliders: HashMap<&'static str, Entity<SliderState>>,
    /// 設定 → テーマの色ごとの入力欄
    pub color_inputs: HashMap<&'static str, Entity<InputState>>,
    pub settings_scroll: ScrollHandle,
    pub font_family: SharedString,
    /// テーマで指定したフォントを読み込んだもの（パス, ファミリー名）
    pub theme_font: Option<(std::path::PathBuf, SharedString)>,
    pub last_title: String,
    /// 入力欄の薄い案内文（チャンネルが変わったら変える）
    pub compose_hint: String,
    _subs: Vec<Subscription>,
}

fn new_input(window: &mut Window, cx: &mut Context<Root>, placeholder: &str, masked: bool) -> Entity<InputState> {
    let p = placeholder.to_string();
    cx.new(|cx| InputState::new(window, cx).placeholder(p).masked(masked))
}

/// 入力欄の中身を文字列に写す（打ったとき）。limit = 文字数の上限
fn bind<M: gpui::component::input::InputModeKind + 'static>(
    cx: &mut Context<Root>,
    window: &mut Window,
    e: &Entity<gpui::base::input::InputBaseState<M>>,
    limit: Option<usize>,
    field: fn(&mut AppState) -> Option<&mut String>,
) -> Subscription {
    cx.subscribe_in(e, window, move |this: &mut Root, state, ev: &InputEvent, window, cx| {
        if let InputEvent::Change = ev {
            let mut v = state.read(cx).value().to_string();
            if let Some(l) = limit {
                if v.chars().count() > l {
                    v = v.chars().take(l).collect();
                    let v2 = v.clone();
                    state.update(cx, |s, cx| s.set_value(v2, window, cx));
                }
            }
            if let Some(f) = field(&mut this.app) {
                *f = v;
            }
            cx.notify();
        }
    })
}

/// 文字列が（ロジック側で）変わっていたら入力欄に写す
fn sync<M: gpui::component::input::InputModeKind + 'static>(e: &Entity<gpui::base::input::InputBaseState<M>>, s: &str, window: &mut Window, cx: &mut App) {
    if e.read(cx).value().as_ref() != s {
        let s = s.to_string();
        e.update(cx, |st, cx| st.set_value(s, window, cx));
    }
}

/// fc-match で日本語の既定フォントのファミリー名を調べる
fn default_family() -> SharedString {
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        if let Ok(o) = std::process::Command::new("fc-match").args(["-f", "%{family[0]}", "sans:lang=ja"]).output() {
            if let Ok(s) = String::from_utf8(o.stdout) {
                if !s.trim().is_empty() {
                    return s.trim().to_string().into();
                }
            }
        }
        "Noto Sans CJK JP".into()
    }
    #[cfg(windows)]
    {
        "Yu Gothic UI".into()
    }
    #[cfg(target_os = "macos")]
    {
        "Hiragino Sans".into()
    }
}

impl Root {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        // 別スレッドからの「描き直して」を受け取る。急がないものは期限までにまとめて1回にする
        let (tx, mut rx) = futures_channel::mpsc::unbounded::<Option<Duration>>();
        let waker = Waker::new(move |d| {
            let _ = tx.unbounded_send(d);
        });
        crate::images::set_waker(waker.clone());
        let deadline: Rc<Cell<Option<Instant>>> = Rc::new(Cell::new(None));
        cx.spawn(async move |this: WeakEntity<Root>, cx: &mut AsyncApp| {
            while let Some(d) = rx.next().await {
                match d {
                    None => {
                        // notify は次の描画までまとめられるので、続けて届いても描くのは1回
                        if this.update(cx, |_, cx| cx.notify()).is_err() {
                            break;
                        }
                    }
                    Some(d) => {
                        let at = Instant::now() + d;
                        if deadline.get().is_some_and(|t| t <= at && t > Instant::now()) {
                            continue;
                        }
                        deadline.set(Some(at));
                        let (this, deadline) = (this.clone(), deadline.clone());
                        cx.spawn(async move |cx: &mut AsyncApp| {
                            cx.background_executor().timer(d).await;
                            if deadline.get() == Some(at) {
                                deadline.set(None);
                            }
                            let _ = this.update(cx, |_, cx| cx.notify());
                        })
                        .detach();
                    }
                }
            }
        })
        .detach();

        let compose = cx.new(|cx| TextareaState::new(window, cx).auto_grow(1, 12).submit_on_enter(true));
        let bio = cx.new(|cx| TextareaState::new(window, cx).auto_grow(4, 8).placeholder("自分について書いてみましょう"));
        let inp = Inputs {
            compose,
            dm_filter: new_input(window, cx, "会話を検索", false),
            switcher: new_input(window, cx, "会話・チャンネル・サーバーを検索", false),
            email: new_input(window, cx, "", false),
            password: new_input(window, cx, "", true),
            token: new_input(window, cx, "", true),
            mfa: new_input(window, cx, "123456", false),
            settings_query: new_input(window, cx, "検索", false),
            name: new_input(window, cx, "", false),
            pronouns: new_input(window, cx, "代名詞を追加", false),
            bio,
            bg_image: new_input(window, cx, "画像のパスか https:// の URL（空ならテーマの指定）", false),
        };
        let mut subs = vec![
            bind(cx, window, &inp.dm_filter, None, |a| Some(&mut a.dm_filter)),
            bind(cx, window, &inp.switcher, None, |a| a.switcher.as_mut().map(|s| &mut s.0)),
            bind(cx, window, &inp.email, None, |a| Some(&mut a.login.email)),
            bind(cx, window, &inp.password, None, |a| Some(&mut a.login.password)),
            bind(cx, window, &inp.token, None, |a| Some(&mut a.token_input)),
            bind(cx, window, &inp.mfa, None, |a| Some(&mut a.login.mfa_code)),
            bind(cx, window, &inp.settings_query, None, |a| Some(&mut a.settings_query)),
            bind(cx, window, &inp.name, Some(st::PROFILE_NAME_MAX), |a| a.profile_draft.as_mut().map(|d| &mut d.fields[0])),
            bind(cx, window, &inp.pronouns, Some(st::PROFILE_PRONOUNS_MAX), |a| a.profile_draft.as_mut().map(|d| &mut d.fields[1])),
            bind(cx, window, &inp.bio, Some(st::PROFILE_BIO_MAX), |a| a.profile_draft.as_mut().map(|d| &mut d.fields[2])),
        ];
        // 入力欄: 文字を写すほか、Enter で送信する
        subs.push(cx.subscribe_in(&inp.compose, window, |this: &mut Root, state, ev: &InputEvent, window, cx| match ev {
            InputEvent::Change => {
                this.app.input = state.read(cx).value().to_string();
                cx.notify();
            }
            InputEvent::PressEnter { shift: false, .. } => {
                this.submit_input(window, cx);
            }
            _ => {}
        }));
        // 背景画像の欄は、入力を終えたときに保存する
        subs.push(cx.subscribe_in(&inp.bg_image, window, |this: &mut Root, state, ev: &InputEvent, _, cx| match ev {
            InputEvent::Change => {
                let v = state.read(cx).value().to_string();
                this.app.settings.background.image = Some(v).filter(|s| !s.is_empty());
                cx.notify();
            }
            InputEvent::Blur | InputEvent::PressEnter { .. } => {
                st::save_settings(&this.app.settings);
                this.app.apply_theme(false);
                cx.notify();
            }
            _ => {}
        }));
        // ログイン画面の Enter
        for e in [&inp.email, &inp.password, &inp.token, &inp.mfa] {
            subs.push(cx.subscribe_in(e, window, |this: &mut Root, _, ev: &InputEvent, _, cx| {
                if let InputEvent::PressEnter { .. } = ev {
                    this.login_submit(cx);
                }
            }));
        }
        subs.push(cx.subscribe_in(&inp.switcher, window, |this: &mut Root, _, ev: &InputEvent, _, cx| {
            if let InputEvent::Change = ev {
                if let Some(s) = this.app.switcher.as_mut() {
                    s.1 = 0;
                }
                cx.notify();
            }
        }));
        subs.push(cx.subscribe_in(&inp.dm_filter, window, |this: &mut Root, _, ev: &InputEvent, _, cx| {
            if let InputEvent::Blur = ev {
                if this.app.dm_filter.trim().is_empty() {
                    this.app.dm_search = false;
                    cx.notify();
                }
            }
        }));
        // ウィンドウを閉じたら終わる（通信・音声のスレッドも止まる）
        // 次の起動ですぐ出せるよう、閉じる前に控えを書いておく
        let root = cx.weak_entity();
        window.on_window_should_close(cx, move |_, cx| {
            let _ = root.update(cx, |this, _| this.app.save_snapshot(true));
            cx.quit();
            true
        });

        let app = AppState::new(waker);
        set_zoom(app.settings.zoom);
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        let mut root = Root {
            app,
            this: cx.weak_entity(),
            focus,
            inp,
            lists: HashMap::new(),
            list_key: (0, 0, false),
            member_list: ListState::new(0, ListAlignment::Top, px(400.0)),
            member_key: (String::new(), u64::MAX),
            hover_msg: None,
            menu: None,
            status_sub: false,
            sliders: HashMap::new(),
            color_inputs: HashMap::new(),
            settings_scroll: ScrollHandle::new(),
            font_family: default_family(),
            theme_font: None,
            last_title: String::new(),
            compose_hint: String::new(),
            _subs: subs,
        };
        root.apply_fonts(cx);
        root.apply_component_theme(window, cx);
        root
    }

    pub fn push_sub(&mut self, s: Subscription) {
        self._subs.push(s);
    }

    /// テーマで指定したフォントを読み込む（無ければ日本語の既定フォント）
    fn apply_fonts(&mut self, cx: &mut Context<Self>) {
        let want = self.app.theme_applied.as_ref().and_then(|t| t.font.clone());
        match want {
            Some(path) if self.theme_font.as_ref().map(|f| &f.0) != Some(&path) => match std::fs::read(&path) {
                Ok(bytes) => {
                    let before: std::collections::HashSet<String> = cx.text_system().all_font_names().into_iter().collect();
                    if cx.text_system().add_fonts(vec![bytes.into()]).is_ok() {
                        let family = cx.text_system().all_font_names().into_iter().find(|n| !before.contains(n));
                        if let Some(f) = family {
                            self.theme_font = Some((path, f.into()));
                        }
                    }
                }
                Err(e) => self.app.theme_error = Some(format!("フォント {}: {e}", path.display())),
            },
            Some(_) => {}
            None => self.theme_font = None,
        }
    }

    pub fn family(&self) -> SharedString {
        self.theme_font.as_ref().map(|f| f.1.clone()).unwrap_or_else(|| self.font_family.clone())
    }

    /// gpui-component の部品（入力欄・スライダーなど）の色をテーマに合わせる
    fn apply_component_theme(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let family = self.family();
        let t = gpui::component::Theme::global_mut(cx);
        t.font_family = family;
        t.font_size = z(16.0);
        t.background = opaque(CHAT()).into();
        t.foreground = TEXT().into();
        t.input = BORDER().into();
        t.border = BORDER().into();
        t.caret = TEXT().into();
        t.selection = BLURPLE().gamma_multiply(0.45).into();
        t.muted = WIDGET().into();
        t.muted_foreground = FAINT().into();
        t.primary = BLURPLE().into();
        t.primary_hover = BLURPLE().lerp_to_gamma(Color32::BLACK, 0.15).into();
        t.primary_active = BLURPLE().lerp_to_gamma(Color32::BLACK, 0.25).into();
        t.primary_foreground = Color32::WHITE.into();
        t.popover = SIDEBAR().into();
        t.popover_foreground = TEXT().into();
        t.accent = SELECTED().into();
        t.accent_foreground = WHITE().into();
        t.ring = BLURPLE().into();
        t.link = LINK().into();
        t.scrollbar = Color32::TRANSPARENT.into();
        t.scrollbar_thumb = WIDGET().into();
        t.scrollbar_thumb_hover = WIDGET_HOVER().into();
        t.slider_bar = BLURPLE().into();
        t.slider_thumb = Color32::WHITE.into();
        t.secondary = WIDGET().into();
        t.secondary_hover = WIDGET_HOVER().into();
        t.secondary_foreground = TEXT().into();
        t.list_hover = HOVER().into();
        t.list_active = SELECTED().into();
        t.danger = RED().into();
        // 部品の多く（ツールチップなど）は colors ではなく tokens を見るので、同じ色にそろえる
        t.tokens = gpui::component::theme::ThemeTokens::from(&t.colors);
        window.set_rem_size(z(16.0));
        window.refresh();
    }

    /// 毎回の描画の前にやること（通信の結果を取り込む・既読・テーマの反映など）
    fn before_render(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let vp = window.viewport_size();
        self.app.screen_w = f32::from(vp.width) / zoom();
        if self.app.theme_watch.as_ref().is_some_and(|rx| rx.try_iter().count() > 0) {
            self.app.apply_theme(true);
        }
        self.app.poll();
        self.app.poll_file_pick();
        self.app.save_snapshot(false);
        if !self.app.in_home() {
            self.app.poll_login();
            // QR コードは画面を開いたら用意しておく（期限切れ・取り消しのあとは押されるまで作り直さない）
            if self.app.backend.is_none() && self.app.login.qr.is_none() {
                self.app.login.qr = Some(st::QrSession::start(&self.app.waker));
            }
        }
        if self.app.font_dirty {
            self.app.font_dirty = false;
            self.apply_fonts(cx);
            self.app.theme_dirty = true;
        }
        if self.app.theme_dirty {
            self.app.theme_dirty = false;
            self.apply_component_theme(window, cx);
        }
        let focused = window.is_window_active();
        self.app.sync_read_state(focused);
        if !self.app.settings_open {
            self.app.mic_test = None;
        }
        for img in crate::images::take_dropped() {
            cx.drop_image(img, Some(window));
        }
        // 行の高さに効く設定が変わったら測り直す
        let key = (self.app.settings.font_size.to_bits(), zoom().to_bits(), self.app.settings.compact);
        if key != self.list_key {
            self.list_key = key;
            for l in self.lists.values() {
                l.state.remeasure();
            }
            self.member_list.remeasure();
        }
        // 読み込んだメッセージを捨てたチャンネルの一覧も捨てる
        if self.lists.len() > self.app.messages.len() + 1 {
            let keep = &self.app.messages;
            self.lists.retain(|k, _| keep.contains_key(k));
        }
        // 入力欄（ロジック側で中身を変えたとき）
        sync(&self.inp.compose, &self.app.input, window, cx);
        sync(&self.inp.token, &self.app.token_input, window, cx);
        sync(&self.inp.email, &self.app.login.email, window, cx);
        sync(&self.inp.password, &self.app.login.password, window, cx);
        sync(&self.inp.mfa, &self.app.login.mfa_code, window, cx);
        sync(&self.inp.dm_filter, &self.app.dm_filter, window, cx);
        sync(&self.inp.settings_query, &self.app.settings_query, window, cx);
        if let Some((q, _)) = &self.app.switcher {
            let q = q.clone();
            sync(&self.inp.switcher, &q, window, cx);
        }
        if let Some(d) = &self.app.profile_draft {
            let f = d.fields.clone();
            sync(&self.inp.name, &f[0], window, cx);
            sync(&self.inp.pronouns, &f[1], window, cx);
            sync(&self.inp.bio, &f[2], window, cx);
        }
        if self.app.focus_input {
            self.app.focus_input = false;
            self.inp.compose.update(cx, |s, cx| s.focus(window, cx));
        }
        // ウィンドウのタイトル
        let title = self.window_title();
        if self.last_title != title {
            window.set_window_title(&title);
            self.last_title = title;
        }
    }

    fn window_title(&self) -> String {
        let a = &self.app;
        match (a.current_channel(), &a.sel_guild) {
            (Some(c), Some(gid)) => a
                .guilds
                .iter()
                .find(|g| &g.id == gid)
                .map(|g| format!("#{} - {} - Discord", c.name.as_deref().unwrap_or_default(), g.name))
                .unwrap_or_else(|| "Discord".into()),
            (Some(c), None) => format!("@{} - Discord", c.dm_label()),
            (None, _) => "Discord".into(),
        }
    }

    /// Enter で入力欄の中身を送る（文字数の上限を超えていたら送らずに知らせる）
    fn submit_input(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let a = &mut self.app;
        a.input = self.inp.compose.read(cx).value().to_string();
        let limit = st::message_text_limit(a.me.as_ref().and_then(|u| u.premium_type));
        let len = st::message_text_len(&a.input);
        if len > limit {
            // 送っても Discord に断られるので、送らずに入力を残して知らせる
            a.compose_notice = Some((format!("本文は{limit}文字までです（いま{len}文字）。{}文字減らすか、分けて送ってください。", len - limit), Instant::now()));
            a.repaint_after(st::COMPOSE_NOTICE_TTL);
        } else if a.sel_channel.as_ref().is_some_and(|cid| !a.compose_submissions.contains_key(cid)) {
            let content = std::mem::take(&mut a.input);
            if !content.trim().is_empty() || a.editing.is_some() || !a.uploads.is_empty() {
                if let Some(cid) = a.sel_channel.clone() {
                    a.submit_compose(cid, content);
                }
            }
        }
        sync(&self.inp.compose, &self.app.input, window, cx);
        cx.notify();
    }

    /// 返信・編集をやめる
    pub fn cancel_compose(&mut self) {
        if self.app.editing.take().is_some() {
            self.app.input.clear();
        }
        self.app.reply_to = None;
    }

    /// ↑ で自分の直前のメッセージを編集（本家と同じ）
    pub fn edit_last(&mut self) {
        let a = &mut self.app;
        let me = a.me.as_ref().map(|u| u.id.clone());
        let last = a.sel_channel.as_ref().and_then(|c| a.messages.get(c)).and_then(|l| l.iter().rev().find(|m| Some(&m.author.id) == me.as_ref())).cloned();
        if let Some(m) = last {
            a.input = m.content.clone();
            a.editing = Some(m);
            a.reply_to = None;
        }
    }

    /// 場所を移したあとの片付け（戻る/進むの履歴・その場所に属さない返信や編集）
    pub fn after_move(&mut self, before: (Option<String>, Option<String>)) {
        let a = &mut self.app;
        if (&a.sel_guild, &a.sel_channel) != (&before.0, &before.1) {
            a.nav_back.push(before);
            if a.nav_back.len() > 100 {
                a.nav_back.remove(0);
            }
            a.nav_fwd.clear();
        }
        self.after_nav();
    }

    fn after_nav(&mut self) {
        let a = &mut self.app;
        let here = a.sel_channel.clone();
        if a.editing.as_ref().is_some_and(|m| Some(&m.channel_id) != here.as_ref()) {
            a.editing = None;
            a.input.clear();
        }
        if a.reply_to.as_ref().is_some_and(|m| Some(&m.channel_id) != here.as_ref()) {
            a.reply_to = None;
        }
        if a.upload_channel != a.sel_channel {
            a.uploads.clear();
            a.compose_notice = None;
            a.upload_channel = a.sel_channel.clone();
        }
        a.take_picked_files();
        self.hover_msg = None;
    }

    /// 戻る（true）/ 進む
    pub fn navigate(&mut self, back: bool) {
        let a = &mut self.app;
        let before = (a.sel_guild.clone(), a.sel_channel.clone());
        let (from, to) = if back { (&mut a.nav_back, &mut a.nav_fwd) } else { (&mut a.nav_fwd, &mut a.nav_back) };
        if let Some((g, c)) = from.pop() {
            to.push(before);
            a.select(g, c);
        }
        self.after_nav();
    }

    /// サーバーを選ぶ（None = DM）
    pub fn pick_guild(&mut self, g: Option<String>) {
        let before = (self.app.sel_guild.clone(), self.app.sel_channel.clone());
        let a = &mut self.app;
        if let Some(gid) = &g {
            a.ensure_guild_channels(gid);
        }
        if a.sel_guild != g {
            a.sel_guild = g;
            a.cache_dirty = true;
            a.sel_channel = None;
        }
        self.after_move(before);
    }

    pub fn open_channel(&mut self, id: String) {
        let before = (self.app.sel_guild.clone(), self.app.sel_channel.clone());
        self.app.open_channel(id);
        self.after_move(before);
    }

    /// 全体のキー操作（入力欄にフォーカスがあっても効くもの）
    fn on_key_capture(&mut self, ev: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let k = &ev.keystroke;
        // プッシュトゥトークのキーを決めている間は、そのキーを覚える
        if self.app.binding_ptt {
            if k.key != "escape" {
                self.app.settings.ptt_key = Some(k.key.clone());
                st::save_settings(&self.app.settings);
            }
            self.app.binding_ptt = false;
            cx.stop_propagation();
            cx.notify();
            return;
        }
        self.ptt(&k.key, true);
        if !self.app.in_home() {
            return;
        }
        let m = &k.modifiers;
        if m.control && !m.alt && !m.shift && k.key == "k" {
            self.app.switcher = if self.app.switcher.is_some() { None } else { Some((String::new(), 0)) };
            if self.app.switcher.is_some() {
                self.inp.switcher.update(cx, |s, cx| s.focus(window, cx));
            }
            cx.stop_propagation();
            cx.notify();
            return;
        }
        if self.app.settings_open {
            if k.key == "escape" {
                self.settings_escape();
                cx.stop_propagation();
                cx.notify();
            }
            return;
        }
        if self.app.switcher.is_some() {
            if self.switcher_key(&k.key) {
                cx.stop_propagation();
                cx.notify();
            }
            return;
        }
        if self.app.pending_delete.is_some() {
            match k.key.as_str() {
                "enter" => {
                    if let Some(m) = self.app.pending_delete.take() {
                        self.app.send(crate::net::Cmd::Delete { channel_id: m.channel_id, id: m.id });
                    }
                }
                "escape" => self.app.pending_delete = None,
                _ => return,
            }
            cx.stop_propagation();
            cx.notify();
            return;
        }
        if k.key == "escape" && (self.app.popup.is_some() || self.menu.is_some()) {
            self.app.popup = None;
            self.menu = None;
            self.status_sub = false;
            cx.stop_propagation();
            cx.notify();
            return;
        }
        if m.alt && !m.control {
            match k.key.as_str() {
                "left" => self.navigate(true),
                "right" => self.navigate(false),
                "up" | "down" => {
                    let order = self.app.sidebar_order();
                    if !order.is_empty() {
                        let d = if k.key == "up" { -1 } else { 1 };
                        let i = self.app.sel_channel.as_ref().and_then(|c| order.iter().position(|x| x == c));
                        let n = match i {
                            Some(i) => (i as i32 + d).clamp(0, order.len() as i32 - 1) as usize,
                            None => 0,
                        };
                        self.open_channel(order[n].clone());
                    }
                }
                _ => return,
            }
            cx.stop_propagation();
            cx.notify();
            return;
        }
        // 入力欄で: Esc は返信・編集をやめる、↑ は直前のメッセージを編集
        let compose_focused = self.inp.compose.read(cx).focus_handle(cx).is_focused(window);
        if compose_focused {
            if k.key == "escape" && (self.app.editing.is_some() || self.app.reply_to.is_some()) {
                self.cancel_compose();
                cx.stop_propagation();
                cx.notify();
            } else if k.key == "up" && !m.modified() && self.app.input.is_empty() && self.app.editing.is_none() {
                self.edit_last();
                cx.stop_propagation();
                cx.notify();
            }
        }
    }

    /// どこにもフォーカスが無いときに文字を打ったら入力欄へ（その文字ごと渡す）
    fn on_key_bubble(&mut self, ev: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let a = &self.app;
        if !a.in_home() || a.settings_open || a.switcher.is_some() || a.pending_delete.is_some() {
            return;
        }
        if !self.focus.is_focused(window) || !a.sel_channel.as_ref().is_some_and(|id| !a.no_access.contains(id)) {
            return;
        }
        let k = &ev.keystroke;
        if k.modifiers.control || k.modifiers.alt {
            return;
        }
        if let Some(ch) = k.key_char.as_ref().filter(|c| !c.chars().any(char::is_control)) {
            self.app.input.push_str(ch);
            self.app.focus_input = true;
            cx.notify();
        }
    }

    /// プッシュトゥトーク（ウィンドウにフォーカスがあるときだけ）
    fn ptt(&mut self, key: &str, down: bool) {
        let s = &self.app.settings;
        if !s.ptt || self.app.binding_ptt {
            return;
        }
        if s.ptt_key.as_deref().is_some_and(|k| k.eq_ignore_ascii_case(key)) {
            self.app.voice.ptt_down.store(down, Relaxed);
        }
    }

    /// テーマで「ウィンドウ全体」に敷く背景画像
    fn window_background(&self) -> Option<AnyElement> {
        let bg = self.app.theme_applied.as_ref().and_then(|t| t.background.as_ref()).filter(|b| b.window)?;
        Some(background(bg, opaque(BG())).into_any_element())
    }
}

/// 色の透明度を外す（透明な色でも背景を暗くする色として使えるように）
pub fn opaque(c: Color32) -> Color32 {
    Color32::from_rgb(c.r(), c.g(), c.b())
}

/// 背景画像を親いっぱいに（縦横比を保って切り抜いて）敷き、上から dim の色を重ねて暗くする
pub fn background(bg: &crate::theme::BackgroundImage, dim: Color32) -> Div {
    // 地の色の上に、見え具合ぶんだけ透かした画像を置く（= 画像に地の色を重ねて暗くしたのと同じ）
    let d = div().absolute().inset_0().bg(dim);
    match loaded(&bg.uri) {
        Some(i) => d.child(img(i).absolute().inset_0().size_full().object_fit(ObjectFit::Cover).opacity(bg.opacity)),
        None => d,
    }
}

impl Render for Root {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.before_render(window, cx);
        let content = if !self.app.in_home() { self.login_view(window, cx).into_any_element() } else { self.main_view(window, cx).into_any_element() };
        div()
            .id("root")
            .track_focus(&self.focus)
            .size_full()
            .relative()
            .bg(opaque(BG()))
            .text_color(TEXT())
            .font_family(self.family())
            .text_size(z(16.0))
            .line_height(relative(1.375))
            .capture_key_down(cx.listener(Self::on_key_capture))
            .on_key_down(cx.listener(Self::on_key_bubble))
            .capture_key_up(cx.listener(|this, ev: &KeyUpEvent, _, _| this.ptt(&ev.keystroke.key, false)))
            .on_mouse_down(MouseButton::Navigate(NavigationDirection::Back), cx.listener(|this, _, _, cx| {
                if this.app.in_home() && !this.app.settings_open {
                    this.navigate(true);
                    cx.notify();
                }
            }))
            .on_mouse_down(MouseButton::Navigate(NavigationDirection::Forward), cx.listener(|this, _, _, cx| {
                if this.app.in_home() && !this.app.settings_open {
                    this.navigate(false);
                    cx.notify();
                }
            }))
            .on_drop(cx.listener(|this, paths: &ExternalPaths, _, cx| {
                let a = &this.app;
                if a.in_home() && a.sel_channel.as_ref().is_some_and(|id| !a.no_access.contains(id)) && a.editing.is_none() {
                    this.app.attach_paths(paths.paths().to_vec());
                    cx.notify();
                }
            }))
            .children(self.window_background())
            .child(content)
    }
}
