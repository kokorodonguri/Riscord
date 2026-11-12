//! 画面のあちこちで使う小さな部品（大きさの単位・アイコン・アバター・ボタン・文字）
use std::cell::Cell;

use gpui::assets::IconName;
use gpui::prelude::*;
use gpui::*;

use crate::app::initials;
use crate::color::Color32;
use crate::images::{self, Img};
use crate::theme::*;

thread_local! {
    static ZOOM: Cell<f32> = const { Cell::new(1.0) };
}

/// UI の拡大率（設定の「UIの拡大率」）
pub fn set_zoom(v: f32) {
    ZOOM.with(|z| z.set(v));
}

pub fn zoom() -> f32 {
    ZOOM.with(|z| z.get())
}

/// 論理ピクセル（拡大率を掛けた大きさ）。画面の大きさはすべてこれで決める
pub fn z(v: f32) -> Pixels {
    px(v * zoom())
}

/// アイコン（Lucide）。色は text_color で変えられる（group_hover でも）
pub fn icon(name: IconName, size: f32, color: impl Into<Hsla>) -> Svg {
    svg()
        .path(name.path())
        .flex_none()
        .size(z(size))
        .text_color(color)
}

/// マウスを乗せたときに出す説明
pub fn tip(text: impl Into<SharedString>) -> impl Fn(&mut Window, &mut App) -> AnyView + 'static {
    let text: SharedString = text.into();
    move |window, cx| gpui::component::tooltip::Tooltip::new(text.clone()).build(window, cx)
}

/// 文字
pub fn text(s: impl Into<SharedString>, size: f32, color: Color32) -> Div {
    div().text_size(z(size)).text_color(color).child(s.into())
}

/// 少し太い文字（名前・見出し）
pub fn medium(s: impl Into<SharedString>, size: f32, color: Color32) -> Div {
    text(s, size, color).font_weight(FontWeight::MEDIUM)
}

/// 1行に収め、はみ出したら … で切る文字
pub fn line(s: impl Into<SharedString>, size: f32, color: Color32) -> Div {
    text(s, size, color).min_w_0().truncate()
}

pub fn hline(color: Color32) -> Div {
    div().h(px(1.0)).w_full().flex_none().bg(color)
}

/// 読み込み中の印
pub fn spinner(size: f32) -> impl IntoElement {
    div()
        .size(z(size))
        .flex()
        .items_center()
        .justify_center()
        .child({
            use gpui::component::Sizable as _;
            gpui::component::spinner::Spinner::new()
                .color(MUTED().into())
                .with_size(gpui::component::Size::Size(z(size)))
        })
}

/// 読み込み済みの画像（まだなら None。読み込みは始める）
pub fn loaded(uri: &str) -> Option<std::sync::Arc<RenderImage>> {
    match images::get(uri) {
        Img::Ready(i) => Some(i),
        _ => None,
    }
}

/// アバターにデコレーション（周りの枠）を重ねる。画像がまだなら何も重ねない
pub fn with_decoration(av: Div, size: f32, deco_url: Option<&str>) -> Div {
    let Some(i) = deco_url.and_then(loaded) else {
        return av;
    };
    // 枠はアバターより 2 割大きく、中心を合わせる
    let off = -size * 0.1;
    div().relative().flex_none().size(z(size)).child(av).child(
        div()
            .absolute()
            .left(z(off))
            .top(z(off))
            .size(z(size * 1.2))
            .child(img(i).size_full().object_fit(ObjectFit::Contain)),
    )
}

/// 画像が読めればそれを、無理なら頭文字入りの図形を出す
pub fn avatar(url: Option<&str>, size: f32, fallback: &str, radius: f32, bg: Color32) -> Div {
    let base = div().size(z(size)).flex_none().rounded(z(radius));
    match url.and_then(loaded) {
        Some(i) => base.child(
            img(i)
                .size_full()
                .rounded(z(radius))
                .object_fit(ObjectFit::Cover),
        ),
        None => base
            .bg(bg)
            .flex()
            .items_center()
            .justify_center()
            .text_size(z((size * 0.34).max(6.0)))
            .text_color(WHITE())
            .child(initials(fallback)),
    }
}

/// アバター右下の状態マーク（ring は背景色。くり抜いたように見せる）。中心の半径 r
pub fn status_mark(status: &str, r: f32, ring: Color32) -> Div {
    let w = (r * 0.5).max(2.0);
    let outer = r + w;
    let inner = div()
        .size(z(r * 2.0))
        .rounded_full()
        .relative()
        .overflow_hidden();
    let inner = match status {
        "online" => inner.bg(ST_ONLINE()),
        "idle" => inner.bg(ST_IDLE()).child(
            div()
                .absolute()
                .size(z(r * 1.24))
                .rounded_full()
                .bg(ring)
                .left(z(r * 0.55 - r * 0.62))
                .top(z(r * 0.55 - r * 0.62)),
        ),
        "dnd" => inner
            .bg(ST_DND())
            .flex()
            .items_center()
            .justify_center()
            .child(
                div()
                    .w(z(r * 1.2))
                    .h(z(r * 0.45))
                    .rounded(z(r * 0.2))
                    .bg(ring),
            ),
        _ => inner
            .bg(ST_OFFLINE())
            .flex()
            .items_center()
            .justify_center()
            .child(div().size(z(r * 0.9)).rounded_full().bg(ring)),
    };
    div()
        .size(z(outer * 2.0))
        .flex_none()
        .rounded_full()
        .bg(ring)
        .flex()
        .items_center()
        .justify_center()
        .child(inner)
}

/// アバターに状態マークを重ねる
pub fn with_status(av: Div, size: f32, status: &str, ring: Color32) -> Div {
    let r = (size * 0.16).max(4.0);
    let outer = r + (r * 0.5).max(2.0);
    // マークの中心がアバターの右下から r*0.8 内側に来るように置く
    let off = size - r * 0.8 - outer;
    div().relative().flex_none().size(z(size)).child(av).child(
        status_mark(status, r, ring)
            .absolute()
            .left(z(off))
            .top(z(off)),
    )
}

/// 吹き出しの顔（DM ボタン・タイトルバー用のロゴ）
pub fn logo(size: f32, color: Color32, eye: Color32) -> Div {
    let s = size;
    let body_h = s * 0.72;
    let body_top = s / 2.0 + s * 0.04 - body_h / 2.0;
    let ear = s * 0.12;
    let eye_w = s * 0.18;
    let eye_h = s * 0.22;
    let mut d = div().relative().flex_none().size(z(s));
    for x in [-1.0_f32, 1.0] {
        let cx = s / 2.0 + x * s * 0.3;
        let cy = s / 2.0 - s * 0.3;
        d = d.child(
            div()
                .absolute()
                .size(z(ear * 2.0))
                .rounded_full()
                .bg(color)
                .left(z(cx - ear))
                .top(z(cy - ear)),
        );
    }
    d = d.child(
        div()
            .absolute()
            .left_0()
            .w(z(s))
            .h(z(body_h))
            .top(z(body_top))
            .rounded(z(s * 0.3))
            .bg(color),
    );
    for x in [-1.0_f32, 1.0] {
        let cx = s / 2.0 + x * s * 0.19;
        let cy = s / 2.0 + s * 0.02;
        d = d.child(
            div()
                .absolute()
                .w(z(eye_w))
                .h(z(eye_h))
                .rounded_full()
                .bg(eye)
                .left(z(cx - eye_w / 2.0))
                .top(z(cy - eye_h / 2.0)),
        );
    }
    d
}

/// 角丸の四角い背景が出るアイコンボタン（マウスを乗せると白くなる）
pub fn icon_button(id: impl Into<ElementId>, name: IconName, size: f32) -> Stateful<Div> {
    div()
        .id(id)
        .group("ib")
        .size(z(size))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded(z(8.0))
        .cursor_pointer()
        .hover(|s| s.bg(SELECTED()))
        .child(icon(name, size * 0.62, MUTED()).group_hover("ib", |s| s.text_color(WHITE())))
}

/// 横いっぱいなどの色付きボタン（文字は中央）
pub fn fill_button(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    fill: Color32,
    fg: Color32,
    enabled: bool,
) -> Stateful<Div> {
    fill_button_part(id, label, fill, fg, enabled, None)
}

/// fill_button の、テーマの部品として色を変えられる版（hover_bg も効く）
pub fn fill_button_part(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    fill: Color32,
    fg: Color32,
    enabled: bool,
    part: Option<crate::theme::Part>,
) -> Stateful<Div> {
    let st = part.map(crate::theme::part_style).unwrap_or_default();
    // 地を変えたのに hover_bg を書いていないときは、変えた地を少し暗くする
    let hover = st
        .hover_bg
        .or(st.bg.map(|c| c.lerp_to_gamma(Color32::BLACK, 0.15)));
    let b = fill_button_base(id, label, fill, fg, enabled, hover);
    match part {
        Some(p) => b.part(p),
        None => b,
    }
}

fn fill_button_base(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    fill: Color32,
    fg: Color32,
    enabled: bool,
    hover: Option<Color32>,
) -> Stateful<Div> {
    let b = div()
        .id(id)
        .h(z(38.0))
        .px(z(16.0))
        .flex()
        .items_center()
        .justify_center()
        .rounded(z(8.0))
        .text_size(z(14.0))
        .font_weight(FontWeight::MEDIUM)
        .child(label.into());
    if enabled {
        b.bg(fill)
            .text_color(fg)
            .cursor_pointer()
            .hover(move |s| s.bg(hover.unwrap_or(fill.lerp_to_gamma(Color32::BLACK, 0.15))))
    } else {
        b.bg(fill.gamma_multiply(0.5))
            .text_color(Color32::from_white_alpha(140))
    }
}

/// 枠のない文字だけのボタン
pub fn text_button(id: impl Into<ElementId>, label: impl Into<SharedString>) -> Stateful<Div> {
    div()
        .id(id)
        .h(z(38.0))
        .px(z(16.0))
        .flex()
        .items_center()
        .justify_center()
        .rounded(z(8.0))
        .text_size(z(14.0))
        .text_color(TEXT())
        .cursor_pointer()
        .hover(|s| s.text_color(WHITE()).bg(HOVER()))
        .child(label.into())
}

/// 小さな灰色のボタン（「再読み込み」など）
pub fn small_button(id: impl Into<ElementId>, label: impl Into<SharedString>) -> Stateful<Div> {
    div()
        .id(id)
        .h(z(30.0))
        .px(z(12.0))
        .flex()
        .items_center()
        .justify_center()
        .rounded(z(6.0))
        .bg(WIDGET())
        .text_size(z(14.0))
        .text_color(TEXT())
        .cursor_pointer()
        .hover(|s| s.bg(WIDGET_HOVER()).text_color(WHITE()))
        .child(label.into())
}

/// 青い文字のリンク
pub fn link_text(id: impl Into<ElementId>, label: impl Into<SharedString>) -> Stateful<Div> {
    div()
        .id(id)
        .text_size(z(14.0))
        .text_color(LINK())
        .cursor_pointer()
        .hover(|s| s.underline())
        .child(label.into())
}

/// オン/オフのスイッチ（オンはアクセント色に ✓、オフは灰色に ×）
pub fn toggle_switch(id: impl Into<ElementId>, on: bool) -> Stateful<Div> {
    let bg = if on {
        BLURPLE()
    } else {
        Color32::from_gray(0x70)
    };
    let knob = div()
        .size(z(24.0))
        .rounded_full()
        .bg(Color32::WHITE)
        .flex()
        .items_center()
        .justify_center()
        .child(icon(
            if on { IconName::Check } else { IconName::Close },
            14.0,
            bg,
        ));
    div()
        .id(id)
        .w(z(56.0))
        .h(z(32.0))
        .flex_none()
        .rounded(z(16.0))
        .bg(bg)
        .px(z(4.0))
        .flex()
        .items_center()
        .when(on, |d| d.justify_end())
        .cursor_pointer()
        .child(knob)
}

/// 枠付きの入れ物
/// テーマの [parts] での上書きを当てる。元の色や形を決めたあと、最後に呼ぶ
pub trait Themed: Styled + Sized {
    fn part(mut self, p: crate::theme::Part) -> Self {
        let s = crate::theme::part_style(p);
        if let Some(c) = s.bg {
            self = self.bg(c);
        }
        if let Some(c) = s.text {
            self = self.text_color(c);
        }
        if let Some(c) = s.border {
            self = self.border_color(c);
        }
        if let Some(w) = s.border_width {
            let w: AbsoluteLength = z(w).into();
            let b = &mut self.style().border_widths;
            (b.top, b.right, b.bottom, b.left) = (Some(w), Some(w), Some(w), Some(w));
        }
        if let Some(r) = s.radius {
            self = self.rounded(z(r));
        }
        if let Some(o) = s.opacity {
            self = self.opacity(o);
        }
        self
    }
}

impl<T: Styled> Themed for T {}

/// マウスを乗せたときの地の色（テーマの hover_bg があればそれ、なければ default）
pub fn part_hover(p: crate::theme::Part, default: Color32) -> Color32 {
    crate::theme::part_style(p).hover_bg.unwrap_or(default)
}

/// 文字の色（テーマの text があればそれ、なければ default）。状態で色を選ぶ所で使う
pub fn part_text(p: crate::theme::Part, default: Color32) -> Color32 {
    crate::theme::part_style(p).text.unwrap_or(default)
}

pub fn card(fill: Color32, border: Color32, radius: f32) -> Div {
    div()
        .bg(fill)
        .border_1()
        .border_color(border)
        .rounded(z(radius))
}

/// 外側を暗くする幕と、真ん中の箱（モーダル）。幕を押したら on_backdrop
pub fn modal(
    id: &'static str,
    content: impl IntoElement,
    on_backdrop: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
    alpha: u8,
) -> impl IntoElement {
    div()
        .id(id)
        .absolute()
        .inset_0()
        .occlude()
        .bg(Color32::from_black_alpha(alpha))
        .flex()
        .items_center()
        .justify_center()
        .on_mouse_down(MouseButton::Left, on_backdrop)
        .child(
            div()
                // 箱の中を押しても幕の「閉じる」が動かないようにする
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .child(content),
        )
}

/// ポップアップの影
pub fn popup_shadow() -> Vec<BoxShadow> {
    vec![BoxShadow {
        color: Color32::from_black_alpha(140).into(),
        offset: point(px(0.), px(8.)),
        blur_radius: px(24.),
        spread_radius: px(0.),
        inset: false,
    }]
}

/// コードを出す等幅フォントのファミリー名
pub fn mono_family() -> SharedString {
    static FAMILY: std::sync::OnceLock<SharedString> = std::sync::OnceLock::new();
    FAMILY
        .get_or_init(|| {
            #[cfg(all(unix, not(target_os = "macos")))]
            {
                let found = std::process::Command::new("fc-match")
                    .args(["-f", "%{family[0]}", "monospace"])
                    .output()
                    .ok()
                    .and_then(|o| String::from_utf8(o.stdout).ok());
                found
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .unwrap_or_else(|| "DejaVu Sans Mono".into())
                    .into()
            }
            #[cfg(windows)]
            {
                "Consolas".into()
            }
            #[cfg(target_os = "macos")]
            {
                "Menlo".into()
            }
        })
        .clone()
}
