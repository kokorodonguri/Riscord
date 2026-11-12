//! メイン画面の並べ方と、右側（チャットのヘッダー・通話・メッセージ一覧・入力欄）
use std::hash::{Hash, Hasher};

use gpui::assets::IconName;
use gpui::component::input::{Input, Textarea};
use gpui::prelude::*;
use gpui::*;

use super::sidebar::speaking_avatar;
use super::*;
use crate::app::{author_name, dm_icon_url, ProfileState, HISTORY_TRIGGER_ROWS, MAX_HISTORY};
use crate::color::Color32;
use crate::model::{Message, User};
use crate::net::{self, Cmd};
use chrono::{DateTime, Local};

/// 行の中身の目印（変わったら測り直す）。t / pt はこの行と前の行の時刻（読み終えたもの）
fn row_key(
    m: &Message,
    prev: Option<&Message>,
    t: Option<DateTime<Local>>,
    pt: Option<DateTime<Local>>,
    compact: bool,
    plugin_revision: u64,
) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    m.hash(&mut h);
    plugin_revision.hash(&mut h);
    super::message::row_layout_at(m, prev, t, pt, compact).hash(&mut h);
    h.finish()
}

fn id_key(id: &str) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    id.hash(&mut h);
    h.finish().max(1)
}

/// 前の行の目印と比べて、変わった所だけ list に知らせる（先頭に読み足しても見ている行は動かない）
fn sync_list(l: &mut MsgList, keys: Vec<u64>, ids: Vec<u64>) {
    let old = &l.keys;
    let p = old.iter().zip(&keys).take_while(|(a, b)| a == b).count();
    let max_s = old.len().min(keys.len()) - p;
    let s = old
        .iter()
        .rev()
        .zip(keys.iter().rev())
        .take(max_s)
        .take_while(|(a, b)| a == b)
        .count();
    if p + s != old.len() || p + s != keys.len() {
        let top = l.state.logical_scroll_top();
        // 見ていたメッセージ（先頭の読み込み中表示を見ていたときは、その次の一番上のメッセージ）
        let anchor = if l.state.is_following_tail() {
            None
        } else if top.item_ix == 0 {
            l.ids
                .get(1)
                .copied()
                .filter(|&id| id != 0)
                .map(|id| (id, px(0.0), true))
        } else {
            l.ids
                .get(top.item_ix)
                .copied()
                .filter(|&id| id != 0)
                .map(|id| (id, top.offset_in_item, false))
        };
        // 先頭行（ix = 0）だけ計測済みのまま後ろに未計測の行が入ると、上へスクロールしたとき
        // 未計測の区間を飛び越えて先頭へ飛ぶので、0 から入れ替える
        let start = if p == 1 { 0 } else { p };
        let added = keys.len() - start - s;
        l.state.splice(start..old.len() - s, added);
        // 古いメッセージが前に入ると、元の一番上の行は日付の区切りや連投のまとまりが変わって key も変わる。
        // 見ていたメッセージの位置へスクロール位置を移す。そのままだと上端に張り付き、読み足しが止まらなくなる
        if let Some((anchor_id, offset, from_head)) = anchor {
            if let Some(new_ix) = ids.iter().position(|&id| id == anchor_id) {
                if from_head {
                    if new_ix > 1 {
                        l.state.scroll_to(ListOffset {
                            item_ix: new_ix,
                            offset_in_item: px(0.0),
                        });
                    }
                } else if l.state.logical_scroll_top().item_ix != new_ix {
                    let keep = if keys.get(new_ix) == old.get(top.item_ix) {
                        offset
                    } else {
                        px(0.0)
                    };
                    l.state.scroll_to(ListOffset {
                        item_ix: new_ix,
                        offset_in_item: keep,
                    });
                }
            }
        }
    }
    l.keys = keys;
    l.ids = ids;
}

impl Root {
    pub fn main_view(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let compact = self.app.screen_w < 700.0;
        self.app.subscribe_member_list();
        self.app.tick_search();
        let narrow_members = self.app.screen_w < 900.0;
        let members_shown = if narrow_members {
            self.app.show_members_narrow
        } else {
            self.app.show_members
        };
        let dm_user = match (&self.app.sel_guild, self.app.current_channel()) {
            (None, Some(c)) if !self.app.friends.open && c.recipients.len() == 1 => {
                Some(c.recipients[0].clone())
            }
            _ => None,
        };

        // 右側（ステータス帯・チャット・メンバー一覧／DM のプロフィール）
        let mut right = div()
            .flex_1()
            .min_w_0()
            .h_full()
            .flex()
            .flex_col()
            .relative();
        if let Some(v) = crate::update::banner() {
            right = right.child(
                div()
                    .flex_none()
                    .bg(GREEN())
                    .px(z(12.0))
                    .py(z(4.0))
                    .flex()
                    .items_center()
                    .gap(z(8.0))
                    .child(
                        line(
                            format!(
                                "Riscord を {v} に更新しました。再起動すると新しい版になります"
                            ),
                            13.0,
                            Color32::WHITE,
                        )
                        .flex_1()
                        .min_w_0(),
                    )
                    .child(
                        div()
                            .id("update_dismiss")
                            .size(z(18.0))
                            .flex_none()
                            .rounded(z(4.0))
                            .flex()
                            .items_center()
                            .justify_center()
                            .cursor_pointer()
                            .hover(|s| s.bg(Color32::from_black_alpha(50)))
                            .tooltip(tip("閉じる"))
                            .child(icon(IconName::Close, 12.0, Color32::WHITE))
                            .on_click(cx.listener(|_, _, _, cx| {
                                crate::update::close_banner();
                                cx.notify();
                            })),
                    ),
            );
        }
        if !self.app.status.is_empty() && self.app.status != "接続済み" {
            let color = if self.app.status.starts_with("エラー") {
                RED()
            } else {
                ORANGE()
            };
            // 操作の結果のお知らせは閉じられる（閉じないと次の接続状態の変化まで出たままになる）。接続の状態そのものは閉じない
            let dismissable = self.app.status != self.app.gateway_status;
            right = right.child(
                div()
                    .flex_none()
                    .bg(color)
                    .px(z(12.0))
                    .py(z(4.0))
                    .flex()
                    .items_center()
                    .gap(z(8.0))
                    .child(
                        line(self.app.status.clone(), 13.0, Color32::WHITE)
                            .flex_1()
                            .min_w_0(),
                    )
                    .when(dismissable, |d| {
                        d.child(
                            div()
                                .id("status_dismiss")
                                .size(z(18.0))
                                .flex_none()
                                .rounded(z(4.0))
                                .flex()
                                .items_center()
                                .justify_center()
                                .cursor_pointer()
                                .hover(|s| s.bg(Color32::from_black_alpha(50)))
                                .tooltip(tip("閉じる"))
                                .child(icon(IconName::Close, 12.0, Color32::WHITE))
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.app.status = this.app.gateway_status.clone();
                                    this.app.message_status_channel = None;
                                    cx.notify();
                                })),
                        )
                    }),
            );
        }
        let mut body = div().flex_1().min_h_0().flex().flex_row().relative();
        body = body.child(self.chat_column(
            members_shown,
            narrow_members,
            dm_user.as_ref(),
            window,
            cx,
        ));
        // ピン一覧・検索結果は、メンバー一覧・プロフィールの場所に出す（狭い画面では重ねる）
        let side = self.app.side != crate::app::SidePanel::None;
        if side && !narrow_members {
            body = body.child(self.side_panel(window, cx).w(z(360.0)).flex_none());
        }
        if !side && self.app.sel_guild.is_some() && members_shown && !narrow_members {
            let (_, mem_w) = self.app.pane_widths();
            body = body.child(
                self.members_panel(cx)
                    .child(self.pane_resizer(ResizeTarget::Members, cx))
                    .w(z(mem_w))
                    .flex_none(),
            );
        }
        if let Some(u) = dm_user
            .as_ref()
            .filter(|_| !side && !self.app.hide_dm_profile && self.app.screen_w > 980.0)
        {
            let key = net::profile_key(&u.id, None);
            if !self.app.profiles.contains_key(&key) {
                self.app.profiles.insert(key.clone(), ProfileState::Loading);
                self.app.send(Cmd::LoadProfile {
                    user_id: u.id.clone(),
                    guild_id: None,
                });
            }
            body = body.child(self.dm_profile_panel(u, cx));
        }
        right = right.child(body);
        // 狭い画面ではチャット幅を縮めず、ヘッダーの下に一覧を重ねる。
        if side && narrow_members {
            let w = self.app.screen_w.min(360.0);
            right = right.child(
                self.side_panel(window, cx)
                    .absolute()
                    .right_0()
                    .top(z(48.0))
                    .bottom_0()
                    .w(z(w))
                    .bg(opaque(BG())),
            );
        } else if self.app.sel_guild.is_some() && members_shown && narrow_members {
            right = right.child(
                self.members_panel(cx)
                    .absolute()
                    .right_0()
                    .top(z(48.0))
                    .bottom_0()
                    .w(z(self.app.overlay_member_width()))
                    .bg(opaque(BG()))
                    .border_l_1()
                    .border_color(DIVIDER()),
            );
        }

        let mut row = div().flex_1().min_h_0().flex().flex_row().relative();
        if !compact {
            row = row.child(self.sidebar(window, cx));
        }
        row = row.child(right);
        if compact && !self.app.compact_sidebar_closed {
            let w = self.app.screen_w.min(st::RAIL_W + self.app.pane_widths().0);
            row = row.child(
                div()
                    .absolute()
                    .left_0()
                    .top_0()
                    .bottom_0()
                    .w(z(w))
                    .bg(opaque(BG()))
                    .border_r_1()
                    .border_color(DIVIDER())
                    .occlude()
                    .child(self.sidebar(window, cx).w_full()),
            );
        }

        div()
            .size_full()
            .flex()
            .flex_col()
            .child(self.titlebar(compact, cx))
            .child(row)
            .children(self.toast_layer())
            .children(self.overlays(window, cx))
    }

    /// 画面下に短く出すお知らせ（「コピーしました」など）。時間が来たら消す
    fn toast_layer(&mut self) -> Option<AnyElement> {
        if self
            .app
            .toast
            .as_ref()
            .is_some_and(|(_, at)| at.elapsed() >= crate::app::TOAST_TTL)
        {
            self.app.toast = None;
        }
        let (t, _) = self.app.toast.as_ref()?;
        Some(
            div()
                .absolute()
                .bottom(z(80.0))
                .left_0()
                .right_0()
                .flex()
                .justify_center()
                .child(
                    div()
                        .bg(opaque(SIDEBAR()))
                        .border_1()
                        .border_color(BORDER())
                        .rounded(z(8.0))
                        .px(z(14.0))
                        .py(z(8.0))
                        .shadow_lg()
                        .child(text(t.clone(), 14.0, WHITE())),
                )
                .into_any_element(),
        )
    }

    /// 上に重ねるもの（メニュー・ポップアップ・ダイアログ・設定）
    fn overlays(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let mut v = Vec::new();
        v.extend(self.menu_layer(window, cx));
        v.extend(self.profile_popup(window, cx));
        v.extend(self.incoming_call(cx));
        v.extend(self.switcher_dialog(cx));
        v.extend(self.delete_dialog(cx));
        v.extend(self.thread_dialog(cx));
        v.extend(self.folder_dialog(cx));
        v.extend(self.role_admin_dialog(cx));
        v.extend(self.moderation_dialog(cx));
        v.extend(self.server_settings_dialog(cx));
        v.extend(self.soundboard_dialog(cx));
        v.extend(self.leave_dialog(cx));
        if self.app.settings_open {
            v.push(self.settings_modal(window, cx).into_any_element());
        }
        if let Some(uri) = self.image_preview.clone() {
            let w = (self.app.screen_w - 80.0).max(0.0);
            let h = (self.pane_h - 140.0).max(0.0);
            let picture = match crate::images::get(&uri) {
                crate::images::Img::Ready(image) => div()
                    .w(z(w))
                    .h(z(h))
                    .child(img(image).size_full().object_fit(ObjectFit::Contain)),
                crate::images::Img::Pending => div()
                    .w(z(w))
                    .h(z(h))
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(spinner(32.0)),
                crate::images::Img::Failed => div()
                    .w(z(w))
                    .h(z(h))
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(text("画像を読み込めませんでした", 16.0, WHITE())),
            };
            let content = div()
                .flex()
                .flex_col()
                .items_center()
                .gap(z(12.0))
                .child(picture)
                .child(
                    div()
                        .flex()
                        .gap(z(24.0))
                        .child(
                            text("ダウンロード", 14.0, WHITE())
                                .id("preview_download")
                                .cursor_pointer()
                                .on_click(cx.listener({
                                    let uri = uri.clone();
                                    move |this, _, _, cx| {
                                        this.start_download(
                                            uri.clone(),
                                            crate::download::name_from_url(&uri),
                                            crate::download::Kind::Image,
                                        );
                                        cx.notify();
                                    }
                                })),
                        )
                        .child(
                            text("ブラウザで開く", 14.0, WHITE())
                                .id("preview_browser")
                                .cursor_pointer()
                                .on_click(move |_, _, cx| cx.open_url(&uri)),
                        )
                        .child(
                            text("閉じる", 14.0, WHITE())
                                .id("preview_close")
                                .cursor_pointer()
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.image_preview = None;
                                    cx.notify();
                                })),
                        ),
                );
            v.push(
                modal(
                    "image_preview",
                    content,
                    cx.listener(|this, _, _, cx| {
                        this.image_preview = None;
                        cx.notify();
                    }),
                    220,
                )
                .into_any_element(),
            );
        }
        v
    }

    fn chat_column(
        &mut self,
        members_shown: bool,
        narrow_members: bool,
        dm_user: Option<&User>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        if self.app.friends.open && self.app.sel_guild.is_none() {
            return self.friends_view(cx);
        }
        let cur = self.app.current_channel().cloned();
        let (prefix, cname) = match &cur {
            Some(c) if self.app.sel_guild.is_none() => ("@", c.dm_label()),
            Some(c) => ("#", c.name.clone().unwrap_or_default()),
            None => ("", String::new()),
        };
        let mut col = div()
            .flex_1()
            .min_w_0()
            .h_full()
            .flex()
            .flex_col()
            .bg(CHAT())
            .relative();
        col = col.child(self.chat_head(
            members_shown,
            narrow_members,
            dm_user.is_some(),
            prefix,
            &cname,
            cx,
        ));
        if let (None, Some(c)) = (&self.app.sel_guild, &cur) {
            col = col.children(self.call_area(&c.id, cx));
        }
        col = col.children(self.stream_view(cx));
        // チャット欄だけに敷く背景画像
        let bg = self
            .app
            .theme_applied
            .as_ref()
            .and_then(|t| t.background.clone())
            .filter(|b| !b.window);
        let mut msgs = div().flex_1().min_h_0().relative().flex().flex_col();
        if let Some(bg) = &bg {
            msgs = msgs.child(background(bg, opaque(CHAT())));
        }
        // フォーラムはメッセージの代わりに投稿の一覧
        if let Some(f) = cur.as_ref().filter(|c| c.is_forum()) {
            msgs = msgs.child(self.forum_view(f, cx));
            return col.child(msgs);
        }
        msgs = msgs.child(self.messages_view(prefix, &cname, cx));
        msgs = msgs.children(self.jump_hint(cx));
        col = col.child(msgs);
        if cur.is_some() {
            col = col.child(self.compose(prefix, &cname, window, cx));
        }
        col
    }

    /// 上へさかのぼっている間、下端に出す「最新へ」の案内。押すか Shift+L で一番下へ
    fn jump_hint(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.scrolled_up() {
            return None;
        }
        let pill = div()
            .id("jump_latest")
            .flex()
            .items_center()
            .gap(z(6.0))
            .px(z(12.0))
            .py(z(5.0))
            .rounded_full()
            .bg(opaque(SIDEBAR()))
            .border_1()
            .border_color(BORDER())
            .shadow_lg()
            .cursor_pointer()
            .hover(|s| s.bg(opaque(WIDGET_HOVER())))
            .on_click(cx.listener(|this, _, _, cx| {
                this.jump_to_latest();
                cx.notify();
            }))
            .child(text("最新のメッセージへ", 13.0, TEXT()))
            .child(div().px(z(5.0)).rounded(z(4.0)).bg(WIDGET()).child(text(
                "Shift+L",
                11.0,
                MUTED(),
            )))
            .child(icon(IconName::ArrowDown, 14.0, MUTED()));
        Some(
            div()
                .absolute()
                .bottom(z(10.0))
                .left_0()
                .right_0()
                .flex()
                .justify_center()
                .child(pill)
                .into_any_element(),
        )
    }

    /// 開いているチャンネルを、一番下から上へさかのぼっているか
    pub fn scrolled_up(&self) -> bool {
        let Some(cid) = self.app.sel_channel.as_ref() else {
            return false;
        };
        self.lists
            .get(cid)
            .is_some_and(|l| !l.state.is_following_tail())
    }

    /// 開いているチャンネルの一番下（最新のメッセージ）へ飛び、新しいメッセージに付いていく
    pub fn jump_to_latest(&mut self) {
        let Some(cid) = self.app.sel_channel.clone() else {
            return;
        };
        // 途中だけを出していたら最新を読み直す（届いたら下端に付いていく）
        if self.app.is_detached(&cid) {
            self.app.back_to_latest(&cid);
            return;
        }
        if let Some(l) = self.lists.get(&cid) {
            l.state.set_follow_mode(FollowMode::Tail);
        }
    }

    fn chat_head(
        &mut self,
        members_shown: bool,
        narrow_members: bool,
        has_dm_user: bool,
        prefix: &'static str,
        cname: &str,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let a = &self.app;
        let head = div()
            .h(z(48.0))
            .flex_none()
            .px(z(16.0))
            .flex()
            .items_center()
            .gap(z(6.0))
            .border_t_1()
            .border_b_1()
            .border_color(DIVIDER())
            .bg(CHAT())
            .part(Part::ChatHeader);
        let Some(c) = a.current_channel() else {
            return head;
        };
        // 左: アイコン（DM は相手）＋名前＋トピック
        let mut left = div().flex_1().min_w_0().flex().items_center().gap(z(6.0));
        if a.sel_guild.is_none() {
            let av = avatar(dm_icon_url(c).as_deref(), 24.0, cname, 12.0, BLURPLE());
            left = left.child(if c.recipients.len() == 1 {
                let stt = a
                    .presences
                    .get(&c.recipients[0].id)
                    .map_or("offline", |p| p.status.as_str());
                with_status(av, 24.0, stt, CHAT())
            } else {
                av
            });
        } else {
            left = left.child(text(prefix, 22.0, FAINT()));
        }
        left = left.child(
            line(cname.to_string(), 16.0, WHITE())
                .font_weight(FontWeight::MEDIUM)
                .flex_none()
                .max_w(z(360.0)),
        );
        if let Some(topic) = c.topic.as_deref().filter(|t| !t.is_empty()) {
            left = left
                .child(
                    div()
                        .w(px(1.0))
                        .h(z(24.0))
                        .mx(z(6.0))
                        .bg(DIVIDER())
                        .flex_none(),
                )
                .child(line(topic.replace('\n', " "), 14.0, MUTED()).flex_1());
        }
        // 右から: 通話・ビデオ・再読込・一覧/プロフィール切替（Vesktop の並び）
        let mut right = div().flex().items_center().gap(z(6.0)).flex_none();
        if let Some(guild_id) = a
            .sel_guild
            .as_ref()
            .filter(|_| matches!(c.kind, 0 | 2 | 4 | 5 | 13 | 15 | 16))
            .filter(|g| a.may_edit_channel_permissions(g, &c.id))
        {
            let guild_id = guild_id.clone();
            let channel_id = c.id.clone();
            right = right.child(
                icon_button("channel_permissions", IconName::Settings, 32.0)
                    .tooltip(tip("チャンネルの権限"))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.app.open_channel_permissions(&guild_id, &channel_id);
                        cx.notify();
                    })),
            );
        }
        if a.sel_guild.is_none() {
            let cid = c.id.clone();
            right = right
                .child(
                    icon_button("call_start", IconName::Phone, 32.0)
                        .tooltip(tip("音声通話を開始"))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            let a = &mut this.app;
                            let active = a.calls.contains_key(&cid)
                                || a.voice_members(&cid).next().is_some();
                            if a.voice_channel().map(|(_, id, _)| id) != Some(&cid) {
                                a.join_voice(None, cid.clone(), !active);
                            }
                            cx.notify();
                        })),
                )
                .child(
                    div()
                        .id("video")
                        .size(z(32.0))
                        .flex()
                        .items_center()
                        .justify_center()
                        .tooltip(tip("ビデオ通話（未対応）"))
                        .child(icon(IconName::Video, 20.0, FAINT().gamma_multiply(0.5))),
                );
        }
        let search_open = a.side == crate::app::SidePanel::Search;
        right = right.child(
            toggle_icon("search_toggle", IconName::Search, search_open)
                .tooltip(tip(if search_open {
                    "検索を閉じる"
                } else {
                    "検索（Ctrl+F）"
                }))
                .on_click(cx.listener(|this, _, window, cx| {
                    this.app.toggle_search();
                    if this.app.side == crate::app::SidePanel::Search {
                        this.inp.search.update(cx, |s, cx| s.focus(window, cx));
                    }
                    cx.notify();
                })),
        );
        // スレッド: 参加・退出。テキストチャンネル: スレッド一覧
        if c.is_thread() {
            let (tid, joined) = (c.id.clone(), a.joined_threads.contains(&c.id));
            right = right.child(
                toggle_icon(
                    "thread_join",
                    if joined {
                        IconName::LogOut
                    } else {
                        IconName::UserPlus
                    },
                    false,
                )
                .tooltip(tip(if joined {
                    "スレッドから退出"
                } else {
                    "スレッドに参加"
                }))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.app.join_thread(&tid, !joined);
                    cx.notify();
                })),
            );
        } else if a.sel_guild.is_some() && c.is_text() {
            let open = a.side == crate::app::SidePanel::Threads;
            right = right.child(
                toggle_icon("threads_toggle", IconName::MessagesSquare, open)
                    .tooltip(tip(if open {
                        "スレッド一覧を閉じる"
                    } else {
                        "スレッド"
                    }))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.app.toggle_thread_panel();
                        cx.notify();
                    })),
            );
        }
        let pins_open = a.side == crate::app::SidePanel::Pins;
        right = right.child(
            toggle_icon("pins_toggle", IconName::Pin, pins_open)
                .tooltip(tip(if pins_open {
                    "ピン留めを閉じる"
                } else {
                    "ピン留めされたメッセージ"
                }))
                .on_click(cx.listener(|this, _, _, cx| {
                    this.app.toggle_pins();
                    cx.notify();
                })),
        );
        let loading = a
            .sel_channel
            .as_ref()
            .is_some_and(|id| a.pending_message_loads.contains_key(id));
        right = right.child(if loading {
            div()
                .id("reload")
                .size(z(32.0))
                .flex()
                .items_center()
                .justify_center()
                .tooltip(tip("メッセージを読み込み中"))
                .child(spinner(18.0))
        } else {
            icon_button("reload", IconName::RotateCw, 32.0)
                .tooltip(tip("再読み込み"))
                .on_click(cx.listener(|this, _, _, cx| {
                    if let Some(c) = this.app.sel_channel.clone() {
                        this.app.load_messages(c);
                    }
                    cx.notify();
                }))
        });
        if a.sel_guild.is_some() {
            right = right.child(
                toggle_icon("members_toggle", IconName::Users, members_shown)
                    .tooltip(tip(if members_shown {
                        "メンバーリストを非表示"
                    } else {
                        "メンバーリストを表示"
                    }))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if narrow_members {
                            this.app.show_members_narrow = !this.app.show_members_narrow;
                            // 狭い画面ではチャンネル一覧と重なるので、☰ と同じく片方だけ開く
                            if this.app.screen_w < 700.0 && this.app.show_members_narrow {
                                this.app.compact_sidebar_closed = true;
                            }
                        } else {
                            this.app.show_members = !this.app.show_members;
                        }
                        cx.notify();
                    })),
            );
        } else if has_dm_user {
            let shown = !a.hide_dm_profile;
            right = right.child(
                toggle_icon("profile_toggle", IconName::PanelRight, shown)
                    .tooltip(tip(if shown {
                        "ユーザープロフィールを非表示"
                    } else {
                        "ユーザープロフィールを表示"
                    }))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.app.hide_dm_profile = shown;
                        cx.notify();
                    })),
            );
        }
        head.child(left).child(right)
    }

    /// 視聴中の画面共有（チャット欄の上に出す）
    fn stream_view(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let (key, phase) = self.app.watching.clone()?;
        let uid = crate::golive::key_user(&key)
            .unwrap_or_default()
            .to_string();
        let name = self
            .app
            .find_user(&uid)
            .map(|u| u.display().to_string())
            .unwrap_or_else(|| uid.clone());
        let h = (self.pane_h * 0.55).clamp(180.0, 720.0);
        let screen = match &self.stream_image {
            Some((_, frame)) => div().size_full().child(
                img(frame.clone())
                    .id("stream-frame")
                    .size_full()
                    .object_fit(ObjectFit::Contain),
            ),
            None => div()
                .size_full()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap(z(10.0))
                .child(spinner(28.0))
                .child(text(
                    if phase == crate::golive::Phase::Live {
                        "映像を待っています…"
                    } else {
                        "配信に接続しています…"
                    },
                    14.0,
                    MUTED(),
                )),
        };
        let head = div()
            .h(z(36.0))
            .flex_none()
            .px(z(12.0))
            .flex()
            .items_center()
            .gap(z(8.0))
            // 映像が届く前は LIVE を出さない（つながっていないのに配信中に見えないように）
            .when(phase == crate::golive::Phase::Live, |d| {
                d.child(super::sidebar::live_badge())
            })
            .child(line(format!("{name} の画面"), 14.0, WHITE()).flex_1())
            .child(
                icon_button("stream_close", IconName::Close, 28.0)
                    .tooltip(tip("視聴をやめる"))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.app.stop_watching();
                        cx.notify();
                    })),
            );
        Some(
            div()
                .h(z(h))
                .flex_none()
                .bg(Color32::BLACK)
                .flex()
                .flex_col()
                .child(head)
                .child(div().flex_1().min_h_0().child(screen))
                .into_any_element(),
        )
    }

    /// DM通話エリア（参加者・呼び出し中の人と、操作ボタン）
    fn call_area(&mut self, cid: &str, cx: &mut Context<Self>) -> Option<AnyElement> {
        let a = &self.app;
        let members: Vec<(String, Option<User>, bool, bool)> = a
            .voice_members(cid)
            .map(|v| {
                (
                    v.user_id.clone(),
                    v.user.clone().or_else(|| a.find_user(&v.user_id)),
                    v.self_mute || v.self_deaf,
                    false,
                )
            })
            .chain(
                a.calls
                    .get(cid)
                    .into_iter()
                    .flatten()
                    .filter(|id| {
                        !a.voice_states
                            .get(*id)
                            .is_some_and(|v| v.channel_id.as_deref() == Some(cid))
                    })
                    .map(|id| (id.clone(), a.find_user(id), false, true)),
            )
            .collect();
        let in_call = a.voice_channel().map(|(_, id, _)| id.as_str()) == Some(cid);
        if members.is_empty() && !in_call && !a.calls.contains_key(cid) {
            return None;
        }
        let mut faces = div().flex().gap(z(20.0)).justify_center();
        for (id, u, muted, ringing) in &members {
            let mut face = div()
                .relative()
                .flex()
                .flex_col()
                .items_center()
                .gap(z(6.0))
                .child(
                    div()
                        .relative()
                        .child(speaking_avatar(
                            u.as_ref(),
                            id,
                            80.0,
                            !ringing && a.is_speaking(id),
                        ))
                        .when(*ringing, |d| {
                            // 呼び出し中は暗くして脈打たせる
                            d.child(
                                div()
                                    .absolute()
                                    .inset_0()
                                    .rounded_full()
                                    .bg(Color32::from_black_alpha(140))
                                    .with_animation(
                                        SharedString::from(format!("ring{id}")),
                                        Animation::new(std::time::Duration::from_millis(1400))
                                            .repeat(),
                                        |d, t| {
                                            d.opacity(
                                                0.4 + 0.6 * (t * std::f32::consts::TAU).sin().abs(),
                                            )
                                        },
                                    ),
                            )
                        })
                        .when(*muted, |d| {
                            d.child(
                                div()
                                    .absolute()
                                    .right_0()
                                    .bottom_0()
                                    .size(z(24.0))
                                    .rounded_full()
                                    .bg(RED())
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .child(icon(IconName::MicOff, 14.0, WHITE())),
                            )
                        }),
                );
            if let Some(u) = u {
                face = face.child(text(u.display().to_string(), 12.0, MUTED()));
            }
            if !ringing && a.is_streaming(id) {
                face = face.child(super::sidebar::live_badge());
                if in_call && a.me.as_ref().is_none_or(|m| &m.id != id) {
                    let uid = id.clone();
                    let face = face
                        .id(SharedString::from(format!("face{id}")))
                        .cursor_pointer()
                        .tooltip(tip("配信を見る"))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.app.watch_stream(&uid);
                            cx.notify();
                        }));
                    faces = faces.child(face);
                    continue;
                }
            }
            faces = faces.child(face);
        }
        let cid_s = cid.to_string();
        let buttons = if in_call {
            let off = a.mic_off();
            let deaf = a.settings.self_deaf;
            let dark = SELECTED();
            div()
                .flex()
                .gap(z(16.0))
                .child(
                    round_button(
                        "call_mic",
                        if off { IconName::MicOff } else { IconName::Mic },
                        if off { WHITE() } else { dark },
                        if off { RED() } else { WHITE() },
                    )
                    .tooltip(tip(if off {
                        "ミュート解除"
                    } else {
                        "ミュート"
                    }))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.app.toggle_mute();
                        cx.notify();
                    })),
                )
                .child(
                    round_button(
                        "call_deaf",
                        if deaf {
                            IconName::HeadphoneOff
                        } else {
                            IconName::Headphones
                        },
                        if deaf { WHITE() } else { dark },
                        if deaf { RED() } else { WHITE() },
                    )
                    .tooltip(tip("スピーカーミュート"))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.app.toggle_deaf();
                        cx.notify();
                    })),
                )
                .child({
                    let sharing = a.my_stream.is_some();
                    round_button(
                        "call_screen",
                        if sharing {
                            IconName::MonitorOff
                        } else {
                            IconName::MonitorUp
                        },
                        if sharing { GREEN() } else { dark },
                        WHITE(),
                    )
                    .tooltip(tip(if sharing {
                        "画面共有を停止"
                    } else {
                        "画面を共有"
                    }))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.app.toggle_stream();
                        cx.notify();
                    }))
                })
                .child(
                    round_button("call_leave", IconName::PhoneOff, RED(), WHITE())
                        .tooltip(tip("切断"))
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.app.leave_voice();
                            cx.notify();
                        })),
                )
        } else {
            div().child(
                round_button("call_join", IconName::Phone, GREEN(), WHITE())
                    .tooltip(tip("通話に参加"))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.app.join_voice(None, cid_s.clone(), false);
                        cx.notify();
                    })),
            )
        };
        // 高さは中身に合わせる（LIVE 表示が付くとボタンが下にはみ出すため）
        Some(
            div()
                .min_h(z(230.0))
                .flex_none()
                .bg(CALL_BG())
                .flex()
                .flex_col()
                .items_center()
                .pt(z(36.0))
                .pb(z(24.0))
                .gap(z(28.0))
                .child(faces)
                .child(buttons)
                .into_any_element(),
        )
    }

    /// メッセージ一覧（仮想スクロール。見えている行だけ描く）
    fn messages_view(
        &mut self,
        prefix: &'static str,
        cname: &str,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(cid) = self.app.sel_channel.clone() else {
            // 選べるものが無いのに「選んでください」と言わない
            let hint = match &self.app.sel_guild {
                Some(g)
                    if self
                        .app
                        .channels
                        .get(g)
                        .is_some_and(|c| !c.iter().any(|c| c.is_text())) =>
                {
                    "このサーバーには表示できるテキストチャンネルがありません"
                }
                None if self.app.dms_loaded && self.app.dms.is_empty() => {
                    "ダイレクトメッセージはまだありません"
                }
                _ => "左からチャンネルを選んでください",
            };
            return div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .child(text(hint, 16.0, MUTED()))
                .into_any_element();
        };
        if !self.app.messages.contains_key(&cid) {
            let err = self.app.message_errors.contains(&cid);
            return div()
                .size_full()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap(z(8.0))
                .when(err, |d| {
                    d.child(text(
                        "メッセージを読み込めませんでした。通信状態を確認してください。",
                        16.0,
                        MUTED(),
                    ))
                    .child(
                        small_button("reload_msgs", "再読み込み").on_click(cx.listener(
                            |this, _, _, cx| {
                                if let Some(c) = this.app.sel_channel.clone() {
                                    this.app.load_messages(c);
                                }
                                cx.notify();
                            },
                        )),
                    )
                })
                .when(!err, |d| d.child(spinner(28.0)))
                .into_any_element();
        }
        let compact = self.app.settings.compact;
        let msgs = &self.app.messages[&cid];
        let no_access = self.app.no_access.contains(&cid);
        let hist_end = self.app.history.get(&cid).is_some_and(|h| h.end);
        // 一覧が変わっていなければ、前に作った行の目印をそのまま使う（全件のハッシュと時刻の読み取りを毎フレームしない）
        let stamp = {
            let mut h = std::collections::hash_map::DefaultHasher::new();
            (
                self.app.msg_rev,
                self.app.plugins.revision,
                compact,
                no_access,
                hist_end,
                prefix,
                cname,
            )
                .hash(&mut h);
            h.finish()
        };
        let fresh = self.lists.get(&cid).is_some_and(|l| l.stamp == stamp);
        let n = if fresh {
            self.lists[&cid].keys.len()
        } else {
            let mut keys = Vec::with_capacity(msgs.len() + 2);
            let mut ids = Vec::with_capacity(msgs.len() + 2);
            {
                let mut h = std::collections::hash_map::DefaultHasher::new();
                (no_access, hist_end, prefix, cname).hash(&mut h);
                keys.push(h.finish());
                ids.push(0);
            }
            let mut pt = None;
            for (i, m) in msgs.iter().enumerate() {
                let t = crate::app::local_dt(&m.timestamp);
                keys.push(row_key(
                    m,
                    i.checked_sub(1).map(|j| &msgs[j]),
                    t,
                    pt,
                    compact,
                    self.app.plugins.revision,
                ));
                ids.push(id_key(&m.id));
                pt = t;
            }
            keys.push(1);
            ids.push(0);
            let n = keys.len();
            match self.lists.get_mut(&cid) {
                Some(l) => sync_list(l, keys, ids),
                None => {
                    let state = ListState::new(n, ListAlignment::Bottom, px(600.0));
                    state.set_follow_mode(FollowMode::Tail);
                    // 下端から離れた・戻ったときだけ描き直す（「最新へ」の案内を出し入れする）
                    let (root, away) = (self.this.clone(), std::cell::Cell::new(false));
                    state.set_scroll_handler(move |ev, _, cx| {
                        if away.replace(!ev.is_following_tail) != !ev.is_following_tail {
                            let _ = root.update(cx, |_, cx| cx.notify());
                        }
                    });
                    self.lists.insert(
                        cid.clone(),
                        MsgList {
                            state,
                            keys,
                            ids,
                            stamp: 0,
                            detached: false,
                        },
                    );
                }
            }
            self.lists.get_mut(&cid).unwrap().stamp = stamp;
            n
        };
        let l = &self.lists[&cid];
        let state = l.state.clone();
        // 上端に近づいたら古いメッセージを読み足す
        let near_top = state.logical_scroll_top().item_ix < HISTORY_TRIGGER_ROWS;
        let first = msgs.first().map(|m| m.id.clone());
        let len = msgs.len();
        // ピン一覧・検索結果から飛んできた: その行（の1つ上）を一番上に出す
        let target = self
            .app
            .scroll_to_message
            .as_ref()
            .filter(|(c, _)| *c == cid)
            .map(|(_, id)| msgs.iter().position(|m| &m.id == id));
        if let Some(found) = target {
            self.app.scroll_to_message = None;
            if let Some(i) = found {
                state.scroll_to(ListOffset {
                    item_ix: i,
                    offset_in_item: px(0.0),
                });
            }
        }
        // 途中だけを出している間は下端に付いていかず、下端に近づいたら続きを読む
        let detached = self.app.is_detached(&cid);
        if let Some(l) = self.lists.get_mut(&cid).filter(|l| l.detached != detached) {
            l.detached = detached;
            l.state.set_follow_mode(if detached {
                FollowMode::Normal
            } else {
                FollowMode::Tail
            });
            if let Some(Some(i)) = target.filter(|_| detached) {
                l.state.scroll_to(ListOffset {
                    item_ix: i,
                    offset_in_item: px(0.0),
                });
            }
        }
        if detached && state.logical_scroll_top().item_ix + HISTORY_TRIGGER_ROWS >= n {
            self.app.load_newer(&cid);
            if self
                .app
                .history
                .get(&cid)
                .is_some_and(|h| h.newer_retry_at.is_some())
            {
                self.app.repaint_after(std::time::Duration::from_secs(1));
            }
        }
        let hist = self.app.history.entry(cid.clone()).or_default();
        let retry_ok = hist.retry_at.is_none_or(|t| std::time::Instant::now() >= t);
        if near_top && !hist.end && hist.pending.is_none() && retry_ok && len < MAX_HISTORY {
            if let Some(first) = first {
                hist.pending = Some(first.clone());
                hist.retry_at = None;
                self.app.send(Cmd::LoadOlder {
                    channel_id: cid.clone(),
                    before: first,
                });
            }
        } else if hist.retry_at.is_some() {
            self.app.repaint_after(std::time::Duration::from_secs(1));
        }
        let this = self.this.clone();
        let (prefix, cname) = (prefix.to_string(), cname.to_string());
        div()
            .id("messages")
            .size_full()
            .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                if !*hovered && this.hover_msg.is_some() {
                    this.hover_msg = None;
                    cx.notify();
                }
            }))
            .child(
                list(state, move |ix, window, cx| {
                    let Some(e) = this.upgrade() else {
                        return div().into_any_element();
                    };
                    e.update(cx, |r, cx| {
                        r.list_item(&cid, ix, n, &prefix, &cname, window, cx)
                    })
                })
                .size_full(),
            )
            .into_any_element()
    }

    fn list_item(
        &mut self,
        cid: &str,
        ix: usize,
        n: usize,
        prefix: &str,
        cname: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if ix == 0 {
            // 先頭: 始まりまで読んだらその旨、まだなら読み込み中の印
            let no_access = self.app.no_access.contains(cid);
            let end = self.app.history.get(cid).is_some_and(|h| h.end);
            let d = div()
                .h(z(st::HISTORY_HEAD_H))
                .px(z(16.0))
                .pb(z(12.0))
                .flex()
                .items_end();
            return if no_access {
                d.child(text("このチャンネルを見る権限がありません", 20.0, MUTED()))
            } else if end {
                d.child(text(format!("{prefix}{cname} の始まりです"), 20.0, WHITE()))
            } else {
                d.justify_center()
                    .items_center()
                    .pb_0()
                    .child(spinner(24.0))
            }
            .into_any_element();
        }
        if ix + 1 == n {
            return div().h(z(16.0)).into_any_element();
        }
        // 行を描く間だけ一覧を借り出す（見えている行ごとに Message を丸ごと複製しない）。message_row は messages を見ない
        let Some(msgs) = self.app.messages.get_mut(cid).map(std::mem::take) else {
            return div().into_any_element();
        };
        let i = ix - 1;
        let el = match msgs.get(i) {
            Some(m) => self
                .message_row(m, i.checked_sub(1).and_then(|j| msgs.get(j)), window, cx)
                .into_any_element(),
            None => div().into_any_element(),
        };
        if let Some(slot) = self.app.messages.get_mut(cid) {
            *slot = msgs;
        }
        el
    }

    /// 入力欄（返信・編集の帯、送信中の表示、添付、＋メニュー）
    fn compose(
        &mut self,
        prefix: &str,
        cname: &str,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        // :名前: を打ちかけなら、入力欄の上に絵文字の候補を出す。「/名前」ならコマンドの候補
        let suggest = self.emoji_suggest(_window, cx);
        let cmd_suggest = self.command_suggest(cx);
        if self.app.cmd_draft.is_none() {
            self.ensure_cmd_inputs(_window, cx);
        }
        let a = &self.app;
        let mut col = div()
            .flex_none()
            .bg(CHAT())
            .px(z(10.0))
            .pt(z(2.0))
            .pb(z(10.0))
            .flex()
            .flex_col();
        let no_access = a
            .sel_channel
            .as_ref()
            .is_some_and(|id| a.no_access.contains(id));
        if no_access {
            return col.child(card(INPUT(), DIVIDER(), 8.0).p(z(12.0)).child(text(
                "このチャンネルを見る権限がないため、メッセージを送信できません。",
                16.0,
                MUTED(),
            )));
        }
        // スレッドのロック・権限・アーカイブ
        if let Some(c) = a.current_channel().filter(|c| c.is_thread()) {
            if let Some(reason) = a.thread_block_reason(c) {
                return col.child(
                    card(INPUT(), DIVIDER(), 8.0)
                        .p(z(12.0))
                        .flex()
                        .items_center()
                        .gap(z(8.0))
                        .child(icon(IconName::Lock, 16.0, MUTED()))
                        .child(text(reason, 15.0, MUTED())),
                );
            }
            if c.archived() {
                col = col.child(div().px(z(4.0)).pb(z(4.0)).flex().items_center().gap(z(6.0)).child(icon(IconName::Archive, 14.0, MUTED())).child(text("このスレッドはアーカイブされています。メッセージを送信すると再開します。", 13.0, MUTED())));
            }
        }
        if let Some(d) = a
            .poll_draft
            .as_ref()
            .filter(|d| Some(&d.channel_id) == a.sel_channel.as_ref())
        {
            let sending = d.pending.is_some();
            let mut form = card(INPUT(), DIVIDER(), 8.0)
                .p(z(10.0))
                .flex()
                .flex_col()
                .gap(z(6.0))
                .child(medium("投票を作成", 16.0, WHITE()))
                .child(
                    Input::new(&self.inp.poll_question)
                        .appearance(false)
                        .disabled(sending),
                )
                .child(
                    Textarea::new(&self.inp.poll_answers)
                        .appearance(false)
                        .disabled(sending),
                );
            let mut durations = div().flex().flex_wrap().gap(z(4.0));
            for (hours, label) in [
                (1, "1時間"),
                (4, "4時間"),
                (8, "8時間"),
                (24, "1日"),
                (72, "3日"),
                (168, "1週間"),
            ] {
                durations = durations.child(
                    fill_button(
                        format!("poll_duration_{hours}"),
                        label,
                        if d.duration == hours {
                            BLURPLE()
                        } else {
                            WIDGET()
                        },
                        WHITE(),
                        !sending,
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if let Some(d) = this.app.poll_draft.as_mut() {
                            if d.pending.is_none() {
                                d.duration = hours;
                            }
                        }
                        cx.notify();
                    })),
                );
            }
            form = form.child(durations).child(
                text_button(
                    "poll_multi",
                    if d.multiselect {
                        "☑ 複数回答"
                    } else {
                        "☐ 複数回答"
                    },
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    if let Some(d) = this.app.poll_draft.as_mut() {
                        if d.pending.is_none() {
                            d.multiselect = !d.multiselect;
                        }
                    }
                    cx.notify();
                })),
            );
            form = form.children(d.error.clone().map(|e| text(e, 13.0, RED())));
            form = form.child(
                div()
                    .flex()
                    .gap(z(8.0))
                    .child(
                        text_button("poll_cancel", "キャンセル").on_click(cx.listener(
                            |this, _, _, cx| {
                                if this
                                    .app
                                    .poll_draft
                                    .as_ref()
                                    .is_some_and(|d| d.pending.is_none())
                                {
                                    this.app.poll_draft = None;
                                }
                                cx.notify();
                            },
                        )),
                    )
                    .child(
                        fill_button(
                            "poll_submit",
                            if sending {
                                "送信中…"
                            } else {
                                "投票を作成"
                            },
                            BLURPLE(),
                            WHITE(),
                            !sending,
                        )
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.app.submit_poll();
                            cx.notify();
                        })),
                    ),
            );
            return col.child(form);
        }
        col = col.children(suggest).children(cmd_suggest);
        if let Some(label) = a.pending_command_label() {
            col = col.child(
                div()
                    .px(z(4.0))
                    .pb(z(4.0))
                    .flex()
                    .items_center()
                    .gap(z(8.0))
                    .child(spinner(14.0))
                    .child(text(format!("{label} を実行中…"), 13.0, MUTED())),
            );
        }
        // コマンドを選んだら、入力欄の代わりに引数の欄
        if a.cmd_draft.is_some() {
            return col.child(self.command_form(_window, cx));
        }
        if let Some((cid, sub)) = a
            .sel_channel
            .as_ref()
            .and_then(|cid| a.compose_submissions.get(cid).map(|s| (cid.clone(), s)))
        {
            let mut b = div()
                .bg(SIDEBAR())
                .rounded(z(8.0))
                .px(z(12.0))
                .py(z(8.0))
                .mb(z(4.0))
                .flex()
                .flex_col()
                .gap(z(6.0));
            if sub.failed {
                let action = if sub.editing.is_some() {
                    "編集"
                } else {
                    "送信"
                };
                let msg = match &sub.rejected {
                    // 断られたなら届いていないので、履歴の確認はいらない
                    Some(reason) => format!("{action}できませんでした：{reason}。内容は保持しています。入力内容を戻して直してください。"),
                    None => format!("{action}結果を確認できませんでした。履歴を確認してから、入力内容を戻して再試行してください。"),
                };
                let can_restore = a.input.is_empty()
                    && a.uploads.is_empty()
                    && a.editing.is_none()
                    && a.reply_to.is_none();
                b = b.child(text(msg, 16.0, RED()));
                if can_restore {
                    b = b.child(div().flex().child(
                        small_button("restore", "入力内容を戻す").on_click(cx.listener(
                            move |this, _, _, cx| {
                                if this.app.restore_compose(&cid) {
                                    this.app.focus_input = true;
                                }
                                cx.notify();
                            },
                        )),
                    ));
                } else {
                    b = b.child(text(
                        "現在の入力や添付を片付けると、保持した内容を戻せます。",
                        12.0,
                        MUTED(),
                    ));
                }
            } else {
                b = b.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(z(8.0))
                        .child(spinner(16.0))
                        .child(text(
                            if sub.editing.is_some() {
                                "保存中…"
                            } else {
                                "送信中…"
                            },
                            16.0,
                            MUTED(),
                        )),
                );
            }
            col = col.child(b);
        }
        // 返信中・編集中の帯（入力欄の上にくっつく）
        let bar = match (&a.editing, &a.reply_to) {
            (Some(_), _) => Some((
                "メッセージを編集中".to_string(),
                "Esc でキャンセル・Enter で保存".to_string(),
            )),
            (None, Some(m)) => Some((
                format!("返信: {}", author_name(m)),
                "Esc でキャンセル".to_string(),
            )),
            _ => None,
        };
        if let Some((label, sub)) = &bar {
            col = col.child(
                div()
                    .bg(SIDEBAR())
                    .border_1()
                    .border_color(DIVIDER())
                    .rounded_t(z(8.0))
                    .pl(z(14.0))
                    .pr(z(8.0))
                    .py(z(6.0))
                    .flex()
                    .items_center()
                    .gap(z(8.0))
                    .child(line(label.clone(), 13.0, MUTED()))
                    .when(a.screen_w >= 600.0, |d| {
                        d.child(line(sub.clone(), 12.0, FAINT()).flex_1())
                    })
                    .when(a.screen_w < 600.0, |d| d.child(div().flex_1()))
                    .child(
                        icon_button("cancel_compose", IconName::CircleX, 22.0)
                            .tooltip(tip("キャンセル"))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.cancel_compose();
                                cx.notify();
                            })),
                    ),
            );
        }
        let mut boxed = div()
            .bg(INPUT())
            .border_1()
            .border_color(DIVIDER())
            .pl(z(12.0))
            .pr(z(16.0))
            .py(z(10.0))
            .flex()
            .flex_col();
        boxed = if bar.is_some() {
            boxed.rounded_b(z(8.0)).border_t_0()
        } else {
            boxed.rounded(z(8.0)).part(Part::ChatInput)
        };
        if bar.is_some() {
            // 返信・編集の帯が上に付いているときは、上の角と枠をつなげたままにする（色だけ変える）
            let st = part_style(Part::ChatInput);
            boxed = boxed
                .when_some(st.bg, |d, c| d.bg(c))
                .when_some(st.border, |d, c| d.border_color(c))
                .when_some(st.text, |d, c| d.text_color(c));
        }
        // 添付の上限を超えて除いたファイルのお知らせ（しばらくするか × で消える）
        if a.compose_notice
            .as_ref()
            .is_some_and(|(_, at)| at.elapsed() >= st::COMPOSE_NOTICE_TTL)
        {
            self.app.compose_notice = None;
        }
        let a = &self.app;
        if let Some((t, _)) = &a.compose_notice {
            boxed = boxed.child(
                div()
                    .flex()
                    .items_start()
                    .gap(z(6.0))
                    .mb(z(8.0))
                    .child(
                        div()
                            .size(z(18.0))
                            .flex_none()
                            .rounded_full()
                            .bg(ORANGE())
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_size(z(12.0))
                            .text_color(Color32::WHITE)
                            .child("!"),
                    )
                    .child(text(t.clone(), 14.0, TEXT()).flex_1())
                    .child(
                        icon_button("dismiss_notice", IconName::CircleX, 22.0)
                            .tooltip(tip("閉じる"))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.app.compose_notice = None;
                                cx.notify();
                            })),
                    ),
            );
        }
        // 送信待ちの添付
        if !a.uploads.is_empty() {
            let mut row = div()
                .id("uploads")
                .flex()
                .gap(z(8.0))
                .overflow_x_scroll()
                .mb(z(10.0));
            for (i, u) in a.uploads.iter().enumerate() {
                let thumb = match &u.thumb {
                    Some(t) => div()
                        .size_full()
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(
                            img(t.clone())
                                .max_w_full()
                                .max_h_full()
                                .rounded(z(4.0))
                                .object_fit(ObjectFit::Contain),
                        ),
                    None => div()
                        .size_full()
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(icon(IconName::File, 32.0, MUTED())),
                };
                row = row.child(
                    div()
                        .flex_none()
                        .bg(SIDEBAR())
                        .rounded(z(6.0))
                        .p(z(6.0))
                        .w(z(132.0))
                        .flex()
                        .flex_col()
                        .gap(z(4.0))
                        .child(
                            div().relative().w(z(120.0)).h(z(90.0)).child(thumb).child(
                                div()
                                    .id(("rm_upload", i))
                                    .absolute()
                                    .top_0()
                                    .right_0()
                                    .size(z(22.0))
                                    .rounded(z(4.0))
                                    .bg(WIDGET())
                                    .hover(|s| s.bg(RED()))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .cursor_pointer()
                                    .tooltip(tip("添付を外す"))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        if i < this.app.uploads.len() {
                                            this.app.uploads.remove(i);
                                        }
                                        cx.notify();
                                    }))
                                    .child(icon(IconName::X, 12.0, WHITE())),
                            ),
                        )
                        .child(line(u.name.clone(), 12.0, MUTED())),
                );
            }
            boxed = boxed.child(row);
        }
        let plus_open = self.menu.is_some_and(|m| m.0 == PopMenu::Plus);
        let emoji_open = self.menu.is_some_and(|m| m.0 == PopMenu::Emoji);
        let hint = format!("{prefix}{cname} へメッセージを送信");
        if self.compose_hint != hint {
            self.compose_hint = hint.clone();
            self.inp
                .compose
                .update(cx, |s, cx| s.set_placeholder(hint, _window, cx));
        }
        let this = self.this.clone();
        let input =
            Textarea::new(&self.inp.compose)
                .appearance(false)
                .on_paste(move |item, _, cx| {
                    let Some(e) = this.upgrade() else {
                        return false;
                    };
                    e.update(cx, |r, cx| {
                        let a = &r.app;
                        if a.editing.is_some() {
                            return false;
                        }
                        let handled = match item.text() {
                            Some(t) if !t.is_empty() => r.app.paste_text(&t),
                            // 画像だけのクリップボードは文字が無い
                            _ => r.app.paste_image(),
                        };
                        cx.notify();
                        handled
                    })
                });
        // 上限の近くで残りの文字数を右下に出す（超えたら赤でマイナス表示。本家と同じ）
        let limit = st::message_text_limit(a.me.as_ref().and_then(|u| u.premium_type));
        let left = limit as i64 - st::message_text_len(&a.input) as i64;
        let entry = div()
            .flex()
            .items_start()
            .gap(z(10.0))
            .child(
                div()
                    .id("plus")
                    .group("plus")
                    .mt(z(6.0))
                    .size(z(24.0))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.menu = if this.menu.is_some_and(|m| m.0 == PopMenu::Plus) {
                            None
                        } else {
                            Some((PopMenu::Plus, window.mouse_position()))
                        };
                        cx.notify();
                    }))
                    .child(
                        icon(
                            IconName::CirclePlus,
                            22.0,
                            if plus_open { WHITE() } else { MUTED() },
                        )
                        .group_hover("plus", |s| s.text_color(WHITE())),
                    ),
            )
            .child(div().flex_1().min_w_0().text_size(z(16.0)).child(input))
            .child(
                div()
                    .id("emoji_btn")
                    .group("emoji_btn")
                    .mt(z(6.0))
                    .size(z(24.0))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .tooltip(tip("絵文字"))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.toggle_emoji_picker(window, cx);
                        cx.notify();
                    }))
                    .child(
                        icon(
                            IconName::FaceSlightlySmiling,
                            22.0,
                            if emoji_open { WHITE() } else { MUTED() },
                        )
                        .group_hover("emoji_btn", |s| s.text_color(WHITE())),
                    ),
            );
        boxed = boxed.child(entry);
        if left < 200 {
            boxed = boxed.child(div().flex().justify_end().child(text(
                left.to_string(),
                12.0,
                if left < 0 { RED() } else { MUTED() },
            )));
        }
        col.child(boxed)
    }
}

/// ヘッダーの表示切替アイコン（オンのときは白）
fn toggle_icon(id: &'static str, name: IconName, on: bool) -> Stateful<Div> {
    div()
        .id(id)
        .group("ti")
        .size(z(32.0))
        .flex()
        .items_center()
        .justify_center()
        .rounded(z(8.0))
        .cursor_pointer()
        .hover(|s| s.bg(SELECTED()))
        .child(
            icon(name, 20.0, if on { WHITE() } else { MUTED() })
                .group_hover("ti", |s| s.text_color(WHITE())),
        )
}

/// 通話画面の丸いボタン
fn round_button(id: &'static str, name: IconName, bg: Color32, fg: Color32) -> Stateful<Div> {
    div()
        .id(id)
        .size(z(56.0))
        .rounded_full()
        .bg(bg)
        .hover(move |s| s.bg(bg.lerp_to_gamma(Color32::WHITE, 0.12)))
        .flex()
        .items_center()
        .justify_center()
        .cursor_pointer()
        .child(icon(name, 24.0, fg))
}

#[cfg(test)]
mod tests {
    use super::{px, sync_list, FollowMode, ListAlignment, ListOffset, ListState, MsgList};

    #[test]
    fn sync_list_preserves_anchor_when_older_prepended_and_first_row_key_changes() {
        // [先頭(10), m1, m2, m3, 末尾(1)]
        let keys = vec![10, 101, 102, 103, 1];
        let ids = vec![0, 1001, 1002, 1003, 0];
        let state = ListState::new(keys.len(), ListAlignment::Bottom, px(600.0));
        state.set_follow_mode(FollowMode::Tail);
        let mut l = MsgList {
            state,
            keys,
            ids,
            stamp: 0,
            detached: false,
        };
        // 一番上のメッセージを見ているときに古い 2 件が前に入り、m1 の key が変わる（日付の区切りなど）
        l.state.scroll_to(ListOffset {
            item_ix: 1,
            offset_in_item: px(12.0),
        });
        sync_list(
            &mut l,
            vec![10, 91, 92, 101_999, 102, 103, 1],
            vec![0, 901, 902, 1001, 1002, 1003, 0],
        );
        assert_eq!(l.state.logical_scroll_top().item_ix, 3);
        // 先頭の読み込み中表示を見ているときに、さらに 2 件入る
        l.state.scroll_to(ListOffset {
            item_ix: 0,
            offset_in_item: px(0.0),
        });
        sync_list(
            &mut l,
            vec![10, 81, 82, 91_999, 92, 101_999, 102, 103, 1],
            vec![0, 801, 802, 901, 902, 1001, 1002, 1003, 0],
        );
        assert_eq!(l.state.logical_scroll_top().item_ix, 3);
    }

    #[test]
    fn sync_list_keeps_following_tail_for_new_messages() {
        let state = ListState::new(3, ListAlignment::Bottom, px(600.0));
        state.set_follow_mode(FollowMode::Tail);
        let mut l = MsgList {
            state,
            keys: vec![10, 101, 1],
            ids: vec![0, 1001, 0],
            stamp: 0,
            detached: false,
        };
        sync_list(&mut l, vec![10, 101, 102, 1], vec![0, 1001, 1002, 0]);
        assert!(l.state.is_following_tail());
        assert_eq!(l.ids, vec![0, 1001, 1002, 0]);
    }
}
