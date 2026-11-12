//! チャンネル・カテゴリの作成と、サーバーのチャンネル一覧をゲートウェイの知らせで更新する処理。
use super::*;

const ADMINISTRATOR: u64 = 1 << 3;
const MANAGE_CHANNELS: u64 = 1 << 4;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ChannelDraft {
    pub guild_id: String,
    /// 0 = テキスト、2 = ボイス、4 = カテゴリ、15 = フォーラム
    pub kind: u8,
    pub name: String,
    pub parent_id: Option<String>,
    pub pending: Option<u64>,
    pub error: Option<String>,
}

/// 作る本体を作る。テキスト系の名前は公式クライアントと同じく小文字にし、空白を - にする
pub fn channel_body(d: &ChannelDraft) -> Result<serde_json::Value, &'static str> {
    if !matches!(d.kind, 0 | 2 | 4 | 15) {
        return Err("作れない種類です");
    }
    let mut name = d.name.trim().to_owned();
    if matches!(d.kind, 0 | 15) {
        name = name
            .split_whitespace()
            .collect::<Vec<_>>()
            .join("-")
            .to_lowercase();
    }
    if name.is_empty() || name.chars().count() > 100 {
        return Err("名前は 1〜100 文字にしてください");
    }
    let mut body = serde_json::json!({ "name": name, "type": d.kind });
    if d.kind != 4 {
        if let Some(p) = &d.parent_id {
            body["parent_id"] = p.clone().into();
        }
    }
    Ok(body)
}

impl App {
    /// チャンネルを作る項目を出すか（分からないときは出しておき、Discord の判定に任せる）
    pub fn can_create_channels(&self, guild_id: &str) -> bool {
        self.guild_perms(guild_id)
            .is_none_or(|p| p & (MANAGE_CHANNELS | ADMINISTRATOR) != 0)
    }

    /// 作成の画面を開く。category = カテゴリを作る
    pub fn open_channel_create(&mut self, guild_id: &str, category: bool) {
        if !self.guilds.iter().any(|g| g.id == guild_id) || !self.can_create_channels(guild_id) {
            return;
        }
        self.channel_draft = Some(ChannelDraft {
            guild_id: guild_id.to_owned(),
            kind: if category { 4 } else { 0 },
            ..Default::default()
        });
    }

    /// サーバーのカテゴリ（作るチャンネルを入れる先の候補）
    pub fn guild_categories(&self, guild_id: &str) -> Vec<(String, String)> {
        let mut cats: Vec<&Channel> = self
            .channels
            .get(guild_id)
            .into_iter()
            .flatten()
            .filter(|c| c.kind == Channel::CATEGORY)
            .collect();
        cats.sort_by_key(|c| c.position);
        cats.into_iter()
            .map(|c| (c.id.clone(), c.name.clone().unwrap_or_default()))
            .collect()
    }

    pub fn submit_channel_create(&mut self) {
        let Some(d) = self.channel_draft.clone() else {
            return;
        };
        if d.pending.is_some() {
            return;
        }
        let body = match channel_body(&d) {
            Ok(b) => b,
            Err(e) => {
                self.channel_draft.as_mut().unwrap().error = Some(e.into());
                return;
            }
        };
        if !self.can_create_channels(&d.guild_id) {
            self.channel_draft.as_mut().unwrap().error =
                Some("チャンネルを管理する権限がありません".into());
            return;
        }
        let id = self.role_queue(net::ApiCall::CreateChannel {
            guild_id: d.guild_id,
            body,
        });
        let draft = self.channel_draft.as_mut().unwrap();
        draft.error = None;
        match id {
            Some(id) => draft.pending = Some(id),
            None => draft.error = Some("送信できませんでした。再接続してください".into()),
        }
    }

    pub fn is_channel_create_request(&self, id: u64) -> bool {
        self.channel_draft
            .as_ref()
            .is_some_and(|d| d.pending == Some(id))
    }

    pub fn channel_create_result(&mut self, result: Result<serde_json::Value, String>) {
        let Some(d) = self.channel_draft.as_mut() else {
            return;
        };
        d.pending = None;
        match result {
            Ok(v) => {
                let gid = d.guild_id.clone();
                self.channel_draft = None;
                let created = serde_json::from_value::<Channel>(v).ok();
                if let Some(c) = created {
                    let (id, text) = (c.id.clone(), c.is_text());
                    self.upsert_guild_channel(&gid, c);
                    // 作ったテキストチャンネルを開く（公式クライアントと同じ）
                    if text && self.sel_guild.as_deref() == Some(gid.as_str()) {
                        self.open_channel(id);
                    }
                }
                self.show_toast("作成しました");
            }
            Err(e) => d.error = Some(super::pins::api_error_text(&e, "作成できませんでした")),
        }
    }

    /// サーバーのチャンネルを足す・置き換える（一覧の並び: テキスト→ボイス、position 順）
    pub(super) fn upsert_guild_channel(&mut self, guild_id: &str, c: Channel) {
        let Some(list) = self.channels.get_mut(guild_id) else {
            // 一覧をまだ読んでいなければ、開いたときに読む
            return;
        };
        match list.iter_mut().find(|x| x.id == c.id) {
            Some(slot) => *slot = c,
            None => list.push(c),
        }
        list.sort_by_key(|c| (c.is_voice(), c.position));
    }

    /// CHANNEL_CREATE / CHANNEL_UPDATE / CHANNEL_DELETE（サーバーのチャンネルだけ。DM は別に扱う）
    pub(super) fn on_guild_channel_event(&mut self, t: &str, d: &serde_json::Value) {
        let Some(gid) = d["guild_id"].as_str().map(str::to_owned) else {
            return;
        };
        if t == "CHANNEL_DELETE" {
            let Some(id) = d["id"].as_str() else { return };
            if let Some(list) = self.channels.get_mut(&gid) {
                list.retain(|c| c.id != id);
            }
            if self.sel_channel.as_deref() == Some(id) && self.sel_guild.as_deref() == Some(&gid) {
                self.sel_channel = None;
            }
            return;
        }
        if let Ok(c) = serde_json::from_value::<Channel>(d.clone()) {
            self.upsert_guild_channel(&gid, c);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app(perms: u64) -> (App, tokio::sync::mpsc::UnboundedReceiver<Cmd>) {
        let mut app = App {
            guilds: vec![serde_json::from_value(serde_json::json!({
                "id": "1", "name": "g", "permissions": perms.to_string()
            }))
            .unwrap()],
            ..Default::default()
        };
        app.channels.insert("1".into(), Vec::new());
        let (_, ev) = std::sync::mpsc::channel();
        let (cmd, rx) = tokio::sync::mpsc::unbounded_channel();
        app.backend = Some(Backend { cmd, ev });
        (app, rx)
    }

    #[test]
    fn body_normalizes_text_names_and_drops_parent_for_categories() {
        let d = |kind, name: &str| ChannelDraft {
            kind,
            name: name.into(),
            parent_id: Some("9".into()),
            ..Default::default()
        };
        let b = channel_body(&d(0, "  Hello  World ")).unwrap();
        assert_eq!(b["name"], "hello-world");
        assert_eq!(b["parent_id"], "9");
        assert_eq!(
            channel_body(&d(2, " ボイス 1 ")).unwrap()["name"],
            "ボイス 1"
        );
        let cat = channel_body(&d(4, "Cat")).unwrap();
        assert!(cat.get("parent_id").is_none());
        assert!(channel_body(&d(0, "   ")).is_err());
        assert!(channel_body(&d(0, &"a".repeat(101))).is_err());
        assert!(channel_body(&d(13, "stage")).is_err());
    }

    #[test]
    fn create_sends_once_and_adds_the_channel() {
        let (mut app, mut rx) = app(MANAGE_CHANNELS);
        app.sel_guild = Some("1".into());
        app.open_channel_create("1", false);
        app.channel_draft.as_mut().unwrap().name = "general".into();
        app.submit_channel_create();
        app.submit_channel_create();
        let Cmd::Api { id, call } = rx.try_recv().unwrap() else {
            panic!()
        };
        assert!(rx.try_recv().is_err(), "送信中は二重に送らない");
        assert!(matches!(call, net::ApiCall::CreateChannel { .. }));
        assert!(app.is_channel_create_request(id));
        app.channel_create_result(Ok(serde_json::json!({
            "id": "50", "type": 0, "name": "general", "guild_id": "1"
        })));
        assert!(app.channel_draft.is_none());
        assert_eq!(app.channels["1"][0].id, "50");
        assert_eq!(app.sel_channel.as_deref(), Some("50"));
    }

    #[test]
    fn failure_keeps_the_draft_and_no_permission_sends_nothing() {
        let (mut app, mut rx) = app(MANAGE_CHANNELS);
        app.open_channel_create("1", true);
        app.channel_draft.as_mut().unwrap().name = "cat".into();
        app.submit_channel_create();
        rx.try_recv().unwrap();
        app.channel_create_result(Err("POST /guilds/1/channels: 403 Forbidden {}".into()));
        let d = app.channel_draft.as_ref().unwrap();
        assert!(d.error.as_deref().unwrap().contains("権限"));
        assert_eq!(d.name, "cat");
        let (mut none, mut rx) = self::app(0);
        none.open_channel_create("1", false);
        assert!(none.channel_draft.is_none());
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn gateway_events_update_the_channel_list() {
        let (mut app, _) = app(0);
        app.sel_guild = Some("1".into());
        let ch = |id: &str, kind: u8, pos: i32| serde_json::json!({"id": id, "type": kind, "name": id, "position": pos, "guild_id": "1"});
        app.on_guild_channel_event("CHANNEL_CREATE", &ch("v", 2, 0));
        app.on_guild_channel_event("CHANNEL_CREATE", &ch("b", 0, 1));
        app.on_guild_channel_event("CHANNEL_CREATE", &ch("a", 0, 0));
        let ids: Vec<&str> = app.channels["1"].iter().map(|c| c.id.as_str()).collect();
        assert_eq!(ids, ["a", "b", "v"]);
        app.on_guild_channel_event("CHANNEL_UPDATE", &ch("b", 0, -1));
        assert_eq!(app.channels["1"][0].id, "b");
        app.sel_channel = Some("a".into());
        app.on_guild_channel_event("CHANNEL_DELETE", &ch("a", 0, 0));
        assert_eq!(app.channels["1"].len(), 2);
        assert!(app.sel_channel.is_none());
        // DM の知らせ（guild_id なし）では何もしない
        app.on_guild_channel_event(
            "CHANNEL_CREATE",
            &serde_json::json!({"id": "dm", "type": 1}),
        );
        assert_eq!(app.channels["1"].len(), 2);
    }
}
