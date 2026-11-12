//! 小さなメニュー（＋・マイク/ヘッドホンの ⌄）とプロフィールのポップアップ
use gpui::assets::IconName;
use gpui::prelude::*;
use gpui::*;

use super::*;
use crate::app::{
    darken, local_dt, name_color, rgb, snowflake_date, status_label, user_avatar_url, ProfileState,
    STATUSES,
};
use crate::color::Color32;
use crate::model::ProfileData;
use crate::net::{self, Cmd};
use crate::voice;

/// ポップアップの箱
fn popup_box() -> Div {
    div()
        .bg(SIDEBAR())
        .border_1()
        .border_color(BORDER())
        .rounded(z(8.0))
        .part(Part::MenuBox)
        .shadow(popup_shadow())
}

/// メニューの1行（左に印、右に › を出せる）
fn menu_row(
    id: impl Into<ElementId>,
    mark: AnyElement,
    label: impl Into<SharedString>,
    chevron: bool,
) -> Stateful<Div> {
    div()
        .id(id)
        .group("mr")
        .h(z(40.0))
        .px(z(10.0))
        .rounded(z(6.0))
        .flex()
        .items_center()
        .gap(z(10.0))
        .cursor_pointer()
        .text_color(TEXT())
        .hover(|s| s.bg(part_hover(Part::MenuRow, HOVER())).text_color(WHITE()))
        .part(Part::MenuRow)
        .child(div().w(z(20.0)).flex().justify_center().child(mark))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .text_size(z(15.0))
                .truncate()
                .child(label.into()),
        )
        .when(chevron, |d| {
            d.child(
                icon(IconName::ChevronRight, 14.0, TEXT())
                    .group_hover("mr", |s| s.text_color(WHITE())),
            )
        })
}

fn menu_divider() -> Div {
    div().my(z(4.0)).mx(z(10.0)).child(hline(DIVIDER()))
}

/// ラジオボタンの丸
fn radio(on: bool) -> Div {
    let d = div().size(z(16.0)).flex_none().rounded_full();
    if on {
        d.bg(WHITE())
            .flex()
            .items_center()
            .justify_center()
            .child(div().size(z(8.0)).rounded_full().bg(BLURPLE()))
    } else {
        d.border_2().border_color(MUTED())
    }
}

impl Root {
    pub fn menu_layer(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let (menu, pos) = self.menu?;
        let close = cx.listener(|this, _: &MouseDownEvent, _, cx| {
            this.menu = None;
            cx.notify();
        });
        let content: AnyElement = match menu {
            PopMenu::Plus => {
                let mut col = popup_box()
                    .p(z(6.0))
                    .w(z(252.0))
                    .flex()
                    .flex_col()
                    .gap(z(2.0));
                col = col.child(
                    menu_row(
                        "plus_upload",
                        icon(IconName::Upload, 20.0, TEXT()).into_any_element(),
                        "ファイルをアップロード",
                        false,
                    )
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.menu = None;
                        this.app.pick_files();
                        cx.notify();
                    })),
                );
                col = col.child(
                    menu_row(
                        "plus_paste",
                        icon(IconName::ClipboardPaste, 20.0, TEXT()).into_any_element(),
                        "画像を貼り付け",
                        false,
                    )
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.menu = None;
                        if !this.app.paste_image() {
                            this.app.status = "クリップボードに画像がありません".into();
                        }
                        cx.notify();
                    })),
                );
                col.into_any_element()
            }
            PopMenu::Mic | PopMenu::Headset => self
                .audio_menu(menu == PopMenu::Mic, window, cx)
                .into_any_element(),
            PopMenu::Guild => self.guild_menu(cx)?.into_any_element(),
            PopMenu::Message => self.message_menu(cx)?.into_any_element(),
            PopMenu::Emoji => self.emoji_picker(cx).into_any_element(),
        };
        // 右クリックのメニューはカーソルの右下に、ボタンのメニューはボタンの上に出す
        let (at, anchor) = if matches!(menu, PopMenu::Guild | PopMenu::Message) {
            (pos + point(z(4.0), z(0.0)), Anchor::TopLeft)
        } else if menu == PopMenu::Emoji {
            // 入力欄の右端のボタンから、左上へ開く
            (pos - point(z(0.0), z(20.0)), Anchor::BottomRight)
        } else {
            (pos - point(z(0.0), z(8.0)), Anchor::BottomLeft)
        };
        Some(
            deferred(
                anchored()
                    .position(at)
                    .anchor(anchor)
                    .snap_to_window_with_margin(z(8.0))
                    .child(div().occlude().on_mouse_down_out(close).child(content)),
            )
            .with_priority(1)
            .into_any_element(),
        )
    }

    /// メッセージの右クリックメニュー
    fn message_menu(&mut self, cx: &mut Context<Self>) -> Option<Div> {
        let m = self.menu_msg.clone()?;
        // 削除済みとして残しているメッセージには、返信・編集・削除をさせない
        let deleted = m.log.as_ref().is_some_and(|l| l.deleted_at.is_some());
        let mine = !deleted && self.app.me.as_ref().is_some_and(|u| u.id == m.author.id);
        let close = |this: &mut Root| {
            this.menu = None;
            this.menu_msg = None;
        };
        let mut col = popup_box()
            .p(z(6.0))
            .w(z(240.0))
            .flex()
            .flex_col()
            .gap(z(2.0));
        let m1 = m.clone();
        col = col.when(!deleted, |col| {
            col.child(
                menu_row(
                    "mm_reply",
                    icon(IconName::Reply, 18.0, TEXT()).into_any_element(),
                    "返信",
                    false,
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    close(this);
                    if this.app.editing.take().is_some() {
                        this.app.input.clear();
                    }
                    this.app.reply_to = Some(m1.clone());
                    this.app.focus_input = true;
                    cx.notify();
                })),
            )
        });
        if mine {
            let m1 = m.clone();
            col = col.child(
                menu_row(
                    "mm_edit",
                    icon(IconName::Pencil, 18.0, TEXT()).into_any_element(),
                    "編集",
                    false,
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    close(this);
                    this.app.input = crate::emoji::shorten(&m1.content).0;
                    this.app.editing = Some(m1.clone());
                    this.app.reply_to = None;
                    this.app.focus_input = true;
                    cx.notify();
                })),
            );
        }
        if !m.content.is_empty() {
            let t = m.content.clone();
            col = col.child(
                menu_row(
                    "mm_copy",
                    icon(IconName::Copy, 18.0, TEXT()).into_any_element(),
                    "テキストをコピー",
                    false,
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    close(this);
                    cx.write_to_clipboard(ClipboardItem::new_string(t.clone()));
                    this.app.show_toast("テキストをコピーしました");
                    cx.notify();
                })),
            );
        }
        let id = m.id.clone();
        col = col.child(
            menu_row(
                "mm_copy_id",
                icon(IconName::Copy, 18.0, TEXT()).into_any_element(),
                "メッセージIDをコピー",
                false,
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                close(this);
                cx.write_to_clipboard(ClipboardItem::new_string(id.clone()));
                this.app.show_toast("メッセージIDをコピーしました");
                cx.notify();
            })),
        );
        // ピン留めできるのは通常のメッセージ・返信・コマンドの結果だけ
        let pinnable = !deleted && matches!(m.kind, 0 | 19 | 20 | 23);
        if pinnable
            && self
                .app
                .can_pin(self.app.sel_guild.as_deref(), &m.channel_id)
        {
            let (m1, pinned) = (m.clone(), m.pinned);
            let (ic, label) = if pinned {
                (IconName::PinOff, "ピン留めを外す")
            } else {
                (IconName::Pin, "メッセージをピン留め")
            };
            col = col.child(
                menu_row(
                    "mm_pin",
                    icon(ic, 18.0, TEXT()).into_any_element(),
                    label,
                    false,
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    close(this);
                    this.app.set_pinned(&m1, !pinned);
                    cx.notify();
                })),
            );
        }
        if !deleted && self.app.can_create_thread(&m) {
            let m1 = m.clone();
            col = col.child(
                menu_row(
                    "mm_thread",
                    icon(IconName::MessagesSquare, 18.0, TEXT()).into_any_element(),
                    "スレッドを作成",
                    false,
                )
                .on_click(cx.listener(move |this, _, window, cx| {
                    close(this);
                    this.app.start_thread_draft(m1.clone());
                    this.inp.thread_name.update(cx, |s, cx| s.focus(window, cx));
                    cx.notify();
                })),
            );
        }
        if mine {
            col = col.child(menu_divider());
            let m1 = m.clone();
            col = col.child(
                menu_row(
                    "mm_delete",
                    icon(IconName::Trash, 18.0, RED()).into_any_element(),
                    "メッセージを削除",
                    false,
                )
                .text_color(RED())
                .hover(|s| s.bg(RED()).text_color(WHITE()))
                .on_click(cx.listener(move |this, _, _, cx| {
                    close(this);
                    this.app.pending_delete = Some(m1.clone());
                    cx.notify();
                })),
            );
        }
        Some(col)
    }

    /// サーバーアイコンの右クリックメニュー
    fn guild_menu(&mut self, cx: &mut Context<Self>) -> Option<Div> {
        let gid = self.menu_guild.clone()?;
        let g = self.app.guilds.iter().find(|g| g.id == gid)?;
        let owner = g.owner;
        let unread = self
            .app
            .unread
            .values()
            .any(|x| x.as_deref() == Some(gid.as_str()));
        let set = self
            .app
            .guild_settings
            .get(&gid)
            .cloned()
            .unwrap_or_default();
        let close = |this: &mut Root| {
            this.menu = None;
            this.menu_guild = None;
        };
        let mut col = popup_box()
            .p(z(6.0))
            .w(z(240.0))
            .flex()
            .flex_col()
            .gap(z(2.0));

        let g1 = gid.clone();
        col = col.child(
            menu_row(
                "gm_read",
                icon(IconName::CheckCheck, 18.0, TEXT()).into_any_element(),
                "既読にする",
                false,
            )
            .when(!unread, |d| d.opacity(0.5))
            .on_click(cx.listener(move |this, _, _, cx| {
                close(this);
                this.app.mark_guild_read(&g1);
                cx.notify();
            })),
        );
        let g1 = gid.clone();
        col = col.child(
            menu_row(
                "gm_invite",
                icon(IconName::UserPlus, 18.0, TEXT()).into_any_element(),
                "サーバーに招待",
                false,
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                close(this);
                this.app.create_guild_invite(&g1);
                cx.notify();
            })),
        );
        col = col.child(menu_divider());

        let g1 = gid.clone();
        let (mute_icon, mute_label) = if set.muted {
            (IconName::Bell, "サーバーのミュートを解除")
        } else {
            (IconName::BellOff, "サーバーをミュート")
        };
        col = col.child(
            menu_row(
                "gm_mute",
                icon(mute_icon, 18.0, TEXT()).into_any_element(),
                mute_label,
                false,
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                let muted = this.app.guild_settings.get(&g1).is_some_and(|s| s.muted);
                this.app.set_guild_settings(&g1, Some(!muted), None);
                cx.notify();
            })),
        );
        col = col.child(div().px(z(10.0)).pt(z(6.0)).pb(z(2.0)).child(text(
            "通知設定",
            12.0,
            MUTED(),
        )));
        for (n, label) in [
            (3u8, "サーバーの既定"),
            (0, "すべてのメッセージ"),
            (1, "@メンションのみ"),
            (2, "なし"),
        ] {
            let g1 = gid.clone();
            col = col.child(
                div()
                    .id(("gm_notify", n as usize))
                    .h(z(32.0))
                    .px(z(10.0))
                    .rounded(z(6.0))
                    .flex()
                    .items_center()
                    .gap(z(10.0))
                    .cursor_pointer()
                    .text_color(TEXT())
                    .hover(|s| s.bg(HOVER()).text_color(WHITE()))
                    .child(
                        div()
                            .w(z(20.0))
                            .flex()
                            .justify_center()
                            .child(radio(set.notifications == n)),
                    )
                    .child(div().flex_1().text_size(z(14.0)).child(label))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.app.set_guild_settings(&g1, None, Some(n));
                        cx.notify();
                    })),
            );
        }
        if !owner {
            col = col.child(menu_divider());
            let g1 = gid.clone();
            col = col.child(
                menu_row(
                    "gm_leave",
                    icon(IconName::LogOut, 18.0, RED()).into_any_element(),
                    "サーバーから脱退",
                    false,
                )
                .text_color(RED())
                .hover(|s| s.bg(RED()).text_color(WHITE()))
                .on_click(cx.listener(move |this, _, _, cx| {
                    close(this);
                    this.app.confirm_leave = Some(g1.clone());
                    cx.notify();
                })),
            );
        }
        col = col.child(menu_divider());
        let g1 = gid.clone();
        col = col.child(
            menu_row(
                "gm_id",
                icon(IconName::IdCard, 18.0, TEXT()).into_any_element(),
                "サーバーIDをコピー",
                false,
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                close(this);
                cx.write_to_clipboard(ClipboardItem::new_string(g1.clone()));
                this.app.show_toast("サーバーIDをコピーしました");
                cx.notify();
            })),
        );
        Some(col)
    }

    /// 左下のマイク／ヘッドホン横の ⌄ メニュー
    fn audio_menu(&mut self, input: bool, window: &mut Window, cx: &mut Context<Self>) -> Div {
        if self.app.audio_devs.is_none() {
            self.app.audio_devs = Some((voice::input_devices(), voice::output_devices()));
        }
        let (ins, outs) = self.app.audio_devs.clone().unwrap_or_default();
        let list = if input { ins } else { outs };
        let cur = if input {
            self.app.settings.input_device.clone()
        } else {
            self.app.settings.output_device.clone()
        };
        let mut devs = div()
            .id("devs")
            .max_h(z(220.0))
            .overflow_y_scroll()
            .flex()
            .flex_col();
        for (i, d) in std::iter::once(None)
            .chain(list.into_iter().map(Some))
            .enumerate()
        {
            let on = cur == d;
            let label = d.clone().unwrap_or_else(|| "デフォルト".into());
            devs = devs.child(
                div()
                    .id(("dev", i))
                    .h(z(32.0))
                    .px(z(8.0))
                    .rounded(z(6.0))
                    .flex()
                    .items_center()
                    .gap(z(8.0))
                    .cursor_pointer()
                    .hover(|s| s.bg(HOVER()))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        let s = &mut this.app.settings;
                        if input {
                            s.input_device = d.clone();
                        } else {
                            s.output_device = d.clone();
                        }
                        this.app.apply_voice_settings();
                        this.app.save_settings();
                        cx.notify();
                    }))
                    .child(radio(on))
                    .child(line(label, 14.0, TEXT()).flex_1()),
            );
        }
        let mut col = popup_box()
            .p(z(10.0))
            .w(z(260.0))
            .flex()
            .flex_col()
            .gap(z(6.0))
            .child(text(
                if input {
                    "入力デバイス"
                } else {
                    "出力デバイス"
                },
                12.0,
                MUTED(),
            ))
            .child(devs)
            .child(menu_divider());
        if input {
            col = col.child(text("入力モード", 12.0, MUTED()));
            for (ptt, label) in [(false, "音声検出"), (true, "プッシュトゥトーク")] {
                let on = self.app.settings.ptt == ptt;
                col = col.child(
                    div()
                        .id(SharedString::from(format!("mode{ptt}")))
                        .h(z(30.0))
                        .px(z(8.0))
                        .rounded(z(6.0))
                        .flex()
                        .items_center()
                        .gap(z(8.0))
                        .cursor_pointer()
                        .hover(|s| s.bg(HOVER()))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.app.settings.ptt = ptt;
                            this.app.apply_voice_settings();
                            this.app.save_settings();
                            cx.notify();
                        }))
                        .child(radio(on))
                        .child(text(label, 14.0, TEXT())),
                );
            }
            col = col.child(self.slider_row("in_gain", "入力音量", window, cx));
        } else {
            col = col.child(self.slider_row("out_gain", "出力音量", window, cx));
        }
        col.child(menu_divider())
            .child(
                small_button("voice_settings", "音声設定…").on_click(cx.listener(
                    |this, _, _, cx| {
                        this.app.settings_open = true;
                        this.app.settings_tab = 2;
                        this.menu = None;
                        cx.notify();
                    },
                )),
            )
    }

    /// プロフィールのポップアップ（アバターや名前を押したとき）
    pub fn profile_popup(
        &mut self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let pp = self.app.popup.clone()?;
        let user = pp.user.clone();
        let key = net::profile_key(&user.id, pp.guild_id.as_deref());
        let failed = matches!(self.app.profiles.get(&key), Some(ProfileState::Failed));
        let loading = matches!(
            self.app.profiles.get(&key),
            Some(ProfileState::Loading) | None
        );
        let data: Option<ProfileData> = match self.app.profiles.get(&key) {
            Some(ProfileState::Ready(d)) => Some(d.clone()),
            _ => None,
        };
        let is_me = self.app.me.as_ref().is_some_and(|m| m.id == user.id);
        let (status, activity) = if is_me {
            (self.app.settings.status.clone(), None)
        } else {
            match self.app.presences.get(&user.id) {
                Some(p) => (p.status.clone(), p.activity.clone()),
                None => ("offline".to_string(), None),
            }
        };
        let dm = self
            .app
            .dms
            .iter()
            .find(|c| c.recipients.len() == 1 && c.recipients[0].id == user.id)
            .map(|c| c.id.clone());
        let width = (self.app.screen_w - 16.0).clamp(0.0, 300.0);
        let max_h = (self.pane_h - 16.0).max(0.0);

        // バナー（画像 → 指定色 → 名前から決めた色の順）
        let color = data
            .as_ref()
            .and_then(|d| d.accent_color)
            .map(rgb)
            .unwrap_or_else(|| darken(name_color(&user.id)));
        let radius = part_style(Part::ProfilePopup).radius.unwrap_or(8.0);
        let ring = part_style(Part::ProfileAvatar).bg.unwrap_or(SIDEBAR());
        let mut banner = div()
            .h(z(120.0))
            .w_full()
            .rounded_t(z(radius))
            .bg(color)
            .overflow_hidden();
        if let Some(h) = data.as_ref().and_then(|d| d.banner.as_ref()) {
            if let Some(i) = loaded(&format!(
                "https://cdn.discordapp.com/banners/{}/{}.png?size=600",
                user.id, h
            )) {
                banner = banner.child(img(i).size_full().object_fit(ObjectFit::Cover));
            }
        }
        let av = div()
            .absolute()
            .left(z(10.0))
            .top(z(120.0 - 50.0))
            .size(z(92.0))
            .rounded_full()
            .bg(ring)
            .flex()
            .items_center()
            .justify_center()
            .child(with_status(
                avatar(
                    Some(&user_avatar_url(&user)),
                    80.0,
                    user.display(),
                    40.0,
                    BLURPLE(),
                ),
                80.0,
                &status,
                ring,
            ));
        let name = data
            .as_ref()
            .and_then(|d| d.nick.clone())
            .unwrap_or_else(|| user.display().to_string());
        let mut sub = user.username.clone();
        if let Some(pr) = data.as_ref().and_then(|d| d.pronouns.as_deref()) {
            sub += &format!(" • {pr}");
        }
        let mut info = div()
            .px(z(16.0))
            .pt(z(52.0))
            .pb(z(16.0))
            .flex()
            .flex_col()
            .gap(z(4.0))
            .child(line(name, 20.0, WHITE()).part(Part::ProfileName))
            .child(line(sub, 14.0, TEXT()).part(Part::ProfileText));
        if !is_me {
            info = info.child(text(status_label(&status), 12.0, MUTED()).part(Part::ProfileMuted));
        }
        if let Some(act) = &activity {
            info = info.child(
                div()
                    .mt(z(4.0))
                    .bg(RAIL())
                    .rounded(z(8.0))
                    .part(Part::ProfileBox)
                    .px(z(10.0))
                    .py(z(6.0))
                    .child(text(act.clone(), 13.0, TEXT()).part(Part::ProfileText)),
            );
        }
        let retry = {
            let pp = pp.clone();
            let key = key.clone();
            cx.listener(move |this, _, _, cx| {
                let cmd = Cmd::LoadProfile {
                    user_id: pp.user.id.clone(),
                    guild_id: pp.guild_id.clone(),
                };
                if this
                    .app
                    .backend
                    .as_ref()
                    .is_some_and(|b| b.cmd.send(cmd).is_ok())
                {
                    this.app.profiles.insert(key.clone(), ProfileState::Loading);
                } else {
                    this.app.status =
                        "エラー: 接続できません。再接続してからお試しください。".into();
                }
                cx.notify();
            })
        };
        if is_me {
            // 自分のときは本家と同じく、自己紹介を名前の下にそのまま出してメニューを並べる
            if let Some(bio) = data.as_ref().and_then(|d| d.bio.clone()) {
                info = info.child(self.bio_text(&bio, 14.0).mt(z(8.0)).part(Part::ProfileText));
            }
            if failed {
                info = info.child(
                    div()
                        .flex()
                        .child(small_button("retry_profile", "再読み込み").on_click(retry)),
                );
            }
            let uid = user.id.clone();
            let mut menu = div()
                .mt(z(10.0))
                .bg(RAIL())
                .rounded(z(8.0))
                .part(Part::ProfileBox)
                .p(z(6.0))
                .flex()
                .flex_col();
            menu = menu.child(
                menu_row(
                    "edit_profile",
                    icon(IconName::Pencil, 18.0, TEXT()).into_any_element(),
                    "プロフィールを編集",
                    false,
                )
                .on_hover(cx.listener(|this, h: &bool, _, cx| {
                    if *h && this.status_sub {
                        this.status_sub = false;
                        cx.notify();
                    }
                }))
                .on_click(cx.listener(|this, _, _, cx| {
                    this.app.popup = None;
                    this.app.settings_open = true;
                    this.app.settings_tab = 4;
                    cx.notify();
                })),
            );
            menu = menu.child(menu_divider());
            // ステータスのサブメニュー（「ステータス」の行に乗せると右に出る）
            let current = self.app.settings.status.clone();
            let mut status_row = div().relative().child(
                menu_row(
                    "status_row",
                    status_mark(&status, 5.0, RAIL()).into_any_element(),
                    status_label(&status),
                    true,
                )
                .on_hover(cx.listener(|this, h: &bool, _, cx| {
                    if *h && !this.status_sub {
                        this.status_sub = true;
                        cx.notify();
                    }
                }))
                .on_click(cx.listener(|this, _, _, cx| {
                    this.status_sub = !this.status_sub;
                    cx.notify();
                })),
            );
            if self.status_sub {
                let mut subm = popup_box().p(z(6.0)).w(z(220.0)).flex().flex_col();
                for (k, label) in STATUSES {
                    let on = k == current;
                    subm = subm.child(
                        menu_row(
                            SharedString::from(format!("st_{k}")),
                            status_mark(k, 5.0, SIDEBAR()).into_any_element(),
                            label,
                            false,
                        )
                        .when(on, |d| {
                            d.child(
                                div()
                                    .size(z(12.0))
                                    .rounded_full()
                                    .bg(BLURPLE())
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .child(div().size(z(5.0)).rounded_full().bg(Color32::WHITE)),
                            )
                        })
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.app.set_status(k.to_string());
                            this.app.popup = None;
                            this.status_sub = false;
                            cx.notify();
                        })),
                    );
                }
                status_row = status_row.child(
                    div()
                        .absolute()
                        .left(z(width - 20.0))
                        .top(z(-6.0))
                        .child(subm),
                );
            }
            menu = menu.child(status_row).child(menu_divider());
            menu = menu.child(
                menu_row(
                    "copy_id",
                    div()
                        .w(z(18.0))
                        .h(z(14.0))
                        .rounded(z(3.0))
                        .bg(TEXT())
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_size(z(9.0))
                        .text_color(RAIL())
                        .child("ID")
                        .into_any_element(),
                    "ユーザーIDをコピー",
                    false,
                )
                .on_hover(cx.listener(|this, h: &bool, _, cx| {
                    if *h && this.status_sub {
                        this.status_sub = false;
                        cx.notify();
                    }
                }))
                .on_click(cx.listener(move |this, _, _, cx| {
                    cx.write_to_clipboard(ClipboardItem::new_string(uid.clone()));
                    this.app.popup = None;
                    this.app.show_toast("ユーザーIDをコピーしました");
                    cx.notify();
                })),
            );
            info = info.child(menu);
        } else {
            let mut about = div()
                .mt(z(6.0))
                .bg(RAIL())
                .rounded(z(8.0))
                .part(Part::ProfileBox)
                .p(z(12.0))
                .flex()
                .flex_col()
                .gap(z(4.0));
            if loading {
                about = about.child(div().flex().justify_center().child(spinner(20.0)));
            } else if failed {
                about = about
                    .child(
                        text("詳しいプロフィールを取得できませんでした", 13.0, MUTED())
                            .part(Part::ProfileMuted),
                    )
                    .child(
                        div()
                            .flex()
                            .child(small_button("retry_profile", "再読み込み").on_click(retry)),
                    );
            }
            let heading = |t: &'static str| {
                text(t, 12.0, WHITE())
                    .font_weight(FontWeight::MEDIUM)
                    .part(Part::ProfileMuted)
            };
            if let Some(bio) = data.as_ref().and_then(|d| d.bio.clone()) {
                about = about
                    .child(heading("自己紹介"))
                    .child(self.bio_text(&bio, 14.0).mb(z(8.0)).part(Part::ProfileText));
            }
            about = about
                .child(heading("Discordメンバー歴"))
                .child(text(snowflake_date(&user.id), 14.0, TEXT()).part(Part::ProfileText));
            if let Some(j) = data
                .as_ref()
                .and_then(|d| d.joined_at.as_deref())
                .and_then(local_dt)
            {
                about = about.child(heading("サーバーメンバー歴").mt(z(8.0))).child(
                    text(j.format("%Y年%-m月%-d日").to_string(), 14.0, TEXT())
                        .part(Part::ProfileText),
                );
            }
            if let Some(roles) = data
                .as_ref()
                .map(|d| d.roles.clone())
                .filter(|r| !r.is_empty())
            {
                let mut wrap = div().flex().flex_wrap().gap(z(4.0));
                for (name, c) in roles {
                    let col = if c == 0 { ST_OFFLINE() } else { rgb(c) };
                    wrap = wrap.child(
                        div()
                            .bg(SELECTED())
                            .rounded(z(4.0))
                            .px(z(6.0))
                            .py(z(3.0))
                            .flex()
                            .items_center()
                            .gap(z(5.0))
                            .max_w(z(width - 60.0))
                            .child(div().size(z(10.0)).flex_none().rounded_full().bg(col))
                            .child(line(name, 12.0, TEXT())),
                    );
                }
                about = about.child(heading("ロール").mt(z(8.0))).child(wrap);
            }
            info = info.child(about);
            if let Some(cid) = dm {
                info = info.child(
                    div().mt(z(10.0)).child(
                        fill_button_part(
                            "send_dm",
                            "メッセージを送る",
                            BLURPLE(),
                            Color32::WHITE,
                            true,
                            Some(Part::ProfileButton),
                        )
                        .w_full()
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.app.popup = None;
                            let before = (this.app.sel_guild.clone(), this.app.sel_channel.clone());
                            this.app.sel_guild = None;
                            this.app.open_channel(cid.clone());
                            this.after_move(before);
                            cx.notify();
                        })),
                    ),
                );
            }
        }
        let body = popup_box()
            .part(Part::ProfilePopup)
            .w(z(width))
            .max_h(z(max_h))
            .flex()
            .flex_col()
            .child(div().relative().flex_none().child(banner).child(av))
            .child(
                div()
                    .id("profile_scroll")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .child(info),
            );
        let close = cx.listener(|this, _: &MouseDownEvent, _, cx| {
            this.app.popup = None;
            this.status_sub = false;
            cx.notify();
        });
        let (pos, corner) = if pp.above {
            (
                point(pp.anchor.origin.x, pp.anchor.origin.y - z(10.0)),
                Anchor::BottomLeft,
            )
        } else {
            (
                point(pp.anchor.origin.x + z(12.0), pp.anchor.origin.y),
                Anchor::TopLeft,
            )
        };
        Some(
            deferred(
                anchored()
                    .position(pos)
                    .anchor(corner)
                    .snap_to_window_with_margin(z(8.0))
                    .child(div().occlude().on_mouse_down_out(close).child(body)),
            )
            .with_priority(2)
            .into_any_element(),
        )
    }
}
