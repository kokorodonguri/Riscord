//! サーバー管理のダイアログ（監査ログ・BAN・招待・イベント・AutoMod）。
use gpui::assets::IconName;
use gpui::component::input::{Input, Textarea};
use gpui::prelude::*;
use gpui::*;

use super::*;
use crate::app::ModTab;

fn field(label: &'static str, child: impl IntoElement) -> Div {
    div()
        .flex()
        .flex_col()
        .gap(z(4.0))
        .child(text(label, 13.0, MUTED()))
        .child(div().bg(INPUT()).rounded(z(6.0)).px(z(8.0)).child(child))
}

impl Root {
    pub fn moderation_dialog(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let m = self.app.moderation.as_ref()?;
        let width = (self.app.screen_w - 32.0).clamp(0.0, 640.0);
        let height = (self.pane_h - 200.0).clamp(100.0, 560.0);
        let busy = m.mutation.is_some();
        let guild_name = self
            .app
            .guilds
            .iter()
            .find(|g| g.id == m.guild_id)
            .map(|g| g.name.clone())
            .unwrap_or_default();
        let mut content = card(SIDEBAR(), BORDER(), 8.0)
            .w(z(width))
            .max_h(z((self.pane_h - 32.0).max(100.0)))
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
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .child(medium("サーバー管理", 20.0, WHITE()))
                            .child(line(guild_name, 13.0, MUTED())),
                    )
                    .child(
                        icon_button("moderation_reload", IconName::RefreshCw, 28.0)
                            .tooltip(tip("再読み込み"))
                            .on_click(cx.listener(|this, _, _, cx| {
                                if this
                                    .app
                                    .moderation
                                    .as_ref()
                                    .is_some_and(|m| m.mutation.is_none())
                                {
                                    this.app.reload_moderation();
                                }
                                cx.notify();
                            })),
                    )
                    .child(icon_button("moderation_close", IconName::X, 28.0).on_click(
                        cx.listener(|this, _, _, cx| {
                            this.app.close_moderation();
                            cx.notify();
                        }),
                    )),
            );

        let mut tabs = div().flex().flex_wrap().gap(z(4.0));
        for tab in ModTab::ALL {
            if !self.app.can_open_mod_tab(&m.guild_id, tab) {
                continue;
            }
            tabs = tabs.child(
                fill_button(
                    format!("moderation_tab_{}", tab.label()),
                    tab.label(),
                    if m.tab == tab { BLURPLE() } else { WIDGET() },
                    WHITE(),
                    !busy,
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.app.set_mod_tab(tab);
                    cx.notify();
                })),
            );
        }
        content = content.child(tabs);
        if let Some(e) = &m.error {
            content = content.child(text(e.clone(), 13.0, RED()));
        }

        let form_open = (m.tab == ModTab::Events && m.event_draft.is_some())
            || (m.tab == ModTab::AutoMod && m.rule_draft.is_some());
        if form_open {
            let form = if m.tab == ModTab::Events {
                self.event_form(busy, cx)
            } else {
                self.rule_form(busy, cx)
            };
            content = content.child(
                div()
                    .id("moderation-form")
                    .max_h(z(height))
                    .overflow_y_scroll()
                    .child(form),
            );
        } else {
            match m.tab {
                ModTab::Events => {
                    content = content.child(
                        div().flex().child(
                            fill_button("event_new", "イベントを作成", BLURPLE(), WHITE(), !busy)
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.app.start_event_draft(None);
                                    cx.notify();
                                })),
                        ),
                    );
                }
                ModTab::AutoMod => {
                    content = content.child(
                        div().flex().child(
                            fill_button(
                                "rule_new",
                                "キーワードのルールを作成",
                                BLURPLE(),
                                WHITE(),
                                !busy,
                            )
                            .on_click(cx.listener(|this, _, _, cx| {
                                if let Some(m) = this
                                    .app
                                    .moderation
                                    .as_mut()
                                    .filter(|m| m.mutation.is_none())
                                {
                                    m.rule_draft = Some(Default::default());
                                    m.error = None;
                                }
                                cx.notify();
                            })),
                        ),
                    );
                }
                _ => {}
            }
            content = content.child(self.moderation_rows(height, busy, cx));
        }
        let close = cx.listener(|this, _: &MouseDownEvent, _, cx| {
            this.app.close_moderation();
            cx.notify();
        });
        Some(modal("moderation-modal", content, close, 170).into_any_element())
    }

    fn moderation_rows(&self, height: f32, busy: bool, cx: &mut Context<Self>) -> AnyElement {
        let m = self.app.moderation.as_ref().unwrap();
        let Some(rows) = &m.rows else {
            return div()
                .flex()
                .items_center()
                .gap(z(8.0))
                .when(m.request.is_some(), |d| {
                    d.child(spinner(16.0))
                        .child(text("読み込み中…", 14.0, MUTED()))
                })
                .into_any_element();
        };
        let mut list = div()
            .id("moderation-rows")
            .max_h(z(height))
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .gap(z(2.0));
        if rows.is_empty() {
            list = list.child(text(
                match m.tab {
                    ModTab::Audit => "記録はありません",
                    ModTab::Bans => "BAN されたユーザーはいません",
                    ModTab::Invites => "招待はありません",
                    ModTab::Events => "予定されたイベントはありません",
                    ModTab::AutoMod => "ルールはありません",
                },
                14.0,
                MUTED(),
            ));
        }
        for row in rows {
            let mut actions = div().flex().flex_none().gap(z(4.0));
            if m.tab == ModTab::Events && row.event.is_some() {
                let id = row.id.clone();
                actions = actions.child(text_button(format!("event_edit_{id}"), "編集").on_click(
                    cx.listener(move |this, _, _, cx| {
                        this.app.start_event_draft(Some(&id));
                        cx.notify();
                    }),
                ));
            }
            if let Some(enabled) = row.enabled {
                let id = row.id.clone();
                actions = actions.child(
                    fill_button(
                        format!("rule_toggle_{id}"),
                        if enabled {
                            "無効にする"
                        } else {
                            "有効にする"
                        },
                        WIDGET(),
                        WHITE(),
                        !busy,
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.app.toggle_rule(&id);
                        cx.notify();
                    })),
                );
            }
            if m.tab != ModTab::Audit {
                let id = row.id.clone();
                let confirming = m.confirm.as_deref() == Some(format!("delete:{id}").as_str());
                let label = match (m.tab, confirming) {
                    (_, true) => "もう一度押して確定",
                    (ModTab::Bans, _) => "BAN を解除",
                    _ => "削除",
                };
                actions = actions.child(
                    fill_button(
                        format!("moderation_delete_{id}"),
                        label,
                        if confirming { RED() } else { WIDGET() },
                        WHITE(),
                        !busy,
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.app.delete_mod_row(&id);
                        cx.notify();
                    })),
                );
            }
            list = list.child(
                div()
                    .px(z(8.0))
                    .py(z(6.0))
                    .rounded(z(6.0))
                    .hover(|d| d.bg(HOVER()))
                    .flex()
                    .items_center()
                    .gap(z(8.0))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .child(line(row.title.clone(), 14.0, TEXT()))
                            .child(line(row.detail.clone(), 12.0, MUTED())),
                    )
                    .child(actions),
            );
        }
        if m.tab == ModTab::Audit && m.has_more {
            list = list.child(div().pt(z(6.0)).flex().justify_center().child(
                if m.request.is_some() {
                    spinner(16.0).into_any_element()
                } else {
                    text_button("audit_more", "さらに読み込む")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.app.more_audit_log();
                            cx.notify();
                        }))
                        .into_any_element()
                },
            ));
        }
        list.into_any_element()
    }

    fn event_form(&self, busy: bool, cx: &mut Context<Self>) -> Div {
        let m = self.app.moderation.as_ref().unwrap();
        let d = m.event_draft.as_ref().unwrap();
        let editing = d.editing.is_some();
        let mut places = div().flex().flex_wrap().gap(z(4.0)).child(
            fill_button(
                "event_place_external",
                "場所を指定",
                if d.channel_id.is_none() {
                    BLURPLE()
                } else {
                    WIDGET()
                },
                WHITE(),
                !busy,
            )
            .on_click(cx.listener(|this, _, _, cx| {
                if let Some(d) = this
                    .app
                    .moderation
                    .as_mut()
                    .and_then(|m| m.event_draft.as_mut())
                {
                    d.channel_id = None;
                }
                cx.notify();
            })),
        );
        for (id, name) in self.app.mod_voice_channels() {
            let selected = d.channel_id.as_deref() == Some(id.as_str());
            places = places.child(
                fill_button(
                    format!("event_place_{id}"),
                    format!("🔊 {name}"),
                    if selected { BLURPLE() } else { WIDGET() },
                    WHITE(),
                    !busy,
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    if let Some(d) = this
                        .app
                        .moderation
                        .as_mut()
                        .and_then(|m| m.event_draft.as_mut())
                    {
                        d.channel_id = Some(id.clone());
                    }
                    cx.notify();
                })),
            );
        }
        let mut form = div()
            .flex()
            .flex_col()
            .gap(z(8.0))
            .child(medium(
                if editing {
                    "イベントを編集"
                } else {
                    "イベントを作成"
                },
                16.0,
                WHITE(),
            ))
            .child(field(
                "イベント名",
                Input::new(&self.inp.event_name)
                    .appearance(false)
                    .disabled(busy)
                    .h(z(34.0)),
            ))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap(z(8.0))
                    .child(
                        field(
                            "開始（ローカル時刻）",
                            Input::new(&self.inp.event_start)
                                .appearance(false)
                                .disabled(busy)
                                .h(z(34.0)),
                        )
                        .flex_1()
                        .min_w(z(180.0)),
                    )
                    .child(
                        field(
                            "終了",
                            Input::new(&self.inp.event_end)
                                .appearance(false)
                                .disabled(busy)
                                .h(z(34.0)),
                        )
                        .flex_1()
                        .min_w(z(180.0)),
                    ),
            )
            .child(text("開催場所", 13.0, MUTED()))
            .child(places);
        if d.channel_id.is_none() {
            form = form.child(field(
                "場所",
                Input::new(&self.inp.event_location)
                    .appearance(false)
                    .disabled(busy)
                    .h(z(34.0)),
            ));
        }
        form.child(field(
            "説明",
            Textarea::new(&self.inp.event_description)
                .appearance(false)
                .disabled(busy),
        ))
        .child(self.form_buttons("event", busy, cx))
    }

    fn rule_form(&self, busy: bool, cx: &mut Context<Self>) -> Div {
        div()
            .flex()
            .flex_col()
            .gap(z(8.0))
            .child(medium("キーワードのルールを作成", 16.0, WHITE()))
            .child(text(
                "入力した語を含むメッセージの送信をブロックします。",
                13.0,
                MUTED(),
            ))
            .child(field(
                "ルール名",
                Input::new(&self.inp.rule_name)
                    .appearance(false)
                    .disabled(busy)
                    .h(z(34.0)),
            ))
            .child(field(
                "キーワード",
                Textarea::new(&self.inp.rule_keywords)
                    .appearance(false)
                    .disabled(busy),
            ))
            .child(self.form_buttons("rule", busy, cx))
    }

    fn form_buttons(&self, kind: &'static str, busy: bool, cx: &mut Context<Self>) -> Div {
        div()
            .flex()
            .justify_end()
            .gap(z(8.0))
            .child(
                text_button(format!("{kind}_cancel"), "キャンセル").on_click(cx.listener(
                    |this, _, _, cx| {
                        if let Some(m) = this
                            .app
                            .moderation
                            .as_mut()
                            .filter(|m| m.mutation.is_none())
                        {
                            m.event_draft = None;
                            m.event_original = None;
                            m.rule_draft = None;
                            m.error = None;
                        }
                        cx.notify();
                    },
                )),
            )
            .child(
                fill_button(
                    format!("{kind}_save"),
                    if busy { "保存中…" } else { "保存" },
                    BLURPLE(),
                    WHITE(),
                    !busy,
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    if kind == "event" {
                        this.app.save_event();
                    } else {
                        this.app.save_rule();
                    }
                    cx.notify();
                })),
            )
    }
}
