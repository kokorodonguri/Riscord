# Riscord Android

Android 10以上、arm64向け。Composeで画面を描画し、RustコアをJNI経由で呼び出します。
ログインの暗号処理・REST通信・GatewayはRust側、画面・Keystore・通知・ファイル選択はAndroid側が担当します。

## ビルド

Linux x86_64、JDK 17または21、Android SDK 35、Build Tools 35、CMake・Ninja、NDK 27.2.12479018、Rust stableを用意します。

```sh
export ANDROID_HOME="$HOME/Android/Sdk"
sdkmanager 'platforms;android-35' 'build-tools;35.0.0' 'ndk;27.2.12479018'
rustup target add aarch64-linux-android
cd android
./gradlew :app:assembleDebug
adb install -r app/build/outputs/apk/debug/app-debug.apk
```

GradleのpreBuildがRustライブラリをビルドしてAPKへ同梱します。
Android NDKのLinuxホスト用ツールはx86_64版なので、ARM Linux上ではx86_64ビルドマシンを使ってください。
既にライブラリを`app/src/main/jniLibs/arm64-v8a/libriscord_core.so`へ配置した場合のみ、
`-Priscord.usePrebuiltNative`でRustビルドを省略できます。

## 実機テスト

```sh
./gradlew connectedDebugAndroidTest
```

JNIのセッション終了・日本語・絵文字の受け渡し、音声デバイス列挙、RustからDiscordへのTLS通信・無効な認証の拒否、QR生成を検証します。
送受信の実アカウントテストは、アプリへログインして専用のテスト用チャンネルで行います。
テストコードにトークンやパスワードを埋め込まないでください。

## 現在の実装

- メール・パスワード、TOTP、QRコード、既存トークンによるログイン
- Keystoreによるトークンの暗号化保存とログアウト
- サーバー・チャンネル・DM一覧、ユーザーIDからのDM開始
- 履歴・リアルタイム受信・送信・返信・編集・削除・リアクション
- 画像・埋め込みの表示、ファイル添付（10MBまで）
- Gatewayのheartbeat、resume、バックオフ再接続
- 任意の常駐モード、DM・メンションの通知
- VC・DM通話、ミュート・消音・音声経路切り替え（実通話は要検証）
- プロフィール・ステータス編集

QRコードは別端末から読み取ります。CAPTCHAが要求された場合はQRログインを利用してください。
常駐モードは設定メニューから明示的に有効化します。OSによる強制終了後の通知は保証しません。

テーマ・Luaプラグイン、複数アカウント、署名付きリリースとアプリ内更新は未実装です。
CIが作るAPKは開発用のdebug署名です。

## スマートフォンのUI検証

公式Android版のDM・サーバー一覧・チャットを接続端末で確認し、72dpのサーバー一覧、
アバター付きの会話一覧、検索、下部のアカウント欄に合わせて構成を整理しています。
メッセージ操作は長押しのシートから行い、返信・編集・添付確認は入力欄へ集約しています。

`MobileUxTest` は保存済みアカウントを復元しない独立モデルと読み取り専用のテスト通信を使います。
実際のDiscordにはメッセージを送信せず、次の動作を端末で検証します。

- 返信・編集のキャンセルと、会話を開き直した後の下書き保持
- 古い履歴の追加、新着受信、画面の戻る操作で閲覧位置が飛ばないこと
- ファイルを選んだ段階では送信せず、添付を取り消せること
- カテゴリの折りたたみ、検索、通話へ参加する前の確認
- 複数画像を横並びに表示すること
- 幅320dp・文字倍率1.4で、キーボード表示中もヘッダーと送信操作が使えること

テスト時の画面は端末のアプリ専用外部領域 `files/ui-validation/` に保存します。
