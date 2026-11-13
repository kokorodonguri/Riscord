# Riscord

**Discord を、Rust と GPUI で。**

Riscord は、Electron を使わずに動作する非公式 Discord クライアントです。表示は日本語と英語に対応しています。チャットや通話をはじめ、画面共有、テーマ、Lua プラグイン、複数アカウントに対応しています。Windows・Linux 向けのデスクトップ版と、開発中の Android 版があります。

[紹介ページ](https://kokorodonguri.github.io/Riscord/) · [ダウンロード](https://github.com/kokorodonguri/Riscord/releases/latest) · [Android 版](android/README.md) · [プラグイン開発ガイド](docs/plugins.md)

> [!WARNING]
> Riscord は Discord の公式アプリではありません。ユーザートークンを使う非公式クライアントの利用は、Discord の利用規約に抵触する可能性があります。利用前に適用される規約を確認してください。トークンはパスワードと同様に扱い、他人に共有しないでください。

## できること

- **チャット** — サーバー、チャンネル、DM の閲覧と送受信。返信、編集、削除、リアクション、メンション、未読表示に対応し、Markdown やコードブロック、引用、スポイラーを表示できます。絵文字ピッカーと `:名前:` の入力補完も使えます。
- **会話を探す・整理する** — メッセージ検索、ピン留め、スレッド、フォーラム、投票、スラッシュコマンドを利用できます。検索は `Ctrl+F` で開けます。
- **画像とファイル** — 画像、GIF、スタンプ、埋め込みの表示、画像のプレビュー・保存、複数ファイルのアップロードとダウンロードに対応しています。Linux では添付動画を音声付きで再生できます。
- **ボイスと画面共有** — サーバーのボイスチャンネルや DM 通話、マイク・出力先の設定、ノイズ抑制、プッシュトゥトーク、サウンドボードに対応しています。画面共有の配信・視聴は Linux のみで、xdg-desktop-portal と GStreamer の H.264 エンコーダー・デコーダーが必要です。画面の音声は送信しません。
- **サーバー管理** — 権限に応じたチャンネル、ロール、招待、メンバーなどの管理画面があります。
- **自分らしく使う** — 表示言語（日本語・English）、テーマ、背景画像、フォント、コンパクト表示、Lua プラグインを設定できます。表示言語は「設定 → 表示 → 言語」で切り替えます。初めて起動したときは OS の言語に合わせます。複数アカウントは分割表示または別ウィンドウで同時に使えます。

機能の利用可否は、Discord 側の権限や環境によって異なります。Android 版の対応状況は[こちら](#android-版)を参照してください。

## 目次

- [ダウンロード](#ダウンロード)
- [ソースからビルド](#ソースからビルド)
- [ログインと複数アカウント](#ログイン)
- [テーマ](#テーマ)・[プラグイン](#プラグイン)
- [プロジェクト構成](#プロジェクト構成)
- [Android 版](#android-版)

## ダウンロード

[Releases](https://github.com/kokorodonguri/Riscord/releases/latest) から、お使いの OS と CPU に合うデスクトップ版をダウンロードしてください。

| OS | x64 | arm64 |
|---|---|---|
| Windows | `riscord-windows-x64.exe` | `riscord-windows-arm64.exe` |
| Linux | `riscord-linux-x64` | `riscord-linux-arm64` |

Android 版はまだリリースで配布していません。開発用（debug 署名）の APK は、GitHub Actions の「Android」ワークフローの実行結果から `riscord-android-arm64-debug` としてダウンロードできます（GitHub へのログインが必要です）。詳しくは下の「Android 版」を参照してください。

Linux ではダウンロード後に実行権限を付けて起動します。次は x64 版の例です。arm64 ではファイル名を読み替えてください。

```sh
chmod +x riscord-linux-x64
./riscord-linux-x64
```

Linux 版の実行には、下の「ソースからビルド」に挙げた ALSA、Vulkan、GStreamer などのライブラリが必要です。

配布版は起動時と、起動中は3時間おきに新しいリリースを確認し、裏でダウンロードして実行ファイルを差し替えます。差し替えが終わると OS の通知と画面上部の帯で知らせ、新しい版は次に起動したときから使われます。確認を止めるには、環境変数 `RISCORD_NO_UPDATE` を設定してください。

## ソースからビルド

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
  gstreamer gst-plugins-base gst-plugins-good gst-libav xdg-desktop-portal
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
  libx11-dev libx11-xcb-dev libxcb1-dev libvulkan-dev xdg-desktop-portal \
  libgstreamer1.0-dev libgstreamer-plugins-base1.0-dev \
  gstreamer1.0-plugins-base gstreamer1.0-plugins-good gstreamer1.0-libav
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
. "$HOME/.cargo/env"
git clone https://github.com/kokorodonguri/Riscord.git
cd Riscord
cargo run --release --locked
```

Rust のインストーラーでは、画面の指示に従って stable ツールチェーンをインストールしてください。

Linux でファイル選択を使うには、`xdg-desktop-portal` に加えてデスクトップ環境に対応したポータルのバックエンドが必要です。Vulkan を利用できるグラフィックドライバーも用意してください。

### ビルド・テスト・デモ

ビルドのみ行う場合は `cargo build --release --locked`、テストを実行する場合は `cargo test --locked` を使います。リポジトリは Cargo のワークスペースで、何も指定しなければデスクトップ版（ルートのクレート）が対象です。Android 版と共有する Rust のコアは `crates/riscord-core` にあり、`cargo test -p riscord-core --locked` でテストできます。

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

`RISCORD_DEMO` に `guild` を指定するとサーバーのチャンネルを、`dm` を指定すると DM を開いた状態で始まります。デモでは通常の保存済みトークンや設定を読み書きしません。

## ログイン

ログイン画面から、次のいずれかの方法を選べます。

- **QR コード**: スマートフォンの Discord アプリで読み取って承認します。パスワードを Riscord に入力せずにログインできます。
- **メールアドレスとパスワード**: 認証アプリによる二段階認証に対応しています。Discord が CAPTCHA を要求した場合は Riscord では表示できないため、QR コードを使ってください。

どの方法でも Riscord はユーザートークンを受け取ります。ログイン画面で保存を選択した場合、トークンは OS の設定ディレクトリ内の `riscord/token` に保存されます（Linux の標準設定では `~/.config/riscord/token`）。

### メインアカウントとサブアカウント

設定の「マイアカウント」→「別のアカウント」で「サブアカウントを追加して横に並べる」を押すと、ウィンドウが左右に分かれ、片側にサブアカウントのログイン画面が表示されます。ウィンドウの幅が狭いときは上下に分かれます。追加済みのアカウントは、同じ画面の「横に並べる」で分割表示に、「別ウィンドウ」で別のウィンドウに開けます。分割は区画の上にある帯のボタンで閉じられ、閉じてもログインしたままです。

メインアカウントとサブアカウントは同時にログインしたまま使え、通話にもそれぞれ参加できます。ウィンドウのタイトルには、操作中のアカウント名が表示されます。

サブアカウントのトークン・設定・記録は `riscord/profiles/<名前>/` に保存されます。テーマとプラグインは全アカウントで共有されます。コマンドラインから `riscord --profile 2` のように起動することもできます。同じアカウントのウィンドウを 2 つ開くことはできません。アプリの自動アップデートはメインアカウントのウィンドウでだけ行われます。

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

### 部品ごとの上書き

`[colors]` は画面全体の配色です。特定の部品だけ変えたいときは `[parts.画面.部品]` に書きます。書いた項目だけが変わり、書かない項目は `[colors]` の色のままです。

```toml
[parts.profile.card]   # DM 右側のプロフィールカード
bg = "#0e0e1080"       # 地を半透明に
radius = 16

[parts.profile.button]
bg = "#ffffff20"
text = "#ffffff"
```

部品には `bg`（地の色）、`text`（文字の色）、`border`（枠線の色）、`border_width`（枠線の太さ）、`radius`（角の丸み）、`opacity`（中身ごとの見え具合）、`hover_bg`（マウスを乗せたときの地の色）を書けます。部品は `window`、`sidebar`、`chat`、`message`、`members`、`menu`、`dialog`、`profile` に分かれていて、一覧は [テンプレート](themes/template.toml) の末尾にあります。一覧にない部品や項目を書くと、その項目だけ読み飛ばして「設定 → テーマ」に警告を表示します。

## プラグイン

Lua で書いた Riscord 専用プラグインを設定画面から管理できます。次のプラグインを同梱しており、「設定 → プラグイン」で有効にできます。

- **CallTimeCounter**: 通話の経過時間を表示します。
- **Translator**: Google 翻訳でメッセージや送信文を翻訳します。翻訳する本文は Google に送られます。
- **MessageLogger**: 削除されたメッセージを赤く残し、編集前の本文をメッセージの下に表示します。記録はこのコンピューターの設定フォルダ（`message-log.json`）にだけ保存されます。自分や Bot のメッセージを記録しない設定もあります。

インストール方法と API は [プラグイン開発ガイド](docs/plugins.md)、実例は [サンプル](examples/plugins/hello) を参照してください。プラグインはメッセージを読んだり送信したりできるため、信頼できるものだけを有効にしてください。

## プロジェクト構成

| パス | 内容 |
|---|---|
| `src/` | GPUI によるデスクトップ UI、通信、音声・画面共有 |
| `crates/riscord-core/` | Android 版と共有する Rust コア |
| `android/` | Jetpack Compose と JNI を使う Android アプリ |
| `themes/` | テーマのテンプレートとサンプル |
| `plugins/`・`examples/plugins/` | 同梱プラグインと開発用サンプル |
| `docs/` | プラグインなどの開発ガイド |

## Android 版

Android 10 以上（arm64）向けのアプリを開発中です。画面は Jetpack Compose で描き、ログイン・REST 通信・Gateway はデスクトップ版と共通の Rust コア（`crates/riscord-core`）を JNI 経由で呼び出します。

現在は、ログイン（メールアドレスとパスワード、二段階認証、QR コード、トークン）、サーバー・チャンネル・DM の表示、メッセージの送受信・返信・編集・削除・リアクション、相手の「入力中…」表示と自分の入力通知、画像と添付、DM・メンションの通知、ボイス通話などに対応しています。テーマ、Lua プラグイン、複数アカウント、署名付きリリースとアプリ内更新はまだありません。

ビルドには Linux x86_64、JDK 17 または 21、Android SDK 35、NDK 27.2.12479018、Rust の `aarch64-linux-android` ターゲットが必要です。

```sh
rustup target add aarch64-linux-android
cd android
./gradlew :app:assembleDebug
```

必要な環境と実機テストの詳細は [Android 版の README](android/README.md) を参照してください。
