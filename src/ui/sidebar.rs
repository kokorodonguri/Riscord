//! 左側: タイトルバー・サーバーレール・チャンネル欄・左下のユーザー欄
use gpui::assets::IconName;
use gpui::component::input::Input;
use gpui::prelude::*;
use gpui::*;

use super::*;
use crate::app::{dm_icon_url, guild_icon_url, status_label, user_avatar_url};
use crate::color::Color32;
use crate::model::{Channel, Presence, User};

/// ドラッグ中のサーバー。指の下にはアイコンだけを出す
#[derive(Clone)]
struct GuildDrag {
    id: String,
    name: String,
    url: Option<String>,
}

impl Render for GuildDrag {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        avatar(self.url.as_deref(), 40.0, &self.name, 12.0, HOVER()).opacity(0.8)
    }
}

#[derive(Clone)]
struct FolderDrag {
    id: String,
    name: String,
}
impl Render for FolderDrag {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .p(z(8.0))
            .rounded(z(8.0))
            .bg(BLURPLE())
            .child(text(self.name.clone(), 14.0, WHITE()))
            .opacity(0.8)
    }
}

/// 行の枠（選択中・マウスを乗せたときの背景、未読の白い印）。文字色は group "row" のホバーで変える
fn row_frame(
    id: impl Into<ElementId>,
    height: f32,
    selected: bool,
    _unread: bool,
) -> Stateful<Div> {
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
    if selected {
        d.bg(SELECTED()).part(Part::SidebarRowSelected)
    } else {
        d = d.hover(|s| s.bg(part_hover(Part::SidebarRow, HOVER())));
        d.part(Part::SidebarRow)
    }
}

/// 未読の印（行の左端の白い小さな縦棒）
fn unread_pill(height: f32) -> Div {
    div()
        .absolute()
        .left(z(-8.0))
        .top(z(height / 2.0 - 4.0))
        .w(z(4.0))
        .h(z(8.0))
        .rounded_r(z(4.0))
        .bg(WHITE())
}

fn row_color(selected: bool, unread: bool) -> Color32 {
    if selected {
        part_text(Part::SidebarRowSelected, WHITE())
    } else if unread {
        WHITE()
    } else {
        part_text(Part::SidebarRow, MUTED())
    }
}

impl Root {
    /// 最上段: 戻る/進むと現在地。何もない所をつかむとウィンドウを動かせる
    pub fn titlebar(&mut self, compact: bool, cx: &mut Context<Self>) -> impl IntoElement {
        let a = &self.app;
        let mut left = div().flex().items_center().gap(z(4.0));
        for (back, name, label) in [
            (true, IconName::ArrowLeft, tr!("戻る", "Back", "返回")),
            (false, IconName::ArrowRight, tr!("進む", "Forward", "前进")),
        ] {
            let enabled = if back {
                !a.nav_back.is_empty()
            } else {
                !a.nav_fwd.is_empty()
            };
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
            let label = if a.compact_sidebar_closed {
                tr!("チャンネル一覧を開く", "Open channel list", "打开频道列表")
            } else {
                tr!(
                    "チャンネル一覧を閉じる",
                    "Close channel list",
                    "关闭频道列表"
                )
            };
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
            None => (
                tr!("ダイレクトメッセージ", "Direct Messages", "私信").to_string(),
                None,
            ),
            Some(g) => a
                .guilds
                .iter()
                .find(|x| &x.id == g)
                .map(|x| (x.name.clone(), guild_icon_url(x)))
                .unwrap_or_default(),
        };
        let place_icon = if a.sel_guild.is_none() {
            logo(16.0, MUTED(), BG())
        } else {
            avatar(url.as_deref(), 18.0, &label, 5.0, HOVER())
        };
        let center = div()
            .absolute()
            .inset_0()
            .when(compact, |d| d.pl(z(100.0)))
            .flex()
            .items_center()
            .justify_center()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(z(8.0))
                    .max_w(z((a.screen_w - 140.0).max(0.0)))
                    .child(place_icon)
                    .child(line(label, 14.0, TEXT()).font_weight(FontWeight::MEDIUM)),
            );
        div()
            .id("titlebar")
            .relative()
            .h(z(36.0))
            .flex_none()
            .px(z(10.0))
            .bg(BG())
            .part(Part::Titlebar)
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
        let (ch_w, _) = self.app.pane_widths();
        div()
            .w(z(st::RAIL_W + ch_w))
            .h_full()
            .flex_none()
            .relative()
            .flex()
            .flex_col()
            .bg(BG())
            .part(Part::SidebarPane)
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .flex_row()
                    .child(self.rail(cx))
                    .child(self.channel_pane(window, cx)),
            )
            .child(self.userbar(cx))
            .child(self.pane_resizer(ResizeTarget::Channels, cx))
    }

    /// round = 丸いアイコン（未読の DM の相手）。サーバーはテーマの形に従う
    fn rail_item(
        id: impl Into<ElementId>,
        selected: bool,
        unread: bool,
        mentions: u32,
        name: &str,
        url: Option<&str>,
        round: bool,
    ) -> Stateful<Div> {
        let bg = if selected { BLURPLE() } else { HOVER() };
        // テーマで変えられるのは形（radius）と枠。地は選択・ホバーで変わるのでそのまま
        let radius = if round {
            20.0
        } else {
            part_style(Part::SidebarServer).radius.unwrap_or(12.0)
        };
        let face = if name.is_empty() {
            div()
                .size(z(40.0))
                .rounded(z(radius))
                .bg(bg)
                .group_hover("rail", |s| s.bg(BLURPLE()))
                .flex()
                .items_center()
                .justify_center()
                .child(logo(22.0, WHITE(), bg))
        } else {
            avatar(url, 40.0, name, radius, bg).group_hover("rail", |s| s.bg(BLURPLE()))
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
        let tip_text = if name.is_empty() {
            tr!("ダイレクトメッセージ", "Direct Messages", "私信").to_string()
        } else {
            name.to_string()
        };
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
            .when(mentions > 0, |d| d.child(mention_badge(mentions)))
    }

    /// サーバーレール
    fn rail(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let a = &self.app;
        let unread = a.unread_guilds();
        let mut col = div()
            .id("rail")
            .w(z(72.0))
            .h_full()
            .flex_none()
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .gap(z(4.0))
            .pt(z(2.0))
            .pb(z(8.0))
            .part(Part::SidebarRail);
        // 未読の DM は、相手のアイコンを DM ボタンの下に並べて件数を出す（本家と同じ）。
        // DM ボタンの数字は、アイコンにできなかった分（DM 一覧が届く前など）だけ
        let unread_dms = a.unread_dms();
        let shown: u32 = unread_dms.iter().map(|(_, n)| n).sum();
        col = col.child(
            Self::rail_item(
                "rail_dm",
                a.sel_guild.is_none(),
                unread.contains(&None),
                a.mention_count(None).saturating_sub(shown),
                "",
                None,
                false,
            )
            .on_click(cx.listener(|this, _, _, cx| {
                this.pick_guild(None);
                cx.notify();
            })),
        );
        for (c, n) in unread_dms {
            let cid = c.id.clone();
            col = col.child(
                Self::rail_item(
                    SharedString::from(format!("rdm{}", c.id)),
                    false,
                    true,
                    n,
                    &c.dm_label(),
                    dm_icon_url(c).as_deref(),
                    true,
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.pick_guild(None);
                    this.open_channel(cid.clone());
                    cx.notify();
                })),
            );
        }
        col = col.child(
            div()
                .h(z(6.0))
                .flex_none()
                .flex()
                .items_center()
                .justify_center()
                .child(div().w(z(32.0)).h(px(1.0)).bg(DIVIDER())),
        );
        enum Entry<'a> {
            Folder(&'a crate::app::GuildFolder),
            Guild(&'a crate::model::Guild),
        }
        let folders = &a.settings.guild_folders;
        let mut membership = std::collections::HashMap::new();
        for (index, folder) in folders.iter().enumerate() {
            for id in &folder.guild_ids {
                membership.entry(id.as_str()).or_insert(index);
            }
        }
        let mut members = vec![Vec::new(); folders.len()];
        for g in &a.guilds {
            if let Some(&index) = membership.get(g.id.as_str()) {
                members[index].push(g);
            }
        }
        let mut entries = Vec::new();
        let mut seen = vec![false; folders.len()];
        for g in &a.guilds {
            if let Some(&index) = membership.get(g.id.as_str()) {
                if !seen[index] {
                    seen[index] = true;
                    entries.push(Entry::Folder(&folders[index]));
                    if !folders[index].collapsed {
                        entries.extend(members[index].iter().copied().map(Entry::Guild));
                    }
                }
            } else {
                entries.push(Entry::Guild(g));
            }
        }
        for (index, folder) in folders.iter().enumerate() {
            if !seen[index] {
                entries.push(Entry::Folder(folder));
            }
        }
        for entry in entries {
            let g = match entry {
                Entry::Guild(g) => g,
                Entry::Folder(folder) => {
                    let fid = folder.id.clone();
                    let drop_id = fid.clone();
                    let menu_id = fid.clone();
                    let anchor = a
                        .guilds
                        .iter()
                        .find(|g| folder.guild_ids.contains(&g.id))
                        .map(|g| g.id.clone());
                    let folder_drag = FolderDrag {
                        id: fid.clone(),
                        name: folder.name.clone(),
                    };
                    let color = folder.color & 0xffffff;
                    let bg =
                        Color32::from_rgb((color >> 16) as u8, (color >> 8) as u8, color as u8);
                    let label = if folder.name.is_empty() {
                        tr!("フォルダ", "Folder", "文件夹")
                    } else {
                        &folder.name
                    };
                    let count = folder
                        .guild_ids
                        .iter()
                        .fold(0u32, |n, id| n.saturating_add(a.mention_count(Some(id))));
                    let has_unread = folder
                        .guild_ids
                        .iter()
                        .any(|id| unread.contains(&Some(id.as_str())));
                    col = col.child(
                        div()
                            .id(SharedString::from(format!("folder-{}", folder.id)))
                            .relative()
                            .w(z(72.0))
                            .h(z(48.0))
                            .flex_none()
                            .flex()
                            .items_center()
                            .justify_center()
                            .cursor_pointer()
                            .tooltip(tip(label.to_string()))
                            .child(
                                div()
                                    .size(z(40.0))
                                    .rounded(z(12.0))
                                    .bg(bg)
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .child(text(
                                        if folder.collapsed { "▸" } else { "▾" },
                                        22.0,
                                        WHITE(),
                                    )),
                            )
                            .when(count > 0, |d| d.child(mention_badge(count)))
                            .when(count == 0 && has_unread, |d| {
                                d.child(
                                    div()
                                        .absolute()
                                        .left_0()
                                        .w(z(4.0))
                                        .h(z(8.0))
                                        .rounded_r(z(4.0))
                                        .bg(WHITE()),
                                )
                            })
                            .on_drag(folder_drag, |d, _, _, cx| cx.new(|_| d.clone()))
                            .on_drop(cx.listener(move |this, d: &FolderDrag, _, cx| {
                                if let Some(target) = &anchor {
                                    if this.app.move_folder_to_guild(&d.id, target) {
                                        this.app.save_settings();
                                    }
                                }
                                cx.notify();
                            }))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if let Some(f) = this
                                    .app
                                    .settings
                                    .guild_folders
                                    .iter_mut()
                                    .find(|f| f.id == fid)
                                {
                                    f.collapsed = !f.collapsed;
                                    this.app.save_settings();
                                }
                                cx.notify();
                            }))
                            .on_drop(cx.listener(move |this, d: &GuildDrag, _, cx| {
                                if this.app.move_guild_to_folder(&d.id, &drop_id) {
                                    this.app.save_settings();
                                }
                                cx.notify();
                            }))
                            .on_mouse_down(
                                MouseButton::Right,
                                cx.listener(move |this, ev: &MouseDownEvent, _, cx| {
                                    this.menu = Some((PopMenu::Folder, ev.position));
                                    this.menu_folder = Some(menu_id.clone());
                                    cx.stop_propagation();
                                    cx.notify();
                                }),
                            ),
                    );
                    continue;
                }
            };
            let sel = a.sel_guild.as_deref() == Some(&g.id);
            let url = guild_icon_url(g);
            let gid = g.id.clone();
            let gid2 = gid.clone();
            let gid3 = gid.clone();
            let gid4 = gid.clone();
            let drag = GuildDrag {
                id: gid.clone(),
                name: g.name.clone(),
                url: url.clone(),
            };
            col = col.child(
                Self::rail_item(
                    SharedString::from(format!("g{}", g.id)),
                    sel,
                    unread.contains(&Some(g.id.as_str())),
                    a.mention_count(Some(&g.id)),
                    &g.name,
                    url.as_deref(),
                    false,
                )
                // ドラッグして別のサーバーの上で離すと、そこへ動かす（並びは settings.json に残す）
                .on_drag(drag, |d, _, _, cx| cx.new(|_| d.clone()))
                .drag_over::<GuildDrag>(|s, _, _, _| s.bg(HOVER()).rounded(z(12.0)))
                .on_drop(cx.listener(move |this, d: &GuildDrag, _, cx| {
                    if this.app.drop_guild_on_guild(&d.id, &gid3) {
                        this.app.save_settings();
                    }
                    cx.notify();
                }))
                .on_drop(cx.listener(move |this, d: &FolderDrag, _, cx| {
                    if this.app.move_folder_to_guild(&d.id, &gid4) {
                        this.app.save_settings();
                    }
                    cx.notify();
                }))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.pick_guild(Some(gid.clone()));
                    cx.notify();
                }))
                .on_mouse_down(
                    MouseButton::Right,
                    cx.listener(move |this, ev: &MouseDownEvent, _, cx| {
                        this.menu = Some((PopMenu::Guild, ev.position));
                        this.menu_guild = Some(gid2.clone());
                        this.menu_from_header = false;
                        this.guild_notify_expanded = false;
                        this.app.request_guild_boost(&gid2);
                        this.app.ensure_guild_channels(&gid2);
                        cx.stop_propagation();
                        cx.notify();
                    }),
                ),
            );
        }
        col
    }

    /// チャンネル欄（左上だけ丸い内容エリアの縁。右側はチャットとの境目として画面下まで）
    fn channel_pane(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let a = &self.app;
        let head = match &a.sel_guild {
            None if a.dm_search => div()
                .w_full()
                .bg(INPUT())
                .border_1()
                .border_color(DIVIDER())
                .rounded(z(8.0))
                .px(z(6.0))
                .child(Input::new(&self.inp.dm_filter).appearance(false).h(z(30.0))),
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
                    .child(tr!(
                        "会話に参加または作成する",
                        "Find or start a conversation",
                        "查找或发起对话"
                    ))
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.app.dm_search = true;
                        this.inp.dm_filter.update(cx, |s, cx| s.focus(window, cx));
                        cx.notify();
                    })),
            ),
            Some(g) => {
                let title = a
                    .guilds
                    .iter()
                    .find(|x| &x.id == g)
                    .map(|x| x.name.clone())
                    .unwrap_or_default();
                // 押すとサーバーのメニュー（公式クライアントと同じく見出しの下に開く）
                let open = self.menu.is_some_and(|(m, _)| m == PopMenu::Guild)
                    && self.menu_from_header
                    && self.menu_guild.as_ref() == Some(g);
                let (gid, slot, slot2) = (
                    g.clone(),
                    self.guild_header_bounds.clone(),
                    self.guild_header_bounds.clone(),
                );
                div().w_full().child(
                    div()
                        .id("guild_header")
                        .relative()
                        .w_full()
                        .h(z(36.0))
                        .pl(z(7.0))
                        .pr(z(4.0))
                        .rounded(z(6.0))
                        .flex()
                        .items_center()
                        .gap(z(6.0))
                        .cursor_pointer()
                        .hover(|s| s.bg(HOVER()))
                        .when(open, |d| d.bg(HOVER()))
                        .child(
                            line(title, 16.0, WHITE())
                                .font_weight(FontWeight::MEDIUM)
                                .flex_1()
                                .min_w_0(),
                        )
                        .child(icon(
                            if open {
                                IconName::X
                            } else {
                                IconName::ChevronDown
                            },
                            16.0,
                            TEXT(),
                        ))
                        .child(
                            canvas(move |bounds, _, _| slot.set(Some(bounds)), |_, _, _, _| {})
                                .absolute()
                                .size_full(),
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if open {
                                this.menu = None;
                                this.menu_guild = None;
                            } else if let Some(b) = slot2.get() {
                                this.menu = Some((
                                    PopMenu::Guild,
                                    b.bottom_left() + point(px(0.0), z(6.0)),
                                ));
                                this.menu_guild = Some(gid.clone());
                                this.menu_from_header = true;
                                this.guild_notify_expanded = false;
                                this.app.request_guild_boost(&gid);
                            }
                            cx.notify();
                        })),
                )
            }
        };
        let head = div()
            .h(z(48.0))
            .flex_none()
            .flex()
            .items_center()
            .pl(z(9.0))
            .pr(z(8.0))
            .border_b_1()
            .border_color(DIVIDER())
            .part(Part::SidebarHeader)
            .child(head);

        let mut list = div()
            .id("channels")
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .gap(px(1.0))
            .pt(z(8.0))
            .pb(z(8.0));
        match &a.sel_guild {
            None => {
                list = list.child(
                    row_frame("friends_open", 42.0, a.friends.open, false)
                        .gap(z(12.0))
                        .px(z(10.0))
                        .child(icon(IconName::UserPlus, 20.0, MUTED()))
                        .child(text(tr!("フレンド", "Friends", "好友"), 16.0, TEXT()))
                        .on_click(cx.listener(|this, _, _, cx| {
                            let before = (this.app.sel_guild.clone(), this.app.sel_channel.clone());
                            this.app.select(None, None);
                            this.app.friends.open = true;
                            this.app.side = crate::app::SidePanel::None;
                            this.after_move(before);
                            if this.app.screen_w < 700.0 {
                                this.app.compact_sidebar_closed = true;
                            }
                            if !this.app.friends.loaded && this.app.friends.loading.is_none() {
                                this.app.fetch_friends();
                            }
                            cx.notify();
                        })),
                );
                list = list.child(
                    div()
                        .h(z(28.0))
                        .flex_none()
                        .flex()
                        .items_center()
                        .pl(z(16.0))
                        .child(
                            text(
                                tr!("ダイレクトメッセージ", "Direct Messages", "私信"),
                                13.0,
                                FAINT(),
                            )
                            .part(Part::SidebarCategory),
                        ),
                );
                let q = a.dm_filter.trim().to_lowercase();
                let shown: Vec<usize> = a
                    .dms
                    .iter()
                    .enumerate()
                    .filter(|(_, c)| q.is_empty() || c.dm_label().to_lowercase().contains(&q))
                    .map(|(i, _)| i)
                    .collect();
                if !shown.is_empty() {
                    // DM は数百件になるので、見えている行だけ作る（行の高さは全部同じ）
                    list = list.child(
                        uniform_list(
                            "dm_list",
                            shown.len(),
                            cx.processor(
                                move |this: &mut Root, range: std::ops::Range<usize>, _, cx| {
                                    let a = &this.app;
                                    range
                                        .filter_map(|k| a.dms.get(*shown.get(k)?))
                                        .map(|c| {
                                            let sel = a.sel_channel.as_deref() == Some(&c.id);
                                            let pres = c
                                                .recipients
                                                .first()
                                                .and_then(|u| a.presences.get(&u.id));
                                            let id = c.id.clone();
                                            div()
                                                .pb(px(1.0))
                                                .child(
                                                    dm_row(
                                                        c,
                                                        sel,
                                                        a.unread.contains_key(&c.id),
                                                        pres,
                                                    )
                                                    .on_click(cx.listener(move |this, _, _, cx| {
                                                        this.open_channel(id.clone());
                                                        cx.notify();
                                                    })),
                                                )
                                                .into_any_element()
                                        })
                                        .collect::<Vec<_>>()
                                },
                            ),
                        )
                        .flex_1()
                        .min_h_0(),
                    );
                } else if !q.is_empty() {
                    list = list.child(div().mt(z(12.0)).flex().justify_center().child(text(
                        tr!("見つかりませんでした", "Nothing found", "未找到"),
                        13.0,
                        FAINT(),
                    )));
                } else if a.dms.is_empty() {
                    // 一覧が届く前は読み込み中、届いて 0 件ならその旨を出す（何も無い画面にしない）
                    list = list.child(div().mt(z(12.0)).flex().justify_center().map(|d| {
                        if a.dms_loaded {
                            d.child(text(
                                tr!(
                                    "ダイレクトメッセージはまだありません",
                                    "No direct messages yet",
                                    "还没有私信"
                                ),
                                13.0,
                                FAINT(),
                            ))
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
                        } else if c.is_text() || c.is_voice() || c.is_forum() {
                            match c.parent_id.as_deref() {
                                None => top.push(c),
                                Some(p) => by_parent.entry(p).or_default().push(c),
                            }
                        }
                    }
                    cats.sort_by_key(|c| c.position);
                    let mut rows: Vec<AnyElement> = Vec::new();
                    if top.is_empty() && by_parent.is_empty() && cats.is_empty() {
                        rows.push(
                            div()
                                .mt(z(12.0))
                                .flex()
                                .justify_center()
                                .child(text(
                                    tr!(
                                        "表示できるチャンネルがありません",
                                        "No channels to show",
                                        "没有可显示的频道"
                                    ),
                                    13.0,
                                    FAINT(),
                                ))
                                .into_any_element(),
                        );
                    }
                    for c in top {
                        if !a.hidden_as_muted(gid, c, cur_voice.as_deref()) {
                            self.channel_rows(c, gid, cur_voice.as_deref(), &mut rows, cx);
                        }
                    }
                    for cat in cats {
                        let cat_id = cat.id.clone();
                        let cat_gid = gid.clone();
                        let can_edit = a.may_edit_channel_permissions(gid, &cat_id);
                        let collapsed = a.is_category_collapsed(&cat.id);
                        let toggle_id = cat.id.clone();
                        rows.push(
                            div()
                                .id(SharedString::from(format!("cat{}", cat.id)))
                                .group("cat")
                                .mt(z(14.0))
                                .h(z(20.0))
                                .flex_none()
                                .pl(z(2.0))
                                .flex()
                                .items_center()
                                .gap(z(2.0))
                                .cursor_pointer()
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.app.toggle_category(&toggle_id);
                                    cx.notify();
                                }))
                                .child(
                                    div()
                                        .w(z(12.0))
                                        .flex_none()
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .child(
                                            icon(
                                                if collapsed {
                                                    IconName::ChevronRight
                                                } else {
                                                    IconName::ChevronDown
                                                },
                                                12.0,
                                                FAINT(),
                                            )
                                            .group_hover("cat", |s| s.text_color(TEXT())),
                                        ),
                                )
                                .child(
                                    line(
                                        cat.name.as_deref().unwrap_or("").to_uppercase(),
                                        12.0,
                                        FAINT(),
                                    )
                                    .group_hover("cat", |s| s.text_color(TEXT()))
                                    .part(Part::SidebarCategory)
                                    .flex_1()
                                    .min_w_0(),
                                )
                                .when(can_edit, |d| {
                                    d.child(
                                        icon_button(
                                            SharedString::from(format!(
                                                "category-permissions-{cat_id}"
                                            )),
                                            IconName::Settings,
                                            20.0,
                                        )
                                        .tooltip(tip(tr!(
                                            "カテゴリの権限",
                                            "Category permissions",
                                            "类别权限"
                                        )))
                                        .on_click(
                                            cx.listener(move |this, _, _, cx| {
                                                // 見出しのクリック（開閉）まで届かせない
                                                cx.stop_propagation();
                                                this.app
                                                    .open_channel_permissions(&cat_gid, &cat_id);
                                                cx.notify();
                                            }),
                                        ),
                                    )
                                })
                                .into_any_element(),
                        );
                        if let Some(l) = by_parent.get(cat.id.as_str()) {
                            for c in l {
                                if (!collapsed
                                    || a.channel_visible_in_collapsed_category(
                                        c,
                                        cur_voice.as_deref(),
                                    ))
                                    && !a.hidden_as_muted(gid, c, cur_voice.as_deref())
                                {
                                    self.channel_rows(c, gid, cur_voice.as_deref(), &mut rows, cx);
                                }
                            }
                        }
                    }
                    list = list.overflow_y_scroll().children(rows);
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
            .part(Part::SidebarChannels)
            .child(head)
            .child(list)
    }

    /// チャンネル1つ分の行（ボイスなら参加者の行も）
    fn channel_rows(
        &self,
        c: &Channel,
        gid: &str,
        cur_voice: Option<&str>,
        rows: &mut Vec<AnyElement>,
        cx: &mut Context<Self>,
    ) {
        let a = &self.app;
        let name = c.name.clone().unwrap_or_default();
        let id = c.id.clone();
        if c.is_voice() {
            let here = cur_voice == Some(c.id.as_str());
            // 開いているだけ（未参加）のときも、開いている印を付ける
            let open = here || a.sel_channel.as_deref() == Some(c.id.as_str());
            let gid = gid.to_string();
            rows.push(
                row_frame(SharedString::from(format!("ch{}", c.id)), 34.0, open, false)
                    .tooltip(tip(if here {
                        tr!("接続しています", "Connected", "已连接")
                    } else {
                        tr!("クリックで参加", "Click to join", "点击加入")
                    }))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if !here {
                            this.app.join_voice(Some(gid.clone()), id.clone(), false);
                        }
                        this.open_channel(id.clone());
                        cx.notify();
                    }))
                    .child(div().w(z(34.0)).flex().justify_center().child(icon(
                        IconName::Volume2,
                        18.0,
                        MUTED(),
                    )))
                    .child(
                        line(name, 15.0, row_color(open, false))
                            .group_hover("row", |s| s.text_color(TEXT())),
                    )
                    .into_any_element(),
            );
            for v in a.voice_members(&c.id) {
                let u = v.user.clone().or_else(|| a.find_user(&v.user_id));
                let live = a.is_streaming(&v.user_id);
                let mut row = voice_member_row(
                    u.as_ref(),
                    &v.user_id,
                    a.is_speaking(&v.user_id),
                    v.self_mute,
                    v.self_deaf,
                    live,
                );
                // 自分以外の配信は、クリックで見る
                if live && here && a.me.as_ref().is_none_or(|m| m.id != v.user_id) {
                    let uid = v.user_id.clone();
                    row = row
                        .cursor_pointer()
                        .tooltip(tip(tr!("配信を見る", "Watch stream", "观看直播")))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.app.watch_stream(&uid);
                            cx.notify();
                        }));
                }
                rows.push(row.into_any_element());
            }
            return;
        }
        let sel = a.sel_channel.as_deref() == Some(&c.id);
        let unread = a.unread.contains_key(&c.id);
        let mark = if c.is_forum() {
            div()
                .w(z(30.0))
                .pl(z(6.0))
                .flex()
                .items_center()
                .child(icon(IconName::MessagesSquare, 18.0, FAINT()))
        } else {
            div()
                .w(z(30.0))
                .pl(z(8.0))
                .text_size(z(20.0))
                .text_color(FAINT())
                .child("#")
        };
        rows.push(
            row_frame(SharedString::from(format!("ch{}", c.id)), 32.0, sel, unread)
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.open_channel(id.clone());
                    cx.notify();
                }))
                .when(unread && !sel, |d| d.child(unread_pill(32.0)))
                .child(mark)
                .child(
                    line(name, 16.0, row_color(sel, unread))
                        .font_weight(FontWeight::MEDIUM)
                        .group_hover("row", |s| {
                            s.text_color(if sel || unread { WHITE() } else { TEXT() })
                        }),
                )
                .into_any_element(),
        );
        // 参加中のスレッドを親の下に
        if let Some(threads) = a.sidebar_threads(gid).remove(c.id.as_str()) {
            for t in threads {
                let sel = a.sel_channel.as_deref() == Some(&t.id);
                let unread = a.unread.contains_key(&t.id);
                let id = t.id.clone();
                rows.push(
                    row_frame(SharedString::from(format!("th{}", t.id)), 28.0, sel, unread)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.open_channel(id.clone());
                            cx.notify();
                        }))
                        .when(unread && !sel, |d| d.child(unread_pill(28.0)))
                        .child(div().w(z(40.0)).pl(z(20.0)).text_color(FAINT()).child("└"))
                        .child(
                            line(
                                t.name.clone().unwrap_or_default(),
                                14.0,
                                row_color(sel, unread),
                            )
                            .group_hover("row", |s| {
                                s.text_color(if sel || unread { WHITE() } else { TEXT() })
                            }),
                        )
                        .into_any_element(),
                );
            }
        }
    }

    /// 左下のユーザー欄（ボイス接続中はその上に接続の状態）
    fn userbar(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let call = self
            .app
            .voice_call_started
            .as_ref()
            .map(|(guild, channel, start)| {
                (guild.clone(), channel.clone(), start.elapsed().as_secs())
            });
        let labels = call
            .as_ref()
            .map(|(guild, channel, elapsed)| {
                self.app
                    .plugins
                    .voice_panel(guild.as_deref(), channel, *elapsed)
            })
            .unwrap_or_default();
        if !labels.is_empty() {
            self.app.repaint_after(std::time::Duration::from_secs(1));
        }
        let a = &self.app;
        let mut card = div()
            .w_full()
            .bg(USERBAR())
            .border_1()
            .border_color(DIVIDER())
            .rounded(z(8.0))
            .part(Part::SidebarUserbar)
            .px(z(8.0))
            .py(z(6.0))
            .flex()
            .flex_col();
        if let Some((gid, cid, connected)) =
            a.voice_channel().map(|(g, c, k)| (g.clone(), c.clone(), k))
        {
            let (cname, place) = a.voice_place(gid.as_deref(), &cid);
            let in_guild = gid.is_some();
            // 通話はつながっていても、ゲートウェイが再接続中なら「接続済み」と出さない
            let (label, color) = match (connected, a.gateway_ok()) {
                (true, true) => (
                    tr!("ボイス接続済み", "Voice Connected", "语音已连接"),
                    GREEN(),
                ),
                (true, false) => (tr!("再接続中…", "Reconnecting…", "正在重新连接…"), ORANGE()),
                (false, _) => (tr!("接続中…", "Connecting…", "正在连接…"), ORANGE()),
            };
            card = card.child(
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
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(z(6.0))
                                    .child(icon(IconName::SignalHigh, 16.0, color))
                                    .child(medium(label, 14.0, color)),
                            )
                            .child(
                                div()
                                    .id("voice_place")
                                    .cursor_pointer()
                                    .min_w_0()
                                    .hover(|s| s.underline())
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        let before = (
                                            this.app.sel_guild.clone(),
                                            this.app.sel_channel.clone(),
                                        );
                                        let a = &mut this.app;
                                        if gid != a.sel_guild {
                                            if let Some(g) = &gid {
                                                a.ensure_guild_channels(g);
                                            }
                                            a.sel_guild = gid.clone();
                                            a.sel_channel = None;
                                        }
                                        // サーバーのボイスチャンネルも、そのチャンネルの画面を開く
                                        a.open_channel(cid.clone());
                                        this.after_move(before);
                                        cx.notify();
                                    }))
                                    .child(line(format!("{cname} / {place}"), 12.0, MUTED())),
                            ),
                    )
                    .when(connected && in_guild, |d| {
                        d.child(
                            icon_button("soundboard", IconName::Volume2, 32.0)
                                .tooltip(tip(tr!("サウンドボード", "Soundboard", "音效板")))
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.app.toggle_soundboard();
                                    cx.notify();
                                })),
                        )
                    })
                    .when(connected, |d| {
                        let sharing = a.my_stream.is_some();
                        let b = icon_button(
                            "screenshare",
                            if sharing {
                                IconName::MonitorOff
                            } else {
                                IconName::MonitorUp
                            },
                            32.0,
                        );
                        // 画面共有ができない OS では、押せないことをはっきり見せる
                        if !super::voice_stage::SCREEN_SHARE {
                            return d.child(
                                b.opacity(0.4)
                                    .cursor_default()
                                    .tooltip(tip(super::voice_stage::screen_share_unsupported())),
                            );
                        }
                        d.child(
                            b.tooltip(tip(if sharing {
                                tr!("画面共有を停止", "Stop sharing", "停止共享屏幕")
                            } else {
                                tr!("画面を共有", "Share screen", "共享屏幕")
                            }))
                            .on_click(cx.listener(
                                |this, _, _, cx| {
                                    this.app.toggle_stream();
                                    cx.notify();
                                },
                            )),
                        )
                    })
                    .child(
                        icon_button("hangup", IconName::PhoneOff, 32.0)
                            .tooltip(tip(tr!("切断", "Disconnected", "已断开")))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.app.leave_voice();
                                cx.notify();
                            })),
                    ),
            );
            if let Some(phase) = &a.my_stream {
                let label = match phase {
                    crate::golive::Phase::Live => match a
                        .stream_viewers
                        .iter()
                        .filter(|v| a.me.as_ref().is_none_or(|m| &m.id != *v))
                        .count()
                    {
                        0 => tr!(
                            "画面を共有しています",
                            "Sharing your screen",
                            "正在共享屏幕"
                        )
                        .to_string(),
                        n => tr!(
                            "画面を共有しています・{n} 人が視聴中",
                            "Sharing your screen · {n} watching",
                            "正在共享屏幕 · {n} 人观看中",
                        ),
                    },
                    _ => tr!(
                        "画面共有を準備しています…",
                        "Preparing screen share…",
                        "正在准备屏幕共享…"
                    )
                    .to_string(),
                };
                card = card.child(
                    div()
                        .pl(z(22.0))
                        .pb(z(4.0))
                        .flex()
                        .items_center()
                        .gap(z(6.0))
                        .when(matches!(phase, crate::golive::Phase::Live), |d| {
                            d.child(live_badge())
                        })
                        .child(text(label, 12.0, MUTED())),
                );
            }
            for label in labels {
                card = card.child(
                    div()
                        .pl(z(22.0))
                        .pb(z(4.0))
                        .child(text(label, 12.0, MUTED())),
                );
            }
            card = card.child(div().my(z(6.0)).child(hline(DIVIDER())));
        } else if let Some((gid, cid, msg)) = a.voice_view.error.clone() {
            // つなげなかったときは、黙って消さずに理由と再試行を出す
            let (cname, _) = a.voice_place(gid.as_deref(), &cid);
            card = card
                .child(
                    div()
                        .h(z(44.0))
                        .flex()
                        .items_center()
                        .gap(z(6.0))
                        .child(icon(IconName::CircleAlert, 16.0, RED()))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .flex()
                                .flex_col()
                                .child(medium(msg, 14.0, RED()))
                                .child(line(cname, 12.0, MUTED())),
                        )
                        .child(
                            icon_button("voice_retry", IconName::RefreshCw, 32.0)
                                .tooltip(tip(tr!("再試行", "Retry", "重试")))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.app.join_voice(gid.clone(), cid.clone(), false);
                                    cx.notify();
                                })),
                        )
                        .child(
                            icon_button("voice_error_close", IconName::X, 32.0)
                                .tooltip(tip(tr!("閉じる", "Close", "关闭")))
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.app.voice_view.error = None;
                                    cx.notify();
                                })),
                        ),
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
                            this.app.open_profile(
                                me2.clone(),
                                Bounds::new(p, size(px(0.), px(0.))),
                                true,
                            );
                        }
                        cx.notify();
                    }))
                    .child(with_status(
                        avatar(
                            Some(&user_avatar_url(me)),
                            32.0,
                            me.display(),
                            16.0,
                            BLURPLE(),
                        ),
                        32.0,
                        &cur,
                        USERBAR(),
                    ))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .child(
                                line(me.display().to_string(), 14.0, WHITE())
                                    .font_weight(FontWeight::MEDIUM),
                            )
                            .child(line(status_label(&cur), 12.0, MUTED())),
                    ),
            );
        } else {
            // ログインが終わるまで（自分の情報が届くまで）は空けておく
            row = row.child(div().flex_1());
        }
        let mic_off = a.mic_off();
        let deaf = a.settings.self_deaf;
        row = row
            .child(self.toggle_with_menu(
                "mic",
                IconName::Mic,
                IconName::MicOff,
                mic_off,
                if mic_off {
                    tr!("ミュート解除", "Unmute", "取消静音")
                } else {
                    tr!("ミュート", "Mute", "静音")
                },
                tr!("入力オプション", "Input options", "输入选项"),
                PopMenu::Mic,
                cx,
            ))
            .child(self.toggle_with_menu(
                "head",
                IconName::Headphones,
                IconName::HeadphoneOff,
                deaf,
                if deaf {
                    tr!("スピーカーミュート解除", "Undeafen", "取消关闭扬声器")
                } else {
                    tr!("スピーカーミュート", "Deafen", "关闭扬声器")
                },
                tr!("出力オプション", "Output options", "输出选项"),
                PopMenu::Headset,
                cx,
            ))
            .child({
                // GitHub の main に新しいコミットがあれば、歯車の右上に赤い点を付ける
                let badge = crate::commits::badge();
                let btn = icon_button("settings_btn", IconName::Settings, 32.0)
                    .tooltip(tip(if badge {
                        tr!(
                            "ユーザー設定（新しいコミットがあります）",
                            "User Settings (new commits available)",
                            "用户设置（有新的提交）"
                        )
                    } else {
                        tr!("ユーザー設定", "User Settings", "用户设置")
                    }))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.app.settings_open = true;
                        cx.notify();
                    }));
                div().relative().child(btn).when(badge, |d| {
                    d.child(
                        div()
                            .absolute()
                            .top(z(4.0))
                            .right(z(4.0))
                            .size(z(8.0))
                            .rounded_full()
                            .bg(RED()),
                    )
                })
            });
        div()
            .flex_none()
            .bg(BG())
            .pl(z(8.0))
            .pr(z(8.0))
            .pt(z(6.0))
            .pb(z(8.0))
            .child(card.child(row))
    }

    /// 左下のマイク・ヘッドホンのような「アイコン＋⌄」の組
    #[allow(clippy::too_many_arguments)]
    fn toggle_with_menu(
        &self,
        id: &'static str,
        on_icon: IconName,
        off_icon: IconName,
        off: bool,
        main_tip: &'static str,
        menu_tip: &'static str,
        menu: PopMenu,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let color = if off { RED() } else { TEXT() };
        let hover_bg = if off {
            Color32::from_rgba_unmultiplied(0xda, 0x37, 0x3c, 70)
        } else {
            SELECTED()
        };
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
            .child(
                icon(if off { off_icon } else { on_icon }, 18.0, color)
                    .group_hover("tg", |s| s.text_color(if off { RED() } else { WHITE() })),
            );
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
                this.menu = if this.menu.is_some_and(|m| m.0 == menu) {
                    None
                } else {
                    Some((menu, window.mouse_position()))
                };
                this.app.audio_devs = None;
                cx.notify();
            }))
            .child(
                icon(IconName::ChevronDown, 12.0, color)
                    .group_hover("tc", |s| s.text_color(WHITE())),
            );
        div()
            .w(z(52.0))
            .h(z(32.0))
            .flex_none()
            .flex()
            .rounded(z(6.0))
            .when(off, |d| {
                d.bg(Color32::from_rgba_unmultiplied(0xda, 0x37, 0x3c, 40))
            })
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
        with_status(
            av,
            32.0,
            presence.map_or("offline", |p| p.status.as_str()),
            if selected { SELECTED() } else { BG() },
        )
    } else {
        av
    };
    let color = row_color(selected, unread);
    let name = line(label, 16.0, color).group_hover("row", move |s| {
        s.text_color(if selected || unread { WHITE() } else { TEXT() })
    });
    let names = match presence
        .and_then(|p| p.activity.clone())
        .filter(|_| one_to_one)
    {
        Some(act) => div()
            .flex_1()
            .min_w_0()
            .flex()
            .flex_col()
            .child(name)
            .child(line(act, 12.0, MUTED())),
        None => div().flex_1().min_w_0().child(name),
    };
    let mut row = row_frame(
        SharedString::from(format!("dm{}", c.id)),
        42.0,
        selected,
        unread,
    )
    .gap(z(12.0))
    .pl(z(8.0))
    .child(av)
    .child(names);
    if unread && !selected {
        row = row.child(unread_pill(42.0));
    }
    if one_to_one {
        row = row.tooltip(tip(status_label(
            presence.map_or("offline", |p| p.status.as_str()),
        )));
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
fn voice_member_row(
    u: Option<&User>,
    id: &str,
    speaking: bool,
    mute: bool,
    deaf: bool,
    live: bool,
) -> Stateful<Div> {
    let name = u
        .map(|u| u.display().to_string())
        .unwrap_or_else(|| id.to_string());
    let mut right = div().flex().items_center().gap(z(4.0));
    if live {
        right = right.child(live_badge());
    }
    if deaf {
        right = right.child(icon(IconName::HeadphoneOff, 14.0, MUTED()));
    }
    if mute || deaf {
        right = right.child(icon(IconName::MicOff, 14.0, MUTED()));
    }
    div()
        .id(SharedString::from(format!("vm{id}")))
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

/// 画面共有中の赤い「LIVE」
pub fn live_badge() -> Div {
    div()
        .flex_none()
        .px(z(4.0))
        .rounded(z(4.0))
        .bg(RED())
        .child(text("LIVE", 10.0, WHITE()).font_weight(FontWeight::BOLD))
}

/// サーバーアイコンの右下の、未読メンション数の赤い印（レールの地の色で 2px 縁取って、アイコンから切り抜いたように見せる）
fn mention_badge(n: u32) -> Div {
    let label = if n > 99 {
        "99+".to_string()
    } else {
        n.to_string()
    };
    div()
        .absolute()
        .bottom(z(2.0))
        .right(z(12.0))
        .min_w(z(20.0))
        .h(z(20.0))
        .px(z(2.0))
        .rounded_full()
        .bg(opaque(BG()))
        .flex()
        .items_center()
        .justify_center()
        .child(
            div()
                .min_w(z(16.0))
                .h(z(16.0))
                .px(z(4.0))
                .rounded_full()
                .bg(RED())
                .flex()
                .items_center()
                .justify_center()
                .text_size(z(12.0))
                .font_weight(FontWeight::BOLD)
                .line_height(z(16.0))
                .text_color(Color32::WHITE)
                // 日本語フォントは数字が行の中で下に寄るので、赤丸の中心に合うよう 1px 上げる
                .child(div().relative().top(z(-1.0)).child(label)),
        )
}
