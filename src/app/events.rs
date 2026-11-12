//! バックエンド（通信スレッド・翻訳・プラグイン）からのイベント処理。
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use gpui::RenderImage;

use super::*;
use crate::images;
use crate::model::Message;
use crate::net::{self, Cmd, Event, VoiceStatus};
use crate:: cache;

impl App {
    pub fn poll(&mut self) {
        for item in self.translator.drain() {
            match item.target {
                crate::translator::Target::Message { channel, id, original, generation } => {
                    if self.plugins.generation == generation && self.plugins.translator_enabled() && self.messages.get(&channel).is_some_and(|items| items.iter().any(|m| m.id == id && m.content == original)) {
                        match item.result {
                            Ok(result) => { if self.translator.messages.len() >= 512 { self.translator.messages.clear(); } self.translator.messages.insert((channel, id), (original, result)); self.plugins.revision = self.plugins.revision.wrapping_add(1); },
                            Err(error) => self.show_toast(&error),
                        }
                    }
                }
                crate::translator::Target::Outgoing { channel, request_id, generation } => {
                    if self.translator.outgoing.get(&channel) != Some(&request_id) { continue; }
                    self.translator.outgoing.remove(&channel);
                    if let Some(sub) = self.compose_submissions.get_mut(&channel).filter(|s| !s.failed) {
                        let result = if self.plugins.translator_enabled() && self.plugins.generation == generation { item.result } else { Err("翻訳中にプラグインが無効になりました".into()) };
                        match result {
                            Ok(text) if !text.trim().is_empty() && message_text_len(&text) <= message_text_limit(self.me.as_ref().and_then(|u| u.premium_type)) => {
                                let cmd = Cmd::Send { channel_id: channel.clone(), content: text.clone(), reply_to: sub.reply.as_ref().map(|m| m.id.clone()), files: sub.uploads.iter().map(|u| (u.name.clone(), u.data.clone())).collect() };
                                sub.content = text;
                                if self.backend.as_ref().is_none_or(|b| b.cmd.send(cmd).is_err()) { sub.failed = true; sub.rejected = Some("送信できませんでした".into()); }
                            }
                            Ok(_) => { sub.failed = true; sub.rejected = Some("翻訳結果が空か文字数制限を超えています".into()); }
                            Err(error) => { sub.failed = true; sub.rejected = Some(error); }
                        }
                    }
                }
            }
        }
        if let Some(delay) = self.message_log.save_if_due() { self.repaint_after(delay); }
        let (actions, next) = self.plugins.tick();
        for action in actions {
            match action { crate::plugins::Action::Send(channel_id, content) => self.send(Cmd::Send { channel_id, content, reply_to: None, files: Vec::new() }) }
        }
        if let Some(delay) = next { self.repaint_after(delay); }
        if self.login_retry_at.is_some_and(|t| t <= std::time::Instant::now()) {
            self.login_retry_at = None;
            if let Some(t) = self.load_token() {
                self.retry_login(t);
            }
        }
        if let Some(cache) = &self.member_index {
            let elapsed = cache.updated.elapsed();
            if elapsed >= MEMBER_INDEX_TTL {
                self.member_index = None;
            } else {
                self.repaint_after(MEMBER_INDEX_TTL - elapsed);
            }
        }
        let Some(b) = &self.backend else { return };
        let events: Vec<Event> = b.ev.try_iter().collect();
        if !events.is_empty() {
            self.cache_dirty = true;
        }
        let mut voice_index_dirty = false;
        let mut sticker_cache_dirty = false;
        let mut member_indexes: HashMap<String, HashMap<String, usize>> = HashMap::new();
        let mut last_member_guild = None;
        if events.iter().any(|event| matches!(event, Event::GuildMembers(..))) {
            if let Some(cache) = self.member_index.take() {
                member_indexes.insert(cache.guild_id, cache.index);
            }
        }
        for e in events {
            if matches!(
                e,
                Event::ComposeResult { .. }
                    | Event::Messages(..)
                    | Event::NoAccess(..)
                    | Event::OlderMessages { .. }
                    | Event::MessageCreate(..)
                    | Event::MessageUpdate { .. }
                    | Event::MessageDelete { .. }
            ) {
                self.bump_msgs();
            }
            match e {
                Event::Api { id: _, call, result } => self.on_api_result(call, result),
                Event::Gateway { t, d } => self.on_gateway_event(&t, d),
                Event::GuildEmojis(list) => {
                    self.guild_emojis.extend(list);
                    self.emoji_rev += 1;
                }
                Event::ReadyExtra(d) => {
                    if let Some(map) = d["my_roles"].as_object() {
                        for (gid, roles) in map {
                            let roles = roles.as_array().into_iter().flatten().filter_map(|r| r.as_str().map(String::from)).collect();
                            self.my_roles.insert(gid.clone(), roles);
                        }
                    }
                    let rs = &d["read_state"];
                    if let Some(entries) = rs["entries"].as_array().or(rs.as_array()) {
                        let cg = &d["channel_guild"];
                        self.mention_counts.clear();
                        self.channel_read_rank.clear();
                        for e in entries {
                            // read_state_type が 0（チャンネル）以外は通知の既読などなので数えない
                            if e["read_state_type"].as_u64().unwrap_or(0) != 0 {
                                continue;
                            }
                            let Some(cid) = e["id"].as_str() else { continue };
                            let viewed = e["last_viewed"].as_u64().unwrap_or(0);
                            let last_msg = e["last_message_id"].as_str().and_then(|s| s.parse::<u64>().ok()).unwrap_or(0);
                            if viewed > 0 || last_msg > 0 {
                                self.channel_read_rank.insert(cid.to_string(), (viewed, last_msg));
                            }
                            if let Some(n) = e["mention_count"].as_u64().filter(|&n| n > 0) {
                                let gid = cg[cid].as_str().map(String::from);
                                self.mention_counts.insert(cid.to_string(), (gid, n as u32));
                            }
                        }
                    }
                    let ugs = &d["user_guild_settings"];
                    let entries = ugs["entries"].as_array().or(ugs.as_array());
                    for e in entries.into_iter().flatten() {
                        if let Some((gid, s)) = GuildSettings::parse(e) {
                            self.guild_settings.insert(gid, s);
                        }
                    }
                    if let Some(folders) = GuildFolder::from_ready_extra(&d) {
                        self.apply_guild_folders(folders);
                    }
                }
                Event::AckFailed { channel_id, message_id } => {
                    // より新しい ACK を既に出していれば古い失敗は無視する。
                    if self.acked.get(&channel_id) == Some(&message_id) {
                        self.acked.remove(&channel_id);
                        self.ack_retry_at.insert(channel_id, std::time::Instant::now() + ACK_RETRY_DELAY);
                        self.repaint_after(ACK_RETRY_DELAY);
                    }
                }
                Event::Me(u) => {
                    if self.remember && !self.token_input.is_empty() {
                        if let Err(e) = save_token(self.profile.as_deref(), self.token_input.trim()) {
                            eprintln!("トークン保存失敗: {e}");
                            self.status = "エラー: ログイン情報をこのPCに保存できませんでした。次回はもう一度ログインしてください。".into();
                        }
                    }
                    self.token_input.clear();
                    // 別アカウントの一覧に名前を出すため、プロファイルのフォルダに覚えておく
                    crate::profile::remember_account(self.profile.as_deref(), u.display());
                    self.me = Some(u);
                    self.status = self.gateway_status.clone();
                }
                Event::AuthFailed => {
                    cache::remove(self.profile.as_deref());
                    self.reset();
                    self.login_error = Some("ログイン情報が無効です。もう一度ログインしてください。".into());
                    return;
                }
                Event::LoginFailed if self.me.is_some() => {
                    // 前回の控えを出している：画面はそのままにして、少し待ってつなぎ直す
                    self.backend = None;
                    for s in self.compose_submissions.values_mut() { s.failed = true; }
                    self.status = "接続できませんでした。もう一度つないでいます…".into();
                    self.gateway_status = self.status.clone();
                    self.login_retry_at = Some(std::time::Instant::now() + LOGIN_RETRY_DELAY);
                    self.repaint_after(LOGIN_RETRY_DELAY);
                    return;
                }
                Event::LoginFailed => {
                    self.backend = None;
                    self.login_error = Some("接続できませんでした。通信状態を確認して、もう一度お試しください。".into());
                }
                Event::MessagesFailed(cid) => {
                    self.pending_message_loads.remove(&cid);
                    if self.loaded_channels.iter().any(|id| id == &cid) {
                        if self.messages.contains_key(&cid) {
                            self.status = MESSAGE_RELOAD_ERROR.into();
                            self.message_status_channel = Some(cid.clone());
                        }
                        self.message_errors.insert(cid);
                    }
                }
                Event::ProfileSaved(r) => {
                    self.profile_saving = false;
                    match r {
                        Ok(()) => {
                            // 次の描画で、保存後のプロフィールを読み直して編集を始め直す
                            self.profile_draft = None;
                            self.profile_error = None;
                            self.profile_saved_at = Some(std::time::Instant::now());
                            if let Some(me) = &self.me {
                                let prefix = format!("{}/", me.id);
                                self.profiles.retain(|k, _| !k.starts_with(&prefix));
                            }
                        }
                        Err(e) => self.profile_error = Some(e),
                    }
                }
                Event::ComposeRejected { channel_id, reason } => {
                    if let Some(submission) = self.compose_submissions.get_mut(&channel_id) {
                        submission.rejected = Some(reason);
                    }
                }
                Event::ComposeResult { channel_id, result } => {
                    if let Err(()) = result {
                        if let Some(submission) = self.compose_submissions.get_mut(&channel_id) {
                            submission.failed = true;
                        }
                    } else if let Some(submission) = self.compose_submissions.remove(&channel_id) {
                        if let Ok(Some(message)) = result {
                            if let Some(changes) = self.pending_message_loads.get_mut(&channel_id) {
                                if let Some(original) = &submission.editing {
                                    let already_changed = changes.iter().any(|change| match change {
                                        MessageChange::Update { id, .. } | MessageChange::Delete(id) => id == &original.id,
                                        _ => false,
                                    });
                                    if !already_changed && message.id == original.id {
                                        changes.push(MessageChange::Update { id: message.id.clone(), content: Some(message.content.clone()), embeds: None });
                                    }
                                } else {
                                    changes.push(MessageChange::Create(message.clone()));
                                }
                            }
                            if let Some(list) = self.messages.get_mut(&channel_id) {
                                if let Some(original) = submission.editing {
                                    if let Some(current) = list.iter_mut().find(|m| m.id == original.id) {
                                        // ゲートウェイから新しい更新が先に届いていれば上書きしない。
                                        if current.content == original.content && message.id == original.id {
                                            current.content = message.content;
                                            current.edited_timestamp = message.edited_timestamp;
                                        }
                                    }
                                } else if !list.iter().any(|m| m.id == message.id) {
                                    // REST の応答が後続のゲートウェイ通知より遅れても時系列を保つ。
                                    let at = list.partition_point(|m| (m.id.len(), m.id.as_str()) < (message.id.len(), message.id.as_str()));
                                    list.insert(at, message);
                                    let cap = if self.sel_channel.as_deref() == Some(&channel_id) { MAX_HISTORY } else { MAX_MSGS };
                                    if list.len() > cap {
                                        let cut = list.len() - cap;
                                        if let Some(h) = self.history.get_mut(&channel_id) { h.end = false; }
                                        sticker_cache_dirty |= list[..cut].iter().any(|m| !m.sticker_items.is_empty());
                                        list.drain(..cut);
                                    }
                                }
                            }
                        } else {
                            self.status = "処理は完了しました。表示が更新されない場合は再読み込みしてください。".into();
                        }
                    }
                }
                Event::Guilds(g) => {
                    self.guilds = g;
                    self.apply_guild_order();
                }
                Event::Dms(mut d) => {
                    d.sort_by_cached_key(|c| std::cmp::Reverse(c.last_id()));
                    self.dms = d;
                    self.dms_loaded = true;
                }
                Event::GuildChannels(gid, mut c) => {
                    // 表示順（テキスト→ボイス、position順）に一度だけ並べておく
                    c.sort_by_key(|c| (c.is_voice(), c.position));
                    self.stale_guilds.remove(&gid);
                    self.channels.insert(gid.clone(), c);
                    if self.sel_guild.as_deref() == Some(gid.as_str()) {
                        let valid = self.sel_channel.as_deref().is_some_and(|cid| {
                            self.channels.get(&gid).is_some_and(|chs| chs.iter().any(|ch| ch.id == cid && ch.is_text()))
                        });
                        if !valid {
                            if let Some(cid) = self.default_guild_channel(&gid) {
                                self.open_channel(cid);
                            }
                        }
                    }
                }
                Event::GuildRoles(gid, roles) => {
                    self.roles.insert(gid, roles);
                    self.member_ver += 1;
                }
                Event::MemberList { guild_id, list_id, groups, ops } => {
                    if let Some(list) = self.member_lists.get_mut(&guild_id) {
                        if list.apply(&list_id, groups, ops) {
                            self.member_ver += 1;
                        }
                    }
                }
                Event::GatewayReady => {
                    // 一からつながったときは購読が消えているので、次の描画で送り直す
                    self.member_sub = None;
                    self.requested_members.clear();
                }
                Event::GuildMembers(gid, members) => {
                    last_member_guild = Some(gid.clone());
                    let list = self.members.entry(gid.clone()).or_default();
                    // 同じ描画前に複数の塊が届いた場合、索引を共有する
                    let index = member_indexes.entry(gid).or_insert_with(||
                        list.iter().enumerate().filter_map(|(i, m)| m.user.as_ref().map(|u| (u.id.clone(), i))).collect());
                    for member in members {
                        let Some(id) = member.user.as_ref().map(|u| u.id.clone()) else { continue };
                        match index.get(&id) {
                            Some(&i) => list[i] = member,
                            None => {
                                index.insert(id, list.len());
                                list.push(member);
                            }
                        }
                    }
                    self.member_ver += 1;
                }
                Event::Messages(cid, mut m) => {
                    let changes = self.pending_message_loads.remove(&cid).unwrap_or_default();
                    // 応答待ちの間に破棄されたチャンネルは復活させない
                    if !self.loaded_channels.iter().any(|id| id == &cid) {
                        continue;
                    }
                    let end = m.len() < net::HISTORY_PAGE;
                    for change in changes { change.apply(&mut m); }
                    let cap = if self.sel_channel.as_deref() == Some(&cid) { MAX_HISTORY } else { MAX_MSGS };
                    let trimmed = m.len() > cap;
                    if trimmed { m.drain(..m.len() - cap); }
                    if self.plugins.logger_enabled() {
                        for x in &m { self.message_log.remember(x); }
                        self.message_log.restore(&cid, &mut m, end && !trimmed);
                    }
                    self.history.insert(cid.clone(), History { end: end && !trimmed, ..Default::default() });
                    self.stale_channels.remove(&cid);
                    self.no_access.remove(&cid);
                    self.clear_message_error_status(&cid);
                    self.message_errors.remove(&cid);
                    self.request_missing_message_members(&cid, &m);
                    if let Some(old) = self.messages.insert(cid, m) {
                        sticker_cache_dirty |= old.iter().any(|m| !m.sticker_items.is_empty());
                    }
                }
                Event::NoAccess(cid) => {
                    self.pending_message_loads.remove(&cid);
                    if !self.loaded_channels.iter().any(|id| id == &cid) {
                        continue;
                    }
                    self.history.insert(cid.clone(), History { end: true, ..Default::default() });
                    self.clear_message_error_status(&cid);
                    self.message_errors.remove(&cid);
                    self.no_access.insert(cid.clone());
                    self.stale_channels.remove(&cid);
                    self.messages.insert(cid, Vec::new());
                }
                Event::OlderMessages { channel_id, before, messages } => {
                    let Some(h) = self.history.get_mut(&channel_id) else { continue };
                    // 一覧を読み直した後などに届いた、今は待っていない応答は捨てる
                    if h.pending.as_deref() != Some(before.as_str()) {
                        continue;
                    }
                    h.pending = None;
                    let Some(older) = messages else {
                        h.retry_at = Some(std::time::Instant::now() + std::time::Duration::from_secs(5));
                        continue;
                    };
                    let end = older.len() < net::HISTORY_PAGE;
                    h.end = end;
                    // 待っている間に一覧が入れ替わっていたら捨てる（次のスクロールで取り直す）
                    let Some(list) = self.messages.get_mut(&channel_id) else { continue };
                    if list.first().map(|m| m.id.as_str()) != Some(before.as_str()) {
                        h.end = false;
                        continue;
                    }
                    let have: HashSet<&str> = list.iter().map(|m| m.id.as_str()).collect();
                    let older: Vec<Message> = older.into_iter().filter(|m| !have.contains(m.id.as_str())).collect();
                    if older.is_empty() {
                        h.end = true;
                        continue;
                    }
                    self.request_missing_message_members(&channel_id, &older);
                    let Some(list) = self.messages.get_mut(&channel_id) else { continue };
                    list.splice(0..0, older);
                    if self.plugins.logger_enabled() {
                        for x in list.iter().take(net::HISTORY_PAGE) { self.message_log.remember(x); }
                        self.message_log.restore(&channel_id, list, end);
                    }
                }
                Event::MessageCreate(m) => {
                    if let (Some(gid), Some(mut member)) = (m.guild_id.clone().or_else(|| self.guild_for_channel(&m.channel_id)), m.member.clone()) {
                        member.user = Some(m.author.clone());
                        let list = self.members.entry(gid).or_default();
                        match list.iter_mut().find(|x| x.user.as_ref().is_some_and(|u| u.id == m.author.id)) {
                            Some(slot) => *slot = member,
                            None => list.push(member),
                        }
                    }
                    self.plugins.event(&m);
                    if self.plugins.logger_enabled() {
                        self.message_log.remember(&m);
                    }
                    self.waker.wake();
                    if let Some(changes) = self.pending_message_loads.get_mut(&m.channel_id) {
                        changes.push(MessageChange::Create(m.clone()));
                    }
                    let mine = self.me.as_ref().is_some_and(|u| u.id == m.author.id);
                    if !mine && self.sel_channel.as_deref() != Some(&m.channel_id) {
                        self.unread.insert(m.channel_id.clone(), m.guild_id.clone());
                    }
                    if !mine {
                        self.on_new_message(&m);
                    }
                    // 新着の DM は必ず最新なので、全体を並べ直さず先頭へ移すだけでよい
                    if let Some(i) = self.dms.iter().position(|c| c.id == m.channel_id) {
                        let mut dm = self.dms.remove(i);
                        dm.last_message_id = Some(m.id.clone());
                        self.dms.insert(0, dm);
                    }
                    if let Some(list) = self.messages.get_mut(&m.channel_id) {
                        // 重複は自分の送信の折り返しなので末尾側から探す
                        if !list.iter().rev().any(|x| x.id == m.id) {
                            let viewing = self.sel_channel.as_deref() == Some(m.channel_id.as_str());
                            list.push(m);
                            // 表示中のチャンネルは遡って読んだ分を消さない（読んでいる行が消えるため）
                            let cap = if viewing { MAX_HISTORY } else { MAX_MSGS };
                            if list.len() > cap {
                                let cut = list.len() - cap;
                                if let Some(h) = self.history.get_mut(&list[0].channel_id) {
                                    h.end = false;
                                }
                                sticker_cache_dirty |= list[..cut].iter().any(|m| !m.sticker_items.is_empty());
                                list.drain(..cut);
                            }
                        }
                    }
                }
                Event::MessageUpdate { channel_id, id, content, embeds } => {
                    if let Some(changes) = self.pending_message_loads.get_mut(&channel_id) {
                        changes.push(MessageChange::Update { id: id.clone(), content: content.clone(), embeds: embeds.clone() });
                    }
                    let logged = content.as_deref().is_some_and(|c| self.on_message_edit(&channel_id, &id, c));
                    if let Some(m) = self.messages.get_mut(&channel_id).and_then(|l| l.iter_mut().find(|m| m.id == id)) {
                        if let Some(c) = content {
                            if c != m.content {
                                m.content = c;
                                m.edited_timestamp = Some(String::new());
                            }
                        }
                        if logged {
                            m.log = self.message_log.mark_for(&id);
                        }
                        if let Some(e) = embeds {
                            m.embeds = e;
                        }
                    }
                }
                Event::MessageDelete { channel_id, id } => {
                    sticker_cache_dirty |= self.on_message_delete(&channel_id, &id);
                }
                Event::Status(s) => {
                    self.gateway_status = s.clone();
                    self.status = s;
                }
                Event::Error(s) => {
                    eprintln!("操作失敗: {s}");
                    let action = if s.starts_with("サーバー一覧:") { "サーバー一覧を取得できませんでした。" }
                        else if s.starts_with("DM一覧:") { "DM一覧を取得できませんでした。" }
                        else if s.starts_with("呼び出し失敗:") { "呼び出しに失敗しました。" }
                        else { "操作を完了できませんでした。" };
                    self.status = format!("エラー: {action}通信状態を確認して、もう一度お試しください。");
                }
                Event::Profile { key, result } => {
                    let st = match result {
                        Ok(d) => ProfileState::Ready(d),
                        Err(e) => {
                            eprintln!("プロフィール取得失敗: {e}");
                            ProfileState::Failed
                        }
                    };
                    self.profiles.insert(key, st);
                }
                Event::VoiceStates(list) => {
                    for v in list {
                        let previous = self.voice_states.get(&v.user_id).and_then(|old| old.channel_id.as_deref());
                        voice_index_dirty |= previous != v.channel_id.as_deref();
                        if v.channel_id.is_some() {
                            self.voice_states.insert(v.user_id.clone(), v);
                        } else {
                            self.voice_states.remove(&v.user_id);
                        }
                    }
                }
                Event::Voice(st) => {
                    match st {
                        VoiceStatus::Disconnected => self.voice_status = None,
                        VoiceStatus::Error(e) => {
                            self.voice_status = None;
                            eprintln!("ボイス接続失敗: {e}");
                            self.status = "エラー: ボイスに接続できませんでした。通信状態と音声設定を確認してください。".into();
                        }
                        st => {
                            if let (VoiceStatus::Connected { channel_id, .. }, Some(me)) = (&st, &self.me) {
                                if let Some(r) = self.calls.get_mut(channel_id) {
                                    r.retain(|u| u != &me.id);
                                }
                            }
                            self.voice_status = Some(st);
                        }
                    }
                    self.update_voice_call_started();
                },
                Event::Call { channel_id, ringing } => {
                    self.calls.insert(channel_id, ringing);
                }
                Event::CallEnded(cid) => {
                    self.calls.remove(&cid);
                    self.voice_states.retain(|_, v| v.channel_id.as_deref() != Some(&cid) || v.guild_id.is_some());
                    voice_index_dirty = true;
                }
                Event::OwnPresence(p) => {
                    if let Some(id) = self.me.as_ref().map(|u| u.id.clone()) {
                        self.presences.insert(id, p);
                        self.member_ver += 1;
                    }
                }
                Event::Presences(list) => {
                    // 並びが変わるのはオンライン/オフラインが入れ替わったときだけ
                    let flips = list.iter().any(|(id, p)| {
                        let was = self.presences.get(id).is_some_and(|o| o.status != "offline");
                        was != (p.status != "offline")
                    });
                    if flips {
                        self.member_ver += 1;
                    }
                    self.presences.extend(list);
                }
                Event::Sticker(id, r) => {
                    // チャンネル破棄後に到着した結果はテクスチャ化しない
                    if !matches!(self.stickers.get(&id), Some(StickerState::Loading)) {
                        continue;
                    }
                    let state = match r {
                        Ok(f) => {
                            let [w, h] = f.size;
                            let frames: Vec<image::Frame> = f
                                .frames
                                .into_iter()
                                .filter_map(|(rgba, ms)| {
                                    let img = image::RgbaImage::from_raw(w as u32, h as u32, rgba)?;
                                    Some(images::bgra_frame(img, std::time::Duration::from_millis(ms.max(10) as u64)))
                                })
                                .collect();
                            if frames.is_empty() {
                                StickerState::Failed
                            } else {
                                StickerState::Ready(Arc::new(RenderImage::new(frames)))
                            }
                        }
                        Err(e) => {
                            eprintln!("スタンプ {id} の読み込み失敗: {e}");
                            StickerState::Failed
                        }
                    };
                    self.stickers.insert(id, state);
                }
            }
        }
        if voice_index_dirty {
            self.reindex_voice();
        }
        if let Some(guild_id) = last_member_guild {
            if let Some(index) = member_indexes.remove(&guild_id) {
                self.member_index = Some(RecentMemberIndex { guild_id, index, updated: std::time::Instant::now() });
                self.repaint_after(MEMBER_INDEX_TTL);
            }
        }
        if sticker_cache_dirty {
            self.prune_stickers();
        }
    }


    pub(super) fn on_api_result(&mut self, call: net::ApiCall, result: Result<serde_json::Value, String>) {
        use net::ApiCall as A;
        let v = match result {
            Ok(v) => v,
            Err(e) => {
                let what = match &call {
                    A::CreateInvite { .. } => "招待リンクを作れませんでした",
                    A::LeaveGuild(_) => "サーバーから脱退できませんでした",
                    A::ModifyGuildSettings { .. } | A::ModifyChannelSettings { .. } => "通知設定を変えられませんでした",
                    A::AckGuild(_) | A::AckBulk(_) => "既読にできませんでした",
                    _ => "Discord への操作に失敗しました",
                };
                eprintln!("{what}: {call:?}: {e}");
                self.show_toast(what);
                return;
            }
        };
        match call {
            A::CreateInvite { .. } => {
                if let Some(code) = v["code"].as_str() {
                    self.clipboard_out = Some(format!("https://discord.gg/{code}"));
                    self.show_toast("招待リンクをコピーしました");
                }
            }
            A::LeaveGuild(gid) => self.remove_guild(&gid),
            A::GetEmojis(gid) => self.set_guild_emojis(gid, v),
            A::ModifyGuildSettings { .. } | A::ModifyChannelSettings { .. } => {
                if let Some((gid, s)) = GuildSettings::parse(&v) {
                    self.guild_settings.insert(gid, s);
                }
            }
            _ => {}
        }
    }

    /// 編集を受け取ったとき（一覧を書き換える前）。MessageLogger が記録したら true
    pub(super) fn on_message_edit(&mut self, channel_id: &str, id: &str, content: &str) -> bool {
        let old = self.messages.get(channel_id).and_then(|l| l.iter().find(|m| m.id == id)).or_else(|| self.message_log.cached(id)).cloned();
        let Some(old) = old.filter(|o| o.content != content) else { return false };
        self.plugins.message_hook("on_edit", &old, Some(content));
        let Some(opt) = self.plugins.logger_options() else { return false };
        let me = self.me.as_ref().map(|u| u.id.as_str());
        if !crate::message_log::MessageLog::wanted(&old, me, opt) {
            return false;
        }
        self.message_log.on_edit(&old, content);
        true
    }

    /// 削除を受け取ったとき。MessageLogger が記録したら一覧に削除済みとして残し、そうでなければ消す。
    /// 消したメッセージにスタンプがあれば true（スタンプの画像を手放すため）
    pub(super) fn on_message_delete(&mut self, channel_id: &str, id: &str) -> bool {
        if let Some(changes) = self.pending_message_loads.get_mut(channel_id) {
            changes.push(MessageChange::Delete(id.to_string()));
        }
        let old = self.messages.get(channel_id).and_then(|l| l.iter().find(|m| m.id == id)).or_else(|| self.message_log.cached(id)).cloned();
        if old.as_ref().and_then(|m| m.log.as_ref()).is_some_and(|log| log.deleted_at.is_some()) {
            return false;
        }
        let mut keep = false;
        if let Some(old) = old.filter(|o| o.log.as_ref().is_none_or(|l| l.deleted_at.is_none())) {
            self.plugins.message_hook("on_delete", &old, None);
            if let Some(opt) = self.plugins.logger_options() {
                let me = self.me.as_ref().map(|u| u.id.as_str());
                if crate::message_log::MessageLog::wanted(&old, me, opt) {
                    self.message_log.on_delete(&old);
                    keep = true;
                }
            }
        }
        let Some(l) = self.messages.get_mut(channel_id) else { return false };
        if keep {
            if let Some(m) = l.iter_mut().find(|m| m.id == id) {
                m.log = self.message_log.mark_for(id);
            }
            return false;
        }
        let stickers = l.iter().any(|m| m.id == id && !m.sticker_items.is_empty());
        l.retain(|m| m.id != id);
        stickers
    }

    pub(super) fn on_gateway_event(&mut self, t: &str, mut d: serde_json::Value) {
        match t {
            "MESSAGE_DELETE_BULK" => {
                let Some(cid) = d["channel_id"].as_str().map(String::from) else { return };
                let ids: Vec<String> = d["ids"].as_array().into_iter().flatten().filter_map(|v| v.as_str().map(String::from)).collect();
                let mut stickers = false;
                for id in ids {
                    stickers |= self.on_message_delete(&cid, &id);
                }
                self.bump_msgs();
                if stickers {
                    self.prune_stickers();
                }
            }
            "GUILD_EMOJIS_UPDATE" => {
                if let Some(gid) = d["guild_id"].as_str().map(String::from) {
                    let list = d.get_mut("emojis").map(serde_json::Value::take).unwrap_or_default();
                    self.set_guild_emojis(gid, list);
                }
            }
            "GUILD_CREATE" if d["emojis"].is_array() => {
                if let Some(gid) = d["id"].as_str().map(String::from) {
                    let list = d.get_mut("emojis").map(serde_json::Value::take).unwrap_or_default();
                    self.set_guild_emojis(gid, list);
                }
            }
            "USER_GUILD_SETTINGS_UPDATE" => {
                if let Some((gid, s)) = GuildSettings::parse(&d) {
                    self.guild_settings.insert(gid, s);
                }
            }
            "USER_SETTINGS_PROTO_UPDATE" => {
                let kind = d["settings"]["type"].as_u64().unwrap_or(1);
                if kind == 1 {
                    if let Some(folders) = d["settings"]["proto"].as_str().and_then(GuildFolder::from_proto_base64) {
                        self.apply_guild_folders(folders);
                    }
                }
            }
            "USER_SETTINGS_UPDATE" => {
                if let Some(folders) = GuildFolder::from_user_settings_json(&d) {
                    self.apply_guild_folders(folders);
                }
            }
            // unavailable のときは障害で一時的に見えないだけなので残す
            "GUILD_DELETE" if d["unavailable"] != true => {
                if let Some(gid) = d["id"].as_str() {
                    self.remove_guild(gid);
                }
            }
            // 公式クライアントなど別の端末で既読にした
            "MESSAGE_ACK" => {
                if let Some(cid) = d["channel_id"].as_str() {
                    if d["manual"] != true {
                        self.unread.remove(cid);
                        if let Some(mid) = d["message_id"].as_str().and_then(|s| s.parse::<u64>().ok()) {
                            let entry = self.channel_read_rank.entry(cid.to_string()).or_insert((0, 0));
                            entry.1 = entry.1.max(mid);
                        }
                    }
                    match d["mention_count"].as_u64() {
                        Some(n) if n > 0 => {
                            if let Some(e) = self.mention_counts.get_mut(cid) {
                                e.1 = n as u32;
                            }
                        }
                        _ => {
                            self.mention_counts.remove(cid);
                        }
                    }
                }
            }
            // メンバーのロールやニックネームが変わった
            "GUILD_MEMBER_UPDATE" | "GUILD_MEMBER_ADD" => {
                let me = self.me.as_ref().map(|u| u.id.as_str());
                if me.is_some() && d["user"]["id"].as_str() == me {
                    if let Some(gid) = d["guild_id"].as_str() {
                        let roles = d["roles"].as_array().into_iter().flatten().filter_map(|r| r.as_str().map(String::from)).collect();
                        self.my_roles.insert(gid.to_string(), roles);
                    }
                }
                if let Some(gid) = d["guild_id"].as_str().map(String::from) {
                    if let Ok(member) = serde_json::from_value::<crate::model::Member>(d) {
                        if let Some(uid) = member.user.as_ref().map(|u| u.id.clone()) {
                            let list = self.members.entry(gid).or_default();
                            match list.iter_mut().find(|x| x.user.as_ref().is_some_and(|u| u.id == uid)) {
                                Some(slot) => *slot = member,
                                None => list.push(member),
                            }
                            self.member_ver += 1;
                        }
                    }
                }
            }
            _ => {}
        }
    }


    /// 他の人の新着メッセージ: メンション数を数え、必要ならデスクトップ通知を出す
    pub(super) fn on_new_message(&mut self, m: &Message) {
        let viewing = self.window_focused && self.sel_channel.as_deref() == Some(m.channel_id.as_str());
        let mentioned = self.mentions_me(m);
        // DM はすべてがメンション扱い（公式と同じ）
        if !viewing && (mentioned || m.guild_id.is_none()) {
            let e = self.mention_counts.entry(m.channel_id.clone()).or_insert((m.guild_id.clone(), 0));
            e.1 += 1;
        }
        let Some(gid) = m.guild_id.as_deref() else { return };
        if viewing {
            return;
        }
        let set = self.guild_settings.get(gid).cloned().unwrap_or_default();
        let wanted = match set.notifications {
            _ if set.muted => false,
            2 => false,
            0 => true,
            // 1 = @メンションのみ、3 = サーバーの既定（多くのサーバーはメンションのみ）
            _ => mentioned,
        };
        if !wanted {
            return;
        }
        let author = m.member.as_ref().and_then(|x| x.nick.as_deref()).unwrap_or_else(|| m.author.display()).to_string();
        let guild = self.guilds.iter().find(|g| g.id == gid).map(|g| g.name.as_str()).unwrap_or("");
        let channel = self.channels.get(gid).and_then(|cs| cs.iter().find(|c| c.id == m.channel_id)).and_then(|c| c.name.as_deref()).unwrap_or("");
        let title = match (channel.is_empty(), guild.is_empty()) {
            (false, false) => format!("{author}（#{channel}・{guild}）"),
            (true, false) => format!("{author}（{guild}）"),
            _ => author,
        };
        let roles = self.roles.get(gid).map(Vec::as_slice).unwrap_or_default();
        let mut body = crate::notify::excerpt(&resolve_mentions(&m.content, m, &self.channels, roles), 200);
        if body.is_empty() {
            body = if m.is_call() { "通話を開始しました".into() } else if !m.attachments.is_empty() { "添付ファイルを送信しました".into() } else if !m.sticker_items.is_empty() { "スタンプを送信しました".into() } else { "新しいメッセージ".into() };
        }
        crate::notify::show(title, body, (m.guild_id.clone(), m.channel_id.clone()), self.notify_clicked.clone(), self.waker.clone());
    }

}
