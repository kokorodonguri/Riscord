//! チャンネルの権限上書きの選択と編集。
use gpui::component::input::Input;
use gpui::prelude::*;
use gpui::*;

use super::*;

impl Root {
    pub(super) fn channel_permissions_form(&self, height: f32, cx: &mut Context<Self>) -> Div {
        let Some(admin) = self.app.role_admin.as_ref() else {
            return div();
        };
        let Some(view) = admin.channel_permissions.as_ref() else {
            return div();
        };
        let Some(snapshot) = view.snapshot.as_ref() else {
            return div();
        };
        let busy = admin.mutation.is_some();
        let allowed = self.app.can_manage_channel_permissions();
        let mut form = div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .gap(z(8.0))
            .child(text(format!("チャンネル: {}", snapshot.name), 14.0, TEXT()));
        if snapshot.parent_id.is_some() {
            form = form.child(text(
                "カテゴリと別の上書きを保存すると、権限の同期が解除されます。",
                12.0,
                MUTED(),
            ));
        }
        if let Some(draft) = &view.draft {
            let editable = self.app.channel_draft_editable();
            let label = if draft.kind == 0 {
                admin
                    .roles
                    .as_ref()
                    .and_then(|roles| roles.iter().find(|r| r.id == draft.id))
                    .map(|r| r.name.clone())
                    .unwrap_or_else(|| draft.id.clone())
            } else {
                self.app
                    .find_user(&draft.id)
                    .map(|u| u.display().to_owned())
                    .unwrap_or_else(|| format!("メンバー {}", draft.id))
            };
            form = form.child(medium(label, 16.0, WHITE()));
            if !editable && !busy && view.channel_request.is_none() && view.parent_request.is_none()
            {
                form = form.child(text(
                    "設定または権限が変わりました。一覧に戻って選択し直してください。",
                    13.0,
                    RED(),
                ));
            }
            let mut list = div()
                .id("channel-permission-bits")
                .flex_1()
                .min_h_0()
                .max_h(z(height))
                .overflow_y_scroll()
                .flex()
                .flex_col()
                .gap(z(4.0));
            for &(index, label) in super::role_admin::PERMISSIONS
                .iter()
                .filter(|(bit, _)| crate::app::CHANNEL_PERMISSION_BITS.contains(bit))
            {
                let bit = 1u128 << index;
                let enabled = editable && self.app.can_toggle_channel_permission(bit);
                let current = if draft.allow & bit != 0 {
                    1
                } else if draft.deny & bit != 0 {
                    2
                } else {
                    0
                };
                let mut options = div().flex().gap(z(3.0));
                for (state, label) in [(0u8, "継承"), (1, "許可"), (2, "拒否")] {
                    options = options.child(
                        fill_button(
                            format!("chperm-{index}-{state}"),
                            label,
                            if current == state {
                                BLURPLE()
                            } else {
                                WIDGET()
                            },
                            WHITE(),
                            enabled,
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if enabled {
                                this.app.set_channel_permission(bit, state);
                            }
                            cx.notify();
                        })),
                    );
                }
                list = list.child(
                    div()
                        .min_h(z(35.0))
                        .flex()
                        .flex_wrap()
                        .items_center()
                        .gap(z(4.0))
                        .child(
                            line(label.to_string(), 13.0, TEXT())
                                .flex_1()
                                .min_w(z(100.0)),
                        )
                        .child(options),
                );
            }
            form = form.child(list);
            let mut actions = div().flex().flex_wrap().justify_end().gap(z(6.0));
            if draft.original.is_some() {
                let can_delete = self.app.can_delete_channel_overwrite();
                let confirm = view.confirm_delete;
                actions = actions.child(
                    fill_button(
                        "chperm-delete",
                        if confirm {
                            "削除を確定"
                        } else {
                            "上書きを削除"
                        },
                        if confirm { RED() } else { WIDGET() },
                        WHITE(),
                        can_delete,
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if !this.app.can_delete_channel_overwrite() {
                            return;
                        }
                        if confirm {
                            this.app.delete_channel_overwrite();
                        } else if let Some(view) = this
                            .app
                            .role_admin
                            .as_mut()
                            .and_then(|a| a.channel_permissions.as_mut())
                        {
                            view.confirm_delete = true;
                        }
                        cx.notify();
                    })),
                );
                if !can_delete && editable {
                    form = form.child(text(
                        "変更できない権限を含む上書きは削除できません。",
                        12.0,
                        MUTED(),
                    ));
                }
            }
            actions = actions.child(text_button("chperm-back", "一覧に戻る").on_click(
                cx.listener(|this, _, _, cx| {
                    if let Some(admin) = this.app.role_admin.as_mut() {
                        if admin.mutation.is_none() {
                            if let Some(view) = admin.channel_permissions.as_mut() {
                                view.draft = None;
                                view.confirm_delete = false;
                            }
                        }
                    }
                    cx.notify();
                }),
            ));
            actions = actions.child(
                fill_button(
                    "chperm-save",
                    if busy { "保存中…" } else { "保存" },
                    BLURPLE(),
                    WHITE(),
                    editable,
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    this.app.save_channel_overwrite();
                    cx.notify();
                })),
            );
            return form.child(actions);
        }
        form = form.child(text(
            "権限を設定するロールまたはメンバーを選択",
            13.0,
            MUTED(),
        ));
        let mut list = div()
            .id("channel-permission-targets")
            .flex_1()
            .min_h_0()
            .max_h(z(height))
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .gap(z(4.0));
        if let Some(roles) = &admin.roles {
            let mut roles = roles.clone();
            roles.sort_by(|a, b| b.position.cmp(&a.position).then_with(|| a.id.cmp(&b.id)));
            for role in roles {
                let enabled = self.app.can_select_channel_target(&role.id, 0);
                let has = snapshot
                    .overwrites
                    .iter()
                    .any(|o| o.kind == 0 && o.id == role.id);
                let id = role.id.clone();
                list = list.child(
                    div()
                        .id(SharedString::from(format!("chperm-role-{id}")))
                        .min_h(z(34.0))
                        .px(z(8.0))
                        .rounded(z(4.0))
                        .flex()
                        .items_center()
                        .gap(z(8.0))
                        .when(enabled, |d| d.cursor_pointer().hover(|d| d.bg(HOVER())))
                        .when(!enabled, |d| d.opacity(0.5))
                        .child(line(role.name, 14.0, TEXT()).flex_1().min_w_0())
                        .children(has.then(|| text("上書きあり", 11.0, MUTED())))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if enabled {
                                this.app.edit_channel_overwrite(&id, 0);
                            }
                            cx.notify();
                        })),
                );
            }
        }
        for overwrite in snapshot.overwrites.iter().filter(|o| o.kind == 1) {
            let id = overwrite.id.clone();
            let label = self
                .app
                .find_user(&id)
                .map(|u| u.display().to_owned())
                .unwrap_or_else(|| format!("メンバー {id}"));
            let enabled = self.app.can_select_channel_target(&id, 1);
            list = list.child(
                div()
                    .id(SharedString::from(format!("chperm-member-{id}")))
                    .min_h(z(34.0))
                    .px(z(8.0))
                    .rounded(z(4.0))
                    .flex()
                    .items_center()
                    .gap(z(8.0))
                    .when(enabled, |d| d.cursor_pointer().hover(|d| d.bg(HOVER())))
                    .when(!enabled, |d| d.opacity(0.5))
                    .child(line(label, 14.0, TEXT()).flex_1().min_w_0())
                    .child(text("メンバー", 11.0, MUTED()))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if enabled {
                            this.app.edit_channel_overwrite(&id, 1);
                        }
                        cx.notify();
                    })),
            );
        }
        form = form.child(list).child(
            div()
                .flex()
                .flex_wrap()
                .gap(z(6.0))
                .items_center()
                .child(
                    div()
                        .flex_1()
                        .min_w(z(140.0))
                        .bg(INPUT())
                        .rounded(z(6.0))
                        .px(z(6.0))
                        .child(
                            Input::new(&self.inp.channel_member_id)
                                .appearance(false)
                                .disabled(!allowed)
                                .h(z(34.0)),
                        ),
                )
                .child(
                    fill_button(
                        "chperm-add-member",
                        if view.member_request.is_some() {
                            "確認中…"
                        } else {
                            "メンバーを追加"
                        },
                        BLURPLE(),
                        WHITE(),
                        allowed,
                    )
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.app.verify_channel_member();
                        cx.notify();
                    })),
                ),
        );
        form
    }
}
