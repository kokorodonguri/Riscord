//! Discord の REST API をひと通り呼べるようにしたもの。
//!
//! 画面からは `Cmd::Api { id, call }` で頼み、結果は `Event::Api { id, call, result }` で返る。
//! id は頼んだ側が付ける番号（どの頼みの結果かを見分ける用。使わなければ 0 でよい）。
//! 本文や結果は Discord の JSON のまま（serde_json::Value）。型を付けたくなったら呼ぶ側で変換する。
//!
//! ユーザーアカウント向けの API は公式の文書がないものが多く、予告なく変わることがある
//! （参考: https://docs.discord.food/ ）。
#![allow(dead_code)]

use reqwest::Method;
use serde_json::{json, Value};

/// REST API の1回分の呼び出し
#[derive(Clone, Debug, PartialEq)]
pub enum ApiCall {
    // ---- 自分・ユーザー ----
    GetMe,
    /// username / global_name / avatar（data URI）/ banner など
    ModifyMe(Value),
    GetUser(String),
    GetProfile {
        user_id: String,
        guild_id: Option<String>,
    },
    /// bio / pronouns / accent_color / banner など
    ModifyProfile(Value),
    GetMyGuilds,
    LeaveGuild(String),
    /// muted / mute_config / message_notifications(0=すべて 1=@メンションのみ 2=なし) /
    /// suppress_everyone / suppress_roles / hide_muted_channels / flags など
    ModifyGuildSettings {
        guild_id: String,
        settings: Value,
    },
    /// チャンネルごとの muted / message_notifications など
    ModifyChannelSettings {
        guild_id: Option<String>,
        channel_id: String,
        settings: Value,
    },
    GetDms,
    /// 1人なら DM、2人以上ならグループ DM
    CreateDm(Vec<String>),
    GetRelationships,
    /// ユーザー名でフレンド申請
    SendFriendRequest {
        username: String,
    },
    /// フレンド申請の承認、または ID でフレンド申請
    AddFriend(String),
    Block(String),
    /// フレンド解除・申請の取り消し・ブロック解除
    RemoveRelationship(String),
    SetFriendNickname {
        user_id: String,
        nickname: Option<String>,
    },
    /// 1=ユーザー設定 2=表示まわり（中身は base64 の protobuf）
    GetSettingsProto(u8),
    ModifySettingsProto {
        kind: u8,
        settings: String,
    },
    GetConnections,
    GetNote(String),
    SetNote {
        user_id: String,
        note: String,
    },
    /// 受信トレイの「メンション」
    GetMentions {
        limit: u32,
        guild_id: Option<String>,
        before: Option<String>,
    },
    DeleteMention(String),

    // ---- メッセージ ----
    GetMessages {
        channel_id: String,
        before: Option<String>,
        after: Option<String>,
        around: Option<String>,
        limit: u32,
    },
    /// content / embeds / message_reference / sticker_ids / poll など
    SendMessage {
        channel_id: String,
        body: Value,
    },
    EditMessage {
        channel_id: String,
        message_id: String,
        body: Value,
    },
    DeleteMessage {
        channel_id: String,
        message_id: String,
    },
    AckMessage {
        channel_id: String,
        message_id: String,
    },
    /// (チャンネル, メッセージ) をまとめて既読にする
    AckBulk(Vec<(String, String)>),
    /// サーバーのチャンネルをすべて既読にする
    AckGuild(String),
    /// emoji は "👍" か "名前:ID"
    AddReaction {
        channel_id: String,
        message_id: String,
        emoji: String,
    },
    RemoveReaction {
        channel_id: String,
        message_id: String,
        emoji: String,
    },
    RemoveUserReaction {
        channel_id: String,
        message_id: String,
        emoji: String,
        user_id: String,
    },
    GetReactions {
        channel_id: String,
        message_id: String,
        emoji: String,
        limit: u32,
        after: Option<String>,
    },
    ClearReactions {
        channel_id: String,
        message_id: String,
    },
    ClearEmojiReactions {
        channel_id: String,
        message_id: String,
        emoji: String,
    },
    /// ピン留め一覧（新しい順に最大 limit 件。before は前のページの最後の pinned_at）
    GetChannelPins {
        channel_id: String,
        before: Option<String>,
        limit: u32,
    },
    Pin {
        channel_id: String,
        message_id: String,
    },
    Unpin {
        channel_id: String,
        message_id: String,
    },
    /// 「入力中…」を出す（10秒ほどで消える）
    Typing(String),
    /// query は ("content", "hello") や ("author_id", "…") などの組
    SearchGuild {
        guild_id: String,
        query: Vec<(String, String)>,
    },
    SearchChannel {
        channel_id: String,
        query: Vec<(String, String)>,
    },
    Crosspost {
        channel_id: String,
        message_id: String,
    },

    // ---- スレッド ----
    CreateThreadFromMessage {
        channel_id: String,
        message_id: String,
        body: Value,
    },
    CreateThread {
        channel_id: String,
        body: Value,
    },
    GetActiveThreads(String),
    GetArchivedThreads {
        channel_id: String,
        private: bool,
        before: Option<String>,
    },
    JoinThread(String),
    LeaveThread(String),
    GetThreadMembers(String),
    /// チャンネルのスレッド・フォーラムの投稿を探す（ユーザーで使える一覧はこれだけ）。
    /// query は archived / sort_by / sort_order / limit / offset / tag など
    SearchThreads {
        channel_id: String,
        query: Vec<(String, String)>,
    },

    // ---- 投票 ----
    GetPollVoters {
        channel_id: String,
        message_id: String,
        answer_id: u32,
    },
    /// 空にすると投票の取り消し
    VotePoll {
        channel_id: String,
        message_id: String,
        answer_ids: Vec<u32>,
    },
    EndPoll {
        channel_id: String,
        message_id: String,
    },

    // ---- チャンネル ----
    GetChannel(String),
    GetGuildChannels(String),
    CreateChannel {
        guild_id: String,
        body: Value,
    },
    ModifyChannel {
        channel_id: String,
        body: Value,
    },
    DeleteChannel(String),
    EditPermissions {
        channel_id: String,
        overwrite_id: String,
        body: Value,
    },
    DeletePermission {
        channel_id: String,
        overwrite_id: String,
    },
    /// max_age（秒、0=無期限）/ max_uses / temporary / unique など
    CreateInvite {
        channel_id: String,
        body: Value,
    },
    GetChannelInvites(String),
    /// recipients が None なら全員を呼ぶ
    Ring {
        channel_id: String,
        recipients: Option<Vec<String>>,
    },
    StopRinging(String),
    AddRecipient {
        channel_id: String,
        user_id: String,
    },
    RemoveRecipient {
        channel_id: String,
        user_id: String,
    },
    GetChannelWebhooks(String),
    CreateWebhook {
        channel_id: String,
        body: Value,
    },

    // ---- サーバー ----
    GetGuild(String),
    GetGuildPreview(String),
    ModifyGuild {
        guild_id: String,
        body: Value,
    },
    GetRoles(String),
    CreateRole {
        guild_id: String,
        body: Value,
    },
    ModifyRole {
        guild_id: String,
        role_id: String,
        body: Value,
    },
    DeleteRole {
        guild_id: String,
        role_id: String,
    },
    GetMember {
        guild_id: String,
        user_id: String,
    },
    SearchMembers {
        guild_id: String,
        query: String,
        limit: u32,
    },
    /// nick など
    ModifyMyMember {
        guild_id: String,
        body: Value,
    },
    /// サーバーごとのプロフィール（bio / pronouns / banner など）
    ModifyMyGuildProfile {
        guild_id: String,
        body: Value,
    },
    /// nick / roles / mute / deaf / channel_id（移動）/ communication_disabled_until（タイムアウト）など
    ModifyMember {
        guild_id: String,
        user_id: String,
        body: Value,
    },
    AddMemberRole {
        guild_id: String,
        user_id: String,
        role_id: String,
    },
    RemoveMemberRole {
        guild_id: String,
        user_id: String,
        role_id: String,
    },
    Kick {
        guild_id: String,
        user_id: String,
    },
    GetBans(String),
    Ban {
        guild_id: String,
        user_id: String,
        delete_message_seconds: u32,
    },
    Unban {
        guild_id: String,
        user_id: String,
    },
    GetEmojis(String),
    CreateEmoji {
        guild_id: String,
        body: Value,
    },
    DeleteEmoji {
        guild_id: String,
        emoji_id: String,
    },
    GetGuildStickers(String),
    GetAuditLog {
        guild_id: String,
        limit: u32,
        before: Option<String>,
    },
    GetGuildInvites(String),
    GetScheduledEvents(String),
    CreateScheduledEvent {
        guild_id: String,
        body: Value,
    },
    ModifyScheduledEvent {
        guild_id: String,
        event_id: String,
        body: Value,
    },
    DeleteScheduledEvent {
        guild_id: String,
        event_id: String,
    },
    /// イベントに「興味あり」
    SetEventInterested {
        guild_id: String,
        event_id: String,
        interested: bool,
    },
    GetOnboarding(String),
    GetGuildWebhooks(String),
    GetAutoModRules(String),
    CreateAutoModRule {
        guild_id: String,
        body: Value,
    },
    DeleteAutoModRule {
        guild_id: String,
        rule_id: String,
    },
    GetVoiceRegions,

    // ---- 招待 ----
    GetInvite(String),
    /// 招待から参加する（キャプチャを求められることがある）
    AcceptInvite(String),
    DeleteInvite(String),

    // ---- スタンプ・アプリ ----
    GetSticker(String),
    GetStickerPacks,
    /// サーバーで使えるスラッシュコマンド
    GetGuildCommands(String),
    /// DM で使えるスラッシュコマンド
    GetChannelCommands(String),
    /// 自分のアカウントに入れたアプリのスラッシュコマンド
    GetUserCommands,
    /// スラッシュコマンドの実行・ボタンの押下など（type / application_id / data / nonce / session_id …）
    Interaction(Value),

    // ---- その他 ----
    GetGateway,
    /// ここにないものを直接呼ぶ（path は /api/v10 より後ろ）
    Raw {
        method: Method,
        path: String,
        body: Option<Value>,
    },
}

/// URL の一部に入れられるようにする（絵文字・検索語など）
pub fn encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

fn query(pairs: &[(&str, Option<String>)]) -> String {
    let parts: Vec<String> = pairs
        .iter()
        .filter_map(|(k, v)| v.as_ref().map(|v| format!("{k}={}", encode(v))))
        .collect();
    if parts.is_empty() {
        String::new()
    } else {
        format!("?{}", parts.join("&"))
    }
}

fn query_list(pairs: &[(String, String)]) -> String {
    let owned: Vec<(&str, Option<String>)> = pairs
        .iter()
        .map(|(k, v)| (k.as_str(), Some(v.clone())))
        .collect();
    query(&owned)
}

impl ApiCall {
    /// (メソッド, パス, 本文)
    pub fn route(&self) -> (Method, String, Option<Value>) {
        use ApiCall::*;
        use Method as M;
        let get = |p: String| (M::GET, p, None);
        let del = |p: String| (M::DELETE, p, None);
        let post = |p: String, b: Value| (M::POST, p, Some(b));
        let put = |p: String, b: Value| (M::PUT, p, Some(b));
        let patch = |p: String, b: Value| (M::PATCH, p, Some(b));
        let some = |s: &Option<String>| s.clone();
        match self {
            GetMe => get("/users/@me".into()),
            ModifyMe(b) => patch("/users/@me".into(), b.clone()),
            GetUser(u) => get(format!("/users/{u}")),
            GetProfile { user_id, guild_id } => get(format!(
                "/users/{user_id}/profile{}",
                query(&[
                    ("with_mutual_guilds", Some("true".into())),
                    ("with_mutual_friends", Some("true".into())),
                    ("guild_id", some(guild_id))
                ])
            )),
            ModifyProfile(b) => patch("/users/@me/profile".into(), b.clone()),
            GetMyGuilds => get("/users/@me/guilds?with_counts=true".into()),
            LeaveGuild(g) => (
                M::DELETE,
                format!("/users/@me/guilds/{g}"),
                Some(json!({ "lurking": false })),
            ),
            ModifyGuildSettings { guild_id, settings } => patch(
                format!("/users/@me/guilds/{guild_id}/settings"),
                settings.clone(),
            ),
            ModifyChannelSettings {
                guild_id,
                channel_id,
                settings,
            } => patch(
                // DM の設定は "@me" に入れる
                format!(
                    "/users/@me/guilds/{}/settings",
                    guild_id.as_deref().unwrap_or("%40me")
                ),
                json!({ "channel_overrides": { channel_id.clone(): settings } }),
            ),
            GetDms => get("/users/@me/channels".into()),
            CreateDm(users) => post("/users/@me/channels".into(), json!({ "recipients": users })),
            GetRelationships => get("/users/@me/relationships".into()),
            SendFriendRequest { username } => post(
                "/users/@me/relationships".into(),
                json!({ "username": username, "discriminator": null }),
            ),
            AddFriend(u) => put(format!("/users/@me/relationships/{u}"), json!({})),
            Block(u) => put(
                format!("/users/@me/relationships/{u}"),
                json!({ "type": 2 }),
            ),
            RemoveRelationship(u) => del(format!("/users/@me/relationships/{u}")),
            SetFriendNickname { user_id, nickname } => patch(
                format!("/users/@me/relationships/{user_id}"),
                json!({ "nickname": nickname }),
            ),
            GetSettingsProto(k) => get(format!("/users/@me/settings-proto/{k}")),
            ModifySettingsProto { kind, settings } => patch(
                format!("/users/@me/settings-proto/{kind}"),
                json!({ "settings": settings }),
            ),
            GetConnections => get("/users/@me/connections".into()),
            GetNote(u) => get(format!("/users/@me/notes/{u}")),
            SetNote { user_id, note } => put(
                format!("/users/@me/notes/{user_id}"),
                json!({ "note": note }),
            ),
            GetMentions {
                limit,
                guild_id,
                before,
            } => get(format!(
                "/users/@me/mentions{}",
                query(&[
                    ("limit", Some(limit.to_string())),
                    ("roles", Some("true".into())),
                    ("everyone", Some("true".into())),
                    ("guild_id", some(guild_id)),
                    ("before", some(before)),
                ])
            )),
            DeleteMention(m) => del(format!("/users/@me/mentions/{m}")),

            GetMessages {
                channel_id,
                before,
                after,
                around,
                limit,
            } => get(format!(
                "/channels/{channel_id}/messages{}",
                query(&[
                    ("limit", Some(limit.to_string())),
                    ("before", some(before)),
                    ("after", some(after)),
                    ("around", some(around))
                ])
            )),
            SendMessage { channel_id, body } => {
                post(format!("/channels/{channel_id}/messages"), body.clone())
            }
            EditMessage {
                channel_id,
                message_id,
                body,
            } => patch(
                format!("/channels/{channel_id}/messages/{message_id}"),
                body.clone(),
            ),
            DeleteMessage {
                channel_id,
                message_id,
            } => del(format!("/channels/{channel_id}/messages/{message_id}")),
            AckMessage {
                channel_id,
                message_id,
            } => post(
                format!("/channels/{channel_id}/messages/{message_id}/ack"),
                json!({ "token": null }),
            ),
            AckBulk(list) => post(
                "/read-states/ack-bulk".into(),
                json!({ "read_states": list.iter().map(|(c, m)| json!({ "channel_id": c, "message_id": m, "read_state_type": 0 })).collect::<Vec<_>>() }),
            ),
            AckGuild(g) => post(format!("/guilds/{g}/ack"), json!({})),
            AddReaction {
                channel_id,
                message_id,
                emoji,
            } => put(
                format!(
                    "/channels/{channel_id}/messages/{message_id}/reactions/{}/@me",
                    encode(emoji)
                ),
                json!({}),
            ),
            RemoveReaction {
                channel_id,
                message_id,
                emoji,
            } => del(format!(
                "/channels/{channel_id}/messages/{message_id}/reactions/{}/@me",
                encode(emoji)
            )),
            RemoveUserReaction {
                channel_id,
                message_id,
                emoji,
                user_id,
            } => del(format!(
                "/channels/{channel_id}/messages/{message_id}/reactions/{}/{user_id}",
                encode(emoji)
            )),
            GetReactions {
                channel_id,
                message_id,
                emoji,
                limit,
                after,
            } => get(format!(
                "/channels/{channel_id}/messages/{message_id}/reactions/{}{}",
                encode(emoji),
                query(&[("limit", Some(limit.to_string())), ("after", some(after))])
            )),
            ClearReactions {
                channel_id,
                message_id,
            } => del(format!(
                "/channels/{channel_id}/messages/{message_id}/reactions"
            )),
            ClearEmojiReactions {
                channel_id,
                message_id,
                emoji,
            } => del(format!(
                "/channels/{channel_id}/messages/{message_id}/reactions/{}",
                encode(emoji)
            )),
            // /channels/{id}/pins は廃止予定。ページ送りできる /messages/pins を使う
            GetChannelPins {
                channel_id,
                before,
                limit,
            } => get(format!(
                "/channels/{channel_id}/messages/pins{}",
                query(&[("before", some(before)), ("limit", Some(limit.to_string()))])
            )),
            Pin {
                channel_id,
                message_id,
            } => put(
                format!("/channels/{channel_id}/messages/pins/{message_id}"),
                json!({}),
            ),
            Unpin {
                channel_id,
                message_id,
            } => del(format!("/channels/{channel_id}/messages/pins/{message_id}")),
            Typing(c) => post(format!("/channels/{c}/typing"), json!({})),
            SearchGuild { guild_id, query } => get(format!(
                "/guilds/{guild_id}/messages/search{}",
                query_list(query)
            )),
            SearchChannel { channel_id, query } => get(format!(
                "/channels/{channel_id}/messages/search{}",
                query_list(query)
            )),
            Crosspost {
                channel_id,
                message_id,
            } => post(
                format!("/channels/{channel_id}/messages/{message_id}/crosspost"),
                json!({}),
            ),

            CreateThreadFromMessage {
                channel_id,
                message_id,
                body,
            } => post(
                format!("/channels/{channel_id}/messages/{message_id}/threads"),
                body.clone(),
            ),
            CreateThread { channel_id, body } => {
                post(format!("/channels/{channel_id}/threads"), body.clone())
            }
            GetActiveThreads(g) => get(format!("/guilds/{g}/threads/active")),
            GetArchivedThreads {
                channel_id,
                private,
                before,
            } => get(format!(
                "/channels/{channel_id}/threads/archived/{}{}",
                if *private { "private" } else { "public" },
                query(&[("before", some(before))])
            )),
            JoinThread(c) => put(format!("/channels/{c}/thread-members/@me"), json!({})),
            LeaveThread(c) => del(format!("/channels/{c}/thread-members/@me")),
            GetThreadMembers(c) => get(format!("/channels/{c}/thread-members")),
            SearchThreads { channel_id, query } => get(format!(
                "/channels/{channel_id}/threads/search{}",
                query_list(query)
            )),

            GetPollVoters {
                channel_id,
                message_id,
                answer_id,
            } => get(format!(
                "/channels/{channel_id}/polls/{message_id}/answers/{answer_id}"
            )),
            VotePoll {
                channel_id,
                message_id,
                answer_ids,
            } => put(
                format!("/channels/{channel_id}/polls/{message_id}/answers/@me"),
                json!({ "answer_ids": answer_ids.iter().map(|a| a.to_string()).collect::<Vec<_>>() }),
            ),
            EndPoll {
                channel_id,
                message_id,
            } => post(
                format!("/channels/{channel_id}/polls/{message_id}/expire"),
                json!({}),
            ),

            GetChannel(c) => get(format!("/channels/{c}")),
            GetGuildChannels(g) => get(format!("/guilds/{g}/channels")),
            CreateChannel { guild_id, body } => {
                post(format!("/guilds/{guild_id}/channels"), body.clone())
            }
            ModifyChannel { channel_id, body } => {
                patch(format!("/channels/{channel_id}"), body.clone())
            }
            DeleteChannel(c) => del(format!("/channels/{c}")),
            EditPermissions {
                channel_id,
                overwrite_id,
                body,
            } => put(
                format!("/channels/{channel_id}/permissions/{overwrite_id}"),
                body.clone(),
            ),
            DeletePermission {
                channel_id,
                overwrite_id,
            } => del(format!("/channels/{channel_id}/permissions/{overwrite_id}")),
            CreateInvite { channel_id, body } => {
                post(format!("/channels/{channel_id}/invites"), body.clone())
            }
            GetChannelInvites(c) => get(format!("/channels/{c}/invites")),
            Ring {
                channel_id,
                recipients,
            } => post(
                format!("/channels/{channel_id}/call/ring"),
                json!({ "recipients": recipients }),
            ),
            StopRinging(c) => post(
                format!("/channels/{c}/call/stop-ringing"),
                json!({ "recipients": null }),
            ),
            AddRecipient {
                channel_id,
                user_id,
            } => put(
                format!("/channels/{channel_id}/recipients/{user_id}"),
                json!({}),
            ),
            RemoveRecipient {
                channel_id,
                user_id,
            } => del(format!("/channels/{channel_id}/recipients/{user_id}")),
            GetChannelWebhooks(c) => get(format!("/channels/{c}/webhooks")),
            CreateWebhook { channel_id, body } => {
                post(format!("/channels/{channel_id}/webhooks"), body.clone())
            }

            GetGuild(g) => get(format!("/guilds/{g}?with_counts=true")),
            GetGuildPreview(g) => get(format!("/guilds/{g}/preview")),
            ModifyGuild { guild_id, body } => patch(format!("/guilds/{guild_id}"), body.clone()),
            GetRoles(g) => get(format!("/guilds/{g}/roles")),
            CreateRole { guild_id, body } => {
                post(format!("/guilds/{guild_id}/roles"), body.clone())
            }
            ModifyRole {
                guild_id,
                role_id,
                body,
            } => patch(format!("/guilds/{guild_id}/roles/{role_id}"), body.clone()),
            DeleteRole { guild_id, role_id } => del(format!("/guilds/{guild_id}/roles/{role_id}")),
            GetMember { guild_id, user_id } => get(format!("/guilds/{guild_id}/members/{user_id}")),
            SearchMembers {
                guild_id,
                query: q,
                limit,
            } => get(format!(
                "/guilds/{guild_id}/members/search{}",
                query(&[
                    ("query", Some(q.clone())),
                    ("limit", Some(limit.to_string()))
                ])
            )),
            ModifyMyMember { guild_id, body } => {
                patch(format!("/guilds/{guild_id}/members/@me"), body.clone())
            }
            ModifyMyGuildProfile { guild_id, body } => {
                patch(format!("/guilds/{guild_id}/profile/@me"), body.clone())
            }
            ModifyMember {
                guild_id,
                user_id,
                body,
            } => patch(
                format!("/guilds/{guild_id}/members/{user_id}"),
                body.clone(),
            ),
            AddMemberRole {
                guild_id,
                user_id,
                role_id,
            } => put(
                format!("/guilds/{guild_id}/members/{user_id}/roles/{role_id}"),
                json!({}),
            ),
            RemoveMemberRole {
                guild_id,
                user_id,
                role_id,
            } => del(format!(
                "/guilds/{guild_id}/members/{user_id}/roles/{role_id}"
            )),
            Kick { guild_id, user_id } => del(format!("/guilds/{guild_id}/members/{user_id}")),
            GetBans(g) => get(format!("/guilds/{g}/bans")),
            Ban {
                guild_id,
                user_id,
                delete_message_seconds,
            } => put(
                format!("/guilds/{guild_id}/bans/{user_id}"),
                json!({ "delete_message_seconds": delete_message_seconds }),
            ),
            Unban { guild_id, user_id } => del(format!("/guilds/{guild_id}/bans/{user_id}")),
            GetEmojis(g) => get(format!("/guilds/{g}/emojis")),
            CreateEmoji { guild_id, body } => {
                post(format!("/guilds/{guild_id}/emojis"), body.clone())
            }
            DeleteEmoji { guild_id, emoji_id } => {
                del(format!("/guilds/{guild_id}/emojis/{emoji_id}"))
            }
            GetGuildStickers(g) => get(format!("/guilds/{g}/stickers")),
            GetAuditLog {
                guild_id,
                limit,
                before,
            } => get(format!(
                "/guilds/{guild_id}/audit-logs{}",
                query(&[("limit", Some(limit.to_string())), ("before", some(before))])
            )),
            GetGuildInvites(g) => get(format!("/guilds/{g}/invites")),
            GetScheduledEvents(g) => {
                get(format!("/guilds/{g}/scheduled-events?with_user_count=true"))
            }
            CreateScheduledEvent { guild_id, body } => {
                post(format!("/guilds/{guild_id}/scheduled-events"), body.clone())
            }
            ModifyScheduledEvent {
                guild_id,
                event_id,
                body,
            } => patch(
                format!("/guilds/{guild_id}/scheduled-events/{event_id}"),
                body.clone(),
            ),
            DeleteScheduledEvent { guild_id, event_id } => {
                del(format!("/guilds/{guild_id}/scheduled-events/{event_id}"))
            }
            SetEventInterested {
                guild_id,
                event_id,
                interested,
            } => {
                let p = format!("/guilds/{guild_id}/scheduled-events/{event_id}/users/@me");
                if *interested {
                    put(p, json!({}))
                } else {
                    del(p)
                }
            }
            GetOnboarding(g) => get(format!("/guilds/{g}/onboarding")),
            GetGuildWebhooks(g) => get(format!("/guilds/{g}/webhooks")),
            GetAutoModRules(g) => get(format!("/guilds/{g}/auto-moderation/rules")),
            CreateAutoModRule { guild_id, body } => post(
                format!("/guilds/{guild_id}/auto-moderation/rules"),
                body.clone(),
            ),
            DeleteAutoModRule { guild_id, rule_id } => del(format!(
                "/guilds/{guild_id}/auto-moderation/rules/{rule_id}"
            )),
            GetVoiceRegions => get("/voice/regions".into()),

            GetInvite(code) => get(format!(
                "/invites/{}?with_counts=true&with_expiration=true",
                encode(code)
            )),
            AcceptInvite(code) => post(format!("/invites/{}", encode(code)), json!({})),
            DeleteInvite(code) => del(format!("/invites/{}", encode(code))),

            GetSticker(s) => get(format!("/stickers/{s}")),
            GetStickerPacks => get("/sticker-packs".into()),
            GetGuildCommands(g) => get(format!("/guilds/{g}/application-command-index")),
            GetChannelCommands(c) => get(format!("/channels/{c}/application-command-index")),
            GetUserCommands => get("/users/@me/application-command-index".into()),
            Interaction(b) => post("/interactions".into(), b.clone()),

            GetGateway => get("/gateway".into()),
            Raw { method, path, body } => (method.clone(), path.clone(), body.clone()),
        }
    }
}

// ---- ゲートウェイに送るもの ----

/// op 8: メンバーを名前の前方一致か ID で取る（結果は GUILD_MEMBERS_CHUNK で届く）
pub fn request_guild_members(
    guild_id: &str,
    query: Option<&str>,
    user_ids: Option<&[String]>,
    limit: u32,
    presences: bool,
) -> Value {
    let mut d = json!({ "guild_id": [guild_id], "limit": limit, "presences": presences });
    match user_ids {
        Some(ids) => d["user_ids"] = json!(ids),
        None => d["query"] = json!(query.unwrap_or("")),
    }
    json!({ "op": 8, "d": d })
}

/// op 13: DM 通話の状態を問い合わせる（CALL_CREATE などが届く）
pub fn call_connect(channel_id: &str) -> Value {
    json!({ "op": 13, "d": { "channel_id": channel_id } })
}

/// op 18: 画面共有を始める（type は "guild" か "call"）
pub fn stream_create(kind: &str, guild_id: Option<&str>, channel_id: &str) -> Value {
    json!({ "op": 18, "d": { "type": kind, "guild_id": guild_id, "channel_id": channel_id, "preferred_region": null } })
}

/// op 19: 画面共有をやめる
pub fn stream_delete(stream_key: &str) -> Value {
    json!({ "op": 19, "d": { "stream_key": stream_key } })
}

/// op 20: 他の人の画面共有を見る
pub fn stream_watch(stream_key: &str) -> Value {
    json!({ "op": 20, "d": { "stream_key": stream_key } })
}

/// op 22: 画面共有を一時停止・再開
pub fn stream_set_paused(stream_key: &str, paused: bool) -> Value {
    json!({ "op": 22, "d": { "stream_key": stream_key, "paused": paused } })
}

/// 画面へそのまま渡すゲートウェイのイベント（解析は受け取った側で）。
/// dispatch で個別に扱っているもの（MESSAGE_CREATE など）はここに入れない
pub const FORWARDED_EVENTS: &[&str] = &[
    "MESSAGE_DELETE_BULK",
    "MESSAGE_REACTION_ADD",
    "MESSAGE_REACTION_ADD_MANY",
    "MESSAGE_REACTION_REMOVE",
    "MESSAGE_REACTION_REMOVE_ALL",
    "MESSAGE_REACTION_REMOVE_EMOJI",
    "MESSAGE_ACK",
    "MESSAGE_POLL_VOTE_ADD",
    "MESSAGE_POLL_VOTE_REMOVE",
    "TYPING_START",
    "GUILD_CREATE",
    "GUILD_UPDATE",
    "GUILD_DELETE",
    "GUILD_MEMBER_ADD",
    "GUILD_MEMBER_UPDATE",
    "GUILD_MEMBER_REMOVE",
    "GUILD_ROLE_CREATE",
    "GUILD_ROLE_UPDATE",
    "GUILD_ROLE_DELETE",
    "GUILD_EMOJIS_UPDATE",
    "GUILD_STICKERS_UPDATE",
    "GUILD_BAN_ADD",
    "GUILD_BAN_REMOVE",
    "GUILD_SCHEDULED_EVENT_CREATE",
    "GUILD_SCHEDULED_EVENT_UPDATE",
    "GUILD_SCHEDULED_EVENT_DELETE",
    "GUILD_SCHEDULED_EVENT_USER_ADD",
    "GUILD_SCHEDULED_EVENT_USER_REMOVE",
    "CHANNEL_CREATE",
    "CHANNEL_UPDATE",
    "CHANNEL_DELETE",
    "CHANNEL_PINS_UPDATE",
    "CHANNEL_PINS_ACK",
    "CHANNEL_RECIPIENT_ADD",
    "CHANNEL_RECIPIENT_REMOVE",
    "THREAD_CREATE",
    "THREAD_UPDATE",
    "THREAD_DELETE",
    "THREAD_LIST_SYNC",
    "THREAD_MEMBER_UPDATE",
    "THREAD_MEMBERS_UPDATE",
    "USER_UPDATE",
    "USER_NOTE_UPDATE",
    "USER_GUILD_SETTINGS_UPDATE",
    "USER_SETTINGS_PROTO_UPDATE",
    "RELATIONSHIP_ADD",
    "RELATIONSHIP_UPDATE",
    "RELATIONSHIP_REMOVE",
    "INVITE_CREATE",
    "INVITE_DELETE",
    "WEBHOOKS_UPDATE",
    "INTERACTION_CREATE",
    "INTERACTION_SUCCESS",
    "INTERACTION_FAILURE",
    "APPLICATION_COMMAND_AUTOCOMPLETE_RESPONSE",
    "STREAM_CREATE",
    "STREAM_SERVER_UPDATE",
    "STREAM_UPDATE",
    "STREAM_DELETE",
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn routes() {
        let (m, p, b) = ApiCall::AddReaction {
            channel_id: "1".into(),
            message_id: "2".into(),
            emoji: "👍".into(),
        }
        .route();
        assert_eq!(m, Method::PUT);
        assert_eq!(p, "/channels/1/messages/2/reactions/%F0%9F%91%8D/@me");
        assert_eq!(b, Some(json!({})));
        let (_, p, _) = ApiCall::GetMessages {
            channel_id: "5".into(),
            before: Some("9".into()),
            after: None,
            around: None,
            limit: 50,
        }
        .route();
        assert_eq!(p, "/channels/5/messages?limit=50&before=9");
        let (_, p, _) = ApiCall::SearchGuild {
            guild_id: "7".into(),
            query: vec![("content".into(), "a b&c".into())],
        }
        .route();
        assert_eq!(p, "/guilds/7/messages/search?content=a%20b%26c");
        let (_, p, _) = ApiCall::GetChannelPins {
            channel_id: "8".into(),
            before: Some("2026-01-01T00:00:00+00:00".into()),
            limit: 50,
        }
        .route();
        assert_eq!(
            p,
            "/channels/8/messages/pins?before=2026-01-01T00%3A00%3A00%2B00%3A00&limit=50"
        );
        let (m, p, _) = ApiCall::Pin {
            channel_id: "8".into(),
            message_id: "9".into(),
        }
        .route();
        assert_eq!(
            (m, p.as_str()),
            (Method::PUT, "/channels/8/messages/pins/9")
        );
        let (m, p, _) = ApiCall::Unpin {
            channel_id: "8".into(),
            message_id: "9".into(),
        }
        .route();
        assert_eq!(
            (m, p.as_str()),
            (Method::DELETE, "/channels/8/messages/pins/9")
        );
        let (_, p, _) = ApiCall::SearchThreads {
            channel_id: "8".into(),
            query: vec![
                ("archived".into(), "false".into()),
                ("tag".into(), "5".into()),
            ],
        }
        .route();
        assert_eq!(p, "/channels/8/threads/search?archived=false&tag=5");
        let (m, p, b) = ApiCall::ModifyGuildSettings {
            guild_id: "3".into(),
            settings: json!({ "muted": true }),
        }
        .route();
        assert_eq!(
            (m, p.as_str(), b),
            (
                Method::PATCH,
                "/users/@me/guilds/3/settings",
                Some(json!({ "muted": true }))
            )
        );
        let (_, p, _) = ApiCall::ModifyChannelSettings {
            guild_id: None,
            channel_id: "4".into(),
            settings: json!({}),
        }
        .route();
        assert_eq!(p, "/users/@me/guilds/%40me/settings");
    }

    #[test]
    fn forwarded_events_are_unique() {
        let mut v = FORWARDED_EVENTS.to_vec();
        v.sort();
        v.dedup();
        assert_eq!(v.len(), FORWARDED_EVENTS.len());
    }
}
