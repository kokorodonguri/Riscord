//! Sandboxed Lua extensions. All calls into Lua have an instruction and memory budget.
use crate::model::Message;
use mlua::{Function, HookTriggers, Lua, LuaOptions, LuaSerdeExt, StdLib, Table, Value};
use serde::Deserialize;
use std::{
    cell::{Cell, RefCell},
    path::{Path, PathBuf},
    rc::Rc,
    time::{Duration, Instant},
};

/// 1 回の呼び出しで使える Lua の命令数
const INSTRUCTION_BUDGET: u32 = 100_000;
/// 命令数を数える間隔。この間隔ごとに経過時間も確かめる。
/// table.sort などの数えていない C の処理を短いループで繰り返されても、締め切りをすぐ確かめられるよう短めにする
const HOOK_INTERVAL: u32 = 100;
/// 1 回の呼び出しの締め切り（C の関数の中で時間を使っても、次の確認で止める）
const CALL_DEADLINE: Duration = Duration::from_millis(200);
/// 読み込み（チャンクの実行）と start() の締め切り。ファイルの読み込みなどが入り、遅い端末では時間がかかるため長め
const START_DEADLINE: Duration = Duration::from_secs(1);
/// C の文字列処理（検索・繰り返し・パターン照合）の作業量。vendor/lua-src の lstrlib.c が registry から引く
const STRING_WORK_BUDGET: i64 = 128 * 1024 * 1024;
const STRING_WORK_KEY: &str = "riscord.strwork";

/// 呼び出しごとの予算を最初から入れ直す。命令数と経過時間はフックで、
/// フックの届かない C の文字列処理は lstrlib.c と共有する registry の値で数える
fn install_budget(lua: &Lua, deadline: Duration) -> mlua::Result<()> {
    lua.set_named_registry_value(STRING_WORK_KEY, STRING_WORK_BUDGET)?;
    let started = Instant::now();
    let ticks = std::cell::Cell::new(0u32);
    lua.set_hook(
        HookTriggers::new().every_nth_instruction(HOOK_INTERVAL),
        move |_, _| {
            ticks.set(ticks.get() + 1);
            if ticks.get() * HOOK_INTERVAL >= INSTRUCTION_BUDGET || started.elapsed() > deadline {
                return Err(mlua::Error::runtime(tr!(
                    "実行時間の制限を超えました",
                    "Exceeded the execution time limit",
                    "超出执行时间限制"
                )));
            }
            Ok(mlua::VmState::Continue)
        },
    );
    Ok(())
}

// Check nesting iteratively before serde recursively walks plugin-provided tables.
// 同じ文字列を何度も参照する表は Lua の中では小さくても、変換すると参照の数だけ複製されるので、
// 文字列（キーを含む）のバイト数も合計して数える
fn json_value(lua: &Lua, value: Value) -> mlua::Result<serde_json::Value> {
    let mut pending = vec![(value.clone(), 0usize)];
    let mut nodes = 0usize;
    let mut bytes = 0usize;
    while let Some((value, depth)) = pending.pop() {
        nodes += 1;
        if depth > 32 || nodes > 10_000 {
            return Err(mlua::Error::runtime(
                "plugin value exceeds nesting or size limit",
            ));
        }
        if let Value::String(text) = &value {
            bytes += text.as_bytes().len();
            if bytes > MAX_VALUE_BYTES {
                return Err(mlua::Error::runtime("plugin value exceeds size limit"));
            }
        }
        if let Value::Table(table) = value {
            for entry in table.pairs::<Value, Value>() {
                let (key, value) = entry?;
                pending.push((key, depth + 1));
                pending.push((value, depth + 1));
                if pending.len() + nodes > 10_000 {
                    return Err(mlua::Error::runtime("plugin value exceeds size limit"));
                }
            }
        }
    }
    lua.from_value(value)
}

const API_VERSION: u32 = 1;
const MAX_SCRIPT: u64 = 1024 * 1024;
/// プラグインから受け取る値（save・options）の文字列の合計バイト数の上限
const MAX_VALUE_BYTES: usize = 2 * 1024 * 1024;
/// 画面に出すプラグインの文字列の上限（文字数）。長くても画面を押しつぶさないように切る
const MAX_ERROR_CHARS: usize = 600;
const MAX_ACTION_LABEL_CHARS: usize = 60;
const MAX_PAGE_LABEL_CHARS: usize = 2000;
/// on_display が返した本文・before_send が返した理由の上限（文字数）
const MAX_DISPLAY_CHARS: usize = 8000;
const MAX_REASON_CHARS: usize = 300;
/// 部品の種類・操作名・アイコン名の上限（バイト数）
const MAX_NAME_BYTES: usize = 256;
/// メッセージの操作（全プラグイン合計）と、ボイスパネルの行の上限
const MAX_MESSAGE_ACTIONS: usize = 40;
const MAX_VOICE_LINES: usize = 8;
/// 全プラグインからの自動送信を合算し、連鎖送信を検出したら停止する。
const MAX_PLUGIN_SENDS_PER_MINUTE: usize = 5;
/// ユーザーの操作（コマンド・メッセージの操作・設定ページ）1 回につき、自動送信に数えない送信の数
const MAX_USER_SENDS_PER_ACTION: usize = 3;
/// 外に影響する操作（URL を開く・読み上げ・状態の変更・コピー）の上限（全プラグイン合計で1分間）。超えた分は捨てる
const MAX_PLUGIN_EFFECTS_PER_MINUTE: usize = 20;
/// 通知・情報ダイアログの上限（全プラグイン合計で1分間）。超えた分は捨てる
const MAX_PLUGIN_NOTICES_PER_MINUTE: usize = 20;
/// 設定ページの部品の上限
const MAX_PAGE_ITEMS: usize = 100;

/// 文字数で切り、切ったら末尾を … にする（UTF-8 の境目で切る）
pub fn clip(s: &str, max_chars: usize) -> String {
    match s.char_indices().nth(max_chars) {
        Some((end, _)) => format!("{}…", &s[..end]),
        None => s.to_owned(),
    }
}

/// バイト数で切る（名前なので … は付けない。UTF-8 の境目で切る）
fn clip_bytes(mut s: String, max_bytes: usize) -> String {
    if s.len() > max_bytes {
        let mut end = max_bytes;
        while !s.is_char_boundary(end) {
            end -= 1;
        }
        s.truncate(end);
    }
    s
}

/// 保存データを一時ファイルに書いてから置き換える（途中で落ちても前のデータが残る）
fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    let tmp = path.with_file_name(format!(
        ".{name}.{}-{}.tmp",
        std::process::id(),
        SEQ.fetch_add(1, Ordering::Relaxed)
    ));
    let result = (|| {
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
        drop(f);
        std::fs::rename(&tmp, path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

/// 保存データを読む。無ければ nil。壊れていたら（JSON として読めない・大きすぎる）
/// `<id>.json.bak` へ退避して Err で知らせる（そのまま nil を返すと、プラグインが空のデータで上書きしてしまう）
fn load_data(path: &Path) -> Result<serde_json::Value, String> {
    let bytes = match std::fs::metadata(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(serde_json::Value::Null),
        Err(e) => return Err(e.to_string()),
        Ok(m) if m.len() > MAX_SCRIPT => None,
        Ok(_) => Some(std::fs::read(path).map_err(|e| e.to_string())?),
    };
    if let Some(v) = bytes.and_then(|b| serde_json::from_slice(&b).ok()) {
        return Ok(v);
    }
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    let bak = path.with_file_name(format!("{name}.bak"));
    let moved = std::fs::rename(path, &bak);
    Err(match moved {
        Ok(()) => tr!(
            "保存データが壊れていたため {} へ移しました",
            "The saved data was corrupted and has been moved to {}",
            "保存的数据已损坏，已移至 {}",
            bak.display()
        ),
        Err(e) => tr!(
            "保存データが壊れています（{} へ移せませんでした: {e}）",
            "The saved data is corrupted (couldn't move it to {}: {e})",
            "保存的数据已损坏（无法移至 {}：{e}）",
            bak.display()
        ),
    })
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
    /// ユーザーの操作を処理している間だけ true。open_url はこの間だけ使える
    user: Rc<Cell<bool>>,
    /// 送信待ちのうち、ユーザーの操作で積まれたので自動送信に数えない数
    user_sends: Cell<usize>,
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
    /// &self の options() で起きたエラー（プラグイン ID, 文）。次の tick でそのプラグインを止める
    option_errors: RefCell<Vec<(String, String)>>,
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
    recent_sends: std::collections::VecDeque<Instant>,
    /// 外へ影響する操作（URL・読み上げ・状態・コピー）を行った時刻（直近1分）
    recent_effects: std::collections::VecDeque<Instant>,
    /// 通知・情報ダイアログを出した時刻（直近1分）
    recent_notices: std::collections::VecDeque<Instant>,
    /// 止めたプラグインの stop() が積んだ操作。次の tick で渡す
    stopped_actions: Vec<Action>,
}

/// メッセージの本文（DM を含む）を外部のサービス（translate.googleapis.com）へ送る同梱プラグイン。
/// 「すべてオン」ではオンにしない
pub const EXTERNAL_PLUGINS: &[&str] = &["translator", "auto-translate"];

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
        install_budget(&lua, START_DEADLINE)?;
        lua.globals().set("pcall", Value::Nil)?;
        lua.globals().set("xpcall", Value::Nil)?;
        for name in ["dofile", "loadfile", "load", "require", "collectgarbage"] {
            lua.globals().set(name, Value::Nil)?;
        }
        let queue = Rc::new(RefCell::new(Vec::new()));
        let timers: Rc<RefCell<Vec<Timer>>> = Rc::new(RefCell::new(Vec::new()));
        let user = Rc::new(Cell::new(false));
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
        let ts = Rc::downgrade(&timers);
        api.set(
            "set_timeout",
            lua.create_function(move |lua, (ms, f): (u64, Function)| {
                let ts = ts
                    .upgrade()
                    .ok_or_else(|| mlua::Error::runtime("Plugin stopped"))?;
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
                // 壊れていたときはエラーにしてプラグインを止め、設定画面にその旨を出す。
                // 壊れたファイルは .bak へ移すので、もう一度オンにすれば空のデータから始められる
                let v = load_data(&file).map_err(mlua::Error::runtime)?;
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
                let v: serde_json::Value = json_value(lua, value)?;
                let bytes = serde_json::to_vec(&v).map_err(mlua::Error::external)?;
                if bytes.len() > MAX_SCRIPT as usize {
                    return Err(mlua::Error::runtime(tr!(
                        "保存データが大きすぎます",
                        "The saved data is too large",
                        "保存的数据过大"
                    )));
                }
                std::fs::create_dir_all(&data_dir).map_err(mlua::Error::external)?;
                write_atomic(&data_file, &bytes).map_err(mlua::Error::external)
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
            let user = user.clone();
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
                        // 受信などの自動の処理から開くと、本文をブラウザ経由で外へ送れてしまう
                        ("open_url", _) if !user.get() => {
                            return Err(mlua::Error::runtime(
                                "open_url: only from commands, message actions or page actions",
                            ))
                        }
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
        // 事前にコンパイルしたバイトコードは検査を通らない命令を作れるので、文字のスクリプトだけ読む
        let callbacks: Table = lua
            .load(script)
            .set_name(&manifest.entry)
            .set_mode(mlua::ChunkMode::Text)
            .eval()?;
        lua.globals().set("__plugin", callbacks.clone())?;
        let has_page = callbacks
            .raw_get::<Value>("page")
            .is_ok_and(|v| matches!(v, Value::Function(_)));
        let runtime = Self {
            has_page,
            lua,
            queue,
            timers,
            user,
            user_sends: Cell::new(0),
        };
        runtime.call_with_deadline("start", (), START_DEADLINE)?;
        Ok(runtime)
    }
    /// ユーザーの操作として f を呼ぶ。その間に積まれた送信は MAX_USER_SENDS_PER_ACTION 件まで自動送信に数えない
    fn as_user<R>(&self, f: impl FnOnce() -> R) -> R {
        let sends = || {
            self.queue
                .borrow()
                .iter()
                .filter(|a| matches!(a, Action::Send(..)))
                .count()
        };
        let before = sends();
        self.user.set(true);
        let result = f();
        self.user.set(false);
        let added = sends().saturating_sub(before);
        self.user_sends
            .set(self.user_sends.get() + added.min(MAX_USER_SENDS_PER_ACTION));
        result
    }
    fn reset_budget(&self) {
        // 失敗するのはメモリ上限に当たったときだけ。そのときは続く呼び出しが失敗して止まる
        let _ = install_budget(&self.lua, CALL_DEADLINE);
    }
    fn call<A: mlua::IntoLuaMulti>(&self, name: &str, args: A) -> mlua::Result<Value> {
        self.call_with_deadline(name, args, CALL_DEADLINE)
    }
    fn call_with_deadline<A: mlua::IntoLuaMulti>(
        &self,
        name: &str,
        args: A,
        deadline: Duration,
    ) -> mlua::Result<Value> {
        let _ = install_budget(&self.lua, deadline);
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
            self.stopped_actions.extend(p.stop());
        }
        self.entries.clear();
        // 読み込みの途中で抜けても、消えたプラグインを指す索引を残さない（最後に作り直す）
        self.index.clear();
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
        self.load_external(enabled, &mut seen);
        self.index = self
            .entries
            .iter()
            .enumerate()
            .map(|(i, p)| (p.manifest.id.clone(), i))
            .collect();
    }
    /// プラグインのフォルダから、ユーザーが追加したプラグインを読み込む（reload の続き）
    fn load_external(&mut self, enabled: &[String], seen: &mut std::collections::HashSet<String>) {
        let dir = directory();
        if let Err(e) = std::fs::create_dir_all(&dir) {
            self.errors.push(e.to_string());
            return;
        }
        let iter = match std::fs::read_dir(&dir) {
            Ok(iter) => iter,
            Err(e) => {
                self.errors.push(format!("{}: {e}", dir.display()));
                return;
            }
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
    }
    /// Set all discovered plugins at once without restarting those already running.
    /// オンにするのは同梱プラグインだけ。本文を外部のサービスへ送るもの（EXTERNAL_PLUGINS）と、
    /// ユーザーがフォルダに追加したプラグイン（中身を確かめていない Lua）は含めず、個別にオンにしてもらう
    pub fn set_all(&mut self, on: bool, enabled: &mut Vec<String>) {
        self.dirty();
        self.actions_cache = None;
        self.generation = self.generation.wrapping_add(1);
        self.revision = self.revision.wrapping_add(1);
        if on {
            for plugin in &mut self.entries {
                if plugin.bundled_source.is_none()
                    || EXTERNAL_PLUGINS.contains(&plugin.manifest.id.as_str())
                {
                    continue;
                }
                if !enabled.contains(&plugin.manifest.id) {
                    enabled.push(plugin.manifest.id.clone());
                }
                if !plugin.running() {
                    plugin.start(&self.data_root, &self.host);
                }
            }
        } else {
            for plugin in &mut self.entries {
                self.stopped_actions.extend(plugin.stop());
            }
            enabled.clear();
            self.selected = None;
        }
    }
    pub fn toggle(&mut self, id: &str, enabled: &mut Vec<String>) {
        self.dirty();
        self.actions_cache = None;
        self.generation = self.generation.wrapping_add(1);
        self.revision = self.revision.wrapping_add(1);
        if let Some(p) = self.entries.iter_mut().find(|p| p.manifest.id == id) {
            if enabled.iter().any(|v| v == id) {
                enabled.retain(|v| v != id);
                self.stopped_actions.extend(p.stop());
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
                    Value::Function(f) => {
                        Ok(Some(r.as_user(|| {
                            f.call((args.to_owned(), channel.to_owned()))
                        })?))
                    }
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
                                item.get::<Option<String>>("icon")?
                                    .map(|icon| clip_bytes(icon, MAX_NAME_BYTES)),
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
                r.as_user(|| f.call(message))
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
            .and_then(|r| match r.call("options", ()) {
                Ok(v) => Some(v),
                Err(e) => {
                    // 握りつぶすと、予算を使い切り続けるプラグインが止まらずに居座る
                    self.option_errors
                        .borrow_mut()
                        .push((id.to_owned(), e.to_string()));
                    None
                }
            })
            .and_then(|v| {
                let r = self
                    .entries
                    .iter()
                    .find(|p| p.manifest.id == id)?
                    .runtime
                    .as_ref()?;
                json_value(&r.lua, v).ok()
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
    /// ログアウトしたとき。前のアカウントの情報・タイマー・送信待ちの操作を捨てる
    /// （次にログインしたアカウントで、前のアカウントの続きが動かないように）
    pub fn end_session(&mut self) {
        let status = self.host.borrow().status.clone();
        *self.host.borrow_mut() = HostInfo {
            status,
            ..Default::default()
        };
        for p in &self.entries {
            if let Some(r) = &p.runtime {
                r.timers.borrow_mut().clear();
                r.queue.borrow_mut().clear();
            }
        }
        self.stopped_actions.clear();
        self.actions_cache = None;
        self.recent_sends.clear();
        self.recent_effects.clear();
        self.recent_notices.clear();
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
            json_value(&r.lua, v)
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
            let id = &p.manifest.id;
            let cache = &self.options_cache;
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
                    Ok(json_value(&r.lua, v).unwrap_or(serde_json::Value::Null))
                };
                let display = matches!(
                    callbacks.raw_get::<Value>("on_display")?,
                    Value::Function(_)
                );
                if display {
                    // どのみち描き直すので設定は比べない。次に読むときに作り直す
                    r.message("on_message", m)?;
                    cache.borrow_mut().remove(id);
                    return Ok(true);
                }
                // 前の値はキャッシュがあればそれを使い、受信のたびに 2 回 JSON にしない
                let cached = cache.borrow().contains_key(id);
                let before = if cached { None } else { Some(options()?) };
                r.message("on_message", m)?;
                let after = options()?;
                let changed = match &before {
                    Some(b) => *b != after,
                    None => cache.borrow().get(id) != Some(&after),
                };
                cache.borrow_mut().insert(id.clone(), after);
                Ok(changed)
            })();
            match result {
                Ok(c) => changed |= c,
                Err(e) => p.fail(e.to_string()),
            }
        }
        if changed {
            self.revision = self.revision.wrapping_add(1);
            self.actions_cache = None;
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
                        return Some(clip(&s, MAX_REASON_CHARS));
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
                    // 表示する本文は画面を押しつぶさないよう切る（送る本文は送信側の上限で扱う）
                    Ok(s) if name == "on_display" => content = clip(&s, MAX_DISPLAY_CHARS),
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
            Some(Ok(Value::Table(t))) => {
                let mut items: Vec<PageItem> = t
                    .sequence_values::<Table>()
                    .take(MAX_PAGE_ITEMS)
                    .filter_map(Result::ok)
                    .map(|v| PageItem {
                        kind: clip_bytes(v.get("kind").unwrap_or_default(), MAX_NAME_BYTES),
                        label: clip(
                            &v.get::<String>("label").unwrap_or_default(),
                            MAX_PAGE_LABEL_CHARS,
                        ),
                        action: clip_bytes(v.get("action").unwrap_or_default(), MAX_NAME_BYTES),
                        enabled: v.get("enabled").unwrap_or(false),
                    })
                    .collect();
                // 切った分があることを知らせる（黙って消えると、末尾のボタンが無いように見える）
                let rest = t.raw_len().saturating_sub(MAX_PAGE_ITEMS);
                if rest > 0 {
                    items.push(PageItem {
                        kind: "text".into(),
                        label: tr!(
                            "（部品が多すぎるため、残りの {rest} 個は表示していません）",
                            "(Too many items: {rest} more are not shown)",
                            "（项目过多，其余 {rest} 个未显示）",
                        ),
                        action: String::new(),
                        enabled: false,
                    });
                }
                items
            }
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
                    .and_then(|v| r.as_user(|| r.call("on_action", (action.to_owned(), v))))
            });
            p.check(result);
        }
    }
    pub fn tick(&mut self) -> (Vec<Action>, Option<Duration>) {
        let mut actions = Vec::new();
        let mut next: Option<Duration> = None;
        let now = Instant::now();
        for recent in [
            &mut self.recent_sends,
            &mut self.recent_effects,
            &mut self.recent_notices,
        ] {
            while recent
                .front()
                .is_some_and(|t| now.duration_since(*t) >= Duration::from_secs(60))
            {
                recent.pop_front();
            }
        }
        // 止めたときの stop() で積まれた操作。止めたプラグインは止め直せないので、送信が上限を超えた分は捨てる
        for action in std::mem::take(&mut self.stopped_actions) {
            if matches!(action, Action::Send(..)) {
                if self.recent_sends.len() >= MAX_PLUGIN_SENDS_PER_MINUTE {
                    continue;
                }
                self.recent_sends.push_back(now);
            }
            actions.push(action);
        }
        let mut display_changed = false;
        for (id, e) in self.option_errors.take() {
            if let Some(p) = self
                .entries
                .iter_mut()
                .find(|p| p.manifest.id == id && p.runtime.is_some())
            {
                p.fail(e);
                self.options_cache.borrow_mut().remove(&id);
                display_changed = true;
            }
        }
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
            // 表示が変わりうるのは on_display を持つか、設定（options）が変わったときだけ
            let display = !due.is_empty()
                && r.lua
                    .globals()
                    .get::<Table>("__plugin")
                    .and_then(|c| c.raw_get::<Value>("on_display"))
                    .is_ok_and(|v| matches!(v, Value::Function(_)));
            let before = if due.is_empty() || display {
                None
            } else {
                Some(
                    self.options_cache
                        .borrow()
                        .get(&p.manifest.id)
                        .cloned()
                        .unwrap_or_else(|| match r.call("options", ()) {
                            Ok(v) => json_value(&r.lua, v).unwrap_or(serde_json::Value::Null),
                            Err(e) => {
                                error = Some(e.to_string());
                                serde_json::Value::Null
                            }
                        }),
                )
            };
            for timer in due.into_iter().filter(|_| error.is_none()) {
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
                if display {
                    display_changed = true;
                    self.options_cache.borrow_mut().remove(&p.manifest.id);
                } else if let Some(before) = before {
                    let after = match r.call("options", ()) {
                        Ok(v) => json_value(&r.lua, v).unwrap_or(serde_json::Value::Null),
                        Err(e) => {
                            p.fail(e.to_string());
                            display_changed = true;
                            continue;
                        }
                    };
                    if after != before {
                        display_changed = true;
                    }
                    self.options_cache
                        .borrow_mut()
                        .insert(p.manifest.id.clone(), after);
                }
                let queued: Vec<_> = r.queue.borrow_mut().drain(..).collect();
                // ユーザーの操作で積まれた送信は数えない
                let sends = queued
                    .iter()
                    .filter(|a| matches!(a, Action::Send(..)))
                    .count()
                    .saturating_sub(r.user_sends.replace(0));
                if self.recent_sends.len() + sends > MAX_PLUGIN_SENDS_PER_MINUTE {
                    p.fail(
                        tr!(
                        "プラグインの送信回数が上限を超えました（全プラグイン合計で1分間に5件）",
                        "Plugin send limit exceeded (5 messages per minute across all plugins)",
                        "插件发送次数超过限制（所有插件每分钟合计5条）"
                    )
                        .to_string(),
                    );
                    continue;
                }
                self.recent_sends.extend(std::iter::repeat_n(now, sends));
                actions.extend(queued);
                for timer in r.timers.borrow().iter() {
                    let delay = timer.at.saturating_duration_since(Instant::now());
                    next = Some(next.map_or(delay, |n| n.min(delay)));
                }
            }
        }
        if display_changed {
            self.revision = self.revision.wrapping_add(1);
            self.actions_cache = None;
        }
        // 外へ影響する操作・通知は全プラグイン合計で数え、上限を超えた分は捨てる
        actions.retain(|a| {
            let (recent, max) = match a {
                Action::OpenUrl(_) | Action::Speak(_) | Action::SetStatus(_) | Action::Copy(_) => {
                    (&mut self.recent_effects, MAX_PLUGIN_EFFECTS_PER_MINUTE)
                }
                Action::Notify(..) | Action::Info(..) => {
                    (&mut self.recent_notices, MAX_PLUGIN_NOTICES_PER_MINUTE)
                }
                _ => return true,
            };
            if recent.len() >= max {
                return false;
            }
            recent.push_back(now);
            true
        });
        (actions, next)
    }
}
impl Drop for Plugins {
    fn drop(&mut self) {
        // 終了時は操作を行う先がないので捨てる
        for plugin in &mut self.entries {
            let _ = plugin.stop();
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
    /// 止める。stop() の中で積まれた操作（状態を戻すなど）を返すので、呼び出し側が次の tick で渡す
    #[must_use]
    fn stop(&mut self) -> Vec<Action> {
        let Some(r) = self.runtime.take() else {
            return vec![];
        };
        let _ = r.call("stop", ());
        let actions = std::mem::take(&mut *r.queue.borrow_mut());
        actions
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
    #[test]
    fn lua_string_native_work_is_bounded() {
        let lua = mlua::Lua::new();
        assert_eq!(
            lua.load("return string.rep('', 9223372036854775807)")
                .eval::<String>()
                .unwrap(),
            ""
        );
        assert!(lua
            .load("return string.rep('x', 1000001)")
            .eval::<String>()
            .is_err());
        assert!(lua
            .load("return string.match(string.rep('a', 100), 'a*a*a*a*a*a*a*a*b')")
            .eval::<mlua::Value>()
            .is_err());
    }

    #[test]
    fn rejects_deep_or_recursive_plugin_values() {
        let lua = mlua::Lua::new();
        let root = lua.create_table().unwrap();
        let mut current = root.clone();
        for _ in 0..40 {
            let next = lua.create_table().unwrap();
            current.set("next", next.clone()).unwrap();
            current = next;
        }
        assert!(super::json_value(&lua, mlua::Value::Table(root)).is_err());
        let cycle = lua.create_table().unwrap();
        cycle.set("self", cycle.clone()).unwrap();
        assert!(super::json_value(&lua, mlua::Value::Table(cycle)).is_err());
    }

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
    fn native_string_work_shares_one_budget_per_call() {
        // 1 回の plain find で O(n·m) の比較をさせても、すぐに止まる
        let (r, dir) = runtime(
            "return { on_send=function(s) local h=('a'):rep(1000000) local n=('a'):rep(200000)..'b' \
             h:find(n,1,true) return s end }",
        );
        let t = Instant::now();
        assert!(r.call("on_send", ("x", "1")).is_err());
        assert!(t.elapsed() < Duration::from_secs(2), "{:?}", t.elapsed());
        // 上限ぎりぎりのパターン照合を繰り返しても、合計で止まる
        let (r2, dir2) = runtime(
            "return { on_send=function(s) local h=('a'):rep(100000) \
             for i=1,10000 do h:find('a*b') end return s end }",
        );
        let t = Instant::now();
        assert!(r2.call("on_send", ("x", "1")).is_err());
        assert!(t.elapsed() < Duration::from_secs(2), "{:?}", t.elapsed());
        // 普通の量の文字列処理は通り、次の呼び出しでは予算が戻る
        let (r3, dir3) = runtime(
            "return { on_send=function(s) for i=1,200 do s:gsub('%s+', ' '):find('hello', 1, true) end return s end }",
        );
        for _ in 0..50 {
            assert!(r3.call("on_send", ("hello world", "1")).is_ok());
        }
        for d in [dir, dir2, dir3] {
            std::fs::remove_dir_all(d).unwrap();
        }
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
    /// 「すべてオン」はユーザーが追加したプラグイン（中身を確かめていない Lua）を起動しない
    #[test]
    fn set_all_skips_user_added_plugins() {
        let mut plugins = Plugins::default();
        for (id, bundled) in [("bundled-one", true), ("user-added", false)] {
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
            plugins.entries.push(Plugin {
                manifest,
                error: None,
                runtime: None,
                path: PathBuf::new(),
                failed: false,
                waker: Default::default(),
                bundled_source: bundled.then_some("return {}"),
            });
        }
        let mut enabled = Vec::new();
        plugins.set_all(true, &mut enabled);
        assert_eq!(enabled, ["bundled-one"]);
        assert!(plugins.entries[0].running());
        assert!(!plugins.entries[1].running());
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
        // 本文を外部へ送るものは「すべてオン」に含まれないので個別にオンにする
        for id in EXTERNAL_PLUGINS {
            plugins.toggle(id, &mut enabled);
        }
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
        assert_eq!(
            send(&mut p, "see this. www.example.com/path"),
            "See this. www.example.com/path"
        );
        assert_eq!(send(&mut p, "hi\nexample.com"), "Hi\nexample.com");
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
    #[test]
    fn todo_page_keeps_controls_and_reaches_later_items() {
        let mut p = only("to-do-list");
        for i in 0..55 {
            assert!(p.command(&format!("/todo task-{i}"), "1"));
        }
        let first = p.page("to-do-list");
        assert!(first.iter().any(|i| i.action == "add"));
        assert!(first.iter().any(|i| i.action == "next_page"));
        p.page_action("to-do-list", "next_page", Default::default());
        let second = p.page("to-do-list");
        assert!(second.iter().any(|i| i.action == "add"));
        assert!(second.iter().any(|i| i.label == "task-54"));
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
    fn automatic_send_loop_stops_at_shared_limit() {
        let (mut plugins, _) = scripted(&[(
            "loop",
            "return {on_message=function() riscord.send_message('2', 'reply') end}",
        )]);
        for _ in 0..MAX_PLUGIN_SENDS_PER_MINUTE {
            plugins.event(&message("reply"));
            assert_eq!(plugins.tick().0.len(), 1);
        }
        plugins.event(&message("reply"));
        assert!(plugins.tick().0.is_empty());
        assert!(!plugins.on("loop"));
        assert!(plugins.entries[0].error.is_some());
    }

    #[test]
    fn send_limit_is_shared_and_expires() {
        let (mut plugins, _) = scripted(&[
            (
                "a",
                "return {on_message=function() riscord.send_message('2', 'reply') end}",
            ),
            (
                "b",
                "return {on_message=function() riscord.send_message('2', 'reply') end}",
            ),
        ]);
        plugins.recent_sends.extend(std::iter::repeat_n(
            Instant::now(),
            MAX_PLUGIN_SENDS_PER_MINUTE - 1,
        ));
        plugins.event(&message("hello"));
        assert_eq!(plugins.tick().0.len(), 1);
        assert!(!plugins.on("b"));
        plugins.recent_sends = std::iter::repeat_n(
            Instant::now() - Duration::from_secs(61),
            MAX_PLUGIN_SENDS_PER_MINUTE,
        )
        .collect();
        plugins.event(&message("hello"));
        assert_eq!(plugins.tick().0.len(), 1);
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
    /// options() のエラーを握りつぶさず、そのプラグインを止める
    #[test]
    fn options_error_stops_the_plugin() {
        let (mut plugins, _) =
            scripted(&[("bad", "return {options=function() error('boom') end}")]);
        assert_eq!(plugins.options("bad"), serde_json::Value::Null);
        assert!(plugins.on("bad"));
        plugins.tick();
        assert!(!plugins.on("bad"));
        assert!(plugins.entries[0]
            .error
            .as_deref()
            .unwrap()
            .contains("boom"));
    }
    /// 上限で止まった gmatch のイテレータを使い直しても、作業量が巻き戻って止まらなくなることはない
    #[test]
    fn exhausted_gmatch_iterator_stays_bounded() {
        let lua = mlua::Lua::new();
        lua.load(
            "local h = ('a'):rep(200000) \
             it = h:gmatch('a*a*a*a*b') \
             assert(not pcall(it))",
        )
        .exec()
        .unwrap();
        let t = Instant::now();
        assert!(lua
            .load("return pcall(it)")
            .eval::<bool>()
            .is_ok_and(|ok| !ok));
        assert!(t.elapsed() < Duration::from_secs(2), "{:?}", t.elapsed());
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
    /// open_url はユーザーの操作の中でだけ使え、受信などの自動の処理から呼ぶと止まる
    #[test]
    fn open_url_only_from_user_actions() {
        let (mut plugins, _) = scripted(&[(
            "opener",
            "return {on_message=function(m) riscord.open_url('https://example.com/?q=' .. m.content) end,
              commands={go=function() riscord.open_url('https://example.com/') return true end},
              message_actions={{label='open', run=function() riscord.open_url('steam://run/1') end}}}",
        )]);
        assert!(plugins.command("/go", "1"));
        plugins.message_action("opener", 1, &message("hi"));
        assert_eq!(
            plugins.tick().0,
            vec![
                Action::OpenUrl("https://example.com/".into()),
                Action::OpenUrl("steam://run/1".into())
            ]
        );
        plugins.event(&message("secret"));
        assert!(plugins.tick().0.is_empty());
        assert!(!plugins.on("opener"));
    }
    /// 外へ影響する操作は全プラグイン合計で 1 分あたりの上限を超えた分を捨てる
    #[test]
    fn side_effects_share_rate_limit() {
        let (mut plugins, _) = scripted(&[(
            "noisy",
            "return {on_message=function() for i = 1, 30 do riscord.speak('x') end riscord.notify('a', 'b') end}",
        )]);
        plugins.event(&message("hi"));
        let actions = plugins.tick().0;
        assert_eq!(
            actions
                .iter()
                .filter(|a| matches!(a, Action::Speak(_)))
                .count(),
            MAX_PLUGIN_EFFECTS_PER_MINUTE
        );
        assert!(actions.iter().any(|a| matches!(a, Action::Notify(..))));
        plugins.event(&message("hi"));
        assert!(plugins
            .tick()
            .0
            .iter()
            .all(|a| !matches!(a, Action::Speak(_))));
        assert!(plugins.on("noisy"));
    }
    /// 手で打ったコマンドの送信は自動送信の上限に数えない
    #[test]
    fn command_sends_do_not_count_as_automatic() {
        let mut plugins = only("better-formatting-redux");
        for i in 0..MAX_PLUGIN_SENDS_PER_MINUTE * 3 {
            assert!(plugins.command(&format!("/fw hello {i}"), "1"));
            let actions = plugins.tick().0;
            assert!(
                matches!(&actions[..], [Action::Send(c, _)] if c == "1"),
                "{actions:?}"
            );
        }
        assert!(plugins.on("better-formatting-redux"));
        assert!(plugins.recent_sends.is_empty());
        // 1 回のコマンドで数えないのは MAX_USER_SENDS_PER_ACTION 件まで
        let (mut plugins, _) = scripted(&[(
            "burst",
            "return {commands={burst=function(_, c) for i = 1, 4 do riscord.send_message(c, 'x') end return true end}}",
        )]);
        assert!(plugins.command("/burst", "1"));
        assert_eq!(plugins.tick().0.len(), 4);
        assert_eq!(plugins.recent_sends.len(), 4 - MAX_USER_SENDS_PER_ACTION);
    }
    /// 部品が上限を超えたら切ったことを知らせる
    #[test]
    fn page_notes_truncated_items() {
        let (mut plugins, _) = scripted(&[(
            "big",
            "return {page=function() local t = {} for i = 1, 150 do t[i] = {kind='text', label='x' .. i} end return t end}",
        )]);
        let items = plugins.page("big");
        assert_eq!(items.len(), MAX_PAGE_ITEMS + 1);
        assert_eq!(items[MAX_PAGE_ITEMS - 1].label, "x100");
        assert!(items[MAX_PAGE_ITEMS].label.contains("50"));
    }
    /// stop() の中で積んだ操作（状態を戻すなど）も次の tick で届く
    #[test]
    fn actions_queued_in_stop_are_delivered() {
        let (mut plugins, mut enabled) = scripted(&[(
            "away",
            "return {stop=function() riscord.set_status('online') end}",
        )]);
        plugins.toggle("away", &mut enabled);
        assert!(!plugins.on("away"));
        assert_eq!(plugins.tick().0, vec![Action::SetStatus("online".into())]);
        assert!(plugins.tick().0.is_empty());
    }
    /// タイマーが動いても、表示が変わりうるときだけ revision を上げる
    #[test]
    fn timer_bumps_revision_only_when_display_can_change() {
        let (mut plugins, _) = scripted(&[(
            "quiet",
            "local s = {n = 0}; local function f() s.hidden = (s.hidden or 0) + 1 end
             return {start=function() riscord.set_timeout(0, f) end, options=function() return {n = s.n} end,
               commands={bump=function() riscord.set_timeout(0, function() s.n = s.n + 1 end) return true end}}",
        )]);
        assert_eq!(plugins.opt_f64("quiet", "n"), Some(0.0));
        let before = plugins.revision;
        plugins.entries[0]
            .runtime
            .as_ref()
            .unwrap()
            .timers
            .borrow_mut()[0]
            .at = Instant::now();
        plugins.tick();
        assert_eq!(plugins.revision, before);
        assert!(plugins.command("/bump", "1"));
        let before = plugins.revision;
        plugins.entries[0]
            .runtime
            .as_ref()
            .unwrap()
            .timers
            .borrow_mut()[0]
            .at = Instant::now();
        plugins.tick();
        assert_ne!(plugins.revision, before);
        assert_eq!(plugins.opt_f64("quiet", "n"), Some(1.0));
    }
    /// 長い本文を UTF-8 の途中で切らず、保存できる大きさに収める
    #[test]
    fn personal_pins_clip_utf8_and_fit_save_limit() {
        let dir = std::env::temp_dir().join(format!(
            "riscord-pins-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let pins: Vec<_> = (0..500)
            .map(|i| serde_json::json!({"id": i.to_string(), "channel_id": "2", "author": "a", "content": "y".repeat(1900), "timestamp": ""}))
            .collect();
        let file = dir.join("personal-pins.json");
        std::fs::write(
            &file,
            serde_json::to_vec(&serde_json::json!({"pins": pins})).unwrap(),
        )
        .unwrap();
        let mut plugins = Plugins::default();
        plugins.data_root = dir.clone();
        plugins.reload(&["personal-pins".to_string()]);
        assert!(plugins.on("personal-pins"));
        let mut m = message(&"あ".repeat(1400));
        m.id = "new".into();
        plugins.message_action("personal-pins", 1, &m);
        assert!(
            plugins.on("personal-pins"),
            "{:?}",
            plugins
                .entries
                .iter()
                .find(|p| p.manifest.id == "personal-pins")
                .map(|p| &p.error)
        );
        assert!(std::fs::metadata(&file).unwrap().len() < MAX_SCRIPT);
        let page = plugins.page("personal-pins");
        let newest = page.iter().find(|i| i.label.contains('あ')).unwrap();
        assert!(newest.label.ends_with("あ…"), "{}", newest.label);
        assert!(page.iter().any(|i| i.action == "next_page"));
        assert!(page.len() <= MAX_PAGE_ITEMS);
        let _ = std::fs::remove_dir_all(dir);
    }
    /// 記録が多くても「記録を消す」が先頭にあり、ページを送れる
    #[test]
    fn removed_connection_alerts_keeps_clear_button_reachable() {
        let mut plugins = only("removed-connection-alerts");
        for i in 0..200 {
            plugins.hook(
                "on_removed",
                &["guild".into(), i.to_string().into(), format!("s{i}").into()],
            );
            let _ = plugins.tick();
        }
        assert!(plugins.on("removed-connection-alerts"));
        let page = plugins.page("removed-connection-alerts");
        assert!(page.len() <= MAX_PAGE_ITEMS);
        assert!(page.iter().any(|i| i.action == "clear"));
        assert!(page.iter().any(|i| i.action == "next_page"));
        assert!(page.iter().any(|i| i.label.ends_with("s199")));
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
            // 本文を外部へ送るものは「すべてオン」に含めず、個別にオンにする
            for id in EXTERNAL_PLUGINS {
                assert!(!plugins.on(id), "round {round}: {id}");
                plugins.toggle(id, &mut enabled);
            }
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
    /// 「すべてオン」は翻訳など本文を外部へ送るプラグインをオンにしない
    #[test]
    fn set_all_skips_external_plugins() {
        let mut plugins = Plugins::default();
        for id in ["translator", "auto-translate", "plain"] {
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
            plugins.entries.push(Plugin {
                manifest,
                error: None,
                runtime: None,
                path: PathBuf::new(),
                failed: false,
                waker: Default::default(),
                bundled_source: Some("return {}"),
            });
        }
        let mut enabled = Vec::new();
        plugins.set_all(true, &mut enabled);
        assert_eq!(enabled, ["plain"]);
        assert!(!plugins.entries[0].running() && !plugins.entries[1].running());
        assert!(plugins.entries[2].running());
    }
    /// 同じ大きな文字列を何度も参照する値は、変換する前に断る
    #[test]
    fn rejects_values_with_too_many_string_bytes() {
        let lua = mlua::Lua::new();
        let big = lua.create_string("x".repeat(1024 * 1024)).unwrap();
        let t = lua.create_table().unwrap();
        for i in 1..=4 {
            t.set(i, big.clone()).unwrap();
        }
        assert!(json_value(&lua, Value::Table(t)).is_err());
        let ok = lua.create_table().unwrap();
        ok.set("k", "small").unwrap();
        assert!(json_value(&lua, Value::Table(ok)).is_ok());
    }
    /// 保存は置き換えで行い、壊れた保存データは .bak へ移してエラーで知らせる
    #[test]
    fn save_is_atomic_and_broken_data_is_backed_up() {
        let (r, dir) = runtime(
            "return { save=function(v) riscord.save(v) end, load=function() return riscord.load() end }",
        );
        let file = dir.join("test.json");
        r.call("save", "hello").unwrap();
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "\"hello\"");
        // 一時ファイルが残らない
        let names: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        assert!(names.iter().all(|n| !n.ends_with(".tmp")), "{names:?}");
        let Value::String(v) = r.call("load", ()).unwrap() else {
            panic!()
        };
        assert_eq!(v.to_str().unwrap(), "hello");
        std::fs::write(&file, "{broken").unwrap();
        assert!(r.call("load", ()).is_err());
        assert!(!file.exists());
        assert_eq!(
            std::fs::read_to_string(dir.join("test.json.bak")).unwrap(),
            "{broken"
        );
        // 退避した後は空のデータから始められる
        assert!(matches!(r.call("load", ()).unwrap(), Value::Nil));
        std::fs::remove_dir_all(dir).unwrap();
    }
    /// プラグインが返す文字列は UTF-8 の境目で切る
    #[test]
    fn plugin_strings_are_clipped() {
        assert_eq!(clip_bytes("あいう".into(), 4), "あ");
        assert_eq!(clip_bytes("abc".into(), 256), "abc");
        let (mut plugins, _) = scripted(&[(
            "long",
            "return {on_display=function(s) return ('あ'):rep(20000) end, \
             before_send=function() return ('x'):rep(5000) end, \
             page=function() return {{kind=('k'):rep(1000), action=('a'):rep(1000), label='l'}} end}",
        )]);
        let shown = plugins.transform("on_display", "hi", "1");
        assert_eq!(shown.chars().count(), MAX_DISPLAY_CHARS + 1);
        let reason = plugins.check_send("hi", "1").unwrap();
        assert_eq!(reason.chars().count(), MAX_REASON_CHARS + 1);
        let page = plugins.page("long");
        assert_eq!(page[0].kind.len(), MAX_NAME_BYTES);
        assert_eq!(page[0].action.len(), MAX_NAME_BYTES);
    }
    /// 通知・情報ダイアログも 1 分あたりの上限を超えた分を捨てる
    #[test]
    fn notices_are_rate_limited() {
        let (mut plugins, _) = scripted(&[(
            "chatty",
            "return {on_message=function() for i = 1, 30 do riscord.notify('a', 'b') riscord.show_info('c', 'd') end end}",
        )]);
        plugins.event(&message("hi"));
        let actions = plugins.tick().0;
        let notices = actions
            .iter()
            .filter(|a| matches!(a, Action::Notify(..) | Action::Info(..)))
            .count();
        assert_eq!(notices, MAX_PLUGIN_NOTICES_PER_MINUTE);
        plugins.event(&message("hi"));
        assert!(plugins
            .tick()
            .0
            .iter()
            .all(|a| !matches!(a, Action::Notify(..) | Action::Info(..))));
    }
    #[test]
    fn rejects_paths_outside_plugin() {
        let dir = std::env::temp_dir();
        assert!(entry_path(&dir, "../").is_err());
        assert!(!valid_id("../escape"));
    }
}
