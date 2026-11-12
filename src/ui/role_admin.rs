//! サーバーのロールを編集するダイアログ。
use gpui::assets::IconName;
use gpui::component::input::{Input, Textarea};
use gpui::prelude::*;
use gpui::*;

use super::*;
use crate::color::Color32;

pub(super) const PERMISSIONS: &[(u32, &str)] = &[
    (0, "招待を作成"),
    (1, "メンバーをキック"),
    (2, "メンバーを BAN"),
    (3, "管理者"),
    (4, "チャンネルを管理"),
    (5, "サーバーを管理"),
    (6, "リアクションを追加"),
    (7, "監査ログを見る"),
    (8, "優先スピーカー"),
    (9, "画面を共有"),
    (10, "チャンネルを見る"),
    (11, "メッセージを送信"),
    (12, "読み上げメッセージ"),
    (13, "メッセージを管理"),
    (14, "リンクを埋め込み"),
    (15, "ファイルを添付"),
    (16, "履歴を読む"),
    (17, "全員にメンション"),
    (18, "外部の絵文字"),
    (19, "サーバーインサイト"),
    (20, "ボイスに接続"),
    (21, "発言"),
    (22, "メンバーをミュート"),
    (23, "メンバーのスピーカーをミュート"),
    (24, "メンバーを移動"),
    (25, "音声検出を使用"),
    (26, "自分のニックネームを変更"),
    (27, "ニックネームを管理"),
    (28, "ロールを管理"),
    (29, "Webhook を管理"),
    (30, "絵文字などを管理"),
    (31, "アプリコマンドを使用"),
    (32, "発言をリクエスト"),
    (33, "イベントを管理"),
    (34, "スレッドを管理"),
    (35, "公開スレッドを作成"),
    (36, "非公開スレッドを作成"),
    (37, "外部のスタンプ"),
    (38, "スレッドに送信"),
    (39, "アクティビティを使用"),
    (40, "メンバーをタイムアウト"),
    (41, "収益化インサイト"),
    (42, "サウンドボードを使用"),
    (43, "絵文字などを作成"),
    (44, "イベントを作成"),
    (45, "外部サウンド"),
    (46, "ボイスメッセージを送信"),
    (48, "ボイスチャンネルのステータス"),
    (49, "投票を作成"),
    (50, "外部アプリを使用"),
    (51, "メッセージをピン留め"),
    (52, "低速モードを無視"),
];

impl Root {
    pub fn role_admin_dialog(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        // サーバー設定の画面の中で出しているときは、そちらで描く
        if self.app.server_settings.is_some() {
            return None;
        }
        self.role_admin_view(false, cx)
    }

    /// ロール・サーバー設定・チャンネル権限の中身。embedded = サーバー設定の画面の右側に出す（枠と題を付けない）
    pub fn role_admin_view(
        &mut self,
        embedded: bool,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let a = self.app.role_admin.as_ref()?;
        let width = (self.app.screen_w - 32.0).clamp(0.0, 620.0);
        let height = (self.pane_h - 120.0).clamp(100.0, 600.0);
        let busy = a.mutation.is_some();
        let loading = a.roles_request.is_some()
            || a.member_request.is_some()
            || a.owner_request.is_some()
            || a.target
                .as_ref()
                .is_some_and(|t| t.member_request.is_some())
            || a.channel_permissions.as_ref().is_some_and(|v| {
                v.channel_request.is_some()
                    || v.parent_request.is_some()
                    || v.member_request.is_some()
            });
        let allowed = if a.channel_permissions.is_some() {
            self.app.can_manage_channel_permissions()
        } else if a.editing_guild {
            self.app.can_manage_guild()
        } else {
            self.app.can_manage_roles()
        };
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
                            medium(
                                if a.channel_permissions.is_some() {
                                    "チャンネル権限"
                                } else if a.editing_guild {
                                    "サーバー設定"
                                } else if a.target.is_some() {
                                    "メンバーのロール"
                                } else {
                                    "サーバーのロール"
                                },
                                20.0,
                                WHITE(),
                            )
                            .flex_1(),
                        )
                        .child(icon_button("roles_close", IconName::X, 28.0).on_click(
                            cx.listener(|this, _, _, cx| {
                                if this
                                    .app
                                    .role_admin
                                    .as_ref()
                                    .is_some_and(|a| a.mutation.is_none())
                                {
                                    this.app.role_admin = None;
                                }
                                cx.notify();
                            }),
                        )),
                )
        };
        if let Some(error) = &a.error {
            content = content.child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap(z(6.0))
                    .items_center()
                    .child(text(error.clone(), 13.0, RED()).flex_1().min_w_0())
                    .child(
                        text_button("roles_retry", "再読み込み").on_click(cx.listener(
                            |this, _, _, cx| {
                                this.app.reload_role_admin();
                                cx.notify();
                            },
                        )),
                    ),
            );
        }
        if loading {
            content = content.child(div().child(text("設定と権限を読み込み中…", 14.0, MUTED())));
        } else if !allowed {
            content = content.child(text(
                if a.channel_permissions.is_some() {
                    "チャンネルの権限を管理できないか、必要な情報を確認できませんでした。"
                } else if a.editing_guild {
                    "サーバーを管理する権限がないか、権限を確認できませんでした。"
                } else {
                    "ロールを管理する権限がないか、権限を確認できませんでした。"
                },
                14.0,
                MUTED(),
            ));
        }
        if a.channel_permissions.is_some() {
            content = content.child(self.channel_permissions_form(height, cx));
            return Some(self.role_admin_frame(content, embedded, cx));
        }
        if a.editing_guild {
            if let Some(draft) = &a.guild_edit {
                let editable = self.app.guild_draft_editable();
                let stale = draft
                    .latest
                    .as_ref()
                    .is_some_and(|latest| latest != &draft.original);
                if stale {
                    content = content.child(div().flex().flex_wrap().items_center().gap(z(8.0))
                        .child(text("サーバー設定が変更されました。最新の内容を読み直してください。", 13.0, RED()).flex_1().min_w_0())
                        .child(text_button("guild_reset", "最新の内容を使う").on_click(cx.listener(|this, _, _, cx| { this.app.reset_guild_draft(); cx.notify(); }))));
                }
                let mut form = div()
                    .id("guild-settings-scroll")
                    .flex_1()
                    .min_h_0()
                    .max_h(z(height))
                    .overflow_y_scroll()
                    .flex()
                    .flex_col()
                    .gap(z(8.0))
                    .child(text("サーバー名", 13.0, MUTED()))
                    .child(
                        div().bg(INPUT()).rounded(z(6.0)).px(z(8.0)).child(
                            Input::new(&self.inp.guild_name)
                                .appearance(false)
                                .disabled(!editable)
                                .h(z(34.0)),
                        ),
                    );
                if draft.original.description.is_some() {
                    form = form.child(text("サーバーの説明", 13.0, MUTED())).child(
                        div().bg(INPUT()).rounded(z(6.0)).px(z(8.0)).child(
                            Textarea::new(&self.inp.guild_description)
                                .appearance(false)
                                .disabled(!editable),
                        ),
                    );
                }
                let groups: [(&str, u8, Option<u64>, &[(u64, &str)]); 4] = [
                    (
                        "認証レベル",
                        0,
                        draft.verification,
                        &[(0, "なし"), (1, "低"), (2, "中"), (3, "高"), (4, "最高")],
                    ),
                    (
                        "既定の通知",
                        1,
                        draft.notifications,
                        &[(0, "すべて"), (1, "メンションのみ")],
                    ),
                    (
                        "不適切な画像のフィルター",
                        2,
                        draft.content_filter,
                        &[(0, "無効"), (1, "ロールのないメンバー"), (2, "全メンバー")],
                    ),
                    (
                        "離席タイムアウト",
                        3,
                        draft.afk_timeout,
                        &[
                            (60, "1分"),
                            (300, "5分"),
                            (900, "15分"),
                            (1800, "30分"),
                            (3600, "1時間"),
                        ],
                    ),
                ];
                for (label, field, current, options) in groups {
                    let Some(current) = current else { continue };
                    form = form.child(text(label, 13.0, MUTED()));
                    let mut choices = div().flex().flex_wrap().gap(z(5.0));
                    if !options.iter().any(|(value, _)| *value == current) {
                        choices = choices.child(text(format!("現在: {current}"), 12.0, MUTED()));
                    }
                    for &(value, label) in options {
                        choices = choices.child(
                            fill_button(
                                format!("guild-setting-{field}-{value}"),
                                label,
                                if value == current {
                                    BLURPLE()
                                } else {
                                    WIDGET()
                                },
                                WHITE(),
                                editable,
                            )
                            .on_click(cx.listener(
                                move |this, _, _, cx| {
                                    if this.app.guild_draft_editable() {
                                        if let Some(d) = this
                                            .app
                                            .role_admin
                                            .as_mut()
                                            .and_then(|a| a.guild_edit.as_mut())
                                        {
                                            match field {
                                                0 => d.verification = Some(value),
                                                1 => d.notifications = Some(value),
                                                2 => d.content_filter = Some(value),
                                                3 => d.afk_timeout = Some(value),
                                                _ => {}
                                            }
                                        }
                                    }
                                    cx.notify();
                                },
                            )),
                        );
                    }
                    form = form.child(choices);
                }
                content = content.child(form).child(
                    div().flex().justify_end().gap(z(8.0)).child(
                        fill_button(
                            "guild_save",
                            if busy {
                                "保存中…"
                            } else {
                                "変更を保存"
                            },
                            BLURPLE(),
                            WHITE(),
                            editable,
                        )
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.app.save_guild_settings();
                            cx.notify();
                        })),
                    ),
                );
            }
            return Some(self.role_admin_frame(content, embedded, cx));
        }
        if let Some(target) = &a.target {
            content = content.child(text(target.name.clone(), 15.0, WHITE()));
            if a.owner_id.as_deref() == Some(target.user_id.as_str())
                && self
                    .app
                    .me
                    .as_ref()
                    .is_none_or(|me| me.id != target.user_id)
            {
                content = content.child(text(
                    "サーバー所有者のロールは変更できません。",
                    13.0,
                    MUTED(),
                ));
            }
            if busy {
                content = content.child(text("ロールを更新中…", 13.0, MUTED()));
            }
            if let (Some(roles), Some(assigned)) = (&a.roles, &target.roles) {
                let mut ordered = roles.clone();
                ordered.sort_by(|x, y| y.position.cmp(&x.position).then_with(|| x.id.cmp(&y.id)));
                let mut list = div()
                    .id("member-role-list")
                    .flex_1()
                    .min_h_0()
                    .max_h(z(height))
                    .overflow_y_scroll()
                    .flex()
                    .flex_col()
                    .gap(z(3.0));
                for role in ordered.iter().filter(|r| r.id != a.guild_id) {
                    let editable = !busy && self.app.can_assign_member_role(role);
                    let assigned = assigned.contains(&role.id);
                    let id = role.id.clone();
                    let label = role.name.clone();
                    list = list.child(
                        div()
                            .id(SharedString::from(format!("member-role-{id}")))
                            .min_h(z(36.0))
                            .px(z(8.0))
                            .rounded(z(4.0))
                            .flex()
                            .items_center()
                            .gap(z(8.0))
                            .when(editable, |d| d.cursor_pointer().hover(|d| d.bg(HOVER())))
                            .when(!editable, |d| d.opacity(0.55))
                            .child(text(if assigned { "☑" } else { "☐" }, 16.0, TEXT()))
                            .child(line(label, 14.0, TEXT()).flex_1().min_w_0())
                            .when(role.managed != Some(false), |d| {
                                d.child(text("連携 / 管理不可", 11.0, MUTED()))
                            })
                            .when(!editable && role.managed == Some(false) && !busy, |d| {
                                d.child(text("権限または階層により変更不可", 11.0, MUTED()))
                            })
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if editable {
                                    this.app.toggle_member_role(&id);
                                }
                                cx.notify();
                            })),
                    );
                }
                content = content.child(list);
            }
            return Some(self.role_admin_frame(content, embedded, cx));
        }
        if let Some(draft) = &a.draft {
            let editable = self.app.role_draft_editable();
            if !editable && !loading {
                content = content.child(text("ロールが変更されたか、編集権限がありません。キャンセルして一覧を確認してください。", 13.0, RED()));
            }
            let everyone = draft.original.as_ref().is_some_and(|r| r.id == a.guild_id);
            let mut form = div()
                .id("role-edit-scroll")
                .flex_1()
                .min_h_0()
                .max_h(z(height))
                .overflow_y_scroll()
                .flex()
                .flex_col()
                .gap(z(8.0))
                .child(medium(
                    if draft.original.is_some() {
                        "ロールを編集"
                    } else {
                        "ロールを作成"
                    },
                    16.0,
                    WHITE(),
                ))
                .child(
                    div().bg(INPUT()).rounded(z(6.0)).px(z(8.0)).child(
                        Input::new(&self.inp.role_name)
                            .appearance(false)
                            .disabled(busy || everyone || !editable)
                            .h(z(34.0)),
                    ),
                )
                .child(text("色", 13.0, MUTED()));
            let mut colors = div().flex().flex_wrap().gap(z(6.0));
            for color in [
                0u32, 0x5865f2, 0x57f287, 0xfee75c, 0xeb459e, 0xed4245, 0xffffff,
            ] {
                let bg = if color == 0 {
                    WIDGET()
                } else {
                    Color32::from_rgb((color >> 16) as u8, (color >> 8) as u8, color as u8)
                };
                colors = colors.child(
                    div()
                        .id(("role-color", color as usize))
                        .size(z(26.0))
                        .rounded_full()
                        .bg(bg)
                        .when(draft.color == color, |d| d.border_2().border_color(WHITE()))
                        .when(!busy && !everyone && editable, |d| d.cursor_pointer())
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if !busy && !everyone && this.app.role_draft_editable() {
                                if let Some(d) =
                                    this.app.role_admin.as_mut().and_then(|a| a.draft.as_mut())
                                {
                                    d.color = color;
                                }
                            }
                            cx.notify();
                        })),
                );
            }
            form = form
                .child(colors)
                .child(
                    text_button(
                        "role_hoist",
                        if draft.hoist {
                            "☑ メンバーを分けて表示"
                        } else {
                            "☐ メンバーを分けて表示"
                        },
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if !busy && !everyone && this.app.role_draft_editable() {
                            if let Some(d) =
                                this.app.role_admin.as_mut().and_then(|a| a.draft.as_mut())
                            {
                                d.hoist = !d.hoist;
                            }
                        }
                        cx.notify();
                    })),
                )
                .child(text("権限（自分が持つ権限のみ変更可能）", 13.0, MUTED()))
                .child(text(
                    "管理者を有効にすると、チャンネルの権限設定をすべて無視します。",
                    12.0,
                    MUTED(),
                ));
            for &(index, label) in PERMISSIONS {
                let bit = 1u128 << index;
                let enabled = editable && !busy && self.app.can_toggle_role_permission(bit);
                let on = draft.permissions & bit != 0;
                form = form.child(
                    div()
                        .id(("role-perm", index as usize))
                        .min_h(z(28.0))
                        .px(z(6.0))
                        .rounded(z(4.0))
                        .flex()
                        .items_center()
                        .gap(z(8.0))
                        .when(enabled, |d| d.cursor_pointer().hover(|d| d.bg(HOVER())))
                        .when(!enabled, |d| d.opacity(0.5))
                        .child(text(if on { "☑" } else { "☐" }, 15.0, TEXT()))
                        .child(text(label, 14.0, TEXT()))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if enabled {
                                this.app.toggle_role_permission(bit);
                            }
                            cx.notify();
                        })),
                );
            }
            content = content.child(form).child(
                div()
                    .flex()
                    .justify_end()
                    .gap(z(8.0))
                    .child(
                        text_button("role_cancel", "キャンセル").on_click(cx.listener(
                            |this, _, _, cx| {
                                if let Some(a) = this.app.role_admin.as_mut() {
                                    if a.mutation.is_none() {
                                        a.draft = None;
                                    }
                                }
                                cx.notify();
                            },
                        )),
                    )
                    .child(
                        fill_button(
                            "role_save",
                            if busy { "保存中…" } else { "保存" },
                            BLURPLE(),
                            WHITE(),
                            editable && !busy,
                        )
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.app.save_role();
                            cx.notify();
                        })),
                    ),
            );
        } else if let Some(roles) = &a.roles {
            let mut ordered = roles.clone();
            ordered.sort_by(|x, y| y.position.cmp(&x.position).then_with(|| x.id.cmp(&y.id)));
            let mut list = div()
                .id("role-list-scroll")
                .flex_1()
                .min_h_0()
                .max_h(z(height))
                .overflow_y_scroll()
                .flex()
                .flex_col()
                .gap(z(3.0));
            for (i, role) in ordered.iter().enumerate() {
                let editable = !busy && self.app.can_manage_role(role);
                let name = role.name.clone();
                let rid = role.id.clone();
                let mut row = div()
                    .min_h(z(38.0))
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(z(4.0))
                    .px(z(4.0))
                    .border_b_1()
                    .border_color(DIVIDER())
                    .child(line(name, 14.0, TEXT()).flex_1().min_w(z(90.0)));
                if role.managed != Some(false) {
                    row = row.child(text("連携 / 管理不可", 11.0, MUTED()));
                }
                if editable {
                    let id = rid.clone();
                    row = row.child(text_button(format!("role-edit-{id}"), "編集").on_click(
                        cx.listener(move |this, _, _, cx| {
                            this.app.edit_role(Some(&id));
                            cx.notify();
                        }),
                    ));
                    if role.id != a.guild_id {
                        for (label, other) in [
                            ("↑", i.checked_sub(1).and_then(|n| ordered.get(n))),
                            ("↓", ordered.get(i + 1)),
                        ] {
                            if let Some(target) = other.filter(|r| {
                                r.id != a.guild_id
                                    && r.position != role.position
                                    && self.app.can_manage_role(r)
                            }) {
                                let id = rid.clone();
                                let target_id = target.id.clone();
                                row = row.child(
                                    text_button(format!("role-move-{label}-{id}"), label).on_click(
                                        cx.listener(move |this, _, _, cx| {
                                            this.app.reorder_role(&id, &target_id);
                                            cx.notify();
                                        }),
                                    ),
                                );
                            }
                        }
                        let id = rid.clone();
                        let confirm = a.confirm_delete.as_deref() == Some(id.as_str());
                        row = row.child(
                            text_button(
                                format!("role-delete-{id}"),
                                if confirm { "削除を確定" } else { "削除" },
                            )
                            .on_click(cx.listener(
                                move |this, _, _, cx| {
                                    if this
                                        .app
                                        .role_admin
                                        .as_ref()
                                        .and_then(|a| a.confirm_delete.as_deref())
                                        == Some(id.as_str())
                                    {
                                        this.app.delete_role(&id);
                                    } else if let Some(a) = this.app.role_admin.as_mut() {
                                        a.confirm_delete = Some(id.clone());
                                    }
                                    cx.notify();
                                },
                            )),
                        );
                    }
                }
                list = list.child(row);
            }
            content = content.child(list).child(
                fill_button(
                    "role_create",
                    "ロールを作成",
                    BLURPLE(),
                    WHITE(),
                    allowed && !busy,
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    this.app.edit_role(None);
                    cx.notify();
                })),
            );
        }
        Some(self.role_admin_frame(content, embedded, cx))
    }

    fn role_admin_frame(&self, content: Div, embedded: bool, cx: &mut Context<Self>) -> AnyElement {
        if embedded {
            return content.into_any_element();
        }
        let close = cx.listener(|this, _: &MouseDownEvent, _, cx| {
            if this
                .app
                .role_admin
                .as_ref()
                .is_some_and(|a| a.mutation.is_none())
            {
                this.app.role_admin = None;
            }
            cx.notify();
        });
        modal("role-admin-modal", content, close, 170).into_any_element()
    }
}
