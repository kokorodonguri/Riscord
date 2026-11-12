//! 設定画面（マイアカウント・プロフィール・表示・音声・テーマ）。本家（Vesktop）と同じく、アプリの上に暗い幕をかけたモーダル
use gpui::assets::IconName;
use gpui::component::input::Input;
use gpui::component::slider::{Slider, SliderEvent, SliderState, SliderValue};
use gpui::prelude::*;
use gpui::*;

use super::*;
use crate::app::{db_to_lin, lin_to_db, user_avatar_url, user_decoration_url, ProfileDraft, Settings};
use crate::color::Color32;
use crate::voice;


mod pages;
/// 設定のページ。添字は settings_tab（一覧での並び順は SETTINGS_ORDER）
const SETTINGS_TABS: [(&str, IconName); 6] =
    [("マイアカウント", IconName::UserRound), ("表示", IconName::Settings), ("音声", IconName::Headphones), ("テーマ", IconName::Palette), ("プロフィール", IconName::Pencil), ("プラグイン", IconName::Settings)];
/// 左の一覧に並べる順（本家と同じく「マイアカウント」の次に「プロフィール」）
const SETTINGS_ORDER: [usize; 6] = [0, 4, 1, 2, 3, 5];
/// 設定検索で各ページに含まれる項目を探すための見出し
const SETTINGS_SECTIONS: [&[&str]; 6] = [
    &["アカウント情報", "アクティビティ", "別のアカウント", "バージョン"],
    &["表示サイズ", "メッセージ表示"],
    &["デバイス", "音量", "入力モード", "入力感度"],
    &["テーマ", "背景画像", "色の調整"],
    &["プロフィール"],
    &["プラグイン"],
];

/// スライダーの範囲（最小, 最大, 刻み）
fn slider_range(key: &str) -> (f32, f32, f32) {
    match key {
        "zoom" => (0.7, 1.6, 0.05),
        "font_size" => (12.0, 22.0, 1.0),
        "vad_db" => (-80.0, 0.0, 1.0),
        "bg_opacity" => (0.0, 1.0, 0.01),
        _ => (0.0, 2.0, 0.01),
    }
}

fn slider_get(s: &Settings, key: &str) -> f32 {
    match key {
        "zoom" => s.zoom,
        "font_size" => s.font_size,
        "in_gain" => s.in_gain,
        "out_gain" => s.out_gain,
        "vad_db" => s.vad_db,
        "bg_opacity" => s.background.opacity.unwrap_or(0.35),
        _ => 0.0,
    }
}

fn slider_text(s: &Settings, key: &str) -> String {
    let v = slider_get(s, key);
    match key {
        "font_size" => format!("{v:.0}px"),
        "vad_db" => format!("{v:.0} dB"),
        _ => format!("{:.0}%", v * 100.0),
    }
}

/// 設定ページの見出し。2つ目からは上に区切り線を引く
fn section(idx: usize, title: &str) -> Div {
    let mut d = div().flex().flex_col();
    if idx > 0 {
        d = d.child(div().mt(z(24.0)).mb(z(40.0)).child(hline(DIVIDER())));
    }
    d.child(medium(title.to_string(), 22.0, WHITE()).mb(z(24.0)))
}

/// 設定の1行。左に項目名（と説明）、右に値や操作を置く
fn row(label: impl Into<SharedString>, desc: Option<String>, right: impl IntoElement) -> Div {
    let mut l = div().flex_1().min_w_0().flex().flex_col().gap(z(4.0)).child(text(label, 16.0, WHITE()));
    if let Some(d) = desc {
        l = l.child(text(d, 14.0, MUTED()));
    }
    div().mb(z(24.0)).min_h(z(40.0)).flex().items_center().gap(z(16.0)).child(l).child(div().flex().items_center().gap(z(16.0)).child(right))
}

/// 設定の四角いボタン（本家の「Edit」と同じ形）。danger = 赤
fn button(id: impl Into<ElementId>, label: &'static str, danger: bool) -> Stateful<Div> {
    let (fill, hover) = if danger { (RED(), RED().lerp_to_gamma(Color32::BLACK, 0.15)) } else { (WIDGET(), WIDGET_HOVER()) };
    div()
        .id(id)
        .h(z(40.0))
        .min_w(z(96.0))
        .px(z(16.0))
        .rounded(z(8.0))
        .bg(fill)
        .hover(move |s| s.bg(hover))
        .flex()
        .items_center()
        .justify_center()
        .text_size(z(15.0))
        .text_color(WHITE())
        .cursor_pointer()
        .child(label)
}

/// 選択肢のカード（本家のラジオボタン）
fn radio_card(id: impl Into<ElementId>, selected: bool, title: &'static str, desc: Option<&'static str>) -> Stateful<Div> {
    let mark = if selected {
        div().size(z(20.0)).flex_none().rounded_full().bg(WHITE()).flex().items_center().justify_center().child(div().size(z(10.0)).rounded_full().bg(BLURPLE()))
    } else {
        div().size(z(18.0)).flex_none().rounded_full().border_2().border_color(MUTED())
    };
    let mut t = div().flex_1().flex().flex_col().child(medium(title, 16.0, WHITE()));
    if let Some(d) = desc {
        t = t.child(text(d, 14.0, MUTED()));
    }
    div()
        .id(id)
        .mb(z(8.0))
        .min_h(z(44.0))
        .px(z(14.0))
        .py(z(10.0))
        .rounded(z(8.0))
        .flex()
        .items_center()
        .gap(z(14.0))
        .cursor_pointer()
        .bg(if selected { SELECTED() } else { RAIL() })
        .when(!selected, |d| d.hover(|s| s.bg(WIDGET())))
        .child(mark)
        .child(t)
}

/// 色付きの枠のお知らせ（本家の青い「i」の箱）
fn notice(accent: Color32, title: &str, body: &str) -> Div {
    div()
        .bg(accent.gamma_multiply(0.08))
        .border_1()
        .border_color(accent)
        .rounded(z(8.0))
        .px(z(16.0))
        .py(z(12.0))
        .flex()
        .items_start()
        .gap(z(10.0))
        .child(icon(IconName::Info, 20.0, accent))
        .child(div().flex_1().min_w_0().flex().flex_col().gap(z(2.0)).child(medium(title.to_string(), 15.0, WHITE())).child(text(body.to_string(), 14.0, TEXT())))
}

/// アイコン付きの大きなボタン（設定画面の操作）
fn action_button(id: &'static str, name: IconName, label: &'static str) -> Stateful<Div> {
    div()
        .id(id)
        .group("ab")
        .h(z(50.0))
        .flex_1()
        .min_w(z(180.0))
        .px(z(14.0))
        .rounded(z(6.0))
        .bg(WIDGET())
        .hover(|s| s.bg(WIDGET_HOVER()))
        .flex()
        .items_center()
        .gap(z(12.0))
        .cursor_pointer()
        .child(icon(name, 22.0, TEXT()).group_hover("ab", |s| s.text_color(WHITE())))
        .child(text(label, 16.0, TEXT()).group_hover("ab", |s| s.text_color(WHITE())))
}

/// 設定画面の枠線付きのカード。accent を渡すとその色の枠にする
fn settings_card(accent: Option<Color32>) -> Div {
    card(Color32::from_white_alpha(4), accent.unwrap_or(BORDER()), 8.0).p(z(16.0))
}

impl Root {
    /// Esc は「ログアウト確認 → 検索の文字 → 設定画面」の順に閉じる（本家と同じ）
    pub fn settings_escape(&mut self) {
        let a = &mut self.app;
        if a.confirm_logout {
            a.confirm_logout = false;
        } else if !a.settings_query.is_empty() {
            a.settings_query.clear();
        } else {
            self.close_settings();
        }
    }

    fn close_settings(&mut self) {
        let a = &mut self.app;
        if a.profile_draft.as_ref().is_some_and(ProfileDraft::dirty) {
            // 保存していない変更があるときは閉じずに、保存バーを赤くして知らせる
            a.profile_nag = Some(std::time::Instant::now());
            a.repaint_after(std::time::Duration::from_millis(1300));
            return;
        }
        a.settings_open = false;
        a.binding_ptt = false;
        a.settings_query.clear();
        a.mic_test = None;
        // 次に開いたときは最新のプロフィールから編集を始める
        a.profile_draft = None;
        a.profile_error = None;
    }

    fn jump(&mut self, tab: usize) {
        let a = &mut self.app;
        if tab != a.settings_tab && a.profile_draft.as_ref().is_some_and(ProfileDraft::dirty) {
            a.profile_nag = Some(std::time::Instant::now());
            a.repaint_after(std::time::Duration::from_millis(1300));
            return;
        }
        a.settings_tab = tab;
        a.settings_query.clear();
        self.settings_scroll.set_offset(point(px(0.), px(0.)));
    }

    /// スライダーの状態（設定の値と同期する）
    fn slider_state(&mut self, key: &'static str, window: &mut Window, cx: &mut Context<Self>) -> Entity<SliderState> {
        if let Some(s) = self.sliders.get(key) {
            let s = s.clone();
            let want = slider_get(&self.app.settings, key);
            if let SliderValue::Single(v) = s.read(cx).value() {
                if (v - want).abs() > 1e-4 {
                    s.update(cx, |st, cx| st.set_value(want, window, cx));
                }
            }
            return s;
        }
        let (min, max, step) = slider_range(key);
        let v = slider_get(&self.app.settings, key);
        let s = cx.new(|_| SliderState::new().min(min).max(max).step(step).default_value(v));
        let sub = cx.subscribe_in(&s, window, move |this: &mut Root, _, ev: &SliderEvent, window, cx| {
            let (v, done) = match ev {
                SliderEvent::Change(v) => (v.start(), false),
                SliderEvent::Release(v) => (v.start(), true),
            };
            let st = &mut this.app.settings;
            match key {
                "zoom" => {
                    st.zoom = v;
                    // 拡大率は離したときに反映する（動かしている間に画面の大きさが変わると操作しにくい）
                    if done {
                        set_zoom(v);
                        this.app.theme_dirty = true;
                        window.refresh();
                    }
                }
                "font_size" => st.font_size = v,
                "in_gain" => st.in_gain = v,
                "out_gain" => st.out_gain = v,
                "vad_db" => st.vad_db = v,
                "bg_opacity" => st.background.opacity = Some(v),
                _ => {}
            }
            if matches!(key, "in_gain" | "out_gain" | "vad_db") {
                this.app.apply_voice_settings();
            }
            if key == "bg_opacity" {
                this.app.apply_theme(false);
            }
            if done || key != "zoom" {
                this.app.save_settings();
            }
            cx.notify();
        });
        self.push_sub(sub);
        self.sliders.insert(key, s.clone());
        s
    }

    /// 見出しと値を上に、横いっぱいのスライダーを下に置く
    pub fn slider_row(&mut self, key: &'static str, label: &'static str, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let s = self.slider_state(key, window, cx);
        div()
            .flex()
            .flex_col()
            .gap(z(8.0))
            .mb(z(16.0))
            .child(div().flex().items_center().child(text(label, 16.0, WHITE()).flex_1()).child(text(slider_text(&self.app.settings, key), 15.0, MUTED())))
            .child(Slider::new(&s))
    }

    pub fn settings_modal(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (sw, sh) = (self.app.screen_w, self.pane_h);
        let compact = sw < 700.0;
        let (w, h) = if compact { ((sw - 16.0).max(0.0), (sh - 16.0).max(0.0)) } else { ((sw - 128.0).max(560.0).min((sw - 16.0).max(0.0)), (sh - 90.0).max(360.0).min((sh - 16.0).max(0.0))) };
        let wallpaper = self.app.theme_applied.as_ref().and_then(|t| t.background.as_ref()).filter(|b| b.window);
        let surface = if wallpaper.is_some() { SETTINGS_BG() } else { opaque(SETTINGS_BG()) };
        let nav_surface = if wallpaper.is_some() { SETTINGS_NAV() } else { opaque(SETTINGS_NAV()) };
        let nav_w = if compact { 0.0 } else { (w * 0.3).clamp(200.0, 260.0) };
        let tab = self.app.settings_tab;

        let mut boxed = div().w(z(w)).h(z(h)).relative().border_1().border_color(BORDER()).rounded(z(12.0)).flex().flex_row().overflow_hidden();
        if let Some(bg) = wallpaper {
            boxed = boxed.child(background(bg, opaque(BG())));
        }
        if !compact {
            boxed = boxed.child(div().bg(nav_surface).child(self.settings_nav(nav_w, cx)));
        }
        // 右: 見出しの帯と内容
        let title: AnyElement = if compact {
            // 狭い画面では左の一覧の代わりに、ページを横に並べる
            let mut tabs = div().id("compact_tabs").flex_1().min_w_0().flex().gap(z(4.0)).overflow_x_scroll();
            for i in SETTINGS_ORDER {
                let on = tab == i;
                tabs = tabs.child(
                    div()
                        .id(("ctab", i))
                        .flex_none()
                        .h(z(32.0))
                        .px(z(10.0))
                        .rounded(z(6.0))
                        .flex()
                        .items_center()
                        .cursor_pointer()
                        .when(on, |d| d.bg(SELECTED()))
                        .hover(|s| s.bg(HOVER()))
                        .text_size(z(14.0))
                        .text_color(if on { WHITE() } else { TEXT() })
                        .child(SETTINGS_TABS[i].0)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.jump(i);
                            cx.notify();
                        })),
                );
            }
            tabs = tabs.child(
                div()
                    .id("ctab_logout")
                    .flex_none()
                    .h(z(32.0))
                    .px(z(10.0))
                    .rounded(z(6.0))
                    .flex()
                    .items_center()
                    .cursor_pointer()
                    .hover(|s| s.bg(HOVER()))
                    .text_size(z(14.0))
                    .text_color(RED())
                    .child("ログアウト")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.app.confirm_logout = true;
                        cx.notify();
                    })),
            );
            tabs.into_any_element()
        } else {
            text(SETTINGS_TABS[tab].0, 16.0, WHITE()).flex_1().into_any_element()
        };
        let bar = div()
            .h(z(48.0))
            .flex_none()
            .px(z(12.0))
            .flex()
            .items_center()
            .gap(z(8.0))
            .border_b_1()
            .border_color(DIVIDER())
            .child(title)
            .child(icon_button("settings_close", IconName::X, 32.0).tooltip(tip("閉じる (Esc)")).on_click(cx.listener(|this, _, _, cx| {
                this.close_settings();
                cx.notify();
            })));
        let side = if compact { 16.0 } else { 24.0 };
        let col_w = (w - nav_w - side * 2.0).clamp(0.0, 700.0);
        let mut content = div().w(z(col_w)).flex().flex_col().pt(z(40.0)).pb(z(64.0));
        content = content.child(div());
        let pages: Vec<AnyElement> = match tab {
            0 => self.account_page(cx),
            2 => self.voice_page(col_w, window, cx),
            3 => self.theme_page(col_w, window, cx),
            4 => self.profile_page(col_w, window, cx),
            5 => self.plugins_page(window, cx),
            _ => self.display_page(window, cx),
        };
        content = content.children(pages);
        let scroll = div()
            .id(SharedString::from(format!("settings_scroll{tab}")))
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .track_scroll(&self.settings_scroll)
            .flex()
            .flex_col()
            .items_center()
            .child(content);
        let mut right = div().flex_1().min_w_0().h_full().bg(surface).flex().flex_col().relative().child(bar).child(scroll);
        if tab == 4 {
            right = right.children(self.profile_save_bar(col_w, cx));
        }
        boxed = boxed.child(right);

        let close = cx.listener(|this, _: &MouseDownEvent, _, cx| {
            if this.app.confirm_logout {
                this.app.confirm_logout = false;
            } else {
                this.close_settings();
            }
            cx.notify();
        });
        // マイクテストは音声タブを開いている間だけ（接続中は本物の入力を使う）
        let want_test = tab == 2 && self.app.voice_channel().is_none();
        if want_test && self.app.mic_test.is_none() {
            self.app.mic_test = Some(voice::MicTest::start(self.app.voice.clone()));
        } else if !want_test {
            self.app.mic_test = None;
        }
        let logout = self.app.confirm_logout.then(|| self.logout_dialog(sw, cx));
        div().absolute().inset_0().child(modal("settings_modal", boxed, close, 170)).children(logout)
    }

    fn settings_nav(&mut self, nav_w: f32, cx: &mut Context<Self>) -> impl IntoElement {
        let a = &self.app;
        let query = a.settings_query.trim().to_lowercase();
        let matches = |s: &str| query.is_empty() || s.to_lowercase().contains(&query);
        let mut col = div().w(z(nav_w)).h_full().flex_none().flex().flex_col().pl(z(16.0)).pr(z(12.0)).pt(z(20.0)).pb(z(12.0)).border_r_1().border_color(DIVIDER());
        if let Some(me) = &a.me {
            col = col.child(
                div()
                    .flex()
                    .items_center()
                    .gap(z(12.0))
                    .pl(z(4.0))
                    .mb(z(16.0))
                    .child(with_decoration(avatar(Some(&user_avatar_url(me)), 48.0, me.display(), 24.0, BLURPLE()), 48.0, user_decoration_url(me).as_deref()))
                    .child(div().flex_1().min_w_0().flex().flex_col().child(line(me.display().to_string(), 16.0, WHITE()).font_weight(FontWeight::MEDIUM)).child(line(me.username.clone(), 14.0, MUTED()))),
            );
        }
        col = col.child(
            div()
                .border_1()
                .border_color(BORDER())
                .rounded(z(8.0))
                .px(z(10.0))
                .flex()
                .items_center()
                .gap(z(6.0))
                .child(icon(IconName::Search, 18.0, MUTED()))
                .child(div().flex_1().text_size(z(15.0)).child(Input::new(&self.inp.settings_query).appearance(false).h(z(38.0)))),
        );
        let mut list = div().id("settings_nav").flex_1().min_h_0().overflow_y_scroll().mt(z(12.0)).flex().flex_col().gap(z(2.0)).child(text("ユーザー設定", 13.0, MUTED()).mb(z(6.0)));
        let mut any = false;
        for i in SETTINGS_ORDER {
            let (name, ic) = SETTINGS_TABS[i];
            let secs = SETTINGS_SECTIONS[i];
            let hit: Vec<usize> = (0..secs.len()).filter(|&j| matches(secs[j])).collect();
            if !matches(name) && hit.is_empty() {
                continue;
            }
            any = true;
            let selected = query.is_empty() && a.settings_tab == i;
            list = list.child(
                div()
                    .id(("nav", i))
                    .h(z(36.0))
                    .px(z(10.0))
                    .rounded(z(8.0))
                    .flex()
                    .items_center()
                    .gap(z(12.0))
                    .cursor_pointer()
                    .when(selected, |d| d.bg(SELECTED()))
                    .hover(|s| s.bg(if selected { SELECTED() } else { HOVER() }))
                    .child(icon(ic, 20.0, if selected { WHITE() } else { TEXT() }))
                    .child(text(name, 16.0, if selected { WHITE() } else { TEXT() }))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.jump(i);
                        cx.notify();
                    })),
            );
        }
        if !any {
            list = list.child(text("見つかりませんでした", 14.0, MUTED()).mt(z(8.0)));
        }
        list = list.child(div().my(z(8.0)).child(hline(DIVIDER()))).child(
            div()
                .id("nav_logout")
                .h(z(36.0))
                .px(z(10.0))
                .rounded(z(8.0))
                .flex()
                .items_center()
                .gap(z(12.0))
                .cursor_pointer()
                .hover(|s| s.bg(HOVER()))
                .child(icon(IconName::LogOut, 20.0, RED()))
                .child(text("ログアウト", 16.0, RED()))
                .on_click(cx.listener(|this, _, _, cx| {
                    this.app.confirm_logout = true;
                    cx.notify();
                })),
        );
        col.child(list)
    }

    fn logout_dialog(&mut self, sw: f32, cx: &mut Context<Self>) -> AnyElement {
        let content = card(SIDEBAR(), BORDER(), 8.0)
            .p(z(20.0))
            .w(z((sw - 56.0).clamp(0.0, 400.0)))
            .flex()
            .flex_col()
            .child(medium("ログアウト", 20.0, WHITE()))
            .child(text("本当にログアウトしますか？ このPCに保存したトークンも削除されるため、次はログインし直す必要があります。", 15.0, TEXT()).mt(z(8.0)))
            .child(
                div()
                    .mt(z(20.0))
                    .flex()
                    .justify_end()
                    .gap(z(8.0))
                    .child(text_button("logout_cancel", "キャンセル").min_w(z(96.0)).on_click(cx.listener(|this, _, _, cx| {
                        this.app.confirm_logout = false;
                        cx.notify();
                    })))
                    .child(fill_button("logout_ok", "ログアウト", RED(), Color32::WHITE, true).min_w(z(96.0)).on_click(cx.listener(|this, _, _, cx| {
                        this.app.confirm_logout = false;
                        this.app.profile_draft = None;
                        this.app.logout();
                        this.lists.clear();
                        cx.notify();
                    }))),
            );
        let close = cx.listener(|this, _: &MouseDownEvent, _, cx| {
            this.app.confirm_logout = false;
            cx.notify();
        });
        modal("logout_confirm", content, close, 120).into_any_element()
    }

}
