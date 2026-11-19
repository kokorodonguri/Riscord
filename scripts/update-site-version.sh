#!/usr/bin/env bash
# 紹介ページ（docs/）の版番号を Cargo.toml の version に合わせる。
#
#   scripts/update-site-version.sh          docs/index.html の版番号（バッジ・JSON-LD・変更点の見出し・
#                                           リリースノートのリンク）と sitemap の lastmod を書き換える
#   scripts/update-site-version.sh --check  docs/index.html の版番号がすべて Cargo.toml と一致するか確かめる
#                                           （release.yml で実行し、食い違っていればリリースを止める）
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
page="$root/docs/index.html"
sitemap="$root/docs/sitemap.xml"
version="$(sed -n 's/^version = "\([^"]*\)"/\1/p' "$root/Cargo.toml" | head -n 1)"

if [[ -z "$version" ]]; then
  echo "Cargo.toml から version を読めませんでした" >&2
  exit 1
fi

if [[ "${1:-}" == "--check" ]]; then
  found="$(grep -oE '"softwareVersion": "[0-9.]+"|最新版 v[0-9.]+|v[0-9.]+ の主な変更|releases/tag/v[0-9.]+' "$page" \
    | grep -oE '[0-9]+\.[0-9]+\.[0-9]+' | sort -u)"
  if [[ "$found" != "$version" ]]; then
    echo "docs/index.html の版番号（$(echo "$found" | tr '\n' ' ')）が Cargo.toml の $version と一致しません" >&2
    exit 1
  fi
  echo "docs/index.html の版番号は $version で一致しています"
  exit 0
fi

sed -i -E \
  -e "s/(\"softwareVersion\": \")[0-9.]+(\")/\1$version\2/" \
  -e "s/(最新版 v)[0-9.]+/\1$version/" \
  -e "s/v[0-9.]+( の主な変更)/v$version\1/" \
  -e "s#(releases/tag/v)[0-9.]+#\1$version#" \
  "$page"
sed -i -E "s#<lastmod>[0-9-]+</lastmod>#<lastmod>$(date +%F)</lastmod>#" "$sitemap"

echo "docs/index.html の版番号を $version に、sitemap の lastmod を $(date +%F) にしました"
echo "「主な変更」の箇条書きは、新しい版の内容に合わせて書き直してください"
"$0" --check
