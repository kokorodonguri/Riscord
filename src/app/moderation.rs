//! サーバーの管理（監査ログ・BAN・招待・イベント・AutoMod・メンバーへの処分）。
//! 画面の出し分けは目安で、最終的な権限判定は Discord が行う。送信前にも入力と権限を確かめる。
use super::*;
use chrono::TimeZone;

const KICK_MEMBERS: u64 = 1 << 1;
const BAN_MEMBERS: u64 = 1 << 2;
const ADMINISTRATOR: u64 = 1 << 3;
const MANAGE_GUILD: u64 = 1 << 5;
const VIEW_AUDIT_LOG: u64 = 1 << 7;
const MANAGE_EVENTS: u64 = 1 << 33;
const MODERATE_MEMBERS: u64 = 1 << 40;
const CREATE_EVENTS: u64 = 1 << 44;
const AUDIT_PAGE: u32 = 50;
/// Discord のタイムアウトの上限（28 日）
const MAX_TIMEOUT_SECS: i64 = 28 * 24 * 3600;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModTab {
    Audit,
    Bans,
    Invites,
    Events,
    AutoMod,
}

impl ModTab {
    pub const ALL: [ModTab; 5] = [
        ModTab::Audit,
        ModTab::Bans,
        ModTab::Invites,
        ModTab::Events,
        ModTab::AutoMod,
    ];

    pub fn label(self) -> &'static str {
        match self {
            ModTab::Audit => "監査ログ",
            ModTab::Bans => "BAN",
            ModTab::Invites => "招待",
            ModTab::Events => "イベント",
            ModTab::AutoMod => "AutoMod",
        }
    }

    fn permission(self) -> u64 {
        match self {
            ModTab::Audit => VIEW_AUDIT_LOG,
            ModTab::Bans => BAN_MEMBERS,
            ModTab::Invites | ModTab::AutoMod => MANAGE_GUILD,
            ModTab::Events => MANAGE_EVENTS | CREATE_EVENTS,
        }
    }
}

/// 一覧の 1 行。種類ごとの操作に要るものだけ持つ
#[derive(Clone, Debug, PartialEq)]
pub struct ModRow {
    pub id: String,
    pub title: String,
    pub detail: String,
    /// AutoMod ルールの有効・無効
    pub enabled: Option<bool>,
    /// 編集できるイベントの中身
    pub event: Option<EventDraft>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct EventDraft {
    /// 編集中のイベント ID（新規は None）
    pub editing: Option<String>,
    pub name: String,
    /// ローカル時刻 "YYYY-MM-DD HH:MM"
    pub start: String,
    pub end: String,
    pub location: String,
    pub description: String,
    /// ボイスチャンネルで開くときのチャンネル
    pub channel_id: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct RuleDraft {
    pub name: String,
    /// カンマか改行区切りのキーワード
    pub keywords: String,
}

/// メンバーへの処分
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MemberAction {
    /// 秒数。0 で解除
    Timeout(i64),
    Kick,
    Ban,
}

impl MemberAction {
    fn permission(self) -> u64 {
        match self {
            MemberAction::Timeout(_) => MODERATE_MEMBERS,
            MemberAction::Kick => KICK_MEMBERS,
            MemberAction::Ban => BAN_MEMBERS,
        }
    }

    pub fn key(self, user_id: &str) -> String {
        match self {
            MemberAction::Timeout(s) => format!("timeout:{s}:{user_id}"),
            MemberAction::Kick => format!("kick:{user_id}"),
            MemberAction::Ban => format!("ban:{user_id}"),
        }
    }
}

pub struct ServerModeration {
    pub guild_id: String,
    pub tab: ModTab,
    pub rows: Option<Vec<ModRow>>,
    pub request: Option<u64>,
    /// 監査ログの続きを読むときの before
    pub before: Option<String>,
    pub has_more: bool,
    /// 送信中の変更（リクエスト ID と完了時に出す文）
    pub mutation: Option<(u64, &'static str)>,
    /// 確認待ちの操作のキー（もう一度押すと実行）
    pub confirm: Option<String>,
    pub error: Option<String>,
    pub event_draft: Option<EventDraft>,
    /// 編集前のイベント（変わった項目だけ送る）
    pub event_original: Option<EventDraft>,
    pub rule_draft: Option<RuleDraft>,
}

impl ServerModeration {
    fn new(guild_id: String, tab: ModTab) -> Self {
        Self {
            guild_id,
            tab,
            rows: None,
            request: None,
            before: None,
            has_more: false,
            mutation: None,
            confirm: None,
            error: None,
            event_draft: None,
            event_original: None,
            rule_draft: None,
        }
    }
}

fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 20
        && id.bytes().all(|b| b.is_ascii_digit())
        && id.parse::<u64>().is_ok_and(|n| n > 0)
}

fn valid_invite_code(code: &str) -> bool {
    !code.is_empty()
        && code.len() <= 64
        && code
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

fn user_name(u: &serde_json::Value) -> Option<String> {
    let name = u
        .get("global_name")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .or_else(|| u.get("username").and_then(|v| v.as_str()))?;
    Some(name.to_owned())
}

/// ISO 8601 をローカル時刻 "YYYY-MM-DD HH:MM" にする
pub fn local_time(iso: &str) -> Option<String> {
    let t = chrono::DateTime::parse_from_rfc3339(iso).ok()?;
    Some(
        t.with_timezone(&chrono::Local)
            .format("%Y-%m-%d %H:%M")
            .to_string(),
    )
}

/// "YYYY-MM-DD HH:MM"（ローカル時刻）を UTC にする
fn parse_local(s: &str) -> Option<chrono::DateTime<chrono::Utc>> {
    let n = chrono::NaiveDateTime::parse_from_str(s.trim(), "%Y-%m-%d %H:%M").ok()?;
    let local = chrono::Local.from_local_datetime(&n).earliest()?;
    Some(local.with_timezone(&chrono::Utc))
}

fn iso(t: chrono::DateTime<chrono::Utc>) -> String {
    t.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

fn audit_action(kind: u64) -> &'static str {
    match kind {
        1 => "サーバーを更新",
        10 => "チャンネルを作成",
        11 => "チャンネルを更新",
        12 => "チャンネルを削除",
        13..=15 => "チャンネル権限を変更",
        20 => "メンバーをキック",
        21 => "メンバーを整理",
        22 => "メンバーを BAN",
        23 => "BAN を解除",
        24 => "メンバーを更新",
        25 => "メンバーのロールを変更",
        26 => "メンバーを移動",
        27 => "メンバーを切断",
        28 => "ボットを追加",
        30 => "ロールを作成",
        31 => "ロールを更新",
        32 => "ロールを削除",
        40 => "招待を作成",
        41 => "招待を更新",
        42 => "招待を削除",
        50..=52 => "Webhook を変更",
        60..=62 => "絵文字を変更",
        72 => "メッセージを削除",
        73 => "メッセージを一括削除",
        74 => "メッセージをピン留め",
        75 => "ピン留めを解除",
        80..=82 => "連携を変更",
        83..=85 => "ステージを変更",
        90..=92 => "スタンプを変更",
        100 => "イベントを作成",
        101 => "イベントを更新",
        102 => "イベントを削除",
        110..=112 => "スレッドを変更",
        121 => "コマンドの権限を変更",
        130..=132 => "サウンドボードを変更",
        140 => "AutoMod ルールを作成",
        141 => "AutoMod ルールを更新",
        142 => "AutoMod ルールを削除",
        143..=145 => "AutoMod がブロック",
        _ => "その他の操作",
    }
}

fn parse_bans(v: &serde_json::Value) -> Option<Vec<ModRow>> {
    let mut out = Vec::new();
    for item in v.as_array()? {
        let user = item.get("user")?;
        let id = user.get("id")?.as_str()?;
        if !valid_id(id) {
            return None;
        }
        let reason = item
            .get("reason")
            .and_then(|r| r.as_str())
            .filter(|r| !r.is_empty())
            .unwrap_or("理由なし");
        out.push(ModRow {
            id: id.to_owned(),
            title: user_name(user).unwrap_or_else(|| id.to_owned()),
            detail: reason.to_owned(),
            enabled: None,
            event: None,
        });
    }
    Some(out)
}

fn parse_invites(v: &serde_json::Value) -> Option<Vec<ModRow>> {
    let mut out = Vec::new();
    for item in v.as_array()? {
        let code = item.get("code")?.as_str()?;
        if !valid_invite_code(code) {
            return None;
        }
        let mut detail = Vec::new();
        if let Some(c) = item
            .get("channel")
            .and_then(|c| c.get("name"))
            .and_then(|n| n.as_str())
        {
            detail.push(format!("#{c}"));
        }
        let uses = item.get("uses").and_then(|u| u.as_u64()).unwrap_or(0);
        match item.get("max_uses").and_then(|u| u.as_u64()) {
            Some(m) if m > 0 => detail.push(format!("使用 {uses}/{m}")),
            _ => detail.push(format!("使用 {uses}")),
        }
        if let Some(name) = item.get("inviter").and_then(user_name) {
            detail.push(format!("作成: {name}"));
        }
        match item
            .get("expires_at")
            .and_then(|e| e.as_str())
            .and_then(local_time)
        {
            Some(t) => detail.push(format!("期限 {t}")),
            None => detail.push("無期限".into()),
        }
        out.push(ModRow {
            id: code.to_owned(),
            title: format!("discord.gg/{code}"),
            detail: detail.join(" · "),
            enabled: None,
            event: None,
        });
    }
    Some(out)
}

/// 監査ログ。行と、続きがありそうか
fn parse_audit(v: &serde_json::Value) -> Option<(Vec<ModRow>, bool)> {
    let entries = v.get("audit_log_entries")?.as_array()?;
    let users: HashMap<&str, String> = v
        .get("users")
        .and_then(|u| u.as_array())
        .into_iter()
        .flatten()
        .filter_map(|u| Some((u.get("id")?.as_str()?, user_name(u)?)))
        .collect();
    let mut out = Vec::with_capacity(entries.len());
    for e in entries {
        let id = e.get("id")?.as_str()?;
        if !valid_id(id) {
            return None;
        }
        let actor = e
            .get("user_id")
            .and_then(|u| u.as_str())
            .map(|u| users.get(u).cloned().unwrap_or_else(|| u.to_owned()))
            .unwrap_or_else(|| "システム".into());
        let action = audit_action(e.get("action_type")?.as_u64()?);
        let mut detail = Vec::new();
        if let Some(t) = e.get("target_id").and_then(|t| t.as_str()) {
            detail.push(format!(
                "対象: {}",
                users.get(t).cloned().unwrap_or_else(|| t.to_owned())
            ));
        }
        if let Some(r) = e
            .get("reason")
            .and_then(|r| r.as_str())
            .filter(|r| !r.is_empty())
        {
            detail.push(format!("理由: {r}"));
        }
        if let Ok(n) = id.parse::<u64>() {
            let ms = (n >> 22) as i64 + 1_420_070_400_000;
            if let Some(t) = chrono::DateTime::from_timestamp_millis(ms) {
                detail.push(
                    t.with_timezone(&chrono::Local)
                        .format("%Y-%m-%d %H:%M")
                        .to_string(),
                );
            }
        }
        out.push(ModRow {
            id: id.to_owned(),
            title: format!("{actor} が{action}"),
            detail: detail.join(" · "),
            enabled: None,
            event: None,
        });
    }
    let more = entries.len() as u32 >= AUDIT_PAGE;
    Some((out, more))
}

fn parse_events(v: &serde_json::Value) -> Option<Vec<ModRow>> {
    let mut out = Vec::new();
    for e in v.as_array()? {
        let id = e.get("id")?.as_str()?;
        if !valid_id(id) {
            return None;
        }
        let name = e.get("name")?.as_str()?.to_owned();
        let start = local_time(e.get("scheduled_start_time")?.as_str()?)?;
        let end = e
            .get("scheduled_end_time")
            .and_then(|t| t.as_str())
            .and_then(local_time)
            .unwrap_or_default();
        let location = e
            .get("entity_metadata")
            .and_then(|m| m.get("location"))
            .and_then(|l| l.as_str())
            .unwrap_or("")
            .to_owned();
        let channel_id = e
            .get("channel_id")
            .and_then(|c| c.as_str())
            .filter(|c| valid_id(c))
            .map(str::to_owned);
        let status = match e.get("status").and_then(|s| s.as_u64()) {
            Some(2) => "開催中",
            Some(3) => "終了",
            Some(4) => "中止",
            _ => "予定",
        };
        let mut detail = vec![status.to_owned(), start.clone()];
        if !location.is_empty() {
            detail.push(location.clone());
        }
        if let Some(n) = e.get("user_count").and_then(|n| n.as_u64()) {
            detail.push(format!("{n} 人が興味あり"));
        }
        let description = e
            .get("description")
            .and_then(|d| d.as_str())
            .unwrap_or("")
            .to_owned();
        // 予定のものだけ編集できる（開催中・終了後は Discord が内容の変更を受け付けない）
        let editable = matches!(e.get("status").and_then(|s| s.as_u64()), Some(1) | None);
        out.push(ModRow {
            id: id.to_owned(),
            title: name.clone(),
            detail: detail.join(" · "),
            enabled: None,
            event: editable.then(|| EventDraft {
                editing: Some(id.to_owned()),
                name,
                start,
                end,
                location,
                description,
                channel_id,
            }),
        });
    }
    Some(out)
}

fn parse_rules(v: &serde_json::Value) -> Option<Vec<ModRow>> {
    let mut out = Vec::new();
    for r in v.as_array()? {
        let id = r.get("id")?.as_str()?;
        if !valid_id(id) {
            return None;
        }
        let enabled = r.get("enabled")?.as_bool()?;
        let trigger = match r.get("trigger_type").and_then(|t| t.as_u64()) {
            Some(1) => {
                let n = r
                    .get("trigger_metadata")
                    .and_then(|m| m.get("keyword_filter"))
                    .and_then(|k| k.as_array())
                    .map_or(0, Vec::len);
                format!("キーワード {n} 件")
            }
            Some(3) => "スパム".into(),
            Some(4) => "キーワードのプリセット".into(),
            Some(5) => "メンションのスパム".into(),
            Some(6) => "メンバーのプロフィール".into(),
            _ => "その他".into(),
        };
        out.push(ModRow {
            id: id.to_owned(),
            title: r.get("name")?.as_str()?.to_owned(),
            detail: format!("{trigger} · {}", if enabled { "有効" } else { "無効" }),
            enabled: Some(enabled),
            event: None,
        });
    }
    Some(out)
}

/// イベントの下書きを検証し、送る本体を作る。編集では変わった項目だけ入れる
pub fn event_body(
    draft: &EventDraft,
    original: Option<&EventDraft>,
    voice_channels: &[String],
    now: chrono::DateTime<chrono::Utc>,
) -> Result<serde_json::Value, &'static str> {
    let name = draft.name.trim();
    if name.is_empty() || name.chars().count() > 100 {
        return Err("イベント名は 1〜100 文字にしてください");
    }
    if draft.description.chars().count() > 1000 {
        return Err("説明は 1000 文字までです");
    }
    let start =
        parse_local(&draft.start).ok_or("開始日時は YYYY-MM-DD HH:MM で入力してください")?;
    let end = match draft.end.trim() {
        "" => None,
        s => Some(parse_local(s).ok_or("終了日時は YYYY-MM-DD HH:MM で入力してください")?),
    };
    let start_changed = original.is_none_or(|o| o.start.trim() != draft.start.trim());
    if start_changed && start <= now {
        return Err("開始日時は未来にしてください");
    }
    if end.is_some_and(|e| e <= start) {
        return Err("終了日時は開始より後にしてください");
    }
    let location = draft.location.trim();
    let voice = match &draft.channel_id {
        Some(c) if voice_channels.contains(c) => Some(c.clone()),
        Some(_) => return Err("ボイスチャンネルが見つかりません"),
        None => None,
    };
    if voice.is_none() {
        if location.is_empty() || location.chars().count() > 100 {
            return Err("場所（1〜100 文字）かボイスチャンネルを選んでください");
        }
        if end.is_none() {
            return Err("場所を指定するイベントには終了日時が必要です");
        }
    }
    let mut body = serde_json::Map::new();
    let mut put = |k: &str, v: serde_json::Value| {
        body.insert(k.to_owned(), v);
    };
    let changed =
        |f: fn(&EventDraft) -> &str| original.is_none_or(|o| f(o).trim() != f(draft).trim());
    if changed(|d| &d.name) {
        put("name", name.into());
    }
    if changed(|d| &d.description) {
        put("description", draft.description.trim().into());
    }
    if start_changed {
        put("scheduled_start_time", iso(start).into());
    }
    if changed(|d| &d.end) {
        put(
            "scheduled_end_time",
            end.map_or(serde_json::Value::Null, |e| iso(e).into()),
        );
    }
    let entity_changed =
        original.is_none_or(|o| o.channel_id != draft.channel_id || o.location.trim() != location);
    if entity_changed {
        match voice {
            Some(c) => {
                put("entity_type", 2.into());
                put("channel_id", c.into());
                put("entity_metadata", serde_json::Value::Null);
            }
            None => {
                put("entity_type", 3.into());
                put("channel_id", serde_json::Value::Null);
                put(
                    "entity_metadata",
                    serde_json::json!({ "location": location }),
                );
                // 場所のイベントは終了日時が必須なので、変わっていなくても一緒に送る
                if let Some(e) = end {
                    put("scheduled_end_time", iso(e).into());
                }
            }
        }
    }
    if original.is_none() {
        put("privacy_level", 2.into());
    }
    if body.is_empty() {
        return Err("変更がありません");
    }
    Ok(serde_json::Value::Object(body))
}

/// キーワードの AutoMod ルールを作る本体
pub fn rule_body(draft: &RuleDraft) -> Result<serde_json::Value, &'static str> {
    let name = draft.name.trim();
    if name.is_empty() || name.chars().count() > 100 {
        return Err("ルール名は 1〜100 文字にしてください");
    }
    let mut keywords: Vec<&str> = Vec::new();
    for k in draft.keywords.split([',', '、', '\n']).map(str::trim) {
        if k.is_empty() || keywords.contains(&k) {
            continue;
        }
        if k.chars().count() > 60 {
            return Err("キーワードは 1 つ 60 文字までです");
        }
        keywords.push(k);
    }
    if keywords.is_empty() {
        return Err("キーワードを 1 つ以上入力してください");
    }
    if keywords.len() > 1000 {
        return Err("キーワードは 1000 件までです");
    }
    Ok(serde_json::json!({
        "name": name,
        "event_type": 1,
        "trigger_type": 1,
        "trigger_metadata": { "keyword_filter": keywords },
        "actions": [{ "type": 1 }],
        "enabled": true,
    }))
}

impl App {
    /// このサーバーでの自分の権限。分からないときは None
    pub fn guild_perms(&self, guild_id: &str) -> Option<u64> {
        let g = self.guilds.iter().find(|g| g.id == guild_id)?;
        if g.owner {
            return Some(u64::MAX);
        }
        let p = match g.permissions.as_deref().and_then(|p| p.parse().ok()) {
            Some(p) => p,
            None => model::base_permissions(
                guild_id,
                self.roles.get(guild_id)?,
                self.my_roles.get(guild_id)?,
            ),
        };
        Some(if p & ADMINISTRATOR != 0 { u64::MAX } else { p })
    }

    fn may(&self, guild_id: &str, bits: u64) -> bool {
        self.guild_perms(guild_id).is_none_or(|p| p & bits != 0)
    }

    /// サーバー管理画面のこのタブを出すか（分からないときは出しておき、Discord の判定に任せる）
    pub fn can_open_mod_tab(&self, guild_id: &str, tab: ModTab) -> bool {
        self.may(guild_id, tab.permission())
    }

    pub fn can_member_action(&self, guild_id: &str, action: MemberAction) -> bool {
        self.may(guild_id, action.permission())
    }

    /// イベントの作成画面を開く（サーバーのメニューの「イベントを作成」）
    pub fn open_event_create(&mut self, guild_id: &str) {
        if !self.can_open_mod_tab(guild_id, ModTab::Events) {
            return;
        }
        self.open_moderation_tab(guild_id, ModTab::Events);
        self.start_event_draft(None);
    }

    /// 指定したタブで開く（ほかのタブの一覧は取りに行かない）
    pub fn open_moderation_tab(&mut self, guild_id: &str, tab: ModTab) {
        if !self.guilds.iter().any(|g| g.id == guild_id) || !self.can_open_mod_tab(guild_id, tab) {
            return;
        }
        self.moderation = Some(ServerModeration::new(guild_id.to_owned(), tab));
        self.reload_moderation();
    }

    pub fn close_moderation(&mut self) {
        if self
            .moderation
            .as_ref()
            .is_some_and(|m| m.mutation.is_none())
        {
            self.moderation = None;
        }
    }

    pub fn set_mod_tab(&mut self, tab: ModTab) {
        let Some(m) = self.moderation.as_ref() else {
            return;
        };
        if m.tab == tab || m.mutation.is_some() || !self.can_open_mod_tab(&m.guild_id, tab) {
            return;
        }
        let m = self.moderation.as_mut().unwrap();
        m.tab = tab;
        m.event_draft = None;
        m.event_original = None;
        m.rule_draft = None;
        self.reload_moderation();
    }

    pub fn reload_moderation(&mut self) {
        let Some(m) = self.moderation.as_mut() else {
            return;
        };
        m.rows = None;
        m.before = None;
        m.has_more = false;
        m.confirm = None;
        m.error = None;
        self.fetch_moderation();
    }

    /// 監査ログの続きを読む
    pub fn more_audit_log(&mut self) {
        if self
            .moderation
            .as_ref()
            .is_some_and(|m| m.tab == ModTab::Audit && m.has_more && m.request.is_none())
        {
            self.fetch_moderation();
        }
    }

    fn fetch_moderation(&mut self) {
        let Some(m) = self.moderation.as_ref() else {
            return;
        };
        let g = m.guild_id.clone();
        let call = match m.tab {
            ModTab::Audit => net::ApiCall::GetAuditLog {
                guild_id: g,
                limit: AUDIT_PAGE,
                before: m.before.clone(),
            },
            ModTab::Bans => net::ApiCall::GetBans(g),
            ModTab::Invites => net::ApiCall::GetGuildInvites(g),
            ModTab::Events => net::ApiCall::GetScheduledEvents(g),
            ModTab::AutoMod => net::ApiCall::GetAutoModRules(g),
        };
        let id = self.role_queue(call);
        let m = self.moderation.as_mut().unwrap();
        m.request = id;
        if id.is_none() {
            m.error = Some("読み込めませんでした。再接続してください".into());
        }
    }

    /// 消すなど取り消せない操作は、1 回目で確認を出し、2 回目で実行する
    fn confirmed(&mut self, key: String) -> bool {
        let Some(m) = self.moderation.as_mut() else {
            return false;
        };
        if m.confirm.as_deref() == Some(key.as_str()) {
            m.confirm = None;
            true
        } else {
            m.confirm = Some(key);
            false
        }
    }

    fn mutate(&mut self, call: net::ApiCall, done: &'static str) {
        let Some(m) = self.moderation.as_ref() else {
            return;
        };
        if m.mutation.is_some() || m.request.is_some() {
            return;
        }
        let id = self.role_queue(call);
        let m = self.moderation.as_mut().unwrap();
        m.error = None;
        match id {
            Some(id) => m.mutation = Some((id, done)),
            None => m.error = Some("送信できませんでした。再接続してください".into()),
        }
    }

    fn row_exists(&self, id: &str) -> bool {
        self.moderation
            .as_ref()
            .and_then(|m| m.rows.as_ref())
            .is_some_and(|r| r.iter().any(|r| r.id == id))
    }

    /// BAN 解除・招待の削除・イベントの削除・ルールの削除（タブで決まる）
    pub fn delete_mod_row(&mut self, id: &str) {
        let Some(m) = self.moderation.as_ref() else {
            return;
        };
        let (g, tab) = (m.guild_id.clone(), m.tab);
        if !self.row_exists(id) || !self.can_open_mod_tab(&g, tab) {
            return;
        }
        let (call, done) = match tab {
            ModTab::Bans => (
                net::ApiCall::Unban {
                    guild_id: g,
                    user_id: id.to_owned(),
                },
                "BAN を解除しました",
            ),
            ModTab::Invites => (
                net::ApiCall::DeleteInvite(id.to_owned()),
                "招待を削除しました",
            ),
            ModTab::Events => (
                net::ApiCall::DeleteScheduledEvent {
                    guild_id: g,
                    event_id: id.to_owned(),
                },
                "イベントを削除しました",
            ),
            ModTab::AutoMod => (
                net::ApiCall::DeleteAutoModRule {
                    guild_id: g,
                    rule_id: id.to_owned(),
                },
                "ルールを削除しました",
            ),
            ModTab::Audit => return,
        };
        if self.confirmed(format!("delete:{id}")) {
            self.mutate(call, done);
        }
    }

    pub fn toggle_rule(&mut self, id: &str) {
        let Some(m) = self.moderation.as_ref() else {
            return;
        };
        let Some(enabled) = m
            .rows
            .as_ref()
            .and_then(|r| r.iter().find(|r| r.id == id))
            .and_then(|r| r.enabled)
        else {
            return;
        };
        if m.tab != ModTab::AutoMod || !self.can_open_mod_tab(&m.guild_id, ModTab::AutoMod) {
            return;
        }
        let call = net::ApiCall::ModifyAutoModRule {
            guild_id: m.guild_id.clone(),
            rule_id: id.to_owned(),
            body: serde_json::json!({ "enabled": !enabled }),
        };
        self.mutate(
            call,
            if enabled {
                "ルールを無効にしました"
            } else {
                "ルールを有効にしました"
            },
        );
    }

    /// サーバーのボイス・ステージチャンネル
    pub fn mod_voice_channels(&self) -> Vec<(String, String)> {
        let Some(m) = self.moderation.as_ref() else {
            return Vec::new();
        };
        self.channels
            .get(&m.guild_id)
            .into_iter()
            .flatten()
            .filter(|c| c.kind == 2 || c.kind == 13)
            .map(|c| (c.id.clone(), c.name.clone().unwrap_or_default()))
            .collect()
    }

    /// イベントの作成（None）か編集（Some）を始める
    pub fn start_event_draft(&mut self, id: Option<&str>) {
        let Some(m) = self.moderation.as_mut() else {
            return;
        };
        if m.mutation.is_some() {
            return;
        }
        let original = match id {
            Some(id) => {
                let Some(e) = m
                    .rows
                    .as_ref()
                    .and_then(|r| r.iter().find(|r| r.id == id))
                    .and_then(|r| r.event.clone())
                else {
                    return;
                };
                Some(e)
            }
            None => None,
        };
        m.event_draft = Some(original.clone().unwrap_or_else(|| {
            let start = chrono::Local::now() + chrono::Duration::hours(1);
            EventDraft {
                start: start.format("%Y-%m-%d %H:00").to_string(),
                end: (start + chrono::Duration::hours(1))
                    .format("%Y-%m-%d %H:00")
                    .to_string(),
                ..Default::default()
            }
        }));
        m.event_original = original;
        m.error = None;
    }

    pub fn save_event(&mut self) {
        let Some(m) = self.moderation.as_ref() else {
            return;
        };
        let Some(draft) = m.event_draft.clone() else {
            return;
        };
        if !self.can_open_mod_tab(&m.guild_id, ModTab::Events) {
            return;
        }
        let voice: Vec<String> = self
            .mod_voice_channels()
            .into_iter()
            .map(|(id, _)| id)
            .collect();
        let m = self.moderation.as_ref().unwrap();
        let body = match event_body(
            &draft,
            m.event_original.as_ref(),
            &voice,
            chrono::Utc::now(),
        ) {
            Ok(b) => b,
            Err(e) => {
                self.moderation.as_mut().unwrap().error = Some(e.into());
                return;
            }
        };
        let guild_id = m.guild_id.clone();
        let (call, done) = match draft.editing {
            Some(event_id) => (
                net::ApiCall::ModifyScheduledEvent {
                    guild_id,
                    event_id,
                    body,
                },
                "イベントを更新しました",
            ),
            None => (
                net::ApiCall::CreateScheduledEvent { guild_id, body },
                "イベントを作成しました",
            ),
        };
        self.mutate(call, done);
    }

    pub fn save_rule(&mut self) {
        let Some(m) = self.moderation.as_ref() else {
            return;
        };
        let Some(draft) = m.rule_draft.as_ref() else {
            return;
        };
        if !self.can_open_mod_tab(&m.guild_id, ModTab::AutoMod) {
            return;
        }
        match rule_body(draft) {
            Ok(body) => {
                let call = net::ApiCall::CreateAutoModRule {
                    guild_id: m.guild_id.clone(),
                    body,
                };
                self.mutate(call, "ルールを作成しました");
            }
            Err(e) => self.moderation.as_mut().unwrap().error = Some(e.into()),
        }
    }

    /// メンバーへの処分。1 回目で確認を求め、2 回目で送る。結果はトーストで知らせる
    pub fn member_action(&mut self, guild_id: &str, user_id: &str, action: MemberAction) -> bool {
        if !valid_id(user_id)
            || !self.guilds.iter().any(|g| g.id == guild_id)
            || self.me.as_ref().is_some_and(|me| me.id == user_id)
            || !self.can_member_action(guild_id, action)
            || self.member_action_pending.is_some()
        {
            return false;
        }
        let key = action.key(user_id);
        if self.member_action_confirm.as_deref() != Some(key.as_str()) {
            self.member_action_confirm = Some(key);
            return false;
        }
        self.member_action_confirm = None;
        let (call, done) = match action {
            MemberAction::Timeout(secs) => {
                let until = if secs <= 0 {
                    serde_json::Value::Null
                } else {
                    let secs = secs.min(MAX_TIMEOUT_SECS);
                    iso(chrono::Utc::now() + chrono::Duration::seconds(secs)).into()
                };
                (
                    net::ApiCall::ModifyMember {
                        guild_id: guild_id.to_owned(),
                        user_id: user_id.to_owned(),
                        body: serde_json::json!({ "communication_disabled_until": until }),
                    },
                    if secs <= 0 {
                        "タイムアウトを解除しました"
                    } else {
                        "タイムアウトしました"
                    },
                )
            }
            MemberAction::Kick => (
                net::ApiCall::Kick {
                    guild_id: guild_id.to_owned(),
                    user_id: user_id.to_owned(),
                },
                "キックしました",
            ),
            MemberAction::Ban => (
                net::ApiCall::Ban {
                    guild_id: guild_id.to_owned(),
                    user_id: user_id.to_owned(),
                    delete_message_seconds: 0,
                },
                "BAN しました",
            ),
        };
        match self.role_queue(call) {
            Some(id) => {
                self.member_action_pending = Some((id, done));
                true
            }
            None => {
                self.show_toast("送信できませんでした。再接続してください");
                false
            }
        }
    }

    pub fn is_moderation_request(&self, id: u64) -> bool {
        self.member_action_pending.is_some_and(|(n, _)| n == id)
            || self
                .moderation
                .as_ref()
                .is_some_and(|m| m.request == Some(id) || m.mutation.is_some_and(|(n, _)| n == id))
    }

    pub fn moderation_result(&mut self, id: u64, result: Result<serde_json::Value, String>) {
        if let Some((_, done)) = self.member_action_pending.filter(|(n, _)| *n == id) {
            self.member_action_pending = None;
            match result {
                Ok(_) => self.show_toast(done),
                Err(e) => {
                    let text = super::pins::api_error_text(&e, "処分できませんでした");
                    self.show_toast(&text);
                }
            }
            return;
        }
        let Some(m) = self.moderation.as_mut() else {
            return;
        };
        if m.request == Some(id) {
            m.request = None;
            let value = match result {
                Ok(v) => v,
                Err(e) => {
                    m.error = Some(super::pins::api_error_text(&e, "読み込めませんでした"));
                    return;
                }
            };
            let parsed = match m.tab {
                ModTab::Audit => parse_audit(&value).map(|(rows, more)| {
                    m.has_more = more;
                    m.before = rows.last().map(|r| r.id.clone());
                    rows
                }),
                ModTab::Bans => parse_bans(&value),
                ModTab::Invites => parse_invites(&value),
                ModTab::Events => parse_events(&value),
                ModTab::AutoMod => parse_rules(&value),
            };
            match parsed {
                Some(rows) => match (&mut m.rows, m.tab) {
                    (Some(old), ModTab::Audit) => old.extend(rows),
                    (slot, _) => *slot = Some(rows),
                },
                None => {
                    m.has_more = false;
                    m.error = Some("応答を読み取れませんでした".into());
                }
            }
            return;
        }
        if let Some((_, done)) = m.mutation.filter(|(n, _)| *n == id) {
            m.mutation = None;
            match result {
                Ok(_) => {
                    m.event_draft = None;
                    m.event_original = None;
                    m.rule_draft = None;
                    self.show_toast(done);
                    self.reload_moderation();
                }
                // 下書きは残して、直してもう一度送れるようにする
                Err(e) => m.error = Some(super::pins::api_error_text(&e, "変更できませんでした")),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::mpsc::UnboundedReceiver;

    fn app(permissions: u64) -> (App, UnboundedReceiver<Cmd>) {
        let mut app = App::default();
        app.me =
            Some(serde_json::from_value(serde_json::json!({"id":"10","username":"me"})).unwrap());
        app.guilds = vec![serde_json::from_value(serde_json::json!({
            "id":"1","name":"guild","permissions":permissions.to_string()
        }))
        .unwrap()];
        let (_, ev) = std::sync::mpsc::channel();
        let (cmd, rx) = tokio::sync::mpsc::unbounded_channel();
        app.backend = Some(Backend { cmd, ev });
        (app, rx)
    }

    fn next(rx: &mut UnboundedReceiver<Cmd>) -> (u64, net::ApiCall) {
        match rx.try_recv().unwrap() {
            Cmd::Api { id, call } => (id, call),
            _ => panic!("expected API"),
        }
    }

    #[test]
    fn tabs_follow_permissions() {
        let (mut app, mut rx) = app(BAN_MEMBERS);
        assert!(!app.can_open_mod_tab("1", ModTab::Audit));
        app.open_moderation_tab("1", ModTab::Audit);
        assert!(app.moderation.is_none(), "権限のないタブでは開かない");
        app.open_moderation_tab("1", ModTab::Bans);
        assert_eq!(app.moderation.as_ref().unwrap().tab, ModTab::Bans);
        assert!(matches!(next(&mut rx).1, net::ApiCall::GetBans(_)));
        // 権限のないタブには切り替えない
        app.set_mod_tab(ModTab::Audit);
        assert_eq!(app.moderation.as_ref().unwrap().tab, ModTab::Bans);
        let (none, _) = self::app(0);
        assert!(ModTab::ALL.iter().all(|t| !none.can_open_mod_tab("1", *t)));
        let (admin, _) = self::app(ADMINISTRATOR);
        assert!(ModTab::ALL.iter().all(|t| admin.can_open_mod_tab("1", *t)));
    }

    #[test]
    fn event_shortcut_opens_events_tab_with_a_new_draft() {
        let (mut app, mut rx) = app(BAN_MEMBERS | MANAGE_EVENTS);
        app.open_event_create("1");
        let m = app.moderation.as_ref().unwrap();
        assert_eq!(m.tab, ModTab::Events);
        assert!(m.event_draft.as_ref().is_some_and(|d| d.editing.is_none()));
        // 最初のタブの一覧は取りに行かず、イベントだけ読む
        assert!(matches!(
            next(&mut rx).1,
            net::ApiCall::GetScheduledEvents(_)
        ));
        assert!(rx.try_recv().is_err());
        let (mut none, _) = self::app(BAN_MEMBERS);
        none.open_event_create("1");
        assert!(none.moderation.is_none());
    }

    #[test]
    fn unban_needs_confirmation_and_reloads_after_success() {
        let (mut app, mut rx) = app(BAN_MEMBERS);
        app.open_moderation_tab("1", ModTab::Bans);
        let (id, _) = next(&mut rx);
        app.moderation_result(
            id,
            Ok(serde_json::json!([{"user":{"id":"55","username":"bad"},"reason":null}])),
        );
        let rows = app.moderation.as_ref().unwrap().rows.clone().unwrap();
        assert_eq!(rows[0].title, "bad");
        assert_eq!(rows[0].detail, "理由なし");
        app.delete_mod_row("55");
        assert!(rx.try_recv().is_err(), "1 回目は確認だけ");
        app.delete_mod_row("55");
        let (id, call) = next(&mut rx);
        assert!(matches!(call, net::ApiCall::Unban { ref user_id, .. } if user_id == "55"));
        // 失敗したら一覧を残して理由を出す
        app.moderation_result(id, Err("DELETE /guilds/1/bans/55: 403 Forbidden {}".into()));
        let m = app.moderation.as_ref().unwrap();
        assert!(m.error.as_deref().unwrap().contains("権限"));
        assert!(m.rows.is_some() && m.mutation.is_none());
        app.delete_mod_row("55");
        app.delete_mod_row("55");
        let (id, _) = next(&mut rx);
        app.moderation_result(id, Ok(serde_json::Value::Null));
        assert!(matches!(next(&mut rx).1, net::ApiCall::GetBans(_)));
        // 一覧にない ID は送らない
        app.delete_mod_row("99");
        app.delete_mod_row("99");
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn malformed_lists_are_errors_not_empty() {
        let (mut app, mut rx) = app(MANAGE_GUILD);
        app.open_moderation_tab("1", ModTab::Invites);
        let (id, _) = next(&mut rx);
        app.moderation_result(id, Ok(serde_json::json!([{"code":"../x"}])));
        let m = app.moderation.as_ref().unwrap();
        assert!(m.rows.is_none() && m.error.is_some());
    }

    #[test]
    fn audit_log_pages_with_before_cursor() {
        let (mut app, mut rx) = app(VIEW_AUDIT_LOG);
        app.open_moderation_tab("1", ModTab::Audit);
        let (id, _) = next(&mut rx);
        let entries: Vec<_> = (0..AUDIT_PAGE)
            .map(|i| serde_json::json!({"id":(1000 - i).to_string(),"user_id":"10","action_type":22,"target_id":"55"}))
            .collect();
        app.moderation_result(
            id,
            Ok(serde_json::json!({"audit_log_entries":entries,"users":[{"id":"10","username":"me"}]})),
        );
        let m = app.moderation.as_ref().unwrap();
        assert!(m.has_more);
        assert_eq!(m.rows.as_ref().unwrap()[0].title, "me がメンバーを BAN");
        app.more_audit_log();
        let (id, call) = next(&mut rx);
        assert!(
            matches!(call, net::ApiCall::GetAuditLog { before: Some(ref b), .. } if b == "951")
        );
        app.moderation_result(
            id,
            Ok(serde_json::json!({"audit_log_entries":[],"users":[]})),
        );
        let m = app.moderation.as_ref().unwrap();
        assert_eq!(m.rows.as_ref().unwrap().len(), AUDIT_PAGE as usize);
        assert!(!m.has_more);
    }

    #[test]
    fn event_body_validates_and_sends_only_changes() {
        let now = chrono::Utc::now();
        let future = |h: i64| {
            (now + chrono::Duration::hours(h))
                .with_timezone(&chrono::Local)
                .format("%Y-%m-%d %H:%M")
                .to_string()
        };
        let mut d = EventDraft {
            name: "集まり".into(),
            start: future(2),
            end: future(3),
            location: "公園".into(),
            ..Default::default()
        };
        let body = event_body(&d, None, &[], now).unwrap();
        assert_eq!(body["entity_type"], 3);
        assert_eq!(body["entity_metadata"]["location"], "公園");
        assert_eq!(body["privacy_level"], 2);
        assert!(body["scheduled_end_time"].is_string());
        // 場所のイベントは終了が要る・過去は不可・終了は開始より後
        let mut bad = d.clone();
        bad.end.clear();
        assert!(event_body(&bad, None, &[], now).is_err());
        bad = d.clone();
        bad.start = future(-1);
        assert!(event_body(&bad, None, &[], now).is_err());
        bad = d.clone();
        bad.end = future(1);
        assert!(event_body(&bad, None, &[], now).is_err());
        bad = d.clone();
        bad.channel_id = Some("77".into());
        assert!(
            event_body(&bad, None, &[], now).is_err(),
            "知らないチャンネル"
        );
        let voice = event_body(&bad, None, &["77".into()], now).unwrap();
        assert_eq!(voice["entity_type"], 2);
        assert_eq!(voice["channel_id"], "77");
        // 編集は変わった項目だけ
        let original = d.clone();
        d.name = "新しい名前".into();
        let body = event_body(&d, Some(&original), &[], now).unwrap();
        assert_eq!(body.as_object().unwrap().len(), 1);
        assert_eq!(body["name"], "新しい名前");
        assert!(event_body(&original, Some(&original), &[], now).is_err());
        // 開始を変えていなければ、開始が過ぎていても他の項目は直せる
        let mut started = original.clone();
        started.start = future(-1);
        let mut renamed = started.clone();
        renamed.description = "説明".into();
        assert!(event_body(&renamed, Some(&started), &[], now).is_ok());
    }

    #[test]
    fn rule_body_dedups_and_limits_keywords() {
        let body = rule_body(&RuleDraft {
            name: "禁止語".into(),
            keywords: "a, b、a\n\n c".into(),
        })
        .unwrap();
        assert_eq!(
            body["trigger_metadata"]["keyword_filter"],
            serde_json::json!(["a", "b", "c"])
        );
        assert!(rule_body(&RuleDraft {
            name: "x".into(),
            keywords: " , ".into()
        })
        .is_err());
        assert!(rule_body(&RuleDraft {
            name: "".into(),
            keywords: "a".into()
        })
        .is_err());
    }

    #[test]
    fn rule_toggle_sends_opposite_state() {
        let (mut app, mut rx) = app(MANAGE_GUILD);
        app.open_moderation_tab("1", ModTab::AutoMod);
        let (id, _) = next(&mut rx);
        app.moderation_result(
            id,
            Ok(serde_json::json!([{"id":"5","name":"r","enabled":true,"trigger_type":3}])),
        );
        app.toggle_rule("5");
        let (_, call) = next(&mut rx);
        assert!(
            matches!(call, net::ApiCall::ModifyAutoModRule { ref body, .. } if body["enabled"] == false)
        );
    }

    #[test]
    fn member_actions_check_permission_target_and_confirm() {
        let (mut app, mut rx) = app(KICK_MEMBERS);
        assert!(!app.member_action("1", "55", MemberAction::Ban));
        assert!(!app.member_action("1", "55", MemberAction::Ban));
        assert!(rx.try_recv().is_err(), "BAN 権限がない");
        assert!(!app.member_action("1", "10", MemberAction::Kick));
        assert!(!app.member_action("1", "10", MemberAction::Kick));
        assert!(rx.try_recv().is_err(), "自分は処分しない");
        assert!(!app.member_action("1", "55", MemberAction::Kick));
        assert!(app.member_action("1", "55", MemberAction::Kick));
        let (id, call) = next(&mut rx);
        assert!(matches!(call, net::ApiCall::Kick { .. }));
        assert!(app.is_moderation_request(id));
        // 結果を待つ間は次を送らない
        assert!(!app.member_action("1", "56", MemberAction::Kick));
        app.moderation_result(id, Ok(serde_json::Value::Null));
        assert!(app.member_action_pending.is_none());
        let (mut app, mut rx) = self::app(MODERATE_MEMBERS);
        app.member_action("1", "55", MemberAction::Timeout(3600));
        app.member_action("1", "55", MemberAction::Timeout(3600));
        let (_, call) = next(&mut rx);
        assert!(matches!(call, net::ApiCall::ModifyMember { ref body, .. }
            if body["communication_disabled_until"].is_string()));
    }
}
