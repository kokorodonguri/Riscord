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

#[derive(Clone, Deserialize)]
pub struct Manifest {
    pub id: String,
    pub name: String,
    pub version: String,
    #[serde(default)]
    pub description: String,
    pub api: u32,
    pub entry: String,
}
#[derive(Clone)]
pub enum Action {
    Send(String, String),
}
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
}
#[derive(Default)]
pub struct Plugins {
    pub entries: Vec<Plugin>,
    pub errors: Vec<String>,
    pub selected: Option<String>,
    pub revision: u64,
    pub generation: u64,
    /// プラグインが riscord.save で書くフォルダ（アカウントごと）
    pub data_root: PathBuf,
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
        "ファイルが大きすぎるか通常のファイルではありません"
    );
    Ok(std::fs::read_to_string(path)?)
}
fn entry_path(dir: &Path, entry: &str) -> anyhow::Result<PathBuf> {
    let root = dir.canonicalize()?;
    let path = dir.join(entry).canonicalize()?;
    anyhow::ensure!(
        path.starts_with(root),
        "エントリポイントがプラグインフォルダの外です"
    );
    Ok(path)
}

impl Runtime {
    fn new(manifest: &Manifest, dir: &Path, data_dir: &Path) -> mlua::Result<Self> {
        let path =
            entry_path(dir, &manifest.entry).map_err(|e| mlua::Error::runtime(e.to_string()))?;
        let script = read_limited(&path).map_err(|e| mlua::Error::runtime(e.to_string()))?;
        Self::from_source(manifest, &script, data_dir)
    }
    fn from_source(manifest: &Manifest, script: &str, data_dir: &Path) -> mlua::Result<Self> {
        let lua = Lua::new_with(
            StdLib::TABLE | StdLib::STRING | StdLib::MATH | StdLib::UTF8,
            LuaOptions::default(),
        )?;
        lua.set_memory_limit(16 * 1024 * 1024)?;
        lua.set_hook(
            HookTriggers::new().every_nth_instruction(100_000),
            |_, _| Err(mlua::Error::runtime("実行時間の制限を超えました")),
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
                    return Err(mlua::Error::runtime(
                        "無効な送信内容、または送信待ちが上限です",
                    ));
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
                    return Err(mlua::Error::runtime("タイマーの上限です"));
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
                    return Err(mlua::Error::runtime("保存データが大きすぎます"));
                }
                std::fs::create_dir_all(&data_dir).map_err(mlua::Error::external)?;
                std::fs::write(&data_file, bytes).map_err(mlua::Error::external)
            })?,
        )?;
        api.set("api_version", API_VERSION)?;
        lua.globals().set("riscord", api)?;
        let callbacks: Table = lua.load(script).set_name(&manifest.entry).eval()?;
        lua.globals().set("__plugin", callbacks)?;
        let runtime = Self { lua, queue, timers };
        runtime.call("start", ())?;
        Ok(runtime)
    }
    fn reset_budget(&self) {
        self.lua.set_hook(
            HookTriggers::new().every_nth_instruction(100_000),
            |_, _| Err(mlua::Error::runtime("実行時間の制限を超えました")),
        );
    }
    fn call<A: mlua::IntoLuaMulti>(&self, name: &str, args: A) -> mlua::Result<Value> {
        self.reset_budget();
        let t: Table = self.lua.globals().get("__plugin")?;
        match t.get::<Value>(name)? {
            Value::Function(f) => f.call(args),
            Value::Nil => Ok(Value::Nil),
            _ => Err(mlua::Error::runtime(format!(
                "{name} は関数である必要があります"
            ))),
        }
    }
    fn message(&self, name: &str, m: &Message) -> mlua::Result<Value> {
        self.message_with(name, m, None)
    }
    fn message_with(&self, name: &str, m: &Message, extra: Option<&str>) -> mlua::Result<Value> {
        let t = self.lua.create_table()?;
        t.set("id", m.id.clone())?;
        t.set("channel_id", m.channel_id.clone())?;
        t.set("guild_id", m.guild_id.clone())?;
        t.set("author_id", m.author.id.clone())?;
        t.set("content", m.content.clone())?;
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
        let manifest = Manifest {
            id: "call-time-counter".into(),
            name: "CallTimeCounter".into(),
            version: "1.0.0".into(),
            description: "ボイス通話の経過時間を表示します（同梱）".into(),
            api: API_VERSION,
            entry: "main.lua".into(),
        };
        let mut bundled = Plugin {
            manifest,
            error: None,
            runtime: None,
            path: PathBuf::new(),
            bundled_source: Some(include_str!("../plugins/call-time-counter/main.lua")),
        };
        if enabled.contains(&bundled.manifest.id) {
            bundled.start(&self.data_root);
        }
        self.entries.push(bundled);
        let manifest = Manifest {
            id: "translator".into(),
            name: "Translator".into(),
            version: "1.0.0".into(),
            description: "メッセージを Google 翻訳へ送信して翻訳します（同梱）".into(),
            api: API_VERSION,
            entry: "main.lua".into(),
        };
        let mut bundled = Plugin {
            manifest,
            error: None,
            runtime: None,
            path: PathBuf::new(),
            bundled_source: Some(include_str!("../plugins/translator/main.lua")),
        };
        if enabled.contains(&bundled.manifest.id) {
            bundled.start(&self.data_root);
        }
        self.entries.push(bundled);
        let manifest = Manifest {
            id: "message-logger".into(),
            name: "MessageLogger".into(),
            version: "1.0.0".into(),
            description: "削除・編集されたメッセージを記録して表示します（同梱）".into(),
            api: API_VERSION,
            entry: "main.lua".into(),
        };
        let mut bundled = Plugin {
            manifest,
            error: None,
            runtime: None,
            path: PathBuf::new(),
            bundled_source: Some(include_str!("../plugins/message-logger/main.lua")),
        };
        if enabled.contains(&bundled.manifest.id) {
            bundled.start(&self.data_root);
        }
        self.entries.push(bundled);
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
        let mut seen = std::collections::HashSet::from([
            "call-time-counter".to_string(),
            "translator".to_string(),
            "message-logger".to_string(),
        ]);
        for path in paths {
            let result = (|| -> anyhow::Result<Manifest> {
                let text = read_limited(&entry_path(&path, "plugin.toml")?)?;
                let m: Manifest = toml::from_str(&text)?;
                anyhow::ensure!(valid_id(&m.id), "不正な ID");
                anyhow::ensure!(m.api == API_VERSION, "未対応の API バージョン: {}", m.api);
                anyhow::ensure!(seen.insert(m.id.clone()), "ID が重複しています: {}", m.id);
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
                    };
                    if enabled.contains(&p.manifest.id) {
                        p.start(&self.data_root);
                    }
                    self.entries.push(p);
                }
                Err(e) => self.errors.push(format!("{}: {e:#}", path.display())),
            }
        }
    }
    /// Set all discovered plugins at once without restarting those already running.
    pub fn set_all(&mut self, on: bool, enabled: &mut Vec<String>) {
        self.generation = self.generation.wrapping_add(1);
        self.revision = self.revision.wrapping_add(1);
        if on {
            for plugin in &mut self.entries {
                if !enabled.contains(&plugin.manifest.id) { enabled.push(plugin.manifest.id.clone()); }
                if !plugin.running() { plugin.start(&self.data_root); }
            }
        } else {
            for plugin in &mut self.entries { plugin.stop(); }
            enabled.clear();
            self.selected = None;
        }
    }
    pub fn toggle(&mut self, id: &str, enabled: &mut Vec<String>) {
        self.generation = self.generation.wrapping_add(1);
        self.revision = self.revision.wrapping_add(1);
        if let Some(p) = self.entries.iter_mut().find(|p| p.manifest.id == id) {
            if enabled.iter().any(|v| v == id) {
                enabled.retain(|v| v != id);
                p.stop();
            } else {
                enabled.push(id.to_owned());
                p.start(&self.data_root);
            }
        }
    }
    pub fn command(&mut self, content: &str, channel: &str) -> bool {
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
    pub fn message_actions(&mut self) -> Vec<(String, usize, String)> {
        let mut all = Vec::new();
        for p in &mut self.entries {
            let result = p
                .runtime
                .as_ref()
                .map(|r| -> mlua::Result<Vec<(usize, String)>> {
                    r.reset_budget();
                    let callbacks: Table = r.lua.globals().get("__plugin")?;
                    let actions: Option<Table> = callbacks.get("message_actions")?;
                    let Some(actions) = actions else {
                        return Ok(vec![]);
                    };
                    let mut items = Vec::new();
                    for (i, item) in actions.sequence_values::<Table>().take(20).enumerate() {
                        let item = item?;
                        items.push((i + 1, item.get::<String>("label")?));
                    }
                    Ok(items)
                });
            match result {
                Some(Ok(items)) => all.extend(
                    items
                        .into_iter()
                        .map(|(i, label)| (p.manifest.id.clone(), i, label)),
                ),
                Some(Err(e)) => p.fail(e.to_string()),
                None => (),
            }
        }
        all
    }
    pub fn message_action(&mut self, id: &str, index: usize, m: &Message) {
        self.revision = self.revision.wrapping_add(1);
        if let Some(p) = self.entries.iter_mut().find(|p| p.manifest.id == id) {
            let result = p.runtime.as_ref().map(|r| -> mlua::Result<Value> {
                r.reset_budget();
                let callbacks: Table = r.lua.globals().get("__plugin")?;
                let actions: Table = callbacks.get("message_actions")?;
                let item: Table = actions.get(index)?;
                let f: Function = item.get("run")?;
                let message = r.lua.create_table()?;
                message.set("id", m.id.clone())?;
                message.set("channel_id", m.channel_id.clone())?;
                message.set("guild_id", m.guild_id.clone())?;
                message.set("author_id", m.author.id.clone())?;
                message.set("content", m.content.clone())?;
                f.call(message)
            });
            p.check(result);
        }
    }
    /// Text supplied by plugins for the active voice connection panel.
    pub fn translator_enabled(&self) -> bool {
        self.entries
            .iter()
            .any(|p| p.manifest.id == "translator" && p.running())
    }
    pub fn translation_outgoing(&mut self) -> Result<Option<String>, String> {
        let p = self
            .entries
            .iter_mut()
            .find(|p| p.manifest.id == "translator")
            .ok_or("翻訳プラグインが見つかりません")?;
        let result = p
            .runtime
            .as_ref()
            .ok_or("翻訳プラグインが停止しています")?
            .call("translation_outgoing", ());
        match result {
            Ok(Value::String(value)) => {
                Ok(Some(value.to_str().map_err(|e| e.to_string())?.to_owned()))
            }
            Ok(Value::Nil) => Ok(None),
            Ok(_) => {
                let error = "翻訳設定の読み取りに失敗しました".to_string();
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
                Some(Ok(Value::String(s))) => match s.to_str() {
                    Ok(value) if value.len() <= 256 => labels.push(value.to_owned()),
                    Ok(_) => p.fail("voice_panel の文字列が長すぎます".into()),
                    Err(e) => p.fail(e.to_string()),
                },
                Some(Ok(Value::Nil)) | None => (),
                Some(Ok(_)) => p.fail("voice_panel は文字列か nil を返してください".into()),
                Some(Err(e)) => p.fail(e.to_string()),
            }
        }
        labels
    }
    pub fn event(&mut self, m: &Message) {
        if self.entries.iter().any(Plugin::running) {
            self.revision = self.revision.wrapping_add(1);
        }
        for p in &mut self.entries {
            let result = p.runtime.as_ref().map(|r| r.message("on_message", m));
            p.check(result);
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
        let p = self.entries.iter_mut().find(|p| p.manifest.id == "message-logger")?;
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
        self.entries.iter().any(|p| p.manifest.id == "message-logger" && p.running())
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
                Some(Ok(_)) => p.fail(format!("{name} は文字列か nil を返してください")),
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
                    label: v.get("label").unwrap_or_default(),
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
    fn start(&mut self, data_dir: &Path) {
        self.error = None;
        let runtime = match self.bundled_source {
            Some(source) => Runtime::from_source(&self.manifest, source, data_dir),
            None => Runtime::new(&self.manifest, &self.path, data_dir),
        };
        match runtime {
            Ok(r) => self.runtime = Some(r),
            Err(e) => self.error = Some(format!("{e:#}")),
        }
    }
    fn stop(&mut self) {
        if let Some(r) = self.runtime.take() {
            let _ = r.call("stop", ());
        }
    }
    fn fail(&mut self, e: String) {
        self.runtime = None;
        self.error = Some(e);
    }
    fn check(&mut self, result: Option<mlua::Result<Value>>) {
        if let Some(Err(e)) = result {
            self.fail(e.to_string());
        }
    }
    pub fn has_page(&self) -> bool {
        self.runtime.as_ref().is_some_and(|runtime| {
            runtime.lua.globals().raw_get::<Table>("__plugin")
                .and_then(|callbacks| callbacks.raw_get::<Value>("page"))
                .is_ok_and(|value| matches!(value, Value::Function(_)))
        })
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
            let manifest = Manifest { id: id.into(), name: id.into(), version: "1".into(), description: String::new(), api: 1, entry: "main.lua".into() };
            plugins.entries.push(Plugin { manifest, error: None, runtime: None, path: PathBuf::new(), bundled_source: Some("local count = 0; return {start=function() count=count+1 end, page=function() return count end}") });
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
        };
        let mut plugins = Plugins::default();
        plugins.entries.push(Plugin {
            manifest,
            error: None,
            runtime: None,
            path: PathBuf::new(),
            bundled_source: Some(include_str!("../plugins/call-time-counter/main.lua")),
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
            };
            let runtime = Runtime::from_source(&manifest, source, Path::new("")).unwrap();
            plugins.entries.push(Plugin {
                manifest,
                error: None,
                runtime: Some(runtime),
                path: PathBuf::new(),
                bundled_source: None,
            });
        }
        assert_eq!(plugins.voice_panel(None, "channel", 0), vec!["channel"]);
        assert!(plugins.entries[0].error.is_some());
        assert!(!plugins.entries[0].running());
        assert!(plugins.entries[1].running());
    }
    #[test]
    fn rejects_paths_outside_plugin() {
        let dir = std::env::temp_dir();
        assert!(entry_path(&dir, "../").is_err());
        assert!(!valid_id("../escape"));
    }
}
