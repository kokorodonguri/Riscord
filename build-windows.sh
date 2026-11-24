#!/usr/bin/env bash
# リモートのUbuntu (cargo-xwin入り) でWindows版をビルドして dist/ に持ってくる
# リモートに必要なもの: rustup + cargo-xwin、~/.local/llvm18（clang-cl・llvm-rc）、cmake、ninja
set -euo pipefail
: "${HOST:?Set HOST to the remote SSH destination}"
cd "$(dirname "$0")"
# git が追跡しているファイルと、.gitignore に当たらない未追跡のファイルだけを送る（未コミットの変更も含む）。
# 作業フォルダに置いた .env・鍵・トークンなど、無視しているファイルはリモートに置かない。
# .git がないとリモートの build.rs がコミットを読めないので、GITHUB_SHA として渡す
sha="$(git rev-parse HEAD 2>/dev/null || true)"
git ls-files -z --cached --others --exclude-standard \
  | while IFS= read -r -d '' f; do [ -e "$f" ] && printf '%s\0' "$f"; done \
  | grep -zv -e '^\.claude/' -e '^\.serena/' -e 'local\.properties$' \
  | tar --null -T - -czf - | ssh "$HOST" 'rm -rf ~/build/riscord && mkdir -p ~/build/riscord && tar -xzf - -C ~/build/riscord'
ssh "$HOST" "GITHUB_SHA='$sha' bash -s" <<'EOF'
set -euo pipefail
. ~/.cargo/env
export PATH=$HOME/.local/bin:$HOME/.local/llvm18/bin:$PATH
# Opus の CMake は clang-cl に SSE4.1/AVX2 用のフラグを付けないので、その部分を外して別にビルドしておく
OPUS_OUT=$HOME/build/opus-win
SRC=$(ls -d ~/.cargo/registry/src/*/libopus_sys-0.3.3/opus | head -1)
X=$HOME/.cache/cargo-xwin/xwin
FL="-nologo -MD -Brepro --target=x86_64-pc-windows-msvc -W0 -Wno-unused-command-line-argument -fuse-ld=lld-link /imsvc $X/crt/include /imsvc $X/sdk/include/ucrt /imsvc $X/sdk/include/um /imsvc $X/sdk/include/shared"
OPUS_ARGS=(
  -DCMAKE_TOOLCHAIN_FILE=$HOME/.cache/cargo-xwin/cmake/clang-cl/x86_64-pc-windows-msvc-toolchain.cmake
  -DCMAKE_BUILD_TYPE=Release -DCMAKE_INSTALL_PREFIX="$OPUS_OUT" -DCMAKE_INSTALL_LIBDIR=lib
  -DBUILD_SHARED_LIBS=OFF -DOPUS_BUILD_SHARED_LIBRARY=OFF -DOPUS_BUILD_TESTING=OFF -DOPUS_BUILD_PROGRAMS=OFF
  -DOPUS_INSTALL_PKG_CONFIG_MODULE=OFF -DOPUS_X86_MAY_HAVE_SSE4_1=OFF -DOPUS_X86_MAY_HAVE_AVX2=OFF
  "-DCMAKE_C_FLAGS=$FL" "-DCMAKE_C_FLAGS_RELEASE=$FL /O2"
)
# ソースの中身・ビルド設定・コンパイラの版をまとめたハッシュをスタンプに書いておき、どれかが変わったら作り直す
# （opus.lib があるだけで使い回すと、ソースやフラグを変えても古いライブラリがリンクされる）
STAMP=$( {
  printf '%s\n' "$SRC" "${OPUS_ARGS[@]}"
  (cd "$SRC" && find . -type f -print0 | LC_ALL=C sort -z | xargs -0 sha256sum)
  sha256sum "$HOME/.cache/cargo-xwin/cmake/clang-cl/x86_64-pc-windows-msvc-toolchain.cmake"
  clang-cl --version 2>/dev/null | head -1 || true
  cmake --version | head -1 || true
} | sha256sum | cut -d ' ' -f 1 )
if [ ! -f "$OPUS_OUT/lib/opus.lib" ] || [ "$(cat "$OPUS_OUT/.stamp" 2>/dev/null || true)" != "$STAMP" ]; then
  # 前の設定の CMake キャッシュや古い成果物を引き継がないよう、まっさらにしてから作る
  rm -rf "$OPUS_OUT"
  PATH=$HOME/.cache/cargo-xwin:$PATH cmake -S "$SRC" -B "$OPUS_OUT/build" -G Ninja "${OPUS_ARGS[@]}" >/dev/null
  PATH=$HOME/.cache/cargo-xwin:$PATH cmake --build "$OPUS_OUT/build" >/dev/null
  cmake --install "$OPUS_OUT/build" >/dev/null
  printf '%s\n' "$STAMP" > "$OPUS_OUT/.stamp"
fi
# libopus_sys はビルドする機械が Unix だと libm をリンクしようとするので、空の m.lib で受ける
[ -f "$OPUS_OUT/lib/m.lib" ] || llvm-ar rcs "$OPUS_OUT/lib/m.lib"
export OPUS_LIB_DIR=$OPUS_OUT OPUS_STATIC=1
cd ~/build/riscord
# pipefail で cargo の失敗をそのまま返す（grep が何も出さなくても失敗にしないよう || true で包む）
cargo xwin build --release --target x86_64-pc-windows-msvc 2>&1 | { grep -v "warning: ring@" || true; } | tail -5
EOF
mkdir -p dist
scp -q "$HOST:build/riscord/target/x86_64-pc-windows-msvc/release/riscord.exe" dist/
ls -lh dist/riscord.exe
