//! ビルド元のコミットを RISCORD_GIT_SHA として埋め込む（src/commits.rs が GitHub の main と比べる）。
//! CI では GITHUB_SHA、手元では git rev-parse HEAD。どちらも取れなければ空文字（比べない）

use std::process::Command;

fn git(args: &[&str]) -> Option<String> {
    let out = Command::new("git").args(args).output().ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8(out.stdout).ok()?.trim().to_string();
    (!s.is_empty()).then_some(s)
}

fn main() {
    println!("cargo:rerun-if-env-changed=GITHUB_SHA");
    // コミットやブランチの切り替えで埋め込み直す
    for p in ["HEAD", "refs/heads", "packed-refs"] {
        if let Some(path) = git(&["rev-parse", "--git-path", p]) {
            println!("cargo:rerun-if-changed={path}");
        }
    }
    let sha = std::env::var("GITHUB_SHA")
        .ok()
        .filter(|s| !s.is_empty())
        .or_else(|| git(&["rev-parse", "HEAD"]))
        .unwrap_or_default();
    println!("cargo:rustc-env=RISCORD_GIT_SHA={sha}");
}
