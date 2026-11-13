//! ピン留めとピン一覧。一覧は GET /channels/{id}/messages/pins（50 件ずつ、pinned_at でページ送り）
use super::*;
use crate::net::ApiCall;

/// 1ページの件数（Discord の上限）
pub const PIN_PAGE: u32 = 50;

pub struct PinItem {
    pub pinned_at: String,
    pub message: Message,
}

/// 右側に出すピン一覧
#[derive(Default)]
pub struct PinsView {
    pub channel_id: String,
    pub items: Vec<PinItem>,
    pub has_more: bool,
    /// 読み込み中のページ（None = 先頭）
    pub loading: Option<Option<String>>,
    /// 現在読み込み中の要求番号
    request: Option<u64>,
    /// 読み込み中に変わった。届いたら先頭から読み直す
    pub stale: bool,
    pub error: Option<String>,
    /// 失敗したページ（外側の None は失敗がない状態）
    failed_before: Option<Option<String>>,
    /// 中身が変わるたびに増やす（画面の一覧を作り直す目印）
    pub rev: u64,
}

/// 「GET /path: 403 Forbidden {…}」から HTTP の状態コードを取り出す
pub fn http_status(e: &str) -> Option<u16> {
    e.split(": ").nth(1)?.split(' ').next()?.parse().ok()
}

/// 失敗を利用者向けの短い言葉にする
pub fn api_error_text(e: &str, what: &str) -> String {
    match http_status(e) {
        Some(403) => tr!(
            "{what}: 権限がありません",
            "{what}: Missing permissions",
            "{what}：缺少权限",
        ),
        Some(404) => tr!(
            "{what}: 見つかりません",
            "{what}: Not found",
            "{what}：未找到",
        ),
        Some(429) => tr!(
            "{what}: 操作が多すぎます。しばらく待ってください",
            "{what}: Too many requests. Please wait a moment",
            "{what}：请求过多。请稍候",
        ),
        Some(s) if s >= 500 => tr!(
            "{what}: Discord 側の障害です",
            "{what}: Discord is having problems",
            "{what}：Discord 出现故障",
        ),
        _ => tr!(
            "{what}: 通信できませんでした",
            "{what}: Couldn't connect",
            "{what}：无法连接",
        ),
    }
}

/// ピン一覧の1ページを読む
fn parse_pins(v: serde_json::Value) -> Result<(Vec<PinItem>, bool), String> {
    let has_more = v["has_more"].as_bool().unwrap_or(false);
    let items =
        v["items"]
            .as_array()
            .ok_or(tr!("items がありません", "Missing items", "缺少 items"))?;
    let mut out = Vec::with_capacity(items.len());
    for it in items {
        let Some(at) = it["pinned_at"].as_str() else {
            continue;
        };
        // 1件だけ形が違っても一覧全体は捨てない
        if let Ok(m) = serde_json::from_value::<Message>(it["message"].clone()) {
            out.push(PinItem {
                pinned_at: at.to_string(),
                message: m,
            });
        }
    }
    if out.is_empty() && (!items.is_empty() || has_more) {
        return Err(tr!(
            "ピン一覧のページを読み取れません",
            "Can't read the page of pins",
            "无法读取置顶列表的页面"
        )
        .into());
    }
    Ok((out, has_more))
}

impl App {
    /// このチャンネルでの自分の権限。DM・まだ分からないときは None
    pub fn channel_perms(&self, guild_id: Option<&str>, channel_id: &str) -> Option<u64> {
        let gid = guild_id?;
        let g = self.guilds.iter().find(|g| g.id == gid)?;
        if g.owner {
            return Some(u64::MAX);
        }
        let me = self.me.as_ref()?.id.as_str();
        let my_roles = self.my_roles.get(gid)?;
        let base = match g.permissions.as_deref().and_then(|p| p.parse().ok()) {
            Some(b) => b,
            None => model::base_permissions(gid, self.roles.get(gid)?, my_roles),
        };
        let ch = self.find_guild_channel(gid, channel_id)?;
        // スレッドは親チャンネルの権限に従う
        let ch = match ch.kind {
            Channel::PUBLIC_THREAD | Channel::PRIVATE_THREAD | Channel::NEWS_THREAD => ch
                .parent_id
                .as_deref()
                .and_then(|p| self.find_guild_channel(gid, p))
                .unwrap_or(ch),
            _ => ch,
        };
        Some(model::channel_permissions(ch, gid, base, my_roles, me))
    }

    /// サーバーのチャンネルかスレッドを探す
    pub fn find_guild_channel(&self, guild_id: &str, channel_id: &str) -> Option<&Channel> {
        self.channels
            .get(guild_id)
            .into_iter()
            .flatten()
            .chain(self.threads.get(guild_id).into_iter().flatten())
            .find(|c| c.id == channel_id)
    }

    /// ピン留めできるか。権限が分からないときは出しておき、断られたら知らせる
    pub fn can_pin(&self, guild_id: Option<&str>, channel_id: &str) -> bool {
        // 古いロールは MANAGE_MESSAGES だけを持っていることがある
        self.channel_perms(guild_id, channel_id)
            .is_none_or(|p| p & (model::PIN_MESSAGES | (1 << 13)) != 0)
    }

    /// 開いているチャンネルのピン一覧を出す・閉じる
    pub fn toggle_pins(&mut self) {
        if self.side == SidePanel::Pins {
            self.close_side();
            return;
        }
        let Some(cid) = self.sel_channel.clone() else {
            return;
        };
        self.side = SidePanel::Pins;
        self.pins = Some(PinsView {
            channel_id: cid,
            ..Default::default()
        });
        self.load_pins(None);
    }

    pub fn close_side(&mut self) {
        self.side = SidePanel::None;
        self.pins = None;
        self.thread_panel = None;
    }

    /// ピン一覧の1ページを頼む（before = None なら先頭から読み直す）
    pub fn load_pins(&mut self, before: Option<String>) {
        let Some(p) = self.pins.as_mut() else { return };
        if p.loading.is_some() {
            if before.is_none() {
                p.stale = true;
            }
            return;
        }
        p.loading = Some(before.clone());
        p.error = None;
        p.failed_before = None;
        p.rev += 1;
        self.next_api_id += 1;
        let id = self.next_api_id;
        p.request = Some(id);
        let call = ApiCall::GetChannelPins {
            channel_id: p.channel_id.clone(),
            before,
            limit: PIN_PAGE,
        };
        let queued = self
            .backend
            .as_ref()
            .is_some_and(|b| b.cmd.send(Cmd::Api { id, call }).is_ok());
        if !queued {
            if let Some(p) = self.pins.as_mut() {
                p.failed_before = p.loading.take();
                p.request = None;
                p.error = Some(tr!("ピン留めされたメッセージを読み込めませんでした。接続を確認して再試行してください", "Couldn't load pinned messages. Check your connection and try again", "无法加载置顶消息。请检查连接后重试").into());
                p.rev += 1;
            }
        }
    }

    /// 次のページ
    pub fn load_more_pins(&mut self) {
        let Some(p) = self.pins.as_ref() else { return };
        if !p.has_more || p.loading.is_some() {
            return;
        }
        let before = p.items.last().map(|i| i.pinned_at.clone());
        if before.is_some() {
            self.load_pins(before);
        }
    }

    /// 一覧の読み込みをやり直す。
    pub fn retry_pins(&mut self) {
        let Some(p) = self
            .pins
            .as_ref()
            .filter(|p| p.error.is_some() && p.loading.is_none())
        else {
            return;
        };
        let Some(before) = p.failed_before.clone() else {
            return;
        };
        self.load_pins(before);
    }

    pub(super) fn on_pins_loaded(
        &mut self,
        id: u64,
        channel_id: String,
        before: Option<String>,
        result: Result<serde_json::Value, String>,
    ) {
        let Some(p) = self.pins.as_mut().filter(|p| {
            p.request == Some(id)
                && p.channel_id == channel_id
                && p.loading.as_ref() == Some(&before)
        }) else {
            return;
        };
        p.request = None;
        p.loading = None;
        p.rev += 1;
        let result = match result {
            Ok(v) => parse_pins(v).map_err(|e| {
                eprintln!("ピン一覧を読めませんでした: {e}");
                tr!(
                    "ピン留めされたメッセージを読み取れませんでした。再試行してください",
                    "Couldn't read pinned messages. Please try again",
                    "无法读取置顶消息。请重试"
                )
                .to_string()
            }),
            Err(e) => {
                eprintln!("ピン一覧を読めませんでした: {e}");
                Err(api_error_text(
                    &e,
                    tr!(
                        "ピン留めされたメッセージを読み込めませんでした",
                        "Couldn't load pinned messages",
                        "无法加载置顶消息"
                    ),
                ))
            }
        };
        match result {
            Ok((items, has_more)) => {
                if before.is_none() {
                    p.items.clear();
                }
                // ページの境目で同じピンが重ならないようにする
                let have: HashSet<String> = p.items.iter().map(|i| i.message.id.clone()).collect();
                p.items
                    .extend(items.into_iter().filter(|i| !have.contains(&i.message.id)));
                p.has_more = has_more;
            }
            Err(e) => {
                p.error = Some(e);
                p.failed_before = Some(before);
            }
        }
        if std::mem::take(&mut p.stale) {
            self.load_pins(None);
        }
    }

    /// ピン留めする・外す
    pub fn set_pinned(&mut self, m: &Message, pin: bool) {
        let (channel_id, message_id) = (m.channel_id.clone(), m.id.clone());
        let call = if pin {
            ApiCall::Pin {
                channel_id,
                message_id,
            }
        } else {
            ApiCall::Unpin {
                channel_id,
                message_id,
            }
        };
        let queued = self
            .backend
            .as_ref()
            .is_some_and(|b| b.cmd.send(Cmd::Api { id: 0, call }).is_ok());
        if !queued {
            self.show_toast(if pin {
                tr!(
                    "ピン留めできませんでした。接続を確認して再試行してください",
                    "Couldn't pin. Check your connection and try again",
                    "无法置顶。请检查连接后重试"
                )
            } else {
                tr!(
                    "ピン留めを外せませんでした。接続を確認して再試行してください",
                    "Couldn't unpin. Check your connection and try again",
                    "无法取消置顶。请检查连接后重试"
                )
            });
        }
    }

    pub(super) fn on_pin_result(
        &mut self,
        channel_id: String,
        message_id: String,
        pin: bool,
        result: Result<serde_json::Value, String>,
    ) {
        match result {
            Ok(_) => {
                self.on_pin_changed(&channel_id, &message_id, pin);
                self.show_toast(if pin {
                    tr!(
                        "メッセージをピン留めしました",
                        "Message pinned",
                        "已置顶消息"
                    )
                } else {
                    tr!("ピン留めを外しました", "Unpinned", "已取消置顶")
                });
            }
            Err(e) => {
                eprintln!("ピン留めの変更に失敗しました: {e}");
                let what = if pin {
                    tr!("ピン留めできませんでした", "Couldn't pin", "无法置顶")
                } else {
                    tr!(
                        "ピン留めを外せませんでした",
                        "Couldn't unpin",
                        "无法取消置顶"
                    )
                };
                let text = match (http_status(&e), e.contains("30003")) {
                    // 1チャンネル 250 件まで
                    (_, true) => tr!(
                        "{what}: ピン留めの上限に達しています",
                        "{what}: The pin limit has been reached",
                        "{what}：已达到置顶上限",
                    ),
                    _ => api_error_text(&e, what),
                };
                self.show_toast(&text);
            }
        }
    }

    /// ピン留めの状態が変わった（自分の操作・MESSAGE_UPDATE）
    pub fn on_pin_changed(&mut self, channel_id: &str, message_id: &str, pinned: bool) {
        if let Some(m) = self
            .messages
            .get_mut(channel_id)
            .and_then(|l| l.iter_mut().find(|m| m.id == message_id))
        {
            if m.pinned != pinned {
                m.pinned = pinned;
                self.bump_msgs();
            }
        }
        let Some(p) = self.pins.as_mut().filter(|p| p.channel_id == channel_id) else {
            return;
        };
        if pinned {
            if !p.items.iter().any(|i| i.message.id == message_id) {
                self.load_pins(None);
            }
        } else if let Some(i) = p.items.iter().position(|i| i.message.id == message_id) {
            p.items.remove(i);
            p.rev += 1;
        }
    }

    /// CHANNEL_PINS_UPDATE: 一覧を開いていれば読み直す
    pub(super) fn on_channel_pins_update(&mut self, d: &serde_json::Value) {
        let Some(cid) = d["channel_id"].as_str() else {
            return;
        };
        if self.pins.as_ref().is_some_and(|p| p.channel_id == cid) {
            self.load_pins(None);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn msg(id: &str, pinned: bool) -> serde_json::Value {
        serde_json::json!({ "id": id, "channel_id": "c", "author": { "id": "u", "username": "u" }, "timestamp": "2026-01-01T00:00:00+00:00", "pinned": pinned })
    }

    fn harness() -> (
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
            vec![serde_json::from_value(msg("1", false)).unwrap()],
        );
        app.touch_channel("c");
        app.sel_channel = Some("c".into());
        (app, tx, rx)
    }

    fn next_pins_call(rx: &mut tokio::sync::mpsc::UnboundedReceiver<Cmd>) -> (u64, ApiCall) {
        match rx.try_recv().unwrap() {
            Cmd::Api {
                id,
                call: call @ ApiCall::GetChannelPins { .. },
            } => (id, call),
            _ => panic!("ピン一覧を頼んでいない"),
        }
    }

    #[test]
    fn pins_without_a_backend_can_be_retried_after_reconnection() {
        let (mut app, _tx, mut rx) = harness();
        let backend = app.backend.take();
        app.toggle_pins();
        let p = app.pins.as_ref().unwrap();
        assert!(p.loading.is_none() && p.error.is_some());
        app.backend = backend;
        app.retry_pins();
        assert!(matches!(
            next_pins_call(&mut rx).1,
            ApiCall::GetChannelPins { before: None, .. }
        ));
        assert!(app.pins.as_ref().unwrap().loading.is_some());
    }

    #[test]
    fn pins_with_a_closed_backend_do_not_load_forever() {
        let (mut app, _tx, rx) = harness();
        drop(rx);
        app.toggle_pins();
        let p = app.pins.as_ref().unwrap();
        assert!(p.loading.is_none() && p.error.is_some());
    }

    #[test]
    fn closing_and_reopening_pins_discards_the_old_response() {
        let (mut app, tx, mut rx) = harness();
        app.toggle_pins();
        let (old_id, old) = next_pins_call(&mut rx);
        app.toggle_pins();
        app.toggle_pins();
        let (current_id, current) = next_pins_call(&mut rx);
        assert_ne!(old_id, current_id);
        tx.send(Event::Api { id: old_id, call: old, result: Ok(serde_json::json!({"has_more":false,"items":[{"pinned_at":"2026-01-01T00:00:00+00:00","message":msg("old",true)}]})) }).unwrap();
        app.poll();
        let p = app.pins.as_ref().unwrap();
        assert!(
            p.loading.is_some() && p.items.is_empty(),
            "閉じる前の応答を新しい一覧に反映しない"
        );
        tx.send(Event::Api { id: current_id, call: current, result: Ok(serde_json::json!({"has_more":false,"items":[{"pinned_at":"2026-01-02T00:00:00+00:00","message":msg("new",true)}]})) }).unwrap();
        app.poll();
        assert_eq!(app.pins.as_ref().unwrap().items[0].message.id, "new");
    }

    #[test]
    fn late_failure_from_closed_pins_does_not_replace_the_new_result() {
        let (mut app, tx, mut rx) = harness();
        app.toggle_pins();
        let (old_id, old) = next_pins_call(&mut rx);
        app.toggle_pins();
        app.toggle_pins();
        let (current_id, current) = next_pins_call(&mut rx);
        tx.send(Event::Api { id: current_id, call: current, result: Ok(serde_json::json!({"has_more":false,"items":[{"pinned_at":"2026-01-02T00:00:00+00:00","message":msg("new",true)}]})) }).unwrap();
        app.poll();
        tx.send(Event::Api {
            id: old_id,
            call: old,
            result: Err("network unavailable".into()),
        })
        .unwrap();
        app.poll();
        let p = app.pins.as_ref().unwrap();
        assert!(p.error.is_none() && p.loading.is_none());
        assert_eq!(p.items[0].message.id, "new");
    }

    #[test]
    fn retry_after_failed_refresh_reloads_the_first_page() {
        let (mut app, tx, mut rx) = harness();
        app.toggle_pins();
        let (id, call) = next_pins_call(&mut rx);
        tx.send(Event::Api { id, call, result: Ok(serde_json::json!({"has_more":true,"items":[{"pinned_at":"2026-01-01T00:00:00+00:00","message":msg("1",true)}]})) }).unwrap();
        app.poll();
        app.load_pins(None);
        let (id, call) = next_pins_call(&mut rx);
        tx.send(Event::Api {
            id,
            call,
            result: Err("network unavailable".into()),
        })
        .unwrap();
        app.poll();
        assert!(app.pins.as_ref().unwrap().error.is_some());
        app.retry_pins();
        let (id, call) = next_pins_call(&mut rx);
        assert!(
            matches!(&call, ApiCall::GetChannelPins { before: None, .. }),
            "更新失敗後は古い続きではなく先頭を再取得する"
        );
        app.retry_pins();
        assert!(rx.try_recv().is_err(), "再取得中の二重送信を防ぐ");
        tx.send(Event::Api { id, call, result: Ok(serde_json::json!({"has_more":false,"items":[{"pinned_at":"2026-01-02T00:00:00+00:00","message":msg("2",true)}]})) }).unwrap();
        app.poll();
        let p = app.pins.as_ref().unwrap();
        assert!(p.error.is_none() && p.items.len() == 1 && p.items[0].message.id == "2");
    }

    #[test]
    fn retry_older_pins_keeps_the_original_cursor_after_an_item_is_removed() {
        let (mut app, tx, mut rx) = harness();
        app.toggle_pins();
        let (id, call) = next_pins_call(&mut rx);
        tx.send(Event::Api {
            id,
            call,
            result: Ok(serde_json::json!({"has_more":true,"items":[
                {"pinned_at":"2026-01-02T00:00:00+00:00","message":msg("1",true)},
                {"pinned_at":"2026-01-01T00:00:00+00:00","message":msg("2",true)}
            ]})),
        })
        .unwrap();
        app.poll();
        app.load_more_pins();
        let (id, call) = next_pins_call(&mut rx);
        tx.send(Event::Api {
            id,
            call,
            result: Err("network unavailable".into()),
        })
        .unwrap();
        app.poll();
        app.on_pin_changed("c", "2", false);
        app.retry_pins();
        assert!(
            matches!(next_pins_call(&mut rx).1, ApiCall::GetChannelPins { before: Some(at), .. } if at == "2026-01-01T00:00:00+00:00")
        );
    }

    #[test]
    fn pins_paginate_by_pinned_at_without_duplicates() {
        let (mut app, tx, mut rx) = harness();
        app.toggle_pins();
        assert_eq!(app.side, SidePanel::Pins);
        let (id, call) = next_pins_call(&mut rx);
        assert!(matches!(
            &call,
            ApiCall::GetChannelPins {
                before: None,
                limit: 50,
                ..
            }
        ));
        let items = |ids: &[(&str, &str)]| {
            serde_json::Value::Array(
                ids.iter()
                    .map(
                        |(id, at)| serde_json::json!({ "pinned_at": at, "message": msg(id, true) }),
                    )
                    .collect(),
            )
        };
        tx.send(Event::Api { id, call, result: Ok(serde_json::json!({ "has_more": true, "items": items(&[("5", "2026-01-05T00:00:00+00:00"), ("4", "2026-01-04T00:00:00+00:00")]) })) }).unwrap();
        app.poll();
        app.load_more_pins();
        let (id, call) = next_pins_call(&mut rx);
        assert!(
            matches!(&call, ApiCall::GetChannelPins { before: Some(b), .. } if b == "2026-01-04T00:00:00+00:00")
        );
        // 境目の 4 がもう一度来ても重ねない
        tx.send(Event::Api { id, call, result: Ok(serde_json::json!({ "has_more": false, "items": items(&[("4", "2026-01-04T00:00:00+00:00"), ("3", "2026-01-03T00:00:00+00:00")]) })) }).unwrap();
        app.poll();
        let ids: Vec<&str> = app
            .pins
            .as_ref()
            .unwrap()
            .items
            .iter()
            .map(|i| i.message.id.as_str())
            .collect();
        assert_eq!(ids, ["5", "4", "3"]);
        // もうページが無ければ頼まない
        app.load_more_pins();
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn pin_changes_sync_messages_and_open_list() {
        let (mut app, tx, mut rx) = harness();
        app.toggle_pins();
        let (id, call) = next_pins_call(&mut rx);
        tx.send(Event::Api { id, call, result: Ok(serde_json::json!({ "has_more": false, "items": [{ "pinned_at": "2026-01-01T00:00:00+00:00", "message": msg("1", true) }] })) }).unwrap();
        app.poll();
        // 別の端末で外された（MESSAGE_UPDATE）
        tx.send(Event::MessageUpdate {
            channel_id: "c".into(),
            id: "1".into(),
            content: None,
            embeds: None,
            pinned: Some(false),
        })
        .unwrap();
        app.poll();
        assert!(app.pins.as_ref().unwrap().items.is_empty());
        assert!(!app.messages["c"][0].pinned);
        // 自分でピン留めした: 一覧は読み直し、メッセージには印を付ける
        tx.send(Event::Api {
            id: 0,
            call: ApiCall::Pin {
                channel_id: "c".into(),
                message_id: "1".into(),
            },
            result: Ok(serde_json::Value::Null),
        })
        .unwrap();
        app.poll();
        assert!(app.messages["c"][0].pinned);
        assert!(matches!(
            next_pins_call(&mut rx).1,
            ApiCall::GetChannelPins { before: None, .. }
        ));
        // 読み込み中に CHANNEL_PINS_UPDATE が来たら、届いてからもう一度読み直す
        tx.send(Event::Gateway {
            t: "CHANNEL_PINS_UPDATE".into(),
            d: serde_json::json!({ "channel_id": "c" }),
        })
        .unwrap();
        app.poll();
        assert!(rx.try_recv().is_err());
        assert!(app.pins.as_ref().unwrap().stale);
    }

    #[test]
    fn pin_failure_explains_permission() {
        let (mut app, tx, _rx) = harness();
        tx.send(Event::Api {
            id: 0,
            call: ApiCall::Pin {
                channel_id: "c".into(),
                message_id: "1".into(),
            },
            result: Err("PUT /channels/c/messages/pins/1: 403 Forbidden {}".into()),
        })
        .unwrap();
        app.poll();
        assert!(app.toast.as_ref().is_some_and(|(t, _)| t.contains("権限")));
        assert!(!app.messages["c"][0].pinned);
    }

    #[test]
    fn pin_action_without_a_backend_reports_failure_and_keeps_the_message() {
        let (mut app, _tx, _rx) = harness();
        app.backend = None;
        let message: Message = serde_json::from_value(msg("1", false)).unwrap();
        app.set_pinned(&message, true);
        assert!(app
            .toast
            .as_ref()
            .is_some_and(|(t, _)| t.contains("ピン留めできませんでした")));
        assert!(!app.messages["c"][0].pinned);
    }

    #[test]
    fn unpin_action_with_a_closed_backend_reports_failure_and_keeps_the_pin() {
        let (mut app, _tx, rx) = harness();
        drop(rx);
        let message: Message = serde_json::from_value(msg("1", true)).unwrap();
        app.messages.get_mut("c").unwrap()[0].pinned = true;
        app.set_pinned(&message, false);
        assert!(app
            .toast
            .as_ref()
            .is_some_and(|(t, _)| t.contains("ピン留めを外せませんでした")));
        assert!(app.messages["c"][0].pinned);
    }

    #[test]
    fn status_is_read_from_error_text() {
        assert_eq!(
            http_status("GET /channels/1/messages/pins: 403 Forbidden {\"code\": 50013}"),
            Some(403)
        );
        assert_eq!(http_status("error sending request"), None);
        assert!(
            api_error_text("PUT /x: 403 Forbidden", "ピン留めできませんでした").contains("権限")
        );
    }

    #[test]
    fn pins_page_skips_broken_items() {
        let v = serde_json::json!({
            "has_more": true,
            "items": [
                { "pinned_at": "2026-01-02T00:00:00+00:00", "message": { "id": "2", "channel_id": "c", "author": { "id": "u", "username": "u" }, "timestamp": "2026-01-01T00:00:00+00:00", "pinned": true } },
                { "pinned_at": "2026-01-01T00:00:00+00:00", "message": { "id": 3 } },
            ],
        });
        let (items, more) = parse_pins(v).unwrap();
        assert!(more);
        assert_eq!(items.len(), 1);
        assert!(items[0].message.pinned);
        assert!(parse_pins(serde_json::json!({})).is_err());
    }

    #[test]
    fn pins_page_with_no_usable_cursor_is_an_error_instead_of_an_empty_list() {
        let bad = serde_json::json!({"has_more":true,"items":[{"pinned_at":"2026-01-01T00:00:00+00:00","message":{"id":"broken"}}]});
        assert!(parse_pins(bad).is_err());
        assert!(parse_pins(serde_json::json!({"has_more":true,"items":[]})).is_err());
        let (items, has_more) =
            parse_pins(serde_json::json!({"has_more":false,"items":[]})).unwrap();
        assert!(items.is_empty() && !has_more);
    }

    #[test]
    fn malformed_pin_page_explains_the_read_failure_and_can_be_retried() {
        let (mut app, tx, mut rx) = harness();
        app.toggle_pins();
        let (id, call) = next_pins_call(&mut rx);
        tx.send(Event::Api {
            id,
            call,
            result: Ok(serde_json::json!({"has_more":true,"items":[]})),
        })
        .unwrap();
        app.poll();
        let p = app.pins.as_ref().unwrap();
        assert!(
            p.loading.is_none()
                && p.error
                    .as_ref()
                    .is_some_and(|e| e.contains("読み取れません"))
        );
        app.retry_pins();
        assert!(matches!(
            next_pins_call(&mut rx).1,
            ApiCall::GetChannelPins { before: None, .. }
        ));
    }
}
