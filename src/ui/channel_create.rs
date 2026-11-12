//! チャンネル・カテゴリを作る画面。
use gpui::assets::IconName;
use gpui::component::input::Input;
use gpui::prelude::*;
use gpui::*;

use super::*;

impl Root {
    pub fn channel_create_dialog(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let d = self.app.channel_draft.clone()?;
        let busy = d.pending.is_some();
        let category = d.kind == 4;
        let width = (self.app.screen_w - 32.0).clamp(0.0, 460.0);
        let mut content = card(SIDEBAR(), BORDER(), 8.0)
            .w(z(width))
            .p(z(20.0))
            .flex()
            .flex_col()
            .gap(z(10.0))
            .child(
                div()
                    .flex()
                    .items_center()
                    .child(
                        medium(
                            if category {
                                "カテゴリを作成"
                            } else {
                                "チャンネルを作成"
                            },
                            20.0,
                            WHITE(),
                        )
                        .flex_1(),
                    )
                    .child(
                        icon_button("channel_create_close", IconName::X, 28.0).on_click(
                            cx.listener(|this, _, _, cx| {
                                if this
                                    .app
                                    .channel_draft
                                    .as_ref()
                                    .is_some_and(|d| d.pending.is_none())
                                {
                                    this.app.channel_draft = None;
                                }
                                cx.notify();
                            }),
                        ),
                    ),
            );
        if !category {
            let mut kinds = div().flex().flex_wrap().gap(z(6.0));
            for (kind, label) in [(0u8, "# テキスト"), (2, "🔊 ボイス"), (15, "💬 フォーラム")]
            {
                kinds = kinds.child(
                    fill_button(
                        SharedString::from(format!("channel_kind_{kind}")),
                        label,
                        if d.kind == kind { BLURPLE() } else { WIDGET() },
                        WHITE(),
                        !busy,
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if let Some(d) = this
                            .app
                            .channel_draft
                            .as_mut()
                            .filter(|d| d.pending.is_none())
                        {
                            d.kind = kind;
                        }
                        cx.notify();
                    })),
                );
            }
            content = content
                .child(text("チャンネルの種類", 13.0, MUTED()))
                .child(kinds);
        }
        content = content
            .child(text(
                if category {
                    "カテゴリ名"
                } else {
                    "チャンネル名"
                },
                13.0,
                MUTED(),
            ))
            .child(
                div().bg(INPUT()).rounded(z(6.0)).px(z(8.0)).child(
                    Input::new(&self.inp.channel_name)
                        .appearance(false)
                        .disabled(busy)
                        .h(z(34.0)),
                ),
            );
        let cats = self.app.guild_categories(&d.guild_id);
        if !category && !cats.is_empty() {
            let mut picks = div().flex().flex_wrap().gap(z(6.0));
            for (id, name) in std::iter::once((None, "カテゴリなし".to_string()))
                .chain(cats.into_iter().map(|(id, n)| (Some(id), n.to_uppercase())))
            {
                let selected = d.parent_id == id;
                picks = picks.child(
                    fill_button(
                        SharedString::from(format!(
                            "channel_parent_{}",
                            id.as_deref().unwrap_or("none")
                        )),
                        name,
                        if selected { BLURPLE() } else { WIDGET() },
                        WHITE(),
                        !busy,
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if let Some(d) = this
                            .app
                            .channel_draft
                            .as_mut()
                            .filter(|d| d.pending.is_none())
                        {
                            d.parent_id = id.clone();
                        }
                        cx.notify();
                    })),
                );
            }
            content = content.child(text("カテゴリ", 13.0, MUTED())).child(
                div()
                    .id("channel-parents")
                    .max_h(z(140.0))
                    .overflow_y_scroll()
                    .child(picks),
            );
        }
        if let Some(e) = &d.error {
            content = content.child(text(e.clone(), 13.0, RED()));
        }
        content = content.child(
            div().flex().justify_end().gap(z(8.0)).child(
                fill_button(
                    "channel_create_submit",
                    if busy { "作成中…" } else { "作成" },
                    BLURPLE(),
                    WHITE(),
                    !busy && !d.name.trim().is_empty(),
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    this.app.submit_channel_create();
                    cx.notify();
                })),
            ),
        );
        let close = cx.listener(|this, _: &MouseDownEvent, _, cx| {
            if this
                .app
                .channel_draft
                .as_ref()
                .is_some_and(|d| d.pending.is_none())
            {
                this.app.channel_draft = None;
            }
            cx.notify();
        });
        Some(modal("channel-create-modal", content, close, 170).into_any_element())
    }
}
