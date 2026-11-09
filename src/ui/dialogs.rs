//! ダイアログ（Ctrl+K のクイックスイッチャー・削除の確認・着信）
use gpui::assets::IconName;
use gpui::component::input::Input;
use gpui::prelude::*;
use gpui::*;

use super::*;
use crate::app::{author_name, dm_icon_url};
use crate::color::Color32;
use crate::net::Cmd;

pub struct Hit {
    prefix: &'static str,
    label: String,
    sub: String,
    target: (Option<String>, Option<String>),
}

impl Root {
    /// スイッチャーの候補（最大10件、よく合うものから）
    fn switcher_hits(&self) -> Vec<Hit> {
        let a = &self.app;
        let Some((query, _)) = &a.switcher else { return Vec::new() };
        let q = query.trim().to_lowercase();
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
        let mut hits: Vec<(u8, Hit)> = Vec::new();
        for c in &a.dms {
            let label = c.dm_label();
            if let Some(s) = score(&label) {
                hits.push((s, Hit { prefix: "@", label, sub: String::new(), target: (None, Some(c.id.clone())) }));
            }
        }
        if !q.is_empty() {
            for g in &a.guilds {
                if let Some(s) = score(&g.name) {
                    hits.push((s, Hit { prefix: "", label: g.name.clone(), sub: "サーバー".into(), target: (Some(g.id.clone()), None) }));
                }
                for c in a.channels.get(&g.id).into_iter().flatten().filter(|c| c.is_text()) {
                    let name = c.name.clone().unwrap_or_default();
                    if let Some(s) = score(&name) {
                        hits.push((s, Hit { prefix: "#", label: name, sub: g.name.clone(), target: (Some(g.id.clone()), Some(c.id.clone())) }));
                    }
                }
            }
        }
        hits.sort_by_key(|(s, _)| *s);
        hits.truncate(10);
        hits.into_iter().map(|(_, h)| h).collect()
    }

    fn switcher_go(&mut self, target: (Option<String>, Option<String>)) {
        self.app.switcher = None;
        let before = (self.app.sel_guild.clone(), self.app.sel_channel.clone());
        self.app.select(target.0, target.1);
        self.after_move(before);
        self.app.focus_input = true;
    }

    /// スイッチャーのキー操作。使ったら true
    pub fn switcher_key(&mut self, key: &str) -> bool {
        let hits = self.switcher_hits();
        let n = hits.len();
        let Some(sw) = self.app.switcher.as_mut() else { return false };
        match key {
            "down" if n > 0 => sw.1 = (sw.1 + 1) % n,
            "up" if n > 0 => sw.1 = (sw.1 + n - 1) % n,
            "enter" => {
                if let Some(h) = hits.into_iter().nth(sw.1.min(n.saturating_sub(1))) {
                    self.switcher_go(h.target);
                }
            }
            "escape" => self.app.switcher = None,
            _ => return false,
        }
        true
    }

    pub fn switcher_dialog(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let (query, sel) = self.app.switcher.clone()?;
        let hits = self.switcher_hits();
        let sel = sel.min(hits.len().saturating_sub(1));
        let w = (self.app.screen_w - 48.0).clamp(0.0, 540.0);
        let mut rows = div().flex().flex_col();
        if hits.is_empty() {
            rows = rows.child(text("見つかりませんでした", 14.0, MUTED()).mt(z(8.0)));
        }
        for (i, h) in hits.into_iter().enumerate() {
            let on = i == sel;
            let target = h.target.clone();
            rows = rows.child(
                div()
                    .id(("hit", i))
                    .h(z(36.0))
                    .px(z(12.0))
                    .rounded(z(6.0))
                    .flex()
                    .items_center()
                    .gap(z(8.0))
                    .cursor_pointer()
                    .when(on, |d| d.bg(SELECTED()))
                    .hover(|s| s.bg(if on { SELECTED() } else { HOVER() }))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.switcher_go(target.clone());
                        cx.notify();
                    }))
                    .child(div().w(z(14.0)).text_size(z(18.0)).text_color(FAINT()).child(h.prefix))
                    .child(line(h.label, 15.0, if on { WHITE() } else { TEXT() }).font_weight(FontWeight::MEDIUM).flex_1())
                    .child(line(h.sub, 12.0, FAINT()).max_w(z(180.0))),
            );
        }
        let content = card(SIDEBAR(), BORDER(), 8.0)
            .p(z(16.0))
            .w(z(w))
            .flex()
            .flex_col()
            .child(medium("どこに行きますか？", 18.0, WHITE()))
            .child(div().mt(z(10.0)).bg(BG()).border_1().border_color(DIVIDER()).rounded(z(8.0)).px(z(8.0)).text_size(z(18.0)).child(Input::new(&self.inp.switcher).appearance(false).h(z(46.0))))
            .child(text(if query.trim().is_empty() { "最近の会話" } else { "検索結果" }, 12.0, FAINT()).mt(z(10.0)).mb(z(4.0)))
            .child(rows)
            .child(text("↑↓ で選択 · Enter で移動 · Esc で閉じる", 11.0, FAINT()).mt(z(10.0)));
        let close = cx.listener(|this, _: &MouseDownEvent, _, cx| {
            this.app.switcher = None;
            cx.notify();
        });
        Some(modal("switcher", content, close, 170).into_any_element())
    }

    /// 削除の確認
    pub fn delete_dialog(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let m = self.app.pending_delete.clone()?;
        let w = (self.app.screen_w - 56.0).clamp(0.0, 440.0);
        let preview_h = 180.0;
        let content = card(SIDEBAR(), BORDER(), 8.0)
            .p(z(20.0))
            .w(z(w))
            .flex()
            .flex_col()
            .child(medium("メッセージを削除", 20.0, WHITE()))
            .child(text("このメッセージを削除しますか？", 15.0, TEXT()).mt(z(8.0)))
            .child(
                card(BG(), DIVIDER(), 8.0)
                    .mt(z(12.0))
                    .p(z(12.0))
                    .flex()
                    .flex_col()
                    .child(line(author_name(&m).to_string(), 15.0, WHITE()).font_weight(FontWeight::MEDIUM))
                    .child(div().id("delete_preview").max_h(z(preview_h)).overflow_y_scroll().child(text(m.content.clone(), 14.0, TEXT()))),
            )
            .child(text("ヒント: Shift を押しながら削除すると、この確認を省けます", 12.0, FAINT()).mt(z(10.0)))
            .child(
                div()
                    .mt(z(16.0))
                    .flex()
                    .justify_end()
                    .gap(z(8.0))
                    .child(text_button("delete_cancel", "キャンセル").min_w(z(96.0)).on_click(cx.listener(|this, _, _, cx| {
                        this.app.pending_delete = None;
                        cx.notify();
                    })))
                    .child(fill_button("delete_ok", "削除", RED(), Color32::WHITE, true).min_w(z(96.0)).on_click(cx.listener(|this, _, _, cx| {
                        if let Some(m) = this.app.pending_delete.take() {
                            this.app.send(Cmd::Delete { channel_id: m.channel_id, id: m.id });
                        }
                        cx.notify();
                    }))),
            );
        let close = cx.listener(|this, _: &MouseDownEvent, _, cx| {
            this.app.pending_delete = None;
            cx.notify();
        });
        Some(modal("delete_modal", content, close, 170).into_any_element())
    }

    /// サーバーから脱退する前の確認
    pub fn leave_dialog(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let gid = self.app.confirm_leave.clone()?;
        let name = self.app.guilds.iter().find(|g| g.id == gid).map(|g| g.name.clone()).unwrap_or_default();
        let w = (self.app.screen_w - 56.0).clamp(0.0, 440.0);
        let content = card(SIDEBAR(), BORDER(), 8.0)
            .p(z(20.0))
            .w(z(w))
            .flex()
            .flex_col()
            .child(medium(format!("「{name}」から脱退"), 20.0, WHITE()))
            .child(text("本当に脱退しますか？ 招待されない限り、再び参加することはできません。", 15.0, TEXT()).mt(z(8.0)))
            .child(
                div()
                    .mt(z(20.0))
                    .flex()
                    .justify_end()
                    .gap(z(8.0))
                    .child(text_button("leave_cancel", "キャンセル").min_w(z(96.0)).on_click(cx.listener(|this, _, _, cx| {
                        this.app.confirm_leave = None;
                        cx.notify();
                    })))
                    .child(fill_button("leave_ok", "サーバーから脱退", RED(), Color32::WHITE, true).min_w(z(96.0)).on_click(cx.listener(|this, _, _, cx| {
                        if let Some(gid) = this.app.confirm_leave.take() {
                            this.app.api(crate::net::ApiCall::LeaveGuild(gid));
                        }
                        cx.notify();
                    }))),
            );
        let close = cx.listener(|this, _: &MouseDownEvent, _, cx| {
            this.app.confirm_leave = None;
            cx.notify();
        });
        Some(modal("leave_modal", content, close, 170).into_any_element())
    }

    /// 着信（DM 通話で呼ばれている）
    pub fn incoming_call(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let a = &self.app;
        let me = a.me.as_ref()?;
        let here = a.voice_channel().map(|(_, c, _)| c.clone());
        let (cid, _) = a.calls.iter().find(|(cid, r)| r.contains(&me.id) && here.as_deref() != Some(cid.as_str()))?;
        let cid = cid.clone();
        let ch = a.dms.iter().find(|c| c.id == cid);
        let name = ch.map(|c| c.dm_label()).unwrap_or_else(|| "不明".into());
        let url = ch.and_then(dm_icon_url);
        let (c1, c2) = (cid.clone(), cid.clone());
        let ring = div().absolute().inset(z(-6.0)).rounded_full().border_2().border_color(GREEN().gamma_multiply(0.6)).with_animation(
            "incoming_pulse",
            Animation::new(std::time::Duration::from_millis(900)).repeat(),
            |d, t| d.opacity(1.0 - t),
        );
        let card = div()
            .w(z(260.0))
            .bg(RAIL())
            .rounded(z(12.0))
            .p(z(20.0))
            .shadow(popup_shadow())
            .flex()
            .flex_col()
            .items_center()
            .child(div().relative().size(z(80.0)).child(ring).child(avatar(url.as_deref(), 80.0, &name, 40.0, BLURPLE())))
            .child(medium(name.clone(), 18.0, WHITE()).mt(z(12.0)))
            .child(text("着信中…", 13.0, MUTED()))
            .child(
                div()
                    .mt(z(16.0))
                    .flex()
                    .gap(z(40.0))
                    .child(round("call_decline", IconName::PhoneOff, RED()).tooltip(tip("拒否")).on_click(cx.listener(move |this, _, _, cx| {
                        let a = &mut this.app;
                        if let (Some(r), Some(me)) = (a.calls.get_mut(&c1), &a.me) {
                            r.retain(|u| u != &me.id);
                        }
                        a.send(Cmd::StopRinging(c1.clone()));
                        cx.notify();
                    })))
                    .child(round("call_answer", IconName::Phone, GREEN()).tooltip(tip("応答")).on_click(cx.listener(move |this, _, _, cx| {
                        this.app.join_voice(None, c2.clone(), false);
                        cx.notify();
                    }))),
            );
        Some(div().absolute().top(z(72.0)).left_0().right_0().flex().justify_center().child(card).into_any_element())
    }
}

fn round(id: &'static str, name: IconName, bg: Color32) -> Stateful<Div> {
    div()
        .id(id)
        .size(z(56.0))
        .rounded_full()
        .bg(bg)
        .hover(move |s| s.bg(bg.lerp_to_gamma(Color32::WHITE, 0.12)))
        .flex()
        .items_center()
        .justify_center()
        .cursor_pointer()
        .child(icon(name, 24.0, Color32::WHITE))
}
