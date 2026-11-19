# Riscord

**Discord を、Rust と GPUI で。**

Riscord は Electron を使わない、非公式の軽量 Discord クライアントです。チャット、通話、画面共有、テーマ、Lua プラグイン、複数アカウントに対応しています。Windows・macOS・Linux 向けのデスクトップ版と、開発中の Android 版があります。表示言語は日本語・English・简体中文から選べます。

[紹介ページ](https://kokorodonguri.github.io/Riscord/) · [ダウンロード](https://github.com/kokorodonguri/Riscord/releases/latest) · [プラグイン開発ガイド](docs/plugins.md) · [Android 版](android/README.md)

> [!WARNING]
> Riscord は Discord の公式アプリではありません。ユーザートークンを使う非公式クライアントの利用は、Discord の利用規約に抵触するおそれがあります。利用前に規約を確認してください。トークンはパスワードと同じように扱い、他人に教えないでください。

## v0.4.0 の主な変更

- **同梱プラグインが約 110 個に**: BetterDiscord の人気プラグインを参考に、Riscord 用に作り直しました。「設定 → プラグイン」に並ぶカードから、使いたいものだけを有効にできます。
- **macOS 版の配布を開始**: Intel（x64）と Apple シリコン（arm64）の両方を配布しています。
- **署名付きの自動更新**: Windows・Linux 版は、リリースの署名を確認できたときだけ自動で更新します。
- 接続の安定性、セキュリティ、ボイスチャンネル、添付ファイル、メンション表示などを改善しました。

すべての変更は [Releases](https://github.com/kokorodonguri/Riscord/releases) を参照してください。

## できること

- **チャット**: サーバー、チャンネル、DM の閲覧と送受信ができます。返信、編集、削除、リアクション、メンション、未読表示に対応しています。Markdown、コードブロック、引用、スポイラーも表示できます。絵文字は、ピッカーや `:名前:` の入力補完で入れられます。
- **探す・整理する**: メッセージ検索（`Ctrl+F`）、クイックスイッチャー（`Ctrl+K`）、ピン留め、スレッド、フォーラム、投票、スラッシュコマンドに対応しています。
- **画像とファイル**: 画像、GIF、スタンプ、埋め込みを表示できます。画像のプレビューと保存、複数ファイルのアップロードとダウンロードもできます。Linux では添付動画を音声付きで再生できます。
- **ボイスと画面共有**: サーバーのボイスチャンネルと DM 通話に参加できます。マイク・出力先の選択、ノイズ抑制、プッシュトゥトーク、サウンドボードにも対応しています。画面共有の配信と視聴は Linux だけの機能です（後述）。
- **サーバー管理**: 権限に応じて、チャンネル、ロール、招待、メンバーなどを管理できます。
- **カスタマイズ**: テーマ、背景画像、フォント、コンパクト表示、Lua プラグインを設定できます。表示言語は初回起動時に OS の言語に合わせ、「設定 → 表示 → 言語」で切り替えられます。
- **複数アカウント**: 複数のアカウントに同時にログインし、分割表示や別ウィンドウで並べて使えます。

使える機能は、Discord 側の権限や環境によって変わります。

## 目次

- [ダウンロード](#ダウンロード)
- [自動更新](#自動更新)
- [ログインと複数アカウント](#ログイン)
- [プラグイン](#プラグイン)
- [テーマ](#テーマ)
- [ソースからビルド](#ソースからビルド)
- [プロジェクト構成](#プロジェクト構成)
- [Android 版](#android-版)

## ダウンロード

[Releases](https://github.com/kokorodonguri/Riscord/releases/latest) から、お使いの OS と CPU に合うファイルをダウンロードしてください。

| OS | x64 | arm64 |
|---|---|---|
| Windows | `riscord-windows-x64.exe` | `riscord-windows-arm64.exe` |
| Linux | `riscord-linux-x64` | `riscord-linux-arm64` |
| macOS | `riscord-macos-x64.zip` | `riscord-macos-arm64.zip` |

`.sig` で終わるファイルは自動更新に使う署名です。手動でダウンロードするときは必要ありません。

### Windows

ダウンロードした `.exe` をそのまま起動します。

### macOS

ZIP を展開して、`Riscord.app` をアプリケーションフォルダに移してから起動してください。Developer ID による署名と公証をしていないため、初回起動時は「システム設定 → プライバシーとセキュリティ」で起動を許可する必要があります。通話を使うときは、マイクへのアクセスも許可してください。

### Linux

実行権限を付けてから起動します。次は x64 版の例です。arm64 版ではファイル名を読み替えてください。

```sh
chmod +x riscord-linux-x64
./riscord-linux-x64
```

実行には ALSA、Vulkan、GStreamer などのライブラリが必要です。パッケージ名は[ソースからビルド](#arch-linux)の手順を参照してください。ファイル選択を使うには、`xdg-desktop-portal` と、デスクトップ環境に合ったポータルのバックエンドも必要です。

画面共有の配信と視聴には、xdg-desktop-portal と、GStreamer の H.264 エンコーダー・デコーダーが必要です。画面の音声は送信しません。

## 自動更新

Windows 版と Linux 版は、起動時と起動中 3 時間ごとに新しいリリースを確認します。新しい版が見つかると裏でダウンロードし、署名を確認してから実行ファイルを差し替えます。差し替えが終わると OS の通知と画面上部の帯で知らせ、次に起動したときから新しい版が使われます。

- 署名を確認できないファイルや、公開鍵を組み込まずにビルドした版では、自動更新はしません。
- 更新の確認を止めるには、環境変数 `RISCORD_NO_UPDATE` を設定してください。
- macOS 版は自動更新に対応していません。新しい ZIP をダウンロードして入れ替えてください。
- 複数アカウントを使っているときは、メインアカウントのウィンドウでだけ更新します。

リリース管理者向けの署名の設定は、[署名の手順](docs/release-signing.md)を参照してください。

## ログイン

ログイン画面では、次の方法を選べます。

- **QR コード**: スマートフォンの Discord アプリで読み取って承認します。Riscord にパスワードを入力せずにログインできます。
- **メールアドレスとパスワード**: 認証アプリによる二段階認証に対応しています。Discord が CAPTCHA を求めた場合は Riscord では表示できないため、QR コードを使ってください。

どの方法でも、Riscord はユーザートークンを受け取ります。ログイン画面で保存を選ぶと、トークンは設定フォルダ内の `riscord/token` に保存されます（Linux の標準では `~/.config/riscord/token`）。

### メインアカウントとサブアカウント

「設定 → マイアカウント → 別のアカウント」で「サブアカウントを追加して横に並べる」を押すと、ウィンドウが左右に分かれ、片側にサブアカウントのログイン画面が出ます。ウィンドウの幅が狭いときは上下に分かれます。

- 追加したアカウントは、同じ画面の「横に並べる」で分割表示に、「別ウィンドウ」で別のウィンドウに開けます。
- 分割は区画の上の帯にあるボタンで閉じられます。閉じてもログインは続きます。
- メインとサブは同時にログインしたまま使え、それぞれ通話にも参加できます。ウィンドウのタイトルには、操作中のアカウント名が出ます。
- サブアカウントのトークン・設定・記録は `riscord/profiles/<名前>/` に保存されます。テーマとプラグインは全アカウントで共通です。
- コマンドラインから `riscord --profile 2` のように起動することもできます。同じアカウントのウィンドウを 2 つ開くことはできません。

## プラグイン

「設定 → プラグイン」で、Lua で書かれた Riscord 用のプラグインを管理できます。同梱プラグインがカード形式で並び、検索、有効・無効の切り替え、設定ページを開くことができます。同梱プラグインは、有効にしたものだけが動きます。

同梱プラグインの例:

| 分類 | 例 |
|---|---|
| 送信・入力 | 長文の自動分割（SplitLargeMessages）、リンクの整形（LinkCleaner）、自動翻訳（AutoTranslate）、タイムスタンプの挿入（InsertTimestamps）、チャットの別名（ChatAliases） |
| メッセージ表示 | 完全なタイムスタンプ（CompleteTimestamps）、バッジやロールの表示（ShowBadgesInChat・TopRoleEverywhere）、タイムゾーン（Timezones）、埋め込みの折りたたみ（CollapseEmbeds） |
| メッセージの操作 | ダブルクリックで返信（DoubleClickToReply）・編集（BetterDoubleClickToEdit）、クイックリアクション（MoreQuickReacts）、個人用ピン（PersonalPins）、リンク先のプレビュー（PeekMessageLinks） |
| サーバー・チャンネル・フレンド | 一括既読（ReadAllNotificationsButton）、チャンネルタブ（ChannelTabs）、サーバーフォルダー（ServerFolders）、DM のピン留め（PinDMs）、メンバー数（MemberCount） |
| その他 | 通話時間（CallTimeCounter）、翻訳（Translator）、削除・編集されたメッセージの記録（MessageLogger） |

全一覧は [`plugins/`](plugins) か設定画面で確認できます。

注意点:

- 翻訳系のプラグインは、翻訳する本文を Google に送ります。
- MessageLogger の記録はこのコンピューターの設定フォルダ（`message-log.json`）に保存され、ログアウト時に削除されます。
- 同梱プラグインは Riscord 用に作り直したものです。BetterDiscord のプラグインをそのまま読み込むことはできません。
- プラグインはメッセージを読んだり送信したりできます。自分で入れるプラグインは、信頼できるものだけを有効にしてください。

自作プラグインの入れ方と API は [プラグイン開発ガイド](docs/plugins.md)、実例は [サンプル](examples/plugins/hello) を参照してください。

## テーマ

設定フォルダ内の `riscord/themes/` に TOML ファイルを置き、「設定 → テーマ」で選びます（Linux の標準では `~/.config/riscord/themes/`）。書いた項目だけが既定のテーマから上書きされます。

```toml
name = "夜空"
author = "me"
# font = "/path/to/font.ttf"

[colors]
bg = "#0b1020"
accent = "#7aa2f7"

[background]
image = "~/Pictures/wall.png" # ローカルのパスか HTTP(S) の URL
opacity = 0.35
```

- すべての色と既定値は [テンプレート](themes/template.toml)、配色の例は [見本](themes/sample.toml) にあります。
- 設定画面の地とメニューの色は `settings_bg` と `settings_nav` で変えられます。省略するとチャット欄の色になります。
- 背景画像は `area = "window"` でウィンドウ全体に表示できます。面の色に透明度（`#rrggbbaa`）を付けると、設定画面からも背景が見えます。

### 部品ごとの上書き

特定の部品だけを変えたいときは、`[parts.画面.部品]` に書きます。書かなかった項目は `[colors]` の色のままです。

```toml
[parts.profile.card]   # DM 右側のプロフィールカード
bg = "#0e0e1080"       # 地を半透明に
radius = 16

[parts.profile.button]
bg = "#ffffff20"
text = "#ffffff"
```

書ける項目は `bg`（地の色）、`text`（文字の色）、`border`（枠線の色）、`border_width`（枠線の太さ）、`radius`（角の丸み）、`opacity`（中身ごとの不透明度）、`hover_bg`（マウスを乗せたときの地の色）です。画面は `window`、`sidebar`、`chat`、`message`、`members`、`menu`、`dialog`、`profile` に分かれています。部品の一覧は [テンプレート](themes/template.toml) の末尾にあります。一覧にない部品や項目は読み飛ばされ、「設定 → テーマ」に警告が出ます。

## ソースからビルド

どの OS でも [Rust（rustup）](https://rust-lang.org/tools/install/) の stable ツールチェーンと CMake が必要です。

### Windows

[Git for Windows](https://git-scm.com/install/windows)、Rust、Visual Studio の C++ ビルドツールをインストールします。Rust のインストール中に C++ ビルドツールの導入を案内されたら、画面の指示に従ってください。インストール後に PowerShell を開き直して、次を実行します。

```powershell
git clone https://github.com/kokorodonguri/Riscord.git
cd Riscord
cargo run --release --locked
```

<details>
<summary><code>libopus_sys</code> のビルドが失敗するとき</summary>

`Could not create named generator Visual Studio 18 2026` と出た場合は、CMake を 4.2 以降に更新してください。新しいターミナルで `cmake --version` と `where.exe cmake` を実行し、古い CMake が PATH で優先されていないことを確かめてから、ビルドキャッシュを消してやり直します。

```powershell
cargo clean -p libopus_sys --release
cargo run --release --locked
```

CMake を更新できない場合は、スタートメニューから「x64 Native Tools Command Prompt for VS」を開き、ジェネレーターを指定してビルドします（Ninja があれば `NMake Makefiles` の代わりに `Ninja` も使えます）。

```bat
set CMAKE_GENERATOR=NMake Makefiles
cargo clean -p libopus_sys --release
cargo run --release --locked
```

</details>

`build-windows.sh` は、特定のリモートホストと `cargo-xwin` を前提にした個人用のスクリプトです。使う場合は環境に合わせて書き換えてください。

### macOS

Xcode Command Line Tools、CMake（Homebrew なら `brew install cmake`）、Rust をインストールして、次を実行します。

```sh
git clone https://github.com/kokorodonguri/Riscord.git
cd Riscord
cargo build --release --locked
scripts/package-macos.sh target/release/riscord dist
open dist/Riscord.app
```

macOS では、検索に `Command+F`、クイックスイッチャーに `Command+K` を使います。画面共有と添付動画の再生には対応していません。

### Arch Linux

```sh
sudo pacman -Syu --needed base-devel git cmake rustup alsa-lib fontconfig freetype2 \
  wayland libxkbcommon libxkbcommon-x11 libx11 libxcb vulkan-headers vulkan-icd-loader \
  gstreamer gst-plugins-base gst-plugins-good gst-libav xdg-desktop-portal
rustup default stable
git clone https://github.com/kokorodonguri/Riscord.git
cd Riscord
cargo run --release --locked
```

### Ubuntu 24.04

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

Rust のインストーラーでは、画面の指示に従って stable ツールチェーンを入れてください。Linux では、Vulkan を使えるグラフィックドライバーも必要です。

### ビルド・テスト・デモ

| やること | コマンド |
|---|---|
| ビルドだけする | `cargo build --release --locked` |
| テストする | `cargo test --locked` |
| 共有コアをテストする | `cargo test -p riscord-core --locked` |

リポジトリは Cargo のワークスペースで、何も指定しなければデスクトップ版（ルートのクレート）が対象になります。

Discord に接続せず、デモデータで画面を確かめるには `RISCORD_DEMO` を設定します。`guild` を指定するとサーバーのチャンネルを、`dm` を指定すると DM を開いた状態で始まります。デモでは、保存済みのトークンや設定を読み書きしません。

```sh
RISCORD_DEMO=1 cargo run --release --locked
```

PowerShell では次のようにします。

```powershell
$env:RISCORD_DEMO = "1"
cargo run --release --locked
Remove-Item Env:RISCORD_DEMO
```

## プロジェクト構成

| パス | 内容 |
|---|---|
| `src/` | GPUI によるデスクトップ版の UI、通信、音声・画面共有 |
| `crates/riscord-core/` | Android 版と共有する Rust のコア |
| `android/` | Jetpack Compose と JNI を使う Android アプリ |
| `plugins/` | 同梱プラグイン（`_lib.lua` は共通の補助関数） |
| `examples/plugins/` | プラグイン開発用のサンプル |
| `themes/` | テーマのテンプレートと見本 |
| `docs/` | プラグイン開発ガイド、署名の手順、紹介ページ |
| `scripts/` | macOS 向けのパッケージ作成など |

## Android 版

Android 10 以上（arm64）向けのアプリを開発しています。画面は Jetpack Compose で描き、ログイン・REST 通信・Gateway はデスクトップ版と共通の Rust コア（`crates/riscord-core`）を JNI 経由で呼び出します。

今は、ログイン（メールアドレスとパスワード、二段階認証、QR コード、トークン）、サーバー・チャンネル・DM の表示、メッセージの送受信・返信・編集・削除・リアクション、「入力中…」の表示と送信、画像と添付、DM・メンションの通知、ボイス通話などに対応しています。デスクトップ版にあるテーマ、Lua プラグイン、複数アカウント、リリースでの配布、アプリ内更新は、Android 版にはまだありません。

開発用（debug 署名）の APK は、GitHub Actions の「Android」ワークフローの実行結果から `riscord-android-arm64-debug` としてダウンロードできます（GitHub へのログインが必要です）。

ビルドには Linux x86_64、JDK 17 または 21、Android SDK 35、NDK 27.2.12479018、Rust の `aarch64-linux-android` ターゲットが必要です。

```sh
rustup target add aarch64-linux-android
cd android
./gradlew :app:assembleDebug
```

必要な環境や実機でのテストについては、[Android 版の README](android/README.md) を参照してください。
