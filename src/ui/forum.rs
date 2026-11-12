//! フォーラムの投稿一覧・新しい投稿、チャンネルのスレッド一覧（右の枠）、メッセージからスレッドを作る画面
use gpui::assets::IconName;
use gpui::component::input::{Input, Textarea};
use gpui::prelude::*;
use gpui::*;

use super::*;
use crate::app::{author_name, snowflake_date, PostDraft};
use crate::model::{Channel, ForumTag};

/// タグの小さな札
fn tag_chip(id: impl Into<ElementId>, t: &ForumTag, on: bool) -> Stateful<Div> {
    let label = match &t.emoji_name {
        Some(e) if t.emoji_id.is_none() => format!("{e} {}", t.name),
        _ => t.name.clone(),
    };
    div()
        .id(id)
        .px(z(8.0))
        .py(z(2.0))
        .rounded_full()
        .flex_none()
        .bg(if on { BLURPLE() } else { WIDGET() })
        .cursor_pointer()
        .hover(|s| s.bg(WIDGET_HOVER()))
        .child(text(label, 12.0, if on { WHITE() } else { TEXT() }))
}

/// 「すべて」「アーカイブ済み」などの切り替えの札
fn filter_chip(id: impl Into<ElementId>, label: &str, on: bool) -> Stateful<Div> {
    div()
        .id(id)
        .px(z(10.0))
        .py(z(3.0))
        .rounded_full()
        .flex_none()
        .border_1()
        .border_color(if on { BLURPLE() } else { DIVIDER() })
        .bg(if on { BLURPLE() } else { CHAT() })
        .cursor_pointer()
        .child(text(
            label.to_string(),
            12.0,
            if on { WHITE() } else { TEXT() },
        ))
}

/// スレッドの「💬 12 · 3日前」のような一行
fn thread_meta(t: &Channel) -> String {
    let mut parts = Vec::new();
    if let Some(n) = t.message_count {
        parts.push(format!("{n} 件のメッセージ"));
    }
    let last = t.last_message_id.as_deref().unwrap_or(&t.id);
    let d = snowflake_date(last);
    if !d.is_empty() {
        parts.push(format!("最終 {d}"));
    }
    parts.join(" · ")
}

impl Root {
    /// フォーラムを開いているときに、メッセージ一覧と入力欄の代わりに出す
    pub fn forum_view(&mut self, forum: &Channel, cx: &mut Context<Self>) -> AnyElement {
        let Some(f) = self.app.forum.as_ref() else {
            return div().into_any_element();
        };
        // 上: 新しい投稿・タグで絞る・アーカイブ済み
        let mut bar = div()
            .flex_none()
            .px(z(16.0))
            .py(z(10.0))
            .flex()
            .flex_wrap()
            .items_center()
            .gap(z(6.0))
            .border_b_1()
            .border_color(DIVIDER());
        let can_post = self
            .app
            .channel_perms(self.app.sel_guild.as_deref(), &forum.id)
            .is_none_or(|p| p & crate::model::SEND_MESSAGES != 0);
        if can_post && self.app.post_draft.is_none() {
            bar = bar.child(
                fill_button("post_new", "新しい投稿", BLURPLE(), WHITE(), true).on_click(
                    cx.listener(|this, _, window, cx| {
                        this.app.post_draft = Some(PostDraft::default());
                        this.inp.post_title.update(cx, |s, cx| s.focus(window, cx));
                        cx.notify();
                    }),
                ),
            );
        }
        bar = bar.child(
            filter_chip("forum_all", "すべて", f.tag.is_none()).on_click(cx.listener(
                |this, _, _, cx| {
                    this.app.set_thread_filter(true, Some(None), None);
                    cx.notify();
                },
            )),
        );
        for t in &forum.available_tags {
            let id = t.id.clone();
            let on = f.tag.as_deref() == Some(t.id.as_str());
            bar = bar.child(
                tag_chip(SharedString::from(format!("forum_tag{}", t.id)), t, on).on_click(
                    cx.listener(move |this, _, _, cx| {
                        this.app.set_thread_filter(
                            true,
                            Some(if on { None } else { Some(id.clone()) }),
                            None,
                        );
                        cx.notify();
                    }),
                ),
            );
        }
        let archived = f.archived;
        bar = bar.child(
            filter_chip("forum_archived", "アーカイブ済み", archived).on_click(cx.listener(
                move |this, _, _, cx| {
                    this.app.set_thread_filter(true, None, Some(!archived));
                    cx.notify();
                },
            )),
        );
        let (n, key) = (f.items.len() + 1, (3, f.channel_id.clone(), f.rev));
        if self.forum_key != key {
            let same = self.forum_key.1 == key.1;
            let top = self.forum_list.logical_scroll_top();
            self.forum_list.reset(n);
            if same {
                self.forum_list.scroll_to(ListOffset {
                    item_ix: top.item_ix.min(n.saturating_sub(1)),
                    offset_in_item: top.offset_in_item,
                });
            }
            self.forum_key = key;
        }
        let mut col = div().flex_1().min_h_0().flex().flex_col().child(bar);
        if self.app.post_draft.is_some() {
            col = col.child(self.post_form(forum, cx));
        }
        let this = self.this.clone();
        col.child(
            list(self.forum_list.clone(), move |ix, _, cx| {
                let Some(e) = this.upgrade() else {
                    return div().into_any_element();
                };
                e.update(cx, |r, cx| r.forum_item(ix, cx))
            })
            .flex_1(),
        )
        .into_any_element()
    }

    /// 新しい投稿の欄
    fn post_form(&mut self, forum: &Channel, cx: &mut Context<Self>) -> Div {
        let Some(d) = self.app.post_draft.as_ref() else {
            return div();
        };
        let boxed = |e: AnyElement| {
            div()
                .bg(INPUT())
                .border_1()
                .border_color(DIVIDER())
                .rounded(z(8.0))
                .px(z(8.0))
                .child(e)
        };
        let mut tags = div().flex().flex_wrap().gap(z(6.0));
        for t in &forum.available_tags {
            let on = d.tags.contains(&t.id);
            let id = t.id.clone();
            tags = tags.child(
                tag_chip(SharedString::from(format!("post_tag{}", t.id)), t, on).on_click(
                    cx.listener(move |this, _, _, cx| {
                        if let Some(d) = this.app.post_draft.as_mut() {
                            if let Some(i) = d.tags.iter().position(|x| x == &id) {
                                d.tags.remove(i);
                            } else if d.tags.len() < 5 {
                                d.tags.push(id.clone());
                            }
                        }
                        cx.notify();
                    }),
                ),
            );
        }
        let sending = d.sending;
        card(SIDEBAR(), DIVIDER(), 8.0)
            .mx(z(16.0))
            .mt(z(10.0))
            .p(z(12.0))
            .flex_none()
            .flex()
            .flex_col()
            .gap(z(8.0))
            .child(boxed(
                Input::new(&self.inp.post_title)
                    .appearance(false)
                    .h(z(34.0))
                    .into_any_element(),
            ))
            .child(boxed(
                Textarea::new(&self.inp.post_body)
                    .appearance(false)
                    .into_any_element(),
            ))
            .when(!forum.available_tags.is_empty(), |c| {
                c.child(text(
                    if forum.requires_tag() {
                        "タグ（1つ以上・5つまで）"
                    } else {
                        "タグ（5つまで）"
                    },
                    12.0,
                    MUTED(),
                ))
                .child(tags)
            })
            .children(d.error.clone().map(|e| text(e, 13.0, RED())))
            .child(
                div()
                    .flex()
                    .justify_end()
                    .gap(z(8.0))
                    .child(
                        text_button("post_cancel", "キャンセル").on_click(cx.listener(
                            |this, _, _, cx| {
                                this.app.post_draft = None;
                                cx.notify();
                            },
                        )),
                    )
                    .child(
                        fill_button(
                            "post_send",
                            if sending { "投稿中…" } else { "投稿" },
                            BLURPLE(),
                            WHITE(),
                            !sending,
                        )
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.app.submit_post();
                            cx.notify();
                        })),
                    ),
            )
    }

    fn forum_item(&mut self, ix: usize, cx: &mut Context<Self>) -> AnyElement {
        let Some(f) = self.app.forum.as_ref() else {
            return div().into_any_element();
        };
        let Some(t) = f.items.get(ix) else {
            // 末尾: 読み込み中・失敗・空・続き
            return if let Some(e) = &f.error {
                div()
                    .p(z(16.0))
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap(z(8.0))
                    .child(text(e.clone(), 14.0, RED()))
                    .child(small_button("forum_retry", "再試行").on_click(cx.listener(
                        |this, _, _, cx| {
                            let more = this.app.forum.as_ref().is_some_and(|f| !f.items.is_empty());
                            this.app.load_threads(true, more);
                            cx.notify();
                        },
                    )))
            } else if f.loading {
                div()
                    .p(z(16.0))
                    .flex()
                    .justify_center()
                    .child(spinner(24.0))
            } else if f.items.is_empty() {
                div().p(z(24.0)).flex().justify_center().child(text(
                    if f.archived {
                        "アーカイブ済みの投稿はありません"
                    } else {
                        "まだ投稿がありません"
                    },
                    15.0,
                    MUTED(),
                ))
            } else if f.has_more {
                self.app.load_threads(true, true);
                div()
                    .p(z(16.0))
                    .flex()
                    .justify_center()
                    .child(spinner(24.0))
            } else {
                div().h(z(16.0))
            }
            .into_any_element();
        };
        let forum_tags = self
            .app
            .current_channel()
            .map(|c| c.available_tags.clone())
            .unwrap_or_default();
        let first = f.first_messages.get(&t.id);
        let preview = first
            .map(|m| m.content.replace('\n', " "))
            .unwrap_or_default();
        let author = first
            .map(|m| author_name(m).to_string())
            .unwrap_or_default();
        let mut tags = div().flex().flex_wrap().gap(z(4.0));
        for tag in forum_tags.iter().filter(|x| t.applied_tags.contains(&x.id)) {
            tags = tags.child(tag_chip(
                SharedString::from(format!("ft{}{}", t.id, tag.id)),
                tag,
                false,
            ));
        }
        let mut title = div().flex().items_center().gap(z(6.0));
        if t.flags & (1 << 1) != 0 {
            title = title.child(icon(IconName::Pin, 14.0, MUTED()));
        }
        if t.locked() {
            title = title.child(icon(IconName::Lock, 14.0, MUTED()));
        } else if t.archived() {
            title = title.child(icon(IconName::Archive, 14.0, MUTED()));
        }
        title = title.child(
            line(t.name.clone().unwrap_or_default(), 16.0, WHITE())
                .font_weight(FontWeight::MEDIUM)
                .flex_1()
                .min_w_0(),
        );
        let open = t.clone();
        div()
            .px(z(16.0))
            .pt(z(10.0))
            .child(
                card(SIDEBAR(), DIVIDER(), 8.0)
                    .id(SharedString::from(format!("post{}", t.id)))
                    .p(z(12.0))
                    .flex()
                    .flex_col()
                    .gap(z(6.0))
                    .cursor_pointer()
                    .hover(|s| s.bg(MSG_HOVER()))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.app.open_thread(open.clone());
                        cx.notify();
                    }))
                    .child(title)
                    .when(!t.applied_tags.is_empty(), |c| c.child(tags))
                    .when(!preview.is_empty(), |c| {
                        c.child(div().max_h(z(44.0)).overflow_hidden().child(text(
                            preview,
                            14.0,
                            TEXT(),
                        )))
                    })
                    .child(line(
                        if author.is_empty() {
                            thread_meta(t)
                        } else {
                            format!("{author} · {}", thread_meta(t))
                        },
                        12.0,
                        FAINT(),
                    )),
            )
            .into_any_element()
    }

    /// 右の枠: チャンネルのスレッド一覧
    pub fn threads_panel(&mut self, cx: &mut Context<Self>) -> Div {
        let Some(v) = self.app.thread_panel.as_ref() else {
            return div();
        };
        let archived = v.archived;
        let tabs = div()
            .flex_none()
            .px(z(12.0))
            .py(z(8.0))
            .flex()
            .gap(z(6.0))
            .child(
                filter_chip("threads_active", "アクティブ", !archived).on_click(cx.listener(
                    |this, _, _, cx| {
                        this.app.set_thread_filter(false, None, Some(false));
                        cx.notify();
                    },
                )),
            )
            .child(
                filter_chip("threads_archived", "アーカイブ済み", archived).on_click(cx.listener(
                    |this, _, _, cx| {
                        this.app.set_thread_filter(false, None, Some(true));
                        cx.notify();
                    },
                )),
            );
        let (n, key) = (
            v.items.len() + 1,
            (4, format!("{}{archived}", v.channel_id), v.rev),
        );
        self.sync_side_list(key, n);
        let this = self.this.clone();
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(super::panel::panel_head(
                "スレッド",
                IconName::MessagesSquare,
                cx,
            ))
            .child(tabs)
            .child(
                list(self.side_list.clone(), move |ix, _, cx| {
                    let Some(e) = this.upgrade() else {
                        return div().into_any_element();
                    };
                    e.update(cx, |r, cx| r.thread_item(ix, cx))
                })
                .flex_1(),
            )
    }

    fn thread_item(&mut self, ix: usize, cx: &mut Context<Self>) -> AnyElement {
        let Some(v) = self.app.thread_panel.as_ref() else {
            return div().into_any_element();
        };
        let Some(t) = v.items.get(ix) else {
            return if let Some(e) = &v.error {
                div()
                    .p(z(16.0))
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap(z(8.0))
                    .child(text(e.clone(), 14.0, RED()))
                    .child(
                        small_button("threads_retry", "再試行").on_click(cx.listener(
                            |this, _, _, cx| {
                                this.app.load_threads(false, false);
                                cx.notify();
                            },
                        )),
                    )
            } else if v.loading {
                div()
                    .p(z(16.0))
                    .flex()
                    .justify_center()
                    .child(spinner(24.0))
            } else if v.items.is_empty() {
                div().p(z(16.0)).flex().justify_center().child(text(
                    if v.archived {
                        "アーカイブ済みのスレッドはありません"
                    } else {
                        "スレッドはありません"
                    },
                    14.0,
                    MUTED(),
                ))
            } else if v.has_more {
                self.app.load_threads(false, true);
                div()
                    .p(z(16.0))
                    .flex()
                    .justify_center()
                    .child(spinner(24.0))
            } else {
                div().h(z(8.0))
            }
            .into_any_element();
        };
        let joined = self.app.joined_threads.contains(&t.id);
        let open = t.clone();
        div()
            .id(SharedString::from(format!("thr{}", t.id)))
            .mx(z(8.0))
            .px(z(10.0))
            .py(z(8.0))
            .rounded(z(6.0))
            .cursor_pointer()
            .hover(|s| s.bg(HOVER()))
            .on_click(cx.listener(move |this, _, _, cx| {
                this.app.open_thread(open.clone());
                cx.notify();
            }))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(z(6.0))
                    .when(t.locked(), |d| d.child(icon(IconName::Lock, 14.0, MUTED())))
                    .child(
                        line(t.name.clone().unwrap_or_default(), 15.0, WHITE())
                            .font_weight(FontWeight::MEDIUM)
                            .flex_1()
                            .min_w_0(),
                    )
                    .when(joined, |d| d.child(text("参加中", 11.0, MUTED()))),
            )
            .child(line(thread_meta(t), 12.0, FAINT()))
            .into_any_element()
    }

    pub fn folder_dialog(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let id = self.app.folder_edit.clone()?;
        let width = (self.app.screen_w - 56.0).clamp(0.0, 440.0);
        let mut colors = div().flex().flex_wrap().gap(z(8.0));
        for color in [
            0x5865f2u32,
            0x57f287,
            0xfee75c,
            0xeb459e,
            0xed4245,
            0x747f8d,
        ] {
            let bg = crate::color::Color32::from_rgb(
                (color >> 16) as u8,
                (color >> 8) as u8,
                color as u8,
            );
            colors = colors.child(
                div()
                    .id(("folder-color", color as usize))
                    .size(z(32.0))
                    .rounded_full()
                    .bg(bg)
                    .when(self.app.folder_color == color, |d| {
                        d.border_2().border_color(WHITE())
                    })
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.app.folder_color = color;
                        cx.notify();
                    })),
            );
        }
        let content =
            card(SIDEBAR(), BORDER(), 8.0)
                .p(z(20.0))
                .w(z(width))
                .flex()
                .flex_col()
                .gap(z(10.0))
                .child(medium("フォルダを編集", 20.0, WHITE()))
                .child(
                    div().bg(INPUT()).rounded(z(8.0)).px(z(8.0)).child(
                        Input::new(&self.inp.folder_name)
                            .appearance(false)
                            .h(z(34.0)),
                    ),
                )
                .child(colors)
                .child(
                    div()
                        .flex()
                        .justify_end()
                        .gap(z(8.0))
                        .child(text_button("folder_edit_cancel", "キャンセル").on_click(
                            cx.listener(|this, _, _, cx| {
                                this.app.folder_edit = None;
                                cx.notify();
                            }),
                        ))
                        .child(
                            fill_button("folder_edit_save", "保存", BLURPLE(), WHITE(), true)
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    let name = this.app.folder_name.trim();
                                    if name.is_empty() {
                                        this.app.show_toast("フォルダ名を入力してください");
                                    } else if let Some(folder) = this
                                        .app
                                        .settings
                                        .guild_folders
                                        .iter_mut()
                                        .find(|f| f.id == id)
                                    {
                                        folder.name = name.to_owned();
                                        folder.color = this.app.folder_color;
                                        this.app.folder_edit = None;
                                        this.app.save_settings();
                                    }
                                    cx.notify();
                                })),
                        ),
                );
        let close = cx.listener(|this, _: &MouseDownEvent, _, cx| {
            this.app.folder_edit = None;
            cx.notify();
        });
        Some(modal("folder_edit_dialog", content, close, 170).into_any_element())
    }

    /// メッセージからスレッドを作る画面
    pub fn thread_dialog(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let d = self.app.thread_draft.as_ref()?;
        let w = (self.app.screen_w - 56.0).clamp(0.0, 440.0);
        let sending = d.sending;
        let content = card(SIDEBAR(), BORDER(), 8.0)
            .part(Part::DialogBox)
            .p(z(20.0))
            .w(z(w))
            .flex()
            .flex_col()
            .gap(z(10.0))
            .child(medium("スレッドを作成", 20.0, WHITE()))
            .child(
                card(BG(), DIVIDER(), 8.0)
                    .p(z(10.0))
                    .child(
                        line(author_name(&d.message).to_string(), 14.0, WHITE())
                            .font_weight(FontWeight::MEDIUM),
                    )
                    .child(div().max_h(z(80.0)).overflow_hidden().child(text(
                        d.message.content.clone(),
                        13.0,
                        TEXT(),
                    ))),
            )
            .child(
                div()
                    .bg(INPUT())
                    .border_1()
                    .border_color(DIVIDER())
                    .rounded(z(8.0))
                    .px(z(8.0))
                    .child(
                        Input::new(&self.inp.thread_name)
                            .appearance(false)
                            .h(z(34.0)),
                    ),
            )
            .children(d.error.clone().map(|e| text(e, 13.0, RED())))
            .child(
                div()
                    .flex()
                    .justify_end()
                    .gap(z(8.0))
                    .child(
                        text_button("thread_cancel", "キャンセル")
                            .min_w(z(96.0))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.app.thread_draft = None;
                                cx.notify();
                            })),
                    )
                    .child(
                        fill_button(
                            "thread_ok",
                            if sending { "作成中…" } else { "作成" },
                            BLURPLE(),
                            WHITE(),
                            !sending,
                        )
                        .min_w(z(96.0))
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.app.submit_thread_draft();
                            cx.notify();
                        })),
                    ),
            );
        let close = cx.listener(|this, _: &MouseDownEvent, _, cx| {
            if !this.app.thread_draft.as_ref().is_some_and(|d| d.sending) {
                this.app.thread_draft = None;
            }
            cx.notify();
        });
        Some(modal("thread_modal", content, close, 170).into_any_element())
    }
}
