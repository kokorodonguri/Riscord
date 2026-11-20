//! Riscord: cargo only tracks this crate's Rust files, so edits to the vendored
//! Lua C sources (our sandbox patches in lstrlib.c) would not rebuild anything.
//! Hash the C sources into an env var that lib.rs embeds: when they change, this
//! crate is recompiled and mlua-sys (which builds Lua through it) reruns too.
use std::hash::{Hash, Hasher};

fn main() {
    let dir = std::path::Path::new("lua-5.4.7");
    println!("cargo:rerun-if-changed={}", dir.display());
    let mut files: Vec<_> = std::fs::read_dir(dir)
        .expect("lua-5.4.7")
        .flatten()
        .map(|e| e.path())
        .collect();
    files.sort();
    let mut h = std::collections::hash_map::DefaultHasher::new();
    for f in files {
        println!("cargo:rerun-if-changed={}", f.display());
        f.file_name().hash(&mut h);
        std::fs::read(&f).unwrap_or_default().hash(&mut h);
    }
    println!("cargo:rustc-env=RISCORD_LUA_SRC_HASH={:016x}", h.finish());
}
