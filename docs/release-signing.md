# 自動更新の署名

Windows と Linux の自動更新は、ビルド時に埋め込んだ Ed25519 公開鍵で署名を検証します。公開鍵がないビルドや署名がないリリースは自動更新できません。更新は手動で行えます。

リリース管理者は、安全なオフライン環境で Ed25519 秘密鍵を作成し、秘密鍵の PEM 全体を GitHub Actions の secret `RISCORD_UPDATE_PRIVATE_KEY` に登録します。対応する生の公開鍵32バイトを小文字16進文字列にして repository variable `RISCORD_UPDATE_PUBLIC_KEY` に登録します。例えば OpenSSL では次のように生成できます。

```sh
umask 077
openssl genpkey -algorithm Ed25519 -out riscord-update-private.pem
openssl pkey -in riscord-update-private.pem -pubout -outform DER | tail -c 32 | xxd -p -c 64
```

秘密鍵はリポジトリに追加せず、安全に保管してください。ワークフローは秘密鍵と公開鍵の一致を確認し、各実行ファイルのバージョン、ファイル名、SHA-256 に署名します。鍵が未設定または不一致なら公開に失敗します。なお repository variable `RISCORD_UPDATE_PUBLIC_KEY` が未設定でも、release.yml の build ジョブは失敗しません。`vars.RISCORD_UPDATE_PUBLIC_KEY` は空文字としてビルドに渡り、公開鍵が空のまま埋め込まれた実行ファイルができます（このビルドでは自動更新の検証が必ず失敗します）。止まるのはその後の release ジョブの「Sign update assets」で、`test -n "$RISCORD_UPDATE_PUBLIC_KEY"` に失敗し、Release は作られません。成果物は Actions の artifact として残るだけなので、変数を設定してからタグのワークフローを実行し直してください。

公開鍵を変更する場合、旧鍵を信頼する利用者に新鍵を含むアプリを安全な別経路で配布する必要があります。

このワークフローはビルド成果物を自動で署名します。公開後の Release 差し替えは検出できますが、ビルド環境そのものが侵害されて悪意ある成果物を作った場合は検出できません。その脅威に備えるには、成果物の独立した検証と署名の承認を別の信頼境界で運用してください。

## Android 版の署名

利用者に配る Android 版は、release ビルド（`debuggable=false`）を固定の鍵で署名したものにします。debug ビルドは adb からアプリのデータ（保存したトークン）を読めるうえ、CI のビルドごとに署名の鍵が変わるため上書きインストールもできません。

1. 鍵を作ります（手元で一度だけ。鍵とパスワードはなくさないよう安全な場所に保管します）。

   ```sh
   keytool -genkeypair -v -keystore riscord-release.jks -alias riscord \
     -keyalg RSA -keysize 4096 -validity 10000
   ```

2. リポジトリの Settings → Secrets and variables → Actions に、次の secrets を登録します。

   | 名前 | 中身 |
   | --- | --- |
   | `RISCORD_ANDROID_KEYSTORE_BASE64` | `base64 -w0 riscord-release.jks` の出力 |
   | `RISCORD_ANDROID_KEYSTORE_PASSWORD` | キーストアのパスワード |
   | `RISCORD_ANDROID_KEY_ALIAS` | 鍵の別名（上の例では `riscord`） |
   | `RISCORD_ANDROID_KEY_PASSWORD` | 鍵のパスワード |

3. 登録後の「Android」ワークフローは、debug の APK に加えて `riscord-android-arm64-release`（署名済みの release APK）も作ります。secrets が無いとき（フォークからの PR など）は release の手順を飛ばします。

手元で release を作るときは、同じ名前の環境変数（`RISCORD_ANDROID_KEYSTORE` にはキーストアのパス）を設定して `./gradlew :app:assembleRelease` を実行します。
