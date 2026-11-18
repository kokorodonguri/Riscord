# 自動更新の署名

Windows と Linux の自動更新は、ビルド時に埋め込んだ Ed25519 公開鍵で署名を検証します。公開鍵がないビルドや署名がないリリースは自動更新できません。更新は手動で行えます。

リリース管理者は、安全なオフライン環境で Ed25519 秘密鍵を作成し、秘密鍵の PEM 全体を GitHub Actions の secret `RISCORD_UPDATE_PRIVATE_KEY` に登録します。対応する生の公開鍵32バイトを小文字16進文字列にして repository variable `RISCORD_UPDATE_PUBLIC_KEY` に登録します。例えば OpenSSL では次のように生成できます。

```sh
umask 077
openssl genpkey -algorithm Ed25519 -out riscord-update-private.pem
openssl pkey -in riscord-update-private.pem -pubout -outform DER | tail -c 32 | xxd -p -c 64
```

秘密鍵はリポジトリに追加せず、安全に保管してください。ワークフローは秘密鍵と公開鍵の一致を確認し、各実行ファイルのバージョン、ファイル名、SHA-256 に署名します。鍵が未設定または不一致なら公開に失敗します。公開鍵を変更する場合、旧鍵を信頼する利用者に新鍵を含むアプリを安全な別経路で配布する必要があります。

このワークフローはビルド成果物を自動で署名します。公開後の Release 差し替えは検出できますが、ビルド環境そのものが侵害されて悪意ある成果物を作った場合は検出できません。その脅威に備えるには、成果物の独立した検証と署名の承認を別の信頼境界で運用してください。
