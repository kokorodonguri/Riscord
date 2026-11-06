#!/bin/sh
# リモートのUbuntu (cargo-xwin入り) でWindows版をビルドして dist/ に持ってくる
set -e
HOST=donguri0423@10.4.23.235
cd "$(dirname "$0")"
tar --exclude=target --exclude=dist -czf - . | ssh $HOST 'rm -rf ~/build/riscord && mkdir -p ~/build/riscord && tar -xzf - -C ~/build/riscord'
ssh $HOST '. ~/.cargo/env && export PATH=$HOME/.local/bin:$PATH && cd ~/build/riscord && cargo xwin build --release --target x86_64-pc-windows-msvc 2>&1 | grep -v "warning: ring@" | tail -3'
mkdir -p dist
scp -q $HOST:build/riscord/target/x86_64-pc-windows-msvc/release/riscord.exe dist/
ls -lh dist/riscord.exe
