#!/bin/sh
# リモートのUbuntu (cargo-xwin入り) でWindows版をビルドして dist/ に持ってくる
# リモートに必要なもの: rustup + cargo-xwin、~/.local/llvm18（clang-cl・llvm-rc）、cmake、ninja
set -e
HOST=donguri0423@10.4.23.235
cd "$(dirname "$0")"
tar --exclude=target --exclude=dist --exclude=.serena -czf - . | ssh $HOST 'rm -rf ~/build/riscord && mkdir -p ~/build/riscord && tar -xzf - -C ~/build/riscord'
ssh $HOST 'sh -s' <<'EOF'
set -e
. ~/.cargo/env
export PATH=$HOME/.local/bin:$HOME/.local/llvm18/bin:$PATH
# Opus の CMake は clang-cl に SSE4.1/AVX2 用のフラグを付けないので、その部分を外して別にビルドしておく
OPUS_OUT=$HOME/build/opus-win
if [ ! -f "$OPUS_OUT/lib/opus.lib" ]; then
  SRC=$(ls -d ~/.cargo/registry/src/*/libopus_sys-0.3.3/opus | head -1)
  X=$HOME/.cache/cargo-xwin/xwin
  FL="-nologo -MD -Brepro --target=x86_64-pc-windows-msvc -W0 -Wno-unused-command-line-argument -fuse-ld=lld-link /imsvc $X/crt/include /imsvc $X/sdk/include/ucrt /imsvc $X/sdk/include/um /imsvc $X/sdk/include/shared"
  PATH=$HOME/.cache/cargo-xwin:$PATH cmake -S "$SRC" -B "$OPUS_OUT/build" -G Ninja \
    -DCMAKE_TOOLCHAIN_FILE=$HOME/.cache/cargo-xwin/cmake/clang-cl/x86_64-pc-windows-msvc-toolchain.cmake \
    -DCMAKE_BUILD_TYPE=Release -DCMAKE_INSTALL_PREFIX="$OPUS_OUT" -DCMAKE_INSTALL_LIBDIR=lib \
    -DBUILD_SHARED_LIBS=OFF -DOPUS_BUILD_SHARED_LIBRARY=OFF -DOPUS_BUILD_TESTING=OFF -DOPUS_BUILD_PROGRAMS=OFF \
    -DOPUS_INSTALL_PKG_CONFIG_MODULE=OFF -DOPUS_X86_MAY_HAVE_SSE4_1=OFF -DOPUS_X86_MAY_HAVE_AVX2=OFF \
    "-DCMAKE_C_FLAGS=$FL" "-DCMAKE_C_FLAGS_RELEASE=$FL /O2" >/dev/null
  PATH=$HOME/.cache/cargo-xwin:$PATH cmake --build "$OPUS_OUT/build" >/dev/null
  cmake --install "$OPUS_OUT/build" >/dev/null
fi
# libopus_sys はビルドする機械が Unix だと libm をリンクしようとするので、空の m.lib で受ける
[ -f "$OPUS_OUT/lib/m.lib" ] || llvm-ar rcs "$OPUS_OUT/lib/m.lib"
export OPUS_LIB_DIR=$OPUS_OUT OPUS_STATIC=1
cd ~/build/riscord
cargo xwin build --release --target x86_64-pc-windows-msvc 2>&1 | grep -v "warning: ring@" | tail -5
EOF
mkdir -p dist
scp -q $HOST:build/riscord/target/x86_64-pc-windows-msvc/release/riscord.exe dist/
ls -lh dist/riscord.exe
