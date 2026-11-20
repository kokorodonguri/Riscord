# Lua プラグイン開発ガイド

Riscord プラグイン API 1 は Lua 5.4 を使います。`examples/plugins/hello` を設定フォルダの `riscord/plugins/hello` にコピーし、設定の「プラグイン」で有効にしてください。Linux の通常の場所は `~/.config/riscord/plugins/` です。プラグインを変更したら「読み込み直す」を押します。エラーは同じ画面に表示されます。デモモードでは独立した一時設定フォルダを使用します。

各フォルダには `plugin.toml` と Lua のエントリファイルを置きます。マニフェストの必須項目は `id`、`name`、`version`、`api = 1`、`entry` です。`description` は任意です。ID は英小文字、数字、`-`、`_` のみで、他のプラグインと重複できません。エントリファイルはプラグインフォルダの中に置いてください。既存の BetterDiscord プラグインは動作しません。

エントリファイルは以下の任意の項目を持つテーブルを返します。コールバックでエラーが発生すると、そのプラグインは停止し、管理画面にエラーが表示されます。エラーで停止したときは `stop()` は呼ばれません（積まれていた操作も捨てられます）。

- `start()` / `stop()`: 有効化時 / 無効化・再読み込み時。`stop()` の中で積んだ操作（状態を戻すなど）も実行されます（終了時を除く）。エラーで停止したときは `stop()` は呼ばれないので、状態を戻す処理に頼りすぎないでください。
- `voice_panel(call)`: ボイス接続中、左下の接続欄に表示する文字列か `nil` を返します。`call` には `channel_id`、`guild_id`（DM は `nil`）、接続確立からの整数秒 `elapsed_seconds` が入ります。表示更新時に呼ばれるため、副作用を起こさないでください。
- `on_message(message)`: 新着メッセージ。`message` は `id`, `channel_id`, `guild_id`, `author_id`, `author_name`, `author_username`, `bot`, `content`, `timestamp`, `mention_everyone`, `mentions`（ユーザー ID の配列）, `attachments`（URL の配列）, `reply_to` を持つテーブルです。履歴の読み込みでは呼ばれません。
- `before_send(content, channel_id)`: 送信を止める理由の文字列を返せます（300 文字を超える分は切られます）。`nil` は送信を続行します。手入力の送信と、プラグインの `riscord.send_message` の両方に適用されます。
- `on_send(content, channel_id)` / `on_display(content, channel_id)`: 文字列を返すと本文を変更し、`nil` を返すと変更しません。`on_display` が返した本文は 8000 文字までで、超える分は切られます。表示加工は表示時に繰り返し呼ばれるため、副作用を起こさないでください。送信加工は手入力の新規メッセージと、プラグインの `riscord.send_message` に適用されます。
- `on_typing(channel_id, user_id, name)`: ユーザーの入力開始。
- `on_presence(user_id, previous_status, status, activity, name)`: プレゼンスの変化。
- `on_voice(user_id, previous_channel_id, channel_id, guild_id, muted, deafened, name)`: ボイス状態の変化。存在しない ID や名前は空文字です。
- `on_delete(message)`: メッセージが削除されたとき。`message` は削除前の内容で、`on_message` と同じ項目を持ちます。本文が分かっているメッセージ（受信済み・読み込み済み）だけが対象です。
- `on_edit(message, new_content)`: 本文が編集されたとき。`message` は編集前の内容、`new_content` は編集後の本文です。
- `commands = { 名前 = function(args, channel_id) ... end }`: チャット欄に `/名前 引数` と入力すると実行します。同名のコマンドが複数ある場合は、同梱プラグインが優先され、その次にプラグインフォルダ内のフォルダ名の順で先のものが使われます。
- `message_actions = { { label = "説明", icon = "link", run = function(message) ... end } }`: メッセージにマウスを重ねたときのツールバーに操作を追加します。`icon` は lucide のアイコン名（`link` `hash` `at-sign` `quote` `pin` `tag` `clock` `palette` など。一覧は `src/ui/kit.rs` の `plugin_icon`）で、省略するとパズルのピースになります。`label` は 60 文字、`icon` は 256 バイトまでです。メッセージの操作は全プラグイン合計で 40 個、1 プラグインあたり 20 個までです。
- `page()`: 専用ページに表示する部品の配列を返します。部品は 100 個までで、超えた分は表示されません（その旨が末尾に出ます）。大事なボタンは先に置き、長い一覧はページに分けてください（同梱の to-do-list を参照）。`{kind="text", label="表示"}`、`{kind="input", label="見出し", action="キー"}`、`{kind="button", label="ボタン", action="操作名"}`、`{kind="switch", label="スイッチ", action="操作名", enabled=true}` が使えます。`label` は 2000 文字、`kind` と `action` は 256 バイトまでで、超える分は切られます。スイッチ操作も `on_action` に通知されるので、状態を更新して次の `page()` で返してください。
- `on_action(action, values)`: ページのボタン操作を受け取ります。`values` は入力部品の `action` をキーにした文字列テーブルです。

ホスト API:

- `riscord.api_version`: API バージョン（1）。
- `riscord.language()`: Riscord の表示言語（`"ja"`・`"en"`・`"zh"` のどれか。`"zh"` は簡体字中国語）。設定で切り替えられるので、ページや表示を作るたびに呼んでください。
- `riscord.send_message(channel_id, content)`: メッセージを送信待ちに追加します。手入力と同じく `before_send` と `on_send` を通ります。自動送信（受信やタイマーなどから送るもの）は全プラグイン合計で 1 分間に 5 件までで、超えるとそのプラグインはエラーで停止します。コマンド・メッセージの操作・ページの操作の中で送ったものは、1 回の操作につき 3 件まで数えません。信頼できるプラグインのみ有効にしてください。送信結果の通知は現在ありません。
- `riscord.notify(title, body)` / `riscord.show_info(title, body)`: 通知または情報画面を表示します。2 つ合わせて全プラグイン合計で 1 分間に 20 回までで、超えた分は黙って捨てられます。デスクトップ通知は一度に 3 件までしか出ず、見出しは 100 文字・本文は 400 文字で切られます。
- `riscord.copy(text)` / `riscord.insert(text)`: クリップボードにコピー、または入力欄に挿入します。
- `riscord.open_url(url)`: `http://`・`https://`・`steam://` の URL を開きます。コマンド・メッセージの操作（`run`）・`on_action` の中でだけ使えます。受信や削除の通知・タイマーなどから呼ぶとエラーになります（本文を URL に入れて外へ送れないようにするため）。
- `riscord.open_channel(id)` / `riscord.set_status(status)`: チャンネルを開くか、`online`・`idle`・`dnd`・`invisible` に状態を変更します。
- `riscord.play_sound(name)` / `riscord.speak(text)`: サウンドを再生、またはテキストを読み上げます。
- `open_url`・`speak`・`set_status`・`copy` は全プラグイン合計で 1 分間に 20 回までです。超えた分は黙って捨てられます。
- `riscord.now()`: Unix 時刻のミリ秒を返します。`riscord.date(format, seconds, utc)` は指定時刻（秒、省略時は現在）を整形します。
- `riscord.me()`: `id`, `name`, `status`, `channel_id`, `guild_id`, `voice_channel_id` を含む現在の状態を返します。
- `riscord.set_timeout(milliseconds, function() ... end)`: 一度だけ実行するタイマー。最短 100 ミリ秒、最長 24 時間、プラグインあたり最大 100 個です。
- `riscord.load()` / `riscord.save(value)`: プラグインごとに JSON としてデータを読み書きします。値は JSON で表せるテーブル、文字列、数値、真偽値などにしてください。保存できるのは JSON にして 1 MiB まで、入れ子は 32 段まで、値とキーの数は合計 1 万個まで、文字列（キーを含む）は合計 2 MiB までです（同じ文字列を何度も入れると、その回数分数えます）。保存は一時ファイルに書いてから置き換えるので、途中で落ちても前のデータが残ります。保存データが壊れていて読めないときは、`load()` はエラーになってプラグインが停止し、壊れたファイルは `<id>.json.bak` に移されます（既にある `.bak` は上書きされます）。もう一度有効にすると空のデータ（`nil`）から始まります。

実行の制限:

- 使える標準ライブラリは `table`・`string`・`math`・`utf8` と基本関数だけです。`pcall`・`xpcall`・`load`・`loadfile`・`dofile`・`require`・`collectgarbage` は消してあります（エラーは捕まえられず、そのままプラグインが停止します）。`io`・`os`・`package`・`debug`・`coroutine` もありません。
- エントリファイルは文字のスクリプトとして読み込みます（コンパイル済みのバイトコードは読み込めません）。1 MiB までです。
- メモリはプラグインごとに 16 MiB までです。
- 1 回の呼び出しは Lua の命令 10 万個まで、かつ 200 ミリ秒までです。エントリファイルの読み込み（チャンクの実行）と `start()` はそれぞれ 1 秒までです。超えるとエラーになり、プラグインが停止します。
- 文字列の繰り返し・検索・パターン照合にも、1 回の呼び出しあたりの処理量の上限があります。
- `send_message`・`notify` などの引数は 1 つ 8000 バイトまで、操作の待ちはプラグインごとに 100 件までです。

ファイル・ネットワーク・OS コマンドへの直接アクセス、モジュール読み込み、トークン取得は提供しません。プラグインには受信メッセージの内容や送信機能が公開されるため、コードの出所を確認してから有効化してください。

同梱の CallTimeCounter はプラグイン一覧から有効にできます。通話中に有効化しても、現在の接続が確立してからの経過時間を表示します。切断・再接続や別チャンネルへの移動でカウンターはリセットされます。

同梱の MessageLogger は設定画面で有効化します。削除されたメッセージを一覧から消さずに赤く残し、編集されたメッセージには編集前の本文を下に表示します。記録は設定フォルダの `riscord/message-log.json` に保存され（削除・編集それぞれ最大 3000 件）、再起動後やチャンネルを開き直したときも元の位置に戻ります。「ページ」では自分や Bot のメッセージを記録しない設定、記録の消去、削除・編集・ゴーストピン（自分へのメンションを含む削除済みメッセージ）の一覧を使えます。一覧の項目を押すとそのチャンネルを開きます。有効にする前に受け取ったメッセージは、削除されても本文が分からないため記録できません。

同梱の Translator と AutoTranslate は、メッセージの本文（DM を含む）を外部の翻訳サービスへ送るため、「すべてオン」ではオンになりません。1 つずつ有効にしてください。

同梱の Translator は設定画面で有効化します。メッセージにマウスを重ねて「Google で翻訳」を押すと原文の下に翻訳を表示し、もう一度押すと閉じます。「ページ」から受信文の翻訳先言語コードを設定できます（既定は表示言語に合わせて `ja`・`en`・`zh-CN`）。送信欄で `$en こんにちは` のように入力すると指定言語に翻訳して送信します。`$jp` は日本語の別名です。翻訳に失敗したときは入力内容を復元できます。

翻訳する文章は Google 翻訳へ送信されます。Google の非公開エンドポイントを使用するため、アクセス制限や仕様変更で利用できない場合があります。受信文は手動で翻訳します。送信文は言語接頭辞のほか、プラグインのページで自動翻訳を有効にできます。自動翻訳中に `!tr ` を先頭に付けると、そのメッセージだけ原文を送信します。翻訳エンジンの切り替えには対応していません。
