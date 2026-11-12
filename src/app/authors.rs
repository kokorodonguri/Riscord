//! REST で読んだメッセージには投稿者のサーバーでの情報（ニックネーム・ロール・サーバー別アバター）が付かない。
//! 手元のメンバーから補い、知らない投稿者はゲートウェイ（op 8）でまとめて問い合わせて、届いたら書き足す。
//! 描画はメッセージの member をそのまま使うので、毎フレームのメンバー探しは要らない。
use super::*;

/// op 8 で一度に問い合わせるユーザー数の上限（Discord の上限）
const MEMBER_REQUEST_CHUNK: usize = 100;

impl App {
    /// チャンネル（スレッドを含む）が属するサーバー
    pub fn guild_of_channel(&self, channel_id: &str) -> Option<String> {
        self.channels
            .iter()
            .chain(self.threads.iter())
            .find(|(_, chs)| chs.iter().any(|c| c.id == channel_id))
            .map(|(g, _)| g.clone())
    }

    /// 読んだメッセージに投稿者の member を補い、分からない投稿者を問い合わせる
    pub fn fill_message_members(&mut self, channel_id: &str, messages: &mut [Message]) {
        let Some(gid) = self.guild_of_channel(channel_id) else {
            return;
        };
        let known: HashMap<&str, &Member> = self
            .members
            .get(&gid)
            .into_iter()
            .flatten()
            .filter_map(|m| Some((m.user.as_ref()?.id.as_str(), m)))
            .collect();
        let requested = self.requested_members.entry(gid.clone()).or_default();
        let mut ask = Vec::new();
        let mut fill = |m: &mut Message| {
            if m.member.is_some() || m.author.id.is_empty() {
                return;
            }
            match known.get(m.author.id.as_str()) {
                Some(member) => m.member = Some(strip_user(member)),
                None => {
                    if requested.insert(m.author.id.clone()) {
                        ask.push(m.author.id.clone());
                    }
                }
            }
        };
        for m in messages.iter_mut() {
            fill(m);
            if let Some(r) = m.referenced_message.as_deref_mut() {
                fill(r);
            }
        }
        for chunk in ask.chunks(MEMBER_REQUEST_CHUNK) {
            self.send(Cmd::Gateway(net::request_guild_members(
                &gid,
                None,
                Some(chunk),
                0,
                false,
            )));
        }
    }

    /// 問い合わせたメンバーが届いたら、そのサーバーの読み込み済みメッセージに書き足す
    pub(super) fn apply_requested_members(&mut self, gid: &str, members: &[Member]) {
        let Some(requested) = self.requested_members.get(gid) else {
            return;
        };
        let found: HashMap<&str, &Member> = members
            .iter()
            .filter_map(|m| Some((m.user.as_ref()?.id.as_str(), m)))
            .filter(|(id, _)| requested.contains(*id))
            .collect();
        if found.is_empty() {
            return;
        }
        let channels: HashSet<&str> = self
            .channels
            .get(gid)
            .into_iter()
            .chain(self.threads.get(gid))
            .flatten()
            .map(|c| c.id.as_str())
            .collect();
        let mut changed = false;
        for (cid, list) in self.messages.iter_mut() {
            if !channels.contains(cid.as_str()) {
                continue;
            }
            for m in list.iter_mut() {
                changed |= patch(m, &found);
                if let Some(r) = m.referenced_message.as_deref_mut() {
                    changed |= patch(r, &found);
                }
            }
        }
        if changed {
            self.bump_msgs();
        }
    }
}

fn patch(m: &mut Message, found: &HashMap<&str, &Member>) -> bool {
    if m.member.is_some() {
        return false;
    }
    let Some(member) = found.get(m.author.id.as_str()) else {
        return false;
    };
    m.member = Some(strip_user(member));
    true
}

/// メッセージに付ける member には user を入れない（Discord の形と同じ。author と重複する）
fn strip_user(m: &Member) -> Member {
    Member {
        user: None,
        ..m.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn msg(id: &str, author: &str) -> Message {
        serde_json::from_value(serde_json::json!({
            "id": id, "channel_id": "c1", "content": "",
            "timestamp": "2026-01-01T00:00:00+00:00",
            "author": {"id": author, "username": author},
        }))
        .unwrap()
    }

    fn member(id: &str, nick: &str) -> Member {
        serde_json::from_value(serde_json::json!({
            "user": {"id": id, "username": id}, "nick": nick, "roles": ["r1"],
        }))
        .unwrap()
    }

    #[test]
    fn rest_messages_get_known_members_and_ask_once_for_the_rest() {
        let mut app = App::default();
        let (_, ev) = std::sync::mpsc::channel();
        let (cmd, mut rx) = tokio::sync::mpsc::unbounded_channel();
        app.backend = Some(Backend { cmd, ev });
        app.channels.insert(
            "g1".into(),
            vec![serde_json::from_value(serde_json::json!({"id":"c1","type":0})).unwrap()],
        );
        app.members.insert("g1".into(), vec![member("u1", "known")]);
        let mut list = vec![msg("1", "u1"), msg("2", "u2"), msg("3", "u2")];
        app.fill_message_members("c1", &mut list);
        assert_eq!(
            list[0].member.as_ref().unwrap().nick.as_deref(),
            Some("known")
        );
        assert!(list[0].member.as_ref().unwrap().user.is_none());
        assert!(list[1].member.is_none());
        let Cmd::Gateway(v) = rx.try_recv().unwrap() else {
            panic!("op 8 を送る")
        };
        assert_eq!(v["op"], 8);
        assert_eq!(v["d"]["user_ids"], serde_json::json!(["u2"]));
        // 同じ人は二度聞かない
        let mut again = vec![msg("4", "u2")];
        app.fill_message_members("c1", &mut again);
        assert!(rx.try_recv().is_err());
        // 届いたら読み込み済みのメッセージに書き足す
        app.messages.insert("c1".into(), list);
        app.apply_requested_members("g1", &[member("u2", "later")]);
        let stored = &app.messages["c1"];
        assert_eq!(
            stored[1].member.as_ref().unwrap().nick.as_deref(),
            Some("later")
        );
        assert_eq!(stored[2].member.as_ref().unwrap().roles, ["r1"]);
        // DM（サーバーのないチャンネル）は何もしない
        let mut dm = vec![msg("9", "u9")];
        app.fill_message_members("dm", &mut dm);
        assert!(dm[0].member.is_none() && rx.try_recv().is_err());
    }
}
