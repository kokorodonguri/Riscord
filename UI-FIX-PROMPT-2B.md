# riscord UI修正 第2弾・残り分（3項目のみ）

前回の指示で未実施だった3項目だけを行う。対象は `src/main.rs` のみ。

## 厳守事項

- 既存機能・既存の変更（メンバーリスト等）を壊さない。変更は以下の3項目だけ。
- 作業後に `cargo build` が通ることを確認し、変更箇所を報告すること。

## 1. Medium ウェイトフォント

`setup_fonts` で通常フォントに加えて Medium ウェイトのフォントも読み込み、`FontFamily::Name("medium")` として登録する。

- Windows: `%WINDIR%\Fonts\YuGothM.ttc`
- Linux: `fc-match -f %{file} "sans:lang=ja:weight=100"` の結果があればそれを、無ければ `/usr/share/fonts/noto-cjk/NotoSansCJK-Medium.ttc`
- macOS: `/System/Library/Fonts/ヒラギノ角ゴシック W5.ttc`
- 読み込めた場合のみ登録する。失敗しても通常フォントで動作を続ける。

以下の表示に、サイズは現状のままフォントだけ `FontId::new(現在のサイズ, egui::FontFamily::Name("medium".into()))` を使う。登録に失敗している場合は通常フォントにフォールバックすること（`FontId::new` は未登録ファミリーでも panic しない想定だが、気になる場合は登録成功フラグで分岐する）。

- チャンネル名（`channel_row` の名前描画）
- チャンネル欄ヘッダーのサーバー名タイトル
- チャットヘッダーのチャンネル名
- メッセージのユーザー名（通常表示・コンパクト表示）
- メンバーリストのメンバー名

## 2. ウィンドウタイトル

本家 Discord と同じ形式に、選択状態が変わったときだけ更新する。

- ギルドのチャンネル選択中: `#チャンネル名 - サーバー名 - Discord`
- DM 選択中: `@相手の表示名 - Discord`
- 未選択: `Discord`

実装は `App` に `last_title: String` を持ち、`main_ui` 内で現在のタイトル文字列を組み立てて、変化していたら `ctx.send_viewport_cmd(egui::ViewportCommand::Title(...))` を送る。

## 3. ログイン画面

`login_ui` の見た目だけを本家に寄せる。ロジックは変更しない。

- カード幅 `ui.set_width(400.0)` → `480.0`
- トークン入力欄の背景を本家の入力欄と同じ暗色（定数 `RAIL` = #1e1f22）にする。`egui::Frame` で囲う等、egui の標準的な方法でよい。
