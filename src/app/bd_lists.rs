//! BetterDiscord 由来の同梱プラグインのうち、サーバー・チャンネル・DM・フレンドの一覧に関わるもの
use super::*;

/// 「a, b, c」を読む
fn list(s: &str) -> Vec<String> {
    s.split([',', ' ', ';', '\n'])
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .collect()
}

impl App {
    /// ServerHider: 隠すサーバーか
    pub fn bd_hidden_guild(&self, guild_id: &str) -> bool {
        self.plugins.on("server-hider")
            && list(&self.plugins.opt_str("server-hider", "hidden"))
                .iter()
                .any(|g| g == guild_id)
            && self.sel_guild.as_deref() != Some(guild_id)
    }

    /// ServerHider に頼んでサーバーを隠す・戻す
    pub fn toggle_hidden_guild(&mut self, guild_id: &str) {
        self.plugins.page_action(
            "server-hider",
            &format!("toggle:{guild_id}"),
            Default::default(),
        );
    }

    /// オンラインのフレンドの数
    pub fn online_friend_count(&self) -> usize {
        self.friends
            .relationships
            .iter()
            .filter(|r| r.kind == 1)
            .filter(|r| {
                self.presences
                    .get(&r.user.id)
                    .is_some_and(|p| p.status != "offline")
            })
            .count()
    }

    /// サーバーのメンバーのうちオンラインの数（読み込んでいる分）
    pub fn online_member_count(&self, guild_id: &str) -> usize {
        self.members
            .get(guild_id)
            .into_iter()
            .flatten()
            .filter(|m| {
                m.user.as_ref().is_some_and(|u| {
                    self.presences
                        .get(&u.id)
                        .is_some_and(|p| p.status != "offline")
                })
            })
            .count()
    }

    /// BetterGuildTooltip・ServerDetails: サーバーのアイコンに重ねたときの説明。None なら名前だけ
    pub fn bd_guild_tip(&self, g: &Guild) -> Option<String> {
        let details = self.plugins.on("server-details");
        let counts = self.plugins.on("better-guild-tooltip") || details;
        if !counts {
            return None;
        }
        let info = self.guild_info.get(&g.id);
        let mut lines = vec![self.guild_label(&g.id, &g.name)];
        let online = self.online_member_count(&g.id);
        match info.and_then(|i| i.member_count) {
            Some(total) if online > 0 => lines.push(tr!(
                "オンライン {online} / メンバー {total}",
                "{online} online / {total} members",
                "在线 {online} / 成员 {total}",
            )),
            Some(total) => lines.push(tr!("メンバー {total}", "{total} members", "成员 {total}",)),
            None if online > 0 => lines.push(tr!(
                "オンライン {online}",
                "{online} online",
                "在线 {online}",
            )),
            None => {}
        }
        if details {
            if let Some(owner) = info.and_then(|i| i.owner_id.as_deref()) {
                let name = self.user_name(owner).unwrap_or_else(|| owner.to_owned());
                lines.push(tr!("オーナー: {name}", "Owner: {name}", "所有者：{name}",));
            }
            lines.push(tr!(
                "作成: {}",
                "Created: {}",
                "创建于：{}",
                snowflake_date(&g.id)
            ));
            if let Some(j) = info.and_then(|i| i.joined_at.as_deref()).and_then(local_dt) {
                lines.push(tr!(
                    "参加: {}",
                    "Joined: {}",
                    "加入于：{}",
                    j.format("%Y-%m-%d")
                ));
            }
            if let Some(tier) = info.map(|i| i.premium_tier).filter(|t| *t > 0) {
                lines.push(tr!(
                    "ブーストレベル {tier}",
                    "Boost level {tier}",
                    "助力等级 {tier}",
                ));
            }
            if let Some(d) = info
                .and_then(|i| i.description.as_deref())
                .filter(|d| !d.is_empty())
            {
                lines.push(crate::notify::excerpt(d, 120));
            }
        }
        Some(lines.join("\n"))
    }

    /// GuildProfile: 情報ダイアログの本文
    pub fn guild_profile_text(&self, guild_id: &str) -> Option<(String, String)> {
        let g = self.guilds.iter().find(|g| g.id == guild_id)?;
        let mut body = self.bd_guild_tip_full(g);
        let friends: Vec<String> = self
            .friends
            .relationships
            .iter()
            .filter(|r| r.kind == 1)
            .filter(|r| {
                self.members
                    .get(guild_id)
                    .into_iter()
                    .flatten()
                    .any(|m| m.user.as_ref().is_some_and(|u| u.id == r.user.id))
            })
            .map(|r| r.user.display().to_owned())
            .take(30)
            .collect();
        if !friends.is_empty() {
            body.push_str(&tr!(
                "\n\nこのサーバーにいるフレンド（読み込んだメンバーのうち）:\n{}",
                "\n\nFriends in this server (among loaded members):\n{}",
                "\n\n此服务器中的好友（已加载成员中）：\n{}",
                friends.join(", ")
            ));
        }
        let channels = self.channels.get(guild_id).map(Vec::len).unwrap_or(0);
        let roles = self.roles.get(guild_id).map(Vec::len).unwrap_or(0);
        let emojis = self.guild_emojis.get(guild_id).map(Vec::len).unwrap_or(0);
        body.push_str(&tr!(
            "\n\nチャンネル {channels} · ロール {roles} · 絵文字 {emojis}\nID: {}",
            "\n\n{channels} channels · {roles} roles · {emojis} emojis\nID: {}",
            "\n\n频道 {channels} · 身份组 {roles} · 表情 {emojis}\nID：{}",
            g.id
        ));
        Some((self.guild_label(&g.id, &g.name), body))
    }

    /// ServerDetails と同じ中身（プラグインの有無に関わらず）
    fn bd_guild_tip_full(&self, g: &Guild) -> String {
        let info = self.guild_info.get(&g.id);
        let mut lines = Vec::new();
        if let Some(total) = info.and_then(|i| i.member_count) {
            lines.push(tr!("メンバー {total}", "{total} members", "成员 {total}",));
        }
        if let Some(owner) = info.and_then(|i| i.owner_id.as_deref()) {
            let name = self.user_name(owner).unwrap_or_else(|| owner.to_owned());
            lines.push(tr!("オーナー: {name}", "Owner: {name}", "所有者：{name}",));
        }
        lines.push(tr!(
            "作成: {}",
            "Created: {}",
            "创建于：{}",
            snowflake_date(&g.id)
        ));
        if let Some(j) = info.and_then(|i| i.joined_at.as_deref()).and_then(local_dt) {
            lines.push(tr!(
                "参加: {}",
                "Joined: {}",
                "加入于：{}",
                j.format("%Y-%m-%d")
            ));
        }
        if let Some(d) = info
            .and_then(|i| i.description.as_deref())
            .filter(|d| !d.is_empty())
        {
            lines.push(d.to_owned());
        }
        lines.join("\n")
    }

    /// ReadAllNotificationsButton: すべてのサーバーと DM を既読にする
    pub fn mark_all_read(&mut self) {
        let guilds: Vec<String> = self
            .unread_guilds()
            .into_iter()
            .flatten()
            .map(str::to_owned)
            .collect();
        for g in &guilds {
            self.mark_guild_read(g);
        }
        // DM は最後のメッセージまで既読にする
        let dms: Vec<(String, String)> = self
            .unread
            .iter()
            .filter(|(_, g)| g.is_none())
            .filter_map(|(c, _)| {
                let last = self
                    .dms
                    .iter()
                    .find(|d| &d.id == c)?
                    .last_message_id
                    .clone()?;
                Some((c.clone(), last))
            })
            .collect();
        if !dms.is_empty() {
            self.api(net::ApiCall::AckBulk(dms));
        }
        self.unread.clear();
        self.mention_counts.clear();
        self.show_toast(tr!(
            "すべて既読にしました",
            "Marked everything as read",
            "已全部标为已读"
        ));
    }

    /// PinDMs: 上に固定する DM のチャンネル ID
    pub fn pinned_dms(&self) -> Vec<String> {
        if !self.plugins.on("pin-dms") {
            return Vec::new();
        }
        list(&self.plugins.opt_str("pin-dms", "pinned"))
    }

    /// PinDMs に頼んで DM を固定・解除する
    pub fn toggle_pinned_dm(&mut self, channel_id: &str) {
        self.plugins.page_action(
            "pin-dms",
            &format!("toggle:{channel_id}"),
            Default::default(),
        );
    }

    /// MessagePeek: チャンネルの最後のメッセージ（読み込んでいれば）
    pub fn last_message_line(&self, channel_id: &str) -> Option<String> {
        if !self.plugins.on("message-peek") {
            return None;
        }
        let m = self.messages.get(channel_id)?.last()?;
        let body = if m.content.is_empty() {
            if m.attachments.is_empty() {
                return None;
            }
            tr!("添付ファイル", "Attachment", "附件").to_string()
        } else {
            m.content.clone()
        };
        Some(format!(
            "{}: {}",
            self.shown_name(&m.author.id, author_name(m)),
            crate::notify::excerpt(&body, 60)
        ))
    }

    /// ChannelsPreview: チャンネルにマウスを重ねたときの最近のメッセージ（読み込んでいれば）
    pub fn channel_preview(&self, channel_id: &str) -> Option<String> {
        if !self.plugins.on("channels-preview") {
            return None;
        }
        let list = self.messages.get(channel_id)?;
        let lines: Vec<String> = list
            .iter()
            .rev()
            .take(5)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .map(|m| {
                format!(
                    "{}: {}",
                    self.shown_name(&m.author.id, author_name(m)),
                    crate::notify::excerpt(&m.content, 80)
                )
            })
            .collect();
        (!lines.is_empty()).then(|| lines.join("\n"))
    }

    /// ChannelTabs・Tabbys: 開いたチャンネルをタブに足す
    pub fn remember_tab(&mut self, channel_id: &str) {
        if !(self.plugins.on("channel-tabs") || self.plugins.on("tabbys")) {
            return;
        }
        if !self.tabs.iter().any(|c| c == channel_id) {
            self.tabs.push(channel_id.to_owned());
            let max = self
                .plugins
                .opt_f64("channel-tabs", "max")
                .unwrap_or(10.0)
                .clamp(2.0, 30.0) as usize;
            while self.tabs.len() > max {
                // 固定（ブックマーク）していないものから消す
                let bookmarks = self.tab_bookmarks();
                match self
                    .tabs
                    .iter()
                    .position(|t| !bookmarks.contains(t) && t != channel_id)
                {
                    Some(i) => {
                        self.tabs.remove(i);
                    }
                    None => break,
                }
            }
        }
    }

    /// Tabbys のブックマーク（閉じないタブ）
    pub fn tab_bookmarks(&self) -> Vec<String> {
        list(&self.plugins.opt_str("tabbys", "bookmarks"))
    }

    /// タブの名前
    pub fn tab_label(&self, channel_id: &str) -> String {
        if let Some(c) = self.dms.iter().find(|c| c.id == channel_id) {
            return format!("@{}", c.dm_label());
        }
        for (g, chs) in self.channels.iter().chain(self.threads.iter()) {
            if let Some(c) = chs.iter().find(|c| c.id == channel_id) {
                return format!(
                    "#{}",
                    self.channel_label(&c.id, c.name.as_deref().unwrap_or_default())
                ) + &self
                    .guilds
                    .iter()
                    .find(|x| &x.id == g)
                    .map(|x| format!(" · {}", self.guild_label(&x.id, &x.name)))
                    .unwrap_or_default();
            }
        }
        channel_id.to_owned()
    }

    /// LazyLoadChannels: 開いてもすぐには読まないチャンネルか
    pub fn lazy_skip(&self, channel_id: &str) -> bool {
        self.plugins.on("lazy-load-channels")
            && !self.lazy_allowed.contains(channel_id)
            && !list(&self.plugins.opt_str("lazy-load-channels", "always"))
                .iter()
                .any(|c| c == channel_id)
    }

    /// LazyLoadChannels: 読み込むことにした
    pub fn lazy_load(&mut self, channel_id: &str) {
        self.lazy_allowed.insert(channel_id.to_owned());
        self.load_messages(channel_id.to_owned());
    }

    /// StreamingChannels: 配信中に中身を隠すチャンネルか
    pub fn streaming_hidden(&self, channel_id: &str) -> bool {
        if !self.plugins.on("streaming-channels") {
            return false;
        }
        let me = self.me.as_ref().map(|u| u.id.as_str()).unwrap_or("");
        if !self.is_streaming(me) {
            return false;
        }
        let ids = list(&self.plugins.opt_str("streaming-channels", "channels"));
        ids.is_empty() || ids.iter().any(|c| c == channel_id)
    }

    /// HideMutedCategories: 隠すカテゴリか（ミュートしたチャンネルを隠しているとき）
    pub fn hide_muted_category(&self, guild_id: &str, category: &Channel) -> bool {
        self.plugins.on("hide-muted-categories")
            && self.settings.hide_muted_guilds.contains(guild_id)
            && self
                .guild_settings
                .get(guild_id)
                .is_some_and(|s| s.muted_channels.contains(&category.id))
    }

    /// StartUpPage: 起動して一覧がそろったら一度だけ、決めた場所を開く
    pub fn startup_page(&mut self) {
        if self.startup_done || !self.plugins.on("start-up-page") || self.guilds.is_empty() {
            return;
        }
        self.startup_done = true;
        let target = self.plugins.opt_str("start-up-page", "page");
        let target = target.trim();
        if target == "friends" {
            self.sel_guild = None;
            self.friends.open = true;
            if !self.friends.loaded {
                self.fetch_friends();
            }
        } else if !target.is_empty() && target.bytes().all(|b| b.is_ascii_digit()) {
            let guild = self.guild_of_channel(target);
            *self.notify_clicked.lock().unwrap() = Some((guild, target.to_owned()));
        }
    }

    /// RemovedConnectionAlerts: フレンド・サーバーから外れたことをプラグインへ
    pub fn removed_connection(&mut self, t: &str, d: &serde_json::Value) {
        let (kind, id) = match t {
            "RELATIONSHIP_REMOVE" => ("friend", d["id"].as_str()),
            "GUILD_DELETE" if !d["unavailable"].as_bool().unwrap_or(false) => {
                ("guild", d["id"].as_str())
            }
            _ => return,
        };
        let Some(id) = id else { return };
        let label = match kind {
            "friend" => self
                .friends
                .relationships
                .iter()
                .find(|r| r.id == id || r.user.id == id)
                .filter(|r| r.kind == 1)
                .map(|r| r.user.display().to_owned()),
            _ => self
                .guilds
                .iter()
                .find(|g| g.id == id)
                .map(|g| g.name.clone()),
        };
        let Some(label) = label else { return };
        self.plugins
            .hook("on_removed", &[kind.into(), id.into(), label.into()]);
    }
}

impl App {
    /// FriendCodes: フレンド招待（discord.gg/…）を作ってコピーする
    pub fn create_friend_code(&mut self) {
        let id = self.api(net::ApiCall::Raw {
            method: reqwest::Method::POST,
            path: "/users/@me/invites".into(),
            body: Some(serde_json::json!({})),
        });
        self.friend_code_request = Some(id);
    }

    /// FriendCodes の結果
    pub fn friend_code_result(
        &mut self,
        id: u64,
        result: &Result<serde_json::Value, String>,
    ) -> bool {
        if self.friend_code_request != Some(id) {
            return false;
        }
        self.friend_code_request = None;
        match result.as_ref().ok().and_then(|v| v["code"].as_str()) {
            Some(code) => {
                let link = format!("https://discord.gg/{code}");
                self.clipboard_out = Some(link.clone());
                self.info_dialog = Some((
                    "FriendCodes".into(),
                    tr!(
                        "フレンド招待を作ってコピーしました:\n{link}\n\nこのリンクを開いた人から、フレンド申請が届きます。",
                        "Created and copied a friend invite:\n{link}\n\nPeople who open this link will send you a friend request.",
                        "已创建并复制好友邀请：\n{link}\n\n打开此链接的人会向你发送好友请求。",
                    ),
                ));
            }
            None => self.show_toast(tr!(
                "フレンド招待を作れませんでした",
                "Couldn't create a friend invite",
                "无法创建好友邀请"
            )),
        }
        true
    }
}
