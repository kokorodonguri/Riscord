# Riscord

**A lightweight, unofficial Discord desktop client written in Rust (egui), with a Japanese UI.** Native (no Electron), with chat, voice, QR code login and themes. Runs on Linux and Windows.

紹介ページ: https://kokorodonguri.github.io/Riscord/

Riscord は、Rust と egui で作られた非公式の Discord デスクトップクライアントです。日本語のインターフェースで、チャットやボイス通話、テーマによる外観のカスタマイズを利用できます。

> Discord の公式アプリではありません。ユーザートークンを使う非公式クライアントの利用は、Discord の利用規約に抵触する可能性があります。利用前に適用される規約を確認してください。トークンはパスワードと同様に扱い、他人に共有しないでください。

## 主な機能

- サーバー、チャンネル、DM の表示とメッセージの送受信
- 画像や GIF、スタンプ、埋め込みコンテンツの表示とファイルのアップロード
- ボイスチャンネルへの接続、マイクと音声出力の設定
- ステータスの変更と、配色・背景画像・フォントのカスタマイズ
- QR コード・メールアドレス・トークンでのログイン、プロフィールの編集

## ビルドと実行

Rust と Cargo、および使用するプラットフォームのネイティブなビルド環境が必要です。Linux では音声用の ALSA 開発ライブラリなどが必要です。

```sh
cargo run --release
```

ビルドのみ行う場合は `cargo build --release`、テストを実行する場合は `cargo test` を使います。Linux のファイル選択には xdg-desktop-portal を使用します。

ログイン画面では次のいずれかでログインします。

- **QR コード**（おすすめ）: スマホの Discord アプリで読み取り、承認します。パスワードを Riscord に入力しません。
- **メールアドレスとパスワード**: 認証アプリによる二段階認証に対応しています。Discord が CAPTCHA を求めた場合は Riscord では表示できないため、QR コードを使ってください。
- **トークン**: 「トークンでログイン」から入力します。

どの方法でも、Riscord が受け取るのは同じユーザートークンです。環境変数 `DISCORD_TOKEN` でも指定でき、保存済みトークンより優先されます。ログイン画面で保存を選択した場合、トークンは OS の設定ディレクトリ内の `riscord/token` に保存されます（Linux の標準設定では `~/.config/riscord/token`）。

## テーマ

設定ディレクトリ内の `riscord/themes/` に TOML ファイルを置いてテーマを追加できます。Linux の標準設定では `~/.config/riscord/themes/` です。配色は指定した項目だけ上書きされます。

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

テーマの設定項目と既定色は `src/theme.rs` を参照してください。

## Windows ビルド

Windows 向けのビルドには対応する Rust ツールチェーンとビルド環境が必要です。`build-windows.sh` は特定のリモートホストと `cargo-xwin` を前提とした個人用スクリプトのため、使用する場合は環境に合わせて変更してください。
