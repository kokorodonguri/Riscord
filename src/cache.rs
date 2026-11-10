//! すぐに開くための手元の控え。前回の自分・サーバー・DM・チャンネル・最近の会話を保存しておき、
//! 次の起動ではログインの完了を待たずにそれを表示する（ログインは裏で進み、届いたものから入れ替わる）。
//! トークンを保存しているときだけ書く。中身は会話なので、トークンと同じく本人だけが読めるようにする。
//! 別のアカウントの控えを出さないよう、トークンのハッシュが一致するときだけ使う
use std::io::Write as _;
use std::path::PathBuf;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

use crate::model::{Channel, Guild, Message, Role, User};

/// 形式を変えたら上げる（古い控えは読まずに捨てる）
pub const VERSION: u32 = 2;
/// 1チャンネルに残す件数
pub const MESSAGES_PER_CHANNEL: usize = 50;
/// 会話を残すチャンネル数（最近開いた順）
pub const CHANNELS: usize = 10;

#[derive(Serialize, Deserialize, Default)]
pub struct Snapshot {
    pub version: u32,
    /// 控えの持ち主（ユーザー ID）。トークンが変わっても同じアカウントなら使う
    pub owner: String,
    pub me: Option<User>,
    pub guilds: Vec<Guild>,
    pub dms: Vec<Channel>,
    /// (サーバー ID, チャンネル一覧)
    pub channels: Vec<(String, Vec<Channel>)>,
    pub roles: Vec<(String, Vec<Role>)>,
    /// (チャンネル ID, 古い順のメッセージ)。古く開いたチャンネルから並ぶ
    pub messages: Vec<(String, Vec<Message>)>,
    pub sel_guild: Option<String>,
    pub sel_channel: Option<String>,
}

pub fn path() -> PathBuf {
    crate::profile::cache_dir().join("snapshot.json")
}

/// 画像の控えを置くフォルダ
pub fn image_dir() -> PathBuf {
    crate::app::cache_root().join("riscord").join("images")
}

/// トークンの持ち主のユーザー ID。Discord のトークンは最初の「.」までがユーザー ID の base64 なので、
/// 通信せずに分かる。読めないトークンでは控えを使わない
pub fn token_owner(token: &str) -> Option<String> {
    use base64::engine::general_purpose::{STANDARD_NO_PAD, URL_SAFE_NO_PAD};
    use base64::Engine as _;
    let head = token.trim().split('.').next()?.trim_end_matches('=');
    let bytes = STANDARD_NO_PAD.decode(head).or_else(|_| URL_SAFE_NO_PAD.decode(head)).ok()?;
    let id = String::from_utf8(bytes).ok()?;
    (!id.is_empty() && id.len() <= 20 && id.bytes().all(|b| b.is_ascii_digit())).then_some(id)
}

pub fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// このトークンの控えがあれば返す
pub fn load(token: &str) -> Option<Snapshot> {
    let bytes = std::fs::read(path()).ok()?;
    let s: Snapshot = match serde_json::from_slice(&bytes) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("起動用の控えを読めませんでした: {e}");
            return None;
        }
    };
    let owner = token_owner(token)?;
    (s.version == VERSION && s.owner == owner && s.me.as_ref().is_some_and(|m| m.id == owner)).then_some(s)
}

/// 同時に2つ書いて壊さないように
static WRITE: Mutex<()> = Mutex::new(());

/// 一時ファイルに書いてから置き換える（途中で落ちても前の控えが残る）
pub fn write(bytes: &[u8]) -> std::io::Result<()> {
    let _guard = WRITE.lock().unwrap_or_else(|e| e.into_inner());
    let p = path();
    std::fs::create_dir_all(p.parent().unwrap())?;
    let tmp = p.with_extension("json.tmp");
    write_private(&tmp, bytes)?;
    std::fs::rename(&tmp, &p)
}

/// 本人だけが読めるファイルとして書く
pub fn write_private(p: &std::path::Path, bytes: &[u8]) -> std::io::Result<()> {
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let mut f = opts.open(p)?;
    f.write_all(bytes)
}

/// ログアウト・ログイン失敗のとき。会話の控えを残さない
pub fn remove() {
    let _guard = WRITE.lock().unwrap_or_else(|e| e.into_inner());
    let _ = std::fs::remove_file(path());
}

/// アイコンの控えがこれより多くなったら、古いものから捨てる
const MAX_IMAGES: usize = 3000;
const KEEP_IMAGES: usize = 2000;

/// アイコンの控えを上限まで減らす（起動時に裏で1回）
pub fn prune_images() {
    let Ok(dir) = std::fs::read_dir(image_dir()) else { return };
    let mut files: Vec<(std::time::SystemTime, PathBuf)> = dir
        .filter_map(|e| e.ok())
        .filter_map(|e| Some((e.metadata().ok()?.modified().ok()?, e.path())))
        .collect();
    if files.len() <= MAX_IMAGES {
        return;
    }
    files.sort();
    for (_, p) in &files[..files.len() - KEEP_IMAGES] {
        let _ = std::fs::remove_file(p);
    }
}
