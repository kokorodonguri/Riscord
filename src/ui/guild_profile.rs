//! このサーバーでの自分のプロフィールを編集する画面。
use gpui::assets::IconName;
use gpui::component::input::{Input, Textarea};
use gpui::prelude::*;
use gpui::*;

use super::*;

impl Root {
    pub fn guild_profile_dialog(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let e = self.app.guild_profile.as_ref()?;
        let busy = !e.pending.is_empty();
        let ready = e.original.is_some();
        let editable = ready && !busy;
        let nick_allowed = self.app.can_change_nick(&e.guild_id);
        let guild_name = self
            .app
            .guilds
            .iter()
            .find(|g| g.id == e.guild_id)
            .map(|g| g.name.clone())
            .unwrap_or_default();
        let width = (self.app.screen_w - 32.0).clamp(0.0, 480.0);
        let field = |label: &'static str, count: Option<(usize, usize)>, child: AnyElement| {
            div()
                .flex()
                .flex_col()
                .gap(z(4.0))
                .child(
                    div()
                        .flex()
                        .child(text(label, 13.0, MUTED()).flex_1())
                        .children(count.map(|(n, max)| {
                            text(
                                format!("{n}/{max}"),
                                12.0,
                                if n > max { RED() } else { FAINT() },
                            )
                        })),
                )
                .child(div().bg(INPUT()).rounded(z(6.0)).px(z(8.0)).child(child))
        };
        let mut content = card(SIDEBAR(), BORDER(), 8.0)
            .w(z(width))
            .p(z(20.0))
            .flex()
            .flex_col()
            .gap(z(12.0))
            .child(
                div()
                    .flex()
                    .items_center()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .child(medium(
                                tr!("サーバープロフィールを編集", "Edit Server Profile"),
                                20.0,
                                WHITE(),
                            ))
                            .child(line(guild_name, 13.0, MUTED())),
                    )
                    .child(
                        icon_button("guild_profile_close", IconName::X, 28.0).on_click(
                            cx.listener(|this, _, _, cx| {
                                this.app.close_guild_profile();
                                cx.notify();
                            }),
                        ),
                    ),
            );
        if !ready && e.error.is_none() {
            content = content.child(
                div()
                    .flex()
                    .items_center()
                    .gap(z(8.0))
                    .child(spinner(16.0))
                    .child(text(tr!("読み込み中…", "Loading…"), 14.0, MUTED())),
            );
        }
        if nick_allowed {
            content = content.child(field(
                tr!("ニックネーム", "Nickname"),
                Some((e.draft.nick.chars().count(), st::NICK_MAX)),
                Input::new(&self.inp.guild_nick)
                    .appearance(false)
                    .disabled(!editable)
                    .h(z(34.0))
                    .into_any_element(),
            ));
        }
        content = content
            .child(field(
                tr!("代名詞", "Pronouns"),
                Some((e.draft.pronouns.chars().count(), st::PRONOUNS_MAX)),
                Input::new(&self.inp.guild_pronouns)
                    .appearance(false)
                    .disabled(!editable)
                    .h(z(34.0))
                    .into_any_element(),
            ))
            .child(field(
                tr!("自己紹介", "About Me"),
                Some((e.draft.bio.chars().count(), st::BIO_MAX)),
                Textarea::new(&self.inp.guild_bio)
                    .appearance(false)
                    .disabled(!editable)
                    .into_any_element(),
            ))
            .child(text(
                tr!("このサーバーのメンバーにだけ表示されます。空にするとアカウントのプロフィールが使われます。", "Shown only to members of this server. Leave empty to use your account profile."),
                12.0,
                FAINT(),
            ));
        if let Some(err) = &e.error {
            content = content.child(text(err.clone(), 13.0, RED()));
        }
        content = content.child(
            div()
                .flex()
                .justify_end()
                .gap(z(8.0))
                .child(
                    text_button("guild_profile_cancel", tr!("キャンセル", "Cancel")).on_click(
                        cx.listener(|this, _, _, cx| {
                            this.app.close_guild_profile();
                            cx.notify();
                        }),
                    ),
                )
                .child(
                    fill_button(
                        "guild_profile_save",
                        if busy {
                            tr!("保存中…", "Saving…")
                        } else {
                            tr!("保存", "Save")
                        },
                        BLURPLE(),
                        WHITE(),
                        editable,
                    )
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.app.save_guild_profile();
                        cx.notify();
                    })),
                ),
        );
        let close = cx.listener(|this, _: &MouseDownEvent, _, cx| {
            this.app.close_guild_profile();
            cx.notify();
        });
        Some(modal("guild-profile-modal", content, close, 170).into_any_element())
    }
}
