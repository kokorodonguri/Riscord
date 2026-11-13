//! ボイスチャンネルの画面まわりの状態（招待・アクティビティの案内・接続の失敗）
use super::*;

/// 招待を作る権限
pub const CREATE_INSTANT_INVITE: u64 = 1 << 0;
/// 招待リンクの有効期限（秒）。サーバーの招待（create_guild_invite）と同じ 7 日
const INVITE_MAX_AGE: u64 = 604_800;

/// 「ボイスに招待」の窓
#[derive(Default)]
pub struct VoiceInvite {
    pub guild_id: String,
    pub channel_id: String,
    /// 招待コードを作っている途中
    pub loading: bool,
    pub code: Option<String>,
    pub error: Option<String>,
    /// 送っている途中・送った・送れなかった DM チャンネル
    pub sending: HashSet<String>,
    pub sent: HashSet<String>,
    pub failed: HashSet<String>,
}

/// 結果を待っているリクエストが何のためのものか。窓を閉じた後に届いた結果も取り違えない
enum Pending {
    Invite(String),
    Send(String),
}

#[derive(Default)]
pub struct VoiceView {
    pub invite: Option<VoiceInvite>,
    /// 「アクティビティを選ぶ」の案内を出しているか
    pub activity_open: bool,
    /// 最後にボイスへつなげなかったチャンネルと理由（再試行・別のチャンネルへの接続で消す）
    pub error: Option<(Option<String>, String, String)>,
    /// 作った招待コード（チャンネル → コード）。開き直すたびに新しく作らない
    codes: HashMap<String, String>,
    pending: HashMap<u64, Pending>,
}

pub fn invite_link(code: &str) -> String {
    format!("https://discord.gg/{code}")
}

impl App {
    /// メインのゲートウェイにつながっているか（再接続中・ログイン前は false）
    pub fn gateway_ok(&self) -> bool {
        self.gateway_status == "接続済み"
    }

    /// このボイスチャンネルの招待を作れるか。権限がまだ分からないときは作ってみて、断られたら知らせる
    pub fn can_invite(&self, guild_id: &str, channel_id: &str) -> bool {
        self.channel_perms(Some(guild_id), channel_id)
            .is_none_or(|p| p & CREATE_INSTANT_INVITE != 0)
    }

    /// 「ボイスに招待」を開く。同じチャンネルで作ったコードがあれば使い回す
    pub fn open_voice_invite(&mut self, guild_id: &str, channel_id: &str) {
        self.voice_view.activity_open = false;
        let mut inv = VoiceInvite {
            guild_id: guild_id.to_string(),
            channel_id: channel_id.to_string(),
            ..Default::default()
        };
        if let Some(code) = self.voice_view.codes.get(channel_id) {
            inv.code = Some(code.clone());
        } else if !self.can_invite(guild_id, channel_id) {
            inv.error = Some(
                tr!(
                    "このチャンネルへの招待を作る権限がありません",
                    "You don't have permission to create invites for this channel"
                )
                .into(),
            );
        } else if self
            .voice_view
            .pending
            .values()
            .any(|p| matches!(p, Pending::Invite(c) if c == channel_id))
        {
            // 閉じる前に頼んだものがまだ返ってきていない。重ねて作らない
            inv.loading = true;
        } else {
            let id = self.api(net::ApiCall::CreateInvite {
                channel_id: channel_id.to_string(),
                body: serde_json::json!({
                    "max_age": INVITE_MAX_AGE,
                    "max_uses": 0,
                    "temporary": false,
                }),
            });
            self.voice_view
                .pending
                .insert(id, Pending::Invite(channel_id.to_string()));
            inv.loading = true;
        }
        self.voice_view.invite = Some(inv);
    }

    pub fn close_voice_invite(&mut self) {
        self.voice_view.invite = None;
    }

    /// 招待リンクをクリップボードへ
    pub fn copy_voice_invite(&mut self) {
        let Some(code) = self.voice_view.invite.as_ref().and_then(|i| i.code.clone()) else {
            return;
        };
        self.clipboard_out = Some(invite_link(&code));
        self.show_toast(tr!("招待リンクをコピーしました", "Invite link copied"));
    }

    /// 招待リンクを DM（グループ DM も）に送る
    pub fn send_voice_invite(&mut self, dm_channel_id: &str) {
        let Some(inv) = self.voice_view.invite.as_ref() else {
            return;
        };
        let Some(code) = inv.code.clone() else {
            return;
        };
        if inv.sending.contains(dm_channel_id) || inv.sent.contains(dm_channel_id) {
            return;
        }
        let id = self.api(net::ApiCall::SendMessage {
            channel_id: dm_channel_id.to_string(),
            body: serde_json::json!({ "content": invite_link(&code) }),
        });
        self.voice_view
            .pending
            .insert(id, Pending::Send(dm_channel_id.to_string()));
        if let Some(inv) = self.voice_view.invite.as_mut() {
            inv.failed.remove(dm_channel_id);
            inv.sending.insert(dm_channel_id.to_string());
        }
    }

    pub fn is_voice_view_request(&self, id: u64) -> bool {
        self.voice_view.pending.contains_key(&id)
    }

    pub fn voice_view_result(&mut self, id: u64, result: Result<serde_json::Value, String>) {
        let Some(p) = self.voice_view.pending.remove(&id) else {
            return;
        };
        match p {
            Pending::Invite(channel_id) => {
                let code = result
                    .as_ref()
                    .ok()
                    .and_then(|v| v["code"].as_str())
                    .filter(|c| !c.is_empty() && c.len() <= 32)
                    .map(str::to_owned);
                if let Some(c) = &code {
                    self.voice_view.codes.insert(channel_id.clone(), c.clone());
                }
                // 今開いている窓が同じチャンネルのときだけ反映する
                let Some(inv) = self
                    .voice_view
                    .invite
                    .as_mut()
                    .filter(|i| i.channel_id == channel_id)
                else {
                    return;
                };
                inv.loading = false;
                match (code, result) {
                    (Some(c), _) => inv.code = Some(c),
                    (None, Err(e)) => {
                        eprintln!("ボイスの招待を作れませんでした: {e}");
                        inv.error = Some(api_error_text(
                            &e,
                            tr!(
                                "招待リンクを作れませんでした",
                                "Couldn't create an invite link"
                            ),
                        ));
                    }
                    (None, Ok(_)) => {
                        inv.error = Some(
                            tr!(
                                "招待リンクを作れませんでした",
                                "Couldn't create an invite link"
                            )
                            .into(),
                        );
                    }
                }
            }
            Pending::Send(dm) => {
                let ok = result.is_ok();
                if let Err(e) = &result {
                    eprintln!("招待を送れませんでした: {e}");
                }
                match self.voice_view.invite.as_mut() {
                    Some(inv) => {
                        inv.sending.remove(&dm);
                        if ok {
                            inv.sent.insert(dm);
                        } else {
                            inv.failed.insert(dm);
                        }
                    }
                    None if !ok => {
                        self.show_toast(tr!("招待を送れませんでした", "Couldn't send the invite"))
                    }
                    None => {}
                }
            }
        }
    }

    /// 「アクティビティを選ぶ」の案内を開く・閉じる
    pub fn toggle_voice_activity(&mut self) {
        self.voice_view.activity_open = !self.voice_view.activity_open;
        if self.voice_view.activity_open {
            self.voice_view.invite = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app_with_backend() -> (App, tokio::sync::mpsc::UnboundedReceiver<Cmd>) {
        let mut app = App::default();
        let (_tx, ev) = std::sync::mpsc::channel();
        let (cmd, rx) = tokio::sync::mpsc::unbounded_channel();
        app.backend = Some(Backend { cmd, ev });
        (app, rx)
    }

    fn api_ids(rx: &mut tokio::sync::mpsc::UnboundedReceiver<Cmd>) -> Vec<u64> {
        let mut v = Vec::new();
        while let Ok(c) = rx.try_recv() {
            if let Cmd::Api { id, .. } = c {
                v.push(id);
            }
        }
        v
    }

    #[test]
    fn invite_code_is_reused_and_late_result_for_other_channel_is_ignored() {
        let (mut app, mut rx) = app_with_backend();
        app.open_voice_invite("g", "v1");
        let first = api_ids(&mut rx);
        assert_eq!(first.len(), 1);
        // 結果が届く前に別のチャンネルの窓に切り替える
        app.open_voice_invite("g", "v2");
        let second = api_ids(&mut rx);
        assert_eq!(second.len(), 1);
        app.voice_view_result(first[0], Ok(serde_json::json!({ "code": "abc" })));
        let inv = app.voice_view.invite.as_ref().unwrap();
        assert_eq!(inv.channel_id, "v2");
        assert!(inv.code.is_none() && inv.loading);
        // 古い結果はクリップボードに入れない
        assert!(app.clipboard_out.is_none());
        // v1 を開き直すと、作ったコードを使い回して新しく作らない
        app.open_voice_invite("g", "v1");
        assert!(api_ids(&mut rx).is_empty());
        assert_eq!(
            app.voice_view.invite.as_ref().unwrap().code.as_deref(),
            Some("abc")
        );
    }

    #[test]
    fn invite_failure_shows_permission_error_and_no_duplicate_request() {
        let (mut app, mut rx) = app_with_backend();
        app.open_voice_invite("g", "v");
        let ids = api_ids(&mut rx);
        app.close_voice_invite();
        // 結果待ちのまま開き直しても、重ねて頼まない
        app.open_voice_invite("g", "v");
        assert!(api_ids(&mut rx).is_empty());
        app.voice_view_result(
            ids[0],
            Err("POST /channels/v/invites: 403 Forbidden {\"code\": 50013}".into()),
        );
        let inv = app.voice_view.invite.as_ref().unwrap();
        assert!(!inv.loading && inv.code.is_none());
        assert!(inv.error.as_deref().unwrap().contains("権限"));
    }

    #[test]
    fn sending_invite_tracks_each_dm_result() {
        let (mut app, mut rx) = app_with_backend();
        app.open_voice_invite("g", "v");
        let id = api_ids(&mut rx)[0];
        app.voice_view_result(id, Ok(serde_json::json!({ "code": "xyz" })));
        app.send_voice_invite("dm1");
        app.send_voice_invite("dm1");
        app.send_voice_invite("dm2");
        let sends = api_ids(&mut rx);
        assert_eq!(sends.len(), 2, "同じ DM に二重に送らない");
        app.voice_view_result(sends[0], Ok(serde_json::json!({})));
        app.voice_view_result(sends[1], Err("x: 500 Internal".into()));
        let inv = app.voice_view.invite.as_ref().unwrap();
        assert!(inv.sent.contains("dm1") && inv.failed.contains("dm2"));
        assert!(inv.sending.is_empty());
    }

    #[test]
    fn voice_error_is_kept_for_its_channel_and_cleared_on_join() {
        let mut app = App::default();
        let (tx, ev) = std::sync::mpsc::channel();
        let (cmd, _rx) = tokio::sync::mpsc::unbounded_channel();
        app.backend = Some(Backend { cmd, ev });
        app.join_voice(Some("g".into()), "v".into(), false);
        tx.send(Event::Voice(VoiceStatus::Error("udp".into())))
            .unwrap();
        app.poll();
        assert!(app.voice_channel().is_none());
        let (g, c, _) = app.voice_view.error.clone().unwrap();
        assert_eq!((g.as_deref(), c.as_str()), (Some("g"), "v"));
        app.join_voice(Some("g".into()), "v".into(), false);
        assert!(app.voice_view.error.is_none());
        // つながるまでは「接続済み」にならない
        assert_eq!(app.voice_channel().map(|(_, _, k)| k), Some(false));
    }

    #[test]
    fn opening_voice_channel_does_not_join_or_load_messages() {
        let (mut app, mut rx) = app_with_backend();
        let ch: Channel =
            serde_json::from_value(serde_json::json!({ "id": "v", "type": 2, "name": "VC" }))
                .unwrap();
        app.channels.insert("g".into(), vec![ch]);
        app.sel_guild = Some("g".into());
        app.open_channel("v".into());
        assert_eq!(app.sel_channel.as_deref(), Some("v"));
        assert!(app.voice_channel().is_none(), "開いただけでは参加しない");
        while let Ok(c) = rx.try_recv() {
            assert!(
                !matches!(c, Cmd::LoadMessages(_) | Cmd::JoinVoice { .. }),
                "ボイスチャンネルを開いてもメッセージの読み込み・参加はしない"
            );
        }
    }
}
