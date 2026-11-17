//! BetterDiscord 由来の同梱プラグインのうち、本体側で行う部分（送信・入力中・受信イベント）。
//! 各機能は `self.plugins.on("<id>")` のときだけ働く。設定は `plugins.options(id)`（_lib.lua の settings）
use super::*;

/// 本文を limit（UTF-16 の単位）以下に分ける。改行・空白の位置で切れるならそこで切る。
/// コードブロックの途中で切ったときは、閉じてから次の部分で開き直す
pub fn split_message(text: &str, limit: usize) -> Vec<String> {
    let limit = limit.max(16);
    let mut parts = Vec::new();
    let mut rest = text.to_string();
    let mut reopen: Option<String> = None;
    while !rest.is_empty() {
        if let Some(fence) = reopen.take() {
            rest = format!("{fence}\n{rest}");
        }
        if message_text_len(&rest) <= limit {
            parts.push(std::mem::take(&mut rest));
            break;
        }
        // limit - 4（閉じる ``` の分）に収まる最後の位置
        let budget = limit - 4;
        let mut used = 0usize;
        let mut cut = 0usize;
        for (i, c) in rest.char_indices() {
            let w = c.len_utf16();
            if used + w > budget {
                break;
            }
            used += w;
            cut = i + c.len_utf8();
        }
        let head = &rest[..cut];
        let at = head
            .rfind('\n')
            .filter(|&i| i > cut / 2)
            .or_else(|| head.rfind(' ').filter(|&i| i > cut / 2))
            .map(|i| i + 1)
            .unwrap_or(cut);
        let mut part = rest[..at].trim_end_matches('\n').to_string();
        let tail = rest[at..].to_string();
        // ``` の数が奇数ならコードブロックの途中
        let fences: Vec<&str> = part.lines().filter(|l| l.trim_start().starts_with("```")).collect();
        if fences.len() % 2 == 1 {
            let open = fences.last().unwrap().trim().to_string();
            part.push_str("\n```");
            reopen = Some(if open.len() > 3 { open } else { "```".into() });
        }
        if !part.trim().is_empty() {
            parts.push(part);
        }
        rest = tail;
    }
    parts
}

impl App {
    /// 送信前に、同梱プラグインの指定（分割・@silent・メンションの扱い）を集める。
    /// 分割したときは content を最後の部分にし、前の部分を extra.before に入れる
    pub fn plugin_send_extra(&self, content: &mut String, replying: bool) -> net::SendExtra {
        send_extra(&self.plugins, self.me.as_ref().and_then(|u| u.premium_type), content, replying)
    }
}

pub fn send_extra(
    p: &crate::plugins::Plugins,
    premium: Option<u8>,
    content: &mut String,
    replying: bool,
) -> net::SendExtra {
    {
        let mut extra = net::SendExtra::default();
        if let Some(rest) = content.strip_prefix("@silent ") {
            *content = rest.to_string();
            extra.flags |= 4096;
        }
        if p.on("auto-silent-message") {
            extra.flags |= 4096;
        }
        if p.on("allowed-mentions") {
            let mut parse = Vec::new();
            if !p.opt_bool("allowed-mentions", "no_users") {
                parse.push("users");
            }
            if !p.opt_bool("allowed-mentions", "no_roles") {
                parse.push("roles");
            }
            if !p.opt_bool("allowed-mentions", "no_everyone") {
                parse.push("everyone");
            }
            extra.allowed_mentions = Some(serde_json::json!({
                "parse": parse,
                "replied_user": !p.opt_bool("allowed-mentions", "no_reply"),
            }));
        }
        if replying && p.on("no-reply-ping") {
            let am = extra
                .allowed_mentions
                .get_or_insert_with(|| serde_json::json!({ "parse": ["users", "roles", "everyone"] }));
            am["replied_user"] = serde_json::json!(false);
        }
        if p.on("split-large-messages") {
            let limit = message_text_limit(premium);
            if message_text_len(content) > limit {
                let mut parts = split_message(content, limit);
                *content = parts.pop().unwrap_or_default();
                extra.before = parts;
            }
        }
        extra
    }
}

impl App {
    /// 入力欄に 1 文字打たれた（Extended Typing Sounds）
    pub fn compose_key(&mut self) {
        if self.plugins.on("extended-typing-sounds") {
            let kind = self.plugins.opt_str("extended-typing-sounds", "sound");
            let name = match kind.as_str() {
                "2" => "key2",
                "3" => "key3",
                _ => "key",
            };
            let volume = self.plugins.opt_f64("extended-typing-sounds", "volume").unwrap_or(60.0) as f32 / 100.0;
            crate::sound::play(name, volume);
        }
    }

    /// 文字数の上限を超えても送れる（SplitLargeMessages）
    pub fn can_send_long(&self) -> bool {
        self.plugins.on("split-large-messages")
    }

    /// 入力欄に文字が打たれた。「入力中…」を 8 秒おきに送る（InvisibleTyping・Incognito が動いていれば送らない）
    pub fn compose_typed(&mut self) {
        let Some(channel) = self.sel_channel.clone() else {
            return;
        };
        if self.input.trim().is_empty()
            || self.input.starts_with('/')
            || self.plugins.on("invisible-typing")
            || self.plugins.opt_bool("incognito", "hide_typing")
            || self.editing.is_some()
        {
            return;
        }
        let now = std::time::Instant::now();
        if self
            .typing_sent
            .as_ref()
            .is_some_and(|(c, t)| *c == channel && now.duration_since(*t).as_secs() < 8)
        {
            return;
        }
        self.typing_sent = Some((channel.clone(), now));
        self.api(net::ApiCall::Typing(channel));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_respects_limit_and_lines() {
        let text = (0..300).map(|i| format!("line {i}")).collect::<Vec<_>>().join("\n");
        let parts = split_message(&text, 2000);
        assert!(parts.len() > 1);
        assert!(parts.iter().all(|p| message_text_len(p) <= 2000));
        assert_eq!(parts.join("\n"), text);
    }

    #[test]
    fn split_reopens_code_blocks() {
        let text = format!("```rust\n{}\n```", "let x = 1;\n".repeat(400));
        let parts = split_message(&text, 2000);
        assert!(parts.len() > 1);
        for p in &parts {
            assert!(message_text_len(p) <= 2000);
            assert_eq!(p.matches("```").count() % 2, 0, "{p}");
        }
        assert!(parts[1].starts_with("```rust"));
    }
}

impl App {
    /// TYPING_START: 入力中の人を覚えて（10 秒で消える）、プラグインに知らせる
    pub fn on_typing_start(&mut self, d: &serde_json::Value) {
        let (Some(channel), Some(user)) = (d["channel_id"].as_str(), d["user_id"].as_str()) else {
            return;
        };
        if self.me.as_ref().is_some_and(|me| me.id == user) {
            return;
        }
        let name = d["member"]["nick"]
            .as_str()
            .or_else(|| d["member"]["user"]["global_name"].as_str())
            .or_else(|| d["member"]["user"]["username"].as_str())
            .map(str::to_owned)
            .or_else(|| self.user_name(user))
            .unwrap_or_else(|| user.to_owned());
        let now = std::time::Instant::now();
        let list = self.typing.entry(channel.to_owned()).or_default();
        list.retain(|(u, _, t)| u != user && now.duration_since(*t).as_secs() < 10);
        list.push((user.to_owned(), name.clone(), now));
        self.repaint_after(std::time::Duration::from_secs(10));
        self.plugins.hook(
            "on_typing",
            &[channel.into(), user.into(), name.into()],
        );
    }

    /// いま入力中の人（10 秒以内）
    pub fn typing_in(&self, channel: &str) -> Vec<(String, String)> {
        let now = std::time::Instant::now();
        self.typing
            .get(channel)
            .into_iter()
            .flatten()
            .filter(|(_, _, t)| now.duration_since(*t).as_secs() < 10)
            .map(|(u, n, _)| (u.clone(), n.clone()))
            .collect()
    }

    /// ユーザーの表示名（知っていれば）
    pub fn user_name(&self, user_id: &str) -> Option<String> {
        if let Some(f) = self.friends.relationships.iter().find(|f| f.user.id == user_id) {
            return Some(f.user.display().to_owned());
        }
        self.members
            .values()
            .flatten()
            .find(|m| m.user.as_ref().is_some_and(|u| u.id == user_id))
            .map(|m| m.display().to_owned())
    }

    /// 少数のプレゼンスの変化をプラグインへ（on_presence(user_id, 前の状態, 今の状態, アクティビティ)）
    pub fn plugin_presence_changes(&mut self, list: &[(String, crate::model::Presence)]) {
        for (id, p) in list {
            let old = self
                .presences
                .get(id)
                .map(|o| o.status.clone())
                .unwrap_or_else(|| "offline".into());
            if old == p.status {
                continue;
            }
            let name = self.user_name(id).unwrap_or_default();
            self.plugins.hook(
                "on_presence",
                &[
                    id.clone().into(),
                    old.into(),
                    p.status.clone().into(),
                    p.activity.clone().unwrap_or_default().into(),
                    name.into(),
                ],
            );
        }
    }

    /// ボイスの出入り・ミュートの変化をプラグインへ
    /// on_voice(user_id, 前のチャンネル, 今のチャンネル, サーバー, ミュート, スピーカーミュート, 名前)
    pub fn plugin_voice_change(&mut self, v: &crate::model::VoiceState) {
        let old = self.voice_states.get(&v.user_id);
        let before = old.and_then(|o| o.channel_id.clone()).unwrap_or_default();
        let unchanged = old.is_some_and(|o| {
            o.channel_id == v.channel_id && o.self_mute == v.self_mute && o.self_deaf == v.self_deaf
        });
        if unchanged {
            return;
        }
        if old.and_then(|o| o.channel_id.as_ref()) != v.channel_id.as_ref() {
            if v.channel_id.is_some() {
                self.voice_joined.insert(v.user_id.clone(), std::time::Instant::now());
            } else {
                self.voice_joined.remove(&v.user_id);
            }
        }
        let name = v
            .user
            .as_ref()
            .map(|u| u.display().to_owned())
            .or_else(|| old.and_then(|o| o.user.as_ref().map(|u| u.display().to_owned())))
            .or_else(|| self.user_name(&v.user_id))
            .unwrap_or_default();
        self.plugins.hook(
            "on_voice",
            &[
                v.user_id.clone().into(),
                before.into(),
                v.channel_id.clone().unwrap_or_default().into(),
                v.guild_id.clone().unwrap_or_default().into(),
                v.self_mute.into(),
                v.self_deaf.into(),
                name.into(),
            ],
        );
    }
}

impl App {
    /// FreeEmojis: 自分では使えないサーバーの絵文字・アニメ絵文字を画像の URL に置き換える
    pub fn free_emoji_links(&self, content: &str) -> String {
        if !self.plugins.on("free-emojis") || self.has_nitro() || !content.contains("<") {
            return content.to_string();
        }
        let size = self
            .plugins
            .opt_f64("free-emojis", "size")
            .unwrap_or(48.0)
            .clamp(16.0, 128.0) as u32;
        let current: Vec<&str> = self
            .sel_guild
            .as_deref()
            .and_then(|g| self.guild_emojis.get(g))
            .into_iter()
            .flatten()
            .filter(|e| e.available && !e.animated)
            .map(|e| e.id.as_str())
            .collect();
        let mut out = String::new();
        let mut rest = content;
        while let Some(start) = rest.find('<') {
            out.push_str(&rest[..start]);
            let tail = &rest[start..];
            let parsed = tail.find('>').and_then(|end| {
                let inner = &tail[1..end];
                let (animated, inner) = match inner.strip_prefix("a:") {
                    Some(r) => (true, r),
                    None => (false, inner.strip_prefix(':')?),
                };
                let (name, id) = inner.split_once(':')?;
                (!id.is_empty() && id.bytes().all(|b| b.is_ascii_digit())).then_some((end, animated, name, id))
            });
            match parsed {
                Some((end, animated, name, id)) if animated || !current.contains(&id) => {
                    let ext = if animated { "gif" } else { "webp" };
                    out.push_str(&format!(
                        "https://cdn.discordapp.com/emojis/{id}.{ext}?size={size}&quality=lossless&name={name}"
                    ));
                    rest = &tail[end + 1..];
                }
                Some((end, ..)) => {
                    out.push_str(&tail[..=end]);
                    rest = &tail[end + 1..];
                }
                None => {
                    out.push('<');
                    rest = &tail[1..];
                }
            }
        }
        out.push_str(rest);
        out
    }

    /// FileNameRandomization: アップロードの名前を変える（拡張子は残す）
    pub fn randomize_upload_names(&self, uploads: &mut [crate::clip::Upload]) {
        if !self.plugins.on("file-name-randomization") {
            return;
        }
        use rand::Rng;
        let mode = self.plugins.opt_str("file-name-randomization", "mode");
        let len = self
            .plugins
            .opt_f64("file-name-randomization", "length")
            .unwrap_or(12.0)
            .clamp(4.0, 64.0) as usize;
        let mut rng = rand::thread_rng();
        for (i, u) in uploads.iter_mut().enumerate() {
            let (stem, ext) = match u.name.rsplit_once('.') {
                Some((s, e)) if !s.is_empty() && e.len() <= 10 => (s.to_string(), format!(".{e}")),
                _ => (u.name.clone(), String::new()),
            };
            let random: String = (0..len)
                .map(|_| rng.sample(rand::distributions::Alphanumeric) as char)
                .collect();
            let name = match mode.as_str() {
                "timestamp" => format!("{}{}", chrono::Utc::now().timestamp_millis() + i as i64, ext),
                "suffix" => format!("{stem}_{}{ext}", &random[..random.len().min(6)]),
                _ => format!("{random}{ext}"),
            };
            // SPOILER_ は残す
            u.name = if u.name.starts_with("SPOILER_") {
                format!("SPOILER_{name}")
            } else {
                name
            };
        }
    }

    /// 新着メッセージで本体が行うプラグインの処理（AutoTranslate など）
    pub fn on_plugin_message(&mut self, m: &Message) {
        let mine = self.me.as_ref().is_some_and(|u| u.id == m.author.id);
        if !mine
            && !m.content.trim().is_empty()
            && self.plugins.on("auto-translate")
            && self.plugins.translator_enabled()
        {
            let ok = self
                .plugins
                .query(
                    "auto-translate",
                    "should_translate",
                    &[m.channel_id.clone().into(), m.author.id.clone().into()],
                )
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            if ok {
                let language = self.plugins.translation_target();
                let target = crate::translator::Target::Message {
                    channel: m.channel_id.clone(),
                    id: m.id.clone(),
                    original: m.content.clone(),
                    generation: self.plugins.generation,
                };
                let _ = self
                    .translator
                    .request(target, m.content.clone(), language, self.waker.clone());
            }
        }
    }

    /// Incognito のアクティビティ非表示と端末情報を、毎回の poll で反映する
    pub fn sync_incognito(&mut self) {
        let hide = self.plugins.opt_bool("incognito", "hide_activity");
        if hide != self.incognito_activity_hidden {
            self.incognito_activity_hidden = hide;
            self.send(Cmd::SetActivity(self.settings.show_activity && !hide));
        }
        if self.plugins.on("all-call-time-counter") && !self.voice_joined.is_empty() {
            self.repaint_after(std::time::Duration::from_secs(1));
        }
        ICON_ANIMATE.store(self.plugins.on("auto-animate"), std::sync::atomic::Ordering::Relaxed);
        ICON_HIRES.store(
            self.plugins.on("high-res-profile-images"),
            std::sync::atomic::Ordering::Relaxed,
        );
        net::GENERIC_CLIENT.store(
            self.plugins.opt_bool("incognito", "generic_client"),
            std::sync::atomic::Ordering::Relaxed,
        );
    }
}
