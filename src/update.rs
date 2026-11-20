//! 自動アップデート。起動時と、その後は数時間おきに GitHub の最新の正式リリースを見て、新しければ裏で
//! ダウンロードし、実行ファイルを差し替える。新しい版が使われるのは次に起動したときから。
//! 差し替えたら OS の通知と、画面上部の帯（banner）で知らせる。
//!
//! - releases/latest はプレリリースと下書きを返さないので、正式リリースだけが対象になる
//! - 差し替えの前に、埋め込まれた公開鍵で署名を確認する
//! - debug ビルドと、target/ の下から動かしている開発中のバイナリは触らない
//! - 環境変数 RISCORD_NO_UPDATE を設定すると確認もしない

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
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
/// 起動中に確認し直す間隔
const RECHECK: Duration = Duration::from_secs(3 * 60 * 60);
/// 「更新しました」の帯を閉じたか
static BANNER_CLOSED: AtomicBool = AtomicBool::new(false);

/// 画面上部の帯に出す版（差し替え済みで、まだ閉じていないとき）
pub fn banner() -> Option<String> {
    match status() {
        Status::Ready(v) if !BANNER_CLOSED.load(Ordering::Relaxed) => Some(v),
        _ => None,
    }
}

pub fn close_banner() {
    BANNER_CLOSED.store(true, Ordering::Relaxed);
}

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
const TARGETS: [(&str, &str); 4] = [
    ("windows", "x86_64"),
    ("windows", "aarch64"),
    ("linux", "x86_64"),
    ("linux", "aarch64"),
];

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
    for arch in [native_arch(), Some(std::env::consts::ARCH)]
        .into_iter()
        .flatten()
    {
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

/// 起動時に一度だけ呼ぶ（2回目以降は何もしない）。確認は裏のスレッドが続ける
pub fn start(waker: &Waker) {
    START.call_once(|| {
        let Some(exe) = updatable_exe() else { return };
        // 前回の差し替えで残った古い版を片付ける（Windows では実行中の exe を消せないので次の起動で）
        let _ = std::fs::remove_file(sibling(&exe, ".old"));
        // 差し替えで退けた古い版（.old-*）。動いているものは Windows では消せないので、消せたものだけ片付く
        remove_siblings(&exe, ".old-", Duration::ZERO);
        // 以前の版が使っていた固定名の一時ファイルと、落ちたインスタンスの残り
        let _ = std::fs::remove_file(sibling(&exe, ".new"));
        remove_stale_downloads(&exe);
        let names = asset_names();
        if names.is_empty() {
            return;
        }
        let waker = waker.clone();
        std::thread::spawn(move || {
            set(Status::Checking, &waker);
            let rt = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(rt) => rt,
                Err(e) => return set(Status::Failed(e.to_string()), &waker),
            };
            // 起動しっぱなしでも新しい版に気づけるよう、間をおいて確認し直す。差し替えたら終わり
            loop {
                match rt.block_on(run(&exe, &names, &waker)) {
                    Ok(Outcome::Latest) => set(Status::Latest, &waker),
                    // 別のプロファイルの Riscord が差し替え中。終わったころにもう一度見る
                    Ok(Outcome::Busy) => {
                        std::thread::sleep(BUSY_RETRY);
                        continue;
                    }
                    Ok(Outcome::Ready(v)) => {
                        set(Status::Ready(v.clone()), &waker);
                        notify_ready(&v);
                        return;
                    }
                    Err(e) => {
                        eprintln!("自動アップデートに失敗: {e:#}");
                        set(Status::Failed(format!("{e:#}")), &waker);
                    }
                }
                std::thread::sleep(RECHECK);
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

const UPDATE_KEY: Option<&str> = option_env!("RISCORD_UPDATE_PUBLIC_KEY");
const MAX_UPDATE_BYTES: u64 = 512 * 1024 * 1024;

fn signed_update_message(version: &str, name: &str, digest: &str) -> String {
    format!("riscord-update-v1\n{version}\n{name}\n{digest}\n")
}

fn verify_update(
    key: &str,
    version: &str,
    name: &str,
    digest: &str,
    signature: &[u8],
) -> Result<()> {
    if key.len() != 64 || !key.bytes().all(|b| b.is_ascii_hexdigit()) {
        bail!("Invalid update verification key");
    }
    let public_key = (0..32)
        .map(|i| u8::from_str_radix(&key[i * 2..i * 2 + 2], 16))
        .collect::<std::result::Result<Vec<_>, _>>()?;
    ring::signature::UnparsedPublicKey::new(&ring::signature::ED25519, public_key)
        .verify(
            signed_update_message(version, name, digest).as_bytes(),
            signature,
        )
        .map_err(|_| anyhow!("Update signature verification failed"))
}

async fn bounded_body(mut response: reqwest::Response, limit: u64) -> Result<Vec<u8>> {
    if response.content_length().is_some_and(|size| size > limit) {
        bail!("Download exceeds size limit");
    }
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        if (body.len() as u64).saturating_add(chunk.len() as u64) > limit {
            bail!("Download exceeds size limit");
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

/// 新しい版に差し替えたらその版を返す。最新なら None
/// 1 回の確認の結果
enum Outcome {
    Latest,
    /// 他のインスタンスが差し替え中で、ロックを取れなかった
    Busy,
    /// この版に差し替えた（または他のインスタンスが差し替え済み）
    Ready(String),
}

/// ロックを取れなかったときに、もう一度確かめるまでの間
const BUSY_RETRY: Duration = Duration::from_secs(60);

async fn run(exe: &Path, names: &[&str], waker: &Waker) -> Result<Outcome> {
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
        return Ok(Outcome::Latest);
    }
    let version = rel.tag_name.clone();
    // 別のプロファイルで動いている Riscord も同じ exe を更新しうるので、ダウンロードの前に
    // ロックを取り、差し替えは 1 つずつにする。先に差し替えたインスタンスがいれば、同じ版を入れ直さない
    let Some(_lock) = UpdateLock::acquire(exe) else {
        return Ok(Outcome::Busy);
    };
    if installed_version(exe).as_deref() == Some(version.as_str()) {
        return Ok(Outcome::Ready(version));
    }
    let asset = names
        .iter()
        .find_map(|n| rel.assets.iter().find(|a| a.name == *n))
        .ok_or_else(|| {
            anyhow!(
                "{}",
                tr!(
                    "{version} に {} がありません",
                    "{version} has no {}",
                    "{version} 中没有 {}",
                    names.join(" / ")
                )
            )
        })?;
    let name = &asset.name;
    let want = asset
        .digest
        .as_deref()
        .and_then(|d| d.strip_prefix("sha256:"))
        .ok_or_else(|| {
            anyhow!(
                "{}",
                tr!(
                    "{name} に SHA-256 が付いていません",
                    "{name} has no SHA-256",
                    "{name} 没有 SHA-256",
                )
            )
        })?
        .to_ascii_lowercase();

    let key = UPDATE_KEY.ok_or_else(|| anyhow!("Update verification key is not configured"))?;
    if asset.size == 0 || asset.size > MAX_UPDATE_BYTES {
        bail!("Update asset exceeds size limit");
    }
    let signature_name = format!("{name}.sig");
    let signature_asset = rel
        .assets
        .iter()
        .find(|a| a.name == signature_name)
        .ok_or_else(|| anyhow!("Update signature is missing"))?;
    if signature_asset.size != 64 {
        bail!("Invalid update signature size");
    }
    let signature_response = http
        .get(&signature_asset.browser_download_url)
        .timeout(Duration::from_secs(30))
        .send()
        .await?
        .error_for_status()?;
    let signature = bounded_body(signature_response, 64).await?;
    if signature.len() != 64 {
        bail!("Invalid update signature size");
    }
    verify_update(key, &version, name, &want, &signature)?;

    set(Status::Downloading(version.clone()), waker);
    let response = http
        .get(&asset.browser_download_url)
        .timeout(Duration::from_secs(600))
        .send()
        .await?
        .error_for_status()?;
    let bytes = bounded_body(response, asset.size).await?;
    if bytes.len() as u64 != asset.size {
        bail!(
            "{}",
            tr!(
                "ダウンロードしたサイズが合いません（{} / {} バイト）",
                "The downloaded size doesn't match ({} / {} bytes)",
                "下载的大小不匹配（{} / {} 字节）",
                bytes.len(),
                asset.size
            )
        );
    }
    let got: String = Sha256::digest(&bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    if got != want {
        bail!(
            "{}",
            tr!(
                "ダウンロードしたファイルの SHA-256 が合いません",
                "The SHA-256 of the downloaded file doesn't match",
                "下载文件的 SHA-256 不匹配",
            )
        );
    }

    // 一時ファイルはインスタンスごとの名前にする（落ちたインスタンスの残りと混ざらないように）
    let new = sibling(
        exe,
        &format!(".new-{}-{:08x}", std::process::id(), rand::random::<u32>()),
    );
    let written = (|| -> std::io::Result<()> {
        use std::io::Write;
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&new)?;
        f.write_all(&bytes)?;
        // rename の前に中身を確実にディスクへ（途中で電源が落ちても壊れた exe を残さない）
        f.sync_all()
    })();
    if let Err(e) = written {
        let _ = std::fs::remove_file(&new);
        return Err(e).with_context(|| {
            tr!(
                "{} に書き込めません",
                "Can't write to {}",
                "无法写入 {}",
                new.display()
            )
        });
    }
    if let Err(e) = replace(exe, &new) {
        let _ = std::fs::remove_file(&new);
        return Err(e);
    }
    // 他のインスタンスが同じ版を入れ直さないよう、差し替えた版を書いておく
    let _ = std::fs::write(sibling(exe, ".updated"), &version);
    Ok(Outcome::Ready(version))
}

/// 差し替え済みの版（exe の隣の .updated）。起動中の版より新しいときだけ意味がある
fn installed_version(exe: &Path) -> Option<String> {
    std::fs::read_to_string(sibling(exe, ".updated"))
        .ok()
        .map(|s| s.trim().to_owned())
}

/// exe の差し替えを 1 インスタンスずつにするロック（exe の隣の .update-lock）。
/// 中に自分の印を書き、drop で消すのは自分の印のときだけ（取り直された他人のロックを消さない）
struct UpdateLock {
    path: PathBuf,
    token: String,
}

/// これより古いロックは、落ちたインスタンスの残りとみなして取り直す
const UPDATE_LOCK_STALE: Duration = Duration::from_secs(10 * 60);

impl UpdateLock {
    fn acquire(exe: &Path) -> Option<UpdateLock> {
        use std::io::Write;
        let path = sibling(exe, ".update-lock");
        let token = format!("{}-{:016x}", std::process::id(), rand::random::<u64>());
        for _ in 0..2 {
            match std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)
            {
                Ok(mut f) => {
                    if f.write_all(token.as_bytes()).is_err() {
                        drop(f);
                        let _ = std::fs::remove_file(&path);
                        return None;
                    }
                    return Some(UpdateLock { path, token });
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                    // 時刻が未来（時計が戻った）なら、いつまでも古くならないので古いとみなす
                    let stale = std::fs::metadata(&path)
                        .and_then(|m| m.modified())
                        .map(|t| {
                            t.elapsed().is_ok_and(|age| age > UPDATE_LOCK_STALE)
                                || t.elapsed().is_err()
                        })
                        .unwrap_or(false);
                    if !stale {
                        return None;
                    }
                    // 古いロックは消さずに別の名前へ移して奪う。rename に成功するのは 1 つだけなので、
                    // 2 つのインスタンスが同時に「古い」と判断しても、片方が作り直したロックを消さない
                    let grave = sibling(exe, &format!(".update-lock.stale-{token}"));
                    if std::fs::rename(&path, &grave).is_err() {
                        return None;
                    }
                    // 古いと判断してから移すまでの間に、他のインスタンスが作り直した新しいロックを
                    // 移してしまったかもしれない。新しければ（上書きせずに）元へ戻して諦める
                    let fresh = std::fs::metadata(&grave)
                        .and_then(|m| m.modified())
                        .is_ok_and(|t| t.elapsed().is_ok_and(|age| age <= UPDATE_LOCK_STALE));
                    if fresh {
                        let _ = std::fs::hard_link(&grave, &path);
                        let _ = std::fs::remove_file(&grave);
                        return None;
                    }
                    let _ = std::fs::remove_file(&grave);
                }
                Err(_) => return None,
            }
        }
        None
    }
}

impl Drop for UpdateLock {
    fn drop(&mut self) {
        if std::fs::read_to_string(&self.path).is_ok_and(|t| t == self.token) {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

/// 落ちたインスタンスが残した一時ファイル（.new-*）を片付ける。
/// 他のインスタンスが書いている最中のものを消さないよう、古いものだけ
fn remove_stale_downloads(exe: &Path) {
    remove_siblings(exe, ".new-", UPDATE_LOCK_STALE);
}

/// exe の隣の「exe の名前 + suffix」で始まるファイルのうち、min_age より古いものを消す
fn remove_siblings(exe: &Path, suffix: &str, min_age: Duration) {
    let (Some(dir), Some(name)) = (exe.parent(), exe.file_name()) else {
        return;
    };
    let mut prefix = name.to_os_string();
    prefix.push(suffix);
    let prefix = prefix.to_string_lossy().into_owned();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        if !entry.file_name().to_string_lossy().starts_with(&prefix) {
            continue;
        }
        let old = entry
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.elapsed().ok())
            .is_some_and(|age| age >= min_age);
        if old {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

/// 実行中のファイルを new に差し替える
fn replace(exe: &Path, new: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(new, std::fs::Permissions::from_mode(0o755))?;
        // 実行中でも rename で置き換えられる（動いているプロセスは古い中身のまま）
        std::fs::rename(new, exe).context(tr!(
            "実行ファイルを置き換えられません",
            "Can't replace the executable",
            "无法替换可执行文件"
        ))?;
    }
    #[cfg(windows)]
    {
        // 実行中の exe は上書きも削除もできないが、名前は変えられる
        // 名前は差し替えごとに変える。別のプロファイルの Riscord が前の .old から動いていると、
        // 固定の名前では消すことも上書きすることもできず、差し替えが毎回失敗するため
        let old = sibling(
            exe,
            &format!(".old-{}-{:08x}", std::process::id(), rand::random::<u32>()),
        );
        std::fs::rename(exe, &old).context(tr!(
            "実行ファイルの名前を変えられません",
            "Can't rename the executable",
            "无法重命名可执行文件"
        ))?;
        if let Err(e) = std::fs::rename(new, exe) {
            let _ = std::fs::rename(&old, exe);
            return Err(e).context(tr!(
                "新しい実行ファイルを置けません",
                "Can't place the new executable",
                "无法放置新的可执行文件"
            ));
        }
    }
    Ok(())
}

fn notify_ready(version: &str) {
    let body = tr!(
        "{version} に更新しました。再起動すると新しい版になります。",
        "Updated to {version}. Restart to use the new version.",
        "已更新到 {version}。重启后即可使用新版本。",
    );
    if let Err(e) = notify_rust::Notification::new()
        .appname("Riscord")
        .summary(tr!(
            "Riscord を更新しました",
            "Riscord updated",
            "Riscord 已更新"
        ))
        .body(&body)
        .show()
    {
        eprintln!("通知を出せませんでした: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stale_lock_is_taken_over_but_others_lock_is_never_removed() {
        let dir = std::env::temp_dir().join(format!("riscord-update-stale-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let exe = dir.join("riscord");
        let lock = sibling(&exe, ".update-lock");
        // 落ちたインスタンスの古いロック
        std::fs::write(&lock, "dead").unwrap();
        let old = std::time::SystemTime::now() - UPDATE_LOCK_STALE - Duration::from_secs(60);
        std::fs::File::options()
            .write(true)
            .open(&lock)
            .unwrap()
            .set_modified(old)
            .unwrap();
        let a = UpdateLock::acquire(&exe).expect("古いロックは取り直せる");
        // 他人に取り直されたら、自分の drop では消さない
        std::fs::write(&lock, "someone else").unwrap();
        drop(a);
        assert_eq!(std::fs::read_to_string(&lock).unwrap(), "someone else");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn update_lock_is_exclusive_and_released_on_drop() {
        let dir = std::env::temp_dir().join(format!("riscord-update-lock-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let exe = dir.join("riscord");
        let a = UpdateLock::acquire(&exe).expect("最初は取れる");
        assert!(
            UpdateLock::acquire(&exe).is_none(),
            "他のインスタンスは待つ"
        );
        drop(a);
        assert!(UpdateLock::acquire(&exe).is_some(), "手放したら取れる");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn signature_binds_version_name_and_digest() {
        use ring::signature::KeyPair;
        let rng = ring::rand::SystemRandom::new();
        let pkcs8 = ring::signature::Ed25519KeyPair::generate_pkcs8(&rng).unwrap();
        let pair = ring::signature::Ed25519KeyPair::from_pkcs8(pkcs8.as_ref()).unwrap();
        let key: String = pair
            .public_key()
            .as_ref()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        let message = signed_update_message("v1.2.3", "riscord-linux-x64", "abc");
        let signature = pair.sign(message.as_bytes());
        assert!(verify_update(
            &key,
            "v1.2.3",
            "riscord-linux-x64",
            "abc",
            signature.as_ref()
        )
        .is_ok());
        assert!(verify_update(
            &key,
            "v1.2.4",
            "riscord-linux-x64",
            "abc",
            signature.as_ref()
        )
        .is_err());
        assert!(verify_update(
            &key,
            "v1.2.3",
            "riscord-linux-arm64",
            "abc",
            signature.as_ref()
        )
        .is_err());
        assert!(verify_update(
            &key,
            "v1.2.3",
            "riscord-linux-x64",
            "def",
            signature.as_ref()
        )
        .is_err());
    }

    #[test]
    fn banner_shows_after_replacing_until_closed() {
        *STATUS.lock().unwrap() = Status::Latest;
        assert_eq!(banner(), None);
        *STATUS.lock().unwrap() = Status::Ready("v9.9.9".into());
        assert_eq!(banner().as_deref(), Some("v9.9.9"));
        close_banner();
        assert_eq!(banner(), None);
        *STATUS.lock().unwrap() = Status::Off;
        BANNER_CLOSED.store(false, Ordering::Relaxed);
    }

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
        let mut in_workflow: Vec<&str> = yml
            .lines()
            .filter_map(|l| l.trim().strip_prefix("asset: "))
            .map(str::trim)
            .collect();
        let mut in_code: Vec<&str> = TARGETS
            .iter()
            .map(|(os, arch)| asset_for(os, arch).unwrap())
            .collect();
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
