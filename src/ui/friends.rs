//! フレンド一覧と申請管理。
use gpui::component::input::Input;
use gpui::prelude::*;
use gpui::*;

use super::*;
use crate::app::{user_avatar_url, FriendAction};

impl Root {
    pub fn friends_view(&mut self, cx: &mut Context<Self>) -> Div {
        let state = &self.app.friends;
        let mut tabs = div().flex().flex_wrap().gap(z(5.0));
        for (tab, label) in [
            (0, "すべて"),
            (1, "オンライン"),
            (2, "保留中"),
            (3, "ブロック"),
            (4, "フレンドに追加"),
        ] {
            tabs = tabs.child(
                fill_button(
                    format!("friends-tab-{tab}"),
                    label,
                    if state.tab == tab {
                        BLURPLE()
                    } else {
                        WIDGET()
                    },
                    WHITE(),
                    true,
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.app.friends.tab = tab;
                    this.app.friends.confirm = None;
                    cx.notify();
                })),
            );
        }
        let mut body = div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .gap(z(8.0))
            .p(z(12.0));
        if let Some(error) = &state.error {
            body = body.child(
                div()
                    .flex()
                    .items_center()
                    .gap(z(8.0))
                    .flex_wrap()
                    .child(text(error.clone(), 13.0, RED()).flex_1().min_w_0())
                    .child(
                        text_button("friends_retry", "再読み込み").on_click(cx.listener(
                            |this, _, _, cx| {
                                this.app.fetch_friends();
                                cx.notify();
                            },
                        )),
                    ),
            );
        }
        if state.tab == 4 {
            let pending = state
                .pending
                .values()
                .any(|a| matches!(a, FriendAction::Request));
            return div()
                .flex_1()
                .min_w_0()
                .h_full()
                .bg(CHAT())
                .flex()
                .flex_col()
                .child(div().p(z(12.0)).child(tabs))
                .child(
                    body.child(medium("フレンドに追加", 18.0, WHITE()))
                        .child(text(
                            "追加したい相手のユーザー名を入力してください。",
                            14.0,
                            MUTED(),
                        ))
                        .child(
                            div()
                                .flex()
                                .flex_wrap()
                                .gap(z(8.0))
                                .items_center()
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w(z(160.0))
                                        .bg(INPUT())
                                        .rounded(z(6.0))
                                        .px(z(8.0))
                                        .child(
                                            Input::new(&self.inp.friend_name)
                                                .appearance(false)
                                                .disabled(pending)
                                                .h(z(36.0)),
                                        ),
                                )
                                .child(
                                    fill_button(
                                        "friend-request",
                                        if pending {
                                            "送信中…"
                                        } else {
                                            "申請を送信"
                                        },
                                        BLURPLE(),
                                        WHITE(),
                                        !pending,
                                    )
                                    .on_click(cx.listener(
                                        |this, _, _, cx| {
                                            this.app.friend_action(FriendAction::Request);
                                            cx.notify();
                                        },
                                    )),
                                ),
                        ),
                );
        }
        let mut rows: Vec<_> = state
            .relationships
            .iter()
            .filter(|r| match state.tab {
                0 => r.kind == 1,
                1 => {
                    r.kind == 1
                        && self
                            .app
                            .presences
                            .get(&r.id)
                            .is_some_and(|p| p.status != "offline")
                }
                2 => r.kind == 3 || r.kind == 4,
                3 => r.kind == 2,
                _ => false,
            })
            .cloned()
            .collect();
        rows.sort_by_cached_key(|r| r.user.display().to_lowercase());
        if rows.is_empty() {
            let note = if state.loading.is_some() && !state.loaded {
                "読み込み中…"
            } else if !state.loaded && state.error.is_none() {
                "フレンド一覧を読み込めませんでした"
            } else {
                "表示するフレンドはいません"
            };
            body = body.child(div().p(z(20.0)).child(text(note, 14.0, MUTED())));
        } else {
            let rows = std::rc::Rc::new(rows);
            body = body.child(
                uniform_list(
                    "friends-list",
                    rows.len(),
                    cx.processor(
                        move |this: &mut Root, range: std::ops::Range<usize>, _, cx| {
                            range
                                .filter_map(|i| rows.get(i))
                                .map(|r| {
                                    let id = r.id.clone();
                                    let user = r.user.clone();
                                    let pending = this
                                        .app
                                        .friends
                                        .pending
                                        .values()
                                        .any(|a| a.user_id() == Some(id.as_str()));
                                    let status = this
                                        .app
                                        .presences
                                        .get(&id)
                                        .map_or("offline", |p| p.status.as_str());
                                    let detail = match r.kind {
                                        2 => "ブロック中",
                                        3 => "受信したフレンド申請",
                                        4 => "送信済みフレンド申請",
                                        _ => crate::app::status_label(status),
                                    };
                                    let profile = div()
                                        .id(SharedString::from(format!("friend-profile-{id}")))
                                        .flex()
                                        .items_center()
                                        .gap(z(10.0))
                                        .flex_1()
                                        .min_w(z(130.0))
                                        .cursor_pointer()
                                        .child(with_status(
                                            avatar(
                                                Some(&user_avatar_url(&user)),
                                                36.0,
                                                user.display(),
                                                18.0,
                                                BLURPLE(),
                                            ),
                                            36.0,
                                            status,
                                            CHAT(),
                                        ))
                                        .child(
                                            div()
                                                .min_w_0()
                                                .flex()
                                                .flex_col()
                                                .child(line(
                                                    user.display().to_string(),
                                                    15.0,
                                                    WHITE(),
                                                ))
                                                .child(line(detail.to_string(), 12.0, MUTED())),
                                        )
                                        .on_click(cx.listener(move |this, _, window, cx| {
                                            let p = window.mouse_position();
                                            this.app.open_profile(
                                                user.clone(),
                                                Bounds::new(p, size(px(0.), px(0.))),
                                                false,
                                            );
                                            cx.notify();
                                        }));
                                    let mut actions = div().flex().flex_wrap().gap(z(4.0));
                                    match r.kind {
                                        1 => {
                                            actions = actions.child(
                                                this.friend_row_button(&id, "DM", 0, pending, cx),
                                            );
                                            actions = actions.child(
                                                this.friend_row_button(&id, "削除", 1, pending, cx),
                                            );
                                            actions = actions.child(this.friend_row_button(
                                                &id,
                                                "ブロック",
                                                2,
                                                pending,
                                                cx,
                                            ));
                                        }
                                        2 => {
                                            actions = actions.child(
                                                this.friend_row_button(&id, "解除", 1, pending, cx),
                                            )
                                        }
                                        3 => {
                                            actions = actions.child(
                                                this.friend_row_button(&id, "承認", 3, pending, cx),
                                            );
                                            actions = actions.child(
                                                this.friend_row_button(&id, "拒否", 1, pending, cx),
                                            );
                                        }
                                        4 => {
                                            actions = actions.child(this.friend_row_button(
                                                &id,
                                                "取り消す",
                                                1,
                                                pending,
                                                cx,
                                            ))
                                        }
                                        _ => {}
                                    }
                                    div()
                                        .h(z(88.0))
                                        .px(z(6.0))
                                        .py(z(5.0))
                                        .border_b_1()
                                        .border_color(DIVIDER())
                                        .flex()
                                        .flex_wrap()
                                        .items_center()
                                        .gap(z(5.0))
                                        .child(profile)
                                        .child(actions)
                                        .into_any_element()
                                })
                                .collect::<Vec<_>>()
                        },
                    ),
                )
                .flex_1()
                .min_h_0(),
            );
        }
        div()
            .flex_1()
            .min_w_0()
            .h_full()
            .bg(CHAT())
            .flex()
            .flex_col()
            .child(div().p(z(12.0)).child(tabs))
            .child(body)
    }

    pub(super) fn friend_row_button(
        &self,
        id: &str,
        label: &'static str,
        kind: u8,
        pending: bool,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let id = id.to_owned();
        let confirmed = self
            .app
            .friends
            .confirm
            .as_ref()
            .is_some_and(|(user, k)| user == &id && *k == kind);
        let destructive = kind == 1 || kind == 2;
        let caption = if confirmed { "確定" } else { label };
        let element_id = format!("friend-{kind}-{id}");
        fill_button(
            element_id,
            caption,
            if confirmed { RED() } else { WIDGET() },
            WHITE(),
            !pending,
        )
        .on_click(cx.listener(move |this, _, _, cx| {
            if pending {
                return;
            }
            if destructive
                && !this
                    .app
                    .friends
                    .confirm
                    .as_ref()
                    .is_some_and(|(user, k)| user == &id && *k == kind)
            {
                this.app.friends.confirm = Some((id.clone(), kind));
            } else {
                this.app.friends.confirm = None;
                let action = match kind {
                    0 => FriendAction::OpenDm(id.clone()),
                    1 => FriendAction::Remove(id.clone()),
                    2 => FriendAction::Block(id.clone()),
                    _ => FriendAction::Accept(id.clone()),
                };
                this.app.friend_action(action);
            }
            cx.notify();
        }))
    }
}
