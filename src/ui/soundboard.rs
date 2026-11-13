//! サウンドボードの一覧。押すと同じボイスチャンネルの全員に音が鳴る。
use gpui::assets::IconName;
use gpui::prelude::*;
use gpui::*;

use super::*;
use crate::app::Sound;

impl Root {
    pub fn soundboard_dialog(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let sb = self.app.soundboard.as_ref()?;
        let width = (self.app.screen_w - 32.0).clamp(0.0, 520.0);
        let height = (self.pane_h - 160.0).clamp(100.0, 480.0);
        let blocked = self.app.soundboard_block_reason();
        let sending = sb.sending.as_ref().map(|(_, s)| s.id.clone());
        let mut content = card(SIDEBAR(), BORDER(), 8.0)
            .w(z(width))
            .p(z(16.0))
            .flex()
            .flex_col()
            .gap(z(8.0))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(z(6.0))
                    .child(
                        medium(tr!("サウンドボード", "Soundboard", "音效板"), 20.0, WHITE())
                            .flex_1(),
                    )
                    .child(icon_button("soundboard_close", IconName::X, 28.0).on_click(
                        cx.listener(|this, _, _, cx| {
                            this.app.soundboard = None;
                            cx.notify();
                        }),
                    )),
            );
        if let Some(reason) = blocked {
            content = content.child(text(reason, 13.0, MUTED()));
        }
        if let Some(e) = &sb.error {
            content = content.child(text(e.clone(), 13.0, RED()));
        }
        let mut body = div()
            .id("soundboard-scroll")
            .max_h(z(height))
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .gap(z(10.0));
        let groups: [(&str, &Option<Vec<Sound>>, bool); 2] = [
            (
                tr!("このサーバー", "This server", "此服务器"),
                &sb.guild,
                sb.guild_request.is_some(),
            ),
            (
                tr!("標準", "Default", "默认"),
                &sb.defaults,
                sb.default_request.is_some(),
            ),
        ];
        for (title, list, loading) in groups {
            let mut grid = div().flex().flex_wrap().gap(z(6.0));
            match list {
                None if loading => grid = grid.child(spinner(16.0)),
                None => {}
                Some(l) if l.is_empty() => {
                    grid = grid.child(text(
                        tr!("音がありません", "No sounds", "没有音效"),
                        13.0,
                        FAINT(),
                    ));
                }
                Some(l) => {
                    for s in l {
                        let id = s.id.clone();
                        let label = match &s.emoji {
                            Some(e) => format!("{e} {}", s.name),
                            None => s.name.clone(),
                        };
                        let busy = sending.as_deref() == Some(s.id.as_str());
                        grid = grid.child(
                            fill_button(
                                SharedString::from(format!("sound_{id}")),
                                if busy {
                                    tr!("送信中…", "Sending…", "正在发送…").to_string()
                                } else {
                                    label
                                },
                                WIDGET(),
                                WHITE(),
                                blocked.is_none() && sending.is_none(),
                            )
                            .on_click(cx.listener(
                                move |this, _, _, cx| {
                                    this.app.send_sound(&id);
                                    cx.notify();
                                },
                            )),
                        );
                    }
                }
            }
            body = body.child(
                div()
                    .flex()
                    .flex_col()
                    .gap(z(4.0))
                    .child(text(title, 13.0, MUTED()))
                    .child(grid),
            );
        }
        content = content.child(body);
        let close = cx.listener(|this, _: &MouseDownEvent, _, cx| {
            this.app.soundboard = None;
            cx.notify();
        });
        Some(modal("soundboard-modal", content, close, 170).into_any_element())
    }
}
