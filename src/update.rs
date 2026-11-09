//! 自動アップデート。起動時に GitHub の最新の正式リリースを見て、新しければ裏でダウンロードし、
//! 実行ファイルを差し替える。新しい版が使われるのは次に起動したときから。
//!
//! - releases/latest はプレリリースと下書きを返さないので、正式リリースだけが対象になる
//! - 差し替えの前に、サイズと SHA-256（GitHub が asset に付ける digest）を確かめる
//! - debug ビルドと、target/ の下から動かしている開発中のバイナリは触らない
//! - 環境変数 RISCORD_NO_UPDATE を設定すると確認もしない

use std::path::{Path, PathBuf};
use std::sync::{Mutex, Once};
use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::wake::Waker;

const LATEST: &str = "https://api.github.com/repos/kokorodonguri/Riscord/releases/latest";
const CURRENT: &str = env!("CARGO_PKG_VERSION");

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Status {
    /// 確認していない（開発中のビルド・止めてある・対応していない環境）
    Off,
    Checking,
    Latest,
    Downloading(String),
    /// 差し替え済み。次に起動したときから使われる
    Ready(String),
    Failed(String),
}

static STATUS: Mutex<Status> = Mutex::new(Status::Off);
static START: Once = Once::new();

pub fn current_version() -> &'static str {
    CURRENT
}

pub fn status() -> Status {
    STATUS.lock().unwrap().clone()
}

fn set(s: Status, waker: &Waker) {
    *STATUS.lock().unwrap() = s;
    waker.wake();
}

/// 配布物の名前（この OS・CPU 向けのものがなければ None）
fn asset_name() -> Option<&'static str> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("windows", "x86_64") => Some("riscord-windows-x64.exe"),
        ("linux", "x86_64") => Some("riscord-linux-x64"),
        _ => None,
    }
}

/// "v1.2.3" や "1.2.3" を数の組にする
fn parse_version(s: &str) -> Option<(u64, u64, u64)> {
    let s = s.trim().trim_start_matches('v');
    let s = s.split(['-', '+']).next()?;
    let mut it = s.split('.').map(|p| p.parse::<u64>().ok());
    let v = (it.next()??, it.next()??, it.next()??);
    it.next().is_none().then_some(v)
}

fn is_newer(tag: &str, current: &str) -> bool {
    matches!((parse_version(tag), parse_version(current)), (Some(a), Some(b)) if a > b)
}

/// 自動で差し替えてよい実行ファイルか（開発中のビルドは触らない）
fn updatable_exe() -> Option<PathBuf> {
    if cfg!(debug_assertions) || cfg!(test) || std::env::var_os("RISCORD_NO_UPDATE").is_some() {
        return None;
    }
    let exe = std::env::current_exe().ok()?;
    let exe = exe.canonicalize().unwrap_or(exe);
    if exe.components().any(|c| c.as_os_str() == "target") {
        return None;
    }
    Some(exe)
}

fn sibling(exe: &Path, suffix: &str) -> PathBuf {
    let mut s = exe.as_os_str().to_owned();
    s.push(suffix);
    PathBuf::from(s)
}

/// 起動時に一度だけ呼ぶ（2回目以降は何もしない）
pub fn start(waker: &Waker) {
    START.call_once(|| {
        let Some(exe) = updatable_exe() else { return };
        // 前回の差し替えで残った古い版を片付ける（Windows では実行中の exe を消せないので次の起動で）
        let _ = std::fs::remove_file(sibling(&exe, ".old"));
        let _ = std::fs::remove_file(sibling(&exe, ".new"));
        let Some(name) = asset_name() else { return };
        let waker = waker.clone();
        std::thread::spawn(move || {
            set(Status::Checking, &waker);
            let rt = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
                Ok(rt) => rt,
                Err(e) => return set(Status::Failed(e.to_string()), &waker),
            };
            match rt.block_on(run(&exe, name, &waker)) {
                Ok(None) => set(Status::Latest, &waker),
                Ok(Some(v)) => {
                    set(Status::Ready(v.clone()), &waker);
                    notify_ready(&v);
                }
                Err(e) => {
                    eprintln!("自動アップデートに失敗: {e:#}");
                    set(Status::Failed(format!("{e:#}")), &waker);
                }
            }
        });
    });
}

#[derive(Deserialize)]
struct Release {
    tag_name: String,
    assets: Vec<Asset>,
}

#[derive(Deserialize)]
struct Asset {
    name: String,
    size: u64,
    browser_download_url: String,
    digest: Option<String>,
}

/// 新しい版に差し替えたらその版を返す。最新なら None
async fn run(exe: &Path, name: &str, waker: &Waker) -> Result<Option<String>> {
    let http = reqwest::Client::builder()
        .user_agent(format!("riscord/{CURRENT}"))
        .connect_timeout(Duration::from_secs(10))
        .build()?;
    let rel: Release = http
        .get(LATEST)
        .header("Accept", "application/vnd.github+json")
        .timeout(Duration::from_secs(30))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    if !is_newer(&rel.tag_name, CURRENT) {
        return Ok(None);
    }
    let version = rel.tag_name.clone();
    let asset = rel.assets.iter().find(|a| a.name == name).ok_or_else(|| anyhow!("{version} に {name} がありません"))?;
    let want = asset
        .digest
        .as_deref()
        .and_then(|d| d.strip_prefix("sha256:"))
        .ok_or_else(|| anyhow!("{name} に SHA-256 が付いていません"))?
        .to_ascii_lowercase();

    set(Status::Downloading(version.clone()), waker);
    let bytes = http.get(&asset.browser_download_url).timeout(Duration::from_secs(600)).send().await?.error_for_status()?.bytes().await?;
    if bytes.len() as u64 != asset.size {
        bail!("ダウンロードしたサイズが合いません（{} / {} バイト）", bytes.len(), asset.size);
    }
    let got: String = Sha256::digest(&bytes).iter().map(|b| format!("{b:02x}")).collect();
    if got != want {
        bail!("ダウンロードしたファイルの SHA-256 が合いません");
    }

    let new = sibling(exe, ".new");
    std::fs::write(&new, &bytes).with_context(|| format!("{} に書き込めません", new.display()))?;
    if let Err(e) = replace(exe, &new) {
        let _ = std::fs::remove_file(&new);
        return Err(e);
    }
    Ok(Some(version))
}

/// 実行中のファイルを new に差し替える
fn replace(exe: &Path, new: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(new, std::fs::Permissions::from_mode(0o755))?;
        // 実行中でも rename で置き換えられる（動いているプロセスは古い中身のまま）
        std::fs::rename(new, exe).context("実行ファイルを置き換えられません")?;
    }
    #[cfg(windows)]
    {
        // 実行中の exe は上書きも削除もできないが、名前は変えられる
        let old = sibling(exe, ".old");
        let _ = std::fs::remove_file(&old);
        std::fs::rename(exe, &old).context("実行ファイルの名前を変えられません")?;
        if let Err(e) = std::fs::rename(new, exe) {
            let _ = std::fs::rename(&old, exe);
            return Err(e).context("新しい実行ファイルを置けません");
        }
    }
    Ok(())
}

fn notify_ready(version: &str) {
    let body = format!("{version} に更新しました。次に起動したときから使われます。");
    if let Err(e) = notify_rust::Notification::new().appname("Riscord").summary("Riscord を更新しました").body(&body).show() {
        eprintln!("通知を出せませんでした: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_versions() {
        assert_eq!(parse_version("v0.2.0"), Some((0, 2, 0)));
        assert_eq!(parse_version("1.10.3"), Some((1, 10, 3)));
        assert_eq!(parse_version("v1.0.0-beta.1"), Some((1, 0, 0)));
        assert_eq!(parse_version("v1.0"), None);
        assert_eq!(parse_version("v1.0.0.1"), None);
        assert_eq!(parse_version("latest"), None);
    }

    #[test]
    fn compares_versions() {
        assert!(is_newer("v0.2.0", "0.1.0"));
        assert!(is_newer("v0.10.0", "0.9.9"));
        assert!(!is_newer("v0.1.0", "0.1.0"));
        assert!(!is_newer("v0.0.9", "0.1.0"));
        assert!(!is_newer("nightly", "0.1.0"));
    }
}
