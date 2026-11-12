//! サーバー設定の画面（公式クライアントと同じく、左のタブで概要・ロール・招待・BAN などを切り替える）。
//! 中身はロール管理（RoleAdmin）とサーバー管理（ServerModeration）をそのまま使い、タブを替えるたびに作り直す。
use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SettingsTab {
    Overview,
    Roles,
    Mod(ModTab),
}

impl SettingsTab {
    pub const ALL: [SettingsTab; 7] = [
        SettingsTab::Overview,
        SettingsTab::Roles,
        SettingsTab::Mod(ModTab::Invites),
        SettingsTab::Mod(ModTab::Bans),
        SettingsTab::Mod(ModTab::Audit),
        SettingsTab::Mod(ModTab::Events),
        SettingsTab::Mod(ModTab::AutoMod),
    ];

    pub fn label(self) -> &'static str {
        match self {
            SettingsTab::Overview => "概要",
            SettingsTab::Roles => "ロール",
            SettingsTab::Mod(t) => t.label(),
        }
    }
}

impl App {
    /// このタブを出すか。概要とロールは権限がなくても見られる（編集できるかは中で確かめる）。
    /// 管理のタブは、権限がないと分かっていれば出さない
    pub fn can_open_settings_tab(&self, guild_id: &str, tab: SettingsTab) -> bool {
        match tab {
            SettingsTab::Overview | SettingsTab::Roles => true,
            SettingsTab::Mod(t) => self.can_open_mod_tab(guild_id, t),
        }
    }

    pub fn open_server_settings(&mut self, guild_id: &str) {
        if !self.guilds.iter().any(|g| g.id == guild_id) {
            return;
        }
        self.role_admin = None;
        self.moderation = None;
        self.server_settings = Some((guild_id.to_owned(), SettingsTab::Overview));
        self.load_settings_tab();
    }

    /// 送信中でなければタブを替える
    pub fn set_settings_tab(&mut self, tab: SettingsTab) {
        let Some((gid, cur)) = self.server_settings.clone() else {
            return;
        };
        if cur == tab || self.settings_busy() || !self.can_open_settings_tab(&gid, tab) {
            return;
        }
        self.server_settings = Some((gid, tab));
        self.load_settings_tab();
    }

    fn settings_busy(&self) -> bool {
        self.role_admin
            .as_ref()
            .is_some_and(|a| a.mutation.is_some())
            || self
                .moderation
                .as_ref()
                .is_some_and(|m| m.mutation.is_some())
    }

    fn load_settings_tab(&mut self) {
        let Some((gid, tab)) = self.server_settings.clone() else {
            return;
        };
        match tab {
            SettingsTab::Overview => {
                self.moderation = None;
                self.open_guild_editor(&gid);
            }
            SettingsTab::Roles => {
                self.moderation = None;
                self.open_role_admin(&gid);
            }
            SettingsTab::Mod(t) => {
                self.role_admin = None;
                match self.moderation.as_ref() {
                    Some(m) if m.guild_id == gid => self.set_mod_tab(t),
                    _ => self.open_moderation_tab(&gid, t),
                }
            }
        }
    }

    /// 送信中でなければ閉じる
    pub fn close_server_settings(&mut self) {
        if self.settings_busy() {
            return;
        }
        self.server_settings = None;
        self.role_admin = None;
        self.moderation = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::mpsc::UnboundedReceiver;

    fn app(perms: u64) -> (App, UnboundedReceiver<Cmd>) {
        let mut app = App::default();
        app.guilds = vec![serde_json::from_value(serde_json::json!({
            "id": "1", "name": "g", "permissions": perms.to_string()
        }))
        .unwrap()];
        let (_, ev) = std::sync::mpsc::channel();
        let (cmd, rx) = tokio::sync::mpsc::unbounded_channel();
        app.backend = Some(Backend { cmd, ev });
        (app, rx)
    }

    fn calls(rx: &mut UnboundedReceiver<Cmd>) -> Vec<net::ApiCall> {
        let mut out = Vec::new();
        while let Ok(Cmd::Api { call, .. }) = rx.try_recv() {
            out.push(call);
        }
        out
    }

    #[test]
    fn tabs_switch_between_role_admin_and_moderation() {
        // サーバー管理 + BAN の権限
        let (mut app, mut rx) = app((1 << 5) | (1 << 2));
        app.open_server_settings("1");
        assert_eq!(
            app.server_settings,
            Some(("1".into(), SettingsTab::Overview))
        );
        assert!(app.role_admin.as_ref().is_some_and(|a| a.editing_guild));
        assert!(app.moderation.is_none());
        calls(&mut rx);
        app.set_settings_tab(SettingsTab::Roles);
        assert!(app.role_admin.as_ref().is_some_and(|a| !a.editing_guild));
        calls(&mut rx);
        app.set_settings_tab(SettingsTab::Mod(ModTab::Bans));
        assert!(app.role_admin.is_none());
        assert_eq!(app.moderation.as_ref().unwrap().tab, ModTab::Bans);
        assert!(matches!(calls(&mut rx)[..], [net::ApiCall::GetBans(_)]));
        app.set_settings_tab(SettingsTab::Mod(ModTab::Invites));
        assert_eq!(app.moderation.as_ref().unwrap().tab, ModTab::Invites);
        // 監査ログの権限はないので、そのタブには替えない（出さない）
        assert!(!app.can_open_settings_tab("1", SettingsTab::Mod(ModTab::Audit)));
        app.set_settings_tab(SettingsTab::Mod(ModTab::Audit));
        assert_eq!(
            app.server_settings.as_ref().unwrap().1,
            SettingsTab::Mod(ModTab::Invites)
        );
        app.close_server_settings();
        assert!(app.server_settings.is_none() && app.moderation.is_none());
    }

    #[test]
    fn busy_tab_is_not_left_or_closed() {
        let (mut app, mut rx) = app(1 << 2);
        app.open_server_settings("1");
        app.set_settings_tab(SettingsTab::Mod(ModTab::Bans));
        calls(&mut rx);
        app.moderation.as_mut().unwrap().mutation = Some((99, "x"));
        app.set_settings_tab(SettingsTab::Overview);
        assert_eq!(
            app.server_settings.as_ref().unwrap().1,
            SettingsTab::Mod(ModTab::Bans)
        );
        app.close_server_settings();
        assert!(app.server_settings.is_some(), "送信中は閉じない");
    }
}
