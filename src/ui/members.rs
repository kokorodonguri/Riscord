//! 右側: サーバーのメンバー一覧と、DM 相手のプロフィール
use gpui::prelude::*;
use gpui::*;

use super::*;
use crate::app::{darken, emoji_url, member_role_color, name_color, rgb, snowflake_date, split_emoji, user_avatar_url, MemberListCache, MemberRow, ProfileState, Seg, MEMBER_HEAD_H, MEMBER_ROW_H};
use crate::model::{ProfileData, User};

impl Root {
    /// メンバー一覧（見えている行だけ描き、見えている範囲を購読する）
    pub fn members_panel(&mut self, _cx: &mut Context<Self>) -> Div {
        let Some(gid) = self.app.sel_guild.clone() else { return div() };
        // グループ分けと並べ替えは、メンバー・ロール・オンライン状態が変わったときだけやり直す
        let a = &mut self.app;
        if a.member_cache.gid != gid || a.member_cache.ver != a.member_ver {
            let members = a.members.get(&gid).map(Vec::as_slice).unwrap_or(&[]);
            let roles = a.roles.get(&gid).map(Vec::as_slice).unwrap_or(&[]);
            // Discord から一覧が届いていればその並びを使い、まだなら手元のメンバーで組み立てる
            let server = a.sel_channel.as_ref().and_then(|cid| a.member_lists.get(&gid).filter(|l| &l.channel_id == cid && l.list_id.is_some()));
            a.member_cache = match server {
                Some(list) => MemberListCache::from_server(&gid, a.member_ver, list, members, roles),
                None => MemberListCache::build(&gid, a.member_ver, members, roles, &a.presences),
            };
        }
        let key = (gid.clone(), a.member_ver);
        let n = a.member_cache.rows.len();
        if self.member_key != key {
            if self.member_key.0 != gid || self.member_list.item_count() != n {
                // サーバーが変わったら先頭から。同じサーバーなら見ている位置はそのまま
                let top = self.member_list.logical_scroll_top();
                self.member_list.reset(n);
                if self.member_key.0 == gid {
                    self.member_list.scroll_to(ListOffset { item_ix: top.item_ix.min(n.saturating_sub(1)), offset_in_item: top.offset_in_item });
                }
            } else {
                self.member_list.remeasure();
            }
            self.member_key = key;
        }
        let this = self.this.clone();
        self.member_list.set_scroll_handler(move |ev: &ListScrollEvent, _, cx| {
            let range = ev.visible_range.clone();
            if let Some(e) = this.upgrade() {
                e.update(cx, |r, cx| {
                    let v = (range.start, range.end.saturating_sub(1));
                    if r.app.member_view != v {
                        r.app.member_view = v;
                        r.app.subscribe_member_list();
                        cx.notify();
                    }
                });
            }
        });
        let this = self.this.clone();
        div().h_full().flex().flex_col().border_l_1().border_t_1().border_color(DIVIDER()).bg(BG()).child(
            list(self.member_list.clone(), move |ix, _, cx| {
                let Some(e) = this.upgrade() else { return div().into_any_element() };
                e.update(cx, |r, cx| r.member_row(ix, cx))
            })
            .flex_1()
            .pb(z(8.0)),
        )
    }

    fn member_row(&mut self, ix: usize, cx: &mut Context<Self>) -> AnyElement {
        let a = &self.app;
        let Some(row) = a.member_cache.rows.get(ix) else { return div().into_any_element() };
        match row {
            MemberRow::Placeholder => {
                // まだ届いていない行（スクロールして購読すると埋まる）
                fit_row(
                    div()
                        .h(z(MEMBER_ROW_H))
                        .px(z(8.0))
                        .flex()
                        .items_center()
                        .gap(z(12.0))
                        .child(div().size(z(32.0)).rounded_full().bg(HOVER()))
                        .child(div().flex_1().h(z(12.0)).mr(z(30.0)).rounded(z(6.0)).bg(HOVER())),
                )
            }
            MemberRow::Header(t) => div().h(z(MEMBER_HEAD_H)).pl(z(16.0)).pb(z(4.0)).flex().items_end().child(line(t.clone(), 12.0, FAINT())).into_any_element(),
            MemberRow::Member(mi) => {
                let gid = a.member_cache.gid.clone();
                let Some(member) = a.members.get(&gid).and_then(|l| l.get(*mi)) else { return div().into_any_element() };
                let Some(user) = member.user.clone() else { return div().into_any_element() };
                let roles = a.roles.get(&gid).map(Vec::as_slice).unwrap_or(&[]);
                let presence = a.presences.get(&user.id);
                let offline = presence.is_none_or(|p| p.status == "offline");
                let color = member_role_color(member, roles).unwrap_or(TEXT());
                let name = member.display().to_string();
                let av = with_status(avatar(Some(&user_avatar_url(&user)), 32.0, &name, 16.0, name_color(&user.id)), 32.0, presence.map_or("offline", |p| p.status.as_str()), BG());
                let names = match presence.and_then(|p| p.activity.clone()) {
                    Some(act) => div().flex_1().min_w_0().flex().flex_col().child(line(name, 16.0, color).font_weight(FontWeight::MEDIUM)).child(line(act, 12.0, MUTED())),
                    None => div().flex_1().min_w_0().child(line(name, 16.0, color).font_weight(FontWeight::MEDIUM)),
                };
                let row = div()
                    .id(("member", *mi))
                    .h(z(MEMBER_ROW_H))
                    .px(z(8.0))
                    .rounded(z(8.0))
                    .flex()
                    .items_center()
                    .gap(z(12.0))
                    .cursor_pointer()
                    .hover(|s| s.bg(HOVER()))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        let p = window.mouse_position();
                        this.app.open_profile(user.clone(), Bounds::new(p, size(px(0.), px(0.))), false);
                        cx.notify();
                    }))
                    .child(div().flex().items_center().gap(z(12.0)).flex_1().min_w_0().when(offline, |d| d.opacity(0.3)).child(av).child(names));
                fit_row(row)
            }
        }
    }

    /// DM 右側のプロフィールカード（Vesktop と同じく右側に常設）
    pub fn dm_profile_panel(&mut self, u: &User, cx: &mut Context<Self>) -> impl IntoElement {
        let key = crate::net::profile_key(&u.id, None);
        let data: Option<ProfileData> = match self.app.profiles.get(&key) {
            Some(ProfileState::Ready(d)) => Some(d.clone()),
            _ => None,
        };
        let presence = self.app.presences.get(&u.id).cloned();
        let accent = data.as_ref().and_then(|d| d.accent_color).map(rgb).unwrap_or_else(|| darken(name_color(&u.id)));
        let status = presence.as_ref().map_or("offline".to_string(), |p| p.status.clone());
        let mut banner = div().relative().h(z(120.0)).w_full().rounded_t(z(8.0)).bg(accent).overflow_hidden();
        if let Some(h) = data.as_ref().and_then(|d| d.banner.as_ref()) {
            let url = format!("https://cdn.discordapp.com/banners/{}/{}.png?size=600", u.id, h);
            if let Some(i) = loaded(&url) {
                banner = banner.child(img(i).size_full().object_fit(ObjectFit::Cover));
            }
        }
        let av = div()
            .absolute()
            .left(z(10.0))
            .top(z(120.0 - 50.0))
            .size(z(92.0))
            .rounded_full()
            .bg(SIDEBAR())
            .flex()
            .items_center()
            .justify_center()
            .child(with_status(avatar(Some(&user_avatar_url(u)), 80.0, u.display(), 40.0, name_color(&u.id)), 80.0, &status, SIDEBAR()));
        let mut info = div().px(z(16.0)).pt(z(48.0)).flex().flex_col().gap(z(4.0)).child(line(u.display().to_string(), 20.0, WHITE()).font_weight(FontWeight::MEDIUM));
        let mut l = u.username.clone();
        if let Some(p) = data.as_ref().and_then(|d| d.pronouns.as_deref()).filter(|p| !p.is_empty()) {
            l.push_str(&format!(" • {p}"));
        }
        info = info.child(text(l, 14.0, MUTED()));
        if let Some(act) = presence.as_ref().and_then(|p| p.activity.clone()) {
            info = info.child(text(act, 13.0, TEXT()).mt(z(4.0)));
        }
        if let Some(bio) = data.as_ref().and_then(|d| d.bio.clone()).filter(|b| !b.is_empty()) {
            info = info.child(self.bio_text(&bio, 14.0).mt(z(10.0)));
        }
        info = info.child(medium("メンバー登録日", 12.0, MUTED()).mt(z(14.0))).child(text(snowflake_date(&u.id), 14.0, TEXT()));
        let u2 = u.clone();
        let card = card(SIDEBAR(), BORDER(), 8.0)
            .size_full()
            .flex()
            .flex_col()
            .child(div().relative().child(banner).child(av))
            .child(div().id("dm_profile_scroll").flex_1().min_h_0().overflow_y_scroll().child(info))
            .child(
                div().p(z(16.0)).child(
                    fill_button("dm_full_profile", "プロフィール全体を表示", RAIL(), WHITE(), true).w_full().on_click(cx.listener(move |this, _, window, cx| {
                        let p = window.mouse_position();
                        this.app.open_profile(u2.clone(), Bounds::new(p, size(px(0.), px(0.))), false);
                        cx.notify();
                    })),
                ),
            );
        div().w(z(340.0)).h_full().flex_none().bg(BG()).border_l_1().border_t_1().border_color(DIVIDER()).p(z(10.0)).child(card)
    }

    /// 自己紹介（カスタム絵文字 <:name:id> は画像で出す）
    pub fn bio_text(&mut self, bio: &str, size: f32) -> Div {
        let mut row = div().flex().flex_wrap().items_center().text_size(z(size)).text_color(TEXT());
        for (i, seg) in split_emoji(bio).into_iter().enumerate() {
            match seg {
                Seg::Text(t) => {
                    let mut r = super::message::Rich::default();
                    r.linked(t, None);
                    row = row.child(div().max_w_full().child(r.element(SharedString::from(format!("bio{i}")))));
                }
                Seg::Emoji { name, id, animated } => {
                    let url = emoji_url(id, animated);
                    let face = match loaded(&url) {
                        Some(im) => div().size(z(size + 4.0)).child(img(im).size_full().object_fit(ObjectFit::Contain)),
                        None => div().size(z(size + 4.0)),
                    };
                    row = row.child(face.id(SharedString::from(format!("bioe{i}"))).tooltip(tip(format!(":{name}:"))));
                }
            }
        }
        row
    }
}

/// 一覧の行をパネル幅に収める（左右 8px あけ、長い名前は … で切る）。
/// list の行は幅を指定しないと中身の幅まで広がり、右端からはみ出す
fn fit_row(row: impl IntoElement + Styled) -> AnyElement {
    div().w_full().px(z(8.0)).child(row.w_full()).into_any_element()
}
