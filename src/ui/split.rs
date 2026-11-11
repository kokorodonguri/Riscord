//! 分割表示（Firefox の Split View のように、1つのウィンドウに2つのアカウントを左右に並べる）。
//! 区画（Root）はそれぞれ自分のプロファイルのトークン・設定・控えを持ち、別々に Discord へつなぐ
use gpui::prelude::*;
use gpui::*;
use std::cell::Cell;
use std::rc::Rc;

use super::{icon, opaque, tip, z, PaneEvent, Root};
use crate::profile;
use crate::theme::*;

/// 区画の間の線の太さ（論理ピクセル）
pub const DIVIDER: f32 = 1.0;
/// 分割表示のときに区画の上に出す帯の高さ
pub(super) const HEADER_H: f32 = 28.0;

fn vertical_split(width: f32, zoom: f32) -> bool {
    (width - DIVIDER).max(0.0) / 2.0 < 700.0 * zoom
}

pub struct Shell {
    /// 左（ウィンドウを開いたときのプロファイル）と、分割していれば右
    panes: Vec<Entity<Root>>,
    _subs: Vec<Subscription>,
    vertical: Rc<Cell<bool>>,
}

impl Shell {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let left_profile = profile::name().map(str::to_string);
        let vertical = Rc::new(Cell::new(false));
        let left = cx.new(|cx| Root::new(window, cx, left_profile.clone(), true, None, vertical.clone()));
        let sub = cx.subscribe_in(&left, window, Self::on_pane_event);
        let mut shell = Shell { panes: vec![left], _subs: vec![sub], vertical };
        // 前回右に開いていたアカウントを開き直す
        // デモで RISCORD_DEMO_SPLIT を付けたら、最初から分割しておく（画面確認用）
        let demo_split = crate::demo::enabled() && std::env::var_os("RISCORD_DEMO_SPLIT").is_some();
        if demo_split {
            shell.open(Some("2".into()), false, window, cx);
        } else if let Some(right) = profile::split_load(left_profile.as_deref()) {
            shell.open(right, false, window, cx);
        }
        // ウィンドウを閉じたら終わる（通信・音声のスレッドも止まる）。
        // 次の起動ですぐ出せるよう、閉じる前にどの区画も控えを書いておく
        let this = cx.weak_entity();
        window.on_window_should_close(cx, move |_, cx| {
            let _ = this.update(cx, |this, cx| {
                for p in &this.panes {
                    p.update(cx, |r, _| {
                        r.app.save_snapshot(true);
                        r.app.message_log.flush();
                    });
                }
            });
            cx.quit();
            true
        });
        shell
    }

    fn left_profile(&self, cx: &App) -> Option<String> {
        self.panes[0].read(cx).app.profile.clone()
    }

    fn on_pane_event(&mut self, _: &Entity<Root>, ev: &PaneEvent, window: &mut Window, cx: &mut Context<Self>) {
        match ev {
            PaneEvent::OpenSplit(p) => self.open(p.clone(), true, window, cx),
        }
    }

    /// そのプロファイルを右に開く。もう右に別のアカウントがあれば閉じてから開く
    fn open(&mut self, right: Option<String>, explicit: bool, window: &mut Window, cx: &mut Context<Self>) {
        let left = self.left_profile(cx);
        if right == left || self.panes.get(1).is_some_and(|p| p.read(cx).app.profile == right) {
            return;
        }
        self.close_right(false, cx);
        let Some(lock) = profile::lock(right.as_deref()) else {
            if explicit {
                self.panes[0].update(cx, |r, cx| {
                    r.app.show_toast(&format!("{} はもう別のウィンドウで開いています", profile::label(right.as_deref())));
                    cx.notify();
                });
            } else {
                // 起動時に開けなかったら（別のウィンドウで開いているなど）、今回は分割しない
                profile::split_save(left.as_deref(), None);
            }
            return;
        };
        let pane = cx.new(|cx| Root::new(window, cx, right.clone(), false, Some(lock), self.vertical.clone()));
        self._subs.push(cx.subscribe_in(&pane, window, Self::on_pane_event));
        self.panes.push(pane);
        self.sync_others(cx);
        profile::split_save(left.as_deref(), Some(right.as_deref()));
        // 開いた区画を操作できるように
        let focus = self.panes[1].read(cx).focus.clone();
        window.focus(&focus, cx);
        cx.notify();
    }

    /// 右の区画を閉じる（ログインしたままにしておくので、次に開けばすぐ戻る）
    fn close_right(&mut self, remember: bool, cx: &mut Context<Self>) {
        let Some(pane) = self.panes.pop() else { return };
        if self.panes.is_empty() {
            self.panes.push(pane);
            return;
        }
        pane.update(cx, |r, _| {
            r.app.save_snapshot(true);
            r.app.message_log.flush();
            // 通話中なら抜ける（区画を閉じると音声のスレッドも止まる）
            r.app.send(crate::net::Cmd::LeaveVoice);
        });
        drop(pane);
        self.sync_others(cx);
        if remember {
            let left = self.left_profile(cx);
            profile::split_save(left.as_deref(), None);
        }
        cx.notify();
    }

    /// 区画それぞれに、分割しているか・もう片方は誰かを教える
    fn sync_others(&mut self, cx: &mut Context<Self>) {
        let profiles: Vec<Option<String>> = self.panes.iter().map(|p| p.read(cx).app.profile.clone()).collect();
        for (i, p) in self.panes.iter().enumerate() {
            let other = (profiles.len() > 1).then(|| profiles[1 - i].clone());
            p.update(cx, |r, cx| {
                r.other = other;
                // 色・拡大率はプロセスで1つなので、分割を閉じたら残った区画が自分のものに戻す
                // （戻さないと右の区画のテーマのまま残り、同じテーマを選び直しても「適用済み」で変わらない）
                r.app.apply_theme(true);
                super::set_zoom(r.app.settings.zoom);
                // 幅が変わるので、行の高さを測り直してもらう（gpui-component の部品の色もここで塗り直す）
                r.app.theme_dirty = true;
                cx.notify();
            });
        }
    }

    /// 区画の上の帯（アカウント名。右の区画には閉じるボタン）
    fn header(&self, i: usize, window: &Window, cx: &mut Context<Self>) -> impl IntoElement {
        let pane = self.panes[i].read(cx);
        let name = pane
            .app
            .me
            .as_ref()
            .map(|u| u.display().to_string())
            .or_else(|| profile::account_name(pane.app.profile.as_deref()))
            .unwrap_or_else(|| "未ログイン".into());
        let label = format!("{name} · {}", profile::label(pane.app.profile.as_deref()));
        let active = pane.focus.contains_focused(window, cx);
        let focus = pane.focus.clone();
        div()
            .id(("split-header", i))
            .flex_none()
            .h(z(HEADER_H))
            .px(z(10.0))
            .flex()
            .items_center()
            .gap(z(8.0))
            .bg(opaque(if active { SELECTED() } else { SIDEBAR() }))
            // 操作中の区画は下線を青くして分かるようにする
            .border_b_2()
            .border_color(if active { BLURPLE() } else { BORDER() })
            .text_size(z(12.0))
            .text_color(if active { TEXT() } else { MUTED() })
            .on_click(cx.listener(move |_, _, window, cx| window.focus(&focus, cx)))
            .child(div().flex_1().min_w_0().overflow_hidden().whitespace_nowrap().text_ellipsis().child(label))
            .when(i == 1, |d| {
                d.child(
                    div()
                        .id("split-close")
                        .flex_none()
                        .p(z(2.0))
                        .rounded(z(4.0))
                        .cursor_pointer()
                        .hover(|s| s.bg(HOVER()))
                        .tooltip(tip("分割を閉じる（ログインしたまま）"))
                        .on_click(cx.listener(|this, _, window, cx| {
                            cx.stop_propagation();
                            this.close_right(true, cx);
                            let focus = this.panes[0].read(cx).focus.clone();
                            window.focus(&focus, cx);
                        }))
                        .child(icon(gpui::assets::IconName::X, 14.0, MUTED())),
                )
            })
    }
}

impl Render for Shell {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let split = self.panes.len() > 1;
        let n = self.panes.len() as f32;
        let vp = window.viewport_size();
        let vertical = split && vertical_split(f32::from(vp.width), self.panes[0].read(cx).app.settings.zoom);
        self.vertical.set(vertical);
        // 区画の幅は半分ずつに決め打ちする（中身の最小幅で片方が押し出されないように）
        let pane_w = (vp.width - px(DIVIDER) * (n - 1.0)) / n;
        let pane_h = (vp.height - px(DIVIDER) * (n - 1.0)) / n;
        let mut row = div().id("shell").size_full().flex().when(vertical, |d| d.flex_col()).when(!vertical, |d| d.flex_row()).overflow_hidden().bg(opaque(BG()));
        for i in 0..self.panes.len() {
            if i > 0 {
                row = row.child(div().flex_none().when(vertical, |d| d.h(px(DIVIDER)).w_full()).when(!vertical, |d| d.w(px(DIVIDER)).h_full()).bg(BORDER()));
            }
            let body = div().flex_1().min_h_0().w_full().relative().overflow_hidden().child(self.panes[i].clone());
            let col = div().flex_none().when(vertical, |d| d.w_full().h(pane_h)).when(!vertical, |d| d.w(pane_w).h_full()).flex().flex_col().overflow_hidden();
            let col = if split { col.child(self.header(i, window, cx)) } else { col };
            row = row.child(col.child(body));
        }
        row
    }
}

#[cfg(test)]
mod tests {
    use super::vertical_split;

    #[test]
    fn split_direction_respects_width_and_zoom() {
        assert!(vertical_split(1400.0, 1.0));
        assert!(!vertical_split(1401.0, 1.0));
        assert!(vertical_split(1401.0, 1.25));
        assert!(!vertical_split(1751.0, 1.25));
    }
}
