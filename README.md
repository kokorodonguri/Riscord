# Riscord

Rust と GPUI で作られた、日本語 UI の非公式 Discord デスクトップクライアントです。Electron を使わず、チャット、ボイス通話、テーマのカスタマイズを利用できます。

**[紹介ページ](https://kokorodonguri.github.io/Riscord/)**

> [!WARNING]
> Riscord は Discord の公式アプリではありません。ユーザートークンを使う非公式クライアントの利用は、Discord の利用規約に抵触する可能性があります。利用前に適用される規約を確認してください。トークンはパスワードと同様に扱い、他人に共有しないでください。

## 主な機能

- サーバー、チャンネル、DM の表示とメッセージの送受信
- 画像、GIF、スタンプ、埋め込みコンテンツの表示とファイルのアップロード
- ボイスチャンネルへの接続、マイクと音声出力の設定
- QR コード、メールアドレスによるログイン
- ステータスとプロフィールの編集、配色・背景画像・フォントのカスタマイズ

## はじめに

以下はソースコードからビルドして起動する手順です。CI では Linux と Windows の x64・arm64 向けビルドを行っています。

### Windows

[Git for Windows](https://git-scm.com/install/windows)、[Rust（rustup）](https://rust-lang.org/tools/install/)、Visual Studio の C++ ビルドツールをインストールしてください。Rust のインストール時に C++ ビルドツールの導入を案内された場合は、画面の指示に従ってください。

インストール後、PowerShell を開き直して実行します。

```powershell
git clone https://github.com/kokorodonguri/Riscord.git
cd Riscord
cargo run --release --locked
```

`build-windows.sh` は特定のリモートホストと `cargo-xwin` を前提とした個人用スクリプトです。使用する場合は環境に合わせて変更してください。

`libopus_sys` のビルドで `Could not create named generator Visual Studio 18 2026` と表示された場合は、CMake を 4.2 以降に更新してください。新しいターミナルで `cmake --version` と `where.exe cmake` を実行し、古い CMake が PATH で優先されていないことを確認してから、失敗時のビルドキャッシュを消して再度実行します。

```powershell
cargo clean -p libopus_sys --release
cargo run --release --locked
```

CMake を更新できない場合は、スタートメニューの「x64 Native Tools Command Prompt for VS」を開き、CMake のジェネレーターを指定してビルドします（Ninja がインストール済みなら `NMake Makefiles` の代わりに `Ninja` も使えます）。

```bat
set CMAKE_GENERATOR=NMake Makefiles
cargo clean -p libopus_sys --release
cargo run --release --locked
```

### Arch Linux

ターミナルで依存パッケージと Rust の stable ツールチェーンを用意し、起動します。

```sh
sudo pacman -Syu --needed base-devel git cmake rustup alsa-lib fontconfig freetype2 \
  wayland libxkbcommon libxkbcommon-x11 libx11 libxcb vulkan-headers vulkan-icd-loader \
  xdg-desktop-portal
rustup default stable
git clone https://github.com/kokorodonguri/Riscord.git
cd Riscord
cargo run --release --locked
```

### Ubuntu

Ubuntu 24.04 では、ターミナルで依存パッケージと Rust の stable ツールチェーンを用意し、起動します。

```sh
sudo apt-get update
sudo apt-get install -y build-essential git curl pkg-config cmake \
  libasound2-dev libfontconfig-dev libfreetype-dev \
  libwayland-dev libxkbcommon-dev libxkbcommon-x11-dev \
  libx11-dev libx11-xcb-dev libxcb1-dev libvulkan-dev xdg-desktop-portal
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
. "$HOME/.cargo/env"
git clone https://github.com/kokorodonguri/Riscord.git
cd Riscord
cargo run --release --locked
```

Rust のインストーラーでは、画面の指示に従って stable ツールチェーンをインストールしてください。

Linux でファイル選択を使うには、`xdg-desktop-portal` に加えてデスクトップ環境に対応したポータルのバックエンドが必要です。Vulkan を利用できるグラフィックドライバーも用意してください。

### ビルド・テスト・デモ

ビルドのみ行う場合は `cargo build --release --locked`、テストを実行する場合は `cargo test --locked` を使います。

Discord に接続せず、デモデータで画面を確認するには、Linux では次のコマンドを使います。

```sh
RISCORD_DEMO=1 cargo run --release --locked
```

Windows の PowerShell では次のように実行します。

```powershell
$env:RISCORD_DEMO = "1"
cargo run --release --locked
Remove-Item Env:RISCORD_DEMO
```

`RISCORD_DEMO` に `guild` を指定すると、サーバーのチャンネルを開いた状態で始まります。デモでは通常の保存済みトークンや設定を読み書きしません。

### ログイン

ログイン画面から、次のいずれかの方法を選べます。

- **QR コード**: スマートフォンの Discord アプリで読み取って承認します。パスワードを Riscord に入力せずにログインできます。
- **メールアドレスとパスワード**: 認証アプリによる二段階認証に対応しています。Discord が CAPTCHA を要求した場合は Riscord では表示できないため、QR コードを使ってください。

どの方法でも Riscord はユーザートークンを受け取ります。ログイン画面で保存を選択した場合、トークンは OS の設定ディレクトリ内の `riscord/token` に保存されます（Linux の標準設定では `~/.config/riscord/token`）。

## テーマ

設定ディレクトリ内の `riscord/themes/` に TOML ファイルを置き、設定画面で選択します。Linux の標準設定では `~/.config/riscord/themes/` です。指定した項目だけが既定のテーマから上書きされます。

```toml
name = "夜空"
author = "me"
# font = "/path/to/font.ttf"

[colors]
bg = "#0b1020"
accent = "#7aa2f7"

[background]
image = "~/Pictures/wall.png" # ローカルパスまたは HTTP(S) URL
opacity = 0.35
```

すべての色と既定値を記載した [テンプレート](themes/template.toml) と [配色の見本](themes/sample.toml) を用意しています。設定画面の地とメニューは `settings_bg` と `settings_nav` で個別に変更できます。省略するとチャット欄の色を引き継ぎます。背景画像は `area = "window"` でウィンドウ全体に表示できます。面の色に透明度（`#rrggbbaa`）を指定すると設定画面にも背景が見えます。
