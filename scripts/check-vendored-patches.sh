#!/usr/bin/env bash
# Cargo.toml の [patch.crates-io] に書いた vendor/ の crate が、本当にビルドで使われているか確かめる。
# 版がずれるとパッチは黙って無視され、crates.io の元の版（lua-src なら制限なしの文字列処理）が入ってしまう。
#
#   - Cargo.lock に [[patch.unused]] がない
#   - パッチした crate は Cargo.lock に 1 つだけあり、source 行がない（＝パス依存）
#   - lua-src は cargo tree でも vendor/lua-src を指している
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$root"

names="$(awk '
  /^\[/ { inpatch = ($0 == "[patch.crates-io]"); next }
  inpatch && /^[A-Za-z0-9_-]+[[:space:]]*=.*path[[:space:]]*=/ { sub(/[[:space:]]*=.*/, ""); print }
' Cargo.toml)"

if [[ -z "$names" ]]; then
  echo "Cargo.toml の [patch.crates-io] からパス指定の crate を読めませんでした" >&2
  exit 1
fi

fail=0
if grep -q '^\[\[patch.unused\]\]' Cargo.lock; then
  echo "Cargo.lock に使われていないパッチ（[[patch.unused]]）があります" >&2
  fail=1
fi

for name in $names; do
  # Cargo.lock の [[package]] ごとに、名前が一致するものの source を数える
  read -r total registry < <(awk -v n="$name" '
    /^\[\[/ { if (hit) { total++; if (src) reg++ } hit = 0; src = 0; next }
    $0 == "name = \"" n "\"" { hit = 1 }
    /^source = / { src = 1 }
    END { if (hit) { total++; if (src) reg++ } print total + 0, reg + 0 }
  ' Cargo.lock)
  if [[ "$total" -ne 1 || "$registry" -ne 0 ]]; then
    echo "$name: Cargo.lock に $total 件（うち crates.io などから $registry 件）あります。vendor/ の版だけが使われるはずです" >&2
    fail=1
  else
    echo "$name: vendor/ の版を使っています"
  fi
done

lua_tree="$(cargo tree --locked -i lua-src --target all -e normal,build)"
if [[ "${lua_tree%%$'\n'*}" != *"$root/vendor/lua-src"* ]]; then
  echo "cargo tree で lua-src が vendor/lua-src を指していません" >&2
  fail=1
fi

exit "$fail"
