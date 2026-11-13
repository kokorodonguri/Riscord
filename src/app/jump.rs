//! メッセージへ飛ぶ（ピン一覧・検索結果から）。
//!
//! 読み込み済みの範囲にあればそこへ移るだけ。無ければ `around` で前後を読み、その範囲だけを一覧にする。
//! この一覧は最新とつながっていない（`History::newer`）ので、その間は新着を足さず、既読も送らず、
//! 控え（cache）にも書かない。下へ読み進めるか「最新のメッセージへ」で最新に戻る。
use super::*;
use crate::net::ApiCall;

/// 飛ぶ先を探している途中
pub struct Jump {
    pub channel_id: String,
    pub message_id: String,
    /// 前後の読み込みを頼んだ
    pub around: bool,
}

/// 飛んだ先の行を目立たせる時間
pub const JUMP_FLASH: std::time::Duration = std::time::Duration::from_secs(2);

/// Discord の ID（数字の文字列）の並び順
pub fn id_order(a: &str, b: &str) -> std::cmp::Ordering {
    (a.len(), a).cmp(&(b.len(), b))
}

/// 新しい順で届いた一覧を古い順にし、重複を除く
pub fn sorted_page(mut list: Vec<Message>) -> Vec<Message> {
    list.sort_by(|a, b| id_order(&a.id, &b.id));
    list.dedup_by(|a, b| a.id == b.id);
    list
}

impl App {
    /// そのメッセージのところへ移る（必要ならサーバー・チャンネルも移る）
    pub fn jump_to_message(
        &mut self,
        guild_id: Option<String>,
        channel_id: String,
        message_id: String,
    ) {
        if self.sel_channel.as_deref() != Some(channel_id.as_str()) {
            self.select(guild_id, Some(channel_id.clone()));
        }
        self.jump = Some(Jump {
            channel_id,
            message_id,
            around: false,
        });
        self.resolve_jump();
    }

    /// 一覧が届いたあとなどに、飛ぶ先が見つかったか確かめる
    pub fn resolve_jump(&mut self) {
        let Some(j) = self.jump.as_mut() else { return };
        // 途中で別のチャンネルへ移ったらやめる
        if self.sel_channel.as_deref() != Some(j.channel_id.as_str()) {
            self.jump = None;
            return;
        }
        // 開いたばかりで最新の一覧を待っている
        if self.pending_message_loads.contains_key(&j.channel_id) {
            return;
        }
        let Some(list) = self.messages.get(&j.channel_id) else {
            return;
        };
        if list.iter().any(|m| m.id == j.message_id) {
            let j = self.jump.take().unwrap();
            self.jump_flash = Some((j.message_id.clone(), std::time::Instant::now()));
            self.scroll_to_message = Some((j.channel_id, j.message_id));
            self.repaint_after(JUMP_FLASH);
            return;
        }
        if j.around {
            // 前後を読んでも無かった（削除済みなど）
            self.jump = None;
            self.show_toast(tr!("メッセージが見つかりませんでした", "Message not found"));
            return;
        }
        j.around = true;
        let call = ApiCall::GetMessages {
            channel_id: j.channel_id.clone(),
            before: None,
            after: None,
            around: Some(j.message_id.clone()),
            limit: net::HISTORY_PAGE as u32,
        };
        self.send(Cmd::Api { id: 0, call });
    }

    /// 飛んだ先の行を目立たせているか
    pub fn is_jump_flash(&self, id: &str) -> bool {
        self.jump_flash
            .as_ref()
            .is_some_and(|(m, at)| m == id && at.elapsed() < JUMP_FLASH)
    }

    /// 最新とつながっていない一覧を出しているか
    pub fn is_detached(&self, channel_id: &str) -> bool {
        self.history.get(channel_id).is_some_and(|h| h.newer)
    }

    /// 前後を読んだ結果
    pub(super) fn on_around_loaded(
        &mut self,
        channel_id: String,
        message_id: String,
        result: Result<serde_json::Value, String>,
    ) {
        let waiting = self
            .jump
            .as_ref()
            .is_some_and(|j| j.channel_id == channel_id && j.message_id == message_id && j.around);
        if !waiting {
            return;
        }
        let list = match result
            .map(|v| serde_json::from_value::<Vec<Message>>(v).map_err(|e| e.to_string()))
        {
            Ok(Ok(list)) => sorted_page(list),
            Ok(Err(e)) | Err(e) => {
                eprintln!("メッセージの前後を読めませんでした: {e}");
                self.jump = None;
                self.show_toast(tr!(
                    "メッセージを読み込めませんでした",
                    "Couldn't load the message"
                ));
                return;
            }
        };
        // 応答待ちの間に破棄されたチャンネルは復活させない
        if !self.loaded_channels.iter().any(|id| id == &channel_id) {
            self.jump = None;
            return;
        }
        let mut list = list;
        self.fill_message_members(&channel_id, &mut list);
        let latest = self.channel_last_message(&channel_id);
        let newer = match (&latest, list.last()) {
            (Some(l), Some(last)) => id_order(&last.id, l).is_lt(),
            // 最新の ID が分からなければ、つながっていない扱いにする（「最新へ」で戻れる）
            _ => true,
        };
        if newer {
            self.history.insert(
                channel_id.clone(),
                History {
                    newer: true,
                    ..Default::default()
                },
            );
            self.stale_channels.remove(&channel_id);
            if let Some(old) = self.messages.insert(channel_id, list) {
                if old.iter().any(|m| !m.sticker_items.is_empty()) {
                    self.prune_stickers();
                }
            }
            self.bump_msgs();
        } else if let Some(cur) = self.messages.get_mut(&channel_id) {
            // 最新まで届いている: 今の一覧に足すだけ（つながっていない一覧は作らない）
            let have: HashSet<String> = cur.iter().map(|m| m.id.clone()).collect();
            cur.extend(list.into_iter().filter(|m| !have.contains(&m.id)));
            cur.sort_by(|a, b| id_order(&a.id, &b.id));
            self.bump_msgs();
        }
        self.resolve_jump();
    }

    /// つながっていない一覧の下端に近づいたら、続きを読む
    pub fn load_newer(&mut self, channel_id: &str) {
        let Some(h) = self.history.get_mut(channel_id) else {
            return;
        };
        if !h.newer
            || h.pending_newer.is_some()
            || h.newer_retry_at
                .is_some_and(|t| std::time::Instant::now() < t)
        {
            return;
        }
        let Some(last) = self
            .messages
            .get(channel_id)
            .and_then(|l| l.last())
            .map(|m| m.id.clone())
        else {
            return;
        };
        h.pending_newer = Some(last.clone());
        h.newer_retry_at = None;
        let call = ApiCall::GetMessages {
            channel_id: channel_id.to_string(),
            before: None,
            after: Some(last),
            around: None,
            limit: net::HISTORY_PAGE as u32,
        };
        self.send(Cmd::Api { id: 0, call });
    }

    pub(super) fn on_newer_loaded(
        &mut self,
        channel_id: String,
        after: String,
        result: Result<serde_json::Value, String>,
    ) {
        let Some(h) = self.history.get_mut(&channel_id) else {
            return;
        };
        if h.pending_newer.as_deref() != Some(after.as_str()) {
            return;
        }
        h.pending_newer = None;
        let page = match result
            .map(|v| serde_json::from_value::<Vec<Message>>(v).map_err(|e| e.to_string()))
        {
            Ok(Ok(p)) => sorted_page(p),
            Ok(Err(e)) | Err(e) => {
                eprintln!("続きのメッセージを読めませんでした: {e}");
                h.newer_retry_at =
                    Some(std::time::Instant::now() + std::time::Duration::from_secs(5));
                self.repaint_after(std::time::Duration::from_secs(5));
                return;
            }
        };
        h.newer = page.len() >= net::HISTORY_PAGE;
        let mut page = page;
        self.fill_message_members(&channel_id, &mut page);
        let Some(list) = self.messages.get_mut(&channel_id) else {
            return;
        };
        // 待っている間に一覧が入れ替わっていたら捨てる
        if list.last().map(|m| m.id.as_str()) != Some(after.as_str()) {
            if let Some(h) = self.history.get_mut(&channel_id) {
                h.newer = true;
            }
            return;
        }
        let have: HashSet<String> = list.iter().map(|m| m.id.clone()).collect();
        list.extend(page.into_iter().filter(|m| !have.contains(&m.id)));
        if list.len() > MAX_HISTORY {
            let cut = list.len() - MAX_HISTORY;
            list.drain(..cut);
            if let Some(h) = self.history.get_mut(&channel_id) {
                h.end = false;
            }
        }
        self.bump_msgs();
    }

    /// つながっていない一覧をやめて、最新を読み直す
    pub fn back_to_latest(&mut self, channel_id: &str) {
        if self.is_detached(channel_id) {
            self.load_messages(channel_id.to_string());
        }
    }

    /// チャンネルの最新メッセージの ID（分かれば）
    pub fn channel_last_message(&self, channel_id: &str) -> Option<String> {
        self.dms
            .iter()
            .chain(self.channels.values().flatten())
            .chain(self.threads.values().flatten())
            .find(|c| c.id == channel_id)
            .and_then(|c| c.last_message_id.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn msg(id: &str) -> Message {
        serde_json::from_value(serde_json::json!({ "id": id, "channel_id": "c", "author": { "id": "u", "username": "u" }, "timestamp": "2026-01-01T00:00:00+00:00" })).unwrap()
    }

    /// 頼んだコマンドを覗ける App（チャンネル c に 100000.. のメッセージ n 件）
    fn harness(
        n: usize,
    ) -> (
        App,
        std::sync::mpsc::Sender<Event>,
        tokio::sync::mpsc::UnboundedReceiver<Cmd>,
    ) {
        let mut app = App::default();
        let (tx, ev) = std::sync::mpsc::channel();
        let (cmd, rx) = tokio::sync::mpsc::unbounded_channel();
        app.backend = Some(Backend { cmd, ev });
        app.messages.insert(
            "c".into(),
            (0..n).map(|i| msg(&(100000 + i).to_string())).collect(),
        );
        app.touch_channel("c");
        app.sel_channel = Some("c".into());
        app.dms.push(
            serde_json::from_value(
                serde_json::json!({ "id": "c", "type": 1, "last_message_id": "900000" }),
            )
            .unwrap(),
        );
        (app, tx, rx)
    }

    fn api(call: ApiCall, v: serde_json::Value) -> Event {
        Event::Api {
            id: 0,
            call,
            result: Ok(v),
        }
    }

    fn page(ids: std::ops::Range<u64>) -> serde_json::Value {
        // Discord は新しい順で返す
        serde_json::Value::Array(
            ids.rev()
                .map(|i| serde_json::to_value(msg(&i.to_string())).unwrap())
                .collect(),
        )
    }

    #[test]
    fn jump_inside_loaded_messages_only_scrolls() {
        let (mut app, _tx, mut rx) = harness(10);
        app.jump_to_message(None, "c".into(), "100003".into());
        assert_eq!(app.scroll_to_message, Some(("c".into(), "100003".into())));
        assert!(app.is_jump_flash("100003"));
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn jump_outside_loads_around_and_detaches_until_latest() {
        let (mut app, tx, mut rx) = harness(10);
        app.jump_to_message(None, "c".into(), "500025".into());
        let call = match rx.try_recv().unwrap() {
            Cmd::Api {
                call:
                    call @ ApiCall::GetMessages {
                        around: Some(_), ..
                    },
                ..
            } => call,
            _ => panic!("前後の読み込みを頼んでいない"),
        };
        tx.send(api(call, page(500000..500050))).unwrap();
        app.poll();
        assert!(app.is_detached("c"));
        assert_eq!(app.messages["c"].len(), 50);
        assert_eq!(app.scroll_to_message, Some(("c".into(), "500025".into())));
        // つながっていない間は新着を足さない
        tx.send(Event::MessageCreate(msg("900001"))).unwrap();
        app.poll();
        assert!(!app.messages["c"].iter().any(|m| m.id == "900001"));
        // 下へ読み進めて最新まで届いたら、つながった扱いに戻る
        app.load_newer("c");
        let call = match rx.try_recv().unwrap() {
            Cmd::Api {
                call: call @ ApiCall::GetMessages { after: Some(_), .. },
                ..
            } => call,
            _ => panic!("続きを頼んでいない"),
        };
        tx.send(api(call, page(500050..500060))).unwrap();
        app.poll();
        assert!(!app.is_detached("c"));
        assert_eq!(app.messages["c"].last().unwrap().id, "500059");
    }

    #[test]
    fn back_to_latest_reloads_and_reattaches() {
        let (mut app, tx, mut rx) = harness(10);
        app.history.insert(
            "c".into(),
            History {
                newer: true,
                ..Default::default()
            },
        );
        app.back_to_latest("c");
        assert!(matches!(rx.try_recv().unwrap(), Cmd::LoadMessages(c) if c == "c"));
        tx.send(Event::Messages("c".into(), vec![msg("900000")]))
            .unwrap();
        app.poll();
        assert!(!app.is_detached("c"));
        // つながっていない一覧は控えに書かない
        app.history.insert(
            "c".into(),
            History {
                newer: true,
                ..Default::default()
            },
        );
        assert!(app.is_detached("c"));
    }

    #[test]
    fn missing_jump_target_is_reported() {
        let (mut app, tx, mut rx) = harness(10);
        app.jump_to_message(None, "c".into(), "700000".into());
        let Cmd::Api { call, .. } = rx.try_recv().unwrap() else {
            panic!()
        };
        tx.send(api(call, page(600000..600010))).unwrap();
        app.poll();
        assert!(app.jump.is_none());
        assert!(app
            .toast
            .as_ref()
            .is_some_and(|(t, _)| t.contains("見つかりません")));
    }

    #[test]
    fn ids_sort_numerically_and_dedup() {
        let page = sorted_page(vec![msg("100"), msg("99"), msg("1000"), msg("100")]);
        let ids: Vec<&str> = page.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(ids, ["99", "100", "1000"]);
    }
}
