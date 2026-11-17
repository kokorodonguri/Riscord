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
    bundled_plugins();
}

/// plugins/<フォルダ>/plugin.toml と Lua を同梱プラグインの表（OUT_DIR/bundled_plugins.rs）にする
fn bundled_plugins() {
    let root = std::path::Path::new(&std::env::var("CARGO_MANIFEST_DIR").unwrap()).join("plugins");
    println!("cargo:rerun-if-changed={}", root.display());
    let mut dirs: Vec<_> = std::fs::read_dir(&root)
        .map(|it| {
            it.flatten()
                .map(|e| e.path())
                .filter(|p| p.join("plugin.toml").is_file())
                .collect()
        })
        .unwrap_or_default();
    dirs.sort();
    let mut out = String::from("pub static BUNDLED: &[(&str, &str)] = &[\n");
    for dir in dirs {
        println!("cargo:rerun-if-changed={}", dir.display());
        for f in ["plugin.toml", "main.lua"] {
            println!("cargo:rerun-if-changed={}", dir.join(f).display());
        }
        out.push_str(&format!(
            "    (include_str!({:?}), include_str!({:?})),\n",
            dir.join("plugin.toml"),
            dir.join("main.lua")
        ));
    }
    out.push_str("];\n");
    let dest = std::path::Path::new(&std::env::var("OUT_DIR").unwrap()).join("bundled_plugins.rs");
    std::fs::write(dest, out).unwrap();
}
