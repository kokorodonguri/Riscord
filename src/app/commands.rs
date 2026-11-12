//! スラッシュコマンド（アプリのコマンドをユーザーとして実行する）。
//!
//! 使えるコマンドは application-command-index から取る（サーバー・DM・自分のアカウントに入れたアプリ）。
//! 実行は POST /interactions に type 2 を送る。本文に「/コマンド」と書いて送るのではない。
//! 結果は INTERACTION_SUCCESS / INTERACTION_FAILURE（nonce で見分ける）と、アプリの返事のメッセージで届く。
//! 候補を問い合わせる引数（autocomplete）は type 4 を送り、APPLICATION_COMMAND_AUTOCOMPLETE_RESPONSE で受け取る。
use super::*;
use crate::net::ApiCall;
use serde::Deserialize;

/// 一度読んだコマンドの一覧を使い回す時間
const INDEX_TTL: std::time::Duration = std::time::Duration::from_secs(600);
/// 実行してから返事を待つ時間
const INTERACTION_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);
/// 候補の問い合わせを、打つのを止めてから送るまで
const AUTOCOMPLETE_DEBOUNCE: std::time::Duration = std::time::Duration::from_millis(300);

pub const OPT_SUB_COMMAND: u8 = 1;
pub const OPT_SUB_COMMAND_GROUP: u8 = 2;
pub const OPT_STRING: u8 = 3;
pub const OPT_INTEGER: u8 = 4;
pub const OPT_BOOLEAN: u8 = 5;
pub const OPT_USER: u8 = 6;
pub const OPT_CHANNEL: u8 = 7;
pub const OPT_ROLE: u8 = 8;
pub const OPT_MENTIONABLE: u8 = 9;
pub const OPT_NUMBER: u8 = 10;
pub const OPT_ATTACHMENT: u8 = 11;

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct CommandChoice {
    pub name: String,
    pub value: serde_json::Value,
}

#[derive(Clone, Debug, Deserialize)]
pub struct CommandOption {
    #[serde(rename = "type")]
    pub kind: u8,
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub required: bool,
    #[serde(default)]
    pub choices: Vec<CommandChoice>,
    #[serde(default)]
    pub options: Vec<CommandOption>,
    #[serde(default)]
    pub autocomplete: bool,
    #[serde(default)]
    pub channel_types: Vec<u8>,
    #[serde(default)]
    pub min_value: Option<f64>,
    #[serde(default)]
    pub max_value: Option<f64>,
}

impl CommandOption {
    pub fn is_sub(&self) -> bool {
        matches!(self.kind, OPT_SUB_COMMAND | OPT_SUB_COMMAND_GROUP)
    }
}

#[derive(Clone, Debug, Deserialize)]
pub struct AppCommand {
    pub id: String,
    pub application_id: String,
    #[serde(default)]
    pub version: String,
    /// 1 = スラッシュコマンド（2・3 は右クリックのメニュー用）
    #[serde(rename = "type", default = "one")]
    pub kind: u8,
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub options: Vec<CommandOption>,
    #[serde(default)]
    pub default_member_permissions: Option<String>,
    /// 0 = サーバー、1 = Bot の DM、2 = それ以外の DM
    #[serde(default)]
    pub contexts: Option<Vec<u8>>,
    /// 実行するときにそのまま付ける
    #[serde(skip)]
    pub raw: serde_json::Value,
    /// アプリの名前（一覧の表示用）
    #[serde(skip)]
    pub app_name: String,
}

fn one() -> u8 {
    1
}

/// 場所ごとのコマンドの一覧
#[derive(Default)]
pub struct CommandIndex {
    pub commands: Vec<AppCommand>,
    pub loaded_at: Option<std::time::Instant>,
    pub loading: bool,
    /// 読めない場所（権限がない・Bot でない相手の DM など）。読み直さない
    pub failed: bool,
    /// 通信の失敗などで読めなかった。この時刻を過ぎたら読み直す
    pub retry_at: Option<std::time::Instant>,
}

/// 一時的な失敗のあと、読み直すまでの間
const RETRY_AFTER: std::time::Duration = std::time::Duration::from_secs(30);

/// Discord に断られた（4xx）失敗か。"GET /path: 403 Forbidden ..." の形の理由から読む
fn is_client_error(e: &str) -> bool {
    e.split(": ").any(|p| {
        p.len() >= 3 && p.starts_with('4') && p.as_bytes()[..3].iter().all(u8::is_ascii_digit)
    })
}

/// 入力中のコマンド
pub struct CommandDraft {
    pub command: AppCommand,
    /// 選んだサブコマンド（グループ → サブコマンドの順）
    pub path: Vec<String>,
    /// 引数の入力（名前 → 欄の文字と、候補から選んだ値）
    pub values: HashMap<String, (String, Option<serde_json::Value>)>,
    /// 候補を出している引数
    pub focus: Option<String>,
    /// 問い合わせた候補（引数, 候補）
    pub suggestions: Option<(String, Vec<CommandChoice>)>,
    pub ac_nonce: Option<String>,
    pub ac_due: Option<std::time::Instant>,
    pub error: Option<String>,
}

/// 送った実行（返事待ち）
pub struct PendingInteraction {
    pub channel_id: String,
    pub label: String,
    pub at: std::time::Instant,
}

/// 一覧の応答から、スラッシュコマンドだけ取り出す（アプリの名前を付ける）
pub fn parse_index(v: &serde_json::Value) -> Vec<AppCommand> {
    let apps: HashMap<&str, &str> = v["applications"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|a| Some((a["id"].as_str()?, a["name"].as_str()?)))
        .collect();
    v["application_commands"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|c| {
            let mut cmd: AppCommand = serde_json::from_value(c.clone()).ok()?;
            (cmd.kind == 1).then(|| {
                cmd.app_name = apps
                    .get(cmd.application_id.as_str())
                    .copied()
                    .unwrap_or_default()
                    .to_string();
                cmd.raw = c.clone();
                cmd
            })
        })
        .collect()
}

/// 名前で絞る（前方一致を先に）
pub fn filter_commands<'a>(list: &'a [AppCommand], q: &str, limit: usize) -> Vec<&'a AppCommand> {
    let q = q.to_lowercase();
    let mut head: Vec<&AppCommand> = list.iter().filter(|c| c.name.starts_with(&q)).collect();
    let rest = list.iter().filter(|c| {
        !c.name.starts_with(&q)
            && (c.name.contains(&q) || c.description.to_lowercase().contains(&q))
    });
    head.sort_by(|a, b| a.name.len().cmp(&b.name.len()).then(a.name.cmp(&b.name)));
    head.extend(rest);
    head.truncate(limit);
    head
}

/// 今の選択で入力する引数（サブコマンドを選び終えていなければ、その選択肢）
pub fn leaf_options<'a>(cmd: &'a AppCommand, path: &[String]) -> &'a [CommandOption] {
    let mut opts = cmd.options.as_slice();
    for p in path {
        match opts.iter().find(|o| o.is_sub() && &o.name == p) {
            Some(o) => opts = &o.options,
            None => break,
        }
    }
    opts
}

/// サブコマンドをまだ選ぶ必要があるか
pub fn needs_sub(cmd: &AppCommand, path: &[String]) -> bool {
    leaf_options(cmd, path).iter().any(|o| o.is_sub())
}

/// 引数1つの値を作る。名前などからの解決は resolve に任せる
pub fn option_value(
    o: &CommandOption,
    text: &str,
    chosen: Option<&serde_json::Value>,
    resolve: &dyn Fn(u8, &str) -> Option<String>,
) -> Result<Option<serde_json::Value>, String> {
    if let Some(v) = chosen {
        return Ok(Some(v.clone()));
    }
    let t = text.trim();
    if t.is_empty() {
        return if o.required {
            Err(format!("「{}」を入力してください", o.name))
        } else {
            Ok(None)
        };
    }
    // 選択肢があるなら、その名前か値のどれか
    if !o.choices.is_empty() {
        return o
            .choices
            .iter()
            .find(|c| {
                c.name == t
                    || c.value.as_str() == Some(t)
                    || c.value
                        .as_f64()
                        .is_some_and(|n| t.parse::<f64>().ok() == Some(n))
            })
            .map(|c| Some(c.value.clone()))
            .ok_or_else(|| format!("「{}」は候補から選んでください", o.name));
    }
    let range = |n: f64| -> Result<(), String> {
        if o.min_value.is_some_and(|m| n < m) || o.max_value.is_some_and(|m| n > m) {
            return Err(format!("「{}」の値が範囲外です", o.name));
        }
        Ok(())
    };
    Ok(Some(match o.kind {
        OPT_STRING => serde_json::Value::String(t.to_string()),
        OPT_INTEGER => {
            let n: i64 = t
                .parse()
                .map_err(|_| format!("「{}」には整数を入力してください", o.name))?;
            range(n as f64)?;
            n.into()
        }
        OPT_NUMBER => {
            let n: f64 = t
                .parse()
                .map_err(|_| format!("「{}」には数を入力してください", o.name))?;
            range(n)?;
            n.into()
        }
        OPT_BOOLEAN => match t {
            "true" | "はい" => true.into(),
            "false" | "いいえ" => false.into(),
            _ => return Err(format!("「{}」ははい・いいえで選んでください", o.name)),
        },
        OPT_USER | OPT_CHANNEL | OPT_ROLE | OPT_MENTIONABLE => {
            let id = resolve(o.kind, t)
                .ok_or_else(|| format!("「{}」の「{t}」が見つかりません", o.name))?;
            serde_json::Value::String(id)
        }
        OPT_ATTACHMENT => return Err("添付ファイルの引数にはまだ対応していません".into()),
        _ => serde_json::Value::String(t.to_string()),
    }))
}

/// Discord の ID の形の nonce（重複を防ぐ）
pub fn make_nonce() -> String {
    let ms = chrono::Utc::now().timestamp_millis() - 1_420_070_400_000;
    let r = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0) as u64
        & ((1 << 22) - 1);
    (((ms.max(0) as u64) << 22) | r).to_string()
}

impl App {
    /// 今いる場所のコマンド一覧の鍵
    fn command_scope(&self) -> Option<String> {
        match (&self.sel_guild, &self.sel_channel) {
            (Some(g), _) => Some(format!("g{g}")),
            (None, Some(c)) => Some(format!("c{c}")),
            _ => None,
        }
    }

    /// 今いる場所で使えるコマンドを読む（読んでいなければ・古ければ）
    pub fn ensure_commands(&mut self) {
        let Some(scope) = self.command_scope() else {
            return;
        };
        let mut calls = Vec::new();
        for (key, call) in [
            (
                scope.clone(),
                match scope.split_at(1) {
                    ("g", g) => ApiCall::GetGuildCommands(g.to_string()),
                    (_, c) => ApiCall::GetChannelCommands(c.to_string()),
                },
            ),
            ("u".to_string(), ApiCall::GetUserCommands),
        ] {
            let ix = self.command_index.entry(key).or_default();
            let fresh = ix.loaded_at.is_some_and(|t| t.elapsed() < INDEX_TTL);
            let waiting = ix.retry_at.is_some_and(|t| t > std::time::Instant::now());
            if !ix.loading && !fresh && !ix.failed && !waiting {
                ix.loading = true;
                calls.push(call);
            }
        }
        for c in calls {
            self.api(c);
        }
    }

    pub(super) fn on_commands_loaded(
        &mut self,
        key: String,
        result: Result<serde_json::Value, String>,
    ) {
        let ix = self.command_index.entry(key).or_default();
        ix.loading = false;
        match result {
            Ok(v) => {
                ix.commands = parse_index(&v);
                ix.loaded_at = Some(std::time::Instant::now());
                ix.failed = false;
                ix.retry_at = None;
            }
            Err(e) => {
                eprintln!("コマンドの一覧を読めませんでした: {e}");
                // 403（DM の相手が Bot でないなど）は読み直さない。通信の失敗などは少し待って読み直す
                if is_client_error(&e) {
                    ix.failed = true;
                    ix.retry_at = None;
                } else {
                    ix.retry_at = Some(std::time::Instant::now() + RETRY_AFTER);
                }
            }
        }
    }

    /// 今いる場所で使えるコマンド
    pub fn available_commands(&self) -> Vec<AppCommand> {
        let Some(scope) = self.command_scope() else {
            return Vec::new();
        };
        let in_guild = self.sel_guild.is_some();
        let bot_dm = !in_guild
            && self
                .current_channel()
                .is_some_and(|c| c.recipients.len() == 1 && c.recipients[0].bot);
        let context = if in_guild {
            0
        } else if bot_dm {
            1
        } else {
            2
        };
        let perms = self
            .sel_channel
            .as_deref()
            .and_then(|c| self.channel_perms(self.sel_guild.as_deref(), c));
        if perms.is_some_and(|p| p & model::USE_APPLICATION_COMMANDS == 0) {
            return Vec::new();
        }
        let mut seen = HashSet::new();
        let mut out = Vec::new();
        for key in [scope.as_str(), "u"] {
            for c in self
                .command_index
                .get(key)
                .map(|i| i.commands.as_slice())
                .unwrap_or(&[])
            {
                if c.contexts
                    .as_ref()
                    .is_some_and(|ctx| !ctx.contains(&context))
                {
                    continue;
                }
                // 既定で権限が要るコマンドは、その権限が無ければ出さない
                let need: u64 = c
                    .default_member_permissions
                    .as_deref()
                    .and_then(|p| p.parse().ok())
                    .unwrap_or(0);
                if in_guild && need != 0 && perms.is_some_and(|p| p & need != need) {
                    continue;
                }
                if seen.insert(c.id.clone()) {
                    out.push(c.clone());
                }
            }
        }
        out
    }

    /// 入力欄が「/名前」だけのとき、その名前（候補を出す）
    pub fn command_query(&self) -> Option<&str> {
        if self.editing.is_some() || self.cmd_draft.is_some() {
            return None;
        }
        let q = self.input.strip_prefix('/')?;
        (!q.contains(char::is_whitespace)).then_some(q)
    }

    /// コマンドを選んだ: 入力欄を引数の欄に切り替える
    pub fn start_command(&mut self, command: AppCommand) {
        self.input.clear();
        self.reply_to = None;
        self.cmd_draft = Some(CommandDraft {
            command,
            path: Vec::new(),
            values: HashMap::new(),
            focus: None,
            suggestions: None,
            ac_nonce: None,
            ac_due: None,
            error: None,
        });
    }

    pub fn cancel_command(&mut self) {
        self.cmd_draft = None;
        self.focus_input = true;
    }

    /// 引数の欄が変わった（候補から選んだら value に値）
    pub fn set_command_value(
        &mut self,
        name: &str,
        text: String,
        value: Option<serde_json::Value>,
    ) {
        let Some(d) = self.cmd_draft.as_mut() else {
            return;
        };
        // 候補から選んで欄の文字を書き換えた通知: 選んだ値を消さない
        if value.is_none()
            && d.values
                .get(name)
                .is_some_and(|(t, v)| *t == text && v.is_some())
        {
            return;
        }
        let chosen = value.is_some();
        d.values.insert(name.to_string(), (text, value));
        d.focus = Some(name.to_string());
        d.error = None;
        // 候補を問い合わせる引数なら、手を止めたら問い合わせる
        let auto = leaf_options(&d.command, &d.path)
            .iter()
            .any(|o| o.name == name && o.autocomplete);
        if chosen {
            d.suggestions = None;
        }
        if auto && !chosen {
            d.ac_due = Some(std::time::Instant::now() + AUTOCOMPLETE_DEBOUNCE);
            self.repaint_after(AUTOCOMPLETE_DEBOUNCE);
        }
    }

    /// 名前・ID からユーザー・チャンネル・ロールの ID を引く
    fn resolve_mention(&self, kind: u8, t: &str) -> Option<String> {
        let t = t.trim();
        let inner = t
            .trim_start_matches(['<', '@', '#', '&', '!'])
            .trim_end_matches('>');
        if !inner.is_empty() && inner.bytes().all(|b| b.is_ascii_digit()) {
            return Some(inner.to_string());
        }
        let g = self.sel_guild.as_deref();
        if matches!(kind, OPT_USER | OPT_MENTIONABLE) {
            let names = super::search::SearchNamesFor {
                app: self,
                guild: g,
            };
            if let Some(id) = crate::app::SearchNames::user_id(&names, t) {
                return Some(id);
            }
        }
        if matches!(kind, OPT_ROLE | OPT_MENTIONABLE) {
            let name = t.trim_start_matches('@');
            if let Some(r) = g
                .and_then(|g| self.roles.get(g))
                .into_iter()
                .flatten()
                .find(|r| r.name == name)
            {
                return Some(r.id.clone());
            }
        }
        if kind == OPT_CHANNEL {
            let names = super::search::SearchNamesFor {
                app: self,
                guild: g,
            };
            return crate::app::SearchNames::channel_id(&names, t);
        }
        None
    }

    /// 引数を Discord の形にする（サブコマンドで包む）
    fn command_options(
        &self,
        d: &CommandDraft,
        focused: Option<&str>,
    ) -> Result<Vec<serde_json::Value>, String> {
        let mut leaf = Vec::new();
        for o in leaf_options(&d.command, &d.path) {
            let (text, chosen) = d.values.get(&o.name).cloned().unwrap_or_default();
            if focused == Some(o.name.as_str()) {
                // 問い合わせでは、打ちかけの文字をそのまま送る
                leaf.push(serde_json::json!({ "type": o.kind, "name": o.name, "value": text, "focused": true }));
                continue;
            }
            let value = if focused.is_some() {
                option_value(o, &text, chosen.as_ref(), &|k, t| {
                    self.resolve_mention(k, t)
                })
                .ok()
                .flatten()
            } else {
                option_value(o, &text, chosen.as_ref(), &|k, t| {
                    self.resolve_mention(k, t)
                })?
            };
            if let Some(v) = value {
                leaf.push(serde_json::json!({ "type": o.kind, "name": o.name, "value": v }));
            }
        }
        // 内側から順にサブコマンド・グループで包む
        let mut opts = leaf;
        let mut level = d.command.options.as_slice();
        let mut chain = Vec::new();
        for p in &d.path {
            let Some(o) = level.iter().find(|o| o.is_sub() && &o.name == p) else {
                break;
            };
            chain.push((o.kind, o.name.clone()));
            level = &o.options;
        }
        for (kind, name) in chain.into_iter().rev() {
            opts = vec![serde_json::json!({ "type": kind, "name": name, "options": opts })];
        }
        Ok(opts)
    }

    fn interaction_body(
        &self,
        kind: u8,
        d: &CommandDraft,
        options: Vec<serde_json::Value>,
        nonce: &str,
    ) -> Option<serde_json::Value> {
        let channel_id = self.sel_channel.clone()?;
        let c = &d.command;
        Some(serde_json::json!({
            "type": kind,
            "application_id": c.application_id,
            "guild_id": self.sel_guild,
            "channel_id": channel_id,
            "session_id": self.session_id,
            "nonce": nonce,
            "analytics_location": "slash_ui",
            "data": {
                "version": c.version,
                "id": c.id,
                "name": c.name,
                "type": 1,
                "options": options,
                "application_command": c.raw,
                "attachments": [],
            },
        }))
    }

    /// コマンドを実行する
    pub fn run_command(&mut self) {
        let Some(d) = self.cmd_draft.as_ref() else {
            return;
        };
        if self.session_id.is_none() {
            if let Some(d) = self.cmd_draft.as_mut() {
                d.error =
                    Some("接続の準備ができていません。少し待ってからもう一度お試しください".into());
            }
            return;
        }
        if needs_sub(&d.command, &d.path) {
            if let Some(d) = self.cmd_draft.as_mut() {
                d.error = Some("サブコマンドを選んでください".into());
            }
            return;
        }
        let options = match self.command_options(d, None) {
            Ok(o) => o,
            Err(e) => {
                if let Some(d) = self.cmd_draft.as_mut() {
                    d.error = Some(e);
                }
                return;
            }
        };
        let nonce = make_nonce();
        let Some(body) = self.interaction_body(2, d, options, &nonce) else {
            return;
        };
        let label = std::iter::once(d.command.name.as_str())
            .chain(d.path.iter().map(String::as_str))
            .collect::<Vec<_>>()
            .join(" ");
        let channel_id = self.sel_channel.clone().unwrap_or_default();
        self.pending_interactions.insert(
            nonce,
            PendingInteraction {
                channel_id,
                label: format!("/{label}"),
                at: std::time::Instant::now(),
            },
        );
        self.cmd_draft = None;
        self.focus_input = true;
        self.api(ApiCall::Interaction(body));
        self.repaint_after(INTERACTION_TIMEOUT);
    }

    /// 毎回の描画で呼ぶ: 候補の問い合わせを送る・返事の来ない実行を知らせる
    pub fn tick_commands(&mut self) {
        let now = std::time::Instant::now();
        if let Some(d) = self
            .cmd_draft
            .as_mut()
            .filter(|d| d.ac_due.is_some_and(|t| now >= t))
        {
            d.ac_due = None;
            let Some(focus) = d.focus.clone() else { return };
            let nonce = make_nonce();
            let body = self
                .command_options(self.cmd_draft.as_ref().unwrap(), Some(&focus))
                .ok()
                .and_then(|o| {
                    self.interaction_body(4, self.cmd_draft.as_ref().unwrap(), o, &nonce)
                });
            if let (Some(body), Some(d)) = (body, self.cmd_draft.as_mut()) {
                d.ac_nonce = Some(nonce);
                self.api(ApiCall::Interaction(body));
            }
        }
        let late: Vec<String> = self
            .pending_interactions
            .iter()
            .filter(|(_, p)| p.at.elapsed() >= INTERACTION_TIMEOUT)
            .map(|(n, _)| n.clone())
            .collect();
        for n in late {
            if let Some(p) = self.pending_interactions.remove(&n) {
                self.show_toast(&format!("{} にアプリが応答しませんでした", p.label));
            }
        }
    }

    /// 実行・候補の問い合わせの送信結果（届いたかどうか。結果そのものはゲートウェイで来る）
    pub(super) fn on_interaction_sent(
        &mut self,
        body: serde_json::Value,
        result: Result<serde_json::Value, String>,
    ) {
        let Err(e) = result else { return };
        eprintln!("コマンドを送れませんでした: {e}");
        let nonce = body["nonce"].as_str().unwrap_or_default();
        if body["type"] == 4 {
            return;
        }
        if let Some(p) = self.pending_interactions.remove(nonce) {
            self.show_toast(&api_error_text(
                &e,
                &format!("{} を実行できませんでした", p.label),
            ));
        }
    }

    /// INTERACTION_* / APPLICATION_COMMAND_AUTOCOMPLETE_RESPONSE
    pub(super) fn on_interaction_event(&mut self, t: &str, d: &serde_json::Value) {
        let nonce = d["nonce"].as_str().unwrap_or_default();
        match t {
            "INTERACTION_CREATE" => {
                // 受け付けられた: 返事のメッセージを待つ
                if let Some(p) = self.pending_interactions.get_mut(nonce) {
                    p.at = std::time::Instant::now();
                }
            }
            "INTERACTION_SUCCESS" => {
                self.pending_interactions.remove(nonce);
            }
            "INTERACTION_FAILURE" => {
                if let Some(p) = self.pending_interactions.remove(nonce) {
                    self.show_toast(&format!("{} の実行に失敗しました", p.label));
                }
            }
            "APPLICATION_COMMAND_AUTOCOMPLETE_RESPONSE" => {
                let Some(dr) = self
                    .cmd_draft
                    .as_mut()
                    .filter(|dr| dr.ac_nonce.as_deref() == Some(nonce))
                else {
                    return;
                };
                let choices: Vec<CommandChoice> = d["choices"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|c| serde_json::from_value(c.clone()).ok())
                    .collect();
                if let Some(f) = dr.focus.clone() {
                    dr.suggestions = Some((f, choices));
                }
            }
            _ => {}
        }
    }

    /// 返事待ちの実行（入力欄の上に出す）
    pub fn pending_command_label(&self) -> Option<&str> {
        let c = self.sel_channel.as_deref()?;
        self.pending_interactions
            .values()
            .find(|p| p.channel_id == c)
            .map(|p| p.label.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn index() -> serde_json::Value {
        serde_json::json!({
            "applications": [{ "id": "a1", "name": "Bot" }],
            "application_commands": [
                { "id": "1", "application_id": "a1", "version": "9", "type": 1, "name": "ping", "description": "応答を確かめる" },
                { "id": "2", "application_id": "a1", "version": "9", "type": 2, "name": "Report" },
                { "id": "3", "application_id": "a1", "version": "9", "type": 1, "name": "config", "description": "設定", "default_member_permissions": "8",
                  "options": [{ "type": 2, "name": "role", "description": "", "options": [{ "type": 1, "name": "set", "description": "", "options": [
                      { "type": 8, "name": "target", "description": "", "required": true },
                      { "type": 4, "name": "level", "description": "", "min_value": 1, "max_value": 5 },
                  ] }] }] },
                { "id": "4", "application_id": "a1", "version": "9", "type": 1, "name": "pick", "description": "", "options": [
                    { "type": 3, "name": "color", "description": "", "required": true, "choices": [{ "name": "赤", "value": "red" }] },
                    { "type": 3, "name": "city", "description": "", "autocomplete": true },
                ] },
            ],
        })
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
        app.sel_guild = Some("g".into());
        app.sel_channel = Some("c".into());
        app.session_id = Some("sess".into());
        app.roles.insert(
            "g".into(),
            vec![Role {
                id: "r9".into(),
                name: "mod".into(),
                color: 0,
                position: 0,
                hoist: false,
                managed: Some(false),
                permissions: "0".into(),
            }],
        );
        (app, tx, rx)
    }

    #[test]
    fn index_failure_retries_only_when_not_refused() {
        assert!(is_client_error(
            "GET /guilds/1/application-command-index: 403 Forbidden {}"
        ));
        assert!(!is_client_error(
            "GET /guilds/1/application-command-index: 502 Bad Gateway"
        ));
        assert!(!is_client_error(
            "error sending request for url (https://discord.com/api/v9/x)"
        ));
        let (mut app, _tx, _rx) = harness();
        app.on_commands_loaded("u".into(), Err("error sending request".into()));
        assert!(
            !app.command_index["u"].failed && app.command_index["u"].retry_at.is_some(),
            "a network failure is retried later"
        );
        app.on_commands_loaded(
            "u".into(),
            Err("GET /users/@me/application-command-index: 403 Forbidden".into()),
        );
        assert!(
            app.command_index["u"].failed && app.command_index["u"].retry_at.is_none(),
            "a refusal is not retried"
        );
        app.on_commands_loaded("u".into(), Ok(index()));
        assert!(app.command_index["u"].retry_at.is_none() && !app.command_index["u"].failed);
    }

    #[test]
    fn index_keeps_only_slash_commands_and_filters_by_name() {
        let list = parse_index(&index());
        assert_eq!(
            list.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(),
            ["ping", "config", "pick"]
        );
        assert_eq!(list[0].app_name, "Bot");
        assert_eq!(
            filter_commands(&list, "p", 10)
                .iter()
                .map(|c| c.name.as_str())
                .collect::<Vec<_>>(),
            ["pick", "ping"]
        );
        assert_eq!(filter_commands(&list, "設定", 10)[0].name, "config");
    }

    #[test]
    fn values_are_checked_by_type() {
        let list = parse_index(&index());
        let pick = &list[2];
        let none = |_: u8, _: &str| None;
        assert_eq!(
            option_value(&pick.options[0], "赤", None, &none).unwrap(),
            Some("red".into())
        );
        assert!(option_value(&pick.options[0], "青", None, &none).is_err());
        assert!(option_value(&pick.options[0], "", None, &none)
            .unwrap_err()
            .contains("color"));
        assert_eq!(
            option_value(&pick.options[1], "", None, &none).unwrap(),
            None
        );
        let level = &leaf_options(&list[1], &["role".into(), "set".into()])[1];
        assert!(option_value(level, "9", None, &none)
            .unwrap_err()
            .contains("範囲"));
        assert!(option_value(level, "x", None, &none).is_err());
        assert_eq!(
            option_value(level, "3", None, &none).unwrap(),
            Some(3.into())
        );
    }

    #[test]
    fn running_a_subcommand_sends_a_real_interaction() {
        let (mut app, tx, mut rx) = harness();
        tx.send(Event::Api {
            id: 0,
            call: ApiCall::GetGuildCommands("g".into()),
            result: Ok(index()),
        })
        .unwrap();
        app.poll();
        let config = app
            .available_commands()
            .into_iter()
            .find(|c| c.name == "config")
            .unwrap();
        app.start_command(config);
        assert!(needs_sub(&app.cmd_draft.as_ref().unwrap().command, &[]));
        app.cmd_draft.as_mut().unwrap().path = vec!["role".into(), "set".into()];
        app.set_command_value("target", "mod".into(), None);
        app.run_command();
        let Cmd::Api {
            call: ApiCall::Interaction(body),
            ..
        } = rx.try_recv().unwrap()
        else {
            panic!("実行を送っていない")
        };
        assert_eq!(body["type"], 2);
        assert_eq!(body["session_id"], "sess");
        assert_eq!(body["data"]["application_command"]["id"], "3");
        let sub = &body["data"]["options"][0];
        assert_eq!(
            (sub["type"].as_u64(), sub["name"].as_str()),
            (Some(2), Some("role"))
        );
        assert_eq!(
            sub["options"][0]["options"][0],
            serde_json::json!({ "type": 8, "name": "target", "value": "r9" })
        );
        // 失敗の知らせを nonce で受け取る
        let nonce = body["nonce"].as_str().unwrap().to_string();
        assert!(app.pending_command_label().is_some());
        tx.send(Event::Gateway {
            t: "INTERACTION_FAILURE".into(),
            d: serde_json::json!({ "nonce": nonce }),
        })
        .unwrap();
        app.poll();
        assert!(app.pending_command_label().is_none());
        assert!(app
            .toast
            .as_ref()
            .is_some_and(|(t, _)| t.contains("/config role set")));
    }

    #[test]
    fn missing_required_value_is_not_sent() {
        let (mut app, tx, mut rx) = harness();
        tx.send(Event::Api {
            id: 0,
            call: ApiCall::GetGuildCommands("g".into()),
            result: Ok(index()),
        })
        .unwrap();
        app.poll();
        let pick = app
            .available_commands()
            .into_iter()
            .find(|c| c.name == "pick")
            .unwrap();
        app.start_command(pick);
        app.run_command();
        assert!(rx.try_recv().is_err());
        assert!(app
            .cmd_draft
            .as_ref()
            .unwrap()
            .error
            .as_ref()
            .unwrap()
            .contains("color"));
    }

    #[test]
    fn autocomplete_asks_with_focused_option_and_reads_reply() {
        let (mut app, tx, mut rx) = harness();
        tx.send(Event::Api {
            id: 0,
            call: ApiCall::GetGuildCommands("g".into()),
            result: Ok(index()),
        })
        .unwrap();
        app.poll();
        let pick = app
            .available_commands()
            .into_iter()
            .find(|c| c.name == "pick")
            .unwrap();
        app.start_command(pick);
        app.set_command_value("city", "tok".into(), None);
        app.tick_commands();
        assert!(rx.try_recv().is_err(), "打つのを止める前に問い合わせた");
        app.cmd_draft.as_mut().unwrap().ac_due = Some(std::time::Instant::now());
        app.tick_commands();
        let Cmd::Api {
            call: ApiCall::Interaction(body),
            ..
        } = rx.try_recv().unwrap()
        else {
            panic!()
        };
        assert_eq!(body["type"], 4);
        assert!(body["data"]["options"]
            .as_array()
            .unwrap()
            .iter()
            .any(|o| o["name"] == "city" && o["focused"] == true && o["value"] == "tok"));
        tx.send(Event::Gateway { t: "APPLICATION_COMMAND_AUTOCOMPLETE_RESPONSE".into(), d: serde_json::json!({ "nonce": body["nonce"], "choices": [{ "name": "東京", "value": "tokyo" }] }) }).unwrap();
        app.poll();
        let (name, choices) = app.cmd_draft.as_ref().unwrap().suggestions.clone().unwrap();
        assert_eq!(
            (name.as_str(), choices[0].value.as_str()),
            ("city", Some("tokyo"))
        );
    }

    #[test]
    fn permission_gated_commands_are_hidden() {
        let (mut app, tx, _rx) = harness();
        app.me = Some(
            serde_json::from_value(serde_json::json!({ "id": "me", "username": "me" })).unwrap(),
        );
        app.guilds.push(Guild {
            id: "g".into(),
            name: "g".into(),
            icon: None,
            permissions: Some((1u64 << 31 | 1 << 10).to_string()),
            owner: false,
        });
        app.my_roles.insert("g".into(), vec![]);
        app.channels.insert(
            "g".into(),
            vec![serde_json::from_value(serde_json::json!({ "id": "c", "type": 0 })).unwrap()],
        );
        tx.send(Event::Api {
            id: 0,
            call: ApiCall::GetGuildCommands("g".into()),
            result: Ok(index()),
        })
        .unwrap();
        app.poll();
        let names: Vec<String> = app
            .available_commands()
            .into_iter()
            .map(|c| c.name)
            .collect();
        assert!(
            !names.contains(&"config".to_string()),
            "管理者向けのコマンドが出ている"
        );
        assert!(names.contains(&"ping".to_string()));
    }
}
