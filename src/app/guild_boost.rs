//! サーバーブーストの状況（メニューの 1 行目に出すだけ。ブーストの購入はしない）。
use super::*;

/// (レベル, ブーストの数)
pub type BoostStatus = (u8, u32);

fn parse_boost(v: &serde_json::Value) -> Option<BoostStatus> {
    let tier = u8::try_from(v.get("premium_tier")?.as_u64()?).ok()?.min(3);
    let count = v
        .get("premium_subscription_count")
        .and_then(|c| c.as_u64())
        .map_or(0, |c| u32::try_from(c).unwrap_or(u32::MAX));
    Some((tier, count))
}

impl App {
    /// メニューを開いたときに 1 回だけ取りに行く（サーバーごと）
    pub fn request_guild_boost(&mut self, guild_id: &str) {
        if self.guild_boost.contains_key(guild_id)
            || self.boost_requests.values().any(|g| g == guild_id)
        {
            return;
        }
        if let Some(id) = self.role_queue(net::ApiCall::GetGuild(guild_id.to_owned())) {
            self.boost_requests.insert(id, guild_id.to_owned());
        }
    }

    pub fn is_boost_request(&self, id: u64) -> bool {
        self.boost_requests.contains_key(&id)
    }

    pub fn boost_result(&mut self, id: u64, result: Result<serde_json::Value, String>) {
        let Some(gid) = self.boost_requests.remove(&id) else {
            return;
        };
        // 取れなかったときは何も出さない（メニューは「サーバーブースト」だけになる）
        if let Some(b) = result.ok().as_ref().and_then(parse_boost) {
            self.guild_boost.insert(gid, b);
        }
    }

    /// GUILD_UPDATE にもブーストの数が入っている
    pub(super) fn on_guild_update_boost(&mut self, d: &serde_json::Value) {
        if let (Some(gid), Some(b)) = (d["id"].as_str(), parse_boost(d)) {
            if self.guild_boost.contains_key(gid) {
                self.guild_boost.insert(gid.to_owned(), b);
            }
        }
    }

    /// メニューの 1 行目の文
    pub fn boost_label(&self, guild_id: &str) -> String {
        match self.guild_boost.get(guild_id) {
            Some((0, 0)) => {
                tr!("サーバーブースト: まだありません", "Server Boost: none yet").into()
            }
            Some((0, n)) => tr!("サーバーブースト: {n} ブースト", "Server Boost: {n} boosts",),
            Some((tier, n)) => tr!(
                "サーバーブースト: レベル {tier}（{n} ブースト）",
                "Server Boost: Level {tier} ({n} boosts)",
            ),
            None => tr!("サーバーブースト", "Server Boost").into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_once_and_formats_the_status() {
        let mut app = App::default();
        let (_, ev) = std::sync::mpsc::channel();
        let (cmd, mut rx) = tokio::sync::mpsc::unbounded_channel();
        app.backend = Some(Backend { cmd, ev });
        app.request_guild_boost("1");
        app.request_guild_boost("1");
        let Cmd::Api { id, call } = rx.try_recv().unwrap() else {
            panic!()
        };
        assert!(matches!(call, net::ApiCall::GetGuild(ref g) if g == "1"));
        assert!(rx.try_recv().is_err(), "取得中は二度送らない");
        assert_eq!(app.boost_label("1"), "サーバーブースト");
        assert!(app.is_boost_request(id));
        app.boost_result(
            id,
            Ok(serde_json::json!({"id": "1", "premium_tier": 2, "premium_subscription_count": 9})),
        );
        assert_eq!(
            app.boost_label("1"),
            "サーバーブースト: レベル 2（9 ブースト）"
        );
        app.request_guild_boost("1");
        assert!(rx.try_recv().is_err(), "取れたあとは送らない");
        app.on_guild_update_boost(
            &serde_json::json!({"id": "1", "premium_tier": 0, "premium_subscription_count": 1}),
        );
        assert_eq!(app.boost_label("1"), "サーバーブースト: 1 ブースト");
    }
}
