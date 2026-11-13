//! サウンドボード。鳴らすと Discord が同じボイスチャンネルの全員に知らせ、各端末がそれぞれ音を鳴らす。
use super::*;

const SPEAK: u64 = 1 << 21;
const USE_SOUNDBOARD: u64 = 1 << 42;
/// 自分が鳴らした音の知らせが届いたとき、二重に鳴らさないための猶予
const OWN_ECHO: std::time::Duration = std::time::Duration::from_secs(5);

#[derive(Clone, Debug, PartialEq)]
pub struct Sound {
    pub id: String,
    pub name: String,
    pub emoji: Option<String>,
    /// サーバーの音ならそのサーバー（標準の音は None）
    pub guild_id: Option<String>,
    pub volume: f32,
}

#[derive(Default)]
pub struct Soundboard {
    pub defaults: Option<Vec<Sound>>,
    pub guild: Option<Vec<Sound>>,
    pub default_request: Option<u64>,
    pub guild_request: Option<u64>,
    /// 送信中（リクエスト ID と音）
    pub sending: Option<(u64, Sound)>,
    pub error: Option<String>,
}

fn parse_sound(v: &serde_json::Value) -> Option<Sound> {
    let id = v.get("sound_id")?.as_str()?;
    if id.is_empty() || id.len() > 20 || !id.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    // 使えなくなった音（ブーストが切れたなど）は出さない
    if v.get("available").and_then(|a| a.as_bool()) == Some(false) {
        return None;
    }
    Some(Sound {
        id: id.to_owned(),
        name: v.get("name")?.as_str()?.to_owned(),
        emoji: v
            .get("emoji_name")
            .and_then(|e| e.as_str())
            .filter(|e| !e.is_empty())
            .map(str::to_owned),
        guild_id: v
            .get("guild_id")
            .and_then(|g| g.as_str())
            .map(str::to_owned),
        volume: v.get("volume").and_then(|n| n.as_f64()).unwrap_or(1.0) as f32,
    })
}

/// 標準の音は配列、サーバーの音は { items: [...] }
fn parse_sounds(v: &serde_json::Value) -> Option<Vec<Sound>> {
    let items = v
        .as_array()
        .or_else(|| v.get("items").and_then(|i| i.as_array()))?;
    Some(items.iter().filter_map(parse_sound).collect())
}

impl App {
    fn soundboard_voice(&self) -> Option<(Option<String>, String)> {
        match self.voice_status.as_ref()? {
            VoiceStatus::Connected {
                guild_id,
                channel_id,
                ..
            } => Some((guild_id.clone(), channel_id.clone())),
            _ => None,
        }
    }

    /// サウンドボードを使えない理由（使えるなら None）
    pub fn soundboard_block_reason(&self) -> Option<&'static str> {
        let Some((guild_id, channel_id)) = self.soundboard_voice() else {
            return Some(tr!(
                "ボイスチャンネルに接続していません",
                "Not connected to a voice channel",
                "未连接语音频道"
            ));
        };
        let Some(guild_id) = guild_id else {
            return Some(tr!(
                "サウンドボードはサーバーのボイスチャンネルで使えます",
                "The soundboard works in server voice channels",
                "音效板可在服务器的语音频道中使用"
            ));
        };
        if self.settings.self_mute || self.settings.self_deaf {
            return Some(tr!(
                "ミュート中はサウンドボードを使えません",
                "You can't use the soundboard while muted",
                "静音时无法使用音效板"
            ));
        }
        if self
            .channel_perms(Some(&guild_id), &channel_id)
            .is_some_and(|p| p & (SPEAK | USE_SOUNDBOARD) != (SPEAK | USE_SOUNDBOARD))
        {
            return Some(tr!(
                "このチャンネルでサウンドボードを使う権限がありません",
                "You don't have permission to use the soundboard in this channel",
                "你没有在此频道使用音效板的权限"
            ));
        }
        None
    }

    /// サウンドボードを開く・閉じる
    pub fn toggle_soundboard(&mut self) {
        if self.soundboard.take().is_some() {
            return;
        }
        let Some((guild_id, _)) = self.soundboard_voice() else {
            return;
        };
        let mut sb = Soundboard {
            default_request: self.role_queue(net::ApiCall::GetDefaultSounds),
            ..Default::default()
        };
        if let Some(g) = guild_id {
            sb.guild_request = self.role_queue(net::ApiCall::GetGuildSounds(g));
        } else {
            sb.guild = Some(Vec::new());
        }
        if sb.default_request.is_none() {
            sb.error = Some(
                tr!(
                    "読み込めませんでした。再接続してください",
                    "Couldn't load. Please reconnect",
                    "无法加载。请重新连接"
                )
                .into(),
            );
        }
        self.soundboard = Some(sb);
    }

    pub fn send_sound(&mut self, sound_id: &str) {
        let Some(sb) = self.soundboard.as_ref() else {
            return;
        };
        if sb.sending.is_some() {
            return;
        }
        let Some(sound) = sb
            .defaults
            .iter()
            .chain(sb.guild.iter())
            .flatten()
            .find(|s| s.id == sound_id)
            .cloned()
        else {
            return;
        };
        if let Some(reason) = self.soundboard_block_reason() {
            self.soundboard.as_mut().unwrap().error = Some(reason.into());
            return;
        }
        let Some((_, channel_id)) = self.soundboard_voice() else {
            return;
        };
        let id = self.role_queue(net::ApiCall::SendSound {
            channel_id,
            sound_id: sound.id.clone(),
            source_guild_id: sound.guild_id.clone(),
        });
        let sb = self.soundboard.as_mut().unwrap();
        sb.error = None;
        match id {
            Some(id) => sb.sending = Some((id, sound)),
            None => {
                sb.error = Some(
                    tr!(
                        "送信できませんでした。再接続してください",
                        "Couldn't send. Please reconnect",
                        "无法发送。请重新连接"
                    )
                    .into(),
                )
            }
        }
    }

    pub fn is_soundboard_request(&self, id: u64) -> bool {
        self.soundboard.as_ref().is_some_and(|sb| {
            sb.default_request == Some(id)
                || sb.guild_request == Some(id)
                || sb.sending.as_ref().is_some_and(|(n, _)| *n == id)
        })
    }

    pub fn soundboard_result(&mut self, id: u64, result: Result<serde_json::Value, String>) {
        let Some(sb) = self.soundboard.as_mut() else {
            return;
        };
        if sb.sending.as_ref().is_some_and(|(n, _)| *n == id) {
            let (_, sound) = sb.sending.take().unwrap();
            match result {
                Ok(_) => {
                    // 自分の端末でも鳴らす。あとで届く知らせでは鳴らさない
                    self.recent_sound = Some((sound.id.clone(), std::time::Instant::now()));
                    self.send(Cmd::PlaySound {
                        sound_id: sound.id,
                        volume: sound.volume,
                    });
                }
                Err(e) => {
                    sb.error = Some(super::pins::api_error_text(
                        &e,
                        tr!("鳴らせませんでした", "Couldn't play it", "无法播放"),
                    ))
                }
            }
            return;
        }
        let parsed = result.ok().as_ref().and_then(parse_sounds);
        if sb.default_request == Some(id) {
            sb.default_request = None;
            sb.defaults = Some(parsed.clone().unwrap_or_default());
        } else if sb.guild_request == Some(id) {
            sb.guild_request = None;
            sb.guild = Some(parsed.clone().unwrap_or_default());
        }
        if parsed.is_none() {
            sb.error = Some(
                tr!(
                    "サウンドの一覧を読み込めませんでした",
                    "Couldn't load the sounds",
                    "无法加载音效列表"
                )
                .into(),
            );
        }
    }

    /// VOICE_CHANNEL_EFFECT_SEND。同じボイスチャンネルで鳴らされた音を鳴らす
    pub(super) fn on_voice_effect(&mut self, d: &serde_json::Value) {
        let Some(sound_id) = d.get("sound_id").and_then(|s| match s {
            serde_json::Value::String(s) => Some(s.clone()),
            serde_json::Value::Number(n) => Some(n.to_string()),
            _ => None,
        }) else {
            return;
        };
        let Some((_, channel_id)) = self.soundboard_voice() else {
            return;
        };
        if d.get("channel_id").and_then(|c| c.as_str()) != Some(channel_id.as_str())
            || self.settings.self_deaf
        {
            return;
        }
        let me = self.me.as_ref().map(|m| m.id.as_str());
        if me.is_some() && d.get("user_id").and_then(|u| u.as_str()) == me {
            if let Some((id, at)) = &self.recent_sound {
                if *id == sound_id && at.elapsed() < OWN_ECHO {
                    self.recent_sound = None;
                    return;
                }
            }
        }
        let volume = d
            .get("sound_volume")
            .and_then(|v| v.as_f64())
            .unwrap_or(1.0) as f32;
        self.send(Cmd::PlaySound { sound_id, volume });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::mpsc::UnboundedReceiver;

    fn connected(perms: u64) -> (App, UnboundedReceiver<Cmd>) {
        let mut app = App::default();
        app.me =
            Some(serde_json::from_value(serde_json::json!({"id":"10","username":"me"})).unwrap());
        app.guilds = vec![serde_json::from_value(serde_json::json!({
            "id":"1","name":"g","permissions":perms.to_string()
        }))
        .unwrap()];
        app.my_roles.insert("1".into(), Vec::new());
        app.channels.insert(
            "1".into(),
            vec![serde_json::from_value(serde_json::json!({"id":"20","type":2})).unwrap()],
        );
        app.voice_status = Some(VoiceStatus::Connected {
            guild_id: Some("1".into()),
            channel_id: "20".into(),
        });
        let (_, ev) = std::sync::mpsc::channel();
        let (cmd, rx) = tokio::sync::mpsc::unbounded_channel();
        app.backend = Some(Backend { cmd, ev });
        (app, rx)
    }

    fn api(rx: &mut UnboundedReceiver<Cmd>) -> (u64, net::ApiCall) {
        match rx.try_recv().unwrap() {
            Cmd::Api { id, call } => (id, call),
            _ => panic!("expected API"),
        }
    }

    #[test]
    fn sends_sound_and_plays_it_once_locally() {
        let (mut app, mut rx) = connected(SPEAK | USE_SOUNDBOARD | (1 << 10));
        app.toggle_soundboard();
        let (a, _) = api(&mut rx);
        let (b, _) = api(&mut rx);
        app.soundboard_result(
            a,
            Ok(
                serde_json::json!([{"sound_id":"5","name":"ducky","volume":0.5,"emoji_name":"🦆"},
                {"sound_id":"6","name":"gone","available":false}]),
            ),
        );
        app.soundboard_result(
            b,
            Ok(serde_json::json!({"items":[{"sound_id":"7","name":"mine","guild_id":"1"}]})),
        );
        let sb = app.soundboard.as_ref().unwrap();
        assert_eq!(
            sb.defaults.as_ref().unwrap().len(),
            1,
            "使えない音は出さない"
        );
        assert!(sb.error.is_none());
        app.send_sound("7");
        let (id, call) = api(&mut rx);
        assert!(
            matches!(call, net::ApiCall::SendSound { ref source_guild_id, .. }
            if source_guild_id.as_deref() == Some("1"))
        );
        app.soundboard_result(id, Ok(serde_json::Value::Null));
        assert!(
            matches!(rx.try_recv().unwrap(), Cmd::PlaySound { ref sound_id, .. } if sound_id == "7")
        );
        // 自分が鳴らした音の知らせでは二重に鳴らさない
        let effect = serde_json::json!({"channel_id":"20","user_id":"10","sound_id":"7"});
        app.on_voice_effect(&effect);
        assert!(rx.try_recv().is_err());
        // 他の人が鳴らした音は鳴らす。別のチャンネルの音は鳴らさない
        app.on_voice_effect(&serde_json::json!({"channel_id":"20","user_id":"11","sound_id":"5","sound_volume":0.3}));
        assert!(
            matches!(rx.try_recv().unwrap(), Cmd::PlaySound { volume, .. } if (volume - 0.3).abs() < 1e-6)
        );
        app.on_voice_effect(&serde_json::json!({"channel_id":"99","user_id":"11","sound_id":"5"}));
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn blocked_without_permission_or_while_muted() {
        let (mut app, mut rx) = connected(SPEAK | (1 << 10));
        assert!(app.soundboard_block_reason().unwrap().contains("権限"));
        app.toggle_soundboard();
        let (a, _) = api(&mut rx);
        api(&mut rx);
        app.soundboard_result(a, Ok(serde_json::json!([{"sound_id":"5","name":"x"}])));
        app.send_sound("5");
        assert!(rx.try_recv().is_err());
        assert!(app.soundboard.as_ref().unwrap().error.is_some());
        let (mut app, _) = connected(SPEAK | USE_SOUNDBOARD | (1 << 10));
        app.settings.self_mute = true;
        assert!(app.soundboard_block_reason().unwrap().contains("ミュート"));
    }
}
