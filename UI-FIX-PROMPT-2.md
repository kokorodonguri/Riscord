# riscord UI修正 第2弾（本家Discordへの追随：メンバーリスト実装＋ロールカラー＋細部）

riscord（Rust + egui 0.31 製 Discord クライアント）を本家 Discord デスクトップ版にさらに近づける。最大の差分である「メンバーリストが無い」点を、データ取得込みで実装する。

## 厳守事項

- 既存のファイル構成・命名・日本語コメントの流儀を維持する。変更範囲はこの指示の項目だけ。
- 既存機能（ログイン、メッセージ送受信、ボイス、プロフィールポップアップ、設定）を壊さない。
- 作業後に `cargo build` が通ることを確認し、変更箇所の一覧と、動作確認できていない点を正直に報告すること。
- ユーザートークン前提のゲートウェイである。identify に intents を追加してはいけない（現状 intents 無しで運用中）。

## 1. データモデル（src/model.rs）

以下を追加する。

```rust
/// サーバーのロール
#[derive(Clone, Debug, Deserialize)]
pub struct Role {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub color: u32,
    #[serde(default)]
    pub position: i64,
    #[serde(default)]
    pub hoist: bool,
}

/// サーバーメンバー（ゲートウェイのメンバーチャンク / メッセージの member から作る）
#[derive(Clone, Debug, Deserialize)]
pub struct Member {
    #[serde(default)]
    pub user: Option<User>,
    #[serde(default)]
    pub nick: Option<String>,
    #[serde(default)]
    pub roles: Vec<String>,
}
```

`Message` にフィールドを追加する（ギルドメッセージには API/ゲートウェイとも `member` が付く）。

```rust
    #[serde(default)]
    pub member: Option<Member>,
```

`Member` に表示名ヘルパーを付ける： `display()` は nick があればそれ、無ければ user.display()、user が無ければ空文字。

## 2. ロール取得（src/net.rs、REST）

- `Event` に `GuildRoles(String, Vec<Role>)` を追加する。
- 既存の `Cmd::LoadGuildChannels(gid)` の処理でチャンネル一覧を取得する際、同じタイミングで `GET /guilds/{gid}/roles` も叩き、成功したら `Event::GuildRoles` を送る。API の呼び出し方（ベースURL・認証ヘッダー）は既存の REST 呼び出しに揃える。
- 失敗してもエラーイベントは出さない（ロール無しとして続行）。

## 3. メンバー取得（src/net.rs、ゲートウェイ op 8）

本家クライアントと同じく、ゲートウェイの Request Guild Members（op 8）で取得する。

- `Cmd` に `LoadMembers(String)`（guild_id）を追加する。
- Cmd からゲートウェイ書き込み側への受け渡しは、既存の `Cmd::JoinVoice` がたどる経路（内部メッセージ経由でゲートウェイタスクへ渡す仕組み）に倣う。
- ゲートウェイ側では次の JSON を送信する：
  `{"op": 8, "d": {"guild_id": gid, "query": "", "limit": 0, "presences": true}}`
- ディスパッチに `"GUILD_MEMBERS_CHUNK"` を追加する。`members` 配列を `Vec<Member>` に変換し、`Event::GuildMembers(gid, members)` を送る。チャンクに `presences` 配列が含まれていれば、既存の Presence 変換処理を再利用して `Event::Presences` も送る。
- `Event` に `GuildMembers(String, Vec<Member>)` を追加する。

## 4. アプリ状態と読み込みトリガー（src/main.rs）

`App` に追加する：

- `roles: HashMap<String, Vec<Role>>`（guild_id → ロール一覧）
- `members: HashMap<String, Vec<Member>>`（guild_id → メンバー一覧）
- `members_requested: HashSet<String>`（op 8 を送済みのギルド）
- `show_members: bool`（初期値 true）

動作：

- ギルドが選択されたとき（既存の `LoadGuildChannels` を送る箇所）、roles が未取得なら同時に取得され（2. の仕組み上 LoadGuildChannels に相乗りでよい）、members が未要求なら `Cmd::LoadMembers(gid)` を送り `members_requested` に記録する。
- `Event::GuildRoles` / `Event::GuildMembers` を `poll()` で受けて保存する。GuildMembers はチャンクが複数回来る前提で、user.id をキーにマージする（既存エントリは新しいもので置き換え、重複させない）。

## 5. メンバーリスト UI（src/main.rs、本家準拠）

ギルド選択中かつ `show_members` が true のとき、右端にメンバーリストを表示する。

- `egui::SidePanel::right("members")`、幅 240px 固定、リサイズ不可、区切り線なし、背景は定数 `SIDEBAR`。
- パネルの配置順序は egui の流儀に従い、チャット側の TopBottomPanel / CentralPanel より前に宣言すること。
- グループ分け（本家どおり）：
  1. `hoist == true` のロールを position の降順に。各グループには「そのロールを持ち、より上位の hoist グループに入っていない」メンバーで、オンライン（presence の status が "offline" 以外、または presence 自体が無い場合はオフライン扱い…ではなく presence が無いメンバーはオフライン側に寄せる）のみ入れる。
  2. 「オンライン」グループ：どの hoist グループにも入らなかったオンラインのメンバー。
  3. 「オフライン」グループ：オフラインのメンバー全員。
- グループ見出し：`ロール名 — 人数` の形式（例：`管理者 — 3`、`オンライン — 12`、`オフライン — 40`）。12px、色は `MUTED`。上に 16px の余白、左に 16px のインデント。空のグループは出さない。
- メンバー行：高さ 42px。左端から 8px の内側余白を取り、行の背景は角丸 4px で、ホバー時に `HOVER` 色。
  - アバター 32px（円形）、その右 12px に名前（16px）。
  - 名前の色は、そのメンバーの最上位ロール（position 最大）の color が 0 でなければその色、無ければ `TEXT`。
  - presence に activity があれば名前の下に 12px、`MUTED` で表示する。
  - アバター右下にステータスドットを既存の `paint_status` で描く（リング色は `SIDEBAR`、ホバー時は `HOVER`）。
  - オフラインのメンバーは行全体を不透明度 0.3 で描く（色に `gamma_multiply(0.3)` を掛ける等、既存の表現方法に合わせる）。
  - 行内での並びは表示名の昇順（大文字小文字を区別しない）。
  - クリックしたら既存のプロフィールポップアップを開く（メッセージ行のアバタークリックと同じ `clicked_user` の仕組みを使う）。
- スクロールは `ScrollArea::vertical` で行う。

## 6. チャットヘッダーにメンバーリスト切替ボタン

- チャットヘッダー（`chat_head`）の右側に、本家のメンバーリストアイコン相当のトグルボタンを置く。既存の `Icon` / `paint_glyph` の仕組みに `Members`（人物が2人並んだ形）を追加し、32px のボタ
...[truncated 2247 chars]