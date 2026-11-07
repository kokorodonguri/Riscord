//! 小さなメニュー（＋・マイク/ヘッドホンの ⌄）とプロフィールのポップアップ
use gpui::assets::IconName;
use gpui::prelude::*;
use gpui::*;

use super::*;
use crate::app::{darken, local_dt, name_color, rgb, snowflake_date, status_label, user_avatar_url, ProfileState, STATUSES};
use crate::color::Color32;
use crate::model::ProfileData;
use crate::net::{self, Cmd};
use crate::voice;

/// ポップアップの箱
fn popup_box() -> Div {
    div().bg(SIDEBAR()).border_1().border_color(BORDER()).rounded(z(8.0)).shadow(popup_shadow())
}

/// メニューの1行（左に印、右に › を出せる）
fn menu_row(id: impl Into<ElementId>, mark: AnyElement, label: impl Into<SharedString>, chevron: bool) -> Stateful<Div> {
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
        .hover(|s| s.bg(HOVER()).text_color(WHITE()))
        .child(div().w(z(20.0)).flex().justify_center().child(mark))
        .child(div().flex_1().min_w_0().text_size(z(15.0)).truncate().child(label.into()))
        .when(chevron, |d| d.child(icon(IconName::ChevronRight, 14.0, TEXT()).group_hover("mr", |s| s.text_color(WHITE()))))
}

fn menu_divider() -> Div {
    div().my(z(4.0)).mx(z(10.0)).child(hline(DIVIDER()))
}

/// ラジオボタンの丸
fn radio(on: bool) -> Div {
    let d = div().size(z(16.0)).flex_none().rounded_full();
    if on {
        d.bg(WHITE()).flex().items_center().justify_center().child(div().size(z(8.0)).rounded_full().bg(BLURPLE()))
    } else {
        d.border_2().border_color(MUTED())
    }
}

impl Root {
    pub fn menu_layer(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let (menu, pos) = self.menu?;
        let close = cx.listener(|this, _: &MouseDownEvent, _, cx| {
            this.menu = None;
            cx.notify();
        });
        let content: AnyElement = match menu {
            PopMenu::Plus => {
                let mut col = popup_box().p(z(6.0)).w(z(252.0)).flex().flex_col().gap(z(2.0));
                col = col.child(menu_row("plus_upload", icon(IconName::Upload, 20.0, TEXT()).into_any_element(), "ファイルをアップロード", false).on_click(cx.listener(|this, _, _, cx| {
                    this.menu = None;
                    this.app.pick_files();
                    cx.notify();
                })));
                col = col.child(menu_row("plus_paste", icon(IconName::ClipboardPaste, 20.0, TEXT()).into_any_element(), "画像を貼り付け", false).on_click(cx.listener(|this, _, _, cx| {
                    this.menu = None;
                    if !this.app.paste_image() {
                        this.app.status = "クリップボードに画像がありません".into();
                    }
                    cx.notify();
                })));
                col.into_any_element()
            }
            PopMenu::Mic | PopMenu::Headset => self.audio_menu(menu == PopMenu::Mic, window, cx).into_any_element(),
        };
        Some(
            deferred(anchored().position(pos - point(z(0.0), z(8.0))).anchor(Anchor::BottomLeft).snap_to_window_with_margin(z(8.0)).child(div().occlude().on_mouse_down_out(close).child(content)))
                .with_priority(1)
                .into_any_element(),
        )
    }

    /// 左下のマイク／ヘッドホン横の ⌄ メニュー
    fn audio_menu(&mut self, input: bool, window: &mut Window, cx: &mut Context<Self>) -> Div {
        if self.app.audio_devs.is_none() {
            self.app.audio_devs = Some((voice::input_devices(), voice::output_devices()));
        }
        let (ins, outs) = self.app.audio_devs.clone().unwrap_or_default();
        let list = if input { ins } else { outs };
        let cur = if input { self.app.settings.input_device.clone() } else { self.app.settings.output_device.clone() };
        let mut devs = div().id("devs").max_h(z(220.0)).overflow_y_scroll().flex().flex_col();
        for (i, d) in std::iter::once(None).chain(list.into_iter().map(Some)).enumerate() {
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
                        st::save_settings(&this.app.settings);
                        cx.notify();
                    }))
                    .child(radio(on))
                    .child(line(label, 14.0, TEXT()).flex_1()),
            );
        }
        let mut col = popup_box().p(z(10.0)).w(z(260.0)).flex().flex_col().gap(z(6.0)).child(text(if input { "入力デバイス" } else { "出力デバイス" }, 12.0, MUTED())).child(devs).child(menu_divider());
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
                            st::save_settings(&this.app.settings);
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
        col.child(menu_divider()).child(small_button("voice_settings", "音声設定…").on_click(cx.listener(|this, _, _, cx| {
            this.app.settings_open = true;
            this.app.settings_tab = 2;
            this.menu = None;
            cx.notify();
        })))
    }

    /// プロフィールのポップアップ（アバターや名前を押したとき）
    pub fn profile_popup(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let pp = self.app.popup.clone()?;
        let user = pp.user.clone();
        let key = net::profile_key(&user.id, pp.guild_id.as_deref());
        let failed = matches!(self.app.profiles.get(&key), Some(ProfileState::Failed));
        let loading = matches!(self.app.profiles.get(&key), Some(ProfileState::Loading) | None);
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
        let dm = self.app.dms.iter().find(|c| c.recipients.len() == 1 && c.recipients[0].id == user.id).map(|c| c.id.clone());
        let width = (self.app.screen_w - 16.0).clamp(0.0, 300.0);
        let max_h = (f32::from(window.viewport_size().height) / zoom() - 16.0).max(0.0);

        // バナー（画像 → 指定色 → 名前から決めた色の順）
        let color = data.as_ref().and_then(|d| d.accent_color).map(rgb).unwrap_or_else(|| darken(name_color(&user.id)));
        let mut banner = div().h(z(120.0)).w_full().rounded_t(z(8.0)).bg(color).overflow_hidden();
        if let Some(h) = data.as_ref().and_then(|d| d.banner.as_ref()) {
            if let Some(i) = loaded(&format!("https://cdn.discordapp.com/banners/{}/{}.png?size=600", user.id, h)) {
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
            .child(with_status(avatar(Some(&user_avatar_url(&user)), 80.0, user.display(), 40.0, BLURPLE()), 80.0, &status, SIDEBAR()));
        let name = data.as_ref().and_then(|d| d.nick.clone()).unwrap_or_else(|| user.display().to_string());
        let mut sub = user.username.clone();
        if let Some(pr) = data.as_ref().and_then(|d| d.pronouns.as_deref()) {
            sub += &format!(" • {pr}");
        }
        let mut info = div().px(z(16.0)).pt(z(52.0)).pb(z(16.0)).flex().flex_col().gap(z(4.0)).child(line(name, 20.0, WHITE())).child(line(sub, 14.0, TEXT()));
        if !is_me {
            info = info.child(text(status_label(&status), 12.0, MUTED()));
        }
        if let Some(act) = &activity {
            info = info.child(div().mt(z(4.0)).bg(RAIL()).rounded(z(8.0)).px(z(10.0)).py(z(6.0)).child(text(act.clone(), 13.0, TEXT())));
        }
        let retry = {
            let pp = pp.clone();
            let key = key.clone();
            cx.listener(move |this, _, _, cx| {
                let cmd = Cmd::LoadProfile { user_id: pp.user.id.clone(), guild_id: pp.guild_id.clone() };
                if this.app.backend.as_ref().is_some_and(|b| b.cmd.send(cmd).is_ok()) {
                    this.app.profiles.insert(key.clone(), ProfileState::Loading);
                } else {
                    this.app.status = "エラー: 接続できません。再接続してからお試しください。".into();
                }
                cx.notify();
            })
        };
        if is_me {
            // 自分のときは本家と同じく、自己紹介を名前の下にそのまま出してメニューを並べる
            if let Some(bio) = data.as_ref().and_then(|d| d.bio.clone()) {
                info = info.child(self.bio_text(&bio, 14.0).mt(z(8.0)));
            }
            if failed {
                info = info.child(div().flex().child(small_button("retry_profile", "再読み込み").on_click(retry)));
            }
            let uid = user.id.clone();
            let mut menu = div().mt(z(10.0)).bg(RAIL()).rounded(z(8.0)).p(z(6.0)).flex().flex_col();
            menu = menu.child(
                menu_row("edit_profile", icon(IconName::Pencil, 18.0, TEXT()).into_any_element(), "プロフィールを編集", false)
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
                menu_row("status_row", status_mark(&status, 5.0, RAIL()).into_any_element(), status_label(&status), true)
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
                        menu_row(SharedString::from(format!("st_{k}")), status_mark(k, 5.0, SIDEBAR()).into_any_element(), label, false)
                            .when(on, |d| d.child(div().size(z(12.0)).rounded_full().bg(BLURPLE()).flex().items_center().justify_center().child(div().size(z(5.0)).rounded_full().bg(Color32::WHITE))))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.app.set_status(k.to_string());
                                this.app.popup = None;
                                this.status_sub = false;
                                cx.notify();
                            })),
                    );
                }
                status_row = status_row.child(div().absolute().left(z(width - 20.0)).top(z(-6.0)).child(subm));
            }
            menu = menu.child(status_row).child(menu_divider());
            menu = menu.child(
                menu_row("copy_id", div().w(z(18.0)).h(z(14.0)).rounded(z(3.0)).bg(TEXT()).flex().items_center().justify_center().text_size(z(9.0)).text_color(RAIL()).child("ID").into_any_element(), "ユーザーIDをコピー", false)
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
            let mut about = div().mt(z(6.0)).bg(RAIL()).rounded(z(8.0)).p(z(12.0)).flex().flex_col().gap(z(4.0));
            if loading {
                about = about.child(div().flex().justify_center().child(spinner(20.0)));
            } else if failed {
                about = about.child(text("詳しいプロフィールを取得できませんでした", 13.0, MUTED())).child(div().flex().child(small_button("retry_profile", "再読み込み").on_click(retry)));
            }
            let heading = |t: &'static str| text(t, 12.0, WHITE()).font_weight(FontWeight::MEDIUM);
            if let Some(bio) = data.as_ref().and_then(|d| d.bio.clone()) {
                about = about.child(heading("自己紹介")).child(self.bio_text(&bio, 14.0).mb(z(8.0)));
            }
            about = about.child(heading("Discordメンバー歴")).child(text(snowflake_date(&user.id), 14.0, TEXT()));
            if let Some(j) = data.as_ref().and_then(|d| d.joined_at.as_deref()).and_then(local_dt) {
                about = about.child(heading("サーバーメンバー歴").mt(z(8.0))).child(text(j.format("%Y年%-m月%-d日").to_string(), 14.0, TEXT()));
            }
            if let Some(roles) = data.as_ref().map(|d| d.roles.clone()).filter(|r| !r.is_empty()) {
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
                info = info.child(div().mt(z(10.0)).child(fill_button("send_dm", "メッセージを送る", BLURPLE(), Color32::WHITE, true).w_full().on_click(cx.listener(move |this, _, _, cx| {
                    this.app.popup = None;
                    let before = (this.app.sel_guild.clone(), this.app.sel_channel.clone());
                    this.app.sel_guild = None;
                    this.app.open_channel(cid.clone());
                    this.after_move(before);
                    cx.notify();
                }))));
            }
        }
        let body = popup_box()
            .w(z(width))
            .max_h(z(max_h))
            .flex()
            .flex_col()
            .child(div().relative().flex_none().child(banner).child(av))
            .child(div().id("profile_scroll").flex_1().min_h_0().overflow_y_scroll().child(info));
        let close = cx.listener(|this, _: &MouseDownEvent, _, cx| {
            this.app.popup = None;
            this.status_sub = false;
            cx.notify();
        });
        let (pos, corner) = if pp.above {
            (point(pp.anchor.origin.x, pp.anchor.origin.y - z(10.0)), Anchor::BottomLeft)
        } else {
            (point(pp.anchor.origin.x + z(12.0), pp.anchor.origin.y), Anchor::TopLeft)
        };
        Some(deferred(anchored().position(pos).anchor(corner).snap_to_window_with_margin(z(8.0)).child(div().occlude().on_mouse_down_out(close).child(body))).with_priority(2).into_any_element())
    }
}
