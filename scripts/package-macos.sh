#!/bin/bash
# ビルド済みバイナリからマイクの使用目的を含む macOS アプリを作る。
set -euo pipefail

if [ "$#" -ne 2 ]; then
  echo "Usage: $0 <riscord binary> <output directory>" >&2
  exit 2
fi

binary=$1
output=$2
root=$(cd "$(dirname "$0")/.." && pwd)
version=$(sed -n 's/^version = "\([^"]*\)"/\1/p' "$root/Cargo.toml" | head -1)
test -f "$binary"
test -n "$version"
mkdir -p "$output"
app="$output/Riscord.app"
rm -rf "$app"
mkdir -p "$app/Contents/MacOS"
cp "$binary" "$app/Contents/MacOS/riscord"
chmod 755 "$app/Contents/MacOS/riscord"
sed "s/@VERSION@/$version/g" "$root/scripts/macos-Info.plist" > "$app/Contents/Info.plist"
codesign --force --sign - "$app"
