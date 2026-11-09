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

### 必要な環境

- Rust と Cargo（CI では stable を使用）
- Linux または Windows のネイティブビルド環境
- Linux では ALSA などの開発ライブラリと、ファイル選択用の xdg-desktop-portal

Ubuntu 24.04 では、CI と同じビルド依存パッケージを次のようにインストールできます。

```sh
sudo apt-get update
sudo apt-get install -y --no-install-recommends \
  pkg-config cmake libasound2-dev libfontconfig-dev libfreetype-dev \
  libwayland-dev libxkbcommon-dev libxkbcommon-x11-dev \
  libx11-dev libx11-xcb-dev libxcb1-dev libvulkan-dev
```

### ビルドと起動

```sh
git clone https://github.com/kokorodonguri/Riscord.git
cd Riscord
cargo run --release --locked
```

ビルドのみ行う場合は `cargo build --release --locked`、テストを実行する場合は `cargo test --locked` を使います。CI では Linux と Windows の x64・arm64 向けビルドを行っています。

Discord に接続せず、デモデータで画面を確認することもできます。

```sh
RISCORD_DEMO=1 cargo run --release --locked
```

`RISCORD_DEMO=guild` を指定すると、サーバーのチャンネルを開いた状態で始まります。デモでは通常の保存済みトークンや設定を読み書きしません。

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

すべての色と既定値を記載した [テンプレート](themes/template.toml) と [配色の見本](themes/sample.toml) を用意しています。背景画像は `area = "window"` でウィンドウ全体に表示できます。面の色に透明度（`#rrggbbaa`）を指定すると背景が見えます。

## Windows ビルド

Windows では対応する Rust ツールチェーンとビルド環境を用意し、上記の Cargo コマンドを実行してください。`build-windows.sh` は特定のリモートホストと `cargo-xwin` を前提とした個人用スクリプトです。使用する場合は環境に合わせて変更してください。
