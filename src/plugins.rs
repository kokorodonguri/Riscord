//! Sandboxed Lua extensions. All calls into Lua have an instruction and memory budget.
use crate::model::Message;
use mlua::{Function, HookTriggers, Lua, LuaOptions, LuaSerdeExt, StdLib, Table, Value};
use serde::Deserialize;
use std::{
    cell::RefCell,
    path::{Path, PathBuf},
    rc::Rc,
    time::{Duration, Instant},
};

const API_VERSION: u32 = 1;
const MAX_SCRIPT: u64 = 1024 * 1024;
/// 画面に出すプラグインの文字列の上限（文字数）。長くても画面を押しつぶさないように切る
const MAX_ERROR_CHARS: usize = 600;
const MAX_ACTION_LABEL_CHARS: usize = 60;
const MAX_PAGE_LABEL_CHARS: usize = 2000;
/// メッセージの操作（全プラグイン合計）と、ボイスパネルの行の上限
const MAX_MESSAGE_ACTIONS: usize = 40;
const MAX_VOICE_LINES: usize = 8;

/// 文字数で切り、切ったら末尾を … にする（UTF-8 の境目で切る）
pub fn clip(s: &str, max_chars: usize) -> String {
    match s.char_indices().nth(max_chars) {
        Some((end, _)) => format!("{}…", &s[..end]),
        None => s.to_owned(),
    }
}

#[derive(Clone, Deserialize)]
pub struct Manifest {
    pub id: String,
    pub name: String,
    pub version: String,
    #[serde(default)]
    pub description: String,
    pub api: u32,
    pub entry: String,
    /// 同梱プラグインの英語・中国語の説明（description は日本語）
    #[serde(default)]
    pub description_en: String,
    #[serde(default)]
    pub description_zh: String,
    /// BetterDiscord のプラグインを Riscord 向けに作り直したもの
    #[serde(default)]
    pub bd: bool,
}
impl Manifest {
    /// 表示言語に合わせた説明
    pub fn describe(&self) -> &str {
        let pick = match crate::i18n::code() {
            "en" => &self.description_en,
            "zh" => &self.description_zh,
            _ => &self.description,
        };
        if pick.is_empty() {
            &self.description
        } else {
            pick
        }
    }
}
mod bundled {
    include!(concat!(env!("OUT_DIR"), "/bundled_plugins.rs"));
}
/// 同梱プラグインの先頭に連結する共通部品
const LIB: &str = include_str!("../plugins/_lib.lua");

#[derive(Clone, Debug, PartialEq)]
pub enum Action {
    Send(String, String),
    /// デスクトップ通知（タイトル, 本文）
    Notify(String, String),
    /// クリップボードへ文字列を書く
    Copy(String),
    /// ブラウザなどで URL を開く
    OpenUrl(String),
    /// online / idle / dnd / invisible
    SetStatus(String),
    /// 入力欄へ文字列を差し込む
    Insert(String),
    /// チャンネルを開く
    OpenChannel(String),
    /// 効果音（"message" / "mention" / "join" / "leave" / "mute" / "key" など）
    Sound(String),
    /// 読み上げ
    Speak(String),
    /// 情報ダイアログ（見出し, 本文）
    Info(String, String),
}
/// プラグインから読める本体の状態（App が更新する）
#[derive(Default, Clone)]
pub struct HostInfo {
    pub me_id: String,
    pub me_name: String,
    pub status: String,
    pub channel_id: String,
    pub guild_id: String,
    pub voice_channel_id: String,
}
/// メッセージの操作 (プラグイン id, 番号, 表示名, アイコン名)
pub type MessageAction = (String, usize, String, Option<String>);
#[derive(Clone)]
pub struct PageItem {
    pub kind: String,
    pub label: String,
    pub action: String,
    pub enabled: bool,
}
struct Timer {
    at: Instant,
    callback: mlua::RegistryKey,
}
struct Runtime {
    has_page: bool,
    lua: Lua,
    queue: Rc<RefCell<Vec<Action>>>,
    timers: Rc<RefCell<Vec<Timer>>>,
}
pub struct Plugin {
    pub manifest: Manifest,
    pub error: Option<String>,
    runtime: Option<Runtime>,
    path: PathBuf,
    bundled_source: Option<&'static str>,
    /// 実行中のエラーで止まったが、まだ Plugins::settle で表示に反映していない
    failed: bool,
    /// 止まったことを画面に知らせる（描画中に止まっても次の描画で反映する）
    waker: crate::wake::Waker,
}
#[derive(Default)]
pub struct Plugins {
    pub entries: Vec<Plugin>,
    /// id → entries の位置
    index: std::collections::HashMap<String, usize>,
    /// options() の結果のキャッシュ
    options_cache: RefCell<std::collections::HashMap<String, serde_json::Value>>,
    pub host: Rc<RefCell<HostInfo>>,
    pub errors: Vec<String>,
    pub selected: Option<String>,
    pub revision: u64,
    pub generation: u64,
    /// プラグインが riscord.save で書くフォルダ（アカウントごと）
    pub data_root: PathBuf,
    /// プラグインが止まったときに描き直しを頼む先（App が入れる）
    pub waker: crate::wake::Waker,
    /// エラーで止まったが、まだ画面で知らせていないプラグインの名前（take_stopped で取り出す）
    stopped: Vec<String>,
    /// message_actions() の結果（revision ごと）。ツールバーは毎フレーム描くので Lua を毎回呼ばない
    actions_cache: Option<(u64, Vec<MessageAction>)>,
}

pub fn directory() -> PathBuf {
    crate::app::config_root().join("riscord/plugins")
}
fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .bytes()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-' || c == b'_')
}
fn read_limited(path: &Path) -> anyhow::Result<String> {
    let m = std::fs::metadata(path)?;
    anyhow::ensure!(
        m.is_file() && m.len() <= MAX_SCRIPT,
        "{}",
        tr!(
            "ファイルが大きすぎるか通常のファイルではありません",
            "The file is too large or isn't a regular file",
            "文件过大或不是普通文件",
        )
    );
    Ok(std::fs::read_to_string(path)?)
}
fn entry_path(dir: &Path, entry: &str) -> anyhow::Result<PathBuf> {
    let root = dir.canonicalize()?;
    let path = dir.join(entry).canonicalize()?;
    anyhow::ensure!(
        path.starts_with(root),
        "{}",
        tr!(
            "エントリポイントがプラグインフォルダの外です",
            "The entry point is outside the plugin folder",
            "入口点位于插件文件夹之外",
        )
    );
    Ok(path)
}

/// Lua に渡すメッセージ
fn message_table(lua: &Lua, m: &Message) -> mlua::Result<Table> {
    let t = lua.create_table()?;
    t.set("id", m.id.clone())?;
    t.set("channel_id", m.channel_id.clone())?;
    t.set("guild_id", m.guild_id.clone())?;
    t.set("author_id", m.author.id.clone())?;
    t.set(
        "author_name",
        m.member
            .as_ref()
            .and_then(|mb| mb.nick.clone())
            .unwrap_or_else(|| m.author.display().to_owned()),
    )?;
    t.set("author_username", m.author.username.clone())?;
    t.set("bot", m.author.bot)?;
    t.set("content", m.content.clone())?;
    t.set("timestamp", m.timestamp.clone())?;
    t.set("mention_everyone", m.mention_everyone)?;
    let mentions = lua.create_table()?;
    for (i, u) in m.mentions.iter().enumerate() {
        mentions.set(i + 1, u.id.clone())?;
    }
    t.set("mentions", mentions)?;
    let attachments = lua.create_table()?;
    for (i, a) in m.attachments.iter().enumerate() {
        attachments.set(i + 1, a.url.clone())?;
    }
    t.set("attachments", attachments)?;
    t.set(
        "reply_to",
        m.referenced_message.as_ref().map(|r| r.id.clone()),
    )?;
    Ok(t)
}

impl Runtime {
    #[cfg(test)]
    fn new(manifest: &Manifest, dir: &Path, data_dir: &Path) -> mlua::Result<Self> {
        let path =
            entry_path(dir, &manifest.entry).map_err(|e| mlua::Error::runtime(e.to_string()))?;
        let script = read_limited(&path).map_err(|e| mlua::Error::runtime(e.to_string()))?;
        Self::from_source(manifest, &script, data_dir)
    }
    #[cfg(test)]
    fn from_source(manifest: &Manifest, script: &str, data_dir: &Path) -> mlua::Result<Self> {
        Self::with_host(manifest, script, data_dir, Default::default())
    }
    fn with_host(
        manifest: &Manifest,
        script: &str,
        data_dir: &Path,
        host: Rc<RefCell<HostInfo>>,
    ) -> mlua::Result<Self> {
        let lua = Lua::new_with(
            StdLib::TABLE | StdLib::STRING | StdLib::MATH | StdLib::UTF8,
            LuaOptions::default(),
        )?;
        lua.set_memory_limit(16 * 1024 * 1024)?;
        lua.set_hook(
            HookTriggers::new().every_nth_instruction(100_000),
            |_, _| {
                Err(mlua::Error::runtime(tr!(
                    "実行時間の制限を超えました",
                    "Exceeded the execution time limit",
                    "超出执行时间限制"
                )))
            },
        );
        lua.globals().set("pcall", Value::Nil)?;
        lua.globals().set("xpcall", Value::Nil)?;
        for name in ["dofile", "loadfile", "load", "require", "collectgarbage"] {
            lua.globals().set(name, Value::Nil)?;
        }
        let queue = Rc::new(RefCell::new(Vec::new()));
        let timers: Rc<RefCell<Vec<Timer>>> = Rc::new(RefCell::new(Vec::new()));
        let api = lua.create_table()?;
        let q = queue.clone();
        api.set(
            "send_message",
            lua.create_function(move |_, (channel, content): (String, String)| {
                if channel.len() > 64
                    || content.trim().is_empty()
                    || content.len() > 8000
                    || q.borrow().len() >= 100
                {
                    return Err(mlua::Error::runtime(tr!(
                        "無効な送信内容、または送信待ちが上限です",
                        "Invalid message, or the send queue is full",
                        "发送内容无效，或发送队列已满"
                    )));
                }
                q.borrow_mut().push(Action::Send(channel, content));
                Ok(())
            })?,
        )?;
        let ts = timers.clone();
        api.set(
            "set_timeout",
            lua.create_function(move |lua, (ms, f): (u64, Function)| {
                if ts.borrow().len() >= 100 {
                    return Err(mlua::Error::runtime(tr!(
                        "タイマーの上限です",
                        "Timer limit reached",
                        "已达到计时器上限"
                    )));
                }
                let callback = lua.create_registry_value(f)?;
                ts.borrow_mut().push(Timer {
                    at: Instant::now() + Duration::from_millis(ms.clamp(100, 86_400_000)),
                    callback,
                });
                Ok(())
            })?,
        )?;
        let data_dir = data_dir.to_path_buf();
        let data_file = data_dir.join(format!("{}.json", manifest.id));
        let file = data_file.clone();
        api.set(
            "load",
            lua.create_function(move |lua, ()| {
                let v: serde_json::Value = std::fs::read(&file)
                    .ok()
                    .filter(|b| b.len() <= MAX_SCRIPT as usize)
                    .and_then(|b| serde_json::from_slice(&b).ok())
                    .unwrap_or(serde_json::Value::Null);
                if v.is_null() {
                    Ok(Value::Nil)
                } else {
                    lua.to_value(&v)
                }
            })?,
        )?;
        api.set(
            "save",
            lua.create_function(move |lua, value: Value| {
                let v: serde_json::Value = lua.from_value(value)?;
                let bytes = serde_json::to_vec(&v).map_err(mlua::Error::external)?;
                if bytes.len() > MAX_SCRIPT as usize {
                    return Err(mlua::Error::runtime(tr!(
                        "保存データが大きすぎます",
                        "The saved data is too large",
                        "保存的数据过大"
                    )));
                }
                std::fs::create_dir_all(&data_dir).map_err(mlua::Error::external)?;
                std::fs::write(&data_file, bytes).map_err(mlua::Error::external)
            })?,
        )?;
        for (name, make) in [
            ("notify", 2usize),
            ("copy", 1),
            ("open_url", 1),
            ("set_status", 1),
            ("insert", 1),
            ("open_channel", 1),
            ("play_sound", 1),
            ("speak", 1),
            ("show_info", 2),
        ] {
            let q = queue.clone();
            api.set(
                name,
                lua.create_function(move |_, (a, b): (String, Option<String>)| {
                    if a.len() > 8000
                        || b.as_ref().is_some_and(|b| b.len() > 8000)
                        || q.borrow().len() >= 100
                    {
                        return Err(mlua::Error::runtime(tr!(
                            "内容が長すぎるか、操作の待ちが上限です",
                            "The content is too long, or too many actions are queued",
                            "内容过长，或待处理操作已达上限"
                        )));
                    }
                    let action = match (name, make) {
                        ("notify", _) => Action::Notify(a, b.unwrap_or_default()),
                        ("copy", _) => Action::Copy(a),
                        ("open_url", _)
                            if a.starts_with("https://")
                                || a.starts_with("http://")
                                || a.starts_with("steam://") =>
                        {
                            Action::OpenUrl(a)
                        }
                        ("open_url", _) => {
                            return Err(mlua::Error::runtime(
                                "open_url: http(s):// or steam:// only",
                            ))
                        }
                        ("set_status", _)
                            if ["online", "idle", "dnd", "invisible"].contains(&a.as_str()) =>
                        {
                            Action::SetStatus(a)
                        }
                        ("set_status", _) => {
                            return Err(mlua::Error::runtime(
                                "set_status: online / idle / dnd / invisible",
                            ))
                        }
                        ("insert", _) => Action::Insert(a),
                        ("open_channel", _) => Action::OpenChannel(a),
                        ("play_sound", _) => Action::Sound(a),
                        ("show_info", _) => Action::Info(a, b.unwrap_or_default()),
                        _ => Action::Speak(a),
                    };
                    q.borrow_mut().push(action);
                    Ok(())
                })?,
            )?;
        }
        api.set(
            "now",
            lua.create_function(|_, ()| Ok(chrono::Utc::now().timestamp_millis()))?,
        )?;
        // chrono の書式で日時を作る（秒。省略時は今）。utc=true で UTC
        api.set(
            "date",
            lua.create_function(|_, (fmt, secs, utc): (String, Option<f64>, Option<bool>)| {
                use std::fmt::Write;
                let secs = secs.unwrap_or_else(|| chrono::Utc::now().timestamp() as f64);
                let Some(t) = chrono::DateTime::from_timestamp(secs as i64, 0) else {
                    return Ok(String::new());
                };
                let mut out = String::new();
                let ok = if utc.unwrap_or(false) {
                    write!(out, "{}", t.format(&fmt)).is_ok()
                } else {
                    write!(out, "{}", t.with_timezone(&chrono::Local).format(&fmt)).is_ok()
                };
                Ok(if ok { out } else { String::new() })
            })?,
        )?;
        let h = host.clone();
        api.set(
            "me",
            lua.create_function(move |lua, ()| {
                let h = h.borrow();
                let t = lua.create_table()?;
                t.set("id", h.me_id.clone())?;
                t.set("name", h.me_name.clone())?;
                t.set("status", h.status.clone())?;
                t.set("channel_id", h.channel_id.clone())?;
                t.set("guild_id", h.guild_id.clone())?;
                t.set("voice_channel_id", h.voice_channel_id.clone())?;
                Ok(t)
            })?,
        )?;
        api.set("api_version", API_VERSION)?;
        // 表示言語（"ja" / "en" / "zh"）。設定で切り替えられるので、表示を作るたびに呼ぶ
        api.set(
            "language",
            lua.create_function(|_, ()| Ok(crate::i18n::code()))?,
        )?;
        lua.globals().set("riscord", api)?;
        let callbacks: Table = lua.load(script).set_name(&manifest.entry).eval()?;
        lua.globals().set("__plugin", callbacks.clone())?;
        let has_page = callbacks
            .raw_get::<Value>("page")
            .is_ok_and(|v| matches!(v, Value::Function(_)));
        let runtime = Self {
            has_page,
            lua,
            queue,
            timers,
        };
        runtime.call("start", ())?;
        Ok(runtime)
    }
    fn reset_budget(&self) {
        self.lua.set_hook(
            HookTriggers::new().every_nth_instruction(100_000),
            |_, _| {
                Err(mlua::Error::runtime(tr!(
                    "実行時間の制限を超えました",
                    "Exceeded the execution time limit",
                    "超出执行时间限制"
                )))
            },
        );
    }
    fn call<A: mlua::IntoLuaMulti>(&self, name: &str, args: A) -> mlua::Result<Value> {
        self.reset_budget();
        let t: Table = self.lua.globals().get("__plugin")?;
        match t.get::<Value>(name)? {
            Value::Function(f) => f.call(args),
            Value::Nil => Ok(Value::Nil),
            _ => Err(mlua::Error::runtime(tr!(
                "{name} は関数である必要があります",
                "{name} must be a function",
                "{name} 必须是函数",
            ))),
        }
    }
    fn message(&self, name: &str, m: &Message) -> mlua::Result<Value> {
        self.message_with(name, m, None)
    }
    fn message_with(&self, name: &str, m: &Message, extra: Option<&str>) -> mlua::Result<Value> {
        self.reset_budget();
        let t = message_table(&self.lua, m)?;
        self.call(name, (t, extra))
    }
}
impl Plugins {
    pub fn reload(&mut self, enabled: &[String]) {
        self.generation = self.generation.wrapping_add(1);
        self.revision = self.revision.wrapping_add(1);
        for p in &mut self.entries {
            p.stop();
        }
        self.entries.clear();
        self.errors.clear();
        self.selected = None;
        self.options_cache.borrow_mut().clear();
        self.actions_cache = None;
        self.stopped.clear();
        let mut seen = std::collections::HashSet::new();
        for (toml_text, source) in bundled::BUNDLED {
            let manifest: Manifest = match toml::from_str(toml_text) {
                Ok(m) => m,
                Err(e) => {
                    self.errors.push(format!("bundled: {e}"));
                    continue;
                }
            };
            seen.insert(manifest.id.clone());
            let mut bundled = Plugin {
                manifest,
                error: None,
                runtime: None,
                path: PathBuf::new(),
                bundled_source: Some(source),
                failed: false,
                waker: self.waker.clone(),
            };
            if enabled.contains(&bundled.manifest.id) {
                bundled.start(&self.data_root, &self.host);
            }
            self.entries.push(bundled);
        }
        let dir = directory();
        if let Err(e) = std::fs::create_dir_all(&dir) {
            self.errors.push(e.to_string());
            return;
        }
        let Ok(iter) = std::fs::read_dir(&dir) else {
            return;
        };
        let mut paths: Vec<_> = iter
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.is_dir())
            .collect();
        paths.sort();
        for path in paths {
            let result = (|| -> anyhow::Result<Manifest> {
                let text = read_limited(&entry_path(&path, "plugin.toml")?)?;
                let m: Manifest = toml::from_str(&text)?;
                anyhow::ensure!(
                    valid_id(&m.id),
                    "{}",
                    tr!("不正な ID", "Invalid ID", "无效的 ID",)
                );
                anyhow::ensure!(
                    m.api == API_VERSION,
                    "{}",
                    tr!(
                        "未対応の API バージョン: {}",
                        "Unsupported API version: {}",
                        "不支持的 API 版本：{}",
                        m.api
                    )
                );
                anyhow::ensure!(
                    seen.insert(m.id.clone()),
                    "{}",
                    tr!(
                        "ID が重複しています: {}",
                        "Duplicate ID: {}",
                        "ID 重复：{}",
                        m.id
                    )
                );
                Ok(m)
            })();
            match result {
                Ok(manifest) => {
                    let mut p = Plugin {
                        manifest,
                        error: None,
                        runtime: None,
                        path,
                        bundled_source: None,
                        failed: false,
                        waker: self.waker.clone(),
                    };
                    if enabled.contains(&p.manifest.id) {
                        p.start(&self.data_root, &self.host);
                    }
                    self.entries.push(p);
                }
                Err(e) => self.errors.push(format!("{}: {e:#}", path.display())),
            }
        }
        self.index = self
            .entries
            .iter()
            .enumerate()
            .map(|(i, p)| (p.manifest.id.clone(), i))
            .collect();
    }
    /// Set all discovered plugins at once without restarting those already running.
    pub fn set_all(&mut self, on: bool, enabled: &mut Vec<String>) {
        self.dirty();
        self.generation = self.generation.wrapping_add(1);
        self.revision = self.revision.wrapping_add(1);
        if on {
            for plugin in &mut self.entries {
                if !enabled.contains(&plugin.manifest.id) {
                    enabled.push(plugin.manifest.id.clone());
                }
                if !plugin.running() {
                    plugin.start(&self.data_root, &self.host);
                }
            }
        } else {
            for plugin in &mut self.entries {
                plugin.stop();
            }
            enabled.clear();
            self.selected = None;
        }
    }
    pub fn toggle(&mut self, id: &str, enabled: &mut Vec<String>) {
        self.dirty();
        self.generation = self.generation.wrapping_add(1);
        self.revision = self.revision.wrapping_add(1);
        if let Some(p) = self.entries.iter_mut().find(|p| p.manifest.id == id) {
            if enabled.iter().any(|v| v == id) {
                enabled.retain(|v| v != id);
                p.stop();
                // 止めたプラグインの設定ページは開いたままにしない
                if self.selected.as_deref() == Some(id) {
                    self.selected = None;
                }
            } else {
                enabled.push(id.to_owned());
                p.start(&self.data_root, &self.host);
            }
        }
    }
    pub fn command(&mut self, content: &str, channel: &str) -> bool {
        self.dirty();
        let Some(line) = content.strip_prefix('/') else {
            return false;
        };
        let (name, args) = line.split_once(' ').unwrap_or((line, ""));
        for p in &mut self.entries {
            let result = p.runtime.as_ref().map(|r| -> mlua::Result<Option<Value>> {
                r.reset_budget();
                let callbacks: Table = r.lua.globals().get("__plugin")?;
                let commands: Option<Table> = callbacks.get("commands")?;
                let Some(commands) = commands else {
                    return Ok(None);
                };
                match commands.get::<Value>(name)? {
                    Value::Function(f) => Ok(Some(f.call((args.to_owned(), channel.to_owned()))?)),
                    _ => Ok(None),
                }
            });
            match result {
                Some(Ok(Some(_))) => {
                    self.revision = self.revision.wrapping_add(1);
                    return true;
                }
                Some(Err(e)) => {
                    p.fail(e.to_string());
                    return true;
                }
                _ => (),
            }
        }
        false
    }
    /// (プラグイン id, 番号, 表示名, アイコン名)
    pub fn message_actions(&mut self) -> Vec<(String, usize, String, Option<String>)> {
        if let Some((rev, cached)) = &self.actions_cache {
            if *rev == self.revision {
                return cached.clone();
            }
        }
        let mut all = Vec::new();
        for p in &mut self.entries {
            if all.len() >= MAX_MESSAGE_ACTIONS {
                break;
            }
            let result =
                p.runtime
                    .as_ref()
                    .map(|r| -> mlua::Result<Vec<(usize, String, Option<String>)>> {
                        r.reset_budget();
                        let callbacks: Table = r.lua.globals().get("__plugin")?;
                        let actions: Option<Table> = callbacks.get("message_actions")?;
                        let Some(actions) = actions else {
                            return Ok(vec![]);
                        };
                        let mut items = Vec::new();
                        for (i, item) in actions.sequence_values::<Table>().take(20).enumerate() {
                            let item = item?;
                            items.push((
                                i + 1,
                                clip(&item.get::<String>("label")?, MAX_ACTION_LABEL_CHARS),
                                item.get::<Option<String>>("icon")?,
                            ));
                        }
                        Ok(items)
                    });
            match result {
                Some(Ok(items)) => all.extend(
                    items
                        .into_iter()
                        .map(|(i, label, icon)| (p.manifest.id.clone(), i, label, icon)),
                ),
                Some(Err(e)) => p.fail(e.to_string()),
                None => (),
            }
        }
        all.truncate(MAX_MESSAGE_ACTIONS);
        // 止まったプラグインがあれば revision が変わるので、次の呼び出しで作り直す
        self.settle();
        self.actions_cache = Some((self.revision, all.clone()));
        all
    }
    pub fn message_action(&mut self, id: &str, index: usize, m: &Message) {
        self.dirty();
        self.revision = self.revision.wrapping_add(1);
        if let Some(p) = self.entries.iter_mut().find(|p| p.manifest.id == id) {
            let result = p.runtime.as_ref().map(|r| -> mlua::Result<Value> {
                r.reset_budget();
                let callbacks: Table = r.lua.globals().get("__plugin")?;
                let actions: Table = callbacks.get("message_actions")?;
                let item: Table = actions.get(index)?;
                let f: Function = item.get("run")?;
                let message = message_table(&r.lua, m)?;
                f.call(message)
            });
            p.check(result);
        }
    }
    /// id のプラグインが動いているか（同梱プラグインの本体側の機能を切り替える）
    pub fn on(&self, id: &str) -> bool {
        match self.index.get(id) {
            Some(&i) => self
                .entries
                .get(i)
                .is_some_and(|p| p.manifest.id == id && p.running()),
            None => self
                .entries
                .iter()
                .any(|p| p.manifest.id == id && p.running()),
        }
    }
    /// 動いているプラグインの options()（_lib.lua の settings の値）。止まっていれば Null
    pub fn options(&self, id: &str) -> serde_json::Value {
        if !self.on(id) {
            return serde_json::Value::Null;
        }
        if let Some(v) = self.options_cache.borrow().get(id) {
            return v.clone();
        }
        let v = self
            .entries
            .iter()
            .find(|p| p.manifest.id == id)
            .and_then(|p| p.runtime.as_ref())
            .and_then(|r| r.call("options", ()).ok())
            .and_then(|v| {
                let r = self
                    .entries
                    .iter()
                    .find(|p| p.manifest.id == id)?
                    .runtime
                    .as_ref()?;
                r.lua.from_value::<serde_json::Value>(v).ok()
            })
            .unwrap_or(serde_json::Value::Null);
        self.options_cache
            .borrow_mut()
            .insert(id.to_owned(), v.clone());
        v
    }
    /// options の key を、全体を複製せずに読む（描画のたびに何度も呼ばれるため）
    fn with_option<R>(
        &self,
        id: &str,
        key: &str,
        f: impl FnOnce(Option<&serde_json::Value>) -> R,
    ) -> R {
        if !self.on(id) {
            return f(None);
        }
        if !self.options_cache.borrow().contains_key(id) {
            let _ = self.options(id);
        }
        f(self.options_cache.borrow().get(id).and_then(|v| v.get(key)))
    }
    /// 動いていれば options の key（真偽）。止まっていれば false
    pub fn opt_bool(&self, id: &str, key: &str) -> bool {
        self.with_option(id, key, |v| v.and_then(|v| v.as_bool()).unwrap_or(false))
    }
    pub fn opt_str(&self, id: &str, key: &str) -> String {
        self.with_option(id, key, |v| match v {
            Some(serde_json::Value::String(s)) => s.clone(),
            Some(serde_json::Value::Number(n)) => n.to_string(),
            _ => String::new(),
        })
    }
    pub fn opt_f64(&self, id: &str, key: &str) -> Option<f64> {
        self.with_option(id, key, |v| match v? {
            serde_json::Value::Number(n) => n.as_f64(),
            serde_json::Value::String(s) => s.trim().parse().ok(),
            _ => None,
        })
    }
    fn dirty(&self) {
        self.options_cache.borrow_mut().clear();
    }
    /// 実行中のエラーで止まったプラグインを表示に反映する（キャッシュを捨てて描き直させる）
    fn settle(&mut self) {
        let mut any = false;
        for p in &mut self.entries {
            if std::mem::take(&mut p.failed) {
                any = true;
                if self.stopped.len() < 20 && !self.stopped.contains(&p.manifest.name) {
                    self.stopped.push(p.manifest.name.clone());
                }
            }
        }
        if any {
            self.dirty();
            self.revision = self.revision.wrapping_add(1);
            self.actions_cache = None;
        }
    }
    /// エラーで止まったプラグインの名前（前回から増えた分）。表示の更新も済ませる
    pub fn take_stopped(&mut self) -> Vec<String> {
        self.settle();
        std::mem::take(&mut self.stopped)
    }
    /// 任意のフック（on_typing / on_presence / on_voice など）を、動いている全プラグインへ知らせる
    pub fn hook(&mut self, name: &str, args: &[serde_json::Value]) {
        for p in &mut self.entries {
            let result = p.runtime.as_ref().map(|r| -> mlua::Result<Value> {
                let callbacks: Table = r.lua.globals().get("__plugin")?;
                if !matches!(callbacks.raw_get::<Value>(name)?, Value::Function(_)) {
                    return Ok(Value::Nil);
                }
                let values = args
                    .iter()
                    .map(|a| r.lua.to_value(a))
                    .collect::<mlua::Result<Vec<_>>>()?;
                r.call(name, mlua::MultiValue::from_vec(values))
            });
            p.check(result);
        }
    }
    /// プラグインの関数を呼んで JSON で受け取る（id のプラグインだけ）。動いていなければ None
    pub fn query(
        &mut self,
        id: &str,
        name: &str,
        args: &[serde_json::Value],
    ) -> Option<serde_json::Value> {
        let p = self.entries.iter_mut().find(|p| p.manifest.id == id)?;
        let r = p.runtime.as_ref()?;
        let result = (|| -> mlua::Result<serde_json::Value> {
            let values = args
                .iter()
                .map(|a| r.lua.to_value(a))
                .collect::<mlua::Result<Vec<_>>>()?;
            let v = r.call(name, mlua::MultiValue::from_vec(values))?;
            r.lua.from_value(v)
        })();
        match result {
            Ok(v) => Some(v),
            Err(e) => {
                p.fail(e.to_string());
                None
            }
        }
    }
    pub fn translator_enabled(&self) -> bool {
        self.on("translator")
    }
    pub fn translation_outgoing(&mut self) -> Result<Option<String>, String> {
        let p = self
            .entries
            .iter_mut()
            .find(|p| p.manifest.id == "translator")
            .ok_or(tr!(
                "翻訳プラグインが見つかりません",
                "Translation plugin not found",
                "找不到翻译插件"
            ))?;
        let result = p
            .runtime
            .as_ref()
            .ok_or(tr!(
                "翻訳プラグインが停止しています",
                "The translation plugin is stopped",
                "翻译插件已停止"
            ))?
            .call("translation_outgoing", ());
        match result {
            Ok(Value::String(value)) => {
                Ok(Some(value.to_str().map_err(|e| e.to_string())?.to_owned()))
            }
            Ok(Value::Nil) => Ok(None),
            Ok(_) => {
                let error = tr!(
                    "翻訳設定の読み取りに失敗しました",
                    "Couldn't read the translation settings",
                    "读取翻译设置失败"
                )
                .to_string();
                p.fail(error.clone());
                Err(error)
            }
            Err(error) => {
                let error = error.to_string();
                p.fail(error.clone());
                Err(error)
            }
        }
    }
    pub fn translation_target(&mut self) -> String {
        let Some(p) = self
            .entries
            .iter_mut()
            .find(|p| p.manifest.id == "translator")
        else {
            return "ja".into();
        };
        let result = p.runtime.as_ref().map(|r| r.call("translation_target", ()));
        match result {
            Some(Ok(Value::String(s))) => s
                .to_str()
                .ok()
                .and_then(|s| crate::translator::language(&s))
                .unwrap_or_else(|| "ja".into()),
            Some(Err(e)) => {
                p.fail(e.to_string());
                "ja".into()
            }
            _ => "ja".into(),
        }
    }
    pub fn voice_panel(
        &mut self,
        guild_id: Option<&str>,
        channel_id: &str,
        elapsed_seconds: u64,
    ) -> Vec<String> {
        let mut labels = Vec::new();
        for p in &mut self.entries {
            let result = p.runtime.as_ref().map(|r| -> mlua::Result<Value> {
                let call = r.lua.create_table()?;
                call.set("guild_id", guild_id)?;
                call.set("channel_id", channel_id)?;
                call.set("elapsed_seconds", elapsed_seconds)?;
                r.call("voice_panel", call)
            });
            match result {
                Some(Ok(Value::String(_))) if labels.len() >= MAX_VOICE_LINES => (),
                Some(Ok(Value::String(s))) => match s.to_str() {
                    Ok(value) if value.len() <= 256 => labels.push(value.to_owned()),
                    Ok(_) => p.fail(
                        tr!(
                            "voice_panel の文字列が長すぎます",
                            "The voice_panel string is too long",
                            "voice_panel 字符串过长"
                        )
                        .into(),
                    ),
                    Err(e) => p.fail(e.to_string()),
                },
                Some(Ok(Value::Nil)) | None => (),
                Some(Ok(_)) => p.fail(
                    tr!(
                        "voice_panel は文字列か nil を返してください",
                        "voice_panel must return a string or nil",
                        "voice_panel 必须返回字符串或 nil"
                    )
                    .into(),
                ),
                Some(Err(e)) => p.fail(e.to_string()),
            }
        }
        labels
    }
    /// 受信したメッセージを知らせる。表示が変わりうるとき（on_display を持つ、または設定が変わった）だけ
    /// revision を上げる。たくさん有効でも、受信のたびに全行を作り直さないため
    pub fn event(&mut self, m: &Message) {
        let mut changed = false;
        for p in &mut self.entries {
            let Some(r) = p.runtime.as_ref() else {
                continue;
            };
            let result = (|| -> mlua::Result<bool> {
                let callbacks: Table = r.lua.globals().get("__plugin")?;
                if !matches!(
                    callbacks.raw_get::<Value>("on_message")?,
                    Value::Function(_)
                ) {
                    return Ok(false);
                }
                let options = || -> mlua::Result<serde_json::Value> {
                    let v = r.call("options", ())?;
                    Ok(r.lua.from_value(v).unwrap_or(serde_json::Value::Null))
                };
                let before = options()?;
                r.message("on_message", m)?;
                let display = matches!(
                    callbacks.raw_get::<Value>("on_display")?,
                    Value::Function(_)
                );
                Ok(display || options()? != before)
            })();
            match result {
                Ok(c) => changed |= c,
                Err(e) => p.fail(e.to_string()),
            }
        }
        if changed {
            self.dirty();
            self.revision = self.revision.wrapping_add(1);
        }
    }
    /// 削除（on_delete）・編集（on_edit、2つ目の引数は編集後の本文）をプラグインへ知らせる。m は削除・編集前のメッセージ
    pub fn message_hook(&mut self, name: &str, m: &Message, extra: Option<&str>) {
        for p in &mut self.entries {
            let result = p.runtime.as_ref().map(|r| r.message_with(name, m, extra));
            p.check(result);
        }
    }
    /// MessageLogger が動いていれば、その記録の条件
    pub fn logger_options(&mut self) -> Option<crate::message_log::Options> {
        let p = self
            .entries
            .iter_mut()
            .find(|p| p.manifest.id == "message-logger")?;
        let result = p.runtime.as_ref()?.call("logger_options", ());
        match result {
            Ok(Value::Table(t)) => Some(crate::message_log::Options {
                ignore_self: t.get("ignore_self").unwrap_or(false),
                ignore_bots: t.get("ignore_bots").unwrap_or(false),
            }),
            Ok(_) => Some(Default::default()),
            Err(e) => {
                p.fail(e.to_string());
                None
            }
        }
    }
    pub fn logger_enabled(&self) -> bool {
        self.on("message-logger")
    }
    /// before_send: 送る前に止める理由（文字列）を返したプラグインがあれば、その理由
    pub fn check_send(&mut self, content: &str, channel: &str) -> Option<String> {
        for p in &mut self.entries {
            let result = p
                .runtime
                .as_ref()
                .map(|r| r.call("before_send", (content.to_owned(), channel.to_owned())));
            match result {
                Some(Ok(Value::String(s))) => {
                    if let Ok(s) = s.to_str() {
                        return Some(s.to_owned());
                    }
                }
                Some(Err(e)) => p.fail(e.to_string()),
                _ => (),
            }
        }
        None
    }
    pub fn transform(&mut self, name: &str, content: &str, channel: &str) -> String {
        let mut content = content.to_owned();
        for p in &mut self.entries {
            let result = p
                .runtime
                .as_ref()
                .map(|r| r.call(name, (content.clone(), channel.to_owned())));
            match result {
                Some(Ok(Value::String(s))) => match s.to_str() {
                    Ok(s) => content = s.to_owned(),
                    Err(e) => p.fail(e.to_string()),
                },
                Some(Ok(Value::Nil)) | None => (),
                Some(Ok(_)) => p.fail(tr!(
                    "{name} は文字列か nil を返してください",
                    "{name} must return a string or nil",
                    "{name} 必须返回字符串或 nil",
                )),
                Some(Err(e)) => p.fail(e.to_string()),
            }
        }
        content
    }
    pub fn page(&mut self, id: &str) -> Vec<PageItem> {
        let Some(p) = self.entries.iter_mut().find(|p| p.manifest.id == id) else {
            return vec![];
        };
        let result = p.runtime.as_ref().map(|r| r.call("page", ()));
        match result {
            Some(Ok(Value::Table(t))) => t
                .sequence_values::<Table>()
                .take(100)
                .filter_map(Result::ok)
                .map(|v| PageItem {
                    kind: v.get("kind").unwrap_or_default(),
                    label: clip(
                        &v.get::<String>("label").unwrap_or_default(),
                        MAX_PAGE_LABEL_CHARS,
                    ),
                    action: v.get("action").unwrap_or_default(),
                    enabled: v.get("enabled").unwrap_or(false),
                })
                .collect(),
            Some(Err(e)) => {
                p.fail(e.to_string());
                vec![]
            }
            _ => vec![],
        }
    }
    pub fn page_action(
        &mut self,
        id: &str,
        action: &str,
        values: std::collections::HashMap<String, String>,
    ) {
        self.dirty();
        self.revision = self.revision.wrapping_add(1);
        if let Some(p) = self.entries.iter_mut().find(|p| p.manifest.id == id) {
            let result = p.runtime.as_ref().map(|r| {
                r.lua
                    .to_value(&values)
                    .and_then(|v| r.call("on_action", (action.to_owned(), v)))
            });
            p.check(result);
        }
    }
    pub fn tick(&mut self) -> (Vec<Action>, Option<Duration>) {
        let mut actions = Vec::new();
        let mut next: Option<Duration> = None;
        for p in &mut self.entries {
            let Some(r) = &p.runtime else { continue };
            let now = Instant::now();
            let mut due = Vec::new();
            {
                let mut timers = r.timers.borrow_mut();
                let mut i = 0;
                while i < timers.len() {
                    if timers[i].at <= now {
                        due.push(timers.remove(i));
                    } else {
                        let d = timers[i].at.saturating_duration_since(now);
                        next = Some(next.map_or(d, |n| n.min(d)));
                        i += 1;
                    }
                }
            }
            let mut error = None;
            if !due.is_empty() {
                self.revision = self.revision.wrapping_add(1);
                self.options_cache.borrow_mut().clear();
            }
            for timer in due {
                r.reset_budget();
                let result = r
                    .lua
                    .registry_value::<Function>(&timer.callback)
                    .and_then(|f| f.call::<()>(()));
                if let Err(e) = result {
                    error = Some(e.to_string());
                    break;
                }
            }
            if let Some(e) = error {
                p.fail(e);
            } else {
                actions.extend(r.queue.borrow_mut().drain(..));
                for timer in r.timers.borrow().iter() {
                    let delay = timer.at.saturating_duration_since(Instant::now());
                    next = Some(next.map_or(delay, |n| n.min(delay)));
                }
            }
        }
        (actions, next)
    }
}
impl Drop for Plugins {
    fn drop(&mut self) {
        for plugin in &mut self.entries {
            plugin.stop();
        }
    }
}
impl Plugin {
    fn start(&mut self, data_dir: &Path, host: &Rc<RefCell<HostInfo>>) {
        self.error = None;
        let runtime = match self.bundled_source {
            Some(source) => Runtime::with_host(
                &self.manifest,
                &format!("{LIB}\n{source}"),
                data_dir,
                host.clone(),
            ),
            None => entry_path(&self.path, &self.manifest.entry)
                .and_then(|path| read_limited(&path))
                .map_err(|e| mlua::Error::runtime(e.to_string()))
                .and_then(|script| {
                    Runtime::with_host(&self.manifest, &script, data_dir, host.clone())
                }),
        };
        match runtime {
            Ok(r) => self.runtime = Some(r),
            Err(e) => self.error = Some(clip(&format!("{e:#}"), MAX_ERROR_CHARS)),
        }
    }
    fn stop(&mut self) {
        if let Some(r) = self.runtime.take() {
            let _ = r.call("stop", ());
        }
    }
    /// 実行中のエラーで止める。本体側の機能は on() が false になるので標準の表示に戻る
    fn fail(&mut self, e: String) {
        self.runtime = None;
        self.error = Some(clip(&e, MAX_ERROR_CHARS));
        self.failed = true;
        self.waker.wake();
    }
    fn check(&mut self, result: Option<mlua::Result<Value>>) {
        if let Some(Err(e)) = result {
            self.fail(e.to_string());
        }
    }
    pub fn has_page(&self) -> bool {
        self.runtime
            .as_ref()
            .is_some_and(|runtime| runtime.has_page)
    }
    pub fn running(&self) -> bool {
        self.runtime.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    static COUNTER: AtomicUsize = AtomicUsize::new(0);
    fn runtime(script: &str) -> (Runtime, PathBuf) {
        let dir = std::env::temp_dir().join(format!(
            "riscord-plugin-test-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("main.lua"), script).unwrap();
        let m = Manifest {
            id: "test".into(),
            name: "test".into(),
            version: "1".into(),
            description: String::new(),
            api: 1,
            entry: "main.lua".into(),
            description_en: String::new(),
            description_zh: String::new(),
            bd: false,
        };
        (Runtime::new(&m, &dir, &dir).unwrap(), dir)
    }
    #[test]
    fn restricted_lua_and_command_queue() {
        let (r, dir) = runtime("assert(io == nil and os == nil and package == nil and require == nil and pcall == nil); return {start=function() riscord.send_message('123', 'hello') end}");
        assert!(matches!(&r.queue.borrow()[0], Action::Send(c, s) if c == "123" && s == "hello"));
        std::fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn infinite_loop_is_interrupted() {
        let (r, dir) = runtime("return { on_send = function() while true do end end }");
        assert!(r.call("on_send", ("x", "1")).is_err());
        std::fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn plugin_can_transform_and_schedule() {
        let (r, dir) = runtime("return { start=function() riscord.set_timeout(100, function() riscord.send_message('1', 'timer') end) end, on_send=function(s) return s .. '!' end }");
        let Value::String(s) = r.call("on_send", ("hello", "1")).unwrap() else {
            panic!()
        };
        assert_eq!(s.to_str().unwrap(), "hello!");
        assert_eq!(r.timers.borrow().len(), 1);
        std::fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn budget_is_reset_between_calls() {
        let (r, dir) = runtime(
            "return { on_send=function(s) local n=0 for i=1,1000 do n=n+i end return s end }",
        );
        for _ in 0..1000 {
            assert!(r.call("on_send", ("ok", "1")).is_ok());
        }
        std::fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn sample_loads_and_builds_page() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/plugins/hello");
        let manifest: Manifest =
            toml::from_str(&std::fs::read_to_string(dir.join("plugin.toml")).unwrap()).unwrap();
        let r = Runtime::new(&manifest, &dir, &dir).unwrap();
        let Value::Table(page) = r.call("page", ()).unwrap() else {
            panic!()
        };
        assert_eq!(page.raw_len(), 3);
    }
    #[test]
    fn set_all_starts_stops_and_preserves_running_plugin_state() {
        let mut plugins = Plugins::default();
        for id in ["first", "second"] {
            let manifest = Manifest {
                id: id.into(),
                name: id.into(),
                version: "1".into(),
                description: String::new(),
                api: 1,
                entry: "main.lua".into(),
                description_en: String::new(),
                description_zh: String::new(),
                bd: false,
            };
            plugins.entries.push(Plugin { manifest, error: None, runtime: None, path: PathBuf::new(), failed: false, waker: Default::default(), bundled_source: Some("local count = 0; return {start=function() count=count+1 end, page=function() return count end}") });
        }
        let mut enabled = vec!["missing".to_string()];
        plugins.set_all(true, &mut enabled);
        assert!(plugins.entries.iter().all(Plugin::running));
        assert_eq!(enabled, ["missing", "first", "second"]);
        plugins.set_all(true, &mut enabled);
        assert_eq!(enabled, ["missing", "first", "second"]);
        for plugin in &plugins.entries {
            let value = plugin.runtime.as_ref().unwrap().call("page", ()).unwrap();
            assert!(matches!(value, Value::Integer(1)));
        }
        plugins.selected = Some("first".into());
        plugins.set_all(false, &mut enabled);
        assert!(enabled.is_empty());
        assert!(plugins.entries.iter().all(|plugin| !plugin.running()));
        assert!(plugins.selected.is_none());
    }
    #[test]
    fn bundled_call_timer_renders_and_can_be_disabled() {
        let manifest = Manifest {
            id: "call-time-counter".into(),
            name: "CallTimeCounter".into(),
            version: "1".into(),
            description: String::new(),
            api: 1,
            entry: "main.lua".into(),
            description_en: String::new(),
            description_zh: String::new(),
            bd: false,
        };
        let mut plugins = Plugins::default();
        plugins.entries.push(Plugin {
            manifest,
            error: None,
            runtime: None,
            path: PathBuf::new(),
            bundled_source: Some(include_str!("../plugins/call-time-counter/main.lua")),
            failed: false,
            waker: Default::default(),
        });
        let mut enabled = Vec::new();
        plugins.toggle("call-time-counter", &mut enabled);
        assert_eq!(
            plugins.voice_panel(None, "dm", 0),
            vec!["通話時間: 00:00:00"]
        );
        assert_eq!(
            plugins.voice_panel(Some("guild"), "voice", 90061),
            vec!["通話時間: 25:01:01"]
        );
        plugins.toggle("call-time-counter", &mut enabled);
        assert!(plugins.voice_panel(None, "dm", 5).is_empty());
        assert!(enabled.is_empty());
    }
    #[test]
    fn broken_voice_panel_does_not_stop_other_plugins() {
        let mut plugins = Plugins::default();
        for (id, source) in [
            (
                "broken",
                "return {voice_panel=function() error('failure') end}",
            ),
            (
                "ok",
                "return {voice_panel=function(call) return call.channel_id end}",
            ),
        ] {
            let manifest = Manifest {
                id: id.into(),
                name: id.into(),
                version: "1".into(),
                description: String::new(),
                api: 1,
                entry: "main.lua".into(),
                description_en: String::new(),
                description_zh: String::new(),
                bd: false,
            };
            let runtime = Runtime::from_source(&manifest, source, Path::new("")).unwrap();
            plugins.entries.push(Plugin {
                manifest,
                error: None,
                runtime: Some(runtime),
                path: PathBuf::new(),
                bundled_source: None,
                failed: false,
                waker: Default::default(),
            });
        }
        assert_eq!(plugins.voice_panel(None, "channel", 0), vec!["channel"]);
        assert!(plugins.entries[0].error.is_some());
        assert!(!plugins.entries[0].running());
        assert!(plugins.entries[1].running());
    }
    /// 同梱プラグインをすべて読み込み、ページを作り、主なフックを呼んでも止まらない
    #[test]
    fn every_bundled_plugin_starts_and_builds_page() {
        let dir = std::env::temp_dir().join(format!("riscord-bundled-test-{}", std::process::id()));
        let mut plugins = Plugins::default();
        plugins.data_root = dir.clone();
        plugins.reload(&[]);
        let ids: Vec<String> = plugins
            .entries
            .iter()
            .filter(|p| p.bundled_source.is_some())
            .map(|p| p.manifest.id.clone())
            .collect();
        assert!(ids.len() >= 3);
        let mut enabled = Vec::new();
        plugins.set_all(true, &mut enabled);
        for p in plugins
            .entries
            .iter()
            .filter(|p| p.bundled_source.is_some())
        {
            assert!(p.running(), "{}: {:?}", p.manifest.id, p.error);
            assert!(!p.manifest.description_en.is_empty(), "{}", p.manifest.id);
            assert!(!p.manifest.description_zh.is_empty(), "{}", p.manifest.id);
        }
        for id in &ids {
            let _ = plugins.page(id);
            let _ = plugins.options(id);
        }
        let m: Message = serde_json::from_value(serde_json::json!({
            "id": "1", "channel_id": "2", "author": {"id": "3", "username": "u"},
            "content": "hello https://example.com/?utm_source=x", "timestamp": "2026-01-01T00:00:00+00:00"
        }))
        .unwrap();
        plugins.event(&m);
        plugins.message_hook("on_delete", &m, None);
        plugins.message_hook("on_edit", &m, Some("edited"));
        let _ = plugins.transform("on_send", "hello world", "2");
        let _ = plugins.transform("on_display", "hello world", "2");
        plugins.hook("on_typing", &["2".into(), "3".into(), "u".into()]);
        plugins.hook(
            "on_presence",
            &[
                "3".into(),
                "offline".into(),
                "online".into(),
                "".into(),
                "u".into(),
            ],
        );
        plugins.hook(
            "on_voice",
            &[
                "3".into(),
                "".into(),
                "9".into(),
                "".into(),
                false.into(),
                false.into(),
                "u".into(),
            ],
        );
        let _ = plugins.message_actions();
        let _ = plugins.tick();
        for p in plugins
            .entries
            .iter()
            .filter(|p| p.bundled_source.is_some())
        {
            assert!(p.running(), "{}: {:?}", p.manifest.id, p.error);
        }
        let _ = std::fs::remove_dir_all(dir);
    }
    /// 同梱プラグインを 1 つだけ動かす（保存先は一時フォルダ）
    fn only(id: &str) -> Plugins {
        let dir = std::env::temp_dir().join(format!(
            "riscord-only-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let mut plugins = Plugins::default();
        plugins.data_root = dir;
        plugins.reload(&[id.to_string()]);
        assert!(
            plugins.on(id),
            "{id}: {:?}",
            plugins
                .entries
                .iter()
                .find(|p| p.manifest.id == id)
                .map(|p| &p.error)
        );
        plugins
    }
    fn send(p: &mut Plugins, s: &str) -> String {
        p.transform("on_send", s, "1")
    }
    #[test]
    fn bundled_send_transforms() {
        let mut p = only("link-cleaner");
        assert_eq!(
            send(
                &mut p,
                "see https://example.com/a?utm_source=x&id=3&fbclid=y#top ok"
            ),
            "see https://example.com/a?id=3#top ok"
        );
        assert_eq!(
            send(&mut p, "https://youtu.be/abc?si=zzz"),
            "https://youtu.be/abc"
        );
        let mut p = only("link-converter");
        assert_eq!(
            send(&mut p, "https://x.com/a/status/1"),
            "https://fxtwitter.com/a/status/1"
        );
        assert_eq!(
            send(&mut p, "https://instagram.com/p/1"),
            "https://instagram.com/p/1"
        );
        let mut p = only("write-uppercase");
        assert_eq!(send(&mut p, "hello. world? yes"), "Hello. World? Yes");
        assert_eq!(
            send(&mut p, "https://example.com/Path. hello"),
            "https://example.com/Path. Hello"
        );
        assert_eq!(
            send(
                &mut p,
                "hello. https://example.com/a\nhttp://example.org/b\nworld"
            ),
            "Hello. https://example.com/a\nhttp://example.org/b\nWorld"
        );
        assert_eq!(send(&mut p, "/cmd x"), "/cmd x");
        assert_eq!(send(&mut p, "hello `code`"), "hello `code`");
        let mut p = only("chat-aliases");
        assert_eq!(send(&mut p, "brb soon"), "be right back soon");
        assert_eq!(send(&mut p, "brbx"), "brbx");
        let mut p = only("unicode-emojis");
        assert_eq!(send(&mut p, "hi :) <3"), "hi 🙂 ❤️");
        let mut p = only("better-formatting-redux");
        assert_eq!(send(&mut p, "a [fw]ab[/fw] [up]ab[/up]"), "a ａｂ qɐ");
        let mut p = only("chat-filter");
        p.page_action(
            "chat-filter",
            "__save",
            [("censor".to_string(), "bad".to_string())].into(),
        );
        assert_eq!(p.transform("on_display", "so BAD", "1"), "so ***");
        let mut p = only("hold-your-tongue");
        p.page_action(
            "hold-your-tongue",
            "__save",
            [("keywords".to_string(), "secret".to_string())].into(),
        );
        assert!(p.check_send("a secret", "1").is_some());
        assert!(p.check_send("a secret", "1").is_none());
        assert!(p.check_send("fine", "1").is_none());
        let mut p = only("replace-timestamps");
        let out = send(&mut p, "meet at 18:30 ok");
        assert!(
            out.starts_with("meet at <t:") && out.ends_with(":t> ok"),
            "{out}"
        );
        assert_eq!(send(&mut p, "score 2:1"), "score 2:1");
        let mut p = only("insert-timestamps");
        assert!(p.command("/ts 2026-01-01 00:00 R", "1"));
        let (actions, _) = p.tick();
        assert!(
            matches!(&actions[0], Action::Insert(s) if s.starts_with("<t:") && s.ends_with(":R>")),
            "{actions:?}"
        );
        let mut p = only("to-do-list");
        assert!(p.command("/todo !buy milk", "1"));
        assert!(p.page("to-do-list").iter().any(|i| i.label == "! buy milk"));
    }
    /// スクリプトから直接プラグインを作る（id = 名前）
    fn scripted(list: &[(&str, &str)]) -> (Plugins, Vec<String>) {
        let mut plugins = Plugins::default();
        let mut enabled = Vec::new();
        for (id, source) in list {
            let manifest = Manifest {
                id: (*id).into(),
                name: (*id).into(),
                version: "1".into(),
                description: String::new(),
                api: 1,
                entry: "main.lua".into(),
                description_en: String::new(),
                description_zh: String::new(),
                bd: false,
            };
            let runtime = Runtime::from_source(&manifest, source, Path::new("")).unwrap();
            plugins.entries.push(Plugin {
                manifest,
                error: None,
                runtime: Some(runtime),
                path: PathBuf::new(),
                bundled_source: None,
                failed: false,
                waker: Default::default(),
            });
            enabled.push((*id).to_string());
        }
        plugins.index = plugins
            .entries
            .iter()
            .enumerate()
            .map(|(i, p)| (p.manifest.id.clone(), i))
            .collect();
        (plugins, enabled)
    }
    fn message(content: &str) -> Message {
        serde_json::from_value(serde_json::json!({
            "id": "1", "channel_id": "2", "author": {"id": "3", "username": "u"},
            "content": content, "timestamp": "2026-01-01T00:00:00+00:00"
        }))
        .unwrap()
    }
    #[test]
    fn clip_counts_characters_in_every_language() {
        assert_eq!(clip("hello", 5), "hello");
        assert_eq!(clip("hello world", 5), "hello…");
        assert_eq!(clip("日本語のエラーです", 4), "日本語の…");
        assert_eq!(clip("插件因错误已停止", 2), "插件…");
        assert_eq!(clip("", 3), "");
    }
    /// 描画中（on_display）に落ちたプラグインは止まり、表示のキャッシュが作り直され、名前が 1 回だけ届く
    #[test]
    fn failing_plugin_stops_and_refreshes_display() {
        let long = "x".repeat(5000);
        let (mut plugins, _) = scripted(&[
            (
                "broken",
                &format!("return {{on_display=function() error('{long}') end}}"),
            ),
            ("ok", "return {on_display=function(s) return s .. '!' end}"),
        ]);
        let before = plugins.revision;
        assert_eq!(plugins.transform("on_display", "hi", "1"), "hi!");
        assert!(!plugins.on("broken"));
        assert!(plugins.on("ok"));
        let error = plugins.entries[0].error.clone().unwrap();
        assert!(
            error.chars().count() <= MAX_ERROR_CHARS + 1,
            "{}",
            error.len()
        );
        assert_eq!(plugins.take_stopped(), vec!["broken".to_string()]);
        assert_ne!(plugins.revision, before);
        let after = plugins.revision;
        assert!(plugins.take_stopped().is_empty());
        assert_eq!(plugins.revision, after);
    }
    /// 受信のたびに revision を上げるのは、表示が変わりうるときだけ
    #[test]
    fn message_event_bumps_revision_only_when_display_can_change() {
        let (mut plugins, _) = scripted(&[
            ("quiet", "local n = 0; return {on_message=function() n = n + 1 end, options=function() return {} end}"),
            ("plain", "return {}"),
        ]);
        let before = plugins.revision;
        for _ in 0..100 {
            plugins.event(&message("hello"));
        }
        assert_eq!(plugins.revision, before);
        let (mut plugins, _) = scripted(&[(
            "counter",
            "local s = {n = 0}; return {on_message=function() s.n = s.n + 1 end, options=function() return s end}",
        )]);
        assert_eq!(plugins.opt_f64("counter", "n"), Some(0.0));
        plugins.event(&message("hello"));
        assert_eq!(plugins.opt_f64("counter", "n"), Some(1.0));
        let (mut plugins, _) = scripted(&[(
            "display",
            "return {on_message=function() end, on_display=function(s) return s end}",
        )]);
        let before = plugins.revision;
        plugins.event(&message("hello"));
        assert_ne!(plugins.revision, before);
    }
    /// 操作が多すぎても上限で止め、長い名前は切り、同じ状態なら Lua を呼び直さない
    #[test]
    fn message_actions_are_capped_and_cached() {
        let label = "長い名前".repeat(50);
        let many = format!(
            "local calls = 0; local a = {{}}; for i = 1, 20 do a[i] = {{label = '{label}', run = function() end}} end; return {{message_actions = a, page = function() return {{}} end}}"
        );
        let list: Vec<(String, String)> = (0..5).map(|i| (format!("p{i}"), many.clone())).collect();
        let list: Vec<(&str, &str)> = list.iter().map(|(a, b)| (a.as_str(), b.as_str())).collect();
        let (mut plugins, _) = scripted(&list);
        let actions = plugins.message_actions();
        assert_eq!(actions.len(), MAX_MESSAGE_ACTIONS);
        assert!(actions
            .iter()
            .all(|(_, _, l, _)| l.chars().count() <= MAX_ACTION_LABEL_CHARS + 1));
        // キャッシュから返る（中身は同じ）
        assert_eq!(plugins.message_actions().len(), MAX_MESSAGE_ACTIONS);
        let mut enabled = vec!["p0".to_string()];
        plugins.toggle("p0", &mut enabled);
        assert!(!plugins.on("p0"));
        assert!(plugins.message_actions().iter().all(|(id, ..)| id != "p0"));
    }
    /// 同梱プラグインの全 ON / 全 OFF と 1 つずつの切り替えを繰り返しても、状態がずれない
    #[test]
    fn toggling_all_bundled_plugins_repeatedly_stays_consistent() {
        let dir = std::env::temp_dir().join(format!("riscord-toggle-test-{}", std::process::id()));
        let mut plugins = Plugins::default();
        plugins.data_root = dir.clone();
        plugins.reload(&[]);
        let ids: Vec<String> = plugins
            .entries
            .iter()
            .filter(|p| p.bundled_source.is_some())
            .map(|p| p.manifest.id.clone())
            .collect();
        let mut enabled = Vec::new();
        for round in 0..5 {
            plugins.set_all(true, &mut enabled);
            for id in &ids {
                assert!(plugins.on(id), "round {round}: {id}");
            }
            // 受信・表示・操作を何度も呼んでも止まらない
            for i in 0..50 {
                plugins.event(&message(&format!(
                    "hello {i} https://example.com/?utm_source=x"
                )));
                let _ = plugins.transform("on_display", "hello world", "2");
                let _ = plugins.message_actions();
                let _ = plugins.tick();
            }
            assert!(plugins.take_stopped().is_empty());
            plugins.selected = ids.first().cloned();
            for id in &ids {
                plugins.toggle(id, &mut enabled);
                assert!(!plugins.on(id), "{id}");
                plugins.toggle(id, &mut enabled);
                assert!(plugins.on(id), "{id}");
            }
            plugins.toggle(&ids[0], &mut enabled);
            assert!(plugins.selected.is_none());
            plugins.set_all(false, &mut enabled);
            assert!(enabled.is_empty());
            assert!(plugins.entries.iter().all(|p| !p.running()));
            assert!(plugins.message_actions().is_empty());
        }
        let _ = std::fs::remove_dir_all(dir);
    }
    #[test]
    fn rejects_paths_outside_plugin() {
        let dir = std::env::temp_dir();
        assert!(entry_path(&dir, "../").is_err());
        assert!(!valid_id("../escape"));
    }
}
