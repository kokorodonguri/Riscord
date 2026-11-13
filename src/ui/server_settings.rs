//! サーバー設定の画面。左にタブ、右に中身（狭い画面ではタブを上に並べる）。
use gpui::assets::IconName;
use gpui::prelude::*;
use gpui::*;

use super::*;
use crate::app::SettingsTab;

impl Root {
    pub fn server_settings_dialog(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let (gid, tab) = self.app.server_settings.clone()?;
        let narrow = self.app.screen_w < 640.0;
        let width = (self.app.screen_w - 32.0).clamp(0.0, 900.0);
        let height = (self.pane_h - 48.0).max(200.0);
        let guild_name = self
            .app
            .guilds
            .iter()
            .find(|g| g.id == gid)
            .map(|g| g.name.clone())
            .unwrap_or_default();

        let mut nav = div().flex().gap(z(2.0));
        nav = if narrow {
            nav.flex_wrap()
        } else {
            nav.flex_col().w(z(180.0)).flex_none()
        };
        if !narrow {
            nav = nav.child(div().px(z(10.0)).pb(z(6.0)).child(line(
                guild_name.to_uppercase(),
                12.0,
                FAINT(),
            )));
        }
        for t in SettingsTab::ALL {
            if !self.app.can_open_settings_tab(&gid, t) {
                continue;
            }
            let selected = t == tab;
            nav = nav.child(
                div()
                    .id(SharedString::from(format!("settings_tab_{}", t.label())))
                    .h(z(32.0))
                    .px(z(10.0))
                    .rounded(z(6.0))
                    .flex()
                    .items_center()
                    .cursor_pointer()
                    .text_size(z(15.0))
                    .text_color(if selected { WHITE() } else { MUTED() })
                    .when(selected, |d| d.bg(SELECTED()))
                    .hover(|d| d.bg(HOVER()).text_color(TEXT()))
                    .child(t.label())
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.app.set_settings_tab(t);
                        cx.notify();
                    })),
            );
        }

        let body = match tab {
            SettingsTab::Overview | SettingsTab::Roles => self.role_admin_view(true, cx),
            SettingsTab::Mod(_) => self.moderation_view(true, cx),
        }
        .unwrap_or_else(|| {
            div()
                .flex()
                .items_center()
                .gap(z(8.0))
                .child(spinner(16.0))
                .child(text(
                    tr!("読み込み中…", "Loading…", "正在加载…"),
                    14.0,
                    MUTED(),
                ))
                .into_any_element()
        });

        let header = div()
            .flex()
            .items_center()
            .gap(z(8.0))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(medium(tab.label(), 20.0, WHITE()))
                    .when(narrow, |d| d.child(line(guild_name.clone(), 13.0, MUTED()))),
            )
            .child(
                icon_button("server_settings_close", IconName::X, 28.0)
                    .tooltip(tip(tr!("閉じる", "Close", "关闭")))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.app.close_server_settings();
                        cx.notify();
                    })),
            );
        let right = div()
            .id("server-settings-body")
            .flex_1()
            .min_w_0()
            .min_h_0()
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .gap(z(12.0))
            .child(header)
            .child(body);
        let mut content = card(SIDEBAR(), BORDER(), 8.0)
            .w(z(width))
            .h(z(height.min(720.0)))
            .p(z(16.0))
            .flex()
            .gap(z(16.0));
        content = if narrow {
            content.flex_col().child(nav).child(right)
        } else {
            content
                .child(nav)
                .child(div().w(px(1.0)).h_full().bg(DIVIDER()))
                .child(right)
        };
        let close = cx.listener(|this, _: &MouseDownEvent, _, cx| {
            this.app.close_server_settings();
            cx.notify();
        });
        Some(modal("server-settings-modal", content, close, 170).into_any_element())
    }
}
