//! ログイン・ボイス通話・ステータス・プロフィール操作。
use std::sync::atomic::Ordering::Relaxed;

use gpui::{Bounds, Pixels};

use super::*;
use crate::login::{self, LoginEvent};
use crate::model::{Channel, User, VoiceState};
use crate::net::{self, Cmd, VoiceStatus};

impl App {
    // ---- ログイン画面 ----
    /// ログインの通信（QR コード・メールアドレス・二段階認証）の結果を受け取る
    pub fn poll_login(&mut self) {
        let mut token = None;
        if let Some(rx) = &self.login.busy {
            match rx.try_recv() {
                Ok(e) => {
                    self.login.busy = None;
                    match e {
                        LoginEvent::Token(t) => token = Some(t),
                        LoginEvent::Mfa { ticket } => {
                            self.login.mfa_ticket = Some(ticket);
                            self.login.mfa_code.clear();
                            self.login_error = None;
                        }
                        LoginEvent::Error(e) => self.login_error = Some(e),
                        _ => {}
                    }
                }
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    self.login.busy = None;
                    self.login_error = Some("ログインできませんでした。もう一度お試しください。".into());
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => {}
            }
        }
        if let Some(qr) = self.login.qr.as_mut() {
            while let Ok(e) = qr.rx.try_recv() {
                match e {
                    LoginEvent::QrUrl(url) => {
                        qr.modules = login::qr_modules(&url);
                        qr.scanned = None;
                    }
                    LoginEvent::QrScanned { name, avatar_url } => qr.scanned = Some((name, avatar_url)),
                    LoginEvent::QrEnded(m) | LoginEvent::Error(m) => qr.ended = Some(m),
                    LoginEvent::Token(t) => token = Some(t),
                    LoginEvent::Mfa { .. } => {}
                }
            }
        }
        if let Some(t) = token {
            self.finish_login(t);
        }
    }

    /// 受け取ったトークンでつなぐ。保存は今までどおり、自分の情報が届いたとき（Event::Me）に token_input から行う
    pub fn finish_login(&mut self, token: String) {
        self.login.qr = None;
        self.login.password.clear();
        self.login.mfa_ticket = None;
        self.login_error = None;
        self.token_input = token.clone();
        self.start(token);
    }


    pub fn sidebar_order(&self) -> Vec<String> {
        match &self.sel_guild {
            None => self.dms.iter().map(|c| c.id.clone()).collect(),
            Some(g) => {
                let Some(chs) = self.channels.get(g) else { return Vec::new() };
                let mut out: Vec<String> = chs.iter().filter(|c| c.is_text() && c.parent_id.is_none()).map(|c| c.id.clone()).collect();
                let mut cats: Vec<&Channel> = chs.iter().filter(|c| c.kind == Channel::CATEGORY).collect();
                cats.sort_by_key(|c| c.position);
                for cat in cats {
                    let collapsed = self.is_category_collapsed(&cat.id);
                    out.extend(
                        chs.iter()
                            .filter(|c| {
                                c.is_text()
                                    && c.parent_id.as_deref() == Some(&cat.id)
                                    && (!collapsed || self.channel_visible_in_collapsed_category(c, None))
                            })
                            .map(|c| c.id.clone()),
                    );
                }
                out
            }
        }
    }

    pub fn select(&mut self, guild: Option<String>, channel: Option<String>) {
        if let Some(gid) = &guild {
            self.ensure_guild_channels(gid);
        }
        self.sel_guild = guild;
        self.sel_channel = None;
        self.cache_dirty = true;
        if let Some(c) = channel {
            self.open_channel(c);
        } else if self.screen_w < 700.0 {
            self.compact_sidebar_closed = false;
        }
    }

    /// (チャンネル名, サーバー名)
    pub fn voice_place(&self, guild_id: Option<&str>, channel_id: &str) -> (String, String) {
        match guild_id {
            Some(g) => {
                let ch = self.channels.get(g).and_then(|l| l.iter().find(|c| c.id == channel_id)).and_then(|c| c.name.clone());
                let gn = self.guilds.iter().find(|x| x.id == g).map(|x| x.name.clone());
                (ch.unwrap_or_else(|| "ボイス".into()), gn.unwrap_or_default())
            }
            None => {
                let n = self.dms.iter().find(|c| c.id == channel_id).map(|c| c.dm_label()).unwrap_or_default();
                (n, "ダイレクトメッセージ".into())
            }
        }
    }

    pub fn apply_voice_settings(&self) {
        let (v, s) = (&self.voice, &self.settings);
        v.mute.store(s.self_mute, Relaxed);
        v.deaf.store(s.self_deaf, Relaxed);
        if v.ptt.load(Relaxed) != s.ptt {
            // 入力モードの切り替えをまたいで古い押下状態を引き継がない。
            v.ptt_down.store(false, Relaxed);
            v.ptt.store(s.ptt, Relaxed);
        }
        v.set_vad_threshold(db_to_lin(s.vad_db));
        v.set_in_gain(s.in_gain);
        v.set_out_gain(s.out_gain);
        let mut changed = false;
        for (slot, val) in [(&v.input_device, &s.input_device), (&v.output_device, &s.output_device)] {
            let mut g = slot.lock().unwrap();
            if *g != *val {
                *g = val.clone();
                changed = true;
            }
        }
        if changed {
            v.restart.store(true, Relaxed);
        }
    }

    pub fn voice_changed(&mut self) {
        self.apply_voice_settings();
        self.save_settings();
        self.send(Cmd::VoiceFlags);
    }

    pub fn toggle_mute(&mut self) {
        if self.settings.self_deaf {
            self.settings.self_deaf = false;
            self.settings.self_mute = false;
        } else {
            self.settings.self_mute = !self.settings.self_mute;
        }
        self.voice_changed();
    }

    pub fn toggle_deaf(&mut self) {
        self.settings.self_deaf = !self.settings.self_deaf;
        self.voice_changed();
    }

    pub fn mic_off(&self) -> bool {
        self.settings.self_mute || self.settings.self_deaf
    }

    pub fn join_voice(&mut self, guild_id: Option<String>, channel_id: String, ring: bool) {
        self.voice_status = Some(VoiceStatus::Connecting { guild_id: guild_id.clone(), channel_id: channel_id.clone() });
        self.update_voice_call_started();
        self.send(Cmd::JoinVoice { guild_id, channel_id, ring });
    }

    pub fn leave_voice(&mut self) {
        self.voice_status = None;
        self.update_voice_call_started();
        self.send(Cmd::LeaveVoice);
    }

    pub(super) fn update_voice_call_started(&mut self) {
        match &self.voice_status {
            Some(VoiceStatus::Connected { guild_id, channel_id }) => {
                if !self.voice_call_started.as_ref().is_some_and(|(g, c, _)| g == guild_id && c == channel_id) {
                    self.voice_call_started = Some((guild_id.clone(), channel_id.clone(), std::time::Instant::now()));
                }
            }
            _ => self.voice_call_started = None,
        }
    }

    /// 接続中（または接続しようとしている）チャンネル
    pub fn voice_channel(&self) -> Option<(&Option<String>, &String, bool)> {
        match self.voice_status.as_ref()? {
            VoiceStatus::Connecting { guild_id, channel_id } => Some((guild_id, channel_id, false)),
            VoiceStatus::Connected { guild_id, channel_id } => Some((guild_id, channel_id, true)),
            _ => None,
        }
    }

    /// channel_id → そこにいる user_id（並び順固定）。voice_states が変わったら作り直す
    pub fn reindex_voice(&mut self) {
        self.voice_index.clear();
        for v in self.voice_states.values() {
            if let Some(c) = &v.channel_id {
                self.voice_index.entry(c.clone()).or_default().push(v.user_id.clone());
            }
        }
        for l in self.voice_index.values_mut() {
            l.sort();
        }
    }

    pub fn find_user(&self, id: &str) -> Option<User> {
        if let Some(me) = self.me.as_ref().filter(|m| m.id == id) {
            return Some(me.clone());
        }
        if let Some(u) = self.voice_states.get(id).and_then(|v| v.user.clone()) {
            return Some(u);
        }
        self.dms.iter().flat_map(|c| c.recipients.iter()).find(|u| u.id == id).cloned()
    }

    pub fn is_speaking(&self, id: &str) -> bool {
        if self.me.as_ref().is_some_and(|m| m.id == id) {
            return self.voice.transmitting.load(Relaxed);
        }
        self.voice.speaking.lock().unwrap().contains(id)
    }

    /// あるボイスチャンネルにいる人
    pub fn voice_members<'a>(&'a self, channel_id: &str) -> impl Iterator<Item = &'a VoiceState> + 'a {
        self.voice_index
            .get(channel_id)
            .into_iter()
            .flatten()
            .filter_map(|id| self.voice_states.get(id))
    }

    pub fn set_status(&mut self, st: String) {
        self.send(Cmd::SetStatus(st.clone()));
        self.settings.status = st;
        self.save_settings();
    }

    pub fn open_profile(&mut self, user: User, anchor: Bounds<Pixels>, above: bool) {
        let guild_id = self.sel_guild.clone();
        let key = net::profile_key(&user.id, guild_id.as_deref());
        if !self.profiles.contains_key(&key) {
            self.profiles.insert(key, ProfileState::Loading);
            self.send(Cmd::LoadProfile { user_id: user.id.clone(), guild_id: guild_id.clone() });
        }
        self.popup = Some(ProfilePopup { user, guild_id, anchor, above });
    }

    pub fn save_profile(&mut self) {
        let Some(d) = &self.profile_draft else { return };
        let changed = |i: usize| (d.fields[i].trim() != d.base[i].trim()).then(|| d.fields[i].trim().to_string());
        let avatar = match &d.avatar {
            Some(AvatarEdit::Set { uri, .. }) => Some(Some(uri.clone())),
            Some(AvatarEdit::Remove) => Some(None),
            None => None,
        };
        self.send(Cmd::UpdateProfile { global_name: changed(0), pronouns: changed(1), bio: changed(2), avatar });
        self.profile_saving = true;
        self.profile_error = None;
    }

}
