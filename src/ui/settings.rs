//! 設定画面（マイアカウント・プロフィール・表示・音声・テーマ）。本家（Vesktop）と同じく、アプリの上に暗い幕をかけたモーダル
use gpui::assets::IconName;
use gpui::component::input::{Input, InputEvent, InputState, Textarea};
use gpui::component::slider::{Slider, SliderEvent, SliderState, SliderValue};
use gpui::prelude::*;
use gpui::*;

use super::*;
use crate::app::{
    db_to_lin, lin_to_db, open_folder, spawn_file_dialog, themes_dir, user_avatar_url, AvatarEdit,
    ProfileDraft, ProfileState, Settings,
};
use crate::color::Color32;
use crate::net::{self, Cmd};
use crate::{theme, voice};

/// 設定のページ。添字は settings_tab（一覧での並び順は SETTINGS_GROUPS）
const SETTINGS_TABS: [(crate::i18n::Tr, IconName); 6] = [
    (
        trc!("マイアカウント", "My Account", "我的账号"),
        IconName::UserRound,
    ),
    (trc!("表示", "Appearance", "外观"), IconName::Settings),
    (
        trc!("音声・ビデオ", "Voice & Video", "语音与视频"),
        IconName::Mic,
    ),
    (trc!("テーマ", "Themes", "主题"), IconName::Palette),
    (
        trc!("プロフィール", "Profiles", "个人资料"),
        IconName::Pencil,
    ),
    (trc!("プラグイン", "Plugins", "插件"), IconName::Cpu),
];
/// 左の一覧のカテゴリ（見出し, ページの添字）。見出しが None の組は見出しを出さない（公式の先頭と同じ）。
/// 本家と同じく「マイアカウント」の次に「プロフィール」
const SETTINGS_GROUPS: [(Option<crate::i18n::Tr>, &[usize]); 3] = [
    (None, &[0, 4]),
    (Some(trc!("体験", "Experience", "体验")), &[2, 1, 3]),
    (Some(crate::i18n::Tr("Riscord", "Riscord", "Riscord")), &[5]),
];

/// カテゴリを平たくした並び（狭い画面の横並びタブ）
fn settings_order() -> impl Iterator<Item = usize> {
    SETTINGS_GROUPS.iter().flat_map(|(_, p)| p.iter().copied())
}
/// 設定検索で各ページに含まれる項目を探すための見出し
const SETTINGS_SECTIONS: [&[crate::i18n::Tr]; 6] = [
    &[
        trc!("アカウント情報", "Account Info", "账号信息"),
        trc!("アクティビティ", "Activity", "活动"),
        trc!("別のアカウント", "Other Accounts", "其他账号"),
        trc!("バージョン", "Version", "版本"),
    ],
    &[
        trc!("表示サイズ", "Display Size", "显示尺寸"),
        trc!("フォント", "Font", "字体"),
        trc!("メッセージ表示", "Message Display", "消息显示"),
        trc!("言語", "Language", "语言"),
        trc!("Language", "言語", "Language / 言語"),
    ],
    &[
        trc!("デバイス", "Devices", "设备"),
        trc!("音量", "Volume", "音量"),
        trc!("入力モード", "Input Mode", "输入模式"),
        trc!("入力感度", "Input Sensitivity", "输入灵敏度"),
        trc!("配信画質", "Stream Quality", "直播画质"),
    ],
    &[
        trc!("テーマ", "Themes", "主题"),
        trc!("背景画像", "Background Image", "背景图片"),
        trc!("色の調整", "Color Adjustments", "颜色调整"),
    ],
    &[trc!("プロフィール", "Profiles", "个人资料")],
    &[trc!("プラグイン", "Plugins", "插件")],
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
    let mut l = div()
        .flex_1()
        .min_w_0()
        .flex()
        .flex_col()
        .gap(z(4.0))
        .child(text(label, 16.0, WHITE()));
    if let Some(d) = desc {
        l = l.child(text(d, 14.0, MUTED()));
    }
    div()
        .mb(z(24.0))
        .min_h(z(40.0))
        .flex()
        .items_center()
        .gap(z(16.0))
        .child(l)
        .child(div().flex().items_center().gap(z(16.0)).child(right))
}

/// 設定の四角いボタン（本家の「Edit」と同じ形）。danger = 赤
fn button(id: impl Into<ElementId>, label: &'static str, danger: bool) -> Stateful<Div> {
    let (fill, hover) = if danger {
        (RED(), RED().lerp_to_gamma(Color32::BLACK, 0.15))
    } else {
        (WIDGET(), WIDGET_HOVER())
    };
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
fn radio_card(
    id: impl Into<ElementId>,
    selected: bool,
    title: &'static str,
    desc: Option<&'static str>,
) -> Stateful<Div> {
    let mark = if selected {
        div()
            .size(z(20.0))
            .flex_none()
            .rounded_full()
            .bg(WHITE())
            .flex()
            .items_center()
            .justify_center()
            .child(div().size(z(10.0)).rounded_full().bg(BLURPLE()))
    } else {
        div()
            .size(z(18.0))
            .flex_none()
            .rounded_full()
            .border_2()
            .border_color(MUTED())
    };
    let mut t = div()
        .flex_1()
        .flex()
        .flex_col()
        .child(medium(title, 16.0, WHITE()));
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
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(z(2.0))
                .child(medium(title.to_string(), 15.0, WHITE()))
                .child(text(body.to_string(), 14.0, TEXT())),
        )
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
    card(
        Color32::from_white_alpha(4),
        accent.unwrap_or(BORDER()),
        8.0,
    )
    .p(z(16.0))
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
    fn slider_state(
        &mut self,
        key: &'static str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<SliderState> {
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
        let s = cx.new(|_| {
            SliderState::new()
                .min(min)
                .max(max)
                .step(step)
                .default_value(v)
        });
        let sub = cx.subscribe_in(
            &s,
            window,
            move |this: &mut Root, _, ev: &SliderEvent, window, cx| {
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
            },
        );
        self.push_sub(sub);
        self.sliders.insert(key, s.clone());
        s
    }

    /// 見出しと値を上に、横いっぱいのスライダーを下に置く
    pub fn slider_row(
        &mut self,
        key: &'static str,
        label: &'static str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let s = self.slider_state(key, window, cx);
        div()
            .flex()
            .flex_col()
            .gap(z(8.0))
            .mb(z(16.0))
            .child(
                div()
                    .flex()
                    .items_center()
                    .child(text(label, 16.0, WHITE()).flex_1())
                    .child(text(slider_text(&self.app.settings, key), 15.0, MUTED())),
            )
            .child(Slider::new(&s))
    }

    pub fn settings_modal(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let (sw, sh) = (self.app.screen_w, self.pane_h);
        let compact = sw < 700.0;
        let (w, h) = if compact {
            ((sw - 16.0).max(0.0), (sh - 16.0).max(0.0))
        } else {
            (
                (sw - 128.0).max(560.0).min((sw - 16.0).max(0.0)),
                (sh - 90.0).max(360.0).min((sh - 16.0).max(0.0)),
            )
        };
        let wallpaper = self
            .app
            .theme_applied
            .as_ref()
            .and_then(|t| t.background.as_ref())
            .filter(|b| b.window);
        let surface = if wallpaper.is_some() {
            SETTINGS_BG()
        } else {
            opaque(SETTINGS_BG())
        };
        let nav_surface = if wallpaper.is_some() {
            SETTINGS_NAV()
        } else {
            opaque(SETTINGS_NAV())
        };
        let nav_w = if compact {
            0.0
        } else {
            (w * 0.3).clamp(200.0, 260.0)
        };
        let tab = self.app.settings_tab;

        let mut boxed = div()
            .w(z(w))
            .h(z(h))
            .relative()
            .border_1()
            .border_color(BORDER())
            .rounded(z(12.0))
            .flex()
            .flex_row()
            .overflow_hidden();
        if let Some(bg) = wallpaper {
            boxed = boxed.child(background(bg, opaque(BG())));
        }
        if !compact {
            boxed = boxed.child(div().bg(nav_surface).child(self.settings_nav(nav_w, cx)));
        }
        // 右: 見出しの帯と内容
        let title: AnyElement = if compact {
            // 狭い画面では左の一覧の代わりに、ページを横に並べる
            let mut tabs = div()
                .id("compact_tabs")
                .flex_1()
                .min_w_0()
                .flex()
                .gap(z(4.0))
                .overflow_x_scroll();
            for i in settings_order() {
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
                        .child(SETTINGS_TABS[i].0.s())
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
                    .child(tr!("ログアウト", "Log Out", "登出"))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.app.confirm_logout = true;
                        cx.notify();
                    })),
            );
            tabs.into_any_element()
        } else {
            text(SETTINGS_TABS[tab].0.s(), 16.0, WHITE())
                .flex_1()
                .into_any_element()
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
            .child(
                icon_button("settings_close", IconName::X, 32.0)
                    .tooltip(tip(tr!("閉じる (Esc)", "Close (Esc)", "关闭 (Esc)")))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.close_settings();
                        cx.notify();
                    })),
            );
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
        let mut right = div()
            .flex_1()
            .min_w_0()
            .h_full()
            .bg(surface)
            .flex()
            .flex_col()
            .relative()
            .child(bar)
            .child(scroll);
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
        div()
            .absolute()
            .inset_0()
            .child(modal("settings_modal", boxed, close, 170))
            .children(logout)
    }

    fn settings_nav(&mut self, nav_w: f32, cx: &mut Context<Self>) -> impl IntoElement {
        let a = &self.app;
        let query = a.settings_query.trim().to_lowercase();
        let matches = |s: &str| query.is_empty() || s.to_lowercase().contains(&query);
        let mut col = div()
            .w(z(nav_w))
            .h_full()
            .flex_none()
            .flex()
            .flex_col()
            .pl(z(16.0))
            .pr(z(12.0))
            .pt(z(20.0))
            .pb(z(12.0))
            .border_r_1()
            .border_color(DIVIDER());
        if let Some(me) = &a.me {
            col = col.child(
                div()
                    .flex()
                    .items_center()
                    .gap(z(12.0))
                    .pl(z(4.0))
                    .mb(z(16.0))
                    .child(avatar(
                        Some(&user_avatar_url(me)),
                        48.0,
                        me.display(),
                        24.0,
                        BLURPLE(),
                    ))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .child(
                                line(me.display().to_string(), 16.0, WHITE())
                                    .font_weight(FontWeight::MEDIUM),
                            )
                            .child(
                                div()
                                    .id("nav_edit_profile")
                                    .flex()
                                    .items_center()
                                    .gap(z(4.0))
                                    .cursor_pointer()
                                    .text_color(MUTED())
                                    .hover(|s| s.text_color(TEXT()))
                                    .child(div().text_size(z(14.0)).child(tr!(
                                        "プロフィールを編集",
                                        "Edit Profile",
                                        "编辑个人资料"
                                    )))
                                    .child(icon(IconName::Pencil, 14.0, MUTED()))
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.jump(4);
                                        cx.notify();
                                    })),
                            ),
                    ),
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
                .child(
                    div().flex_1().text_size(z(15.0)).child(
                        Input::new(&self.inp.settings_query)
                            .appearance(false)
                            .h(z(38.0)),
                    ),
                ),
        );
        let mut list = div()
            .id("settings_nav")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .mt(z(12.0))
            .flex()
            .flex_col()
            .gap(z(2.0));
        let mut any = false;
        for (head, pages) in SETTINGS_GROUPS {
            // 検索に当たったページだけ出す。1件も無いカテゴリは見出しも区切り線も出さない
            let shown: Vec<usize> = pages
                .iter()
                .copied()
                .filter(|&i| {
                    matches(SETTINGS_TABS[i].0.s())
                        || SETTINGS_SECTIONS[i].iter().any(|s| matches(s.s()))
                })
                .collect();
            if shown.is_empty() {
                continue;
            }
            if any {
                list = list.child(div().my(z(8.0)).child(hline(DIVIDER())));
            }
            any = true;
            if let Some(head) = head {
                list = list.child(text(head.s(), 13.0, MUTED()).pl(z(10.0)).mb(z(6.0)));
            }
            for i in shown {
                let (name, ic) = SETTINGS_TABS[i];
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
                        .child(text(
                            name.s(),
                            16.0,
                            if selected { WHITE() } else { TEXT() },
                        ))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.jump(i);
                            cx.notify();
                        })),
                );
            }
        }
        if !any {
            list = list.child(
                text(
                    tr!("見つかりませんでした", "Nothing found", "未找到"),
                    14.0,
                    MUTED(),
                )
                .mt(z(8.0)),
            );
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
                .child(text(tr!("ログアウト", "Log Out", "登出"), 16.0, RED()))
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
            .child(medium(tr!("ログアウト", "Log Out", "登出"), 20.0, WHITE()))
            .child(text(tr!("本当にログアウトしますか？ このPCに保存したトークンも削除されるため、次はログインし直す必要があります。", "Are you sure you want to log out? The token saved on this PC will also be deleted, so you'll need to log in again.", "确定要登出吗？保存在这台电脑上的令牌也会被删除，下次需要重新登录。"), 15.0, TEXT()).mt(z(8.0)))
            .child(
                div()
                    .mt(z(20.0))
                    .flex()
                    .justify_end()
                    .gap(z(8.0))
                    .child(text_button("logout_cancel", tr!("キャンセル", "Cancel", "取消")).min_w(z(96.0)).on_click(cx.listener(|this, _, _, cx| {
                        this.app.confirm_logout = false;
                        cx.notify();
                    })))
                    .child(fill_button("logout_ok", tr!("ログアウト", "Log Out", "登出"), RED(), Color32::WHITE, true).min_w(z(96.0)).on_click(cx.listener(|this, _, _, cx| {
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

    // ---- 各ページ ----

    fn account_page(&mut self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let mut pages = Vec::new();
        let mut s0 = section(0, tr!("アカウント情報", "Account Info", "账号信息"));
        if let Some(me) = self.app.me.clone() {
            let id = me.id.clone();
            s0 = s0
                .child(row(
                    tr!("表示名", "Display Name", "显示名称"),
                    None,
                    text(me.display().to_string(), 15.0, WHITE()),
                ))
                .child(row(
                    tr!("ユーザー名", "Username", "用户名"),
                    None,
                    text(me.username.clone(), 15.0, WHITE()),
                ))
                .child(row(
                    tr!("ユーザーID", "User ID", "用户 ID"),
                    None,
                    div()
                        .flex()
                        .items_center()
                        .gap(z(16.0))
                        .child(text(me.id.clone(), 15.0, WHITE()))
                        .child(
                            button("copy_my_id", tr!("コピー", "Copy", "复制"), false).on_click(
                                cx.listener(move |this, _, _, cx| {
                                    cx.write_to_clipboard(ClipboardItem::new_string(id.clone()));
                                    this.app
                                        .show_toast(tr!("コピーしました", "Copied", "已复制"));
                                    cx.notify();
                                }),
                            ),
                        ),
                ));
        }
        pages.push(s0.into_any_element());
        let on = self.app.settings.show_activity;
        pages.push(
            section(1, tr!("アクティビティ", "Activity", "活动"))
                .child(row(
                    tr!("「Riscord をプレイ中」を表示", "Show \"Playing Riscord\"", "显示“正在玩 Riscord”"),
                    Some(tr!("Riscord を使っている間、プロフィールのアクティビティに表示します。他の人にも見えます（オフライン表示のときは出ません）。", "Shows it as your profile activity while you use Riscord. Others can see it (not shown while Invisible).", "使用 Riscord 时，在个人资料的活动中显示。其他人也能看到（隐身时不显示）。").into()),
                    toggle_switch("activity_switch", on).on_click(cx.listener(move |this, _, _, cx| {
                        this.app.settings.show_activity = !on;
                        this.app.send(Cmd::SetActivity(!on));
                        this.app.save_settings();
                        cx.notify();
                    })),
                ))
                .into_any_element(),
        );
        let state = match crate::update::status() {
            crate::update::Status::Off => tr!(
                "自動アップデートは無効です（開発中のビルド・対応していない環境など）",
                "Auto-update is disabled (development build, unsupported environment, etc.)",
                "自动更新已禁用（开发版本、不支持的环境等）"
            )
            .to_string(),
            crate::update::Status::Checking => tr!(
                "新しい版を確認しています…",
                "Checking for a new version…",
                "正在检查新版本…"
            )
            .into(),
            crate::update::Status::Latest => tr!(
                "最新の版です",
                "You're on the latest version",
                "已是最新版本"
            )
            .into(),
            crate::update::Status::Downloading(v) => {
                tr!(
                    "{v} をダウンロードしています…",
                    "Downloading {v}…",
                    "正在下载 {v}…",
                )
            }
            crate::update::Status::Ready(v) => {
                tr!(
                    "{v} に更新しました。次に起動したときから使われます",
                    "Updated to {v}. It will be used from the next launch",
                    "已更新到 {v}。下次启动时生效",
                )
            }
            crate::update::Status::Failed(e) => {
                tr!(
                    "更新できませんでした: {e}",
                    "Couldn't update: {e}",
                    "无法更新：{e}",
                )
            }
        };
        pages.push(self.profiles_section(cx));
        let version = match crate::commits::build_short() {
            Some(sha) => format!("Riscord v{} ({sha})", crate::update::current_version()),
            None => format!("Riscord v{}", crate::update::current_version()),
        };
        pages.push(
            section(3, tr!("バージョン", "Version", "版本"))
                .child(row(version, Some(state), div()))
                .child(Self::commits_rows())
                .into_any_element(),
        );
        pages
    }

    /// GitHub の main に来た新しいコミット（src/commits.rs）
    fn commits_rows() -> Div {
        use crate::commits::Status;
        crate::commits::mark_seen();
        let status = crate::commits::status();
        let desc = match &status {
            Status::Off => tr!(
                "ビルド元のコミットが分からないため確認していません",
                "Not checked because the build's commit is unknown",
                "由于不知道构建所用的提交，未进行检查"
            )
            .to_string(),
            Status::Checking => tr!("確認しています…", "Checking…", "正在检查…").into(),
            Status::UpToDate => tr!(
                "このビルドは main の最新です",
                "This build is the latest on main",
                "此版本是 main 上的最新版本"
            )
            .into(),
            Status::Behind { ahead_by, .. } => {
                tr!(
                    "main に {ahead_by} 件の新しいコミットがあります",
                    "main has {ahead_by} new commits",
                    "main 上有 {ahead_by} 个新提交",
                )
            }
            Status::Unknown(e) => e.clone(),
            Status::Failed(e) => tr!(
                "確認できませんでした: {e}",
                "Couldn't check: {e}",
                "无法确认：{e}",
            ),
        };
        let open = match &status {
            Status::Behind { compare_url, .. } => compare_url.clone(),
            _ => "https://github.com/kokorodonguri/Riscord/commits/main".into(),
        };
        let mut d = div().flex().flex_col().child(row(
            tr!(
                "GitHub の新しいコミット",
                "New commits on GitHub",
                "GitHub 上的新提交"
            ),
            Some(desc),
            button(
                "commits_open",
                tr!("GitHub で見る", "View on GitHub", "在 GitHub 上查看"),
                false,
            )
            .on_click(move |_, _, cx| cx.open_url(&open)),
        ));
        if let Status::Behind { commits, .. } = status {
            let mut list = div().flex().flex_col().gap(z(8.0)).mb(z(24.0));
            for c in commits {
                let url = c.url.clone();
                list = list.child(
                    div()
                        .id(SharedString::from(format!("commit-{}", c.sha)))
                        .flex()
                        .items_center()
                        .gap(z(12.0))
                        .cursor_pointer()
                        .on_click(move |_, _, cx| cx.open_url(&url))
                        .child(
                            text(c.sha.chars().take(7).collect::<String>(), 14.0, LINK())
                                .flex_none(),
                        )
                        .child(text(c.title, 14.0, WHITE()).flex_1().min_w_0().truncate())
                        .child(text(c.date, 13.0, MUTED()).flex_none()),
                );
            }
            d = d.child(list);
        }
        d
    }

    /// 別のアカウント。同じウィンドウで横に並べる（分割表示）か、別のウィンドウ（別のプロセス）で開く
    fn profiles_section(&mut self, cx: &mut Context<Self>) -> AnyElement {
        use crate::profile;
        let current = self.app.profile.clone();
        let mut s = section(2, tr!("別のアカウント", "Other Accounts", "其他账号")).child(
            text(tr!("「横に並べる」で、このウィンドウを左右に分けて2つのアカウントを同時に使えます。別のウィンドウで開くこともできます。どれも同時にログインしたまま使え、通話にもそれぞれ入れます。", "\"Side by side\" splits this window so you can use two accounts at once. You can also open one in a separate window. All of them stay logged in at the same time and can each join calls.", "使用“并排显示”可将此窗口左右分开，同时使用两个账号。也可以在单独的窗口中打开。所有账号都能同时保持登录，并可各自加入通话。"), 15.0, MUTED()).mb(z(16.0)),
        );
        let names: Vec<Option<String>> = std::iter::once(None)
            .chain(profile::list().into_iter().map(Some))
            .collect();
        for name in names {
            let here = name == current;
            let beside = self.other.as_ref() == Some(&name);
            let running = here || beside || profile::is_running(name.as_deref());
            let title = profile::label(name.as_deref());
            let who = profile::account_name(name.as_deref())
                .unwrap_or_else(|| tr!("未ログイン", "Not logged in", "未登录").into());
            let state = if here {
                tr!("この区画", "This pane", "此区域")
            } else if beside {
                tr!("横に並べています", "Side by side", "正在并排显示")
            } else if running {
                tr!(
                    "別のウィンドウで開いています",
                    "Open in another window",
                    "已在其他窗口中打开"
                )
            } else {
                tr!("閉じています", "Closed", "已关闭")
            };
            let key = name.clone().unwrap_or_default();
            let mut actions = div().flex().items_center().gap(z(8.0));
            if !running {
                let n = name.clone();
                actions = actions.child(
                    button(
                        SharedString::from(format!("profile_split_{key}")),
                        tr!("横に並べる", "Side by Side", "并排显示"),
                        false,
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.app.settings_open = false;
                        cx.emit(PaneEvent::OpenSplit(n.clone()));
                    })),
                );
                let n = name.clone();
                actions = actions.child(
                    button(
                        SharedString::from(format!("profile_open_{key}")),
                        tr!("別ウィンドウ", "New Window", "新窗口"),
                        false,
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if let Err(e) = profile::launch(n.as_deref()) {
                            this.app.show_toast(&tr!(
                                "ウィンドウを開けませんでした: {e}",
                                "Couldn't open the window: {e}",
                                "无法打开窗口：{e}",
                            ));
                        }
                        cx.notify();
                    })),
                );
                if let Some(n) = name.clone() {
                    let confirming = self.confirm_profile_delete.as_deref() == Some(n.as_str());
                    actions = actions.child(
                        button(
                            SharedString::from(format!("profile_del_{key}")),
                            if confirming {
                                tr!("本当に削除", "Really delete", "确认删除")
                            } else {
                                tr!("削除", "Delete", "删除")
                            },
                            true,
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if this.confirm_profile_delete.as_deref() != Some(n.as_str()) {
                                this.confirm_profile_delete = Some(n.clone());
                            } else {
                                this.confirm_profile_delete = None;
                                match profile::remove(&n) {
                                    Ok(()) => this.app.show_toast(tr!(
                                        "サブアカウントを削除しました",
                                        "Sub account deleted",
                                        "已删除副账号"
                                    )),
                                    Err(e) => this.app.show_toast(&tr!(
                                        "削除できませんでした: {e}",
                                        "Couldn't delete: {e}",
                                        "无法删除：{e}",
                                    )),
                                }
                            }
                            cx.notify();
                        })),
                    );
                }
            }
            s = s.child(row(title, Some(format!("{who} · {state}")), actions));
        }
        s = s.child(
            div().mt(z(8.0)).child(
                button(
                    "profile_add",
                    tr!(
                        "サブアカウントを追加して横に並べる",
                        "Add a sub account side by side",
                        "添加副账号并排显示"
                    ),
                    false,
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    let name = profile::next_name(&profile::list());
                    match profile::create(&name) {
                        Ok(()) => {
                            this.app.settings_open = false;
                            cx.emit(PaneEvent::OpenSplit(Some(name)));
                        }
                        Err(e) => this.app.show_toast(&tr!(
                            "サブアカウントを作れませんでした: {e}",
                            "Couldn't create the sub account: {e}",
                            "无法创建副账号：{e}",
                        )),
                    }
                    cx.notify();
                })),
            ),
        );
        s.into_any_element()
    }

    fn display_page(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let mut pages = Vec::new();
        pages.push(
            section(0, tr!("表示サイズ", "Display Size", "显示尺寸"))
                .child(self.slider_row(
                    "zoom",
                    tr!("UIの拡大率", "UI Zoom", "界面缩放"),
                    window,
                    cx,
                ))
                .child(self.slider_row(
                    "font_size",
                    tr!("メッセージの文字サイズ", "Message Font Size", "消息字号"),
                    window,
                    cx,
                ))
                .into_any_element(),
        );
        pages.push(self.font_section(window, cx));
        let mut s1 = section(1, tr!("メッセージ表示", "Message Display", "消息显示"));
        for (compact, title, desc) in [
            (
                false,
                tr!("標準", "Cozy", "舒适"),
                tr!(
                    "アバターと名前を付けて表示します",
                    "Shows avatars and names",
                    "显示头像和名称"
                ),
            ),
            (
                true,
                tr!("コンパクト", "Compact", "紧凑"),
                tr!(
                    "1行にまとめて、より多くのメッセージを表示します",
                    "Fits each message on one line to show more messages",
                    "每条消息只占一行，以显示更多消息"
                ),
            ),
        ] {
            s1 = s1.child(
                radio_card(
                    SharedString::from(format!("compact{compact}")),
                    self.app.settings.compact == compact,
                    title,
                    Some(desc),
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    if this.app.settings.compact != compact {
                        this.app.settings.compact = compact;
                        this.app.save_settings();
                    }
                    cx.notify();
                })),
            );
        }
        s1 = s1.child(div().h(z(16.0))).child(row(
            tr!(
                "表示設定を初期化",
                "Reset Appearance Settings",
                "重置外观设置"
            ),
            Some(
                tr!(
                    "このページの項目だけを初期値に戻します（テーマ・音声は変わりません）",
                    "Resets only the items on this page (themes and voice stay unchanged)",
                    "仅将此页面的项目恢复为初始值（主题和语音不变）"
                )
                .into(),
            ),
            button("reset_display", tr!("初期化", "Reset", "重置"), false).on_click(cx.listener(
                |this, _, window, cx| {
                    let d = Settings::default();
                    let s = &mut this.app.settings;
                    (s.zoom, s.font_size, s.compact) = (d.zoom, d.font_size, d.compact);
                    if !s.font.is_empty() {
                        s.font.clear();
                        this.app.font_dirty = true;
                    }
                    set_zoom(d.zoom);
                    this.app.theme_dirty = true;
                    this.app.save_settings();
                    window.refresh();
                    cx.notify();
                },
            )),
        ));
        pages.push(s1.into_any_element());

        // 言語。どの言語でも見つけられるよう、見出しは英語も添える
        let mut s2 = section(
            2,
            tr!("言語 / Language", "Language / 言語", "语言 / Language"),
        );
        let current = crate::i18n::setting();
        for (value, title, desc) in [
            (
                crate::i18n::AUTO,
                tr!("自動", "Automatic", "自动"),
                tr!(
                    "OS の言語に合わせます（日本語・中国語以外は英語）",
                    "Follows the OS language (English unless it's Japanese or Chinese)",
                    "跟随操作系统语言（日语、中文以外为英语）"
                ),
            ),
            // 名前はその言語で書き、説明に他の言語での呼び名を添える
            (crate::i18n::JA, "日本語", "Japanese / 日语"),
            (crate::i18n::EN, "English", "英語 / 英语"),
            (crate::i18n::ZH, "简体中文", "Chinese (Simplified) / 中国語"),
        ] {
            s2 = s2.child(
                radio_card(
                    SharedString::from(format!("lang_{value}")),
                    current == value,
                    title,
                    Some(desc),
                )
                .on_click(cx.listener(move |this, _, window, cx| {
                    if crate::i18n::setting() != value {
                        if let Err(e) = crate::app::choose_language(value) {
                            this.app.status = tr!(
                                "言語を保存できませんでした: {e}",
                                "Couldn't save the language: {e}",
                                "无法保存语言：{e}",
                            );
                        }
                        this.app.theme_dirty = true;
                        window.refresh();
                    }
                    cx.notify();
                })),
            );
        }
        s2 = s2.child(
            text(
                tr!(
                    "入力欄の案内文など一部の表示は、再起動すると切り替わります。",
                    "Some text, such as input placeholders, switches after a restart.",
                    "输入框提示文字等部分显示内容需重启后才会切换。"
                ),
                13.0,
                MUTED(),
            )
            .mt(z(8.0)),
        );
        pages.push(s2.into_any_element());
        pages
    }

    /// 表示 → フォント。入っているフォントから選ぶ（絞り込み欄つき）
    fn font_section(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        if self.font_names.is_none() {
            let mut names: Vec<SharedString> = cx
                .text_system()
                .all_font_names()
                .into_iter()
                // GPUI が内部で使う名前（".SystemUIFont" など）は出さない
                .filter(|n| !n.starts_with('.'))
                .map(SharedString::from)
                .collect();
            names.sort_by_key(|n| n.to_lowercase());
            names.dedup();
            self.font_names = Some(names);
        }
        let filter = match &self.font_filter {
            Some(f) => f.clone(),
            None => {
                let f = new_input(
                    window,
                    cx,
                    tr!("フォントを検索", "Search fonts", "搜索字体"),
                    false,
                );
                let sub = cx.subscribe_in(&f, window, |_: &mut Root, _, ev: &InputEvent, _, cx| {
                    if let InputEvent::Change = ev {
                        cx.notify();
                    }
                });
                self.push_sub(sub);
                self.font_filter = Some(f.clone());
                f
            }
        };
        let q = filter.read(cx).value().to_lowercase();
        let cur = self.app.settings.font.clone();
        let names = self.font_names.clone().unwrap_or_default();
        let mut l = div()
            .id("fontlist")
            .max_h(z(240.0))
            .overflow_y_scroll()
            .bg(RAIL())
            .rounded(z(8.0))
            .p(z(4.0))
            .flex()
            .flex_col();
        let auto = tr!(
            "自動（テーマか既定のフォント）",
            "Automatic (theme or default font)",
            "自动（主题或默认字体）"
        );
        let rows = std::iter::once((String::new(), SharedString::from(auto)))
            .filter(|_| q.is_empty())
            .chain(
                names
                    .iter()
                    .filter(|n| q.is_empty() || n.to_lowercase().contains(&q))
                    .map(|n| (n.to_string(), n.clone())),
            );
        let mut any = false;
        for (i, (value, label)) in rows.enumerate() {
            any = true;
            let on = cur == value;
            l = l.child(
                div()
                    .id(("fontrow", i))
                    .flex_none()
                    .h(z(32.0))
                    .px(z(8.0))
                    .rounded(z(6.0))
                    .flex()
                    .items_center()
                    .cursor_pointer()
                    .when(on, |x| x.bg(SELECTED()))
                    .hover(|s| s.bg(if on { SELECTED() } else { HOVER() }))
                    .child(line(label, 14.0, if on { WHITE() } else { TEXT() }))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if this.app.settings.font != value {
                            this.app.settings.font = value.clone();
                            this.app.font_dirty = true;
                            this.app.save_settings();
                        }
                        cx.notify();
                    })),
            );
        }
        if !any {
            l = l.child(
                div()
                    .h(z(32.0))
                    .px(z(8.0))
                    .flex()
                    .items_center()
                    .child(text(
                        tr!("見つかりません", "No fonts found", "未找到字体"),
                        14.0,
                        MUTED(),
                    )),
            );
        }
        let missing = !cur.is_empty() && self.user_font.is_none();
        let desc = if missing {
            tr!(
                "「{cur}」が見つからないので、テーマか既定のフォントを使っています",
                "\"{cur}\" isn't installed, so the theme or default font is used",
                "未找到“{cur}”，正在使用主题或默认字体",
            )
        } else {
            tr!(
                "アプリ全体の文字に使います。テーマで指定したフォントより優先します",
                "Used for all text in the app. Takes priority over the theme's font",
                "用于应用中的所有文字，优先于主题指定的字体"
            )
            .to_string()
        };
        section(1, tr!("フォント", "Font", "字体"))
            .child(text(desc, 14.0, MUTED()).mb(z(12.0)))
            .child(
                div()
                    .border_1()
                    .border_color(BORDER())
                    .rounded(z(8.0))
                    .px(z(10.0))
                    .mb(z(8.0))
                    .flex()
                    .items_center()
                    .gap(z(6.0))
                    .child(icon(IconName::Search, 18.0, MUTED()))
                    .child(
                        div()
                            .flex_1()
                            .text_size(z(15.0))
                            .child(Input::new(&filter).appearance(false).h(z(38.0))),
                    ),
            )
            .child(l)
            .child(
                text(
                    tr!(
                        "あいうえお 漢字 ABC abc 123",
                        "The quick brown fox — あいう 漢字 123",
                        "敏捷的狐狸 あいう ABC 123"
                    ),
                    18.0,
                    TEXT(),
                )
                .mt(z(12.0)),
            )
            .into_any_element()
    }

    fn voice_page(
        &mut self,
        col_w: f32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        // レベルメーターを動かすため、開いている間は描き直し続ける
        self.app.repaint_after(std::time::Duration::from_millis(50));
        if self.app.audio_devs.is_none() {
            self.app.audio_devs = Some((voice::input_devices(), voice::output_devices()));
        }
        let (ins, outs) = self.app.audio_devs.clone().unwrap_or_default();
        let mut pages = Vec::new();
        let mut cols = div()
            .flex()
            .gap(z(16.0))
            .when(col_w < 500.0, |d| d.flex_col());
        for (input, title, list) in [
            (true, tr!("入力デバイス", "Input Device", "输入设备"), ins),
            (
                false,
                tr!("出力デバイス", "Output Device", "输出设备"),
                outs,
            ),
        ] {
            let cur = if input {
                self.app.settings.input_device.clone()
            } else {
                self.app.settings.output_device.clone()
            };
            let mut l = div()
                .id(SharedString::from(format!("devlist{input}")))
                .max_h(z(200.0))
                .overflow_y_scroll()
                .bg(RAIL())
                .rounded(z(8.0))
                .p(z(4.0))
                .flex()
                .flex_col();
            for (i, d) in std::iter::once(None)
                .chain(list.into_iter().map(Some))
                .enumerate()
            {
                let on = cur == d;
                let label = d
                    .clone()
                    .unwrap_or_else(|| tr!("デフォルト", "Default", "默认").into());
                l = l.child(
                    div()
                        .id(("devrow", i))
                        .h(z(32.0))
                        .px(z(8.0))
                        .rounded(z(6.0))
                        .flex()
                        .items_center()
                        .cursor_pointer()
                        .when(on, |x| x.bg(SELECTED()))
                        .hover(|s| s.bg(if on { SELECTED() } else { HOVER() }))
                        .child(line(label, 14.0, if on { WHITE() } else { TEXT() }))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            let s = &mut this.app.settings;
                            if input {
                                s.input_device = d.clone();
                            } else {
                                s.output_device = d.clone();
                            }
                            this.app.apply_voice_settings();
                            this.app.save_settings();
                            cx.notify();
                        })),
                );
            }
            cols = cols.child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(z(6.0))
                    .child(text(title, 12.0, MUTED()))
                    .child(l),
            );
        }
        pages.push(
            section(0, tr!("デバイス", "Devices", "设备"))
                .child(cols)
                .child(div().h(z(12.0)))
                .child(row(
                    tr!(
                        "デバイスが見つからないとき",
                        "When a device isn't found",
                        "找不到设备时"
                    ),
                    Some(
                        tr!(
                            "つなぎ直したマイクやヘッドホンを読み込み直します",
                            "Reloads reconnected microphones and headphones",
                            "重新加载重新连接的麦克风和耳机"
                        )
                        .into(),
                    ),
                    button("redetect", tr!("再検出", "Rescan", "重新检测"), false).on_click(
                        cx.listener(|this, _, _, cx| {
                            this.app.audio_devs = None;
                            cx.notify();
                        }),
                    ),
                ))
                .into_any_element(),
        );
        pages.push(
            section(1, tr!("音量", "Volume", "音量"))
                .child(self.slider_row(
                    "in_gain",
                    tr!("入力音量", "Input Volume", "输入音量"),
                    window,
                    cx,
                ))
                .child(self.slider_row(
                    "out_gain",
                    tr!("出力音量", "Output Volume", "输出音量"),
                    window,
                    cx,
                ))
                .into_any_element(),
        );
        let mut s2 = section(2, tr!("入力モード", "Input Mode", "输入模式"));
        for (ptt, title, desc) in [
            (
                false,
                tr!("音声検出", "Voice Activity", "语音检测"),
                tr!(
                    "話し始めると自動で送信します",
                    "Transmits automatically when you start talking",
                    "开始说话时自动发送"
                ),
            ),
            (
                true,
                tr!("プッシュトゥトーク", "Push to Talk", "按键说话"),
                tr!(
                    "キーを押している間だけ送信します",
                    "Transmits only while you hold the key",
                    "仅在按住按键时发送"
                ),
            ),
        ] {
            s2 = s2.child(
                radio_card(
                    SharedString::from(format!("ptt{ptt}")),
                    self.app.settings.ptt == ptt,
                    title,
                    Some(desc),
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    if this.app.settings.ptt != ptt {
                        this.app.settings.ptt = ptt;
                        this.app.apply_voice_settings();
                        this.app.save_settings();
                    }
                    cx.notify();
                })),
            );
        }
        if self.app.settings.ptt {
            let label = if self.app.binding_ptt {
                tr!(
                    "キーを押してください…（Escで取消）",
                    "Press a key… (Esc to cancel)",
                    "请按下按键…（Esc 取消）"
                )
                .to_string()
            } else {
                self.app
                    .settings
                    .ptt_key
                    .clone()
                    .unwrap_or_else(|| tr!("未設定", "Not set", "未设置").into())
            };
            s2 = s2
                .child(
                    text(tr!("ショートカット", "Shortcut", "快捷键"), 12.0, MUTED())
                        .mt(z(16.0))
                        .mb(z(6.0)),
                )
                .child(
                    div().flex().child(
                        small_button("bind_ptt", label)
                            .h(z(36.0))
                            .min_w(z(240.0))
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.app.binding_ptt = true;
                                window.focus(&this.focus, cx);
                                cx.notify();
                            })),
                    ),
                )
                .child(
                    text(
                        tr!(
                            "このウィンドウが前面にあるときだけ有効です",
                            "Works only while this window is in front",
                            "仅在此窗口位于前台时有效"
                        ),
                        12.0,
                        MUTED(),
                    )
                    .mt(z(6.0)),
                );
        }
        pages.push(s2.into_any_element());
        // 入力感度（レベルメーターとしきい値）
        let level = self.app.voice.level();
        let thr = db_to_lin(self.app.settings.vad_db);
        let frac = |db: f32| ((db + 80.0) / 80.0).clamp(0.0, 1.0);
        let ptt = self.app.settings.ptt;
        let color = if ptt || level >= thr {
            GREEN()
        } else {
            ORANGE()
        };
        let meter_w = col_w.min(320.0);
        let mut meter = div()
            .relative()
            .w(z(meter_w))
            .h(z(10.0))
            .rounded(z(5.0))
            .bg(INPUT())
            .child(
                div()
                    .absolute()
                    .left_0()
                    .top_0()
                    .bottom_0()
                    .w(z(meter_w * frac(lin_to_db(level))))
                    .rounded(z(5.0))
                    .bg(color),
            );
        if !ptt {
            meter = meter.child(
                div()
                    .absolute()
                    .top(z(-3.0))
                    .bottom(z(-3.0))
                    .left(z(meter_w * frac(self.app.settings.vad_db) - 1.0))
                    .w(z(2.0))
                    .bg(WHITE()),
            );
        }
        let noise = self.app.settings.noise_suppression;
        let mut s3 = section(3, tr!("入力感度", "Input Sensitivity", "输入灵敏度"))
            .child(row(
                tr!("ノイズ抑制", "Noise Suppression", "噪声抑制"),
                Some(tr!("キーボードの音や空調など、声以外の雑音を減らします。少し CPU を使い、音声が 10 ms 遅れます。", "Reduces non-voice noise like keyboard sounds and air conditioning. Uses a little CPU and delays audio by 10 ms.", "减少键盘声、空调声等人声以外的噪音。会占用少量 CPU，并使音频延迟 10 ms。").into()),
                toggle_switch("noise_switch", noise).on_click(cx.listener(move |this, _, _, cx| {
                    this.app.settings.noise_suppression = !noise;
                    this.app.apply_voice_settings();
                    this.app.save_settings();
                    cx.notify();
                })),
            ))
            .child(meter.mb(z(12.0)));
        if !ptt {
            s3 = s3
                .child(self.slider_row("vad_db", tr!("しきい値", "Threshold", "阈值"), window, cx))
                .child(text(
                    tr!(
                        "話したときにバーが緑になれば送信されます",
                        "Your voice is transmitted when the bar turns green while talking",
                        "说话时进度条变绿即表示正在发送"
                    ),
                    12.0,
                    MUTED(),
                ));
        }
        pages.push(s3.into_any_element());
        use crate::golive::media::QualityPreset;
        let mut quality = section(4, tr!("配信画質", "Stream Quality", "直播画质")).child(text(
            tr!("画面共有を開始する前に選択します。利用できる画質は接続先や端末によって異なります。", "Choose before starting screen sharing. Available quality depends on the connection and device.", "请在开始屏幕共享前选择。可用画质因连接目标和设备而异。"),
            14.0,
            MUTED(),
        ).mb(z(12.0)));
        for (preset, label) in [
            (QualityPreset::P360_30, "360p / 30 fps"),
            (QualityPreset::P360_60, "360p / 60 fps"),
            (QualityPreset::P720_30, "720p / 30 fps"),
            (QualityPreset::P720_60, "720p / 60 fps"),
            (QualityPreset::P1080_30, "1080p / 30 fps"),
            (QualityPreset::P1080_60, "1080p / 60 fps"),
        ] {
            quality = quality.child(
                radio_card(
                    SharedString::from(format!("stream-quality-{preset:?}")),
                    self.app.settings.stream_quality == preset,
                    label,
                    None,
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.app.settings.stream_quality = preset;
                    this.app.save_settings();
                    cx.notify();
                })),
            );
        }
        pages.push(quality.into_any_element());
        pages
    }

    fn plugins_page(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let path = crate::plugins::directory();
        let mut page = section(0, tr!("プラグイン", "Plugins", "插件"))
            .child(text(
                tr!(
                    "プラグインフォルダ: {}",
                    "Plugin folder: {}",
                    "插件文件夹：{}",
                    path.display()
                ),
                14.0,
                MUTED(),
            ))
            .child(
                div()
                    .flex()
                    .gap(z(10.0))
                    .mt(z(12.0))
                    .child(
                        action_button(
                            "open_plugins",
                            IconName::Folder,
                            tr!("フォルダを開く", "Open Folder", "打开文件夹"),
                        )
                        .on_click(move |_, _, _| open_folder(&path)),
                    )
                    .child(
                        action_button(
                            "reload_plugins",
                            IconName::RefreshCw,
                            tr!("読み込み直す", "Reload", "重新加载"),
                        )
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.app.plugins.reload(&this.app.settings.enabled_plugins);
                            this.plugin_inputs.clear();
                            this.app.waker.wake();
                            cx.notify();
                        })),
                    ),
            );
        page = page.child(
            div()
                .flex()
                .flex_wrap()
                .gap(z(10.0))
                .mt(z(12.0))
                .child(
                    action_button(
                        "enable_all_plugins",
                        IconName::Check,
                        tr!("すべてオン", "Turn All On", "全部开启"),
                    )
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.app
                            .plugins
                            .set_all(true, &mut this.app.settings.enabled_plugins);
                        this.app.save_settings();
                        this.app.waker.wake();
                        cx.notify();
                    })),
                )
                .child(
                    action_button(
                        "disable_all_plugins",
                        IconName::X,
                        tr!("すべてオフ", "Turn All Off", "全部关闭"),
                    )
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.app
                            .plugins
                            .set_all(false, &mut this.app.settings.enabled_plugins);
                        this.plugin_inputs.clear();
                        this.app.save_settings();
                        this.app.waker.wake();
                        cx.notify();
                    })),
                ),
        );
        for error in &self.app.plugins.errors {
            page = page.child(notice(
                RED(),
                tr!("読み込みエラー", "Load error", "加载错误"),
                error,
            ));
        }
        for p in &self.app.plugins.entries {
            let id = p.manifest.id.clone();
            let page_id = id.clone();
            let enabled = self.app.settings.enabled_plugins.contains(&id);
            let status = if p.running() {
                tr!("実行中", "Running", "运行中")
            } else if enabled {
                tr!("停止中", "Stopped", "已停止")
            } else {
                tr!("無効", "Disabled", "已禁用")
            };
            let desc = format!(
                "{} · {} · {}\n{}{}",
                p.manifest.id,
                p.manifest.version,
                status,
                p.manifest.description,
                p.error
                    .as_ref()
                    .map(|e| tr!("\nエラー: {e}", "\nError: {e}", "\n错误：{e}",))
                    .unwrap_or_default()
            );
            page = page.child(
                div().mt(z(16.0)).child(
                    settings_card(None).child(row(
                        p.manifest.name.clone(),
                        Some(desc),
                        div()
                            .flex()
                            .gap(z(8.0))
                            .when(p.has_page(), |d| {
                                d.child(
                                    div()
                                        .id(SharedString::from(format!("plugin-page-{id}")))
                                        .cursor_pointer()
                                        .child(text(tr!("ページ", "Page", "页面"), 14.0, LINK()))
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.app.plugins.selected = Some(page_id.clone());
                                            cx.notify();
                                        })),
                                )
                            })
                            .child(
                                toggle_switch(
                                    SharedString::from(format!("plugin-toggle-{id}")),
                                    enabled,
                                )
                                .tooltip(tip(if enabled {
                                    tr!("無効にする", "Disable", "禁用")
                                } else {
                                    tr!("有効にする", "Enable", "启用")
                                }))
                                .on_click(cx.listener(
                                    move |this, _, _, cx| {
                                        this.app
                                            .plugins
                                            .toggle(&id, &mut this.app.settings.enabled_plugins);
                                        this.app.save_settings();
                                        this.app.waker.wake();
                                        cx.notify();
                                    },
                                )),
                            ),
                    )),
                ),
            );
        }
        if let Some(id) = self.app.plugins.selected.clone().filter(|id| {
            self.app
                .plugins
                .entries
                .iter()
                .any(|p| p.manifest.id == *id && p.has_page())
        }) {
            page = page.child(div().mt(z(24.0)).child(medium(
                tr!("{id} のページ", "{id} page", "{id} 页",),
                20.0,
                WHITE(),
            )));
            for (index, item) in self.app.plugins.page(&id).into_iter().enumerate() {
                if item.kind == "button" {
                    let id = id.clone();
                    let action = item.action;
                    page = page.child(
                        div()
                            .id(SharedString::from(format!("plugin-action-{index}")))
                            .mt(z(10.0))
                            .cursor_pointer()
                            .child(text(item.label, 16.0, LINK()))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                // MessageLogger の記録は本体が持っているので、消すのも本体で行う
                                if id == "message-logger" && action == "clear" {
                                    this.app.message_log.clear();
                                    this.app.show_toast(tr!(
                                        "メッセージの記録を消しました",
                                        "Message log cleared",
                                        "已清除消息记录"
                                    ));
                                }
                                let values = this
                                    .plugin_inputs
                                    .iter()
                                    .filter(|((plugin, _), _)| plugin == &id)
                                    .map(|((_, key), input)| {
                                        (key.clone(), input.read(cx).value().to_string())
                                    })
                                    .collect();
                                this.app.plugins.page_action(&id, &action, values);
                                this.app.waker.wake();
                                cx.notify();
                            })),
                    );
                } else if item.kind == "switch" {
                    let plugin_id = id.clone();
                    let action = item.action;
                    page = page.child(
                        div()
                            .mt(z(10.0))
                            .flex()
                            .items_center()
                            .gap(z(12.0))
                            .child(text(item.label, 16.0, TEXT()).flex_1())
                            .child(
                                toggle_switch(
                                    SharedString::from(format!("plugin-switch-{index}")),
                                    item.enabled,
                                )
                                .tooltip(tip(if item.enabled {
                                    tr!("オフにする", "Turn Off", "关闭")
                                } else {
                                    tr!("オンにする", "Turn On", "开启")
                                }))
                                .on_click(cx.listener(
                                    move |this, _, _, cx| {
                                        let values = this
                                            .plugin_inputs
                                            .iter()
                                            .filter(|((plugin, _), _)| plugin == &plugin_id)
                                            .map(|((_, key), input)| {
                                                (key.clone(), input.read(cx).value().to_string())
                                            })
                                            .collect();
                                        this.app.plugins.page_action(&plugin_id, &action, values);
                                        this.app.waker.wake();
                                        cx.notify();
                                    },
                                )),
                            ),
                    );
                } else if item.kind == "text" {
                    page = page.child(div().mt(z(10.0)).child(text(item.label, 16.0, TEXT())));
                } else if item.kind == "input" && !item.action.is_empty() {
                    let key = (id.clone(), item.action.clone());
                    let input = self
                        .plugin_inputs
                        .entry(key)
                        .or_insert_with(|| super::new_input(window, cx, &item.label, false))
                        .clone();
                    page = page.child(
                        div()
                            .mt(z(10.0))
                            .child(text(item.label, 14.0, TEXT()))
                            .child(
                                div()
                                    .mt(z(6.0))
                                    .bg(INPUT())
                                    .border_1()
                                    .border_color(BORDER())
                                    .rounded(z(8.0))
                                    .px(z(6.0))
                                    .child(Input::new(&input).appearance(false).h(z(36.0))),
                            ),
                    );
                }
            }
            if id == "message-logger" {
                page = page.child(self.message_log_view(cx));
            }
        }
        vec![page.into_any_element()]
    }

    /// MessageLogger のログ一覧（削除・編集・ゴーストピン）。押すとそのチャンネルを開く
    fn message_log_view(&mut self, cx: &mut Context<Self>) -> AnyElement {
        /// 一度に並べる件数
        const SHOWN: usize = 200;
        self.app.message_log.ensure_loaded();
        let a = &self.app;
        let me = a.me.as_ref().map(|u| u.id.as_str());
        let log = &a.message_log;
        let ping =
            |m: &crate::model::Message| me.is_some_and(|me| m.mentions.iter().any(|u| u.id == me));
        let pings = log.deleted.values().filter(|d| ping(&d.message)).count();
        let counts = [log.deleted.len(), log.edited.len(), pings];
        let tab = self.log_tab.min(2);
        // (並べ替えの時刻, メッセージ, 本文の行)
        let mut entries: Vec<(String, &crate::model::Message, Vec<(String, Color32)>)> = match tab {
            1 => log
                .edited
                .values()
                .map(|e| {
                    let mut lines: Vec<(String, Color32)> = e
                        .history
                        .iter()
                        .map(|h| {
                            (
                                tr!("編集前: {}", "Before edit: {}", "编辑前：{}", h.content),
                                FAINT(),
                            )
                        })
                        .collect();
                    lines.push((
                        tr!("現在: {}", "Now: {}", "当前：{}", e.message.content),
                        TEXT(),
                    ));
                    (
                        e.history.last().map(|h| h.at.clone()).unwrap_or_default(),
                        &e.message,
                        lines,
                    )
                })
                .collect(),
            _ => log
                .deleted
                .values()
                .filter(|d| tab == 0 || ping(&d.message))
                .map(|d| {
                    let mut lines = vec![(d.message.content.clone(), TEXT())];
                    for f in &d.message.attachments {
                        lines.push((
                            tr!("添付: {}", "Attachments: {}", "附件：{}", f.filename),
                            FAINT(),
                        ));
                    }
                    (d.at.clone(), &d.message, lines)
                })
                .collect(),
        };
        entries.sort_by(|x, y| y.0.cmp(&x.0));
        let total = entries.len();
        let mut tabs = div()
            .mt(z(24.0))
            .flex()
            .gap(z(16.0))
            .items_center()
            .child(medium(tr!("記録", "Log", "记录"), 20.0, WHITE()).mr(z(8.0)));
        for (i, label) in [
            tr!("削除", "Deleted", "已删除"),
            tr!("編集", "Edited", "已编辑"),
            tr!("ゴーストピン", "Ghost pings", "幽灵提及"),
        ]
        .into_iter()
        .enumerate()
        {
            let on = i == tab;
            tabs = tabs.child(
                div()
                    .id(SharedString::from(format!("log-tab-{i}")))
                    .cursor_pointer()
                    .px(z(8.0))
                    .py(z(2.0))
                    .rounded(z(4.0))
                    .when(on, |d| d.bg(SELECTED()))
                    .child(text(
                        format!("{label} {}", counts[i]),
                        16.0,
                        if on { WHITE() } else { MUTED() },
                    ))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.log_tab = i;
                        cx.notify();
                    })),
            );
        }
        let mut col = div().flex().flex_col().child(tabs);
        if total == 0 {
            col = col.child(div().mt(z(12.0)).child(text(
                tr!("まだ記録はありません", "Nothing logged yet", "还没有记录"),
                16.0,
                MUTED(),
            )));
        }
        for (n, (at, m, lines)) in entries.into_iter().take(SHOWN).enumerate() {
            // 開くときに使うサーバー（REST で読んだメッセージには guild_id が無いので、チャンネル一覧から探す）
            let guild = a
                .channels
                .iter()
                .find(|(_, l)| l.iter().any(|c| c.id == m.channel_id))
                .map(|(g, _)| g.clone())
                .or_else(|| m.guild_id.clone());
            let place = match &guild {
                Some(g) => {
                    let ch = a
                        .channels
                        .get(g)
                        .and_then(|l| l.iter().find(|c| c.id == m.channel_id))
                        .and_then(|c| c.name.clone())
                        .unwrap_or_else(|| {
                            tr!("不明なチャンネル", "Unknown channel", "未知频道").into()
                        });
                    let gn = a
                        .guilds
                        .iter()
                        .find(|x| &x.id == g)
                        .map(|x| x.name.clone())
                        .unwrap_or_default();
                    format!("#{ch} · {gn}")
                }
                None => a
                    .dms
                    .iter()
                    .find(|c| c.id == m.channel_id)
                    .map(|c| c.dm_label())
                    .unwrap_or_else(|| {
                        tr!("ダイレクトメッセージ", "Direct Messages", "私信").into()
                    }),
            };
            let when = crate::app::local_dt(&at)
                .map(|t| t.format("%Y/%-m/%-d %H:%M").to_string())
                .unwrap_or_default();
            let head = format!("{} · {place} · {when}", crate::app::author_name(m));
            let mut card = settings_card(None)
                .mt(z(8.0))
                .flex()
                .flex_col()
                .gap(z(4.0))
                .child(text(head, 13.0, MUTED()));
            for (line, color) in lines {
                if !line.is_empty() {
                    card = card.child(text(line, 15.0, color));
                }
            }
            let (cid, open) = (
                m.channel_id.clone(),
                guild.is_some() || a.dms.iter().any(|c| c.id == m.channel_id),
            );
            col = col.child(
                div()
                    .id(SharedString::from(format!("log-entry-{n}")))
                    .when(open, |d| d.cursor_pointer().hover(|s| s.opacity(0.85)))
                    .child(card)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if !open {
                            return;
                        }
                        this.close_settings();
                        this.pick_guild(guild.clone());
                        this.open_channel(cid.clone());
                        cx.notify();
                    })),
            );
        }
        if total > SHOWN {
            col = col.child(div().mt(z(8.0)).child(text(
                tr!(
                    "新しい {SHOWN} 件だけ表示しています（全 {total} 件）",
                    "Showing only the newest {SHOWN} ({total} total)",
                    "仅显示最新的 {SHOWN} 条（共 {total} 条）",
                ),
                13.0,
                FAINT(),
            )));
        }
        col.into_any_element()
    }

    fn theme_page(
        &mut self,
        col_w: f32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        if let Some(p) = self.app.bg_pick.as_ref().and_then(|rx| rx.try_recv().ok()) {
            self.app.bg_pick = None;
            if let Some(p) = p.first() {
                self.app.settings.background.image = Some(p.display().to_string());
                self.app.save_settings();
                self.app.apply_theme(false);
            }
        }
        let bg_text = self
            .app
            .settings
            .background
            .image
            .clone()
            .unwrap_or_default();
        sync(&self.inp.bg_image, &bg_text, window, cx);
        let dir = themes_dir();
        let files = theme::list_files(&dir);
        let mut pages = Vec::new();

        let mut s0 = div().flex().flex_col().child(notice(
            LINK(),
            tr!("テーマについて", "About Themes", "关于主题"),
            &tr!("テーマは {} に置く .toml ファイルです。色・背景画像・フォントを書けて、保存するとすぐ反映されます。\n書き方は「今の配色を書き出す」で作ったファイルか、見本の sample.toml を見てください。", "Themes are .toml files in {}. They can set colors, a background image and fonts, and changes apply as soon as you save.\nFor the format, see a file made with \"Export current colors\" or the sample.toml example.", "主题是放在 {} 中的 .toml 文件。可以设置颜色、背景图片和字体，保存后立即生效。\n写法请参考用“导出当前配色”生成的文件，或示例 sample.toml。", dir.display()),
        ));
        if let Some(e) = &self.app.theme_error {
            s0 = s0.child(div().mt(z(12.0)).child(notice(
                RED(),
                tr!(
                    "テーマを読めませんでした",
                    "Couldn't read the theme",
                    "无法读取主题"
                ),
                e,
            )));
        }
        if !self.app.theme_warnings.is_empty() {
            s0 = s0.child(div().mt(z(12.0)).child(notice(
                ORANGE(),
                tr!(
                    "テーマの一部を読み飛ばしました",
                    "Skipped part of the theme",
                    "跳过了主题的一部分"
                ),
                &self.app.theme_warnings.join("\n"),
            )));
        }
        let d2 = dir.clone();
        let d3 = dir.clone();
        s0 = s0.child(div().h(z(40.0))).child(
            section(0, tr!("テーマ", "Themes", "主题")).child(
                settings_card(None).child(
                    div()
                        .flex()
                        .flex_wrap()
                        .gap(z(10.0))
                        .child(
                            action_button(
                                "open_themes",
                                IconName::Folder,
                                tr!(
                                    "テーマフォルダを開く",
                                    "Open Themes Folder",
                                    "打开主题文件夹"
                                ),
                            )
                            .on_click(move |_, _, _| open_folder(&d2)),
                        )
                        .child(
                            action_button(
                                "reload_themes",
                                IconName::RefreshCw,
                                tr!("読み込み直す", "Reload", "重新加载"),
                            )
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.app.apply_theme(true);
                                cx.notify();
                            })),
                        )
                        .child(
                            action_button(
                                "export_theme",
                                IconName::Upload,
                                tr!(
                                    "今の配色を書き出す",
                                    "Export current colors",
                                    "导出当前配色"
                                ),
                            )
                            .tooltip(tip(tr!(
                                "themes/ に my-theme.toml を作って、それに切り替えます",
                                "Creates my-theme.toml in themes/ and switches to it",
                                "在 themes/ 中创建 my-theme.toml 并切换到它"
                            )))
                            .on_click(cx.listener(
                                move |this, _, _, cx| {
                                    let dir = &d3;
                                    let _ = std::fs::create_dir_all(dir);
                                    let mut path = dir.join("my-theme.toml");
                                    let mut n = 2;
                                    while path.exists() {
                                        path = dir.join(format!("my-theme-{n}.toml"));
                                        n += 1;
                                    }
                                    let text = theme::export(
                                        tr!("マイテーマ", "My Theme", "我的主题"),
                                        &theme::current_colors(),
                                        &this.app.settings.background,
                                    );
                                    match std::fs::write(&path, text) {
                                        Ok(()) => {
                                            this.app.settings.theme = path
                                                .file_name()
                                                .unwrap()
                                                .to_string_lossy()
                                                .into_owned();
                                            // 書き出した色はファイルに入ったので、上書きは消す
                                            this.app.settings.color_overrides.clear();
                                            this.app.save_settings();
                                            this.app.apply_theme(false);
                                        }
                                        Err(e) => {
                                            this.app.theme_error =
                                                Some(format!("{}: {e}", path.display()))
                                        }
                                    }
                                    cx.notify();
                                },
                            )),
                        ),
                ),
            ),
        );
        // テーマの一覧（2列のカード。右上のスイッチで使うテーマを選ぶ）
        let mut entries: Vec<(String, String, String, String)> = theme::BUILTIN
            .iter()
            .map(|(k, name, _)| {
                (
                    format!("builtin:{k}"),
                    name.s().to_string(),
                    "riscord".to_string(),
                    theme::builtin_description(k).to_string(),
                )
            })
            .collect();
        for (file, t) in &files {
            let (name, author, desc) = match t {
                Some(t) => (
                    t.name.clone().unwrap_or_else(|| file.clone()),
                    t.author.clone().unwrap_or_default(),
                    t.description.clone().unwrap_or_else(|| file.clone()),
                ),
                None => (
                    file.clone(),
                    String::new(),
                    tr!("読めないファイルです", "Unreadable file", "无法读取的文件").to_string(),
                ),
            };
            entries.push((file.clone(), name, author, desc));
        }
        let two = col_w >= 500.0;
        let mut grid = div().mt(z(14.0)).flex().flex_wrap().gap(z(16.0));
        for (i, (key, name, author, desc)) in entries.into_iter().enumerate() {
            let on = self.app.settings.theme == key;
            let mut t = div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .child(line(name, 17.0, WHITE()).font_weight(FontWeight::MEDIUM));
            if !author.is_empty() {
                t = t.child(text(format!("by {author}"), 13.0, MUTED()));
            }
            t = t.child(text(desc, 14.0, TEXT()).mt(z(6.0)));
            grid = grid.child(
                settings_card(on.then(BLURPLE))
                    .w(if two {
                        z((col_w - 16.0) / 2.0)
                    } else {
                        z(col_w)
                    })
                    .min_h(z(120.0))
                    .flex()
                    .items_start()
                    .gap(z(8.0))
                    .child(t)
                    .child(toggle_switch(("theme_sw", i), on).on_click(cx.listener(
                        move |this, _, _, cx| {
                            // 使っているものを切ったら既定に戻す
                            this.app.settings.theme = if on {
                                "builtin:black".into()
                            } else {
                                key.clone()
                            };
                            this.app.save_settings();
                            this.app.apply_theme(false);
                            cx.notify();
                        },
                    ))),
            );
        }
        pages.push(s0.child(grid).into_any_element());

        let area = self.app.settings.background.area.clone();
        let mut areas = div().flex().flex_wrap().gap(z(8.0)).mt(z(8.0));
        for (val, label) in [
            (None, tr!("テーマの指定", "Theme's setting", "主题设置")),
            (
                Some("chat"),
                tr!("チャット欄だけ", "Chat area only", "仅聊天区域"),
            ),
            (
                Some("window"),
                tr!("ウィンドウ全体", "Whole window", "整个窗口"),
            ),
        ] {
            let on = area.as_deref() == val;
            let v = val.map(String::from);
            areas = areas.child(
                div()
                    .id(SharedString::from(format!("area{label}")))
                    .h(z(32.0))
                    .px(z(10.0))
                    .rounded(z(6.0))
                    .flex()
                    .items_center()
                    .gap(z(8.0))
                    .cursor_pointer()
                    .when(on, |d| d.bg(SELECTED()))
                    .hover(|s| s.bg(HOVER()))
                    .child(
                        div()
                            .size(z(16.0))
                            .rounded_full()
                            .when(on, |d| d.bg(BLURPLE()).border_2().border_color(WHITE()))
                            .when(!on, |d| d.border_2().border_color(MUTED())),
                    )
                    .child(text(label, 14.0, TEXT()))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.app.settings.background.area = v.clone();
                        this.app.save_settings();
                        this.app.apply_theme(false);
                        cx.notify();
                    })),
            );
        }
        let picking = self.app.bg_pick.is_some();
        pages.push(
            section(1, tr!("背景画像", "Background Image", "背景图片"))
                .child(
                    settings_card(None)
                        .flex()
                        .flex_col()
                        .gap(z(8.0))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(z(8.0))
                                .child(
                                    div()
                                        .flex_1()
                                        .bg(INPUT())
                                        .border_1()
                                        .border_color(BORDER())
                                        .rounded(z(6.0))
                                        .px(z(6.0))
                                        .child(
                                            Input::new(&self.inp.bg_image)
                                                .appearance(false)
                                                .h(z(34.0)),
                                        ),
                                )
                                .child(
                                    small_button("pick_bg", tr!("選ぶ…", "Choose…", "选择…")).when(
                                        !picking,
                                        |b| {
                                            b.on_click(cx.listener(|this, _, _, cx| {
                                                this.app.bg_pick = Some(spawn_file_dialog(
                                                    &this.app.waker,
                                                    tr!(
                                                        "背景画像を選ぶ",
                                                        "Choose a background image",
                                                        "选择背景图片"
                                                    ),
                                                    false,
                                                    Some((
                                                        tr!("画像", "Images", "图片"),
                                                        &["png", "jpg", "jpeg", "webp", "gif"],
                                                    )),
                                                ));
                                                cx.notify();
                                            }))
                                        },
                                    ),
                                ),
                        )
                        .child(areas)
                        .child(self.slider_row(
                            "bg_opacity",
                            tr!("画像の見え具合", "Image opacity", "图片不透明度"),
                            window,
                            cx,
                        )),
                )
                .into_any_element(),
        );

        let has_overrides = !self.app.settings.color_overrides.is_empty();
        let mut s2 = section(2, tr!("色の調整", "Color Adjustments", "颜色调整")).child(row(
            tr!("色ごとの調整", "Per-color Adjustments", "逐色调整"),
            Some(tr!("ここで変えた色はテーマより優先されます。#rrggbb（透かすなら #rrggbbaa）で入力して Enter", "Colors changed here override the theme. Enter #rrggbb (or #rrggbbaa for transparency) and press Enter", "在这里修改的颜色优先于主题。输入 #rrggbb（需要透明则为 #rrggbbaa）后按 Enter").into()),
            div().when(has_overrides, |d| {
                d.child(button("clear_overrides", tr!("調整をすべて消す", "Clear All Adjustments", "清除所有调整"), false).on_click(cx.listener(|this, _, _, cx| {
                    this.app.settings.color_overrides.clear();
                    this.app.save_settings();
                    this.app.apply_theme(false);
                    cx.notify();
                })))
            }),
        ));
        let current = theme::current_colors();
        let mut grid = div().flex().flex_col().gap(z(8.0));
        for ((key, label, _), c) in theme::KEYS.iter().zip(current) {
            let input = self.color_input(key, c, window, cx);
            let overridden = self.app.settings.color_overrides.contains_key(*key);
            let k = *key;
            grid = grid.child(
                div()
                    .flex()
                    .items_center()
                    .gap(z(12.0))
                    .child(
                        div()
                            .size(z(28.0))
                            .flex_none()
                            .rounded(z(6.0))
                            .border_1()
                            .border_color(BORDER())
                            .bg(c),
                    )
                    .child(
                        div()
                            .w(z(130.0))
                            .flex_none()
                            .bg(INPUT())
                            .border_1()
                            .border_color(BORDER())
                            .rounded(z(6.0))
                            .px(z(6.0))
                            .child(Input::new(&input).appearance(false).h(z(30.0))),
                    )
                    .child(text(label.s(), 14.0, TEXT()).flex_1())
                    .when(overridden, |d| {
                        d.child(
                            small_button(
                                SharedString::from(format!("reset_{k}")),
                                tr!("戻す", "Revert", "还原"),
                            )
                            .on_click(cx.listener(
                                move |this, _, _, cx| {
                                    this.app.settings.color_overrides.remove(k);
                                    this.app.save_settings();
                                    this.app.apply_theme(false);
                                    cx.notify();
                                },
                            )),
                        )
                    }),
            );
        }
        s2 = s2.child(settings_card(None).child(grid));
        pages.push(s2.into_any_element());
        pages
    }

    /// 色ごとの入力欄（Enter か入力を終えたときに反映する）
    fn color_input(
        &mut self,
        key: &'static str,
        c: Color32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<InputState> {
        let hex = theme::format_color(c);
        if let Some(e) = self.color_inputs.get(key) {
            let e = e.clone();
            if !e.read(cx).focus_handle(cx).is_focused(window) {
                sync(&e, &hex, window, cx);
            }
            return e;
        }
        let e = cx.new(|cx| InputState::new(window, cx).default_value(hex));
        let sub = cx.subscribe_in(
            &e,
            window,
            move |this: &mut Root, state, ev: &InputEvent, _, cx| {
                if matches!(ev, InputEvent::PressEnter { .. } | InputEvent::Blur) {
                    let v = state.read(cx).value().to_string();
                    if let Some(col) = theme::parse_color(&v) {
                        this.app
                            .settings
                            .color_overrides
                            .insert(key.to_string(), theme::format_color(col));
                        this.app.save_settings();
                        this.app.apply_theme(false);
                        cx.notify();
                    }
                }
            },
        );
        self.push_sub(sub);
        self.color_inputs.insert(key, e.clone());
        e
    }

    /// 設定 → プロフィール。表示名・代名詞・アバター・自己紹介を書き換え、右にプレビューを出す（本家と同じ並び）
    fn profile_page(
        &mut self,
        col_w: f32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let Some(me) = self.app.me.clone() else {
            return Vec::new();
        };
        // アバターのファイル選択の結果を受け取り、正方形に切り抜いて縮めた PNG にする
        if let Some(paths) = self
            .app
            .avatar_pick
            .as_ref()
            .and_then(|rx| rx.try_recv().ok())
        {
            self.app.avatar_pick = None;
            if let Some(path) = paths.into_iter().next() {
                match st::make_avatar(&path) {
                    Ok(edit) => {
                        if let Some(d) = self.app.profile_draft.as_mut() {
                            d.avatar = Some(edit);
                        }
                        self.app.profile_error = None;
                    }
                    Err(e) => self.app.profile_error = Some(e),
                }
            }
        }
        let key = net::profile_key(&me.id, None);
        // 代名詞・自己紹介はプロフィールの取得が済んでから編集を始める（空で上書きしないため）
        if self.app.profile_draft.is_none() {
            match self.app.profiles.get(&key) {
                None => {
                    self.app.profiles.insert(key.clone(), ProfileState::Loading);
                    self.app.send(Cmd::LoadProfile {
                        user_id: me.id.clone(),
                        guild_id: None,
                    });
                }
                Some(ProfileState::Ready(d)) => {
                    let base = [
                        me.global_name.clone().unwrap_or_default(),
                        d.pronouns.clone().unwrap_or_default(),
                        d.bio.clone().unwrap_or_default(),
                    ];
                    self.app.profile_draft = Some(ProfileDraft {
                        fields: base.clone(),
                        base,
                        avatar: None,
                    });
                    sync(
                        &self.inp.name,
                        &self.app.profile_draft.as_ref().unwrap().fields[0],
                        window,
                        cx,
                    );
                    sync(
                        &self.inp.pronouns,
                        &self.app.profile_draft.as_ref().unwrap().fields[1],
                        window,
                        cx,
                    );
                    sync(
                        &self.inp.bio,
                        &self.app.profile_draft.as_ref().unwrap().fields[2],
                        window,
                        cx,
                    );
                }
                _ => {}
            }
        }
        let Some(draft) = self.app.profile_draft.as_ref() else {
            let mut s = section(0, tr!("プロフィール", "Profiles", "个人资料"));
            if matches!(self.app.profiles.get(&key), Some(ProfileState::Failed)) {
                s = s.child(row(
                    tr!(
                        "プロフィールを読み込めませんでした",
                        "Couldn't load the profile",
                        "无法加载个人资料"
                    ),
                    Some(
                        tr!(
                            "通信状態を確認して、もう一度読み込んでください",
                            "Check your connection and load it again",
                            "请检查网络连接后重新加载"
                        )
                        .into(),
                    ),
                    button(
                        "reload_profile",
                        tr!("再読み込み", "Reload", "重新加载"),
                        false,
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.app.profiles.remove(&key);
                        cx.notify();
                    })),
                ));
            } else {
                s = s.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(z(8.0))
                        .child(spinner(18.0))
                        .child(text(
                            tr!(
                                "プロフィールを読み込み中…",
                                "Loading profile…",
                                "正在加载个人资料…"
                            ),
                            15.0,
                            MUTED(),
                        )),
                );
            }
            return vec![s.into_any_element()];
        };
        let wide = col_w >= 600.0;
        let form_w = if wide { col_w - 300.0 - 32.0 } else { col_w };
        let has_avatar = match &draft.avatar {
            Some(AvatarEdit::Set { .. }) => true,
            Some(AvatarEdit::Remove) => false,
            None => me.avatar.is_some(),
        };
        let boxed = |inp: AnyElement| {
            div()
                .w_full()
                .bg(INPUT())
                .border_1()
                .border_color(BORDER())
                .rounded(z(8.0))
                .px(z(6.0))
                .py(z(2.0))
                .text_size(z(15.0))
                .child(inp)
        };
        let mut form = div().w(z(form_w)).flex().flex_col();
        form = form
            .child(text(tr!("表示名", "Display Name", "显示名称"), 13.0, MUTED()).mb(z(6.0)))
            .child(boxed(
                Input::new(&self.inp.name)
                    .appearance(false)
                    .h(z(36.0))
                    .into_any_element(),
            ))
            .child(
                text(tr!("代名詞", "Pronouns", "代词"), 13.0, MUTED())
                    .mt(z(20.0))
                    .mb(z(6.0)),
            )
            .child(boxed(
                Input::new(&self.inp.pronouns)
                    .appearance(false)
                    .h(z(36.0))
                    .into_any_element(),
            ))
            .child(
                text(tr!("アバター", "Avatar", "头像"), 13.0, MUTED())
                    .mt(z(20.0))
                    .mb(z(6.0)),
            )
            .child(
                div()
                    .flex()
                    .gap(z(8.0))
                    .child(
                        fill_button(
                            "change_avatar",
                            tr!("アバターを変更", "Change Avatar", "更换头像"),
                            BLURPLE(),
                            Color32::WHITE,
                            true,
                        )
                        .w(z(140.0))
                        .on_click(cx.listener(|this, _, _, cx| {
                            if this.app.avatar_pick.is_none() {
                                this.app.avatar_pick = Some(spawn_file_dialog(
                                    &this.app.waker,
                                    tr!("アバターを選ぶ", "Choose an avatar", "选择头像"),
                                    false,
                                    Some((
                                        tr!("画像", "Images", "图片"),
                                        &["png", "jpg", "jpeg", "gif", "webp"],
                                    )),
                                ));
                            }
                            cx.notify();
                        })),
                    )
                    .when(has_avatar, |d| {
                        d.child(
                            text_button(
                                "remove_avatar",
                                tr!("アバターを削除", "Remove Avatar", "移除头像"),
                            )
                            .on_click(cx.listener(|this, _, _, cx| {
                                if let Some(d) = this.app.profile_draft.as_mut() {
                                    d.avatar = Some(AvatarEdit::Remove);
                                }
                                cx.notify();
                            })),
                        )
                    }),
            )
            .child(
                text(tr!("自己紹介", "About Me", "个人简介"), 13.0, MUTED())
                    .mt(z(20.0))
                    .mb(z(6.0)),
            )
            .child(boxed(
                Textarea::new(&self.inp.bio)
                    .appearance(false)
                    .into_any_element(),
            ))
            .child(div().flex().justify_end().child(text(
                tr!(
                    "残り {}",
                    "{} left",
                    "剩余 {}",
                    st::PROFILE_BIO_MAX.saturating_sub(draft.fields[2].chars().count())
                ),
                12.0,
                FAINT(),
            )));
        let preview = div()
            .flex()
            .flex_col()
            .child(text(tr!("プレビュー", "Preview", "预览"), 13.0, MUTED()).mb(z(6.0)))
            .child(profile_preview(
                &me,
                draft,
                if wide { 300.0 } else { col_w.min(340.0) },
            ));
        let body = if wide {
            div()
                .flex()
                .items_start()
                .gap(z(32.0))
                .child(form)
                .child(div().pt(z(0.0)).child(preview))
        } else {
            div()
                .flex()
                .flex_col()
                .gap(z(24.0))
                .child(form)
                .child(preview)
        };
        // 保存バーに隠れないよう下を空けておく
        vec![section(0, tr!("プロフィール", "Profiles", "个人资料"))
            .child(body)
            .child(div().h(z(72.0)))
            .into_any_element()]
    }

    /// 保存していない変更があるときに出す、画面下の帯（本家の「変更が保存されていません」）
    fn profile_save_bar(&mut self, col_w: f32, cx: &mut Context<Self>) -> Option<AnyElement> {
        let a = &self.app;
        let dirty = a.profile_draft.as_ref().is_some_and(ProfileDraft::dirty);
        let saved = a
            .profile_saved_at
            .is_some_and(|t| t.elapsed() < std::time::Duration::from_secs(3));
        if !dirty && !saved && a.profile_error.is_none() {
            return None;
        }
        // 閉じようとしたときは赤くして気づかせる（本家と同じ）
        let nag = a
            .profile_nag
            .is_some_and(|t| t.elapsed() < std::time::Duration::from_millis(1200));
        let (msg, color) = if a.profile_saving {
            (tr!("保存中…", "Saving…", "正在保存…").to_string(), MUTED())
        } else if let Some(e) = &a.profile_error {
            (e.clone(), if nag { Color32::WHITE } else { RED() })
        } else if dirty {
            (
                tr!(
                    "ご注意ください — 保存されていない変更があります！",
                    "Careful — you have unsaved changes!",
                    "注意 — 你有未保存的更改！"
                )
                .to_string(),
                if nag { Color32::WHITE } else { TEXT() },
            )
        } else {
            (
                tr!(
                    "プロフィールを保存しました",
                    "Profile saved",
                    "已保存个人资料"
                )
                .to_string(),
                GREEN(),
            )
        };
        let mut bar = div()
            .w(z(col_w))
            .min_h(z(52.0))
            .px(z(16.0))
            .py(z(8.0))
            .rounded(z(8.0))
            .bg(if nag { RED() } else { RAIL() })
            .border_1()
            .border_color(BORDER())
            .flex()
            .items_center()
            .gap(z(8.0))
            .child(text(msg, 14.0, color).flex_1());
        if !dirty || a.profile_saving {
            self.app
                .repaint_after(std::time::Duration::from_millis(500));
        } else {
            bar = bar
                .child(
                    text_button("profile_reset", tr!("リセット", "Reset", "重置")).on_click(
                        cx.listener(|this, _, _, cx| {
                            if let Some(d) = this.app.profile_draft.as_mut() {
                                d.fields = d.base.clone();
                                d.avatar = None;
                            }
                            this.app.profile_error = None;
                            cx.notify();
                        }),
                    ),
                )
                .child(
                    fill_button(
                        "profile_save",
                        tr!("変更を保存", "Save Changes", "保存更改"),
                        GREEN(),
                        Color32::WHITE,
                        true,
                    )
                    .w(z(120.0))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.app.save_profile();
                        cx.notify();
                    })),
                );
        }
        Some(
            div()
                .absolute()
                .left_0()
                .right_0()
                .bottom(z(16.0))
                .flex()
                .justify_center()
                .child(bar)
                .into_any_element(),
        )
    }
}

/// 編集中の内容で描くプロフィールカード
fn profile_preview(me: &crate::model::User, draft: &ProfileDraft, w: f32) -> Div {
    let name = Some(draft.fields[0].trim())
        .filter(|n| !n.is_empty())
        .unwrap_or(&me.username)
        .to_string();
    let av = match &draft.avatar {
        Some(AvatarEdit::Set { tex, .. }) => {
            div().size(z(80.0)).rounded_full().overflow_hidden().child(
                img(tex.clone())
                    .size_full()
                    .rounded_full()
                    .object_fit(ObjectFit::Cover),
            )
        }
        Some(AvatarEdit::Remove) => avatar(None, 80.0, &name, 40.0, BLURPLE()),
        None => avatar(Some(&user_avatar_url(me)), 80.0, &name, 40.0, BLURPLE()),
    };
    let mut info = div()
        .px(z(16.0))
        .pt(z(48.0))
        .pb(z(16.0))
        .flex()
        .flex_col()
        .child(line(name.clone(), 20.0, WHITE()).font_weight(FontWeight::MEDIUM))
        .child(text(me.username.clone(), 14.0, TEXT()));
    if !draft.fields[1].trim().is_empty() {
        info = info.child(text(draft.fields[1].trim().to_string(), 13.0, MUTED()));
    }
    if !draft.fields[2].trim().is_empty() {
        info = info
            .child(div().my(z(10.0)).child(hline(DIVIDER())))
            .child(text(tr!("自己紹介", "About Me", "个人简介"), 12.0, MUTED()))
            .child(text(draft.fields[2].trim().to_string(), 14.0, TEXT()));
    }
    card(RAIL(), BORDER(), 8.0)
        .w(z(w))
        .flex()
        .flex_col()
        .child(
            div()
                .relative()
                .h(z(72.0))
                .rounded_t(z(8.0))
                .bg(BLURPLE())
                .child(
                    div()
                        .absolute()
                        .left(z(10.0))
                        .top(z(72.0 - 46.0))
                        .size(z(92.0))
                        .rounded_full()
                        .bg(RAIL())
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(av),
                ),
        )
        .child(info)
}
