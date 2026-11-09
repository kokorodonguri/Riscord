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

/// 配布しているすべての (OS, CPU)。.github/workflows/release.yml の asset と一致していることをテストで確かめる
#[cfg(test)]
const TARGETS: [(&str, &str); 4] = [("windows", "x86_64"), ("windows", "aarch64"), ("linux", "x86_64"), ("linux", "aarch64")];

/// 配布物の名前（その OS・CPU 向けのものがなければ None）
fn asset_for(os: &str, arch: &str) -> Option<&'static str> {
    match (os, arch) {
        ("windows", "x86_64") => Some("riscord-windows-x64.exe"),
        ("windows", "aarch64") => Some("riscord-windows-arm64.exe"),
        ("linux", "x86_64") => Some("riscord-linux-x64"),
        ("linux", "aarch64") => Some("riscord-linux-arm64"),
        _ => None,
    }
}

/// 取りに行く配布物の名前を、使いたい順に並べる。
/// 実機の CPU 向けを先にする（Windows on ARM で x64 版をエミュレーションで動かしていても arm64 版に移れる）。
/// その版に実機向けが載っていなければ、いま動いているものと同じ CPU 向けを取る
fn asset_names() -> Vec<&'static str> {
    let os = std::env::consts::OS;
    let mut names = Vec::new();
    for arch in [native_arch(), Some(std::env::consts::ARCH)].into_iter().flatten() {
        if let Some(name) = asset_for(os, arch) {
            if !names.contains(&name) {
                names.push(name);
            }
        }
    }
    names
}

/// 実機の CPU（エミュレーションで動いていても実機のものを返す）。分からなければ None
#[cfg(windows)]
fn native_arch() -> Option<&'static str> {
    use std::ffi::c_void;
    #[link(name = "kernel32")]
    extern "system" {
        fn GetModuleHandleW(name: *const u16) -> *mut c_void;
        fn GetProcAddress(module: *mut c_void, name: *const u8) -> *mut c_void;
        fn GetCurrentProcess() -> *mut c_void;
    }
    type IsWow64Process2 = unsafe extern "system" fn(*mut c_void, *mut u16, *mut u16) -> i32;
    const IMAGE_FILE_MACHINE_AMD64: u16 = 0x8664;
    const IMAGE_FILE_MACHINE_ARM64: u16 = 0xAA64;

    let kernel32: Vec<u16> = "kernel32.dll\0".encode_utf16().collect();
    // IsWow64Process2 は Windows 10 1511 から。古い Windows でも起動できるよう、直接リンクせずに探す
    unsafe {
        let module = GetModuleHandleW(kernel32.as_ptr());
        if module.is_null() {
            return None;
        }
        let f = GetProcAddress(module, c"IsWow64Process2".as_ptr().cast());
        if f.is_null() {
            return None;
        }
        let f: IsWow64Process2 = std::mem::transmute(f);
        let (mut process, mut native) = (0u16, 0u16);
        if f(GetCurrentProcess(), &mut process, &mut native) == 0 {
            return None;
        }
        match native {
            IMAGE_FILE_MACHINE_AMD64 => Some("x86_64"),
            IMAGE_FILE_MACHINE_ARM64 => Some("aarch64"),
            _ => None,
        }
    }
}

#[cfg(not(windows))]
fn native_arch() -> Option<&'static str> {
    None
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
        let names = asset_names();
        if names.is_empty() {
            return;
        }
        let waker = waker.clone();
        std::thread::spawn(move || {
            set(Status::Checking, &waker);
            let rt = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
                Ok(rt) => rt,
                Err(e) => return set(Status::Failed(e.to_string()), &waker),
            };
            match rt.block_on(run(&exe, &names, &waker)) {
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
async fn run(exe: &Path, names: &[&str], waker: &Waker) -> Result<Option<String>> {
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
    let asset = names
        .iter()
        .find_map(|n| rel.assets.iter().find(|a| a.name == *n))
        .ok_or_else(|| anyhow!("{version} に {} がありません", names.join(" / ")))?;
    let name = &asset.name;
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

    /// release.yml が載せる配布物と、ここで探す名前が食い違っていないか
    #[test]
    fn asset_names_match_release_workflow() {
        let yml = include_str!("../.github/workflows/release.yml");
        let mut in_workflow: Vec<&str> =
            yml.lines().filter_map(|l| l.trim().strip_prefix("asset: ")).map(str::trim).collect();
        let mut in_code: Vec<&str> = TARGETS.iter().map(|(os, arch)| asset_for(os, arch).unwrap()).collect();
        in_workflow.sort();
        in_code.sort();
        assert_eq!(in_workflow, in_code);
    }

    #[test]
    fn running_build_has_an_asset() {
        if let Some(name) = asset_for(std::env::consts::OS, std::env::consts::ARCH) {
            assert!(asset_names().contains(&name));
        }
    }
}
