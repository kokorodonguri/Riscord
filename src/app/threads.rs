//! スレッドとフォーラム。
//!
//! スレッドは普通のチャンネルと同じく messages / open_channel / 入力欄で扱う（別のチャット実装は持たない）。
//! 知っているスレッドは App::threads（サーバーごと）に置き、READY・GUILD_CREATE・THREAD_* で更新する。
//! 一覧は GET /channels/{id}/threads/search（ユーザーで使えるのはこれ。/threads/active は Bot 専用）。
use super::*;
use crate::net::ApiCall;

/// 1ページの件数（Discord の上限）
pub const THREAD_PAGE: u32 = 25;
const THREAD_RETRIES: u8 = 5;

/// スレッド・投稿の一覧（フォーラムの画面と、チャンネルのスレッド一覧で使う）
#[derive(Default)]
pub struct ThreadsView {
    pub channel_id: String,
    pub guild_id: String,
    /// アーカイブ済みを出す
    pub archived: bool,
    /// このタグの投稿だけ
    pub tag: Option<String>,
    pub items: Vec<Channel>,
    /// フォーラムの投稿の最初のメッセージ（スレッド ID → メッセージ）
    pub first_messages: HashMap<String, Message>,
    pub has_more: bool,
    pub loading: bool,
    pub error: Option<String>,
    /// 最後に送った読み込みの番号
    pub request: u64,
    retries: u8,
    pub retry_at: Option<std::time::Instant>,
    pub rev: u64,
}

/// フォーラムの新しい投稿の下書き
#[derive(Default)]
pub struct PostDraft {
    /// 付けるタグ（題・本文は App::post_title / post_body）
    pub tags: Vec<String>,
    pub sending: bool,
    pub error: Option<String>,
}

/// メッセージからスレッドを作る画面
pub struct ThreadDraft {
    pub message: Message,
    pub name: String,
    pub sending: bool,
    pub error: Option<String>,
}

/// スレッド検索の1ページ（スレッド, 最初のメッセージ, 続きがあるか, 参加中のスレッド）
type ThreadPage = (Vec<Channel>, Vec<Message>, bool, Vec<String>);

/// スレッド検索の結果を読む。索引の準備中なら Err(Some(待つ秒数))
fn parse_thread_page(v: &serde_json::Value) -> Result<ThreadPage, Option<f64>> {
    if v["code"] == 110000 || (v.get("threads").is_none() && v.get("retry_after").is_some()) {
        return Err(Some(v["retry_after"].as_f64().unwrap_or(2.0)));
    }
    let list = v["threads"].as_array().ok_or(None)?;
    let threads = list
        .iter()
        .filter_map(|t| serde_json::from_value::<Channel>(t.clone()).ok())
        .collect();
    let firsts = v["first_messages"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|m| serde_json::from_value::<Message>(m.clone()).ok())
        .collect();
    // 自分が参加しているスレッド
    let joined = v["members"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|m| m["id"].as_str().map(String::from))
        .collect();
    Ok((
        threads,
        firsts,
        v["has_more"].as_bool().unwrap_or(false),
        joined,
    ))
}

/// 投稿の最後の動き（新しい順に並べる）
pub fn thread_activity(c: &Channel) -> u64 {
    c.last_message_id
        .as_deref()
        .or(Some(c.id.as_str()))
        .and_then(|s| s.parse().ok())
        .unwrap_or(0)
}

impl App {
    /// サーバーのスレッドを足す・置き換える
    pub fn upsert_thread(&mut self, guild_id: &str, t: Channel) {
        let list = self.threads.entry(guild_id.to_string()).or_default();
        match list.iter_mut().find(|x| x.id == t.id) {
            Some(x) => *x = t.clone(),
            None => list.push(t.clone()),
        }
        for v in [self.forum.as_mut(), self.thread_panel.as_mut()]
            .into_iter()
            .flatten()
        {
            if let Some(x) = v.items.iter_mut().find(|x| x.id == t.id) {
                *x = t.clone();
                v.rev += 1;
            }
        }
    }

    fn remove_thread(&mut self, guild_id: &str, id: &str) {
        if let Some(l) = self.threads.get_mut(guild_id) {
            l.retain(|t| t.id != id);
        }
        self.joined_threads.remove(id);
        for v in [self.forum.as_mut(), self.thread_panel.as_mut()]
            .into_iter()
            .flatten()
        {
            v.items.retain(|t| t.id != id);
            v.rev += 1;
        }
    }

    /// サイドバーに出すスレッド（参加中・アーカイブされていない）を親ごとに
    pub fn sidebar_threads(&self, guild_id: &str) -> HashMap<&str, Vec<&Channel>> {
        let mut out: HashMap<&str, Vec<&Channel>> = HashMap::new();
        for t in self.threads.get(guild_id).into_iter().flatten() {
            let show = (self.joined_threads.contains(&t.id) && !t.archived())
                || self.sel_channel.as_deref() == Some(t.id.as_str());
            if let (true, Some(p)) = (show, t.parent_id.as_deref()) {
                out.entry(p).or_default().push(t);
            }
        }
        for l in out.values_mut() {
            l.sort_by_key(|t| std::cmp::Reverse(thread_activity(t)));
        }
        out
    }

    /// READY・GUILD_CREATE のスレッド（参加中のものには member が付く）
    pub(super) fn load_guild_threads(&mut self, guild_id: &str, list: &serde_json::Value) {
        for t in list.as_array().into_iter().flatten() {
            let Ok(c) = serde_json::from_value::<Channel>(t.clone()) else {
                continue;
            };
            if !t["member"].is_null() {
                self.joined_threads.insert(c.id.clone());
            }
            self.upsert_thread(guild_id, c);
        }
    }

    /// THREAD_* のゲートウェイイベント
    pub(super) fn on_thread_event(&mut self, t: &str, d: &serde_json::Value) {
        let me = self.me.as_ref().map(|u| u.id.clone()).unwrap_or_default();
        let gid = d["guild_id"].as_str().unwrap_or_default().to_string();
        match t {
            "THREAD_CREATE" | "THREAD_UPDATE" => {
                let Ok(c) = serde_json::from_value::<Channel>(d.clone()) else {
                    return;
                };
                if !d["member"].is_null()
                    || (t == "THREAD_CREATE"
                        && d["newly_created"] == true
                        && c.owner_id.as_deref() == Some(me.as_str()))
                {
                    self.joined_threads.insert(c.id.clone());
                }
                // 開いているフォーラムに新しい投稿が来たら先頭に足す
                if t == "THREAD_CREATE" {
                    if let Some(f) = self.forum.as_mut().filter(|f| {
                        c.parent_id.as_deref() == Some(f.channel_id.as_str()) && !f.archived
                    }) {
                        let fits = f
                            .tag
                            .as_ref()
                            .is_none_or(|tag| c.applied_tags.contains(tag));
                        if fits && !f.items.iter().any(|x| x.id == c.id) {
                            f.items.insert(0, c.clone());
                            f.rev += 1;
                        }
                    }
                }
                self.upsert_thread(&gid, c);
            }
            "THREAD_DELETE" => {
                let Some(id) = d["id"].as_str() else { return };
                self.remove_thread(&gid, id);
                // 開いていたら親のチャンネルへ戻る
                if self.sel_channel.as_deref() == Some(id) {
                    let parent = d["parent_id"].as_str().map(String::from);
                    self.sel_channel = None;
                    if let Some(p) = parent {
                        self.pending_open = Some(p);
                    }
                    self.show_toast(tr!("スレッドが削除されました", "The thread was deleted"));
                }
            }
            "THREAD_LIST_SYNC" => {
                // channel_ids があればその親の分だけ、無ければサーバー全体の一覧が届いた
                let parents: Option<Vec<&str>> = d["channel_ids"]
                    .as_array()
                    .map(|a| a.iter().filter_map(|v| v.as_str()).collect());
                if let Some(l) = self.threads.get_mut(&gid) {
                    l.retain(|t| match &parents {
                        Some(p) => !t.parent_id.as_deref().is_some_and(|x| p.contains(&x)),
                        None => false,
                    });
                }
                self.load_guild_threads(&gid, &d["threads"]);
                for m in d["members"].as_array().into_iter().flatten() {
                    if m["user_id"].as_str() == Some(me.as_str()) {
                        if let Some(id) = m["id"].as_str() {
                            self.joined_threads.insert(id.to_string());
                        }
                    }
                }
            }
            "THREAD_MEMBER_UPDATE" => {
                if let Some(id) = d["id"]
                    .as_str()
                    .filter(|_| d["user_id"].as_str().is_none_or(|u| u == me))
                {
                    self.joined_threads.insert(id.to_string());
                }
            }
            "THREAD_MEMBERS_UPDATE" => {
                let Some(id) = d["id"].as_str() else { return };
                let added = d["added_members"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .any(|m| m["user_id"].as_str() == Some(me.as_str()));
                let removed = d["removed_member_ids"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .any(|m| m.as_str() == Some(me.as_str()));
                if added {
                    self.joined_threads.insert(id.to_string());
                } else if removed {
                    self.joined_threads.remove(id);
                }
            }
            _ => {}
        }
    }

    /// 開いているのがフォーラムなら、その一覧を（なければ）読む
    pub fn ensure_forum(&mut self) {
        let Some(c) = self.current_channel().filter(|c| c.is_forum()).cloned() else {
            self.forum = None;
            return;
        };
        if self.forum.as_ref().is_some_and(|f| f.channel_id == c.id) {
            return;
        }
        let gid = self.sel_guild.clone().unwrap_or_default();
        self.forum = Some(ThreadsView {
            channel_id: c.id,
            guild_id: gid,
            ..Default::default()
        });
        self.post_draft = None;
        self.load_threads(true, false);
    }

    /// チャンネルのスレッド一覧の枠を出す・閉じる
    pub fn toggle_thread_panel(&mut self) {
        if self.side == SidePanel::Threads {
            self.close_side();
            return;
        }
        let Some(cid) = self.sel_channel.clone() else {
            return;
        };
        self.close_side();
        self.side = SidePanel::Threads;
        self.thread_panel = Some(ThreadsView {
            channel_id: cid,
            guild_id: self.sel_guild.clone().unwrap_or_default(),
            ..Default::default()
        });
        self.load_threads(false, false);
    }

    fn view_mut(&mut self, forum: bool) -> Option<&mut ThreadsView> {
        if forum {
            self.forum.as_mut()
        } else {
            self.thread_panel.as_mut()
        }
    }

    /// 一覧を読む（more = 続きのページ）
    pub fn load_threads(&mut self, forum: bool, more: bool) {
        let Some(v) = self.view_mut(forum) else {
            return;
        };
        if v.loading || (more && !v.has_more) {
            return;
        }
        if !more {
            v.items.clear();
            v.first_messages.clear();
            v.has_more = false;
            v.retries = 0;
        }
        v.loading = true;
        v.error = None;
        v.retry_at = None;
        v.rev += 1;
        let mut query = vec![
            ("archived".to_string(), v.archived.to_string()),
            ("sort_by".to_string(), "last_message_time".to_string()),
            ("sort_order".to_string(), "desc".to_string()),
            ("limit".to_string(), THREAD_PAGE.to_string()),
            ("offset".to_string(), v.items.len().to_string()),
        ];
        if let Some(t) = &v.tag {
            query.push(("tag".into(), t.clone()));
        }
        let call = ApiCall::SearchThreads {
            channel_id: v.channel_id.clone(),
            query,
        };
        let id = self.api(call);
        if let Some(v) = self.view_mut(forum) {
            v.request = id;
        }
    }

    /// タグで絞る・アーカイブ済みに切り替える
    pub fn set_thread_filter(
        &mut self,
        forum: bool,
        tag: Option<Option<String>>,
        archived: Option<bool>,
    ) {
        let Some(v) = self.view_mut(forum) else {
            return;
        };
        if let Some(t) = tag {
            v.tag = t;
        }
        if let Some(a) = archived {
            v.archived = a;
        }
        v.loading = false;
        self.load_threads(forum, false);
    }

    /// 待ち時間が過ぎたやり直しを送る
    pub fn tick_threads(&mut self) {
        let now = std::time::Instant::now();
        for forum in [true, false] {
            if let Some(v) = self
                .view_mut(forum)
                .filter(|v| v.retry_at.is_some_and(|t| now >= t))
            {
                v.retry_at = None;
                v.loading = false;
                // 同じページをもう一度
                let keep = v.retries;
                let more = !v.items.is_empty();
                self.load_threads(forum, more);
                if let Some(v) = self.view_mut(forum) {
                    v.retries = keep;
                }
            }
        }
    }

    pub(super) fn on_threads_loaded(&mut self, id: u64, result: Result<serde_json::Value, String>) {
        let forum = if self.forum.as_ref().is_some_and(|v| v.request == id) {
            true
        } else if self.thread_panel.as_ref().is_some_and(|v| v.request == id) {
            false
        } else {
            return;
        };
        let parsed = result.as_ref().map(parse_thread_page);
        let gid = self
            .view_mut(forum)
            .map(|v| v.guild_id.clone())
            .unwrap_or_default();
        if let Ok(Ok((threads, _, _, joined))) = &parsed {
            self.joined_threads.extend(joined.iter().cloned());
            // 開けるよう、知っているスレッドにも入れておく
            for t in threads {
                self.upsert_thread(&gid, t.clone());
            }
        }
        let Some(v) = self.view_mut(forum) else {
            return;
        };
        v.rev += 1;
        match parsed {
            Ok(Ok((threads, firsts, has_more, _))) => {
                v.loading = false;
                let have: HashSet<String> = v.items.iter().map(|t| t.id.clone()).collect();
                v.items
                    .extend(threads.into_iter().filter(|t| !have.contains(&t.id)));
                for m in firsts {
                    v.first_messages.insert(m.channel_id.clone(), m);
                }
                v.has_more = has_more;
            }
            Ok(Err(Some(wait))) if v.retries < THREAD_RETRIES => {
                v.retries += 1;
                let wait = std::time::Duration::from_secs_f64(wait.clamp(1.0, 30.0));
                v.retry_at = Some(std::time::Instant::now() + wait);
                self.repaint_after(wait);
            }
            Ok(Err(_)) => {
                v.loading = false;
                v.error = Some(
                    tr!(
                        "スレッドの一覧を読めませんでした",
                        "Couldn't read the thread list"
                    )
                    .into(),
                );
            }
            Err(e) => {
                eprintln!("スレッドの一覧を読めませんでした: {e}");
                v.loading = false;
                v.error = Some(api_error_text(
                    e,
                    tr!(
                        "スレッドの一覧を読めませんでした",
                        "Couldn't read the thread list"
                    ),
                ));
            }
        }
    }

    /// スレッドを開く（知らないスレッドなら覚えてから）
    pub fn open_thread(&mut self, t: Channel) {
        let gid = self.sel_guild.clone().unwrap_or_default();
        let id = t.id.clone();
        self.upsert_thread(&gid, t);
        self.pending_open = Some(id);
    }

    pub fn join_thread(&mut self, id: &str, join: bool) {
        let call = if join {
            ApiCall::JoinThread(id.to_string())
        } else {
            ApiCall::LeaveThread(id.to_string())
        };
        self.api(call);
    }

    pub(super) fn on_thread_join_result(
        &mut self,
        id: String,
        join: bool,
        result: Result<serde_json::Value, String>,
    ) {
        match result {
            Ok(_) => {
                if join {
                    self.joined_threads.insert(id);
                    self.show_toast(tr!("スレッドに参加しました", "Joined the thread"));
                } else {
                    self.joined_threads.remove(&id);
                    self.show_toast(tr!("スレッドから退出しました", "Left the thread"));
                }
            }
            Err(e) => {
                eprintln!("スレッドの参加・退出に失敗しました: {e}");
                self.show_toast(&api_error_text(
                    &e,
                    if join {
                        tr!("スレッドに参加できませんでした", "Couldn't join the thread")
                    } else {
                        tr!(
                            "スレッドから退出できませんでした",
                            "Couldn't leave the thread"
                        )
                    },
                ));
            }
        }
    }

    /// このスレッドで書けるか。書けなければ理由
    pub fn thread_block_reason(&self, c: &Channel) -> Option<&'static str> {
        if !c.is_thread() {
            return None;
        }
        let perms = self.channel_perms(self.sel_guild.as_deref(), &c.id);
        let manage = perms.is_some_and(|p| p & model::MANAGE_THREADS != 0);
        if c.locked() && !manage {
            return Some(tr!(
                "このスレッドはロックされているため、メッセージを送信できません。",
                "This thread is locked, so you can't send messages."
            ));
        }
        if perms.is_some_and(|p| p & model::SEND_MESSAGES_IN_THREADS == 0) {
            return Some(tr!(
                "このスレッドでメッセージを送信する権限がありません。",
                "You don't have permission to send messages in this thread."
            ));
        }
        None
    }

    /// メッセージからスレッドを作れるか
    pub fn can_create_thread(&self, m: &Message) -> bool {
        let Some(c) = self.current_channel() else {
            return false;
        };
        if self.sel_guild.is_none() || !c.is_text() || m.kind != 0 && m.kind != 19 {
            return false;
        }
        self.channel_perms(self.sel_guild.as_deref(), &c.id)
            .is_none_or(|p| p & model::CREATE_PUBLIC_THREADS != 0)
    }

    pub fn start_thread_draft(&mut self, m: Message) {
        // 名前の初期値は本文の頭（Discord と同じく 100 文字まで）
        let name: String = m
            .content
            .lines()
            .next()
            .unwrap_or("")
            .chars()
            .take(40)
            .collect();
        self.thread_name = if name.trim().is_empty() {
            tr!("新しいスレッド", "New Thread").into()
        } else {
            name
        };
        self.thread_draft = Some(ThreadDraft {
            message: m,
            name: String::new(),
            sending: false,
            error: None,
        });
    }

    pub fn submit_thread_draft(&mut self) {
        let name = self
            .thread_name
            .trim()
            .chars()
            .take(100)
            .collect::<String>();
        let Some(d) = self.thread_draft.as_mut().filter(|d| !d.sending) else {
            return;
        };
        if name.is_empty() {
            d.error = Some(tr!("スレッド名を入力してください", "Enter a thread name").into());
            return;
        }
        d.sending = true;
        d.error = None;
        d.name = name.clone();
        let call = ApiCall::CreateThreadFromMessage {
            channel_id: d.message.channel_id.clone(),
            message_id: d.message.id.clone(),
            body: serde_json::json!({ "name": name, "auto_archive_duration": 1440 }),
        };
        self.api(call);
    }

    pub(super) fn on_thread_created(
        &mut self,
        from_message: bool,
        result: Result<serde_json::Value, String>,
    ) {
        let t = result
            .as_ref()
            .ok()
            .and_then(|v| serde_json::from_value::<Channel>(v.clone()).ok());
        match (t, result) {
            (Some(t), _) => {
                self.joined_threads.insert(t.id.clone());
                if from_message {
                    self.thread_draft = None;
                } else {
                    self.post_draft = None;
                    self.post_title.clear();
                    self.post_body.clear();
                }
                self.open_thread(t);
            }
            (None, r) => {
                let e = r.err().unwrap_or_else(|| {
                    tr!("応答を読めませんでした", "Couldn't read the response").into()
                });
                eprintln!("スレッドを作れませんでした: {e}");
                let text = if from_message {
                    tr!(
                        "スレッドを作成できませんでした",
                        "Couldn't create the thread"
                    )
                } else {
                    tr!("投稿できませんでした", "Couldn't post")
                };
                let msg = api_error_text(&e, text);
                if from_message {
                    if let Some(d) = self.thread_draft.as_mut() {
                        d.sending = false;
                        d.error = Some(msg);
                    }
                } else if let Some(d) = self.post_draft.as_mut() {
                    d.sending = false;
                    d.error = Some(msg);
                }
            }
        }
    }

    /// フォーラムに投稿する
    pub fn submit_post(&mut self) {
        let Some(forum) = self.current_channel().filter(|c| c.is_forum()).cloned() else {
            return;
        };
        let (title, body) = (
            self.post_title.trim().to_string(),
            self.post_body.trim().to_string(),
        );
        let Some(d) = self.post_draft.as_mut().filter(|d| !d.sending) else {
            return;
        };
        d.error = if title.is_empty() {
            Some(tr!("タイトルを入力してください", "Enter a title").into())
        } else if body.is_empty() {
            Some(tr!("本文を入力してください", "Enter a message").into())
        } else if forum.requires_tag() && d.tags.is_empty() {
            Some(
                tr!(
                    "このフォーラムではタグを1つ以上選んでください",
                    "Choose at least one tag in this forum"
                )
                .into(),
            )
        } else {
            None
        };
        if d.error.is_some() {
            return;
        }
        d.sending = true;
        let body = serde_json::json!({
            "name": title.chars().take(100).collect::<String>(),
            "applied_tags": d.tags,
            "message": { "content": body },
        });
        self.api(ApiCall::CreateThread {
            channel_id: forum.id,
            body,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn thread(id: &str, parent: &str, archived: bool) -> serde_json::Value {
        serde_json::json!({ "id": id, "type": 11, "guild_id": "g", "parent_id": parent, "name": format!("t{id}"), "thread_metadata": { "archived": archived, "locked": false } })
    }

    fn harness() -> (
        App,
        std::sync::mpsc::Sender<Event>,
        tokio::sync::mpsc::UnboundedReceiver<Cmd>,
    ) {
        let mut app = App::default();
        let (tx, ev) = std::sync::mpsc::channel();
        let (cmd, rx) = tokio::sync::mpsc::unbounded_channel();
        app.backend = Some(Backend { cmd, ev });
        app.me = Some(
            serde_json::from_value(serde_json::json!({ "id": "me", "username": "me" })).unwrap(),
        );
        app.sel_guild = Some("g".into());
        app.channels.insert("g".into(), vec![
            serde_json::from_value(serde_json::json!({ "id": "c", "type": 0, "name": "一般" })).unwrap(),
            serde_json::from_value(serde_json::json!({ "id": "f", "type": 15, "name": "相談", "available_tags": [{ "id": "t1", "name": "質問" }] })).unwrap(),
        ]);
        (app, tx, rx)
    }

    #[test]
    fn gateway_keeps_joined_threads_in_sidebar() {
        let (mut app, tx, _rx) = harness();
        let mut joined = thread("1", "c", false);
        joined["member"] = serde_json::json!({ "user_id": "me" });
        tx.send(Event::Gateway {
            t: "THREAD_CREATE".into(),
            d: joined,
        })
        .unwrap();
        tx.send(Event::Gateway {
            t: "THREAD_CREATE".into(),
            d: thread("2", "c", false),
        })
        .unwrap();
        app.poll();
        let side = app.sidebar_threads("g");
        assert_eq!(
            side["c"].iter().map(|t| t.id.as_str()).collect::<Vec<_>>(),
            ["1"]
        );
        // アーカイブされたら出さない
        tx.send(Event::Gateway {
            t: "THREAD_UPDATE".into(),
            d: thread("1", "c", true),
        })
        .unwrap();
        app.poll();
        assert!(app.sidebar_threads("g").is_empty());
        // 一覧の同期で置き換わる
        tx.send(Event::Gateway { t: "THREAD_LIST_SYNC".into(), d: serde_json::json!({ "guild_id": "g", "channel_ids": ["c"], "threads": [thread("3", "c", false)], "members": [{ "id": "3", "user_id": "me" }] }) }).unwrap();
        app.poll();
        assert_eq!(
            app.threads["g"]
                .iter()
                .map(|t| t.id.as_str())
                .collect::<Vec<_>>(),
            ["3"]
        );
        assert!(app.joined_threads.contains("3"));
        // 退出したら出さない
        tx.send(Event::Gateway {
            t: "THREAD_MEMBERS_UPDATE".into(),
            d: serde_json::json!({ "id": "3", "guild_id": "g", "removed_member_ids": ["me"] }),
        })
        .unwrap();
        app.poll();
        assert!(!app.joined_threads.contains("3"));
    }

    #[test]
    fn opened_thread_is_a_normal_channel() {
        let (mut app, _tx, _rx) = harness();
        app.upsert_thread(
            "g",
            serde_json::from_value(thread("9", "c", false)).unwrap(),
        );
        app.open_channel("9".into());
        let c = app.current_channel().unwrap();
        assert!(c.is_thread());
        assert_eq!(c.parent_id.as_deref(), Some("c"));
    }

    #[test]
    fn forum_pages_filter_by_tag_and_follow_new_posts() {
        let (mut app, tx, mut rx) = harness();
        app.open_channel("f".into());
        // 開いたチャンネルの読み込み（フォーラムには無いが、今まで通り頼む）は読み飛ばす
        while let Ok(c) = rx.try_recv() {
            if let Cmd::Api {
                id,
                call: call @ ApiCall::SearchThreads { .. },
            } = c
            {
                panic!("まだ頼んでいない {id} {call:?}");
            }
        }
        app.ensure_forum();
        let Cmd::Api {
            id,
            call: ApiCall::SearchThreads { channel_id, query },
        } = rx.try_recv().unwrap()
        else {
            panic!()
        };
        assert_eq!(channel_id, "f");
        assert!(query.contains(&("archived".into(), "false".into())));
        let first = serde_json::json!({ "id": "100", "channel_id": "p1", "author": { "id": "u", "username": "u" }, "content": "本文", "timestamp": "2026-01-01T00:00:00+00:00" });
        tx.send(Event::Api { id, call: ApiCall::SearchThreads { channel_id: "f".into(), query: vec![] }, result: Ok(serde_json::json!({ "threads": [thread("p1", "f", false)], "first_messages": [first], "members": [], "has_more": true })) }).unwrap();
        app.poll();
        let f = app.forum.as_ref().unwrap();
        assert_eq!(f.items.len(), 1);
        assert_eq!(f.first_messages["p1"].content, "本文");
        assert!(f.has_more);
        // タグで絞ると先頭から読み直す
        app.set_thread_filter(true, Some(Some("t1".into())), None);
        let Cmd::Api {
            call: ApiCall::SearchThreads { query, .. },
            ..
        } = rx.try_recv().unwrap()
        else {
            panic!()
        };
        assert!(
            query.contains(&("tag".into(), "t1".into()))
                && query.contains(&("offset".into(), "0".into()))
        );
        assert!(app.forum.as_ref().unwrap().items.is_empty());
    }

    #[test]
    fn locked_thread_blocks_compose_without_manage_permission() {
        let (mut app, _tx, _rx) = harness();
        let mut t = thread("9", "c", false);
        t["thread_metadata"]["locked"] = true.into();
        let c: Channel = serde_json::from_value(t).unwrap();
        assert!(app.thread_block_reason(&c).unwrap().contains("ロック"));
        // オーナーは管理できる
        app.guilds.push(Guild {
            id: "g".into(),
            name: "g".into(),
            icon: None,
            permissions: None,
            owner: true,
        });
        app.upsert_thread("g", c.clone());
        assert!(app.thread_block_reason(&c).is_none());
    }
}
