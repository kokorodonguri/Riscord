//! 左側: タイトルバー・サーバーレール・チャンネル欄・左下のユーザー欄
use gpui::assets::IconName;
use gpui::component::input::Input;
use gpui::prelude::*;
use gpui::*;

use super::*;
use crate::app::{dm_icon_url, guild_icon_url, status_label, user_avatar_url};
use crate::color::Color32;
use crate::model::{Channel, Presence, User};

/// 行の枠（選択中・マウスを乗せたときの背景、未読の白い印）。文字色は group "row" のホバーで変える
fn row_frame(id: impl Into<ElementId>, height: f32, selected: bool, _unread: bool) -> Stateful<Div> {
    let mut d = div()
        .id(id)
        .group("row")
        .relative()
        .h(z(height))
        .mx(z(8.0))
        .flex_none()
        .rounded(z(8.0))
        .flex()
        .items_center()
        .cursor_pointer()
        .overflow_hidden();
    d = if selected { d.bg(SELECTED()) } else { d.hover(|s| s.bg(HOVER())) };
    d
}

/// 未読の印（行の左端の白い小さな縦棒）
fn unread_pill(height: f32) -> Div {
    div().absolute().left(z(-8.0)).top(z(height / 2.0 - 4.0)).w(z(4.0)).h(z(8.0)).rounded_r(z(4.0)).bg(WHITE())
}

fn row_color(selected: bool, unread: bool) -> Color32 {
    if selected || unread {
        WHITE()
    } else {
        MUTED()
    }
}

impl Root {
    /// 最上段: 戻る/進むと現在地。何もない所をつかむとウィンドウを動かせる
    pub fn titlebar(&mut self, compact: bool, cx: &mut Context<Self>) -> impl IntoElement {
        let a = &self.app;
        let mut left = div().flex().items_center().gap(z(4.0));
        for (back, name, label) in [(true, IconName::ArrowLeft, "戻る"), (false, IconName::ArrowRight, "進む")] {
            let enabled = if back { !a.nav_back.is_empty() } else { !a.nav_fwd.is_empty() };
            let b = div()
                .id(if back { "nav_back" } else { "nav_fwd" })
                .group("nav")
                .size(z(26.0))
                .flex()
                .items_center()
                .justify_center()
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .tooltip(tip(label));
            left = left.child(if enabled {
                b.cursor_pointer()
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.navigate(back);
                        cx.notify();
                    }))
                    .child(icon(name, 16.0, MUTED()).group_hover("nav", |s| s.text_color(WHITE())))
            } else {
                b.child(icon(name, 16.0, FAINT().gamma_multiply(0.6)))
            });
        }
        if compact {
            let label = if a.compact_sidebar_closed { "チャンネル一覧を開く" } else { "チャンネル一覧を閉じる" };
            left = left.child(
                icon_button("hamburger", IconName::Menu, 26.0)
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .tooltip(tip(label))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.app.compact_sidebar_closed = !this.app.compact_sidebar_closed;
                        this.app.show_members_narrow = false;
                        cx.notify();
                    })),
            );
        }
        // 中央に現在地（アイコン＋名前）
        let (label, url) = match &a.sel_guild {
            None => ("ダイレクトメッセージ".to_string(), None),
            Some(g) => a.guilds.iter().find(|x| &x.id == g).map(|x| (x.name.clone(), guild_icon_url(x))).unwrap_or_default(),
        };
        let place_icon = if a.sel_guild.is_none() { logo(16.0, MUTED(), BG()) } else { avatar(url.as_deref(), 18.0, &label, 5.0, HOVER()) };
        let center = div()
            .absolute()
            .inset_0()
            .when(compact, |d| d.pl(z(100.0)))
            .flex()
            .items_center()
            .justify_center()
            .child(div().flex().items_center().gap(z(8.0)).max_w(z((a.screen_w - 140.0).max(0.0))).child(place_icon).child(line(label, 14.0, TEXT()).font_weight(FontWeight::MEDIUM)));
        div()
            .id("titlebar")
            .relative()
            .h(z(36.0))
            .flex_none()
            .px(z(10.0))
            .bg(BG())
            .flex()
            .items_center()
            .on_mouse_down(MouseButton::Left, |ev, window, _| {
                if ev.click_count >= 2 {
                    window.zoom_window();
                } else {
                    window.start_window_move();
                }
            })
            .child(center)
            .child(left)
    }

    /// 左列（サーバーレール＋チャンネル欄。下にユーザー欄がまたがる）
    pub fn sidebar(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Div {
        div()
            .w(z(312.0))
            .h_full()
            .flex_none()
            .flex()
            .flex_col()
            .bg(BG())
            .child(div().flex_1().min_h_0().flex().flex_row().child(self.rail(cx)).child(self.channel_pane(window, cx)))
            .child(self.userbar(cx))
    }

    fn rail_item(&self, id: impl Into<ElementId>, selected: bool, unread: bool, name: &str, url: Option<&str>) -> Stateful<Div> {
        let bg = if selected { BLURPLE() } else { HOVER() };
        let face = if name.is_empty() {
            div()
                .size(z(40.0))
                .rounded(z(12.0))
                .bg(bg)
                .group_hover("rail", |s| s.bg(BLURPLE()))
                .flex()
                .items_center()
                .justify_center()
                .child(logo(22.0, WHITE(), bg))
        } else {
            avatar(url, 40.0, name, 12.0, bg).group_hover("rail", |s| s.bg(BLURPLE()))
        };
        let pill_h = if selected {
            40.0
        } else if unread {
            8.0
        } else {
            0.0
        };
        let pill = div()
            .absolute()
            .left_0()
            .w(z(4.0))
            .h(z(pill_h))
            .rounded_r(z(4.0))
            .bg(WHITE())
            .when(!selected, |d| d.group_hover("rail", |s| s.h(z(20.0))));
        let tip_text = if name.is_empty() { "ダイレクトメッセージ".to_string() } else { name.to_string() };
        div()
            .id(id)
            .group("rail")
            .relative()
            .w(z(72.0))
            .h(z(48.0))
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .cursor_pointer()
            .tooltip(tip(tip_text))
            .child(pill)
            .child(face)
    }

    /// サーバーレール
    fn rail(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let a = &self.app;
        let unread = a.unread_guilds();
        let mut col = div().id("rail").w(z(72.0)).h_full().flex_none().overflow_y_scroll().flex().flex_col().gap(z(4.0)).pt(z(2.0)).pb(z(8.0));
        col = col.child(self.rail_item("rail_dm", a.sel_guild.is_none(), unread.contains(&None), "", None).on_click(cx.listener(|this, _, _, cx| {
            this.pick_guild(None);
            cx.notify();
        })));
        col = col.child(div().h(z(6.0)).flex_none().flex().items_center().justify_center().child(div().w(z(32.0)).h(px(1.0)).bg(DIVIDER())));
        for g in &a.guilds {
            let sel = a.sel_guild.as_deref() == Some(&g.id);
            let url = guild_icon_url(g);
            let gid = g.id.clone();
            col = col.child(
                self.rail_item(SharedString::from(format!("g{}", g.id)), sel, unread.contains(&Some(g.id.as_str())), &g.name, url.as_deref())
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.pick_guild(Some(gid.clone()));
                        cx.notify();
                    })),
            );
        }
        col
    }

    /// チャンネル欄（左上だけ丸い内容エリアの縁。右側はチャットとの境目として画面下まで）
    fn channel_pane(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let a = &self.app;
        let head = match &a.sel_guild {
            None if a.dm_search => div().w_full().bg(INPUT()).border_1().border_color(DIVIDER()).rounded(z(8.0)).px(z(6.0)).child(Input::new(&self.inp.dm_filter).appearance(false).h(z(30.0))),
            None => div().w_full().child(
                div()
                    .id("dm_search_btn")
                    .h(z(30.0))
                    .w_full()
                    .rounded(z(8.0))
                    .bg(HOVER())
                    .hover(|s| s.bg(SELECTED()))
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_size(z(14.0))
                    .text_color(MUTED())
                    .cursor(CursorStyle::IBeam)
                    .child("会話に参加または作成する")
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.app.dm_search = true;
                        this.inp.dm_filter.update(cx, |s, cx| s.focus(window, cx));
                        cx.notify();
                    })),
            ),
            Some(g) => {
                let title = a.guilds.iter().find(|x| &x.id == g).map(|x| x.name.clone()).unwrap_or_default();
                div().pl(z(7.0)).w_full().child(line(title, 16.0, WHITE()).font_weight(FontWeight::MEDIUM))
            }
        };
        let head = div().h(z(48.0)).flex_none().flex().items_center().pl(z(9.0)).pr(z(8.0)).border_b_1().border_color(DIVIDER()).child(head);

        let mut list = div().id("channels").flex_1().min_h_0().overflow_y_scroll().flex().flex_col().gap(px(1.0)).pt(z(8.0)).pb(z(8.0));
        match &a.sel_guild {
            None => {
                list = list.child(div().h(z(28.0)).flex_none().flex().items_center().pl(z(16.0)).child(text("ダイレクトメッセージ", 13.0, FAINT())));
                let q = a.dm_filter.trim().to_lowercase();
                let mut shown = 0;
                for c in a.dms.iter().filter(|c| q.is_empty() || c.dm_label().to_lowercase().contains(&q)) {
                    shown += 1;
                    let sel = a.sel_channel.as_deref() == Some(&c.id);
                    let pres = c.recipients.first().and_then(|u| a.presences.get(&u.id));
                    let id = c.id.clone();
                    list = list.child(dm_row(c, sel, a.unread.contains_key(&c.id), pres).on_click(cx.listener(move |this, _, _, cx| {
                        this.open_channel(id.clone());
                        cx.notify();
                    })));
                }
                if shown == 0 && !q.is_empty() {
                    list = list.child(div().mt(z(12.0)).flex().justify_center().child(text("見つかりませんでした", 13.0, FAINT())));
                } else if a.dms.is_empty() {
                    // 一覧が届く前は読み込み中、届いて 0 件ならその旨を出す（何も無い画面にしない）
                    list = list.child(div().mt(z(12.0)).flex().justify_center().map(|d| {
                        if a.dms_loaded {
                            d.child(text("ダイレクトメッセージはまだありません", 13.0, FAINT()))
                        } else {
                            d.child(spinner(20.0))
                        }
                    }));
                }
            }
            Some(gid) => match a.channels.get(gid) {
                None => list = list.child(div().flex().justify_center().child(spinner(20.0))),
                Some(chs) => {
                    let cur_voice = a.voice_channel().map(|(_, c, _)| c.clone());
                    // chs は受信時に表示順へ並べ済み。親ごとに1回の走査で振り分ける
                    let mut top: Vec<&Channel> = Vec::new();
                    let mut by_parent: HashMap<&str, Vec<&Channel>> = HashMap::new();
                    let mut cats: Vec<&Channel> = Vec::new();
                    for c in chs {
                        if c.kind == Channel::CATEGORY {
                            cats.push(c);
                        } else if c.is_text() || c.is_voice() {
                            match c.parent_id.as_deref() {
                                None => top.push(c),
                                Some(p) => by_parent.entry(p).or_default().push(c),
                            }
                        }
                    }
                    cats.sort_by_key(|c| c.position);
                    let mut rows: Vec<AnyElement> = Vec::new();
                    if top.is_empty() && by_parent.is_empty() {
                        rows.push(div().mt(z(12.0)).flex().justify_center().child(text("表示できるチャンネルがありません", 13.0, FAINT())).into_any_element());
                    }
                    for c in top {
                        self.channel_rows(c, gid, cur_voice.as_deref(), &mut rows, cx);
                    }
                    for cat in cats {
                        let Some(l) = by_parent.get(cat.id.as_str()) else { continue };
                        rows.push(
                            div()
                                .mt(z(14.0))
                                .h(z(20.0))
                                .flex_none()
                                .pl(z(16.0))
                                .flex()
                                .items_center()
                                .child(line(cat.name.as_deref().unwrap_or("").to_uppercase(), 12.0, FAINT()))
                                .into_any_element(),
                        );
                        for c in l {
                            self.channel_rows(c, gid, cur_voice.as_deref(), &mut rows, cx);
                        }
                    }
                    list = list.children(rows);
                }
            },
        }
        div()
            .flex_1()
            .min_w_0()
            .h_full()
            .flex()
            .flex_col()
            .border_l_1()
            .border_t_1()
            .border_r_1()
            .border_color(DIVIDER())
            .rounded_tl(z(8.0))
            .child(head)
            .child(list)
    }

    /// チャンネル1つ分の行（ボイスなら参加者の行も）
    fn channel_rows(&self, c: &Channel, gid: &str, cur_voice: Option<&str>, rows: &mut Vec<AnyElement>, cx: &mut Context<Self>) {
        let a = &self.app;
        let name = c.name.clone().unwrap_or_default();
        let id = c.id.clone();
        if c.is_voice() {
            let here = cur_voice == Some(c.id.as_str());
            let gid = gid.to_string();
            rows.push(
                row_frame(SharedString::from(format!("ch{}", c.id)), 34.0, here, false)
                    .tooltip(tip(if here { "接続しています" } else { "クリックで参加" }))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if !here {
                            this.app.join_voice(Some(gid.clone()), id.clone(), false);
                            cx.notify();
                        }
                    }))
                    .child(div().w(z(34.0)).flex().justify_center().child(icon(IconName::Volume2, 18.0, MUTED())))
                    .child(line(name, 15.0, row_color(here, false)).group_hover("row", |s| s.text_color(TEXT())))
                    .into_any_element(),
            );
            for v in a.voice_members(&c.id) {
                let u = v.user.clone().or_else(|| a.find_user(&v.user_id));
                rows.push(voice_member_row(u.as_ref(), &v.user_id, a.is_speaking(&v.user_id), v.self_mute, v.self_deaf).into_any_element());
            }
            return;
        }
        let sel = a.sel_channel.as_deref() == Some(&c.id);
        let unread = a.unread.contains_key(&c.id);
        rows.push(
            row_frame(SharedString::from(format!("ch{}", c.id)), 32.0, sel, unread)
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.open_channel(id.clone());
                    cx.notify();
                }))
                .when(unread && !sel, |d| d.child(unread_pill(32.0)))
                .child(div().w(z(30.0)).pl(z(8.0)).text_size(z(20.0)).text_color(FAINT()).child("#"))
                .child(line(name, 16.0, row_color(sel, unread)).font_weight(FontWeight::MEDIUM).group_hover("row", |s| s.text_color(if sel || unread { WHITE() } else { TEXT() })))
                .into_any_element(),
        );
    }

    /// 左下のユーザー欄（ボイス接続中はその上に接続の状態）
    fn userbar(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let a = &self.app;
        let mut card = div().w_full().bg(USERBAR()).border_1().border_color(DIVIDER()).rounded(z(8.0)).px(z(8.0)).py(z(6.0)).flex().flex_col();
        if let Some((gid, cid, connected)) = a.voice_channel().map(|(g, c, k)| (g.clone(), c.clone(), k)) {
            let (cname, place) = a.voice_place(gid.as_deref(), &cid);
            let color = if connected { GREEN() } else { ORANGE() };
            card = card
                .child(
                    div()
                        .h(z(44.0))
                        .flex()
                        .items_center()
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .flex()
                                .flex_col()
                                .child(div().flex().items_center().gap(z(6.0)).child(icon(IconName::SignalHigh, 16.0, color)).child(medium(if connected { "ボイス接続済み" } else { "接続中…" }, 14.0, color)))
                                .child(
                                    div()
                                        .id("voice_place")
                                        .cursor_pointer()
                                        .min_w_0()
                                        .hover(|s| s.underline())
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            let before = (this.app.sel_guild.clone(), this.app.sel_channel.clone());
                                            let a = &mut this.app;
                                            if gid != a.sel_guild {
                                                if let Some(g) = &gid {
                                                    if !a.channels.contains_key(g) || !a.roles.contains_key(g) {
                                                        a.send(crate::net::Cmd::LoadGuildChannels(g.clone()));
                                                    }
                                                }
                                                a.sel_guild = gid.clone();
                                                a.sel_channel = None;
                                            }
                                            if gid.is_none() {
                                                a.open_channel(cid.clone());
                                            }
                                            this.after_move(before);
                                            cx.notify();
                                        }))
                                        .child(line(format!("{cname} / {place}"), 12.0, MUTED())),
                                ),
                        )
                        .child(icon_button("hangup", IconName::PhoneOff, 32.0).tooltip(tip("切断")).on_click(cx.listener(|this, _, _, cx| {
                            this.app.leave_voice();
                            cx.notify();
                        }))),
                )
                .child(div().my(z(6.0)).child(hline(DIVIDER())));
        }
        let mut row = div().h(z(40.0)).w_full().flex().items_center();
        if let Some(me) = &a.me {
            let cur = a.settings.status.clone();
            let opened = a.popup.as_ref().is_some_and(|p| p.user.id == me.id);
            let me2 = me.clone();
            // アバター・名前・ステータスをまとめて1つのボタンにする（本家と同じく、どこを押しても自分のプロフィールが開く）
            row = row.child(
                div()
                    .id("userbar_me")
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .items_center()
                    .gap(z(8.0))
                    .px(z(4.0))
                    .py(z(2.0))
                    .rounded(z(6.0))
                    .cursor_pointer()
                    .when(opened, |d| d.bg(HOVER()))
                    .hover(|s| s.bg(HOVER()))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(cx.listener(move |this, _, window, cx| {
                        let p = window.mouse_position();
                        if this.app.popup.as_ref().is_some_and(|p| p.user.id == me2.id) {
                            this.app.popup = None;
                        } else {
                            this.app.open_profile(me2.clone(), Bounds::new(p, size(px(0.), px(0.))), true);
                        }
                        cx.notify();
                    }))
                    .child(with_status(avatar(Some(&user_avatar_url(me)), 32.0, me.display(), 16.0, BLURPLE()), 32.0, &cur, USERBAR()))
                    .child(div().flex_1().min_w_0().flex().flex_col().child(line(me.display().to_string(), 14.0, WHITE()).font_weight(FontWeight::MEDIUM)).child(line(status_label(&cur), 12.0, MUTED()))),
            );
        }
        let mic_off = a.mic_off();
        let deaf = a.settings.self_deaf;
        row = row
            .child(self.toggle_with_menu("mic", IconName::Mic, IconName::MicOff, mic_off, if mic_off { "ミュート解除" } else { "ミュート" }, "入力オプション", PopMenu::Mic, cx))
            .child(self.toggle_with_menu("head", IconName::Headphones, IconName::HeadphoneOff, deaf, if deaf { "スピーカーミュート解除" } else { "スピーカーミュート" }, "出力オプション", PopMenu::Headset, cx))
            .child(icon_button("settings_btn", IconName::Settings, 32.0).tooltip(tip("ユーザー設定")).on_click(cx.listener(|this, _, _, cx| {
                this.app.settings_open = true;
                cx.notify();
            })));
        div().flex_none().bg(BG()).pl(z(8.0)).pr(z(8.0)).pt(z(6.0)).pb(z(8.0)).child(card.child(row))
    }

    /// 左下のマイク・ヘッドホンのような「アイコン＋⌄」の組
    #[allow(clippy::too_many_arguments)]
    fn toggle_with_menu(&self, id: &'static str, on_icon: IconName, off_icon: IconName, off: bool, main_tip: &'static str, menu_tip: &'static str, menu: PopMenu, cx: &mut Context<Self>) -> impl IntoElement {
        let color = if off { RED() } else { TEXT() };
        let hover_bg = if off { Color32::from_rgba_unmultiplied(0xda, 0x37, 0x3c, 70) } else { SELECTED() };
        let is_mic = menu == PopMenu::Mic;
        let main = div()
            .id(SharedString::from(format!("{id}_main")))
            .group("tg")
            .w(z(34.0))
            .h(z(32.0))
            .flex()
            .items_center()
            .justify_center()
            .rounded(z(6.0))
            .cursor_pointer()
            .hover(move |s| s.bg(hover_bg))
            .tooltip(tip(main_tip))
            .on_click(cx.listener(move |this, _, _, cx| {
                if is_mic {
                    this.app.toggle_mute();
                } else {
                    this.app.toggle_deaf();
                }
                cx.notify();
            }))
            .child(icon(if off { off_icon } else { on_icon }, 18.0, color).group_hover("tg", |s| s.text_color(if off { RED() } else { WHITE() })));
        let chev = div()
            .id(SharedString::from(format!("{id}_menu")))
            .group("tc")
            .w(z(18.0))
            .h(z(32.0))
            .flex()
            .items_center()
            .justify_center()
            .rounded(z(6.0))
            .cursor_pointer()
            .hover(move |s| s.bg(hover_bg))
            .tooltip(tip(menu_tip))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(cx.listener(move |this, _, window, cx| {
                this.menu = if this.menu.is_some_and(|m| m.0 == menu) { None } else { Some((menu, window.mouse_position())) };
                this.app.audio_devs = None;
                cx.notify();
            }))
            .child(icon(IconName::ChevronDown, 12.0, color).group_hover("tc", |s| s.text_color(WHITE())));
        div()
            .w(z(52.0))
            .h(z(32.0))
            .flex_none()
            .flex()
            .rounded(z(6.0))
            .when(off, |d| d.bg(Color32::from_rgba_unmultiplied(0xda, 0x37, 0x3c, 40)))
            .child(main)
            .child(chev)
    }
}

/// DM の行（相手のアイコン・状態・名前・アクティビティ）
fn dm_row(c: &Channel, selected: bool, unread: bool, presence: Option<&Presence>) -> Stateful<Div> {
    let label = c.dm_label();
    let one_to_one = c.recipients.len() == 1;
    let av = avatar(dm_icon_url(c).as_deref(), 32.0, &label, 16.0, BLURPLE());
    // 1対1のDMだけ相手の状態を出す（情報が無ければオフライン扱い）
    let av = if one_to_one {
        with_status(av, 32.0, presence.map_or("offline", |p| p.status.as_str()), if selected { SELECTED() } else { BG() })
    } else {
        av
    };
    let color = row_color(selected, unread);
    let name = line(label, 16.0, color).group_hover("row", move |s| s.text_color(if selected || unread { WHITE() } else { TEXT() }));
    let names = match presence.and_then(|p| p.activity.clone()).filter(|_| one_to_one) {
        Some(act) => div().flex_1().min_w_0().flex().flex_col().child(name).child(line(act, 12.0, MUTED())),
        None => div().flex_1().min_w_0().child(name),
    };
    let mut row = row_frame(SharedString::from(format!("dm{}", c.id)), 42.0, selected, unread).gap(z(12.0)).pl(z(8.0)).child(av).child(names);
    if unread && !selected {
        row = row.child(unread_pill(42.0));
    }
    if one_to_one {
        row = row.tooltip(tip(status_label(presence.map_or("offline", |p| p.status.as_str()))));
    }
    row
}

/// 話しているときの緑の輪付きアバター
pub fn speaking_avatar(u: Option<&User>, fallback: &str, size: f32, speaking: bool) -> Div {
    let url = u.map(user_avatar_url);
    let name = u.map(|u| u.display()).unwrap_or(fallback);
    let av = avatar(url.as_deref(), size, name, size / 2.0, BLURPLE());
    let w = (size * 0.06).max(2.0);
    div()
        .flex_none()
        .size(z(size + w * 4.0))
        .rounded_full()
        .flex()
        .items_center()
        .justify_center()
        .when(speaking, |d| d.border(z(w)).border_color(GREEN()))
        .child(av)
}

/// ボイスチャンネル内の参加者の行
fn voice_member_row(u: Option<&User>, id: &str, speaking: bool, mute: bool, deaf: bool) -> Div {
    let name = u.map(|u| u.display().to_string()).unwrap_or_else(|| id.to_string());
    let mut right = div().flex().items_center().gap(z(4.0));
    if deaf {
        right = right.child(icon(IconName::HeadphoneOff, 14.0, MUTED()));
    }
    if mute || deaf {
        right = right.child(icon(IconName::MicOff, 14.0, MUTED()));
    }
    div()
        .h(z(30.0))
        .flex_none()
        .ml(z(36.0))
        .mr(z(8.0))
        .px(z(6.0))
        .rounded(z(4.0))
        .flex()
        .items_center()
        .gap(z(8.0))
        .hover(|s| s.bg(HOVER()))
        .child(speaking_avatar(u, id, 22.0, speaking))
        .child(line(name, 14.0, if speaking { WHITE() } else { MUTED() }).flex_1())
        .child(right)
}
