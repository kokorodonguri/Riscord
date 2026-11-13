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
        // サーバー設定の画面の中で出しているときは、そちらで描く
        if self.app.server_settings.is_some() {
            return None;
        }
        self.moderation_view(false, cx)
    }

    /// 監査ログ・BAN・招待・イベント・AutoMod の中身。embedded = サーバー設定の画面の右側に出す（枠・題・タブを付けない）
    pub fn moderation_view(
        &mut self,
        embedded: bool,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
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
        let mut content = if embedded {
            div().flex_1().min_w_0().flex().flex_col().gap(z(8.0))
        } else {
            card(SIDEBAR(), BORDER(), 8.0)
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
                                .child(medium(
                                    tr!("サーバー管理", "Server Management", "服务器管理"),
                                    20.0,
                                    WHITE(),
                                ))
                                .child(line(guild_name, 13.0, MUTED())),
                        )
                        .child(
                            icon_button("moderation_reload", IconName::RefreshCw, 28.0)
                                .tooltip(tip(tr!("再読み込み", "Reload", "重新加载")))
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
                )
        };

        let mut tabs = div().flex().flex_wrap().gap(z(4.0));
        for tab in ModTab::ALL {
            if embedded || !self.app.can_open_mod_tab(&m.guild_id, tab) {
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
        if !embedded {
            content = content.child(tabs);
        }
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
                            fill_button(
                                "event_new",
                                tr!("イベントを作成", "Create Event", "创建活动"),
                                BLURPLE(),
                                WHITE(),
                                !busy,
                            )
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
                                tr!(
                                    "キーワードのルールを作成",
                                    "Create Keyword Rule",
                                    "创建关键词规则"
                                ),
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
        if embedded {
            return Some(content.into_any_element());
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
                    d.child(spinner(16.0)).child(text(
                        tr!("読み込み中…", "Loading…", "正在加载…"),
                        14.0,
                        MUTED(),
                    ))
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
                    ModTab::Audit => tr!("記録はありません", "No entries", "没有记录"),
                    ModTab::Bans => tr!(
                        "BAN されたユーザーはいません",
                        "No banned users",
                        "没有被封禁的用户"
                    ),
                    ModTab::Invites => tr!("招待はありません", "No invites", "没有邀请"),
                    ModTab::Events => tr!(
                        "予定されたイベントはありません",
                        "No scheduled events",
                        "没有预定的活动"
                    ),
                    ModTab::AutoMod => tr!("ルールはありません", "No rules", "没有规则"),
                },
                14.0,
                MUTED(),
            ));
        }
        for row in rows {
            let mut actions = div().flex().flex_none().gap(z(4.0));
            if m.tab == ModTab::Events && row.event.is_some() {
                let id = row.id.clone();
                actions = actions.child(
                    text_button(format!("event_edit_{id}"), tr!("編集", "Edit", "编辑")).on_click(
                        cx.listener(move |this, _, _, cx| {
                            this.app.start_event_draft(Some(&id));
                            cx.notify();
                        }),
                    ),
                );
            }
            if let Some(enabled) = row.enabled {
                let id = row.id.clone();
                actions = actions.child(
                    fill_button(
                        format!("rule_toggle_{id}"),
                        if enabled {
                            tr!("無効にする", "Disable", "禁用")
                        } else {
                            tr!("有効にする", "Enable", "启用")
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
                    (_, true) => tr!(
                        "もう一度押して確定",
                        "Press again to confirm",
                        "再按一次确认"
                    ),
                    (ModTab::Bans, _) => tr!("BAN を解除", "Unban", "解除封禁"),
                    _ => tr!("削除", "Delete", "删除"),
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
                    text_button("audit_more", tr!("さらに読み込む", "Load more", "加载更多"))
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
                tr!("場所を指定", "Set location", "指定地点"),
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
                    tr!("イベントを編集", "Edit Event", "编辑活动")
                } else {
                    tr!("イベントを作成", "Create Event", "创建活动")
                },
                16.0,
                WHITE(),
            ))
            .child(field(
                tr!("イベント名", "Event name", "活动名称"),
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
                            tr!(
                                "開始（ローカル時刻）",
                                "Start (local time)",
                                "开始（本地时间）"
                            ),
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
                            tr!("終了", "End", "结束"),
                            Input::new(&self.inp.event_end)
                                .appearance(false)
                                .disabled(busy)
                                .h(z(34.0)),
                        )
                        .flex_1()
                        .min_w(z(180.0)),
                    ),
            )
            .child(text(tr!("開催場所", "Location", "地点"), 13.0, MUTED()))
            .child(places);
        if d.channel_id.is_none() {
            form = form.child(field(
                tr!("場所", "Location", "地点"),
                Input::new(&self.inp.event_location)
                    .appearance(false)
                    .disabled(busy)
                    .h(z(34.0)),
            ));
        }
        form.child(field(
            tr!("説明", "Description", "说明"),
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
            .child(medium(
                tr!(
                    "キーワードのルールを作成",
                    "Create Keyword Rule",
                    "创建关键词规则"
                ),
                16.0,
                WHITE(),
            ))
            .child(text(
                tr!(
                    "入力した語を含むメッセージの送信をブロックします。",
                    "Blocks messages that contain the words you enter.",
                    "阻止发送包含所输入词语的消息。"
                ),
                13.0,
                MUTED(),
            ))
            .child(field(
                tr!("ルール名", "Rule name", "规则名称"),
                Input::new(&self.inp.rule_name)
                    .appearance(false)
                    .disabled(busy)
                    .h(z(34.0)),
            ))
            .child(field(
                tr!("キーワード", "Keywords", "关键词"),
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
                text_button(
                    format!("{kind}_cancel"),
                    tr!("キャンセル", "Cancel", "取消"),
                )
                .on_click(cx.listener(|this, _, _, cx| {
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
                })),
            )
            .child(
                fill_button(
                    format!("{kind}_save"),
                    if busy {
                        tr!("保存中…", "Saving…", "正在保存…")
                    } else {
                        tr!("保存", "Save", "保存")
                    },
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
