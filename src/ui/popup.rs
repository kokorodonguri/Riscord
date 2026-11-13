//! 小さなメニュー（＋・マイク/ヘッドホンの ⌄）とプロフィールのポップアップ
use gpui::assets::IconName;
use gpui::prelude::*;
use gpui::*;

use super::*;
use crate::app::{
    darken, decoration_for, local_dt, member_avatar_url, name_color, rgb, snowflake_date,
    status_label, user_avatar_url, MemberAction, ProfileState, STATUSES,
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
                        tr!("ファイルをアップロード", "Upload a file"),
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
                        tr!("画像を貼り付け", "Paste image"),
                        false,
                    )
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.menu = None;
                        if !this.app.paste_image() {
                            this.app.status = tr!(
                                "クリップボードに画像がありません",
                                "No image on the clipboard"
                            )
                            .into();
                        }
                        cx.notify();
                    })),
                );
                col = col.child(
                    menu_row(
                        "plus_poll",
                        icon(IconName::MessagesSquare, 20.0, TEXT()).into_any_element(),
                        tr!("投票を作成", "Create Poll"),
                        false,
                    )
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.menu = None;
                        if let Some(draft) = this.app.poll_draft.as_ref() {
                            if Some(&draft.channel_id) != this.app.sel_channel.as_ref() {
                                this.app.show_toast(tr!("作成中の投票があります。元のチャンネルで送信またはキャンセルしてください", "A poll is in progress. Send or cancel it in the original channel"));
                            }
                        } else if let Some(channel_id) = this.app.sel_channel.clone() {
                            this.app.poll_draft = Some(crate::app::PollDraft {
                                channel_id,
                                duration: 24,
                                ..Default::default()
                            });
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
            PopMenu::Folder => self.folder_menu(cx)?.into_any_element(),
            PopMenu::Message => self.message_menu(cx)?.into_any_element(),
            PopMenu::Emoji => self.emoji_picker(cx).into_any_element(),
        };
        // 右クリックのメニューはカーソルの右下に、ボタンのメニューはボタンの上に出す
        let (at, anchor) = if menu == PopMenu::Guild && self.menu_from_header {
            // サーバー名の見出しから開いたときは、見出しの真下に出す
            (pos, Anchor::TopLeft)
        } else if matches!(menu, PopMenu::Guild | PopMenu::Folder | PopMenu::Message) {
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
                    tr!("返信", "Reply"),
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
                    tr!("編集", "Edit"),
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
                    tr!("テキストをコピー", "Copy Text"),
                    false,
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    close(this);
                    cx.write_to_clipboard(ClipboardItem::new_string(t.clone()));
                    this.app
                        .show_toast(tr!("テキストをコピーしました", "Text copied"));
                    cx.notify();
                })),
            );
        }
        let id = m.id.clone();
        col = col.child(
            menu_row(
                "mm_copy_id",
                icon(IconName::Copy, 18.0, TEXT()).into_any_element(),
                tr!("メッセージIDをコピー", "Copy Message ID"),
                false,
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                close(this);
                cx.write_to_clipboard(ClipboardItem::new_string(id.clone()));
                this.app
                    .show_toast(tr!("メッセージIDをコピーしました", "Message ID copied"));
                cx.notify();
            })),
        );
        if mine
            && m.poll.as_ref().is_some_and(|p| {
                !p.results.as_ref().is_some_and(|r| r.is_finalized)
                    && !p
                        .expiry
                        .as_deref()
                        .and_then(|v| chrono::DateTime::parse_from_rfc3339(v).ok())
                        .is_some_and(|end| end <= chrono::Utc::now())
            })
        {
            let (cid, mid) = (m.channel_id.clone(), m.id.clone());
            col = col.child(
                menu_row(
                    "mm_end_poll",
                    icon(IconName::Pin, 18.0, TEXT()).into_any_element(),
                    tr!("投票を終了", "End Poll"),
                    false,
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    close(this);
                    this.app.end_poll(&cid, &mid);
                    cx.notify();
                })),
            );
        }
        // ピン留めできるのは通常のメッセージ・返信・コマンドの結果だけ
        let pinnable = !deleted && matches!(m.kind, 0 | 19 | 20 | 23);
        if pinnable
            && self
                .app
                .can_pin(self.app.sel_guild.as_deref(), &m.channel_id)
        {
            let (m1, pinned) = (m.clone(), m.pinned);
            let (ic, label) = if pinned {
                (IconName::PinOff, tr!("ピン留めを外す", "Unpin Message"))
            } else {
                (IconName::Pin, tr!("メッセージをピン留め", "Pin Message"))
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
                    tr!("スレッドを作成", "Create Thread"),
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
                    tr!("メッセージを削除", "Delete Message"),
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

    fn folder_menu(&mut self, cx: &mut Context<Self>) -> Option<Div> {
        let id = self.menu_folder.clone()?;
        let folder = self
            .app
            .settings
            .guild_folders
            .iter()
            .find(|f| f.id == id)?;
        let name = folder.name.clone();
        let color = folder.color;
        let edit_id = id.clone();
        let delete_id = id;
        Some(
            popup_box()
                .p(z(6.0))
                .w(z(220.0))
                .flex()
                .flex_col()
                .gap(z(2.0))
                .child(
                    menu_row(
                        "folder_edit",
                        icon(IconName::Settings, 18.0, TEXT()).into_any_element(),
                        tr!("フォルダを編集", "Edit Folder"),
                        false,
                    )
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.menu = None;
                        this.menu_folder = None;
                        this.app.folder_name = name.clone();
                        this.app.folder_color = color;
                        this.app.folder_edit = Some(edit_id.clone());
                        this.inp.folder_name.update(cx, |s, cx| {
                            s.set_value(name.clone(), window, cx);
                            s.focus(window, cx);
                        });
                        cx.notify();
                    })),
                )
                .child(
                    menu_row(
                        "folder_delete",
                        icon(IconName::X, 18.0, TEXT()).into_any_element(),
                        tr!("フォルダを削除", "Delete Folder"),
                        false,
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.menu = None;
                        this.menu_folder = None;
                        this.app
                            .settings
                            .guild_folders
                            .retain(|f| f.id != delete_id);
                        this.app.save_settings();
                        cx.notify();
                    })),
                ),
        )
    }

    /// サーバーのメニュー（サーバー名の見出し・アイコンの右クリック）。並びは公式クライアントに合わせる
    fn guild_menu(&mut self, cx: &mut Context<Self>) -> Option<Div> {
        let gid = self.menu_guild.clone()?;
        let g = self.app.guilds.iter().find(|g| g.id == gid)?;
        let owner = g.owner;
        let from_header = self.menu_from_header;
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
        let item = |id: &'static str, name: IconName, label: &'static str| {
            let _ = name;
            div()
                .id(id)
                .h(z(40.0))
                .px(z(10.0))
                .rounded(z(4.0))
                .flex()
                .items_center()
                .cursor_pointer()
                .text_color(TEXT())
                .hover(|s| s.bg(part_hover(Part::MenuRow, HOVER())).text_color(WHITE()))
                .part(Part::MenuRow)
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_size(z(15.0))
                        .truncate()
                        .child(label),
                )
        };

        // 画像のメニューに合わせ、頻繁に使う操作から順に並べる。
        let g1 = gid.clone();
        col = col.child(
            item(
                "gm_read",
                IconName::CheckCheck,
                tr!("既読にする", "Mark as Read"),
            )
            .when(!unread, |d| d.opacity(0.5))
            .on_click(cx.listener(move |this, _, _, cx| {
                close(this);
                this.app.mark_guild_read(&g1);
                cx.notify();
            })),
        );
        col = col.child(menu_divider());
        let g1 = gid.clone();
        col = col.child(
            item(
                "gm_invite",
                IconName::UserPlus,
                tr!("サーバーに招待", "Invite to Server"),
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                close(this);
                this.app.create_guild_invite(&g1);
                cx.notify();
            })),
        );
        col = col.child(menu_divider());
        let g1 = gid.clone();
        col = col.child(
            item(
                "gm_mute",
                IconName::BellOff,
                if set.muted {
                    tr!("サーバーのミュートを解除", "Unmute Server")
                } else {
                    tr!("サーバーをミュート", "Mute Server")
                },
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                let muted = this.app.guild_settings.get(&g1).is_some_and(|s| s.muted);
                this.app.set_guild_settings(&g1, Some(!muted), None);
                cx.notify();
            })),
        );
        let notification_label = match set.notifications {
            0 => tr!("すべてのメッセージ", "All Messages"),
            1 => tr!("@メンションのみ", "Only @mentions"),
            2 => tr!("なし", "None"),
            _ => tr!("サーバーの既定", "Server default"),
        };
        col = col.child(
            div()
                .id("gm_notifications")
                .px(z(10.0))
                .py(z(6.0))
                .rounded(z(4.0))
                .cursor_pointer()
                .hover(|s| s.bg(HOVER()))
                .flex()
                .items_center()
                .child(
                    div()
                        .flex_1()
                        .flex()
                        .flex_col()
                        .child(text(tr!("通知設定", "Notification Settings"), 15.0, TEXT()))
                        .child(text(notification_label, 12.0, MUTED())),
                )
                .child(icon(IconName::ChevronRight, 16.0, MUTED()))
                .on_click(cx.listener(|this, _, _, cx| {
                    this.guild_notify_expanded = !this.guild_notify_expanded;
                    cx.notify();
                })),
        );
        if self.guild_notify_expanded {
            for (n, label) in [
                (3u8, tr!("サーバーの既定", "Server default")),
                (0, tr!("すべてのメッセージ", "All Messages")),
                (1, tr!("@メンションのみ", "Only @mentions")),
                (2, tr!("なし", "None")),
            ] {
                let g1 = gid.clone();
                col = col.child(
                    div()
                        .id(("gm_notify", n as usize))
                        .h(z(32.0))
                        .px(z(10.0))
                        .rounded(z(4.0))
                        .flex()
                        .items_center()
                        .gap(z(10.0))
                        .cursor_pointer()
                        .hover(|s| s.bg(HOVER()))
                        .child(radio(set.notifications == n))
                        .child(text(label, 14.0, TEXT()))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.app.set_guild_settings(&g1, None, Some(n));
                            this.guild_notify_expanded = false;
                            cx.notify();
                        })),
                );
            }
        }
        let g1 = gid.clone();
        let hide = self.app.settings.hide_muted_guilds.contains(&gid);
        col = col.child(
            div()
                .id("gm_hide_muted")
                .h(z(40.0))
                .px(z(10.0))
                .rounded(z(4.0))
                .flex()
                .items_center()
                .gap(z(8.0))
                .cursor_pointer()
                .hover(|s| s.bg(HOVER()))
                .child(
                    div()
                        .flex_1()
                        .text_size(z(14.0))
                        .text_color(TEXT())
                        .child(tr!("ミュートしたチャンネルを隠す", "Hide Muted Channels")),
                )
                .child(
                    div()
                        .size(z(20.0))
                        .rounded(z(4.0))
                        .border_1()
                        .border_color(if hide { BLURPLE() } else { MUTED() })
                        .when(hide, |d| {
                            d.bg(BLURPLE()).child(icon(IconName::Check, 16.0, WHITE()))
                        }),
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.app.toggle_hide_muted(&g1);
                    cx.notify();
                })),
        );
        col = col.child(menu_divider());
        let g1 = gid.clone();
        col = col.child(
            item(
                "gm_settings",
                IconName::Settings,
                tr!("サーバー設定", "Server Settings"),
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                close(this);
                this.app.open_server_settings(&g1);
                cx.notify();
            })),
        );
        if self.app.can_create_channels(&gid) {
            for (id, name, label, category) in [
                (
                    "gm_new_channel",
                    IconName::CirclePlus,
                    tr!("チャンネルを作成", "Create Channel"),
                    false,
                ),
                (
                    "gm_new_category",
                    IconName::Folder,
                    tr!("カテゴリを作成", "Create Category"),
                    true,
                ),
            ] {
                let g1 = gid.clone();
                col = col.child(item(id, name, label).on_click(cx.listener(
                    move |this, _, _, cx| {
                        close(this);
                        this.app.open_channel_create(&g1, category);
                        cx.notify();
                    },
                )));
            }
        }
        if self.app.can_open_mod_tab(&gid, crate::app::ModTab::Events) {
            let g1 = gid.clone();
            col = col.child(
                item(
                    "gm_event",
                    IconName::CirclePlus,
                    tr!("イベントを作成", "Create Event"),
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    close(this);
                    this.app.open_event_create(&g1);
                    cx.notify();
                })),
            );
        }
        let g1 = gid.clone();
        col = col.child(
            item(
                "gm_profile",
                IconName::Pencil,
                tr!("サーバープロフィールを編集", "Edit Server Profile"),
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                close(this);
                this.app.open_guild_profile(&g1);
                cx.notify();
            })),
        );
        if !from_header {
            col = col.child(menu_divider());
            let g1 = gid.clone();
            let in_folder = self
                .app
                .settings
                .guild_folders
                .iter()
                .any(|f| f.guild_ids.contains(&gid));
            col = col.child(
                item(
                    "gm_folder",
                    IconName::Folder,
                    if in_folder {
                        tr!("フォルダから移動", "Remove from Folder")
                    } else {
                        tr!("フォルダを作成", "Create Folder")
                    },
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    close(this);
                    let changed = if in_folder {
                        this.app.remove_guild_from_folder(&g1)
                    } else {
                        this.app.create_guild_folder(&g1).is_some()
                    };
                    if changed {
                        this.app.save_settings();
                    }
                    cx.notify();
                })),
            );
        }
        if !owner {
            col = col.child(menu_divider());
            let g1 = gid.clone();
            col = col.child(
                item(
                    "gm_leave",
                    IconName::LogOut,
                    tr!("サーバーから脱退", "Leave Server"),
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
            item(
                "gm_id",
                IconName::IdCard,
                tr!("サーバーIDをコピー", "Copy Server ID"),
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                close(this);
                cx.write_to_clipboard(ClipboardItem::new_string(g1.clone()));
                this.app
                    .show_toast(tr!("サーバーIDをコピーしました", "Server ID copied"));
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
            let label = d
                .clone()
                .unwrap_or_else(|| tr!("デフォルト", "Default").into());
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
                    tr!("入力デバイス", "Input Device")
                } else {
                    tr!("出力デバイス", "Output Device")
                },
                12.0,
                MUTED(),
            ))
            .child(devs)
            .child(menu_divider());
        if input {
            col = col.child(text(tr!("入力モード", "Input Mode"), 12.0, MUTED()));
            for (ptt, label) in [
                (false, tr!("音声検出", "Voice Activity")),
                (true, tr!("プッシュトゥトーク", "Push to Talk")),
            ] {
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
            col =
                col.child(self.slider_row("in_gain", tr!("入力音量", "Input Volume"), window, cx));
        } else {
            col = col.child(self.slider_row(
                "out_gain",
                tr!("出力音量", "Output Volume"),
                window,
                cx,
            ));
        }
        col.child(menu_divider()).child(
            small_button("voice_settings", tr!("音声設定…", "Voice Settings…")).on_click(
                cx.listener(|this, _, _, cx| {
                    this.app.settings_open = true;
                    this.app.settings_tab = 2;
                    this.menu = None;
                    cx.notify();
                }),
            ),
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
            .child({
                // サーバーで開いたときは、サーバー別のアバターとデコレーションを使う
                let member = pp.guild_id.as_deref().and_then(|g| {
                    self.app
                        .members
                        .get(g)?
                        .iter()
                        .find(|m| m.user.as_ref().is_some_and(|u| u.id == user.id))
                });
                let av_url = match pp.guild_id.as_deref() {
                    Some(g) => member_avatar_url(g, &user, member),
                    None => user_avatar_url(&user),
                };
                with_status(
                    with_decoration(
                        avatar(Some(&av_url), 80.0, user.display(), 40.0, BLURPLE()),
                        80.0,
                        decoration_for(&user, member).as_deref(),
                    ),
                    80.0,
                    &status,
                    ring,
                )
            });
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
                    this.app.status = tr!(
                        "エラー: 接続できません。再接続してからお試しください。",
                        "Error: Not connected. Reconnect and try again."
                    )
                    .into();
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
                info = info.child(div().flex().child(
                    small_button("retry_profile", tr!("再読み込み", "Reload")).on_click(retry),
                ));
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
                    tr!("プロフィールを編集", "Edit Profile"),
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
                            label.s(),
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
                    tr!("ユーザーIDをコピー", "Copy User ID"),
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
                    this.app
                        .show_toast(tr!("ユーザーIDをコピーしました", "User ID copied"));
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
                        text(
                            tr!(
                                "詳しいプロフィールを取得できませんでした",
                                "Couldn't load the full profile"
                            ),
                            13.0,
                            MUTED(),
                        )
                        .part(Part::ProfileMuted),
                    )
                    .child(div().flex().child(
                        small_button("retry_profile", tr!("再読み込み", "Reload")).on_click(retry),
                    ));
            }
            let heading = |t: &'static str| {
                text(t, 12.0, WHITE())
                    .font_weight(FontWeight::MEDIUM)
                    .part(Part::ProfileMuted)
            };
            if let Some(bio) = data.as_ref().and_then(|d| d.bio.clone()) {
                about = about
                    .child(heading(tr!("自己紹介", "About Me")))
                    .child(self.bio_text(&bio, 14.0).mb(z(8.0)).part(Part::ProfileText));
            }
            about = about
                .child(heading(tr!("Discordメンバー歴", "Discord Member Since")))
                .child(text(snowflake_date(&user.id), 14.0, TEXT()).part(Part::ProfileText));
            if let Some(j) = data
                .as_ref()
                .and_then(|d| d.joined_at.as_deref())
                .and_then(local_dt)
            {
                about = about
                    .child(heading(tr!("サーバーメンバー歴", "Server Member Since")).mt(z(8.0)))
                    .child(
                        text(
                            j.format(tr!("%Y年%-m月%-d日", "%b %-d, %Y")).to_string(),
                            14.0,
                            TEXT(),
                        )
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
                about = about
                    .child(heading(tr!("ロール", "Roles")).mt(z(8.0)))
                    .child(wrap);
            }
            info = info.child(about);
            if let Some(cid) = dm.clone() {
                info = info.child(
                    div().mt(z(10.0)).child(
                        fill_button_part(
                            "send_dm",
                            tr!("メッセージを送る", "Send Message"),
                            BLURPLE(),
                            Color32::WHITE,
                            true,
                            Some(Part::ProfileButton),
                        )
                        .w_full()
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.app.popup = None;
                            this.app.friends.open = false;
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
        if !is_me {
            let id = user.id.clone();
            let relationship = self
                .app
                .friends
                .relationships
                .iter()
                .find(|r| r.id == id)
                .map(|r| r.kind);
            let pending = self
                .app
                .friends
                .pending
                .values()
                .any(|a| a.user_id() == Some(id.as_str()));
            if dm.is_none() {
                let uid = id.clone();
                info = info.child(
                    fill_button(
                        "profile_create_dm",
                        tr!("メッセージを送る", "Send Message"),
                        BLURPLE(),
                        WHITE(),
                        !pending,
                    )
                    .mt(z(8.0))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.app
                            .friend_action(crate::app::FriendAction::OpenDm(uid.clone()));
                        cx.notify();
                    })),
                );
            }
            if !self.app.friends.loaded
                && self.app.friends.loading.is_none()
                && self.app.friends.error.is_some()
            {
                info = info.child(
                    div()
                        .mt(z(8.0))
                        .flex()
                        .flex_col()
                        .gap(z(4.0))
                        .child(text(
                            tr!(
                                "フレンド情報を取得できませんでした",
                                "Couldn't load friend info"
                            ),
                            12.0,
                            MUTED(),
                        ))
                        .child(
                            text_button("profile_friends_retry", tr!("再読み込み", "Reload"))
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.app.fetch_friends();
                                    cx.notify();
                                })),
                        ),
                );
            }
            if self.app.friends.loaded {
                let mut controls = div().mt(z(8.0)).flex().flex_wrap().gap(z(4.0));
                match relationship {
                    None => {
                        let uid = id.clone();
                        controls = controls.child(
                            fill_button(
                                "profile_friend_add",
                                tr!("フレンドに追加", "Add Friend"),
                                BLURPLE(),
                                WHITE(),
                                !pending,
                            )
                            .on_click(cx.listener(
                                move |this, _, _, cx| {
                                    this.app.friend_action(crate::app::FriendAction::AddById(
                                        uid.clone(),
                                    ));
                                    cx.notify();
                                },
                            )),
                        );
                        controls = controls.child(self.friend_row_button(
                            &id,
                            tr!("ブロック", "Block"),
                            2,
                            pending,
                            cx,
                        ));
                    }
                    Some(1) => {
                        controls = controls.child(self.friend_row_button(
                            &id,
                            tr!("フレンド削除", "Remove Friend"),
                            1,
                            pending,
                            cx,
                        ));
                        controls = controls.child(self.friend_row_button(
                            &id,
                            tr!("ブロック", "Block"),
                            2,
                            pending,
                            cx,
                        ));
                    }
                    Some(2) => {
                        controls = controls.child(self.friend_row_button(
                            &id,
                            tr!("ブロック解除", "Unblock"),
                            1,
                            pending,
                            cx,
                        ))
                    }
                    Some(3) => {
                        controls = controls.child(self.friend_row_button(
                            &id,
                            tr!("承認", "Accept"),
                            3,
                            pending,
                            cx,
                        ));
                        controls = controls.child(self.friend_row_button(
                            &id,
                            tr!("拒否", "Ignore"),
                            1,
                            pending,
                            cx,
                        ));
                    }
                    Some(4) => {
                        controls = controls.child(self.friend_row_button(
                            &id,
                            tr!("申請を取り消す", "Cancel Request"),
                            1,
                            pending,
                            cx,
                        ))
                    }
                    _ => {}
                }
                info = info.child(controls);
            }
        }
        if let Some(guild_id) = pp.guild_id.clone() {
            let member = user.clone();
            let role_gid = guild_id.clone();
            info = info.child(
                text_button(
                    "profile_member_roles",
                    tr!("サーバーのロールを確認・管理", "View/Manage Server Roles"),
                )
                .mt(z(8.0))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.app.popup = None;
                    this.app.open_member_role_admin(&role_gid, &member);
                    cx.notify();
                })),
            );
            let is_me = self.app.me.as_ref().is_some_and(|me| me.id == user.id);
            let actions = [
                (
                    MemberAction::Timeout(3600),
                    tr!("1時間タイムアウト", "Time Out for 1 Hour"),
                ),
                (
                    MemberAction::Timeout(0),
                    tr!("タイムアウト解除", "Remove Timeout"),
                ),
                (MemberAction::Kick, tr!("キック", "Kick")),
                (MemberAction::Ban, "BAN"),
            ];
            let mut row = div().mt(z(6.0)).flex().flex_wrap().gap(z(4.0));
            let mut any = false;
            for (action, label) in actions {
                if is_me || !self.app.can_member_action(&guild_id, action) {
                    continue;
                }
                any = true;
                let key = action.key(&user.id);
                let confirming = self.app.member_action_confirm.as_deref() == Some(key.as_str());
                let (gid, uid) = (guild_id.clone(), user.id.clone());
                row = row.child(
                    fill_button(
                        SharedString::from(format!("profile_mod_{key}")),
                        if confirming {
                            tr!("もう一度押して確定", "Press again to confirm")
                        } else {
                            label
                        },
                        if confirming { RED() } else { WIDGET() },
                        WHITE(),
                        self.app.member_action_pending.is_none(),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if this.app.member_action(&gid, &uid, action) {
                            this.app.popup = None;
                        }
                        cx.notify();
                    })),
                );
            }
            if any {
                info = info.child(row);
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
