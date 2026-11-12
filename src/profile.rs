//! アカウントごとのプロファイル。`--profile <名前>`（または RISCORD_PROFILE）で起動すると、
//! トークン・設定・控えなどを別のフォルダに持ち、別アカウントのウィンドウとして並べて使える。
//! 名前なしは既定のプロファイルで、保存先は従来どおり（設定フォルダの riscord 直下）
use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

static NAME: OnceLock<Option<String>> = OnceLock::new();
/// 起動中はプロファイルの lock を持ち続ける（同じアカウントのウィンドウを2つ開かせない）
static LOCK: OnceLock<File> = OnceLock::new();

pub fn valid(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 32
        && name
            .bytes()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-' || c == b'_')
}

fn from_args() -> Option<String> {
    let mut args = std::env::args().skip(1);
    let mut name = None;
    while let Some(a) = args.next() {
        if a == "--profile" {
            name = args.next();
        } else if let Some(v) = a.strip_prefix("--profile=") {
            name = Some(v.to_string());
        }
    }
    let name = name
        .or_else(|| std::env::var("RISCORD_PROFILE").ok())
        .filter(|n| !n.is_empty())?;
    if valid(&name) {
        Some(name)
    } else {
        eprintln!("プロファイル名は英小文字・数字・-・_ の 32 文字以内にしてください: {name}");
        std::process::exit(2);
    }
}

/// このプロセスのプロファイル名（既定なら None）
pub fn name() -> Option<&'static str> {
    if cfg!(test) {
        return None;
    }
    NAME.get_or_init(from_args).as_deref()
}

/// 画面に出す呼び名。既定はメインアカウント、追加したものはサブアカウント
pub fn label(name: Option<&str>) -> String {
    match name {
        None => "メインアカウント".into(),
        Some("2") => "サブアカウント".into(),
        Some(n) => format!("サブアカウント {n}"),
    }
}

/// 共有のフォルダ（テーマ・プラグインのコードなど、アカウントに依らないもの）
pub fn shared_dir() -> PathBuf {
    crate::app::config_root().join("riscord")
}

fn dir_for(shared: &Path, name: Option<&str>) -> PathBuf {
    match name {
        None => shared.to_path_buf(),
        Some(n) => shared.join("profiles").join(n),
    }
}

/// そのプロファイル（None は既定）のアカウントのフォルダ（トークン・設定・記録など）。
/// 分割表示では1つのプロセスに2つのアカウントがあるので、プロファイルは画面ごとに持つ
pub fn account_dir(name: Option<&str>) -> PathBuf {
    dir_for(&shared_dir(), name)
}

/// 控え（snapshot.json）を置くフォルダ
pub fn cache_dir(name: Option<&str>) -> PathBuf {
    let base = crate::app::cache_root().join("riscord");
    match name {
        None => base,
        Some(n) => base.join("profiles").join(n),
    }
}

/// 既定以外のプロファイルの一覧（名前順）
pub fn list() -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(shared_dir().join("profiles"))
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().is_dir())
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|n| valid(n))
        .collect();
    v.sort_by(|a, b| (a.len(), a).cmp(&(b.len(), b)));
    v
}

/// 新しいプロファイルに使う、空いている番号（2, 3, …）
pub fn next_name(existing: &[String]) -> String {
    (2..)
        .map(|i: u32| i.to_string())
        .find(|n| !existing.contains(n))
        .unwrap()
}

fn open_lock(dir: &Path) -> std::io::Result<File> {
    std::fs::create_dir_all(dir)?;
    std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(dir.join("lock"))
}

/// lock を取る。もう他のウィンドウが持っていれば false
fn try_lock(dir: &Path) -> std::io::Result<Option<File>> {
    let f = open_lock(dir)?;
    match f.try_lock() {
        Ok(()) => Ok(Some(f)),
        Err(std::fs::TryLockError::WouldBlock) => Ok(None),
        Err(std::fs::TryLockError::Error(e)) => Err(e),
    }
}

/// 起動時に呼ぶ。同じプロファイルがもう開いていれば false
pub fn acquire() -> bool {
    if crate::demo::enabled() {
        return true;
    }
    match try_lock(&account_dir(name())) {
        Ok(Some(f)) => {
            let _ = LOCK.set(f);
            true
        }
        Ok(None) => false,
        // lock を作れない環境（読み取り専用など）では止めずに起動する
        Err(e) => {
            eprintln!("プロファイルの lock を取れませんでした: {e}");
            true
        }
    }
}

/// 分割表示でもう1つのアカウントを開くときの lock。持っている間は他のウィンドウで開かせない。
/// もう開いていれば None
pub fn lock(name: Option<&str>) -> Option<File> {
    if crate::demo::enabled() {
        return open_lock(&std::env::temp_dir()).ok();
    }
    match try_lock(&account_dir(name)) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("プロファイルの lock を取れませんでした: {e}");
            open_lock(&std::env::temp_dir()).ok()
        }
    }
}

/// 分割表示で右に開いているプロファイルを覚えておくファイル（左のプロファイルごと）
fn split_file(left: Option<&str>) -> PathBuf {
    account_dir(left).join("split")
}

/// 前回右に開いていたプロファイル。Some(None) は既定のプロファイル
pub fn split_load(left: Option<&str>) -> Option<Option<String>> {
    let s = std::fs::read_to_string(split_file(left)).ok()?;
    match s.trim() {
        "" => None,
        "@" => Some(None),
        n if valid(n) => Some(Some(n.to_string())),
        _ => None,
    }
}

pub fn split_save(left: Option<&str>, right: Option<Option<&str>>) {
    if crate::demo::enabled() {
        return;
    }
    let p = split_file(left);
    match right {
        Some(r) => {
            let _ = std::fs::create_dir_all(p.parent().unwrap());
            let _ = std::fs::write(p, r.unwrap_or("@"));
        }
        None => {
            let _ = std::fs::remove_file(p);
        }
    }
}

/// そのプロファイル（None は既定）のウィンドウが開いているか
pub fn is_running(name: Option<&str>) -> bool {
    matches!(try_lock(&dir_for(&shared_dir(), name)), Ok(None))
}

/// そのプロファイルのウィンドウを新しく開く
pub fn launch(name: Option<&str>) -> std::io::Result<()> {
    let mut cmd = std::process::Command::new(std::env::current_exe()?);
    if let Some(n) = name {
        cmd.arg("--profile").arg(n);
    }
    // 親（このウィンドウ）の指定を引き継がない
    cmd.env_remove("RISCORD_PROFILE");
    cmd.spawn().map(|_| ())
}

/// そのプロファイルで最後にログインしたアカウントの名前を覚える
pub fn remember_account(name: Option<&str>, display: &str) {
    if crate::demo::enabled() {
        return;
    }
    let p = account_dir(name).join("account");
    if std::fs::read_to_string(&p).ok().as_deref() != Some(display) {
        let _ = std::fs::write(p, display);
    }
}

/// そのプロファイル（None は既定）で最後にログインしたアカウントの名前
pub fn account_name(name: Option<&str>) -> Option<String> {
    let s = std::fs::read_to_string(dir_for(&shared_dir(), name).join("account")).ok()?;
    let s = s.trim().to_string();
    (!s.is_empty()).then_some(s)
}

/// プロファイルを作る（フォルダだけ）
pub fn create(name: &str) -> std::io::Result<()> {
    std::fs::create_dir_all(dir_for(&shared_dir(), Some(name)))
}

/// 開いていないプロファイルを、記録・トークンごと消す
pub fn remove(name: &str) -> std::io::Result<()> {
    if !valid(name) || is_running(Some(name)) {
        return Err(std::io::Error::other(
            "開いているプロファイルは削除できません",
        ));
    }
    std::fs::remove_dir_all(dir_for(&shared_dir(), Some(name)))?;
    let cache = crate::app::cache_root()
        .join("riscord")
        .join("profiles")
        .join(name);
    let _ = std::fs::remove_dir_all(cache);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_checked() {
        assert!(valid("2"));
        assert!(valid("work_alt-1"));
        assert!(!valid(""));
        assert!(!valid("../x"));
        assert!(!valid("Main"));
        assert!(!valid(&"a".repeat(33)));
    }

    #[test]
    fn split_partner_is_remembered_per_left_profile() {
        let left = Some("split-left");
        assert_eq!(split_load(left), None);
        split_save(left, Some(None));
        assert_eq!(split_load(left), Some(None), "default profile on the right");
        split_save(left, Some(Some("3")));
        assert_eq!(split_load(left), Some(Some("3".into())));
        assert_eq!(split_load(Some("split-other")), None);
        split_save(left, None);
        assert_eq!(split_load(left), None);
        let _ = std::fs::remove_dir_all(account_dir(left));
    }

    #[test]
    fn default_profile_keeps_old_folder() {
        let shared = Path::new("/c/riscord");
        assert_eq!(dir_for(shared, None), shared);
        assert_eq!(
            dir_for(shared, Some("2")),
            Path::new("/c/riscord/profiles/2")
        );
    }

    #[test]
    fn labels() {
        assert_eq!(label(None), "メインアカウント");
        assert_eq!(label(Some("2")), "サブアカウント");
        assert_eq!(label(Some("3")), "サブアカウント 3");
    }

    #[test]
    fn next_name_skips_used() {
        assert_eq!(next_name(&[]), "2");
        assert_eq!(next_name(&["2".into(), "3".into(), "work".into()]), "4");
    }

    #[test]
    fn same_profile_cannot_be_locked_twice() {
        let dir = std::env::temp_dir().join(format!("riscord-profile-lock-{}", std::process::id()));
        let first = try_lock(&dir).unwrap();
        assert!(first.is_some());
        assert!(try_lock(&dir).unwrap().is_none());
        drop(first);
        assert!(try_lock(&dir).unwrap().is_some());
        let _ = std::fs::remove_dir_all(dir);
    }
}
