//! メッセージ検索。サーバーは GET /guilds/{id}/messages/search、DM は GET /channels/{id}/messages/search。
//!
//! 入力は Discord と同じく「本文 from:名前 in:チャンネル has:画像 before:2026-01-01」の形で書く。
//! 打つたびには送らず、手を止めて少し経つか Enter で送る。前の検索の結果が後から届いても捨てる（世代で見分ける）。
//! 索引の準備中（202）は、言われた時間だけ待って数回までやり直す。
use super::*;
use crate::net::ApiCall;

/// 1ページの件数（Discord の上限）
pub const SEARCH_PAGE: u32 = 25;
/// offset の上限（Discord の上限）
pub const SEARCH_MAX_OFFSET: u32 = 9975;
/// 打つのを止めてから検索するまで
pub const SEARCH_DEBOUNCE: std::time::Duration = std::time::Duration::from_millis(700);
/// 索引の準備中にやり直す回数
const SEARCH_RETRIES: u8 = 5;

/// どこを探すか
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SearchScope {
    Guild(String),
    /// DM・グループ DM
    Channel(String),
}

pub struct SearchHit {
    pub message: Message,
    /// 「#一般」など（分かれば）
    pub place: String,
}

#[derive(Default)]
pub struct SearchView {
    pub scope: Option<SearchScope>,
    /// 実際に送った条件（ページ送りで使い回す）
    pub params: Vec<(String, String)>,
    pub offset: u32,
    pub total: u32,
    pub hits: Vec<SearchHit>,
    /// 応答のうち表示できない項目があった
    pub partial: bool,
    pub loading: bool,
    pub error: Option<String>,
    /// 最後に送った検索の番号（App::api の id）。違う番号の結果は古いので捨てる
    pub request: u64,
    retries: u8,
    pub retry_at: Option<std::time::Instant>,
    /// 中身が変わるたびに増やす（画面の一覧を作り直す目印）
    pub rev: u64,
    /// 一度でも検索した
    pub searched: bool,
}

impl SearchView {
    pub fn result_label(&self) -> String {
        let count = self.hits.len();
        if self.partial {
            return tr!(
                "表示中: {count} 件（一部の結果を表示できません）",
                "Showing {count} (some results can't be shown)",
                "显示 {count} 条（部分结果无法显示）",
            );
        }
        if count == 0 {
            return if self.total == 0 {
                tr!(
                    "一致するメッセージはありません",
                    "No matching messages",
                    "没有匹配的消息"
                )
                .to_string()
            } else {
                tr!(
                    "このページに表示できる結果はありません（全 {} 件）",
                    "No results to show on this page ({} total)",
                    "此页面没有可显示的结果（共 {} 条）",
                    self.total
                )
            };
        }
        let from = u64::from(self.offset) + 1;
        let to = u64::from(self.offset).saturating_add(count as u64);
        if to > u64::from(self.total) {
            tr!(
                "表示中: {count} 件（全件数は確認できません）",
                "Showing {count} (total unknown)",
                "显示 {count} 条（总数未知）",
            )
        } else {
            tr!(
                "{from}–{to} 件目（全 {} 件）",
                "{from}–{to} of {}",
                "第 {from}–{to} 条（共 {} 条）",
                self.total
            )
        }
    }
}

/// 検索語を解くときに名前から ID を引くもの
pub trait SearchNames {
    fn user_id(&self, name: &str) -> Option<String>;
    fn channel_id(&self, name: &str) -> Option<String>;
}

/// 2015-01-01 からのミリ秒を Discord の ID にする（その時刻より前・後の境目に使う）
pub fn snowflake_at(t: chrono::DateTime<chrono::Utc>) -> u64 {
    let ms = (t.timestamp_millis() - 1_420_070_400_000).max(0) as u64;
    ms << 22
}

/// 「2026-01-31」「2026/1/31」を、この端末の時刻の 0 時として読む
fn parse_day(s: &str) -> Option<chrono::DateTime<chrono::Utc>> {
    let s = s.replace('/', "-");
    let d = chrono::NaiveDate::parse_from_str(&s, "%Y-%m-%d").ok()?;
    let local = d
        .and_hms_opt(0, 0, 0)?
        .and_local_timezone(Local)
        .earliest()?;
    Some(local.with_timezone(&chrono::Utc))
}

/// has: に書ける種類（日本語でも書ける）
fn has_kind(s: &str) -> Option<&'static str> {
    Some(match s.to_lowercase().as_str() {
        "link" | "リンク" => "link",
        "embed" | "埋め込み" => "embed",
        "file" | "ファイル" => "file",
        "video" | "動画" => "video",
        "image" | "画像" => "image",
        "sound" | "音声" => "sound",
        "sticker" | "スタンプ" => "sticker",
        "poll" | "投票" => "poll",
        _ => return None,
    })
}

/// 検索語を Discord の条件にする。名前が見つからない・日付が読めないなどは理由を返す
pub fn parse_search(
    q: &str,
    in_guild: bool,
    names: &dyn SearchNames,
) -> Result<Vec<(String, String)>, String> {
    let mut params = Vec::new();
    let mut words = Vec::new();
    let day = |v: &str| {
        parse_day(v).ok_or_else(|| {
            tr!(
                "日付「{v}」を読めません（2026-01-31 の形で書いてください）",
                "Can't read the date \"{v}\" (write it like 2026-01-31)",
                "无法读取日期“{v}”（请写成 2026-01-31 的形式）",
            )
        })
    };
    for tok in q.split_whitespace() {
        let Some((key, v)) = tok
            .split_once(':')
            .filter(|(k, v)| !v.is_empty() && !k.is_empty())
        else {
            words.push(tok);
            continue;
        };
        match key.to_lowercase().as_str() {
            "from" | "mentions" => {
                let id = names.user_id(v).ok_or_else(|| {
                    tr!(
                        "ユーザー「{v}」が見つかりません",
                        "User \"{v}\" not found",
                        "找不到用户“{v}”",
                    )
                })?;
                params.push((
                    if key.eq_ignore_ascii_case("from") {
                        "author_id"
                    } else {
                        "mentions"
                    }
                    .to_string(),
                    id,
                ));
            }
            "in" => {
                if !in_guild {
                    return Err(tr!(
                        "DM では in: は使えません",
                        "in: can't be used in DMs",
                        "私信中不能使用 in:"
                    )
                    .into());
                }
                let id = names.channel_id(v).ok_or_else(|| {
                    tr!(
                        "チャンネル「{v}」が見つかりません",
                        "Channel \"{v}\" not found",
                        "找不到频道“{v}”",
                    )
                })?;
                params.push(("channel_id".into(), id));
            }
            "has" => {
                let k = has_kind(v).ok_or_else(|| tr!("has:{v} は使えません（link・embed・file・video・image・sound・sticker・poll）", "has:{v} isn't supported (link, embed, file, video, image, sound, sticker, poll)", "不支持 has:{v}（link、embed、file、video、image、sound、sticker、poll）",))?;
                params.push(("has".into(), k.into()));
            }
            "before" => params.push(("max_id".into(), snowflake_at(day(v)?).to_string())),
            "after" => params.push((
                "min_id".into(),
                snowflake_at(day(v)? + chrono::Duration::days(1)).to_string(),
            )),
            "during" => {
                let d = day(v)?;
                params.push(("min_id".into(), snowflake_at(d).to_string()));
                params.push((
                    "max_id".into(),
                    snowflake_at(d + chrono::Duration::days(1)).to_string(),
                ));
            }
            "pinned" => params.push((
                "pinned".into(),
                matches!(v.to_lowercase().as_str(), "true" | "yes" | "はい").to_string(),
            )),
            // 知らない「xx:」は本文として探す（URL など）
            _ => words.push(tok),
        }
    }
    let content = words.join(" ");
    if !content.is_empty() {
        if content.chars().count() > 1024 {
            return Err(tr!("検索語が長すぎます", "The search is too long", "搜索词过长").into());
        }
        params.insert(0, ("content".into(), content));
    }
    Ok(params)
}

/// 検索結果を読む。索引の準備中なら Err(Some(待つ秒数))
fn parse_results(v: &serde_json::Value) -> Result<(Vec<Message>, u32, bool), Option<f64>> {
    if v["code"] == 110000 || (v.get("messages").is_none() && v.get("retry_after").is_some()) {
        return Err(Some(v["retry_after"].as_f64().unwrap_or(2.0)));
    }
    let groups = v["messages"].as_array().ok_or(None)?;
    let total = v["total_results"]
        .as_u64()
        .unwrap_or(groups.len() as u64)
        .min(u32::MAX as u64) as u32;
    let mut out = Vec::with_capacity(groups.len());
    for g in groups {
        // 1件ずつの配列（昔は前後の文脈も入っていて、当たりに hit: true が付いていた）
        let hit = match g.as_array() {
            Some(a) => a.iter().find(|m| m["hit"] == true).or(a.first()),
            None => Some(g),
        };
        if let Some(m) = hit.and_then(|m| serde_json::from_value::<Message>(m.clone()).ok()) {
            out.push(m);
        }
    }
    let partial = out.len() != groups.len();
    if partial && out.is_empty() {
        return Err(None);
    }
    Ok((out, total, partial))
}

/// 今いる場所から名前を引く（スラッシュコマンドの引数でも使う）
pub(super) struct SearchNamesFor<'a> {
    pub app: &'a App,
    pub guild: Option<&'a str>,
}

impl SearchNames for SearchNamesFor<'_> {
    fn user_id(&self, name: &str) -> Option<String> {
        let name = name.trim_start_matches('@');
        if let Some(id) = name
            .strip_prefix("<@")
            .and_then(|s| s.strip_suffix('>'))
            .map(|s| s.trim_start_matches('!'))
        {
            return Some(id.to_string());
        }
        if !name.is_empty() && name.bytes().all(|b| b.is_ascii_digit()) {
            return Some(name.to_string());
        }
        let hit = |u: &User| {
            u.username.eq_ignore_ascii_case(name)
                || u.global_name.as_deref().is_some_and(|g| g == name)
        };
        if let Some(me) = self.app.me.as_ref().filter(|u| hit(u)) {
            return Some(me.id.clone());
        }
        if let Some(g) = self.guild {
            for m in self.app.members.get(g).into_iter().flatten() {
                if let Some(u) = &m.user {
                    if hit(u) || m.nick.as_deref() == Some(name) {
                        return Some(u.id.clone());
                    }
                }
            }
        }
        self.app
            .dms
            .iter()
            .flat_map(|c| c.recipients.iter())
            .find(|u| hit(u))
            .map(|u| u.id.clone())
    }

    fn channel_id(&self, name: &str) -> Option<String> {
        let name = name.trim_start_matches('#');
        let g = self.guild?;
        if let Some(id) = name.strip_prefix("<#").and_then(|s| s.strip_suffix('>')) {
            return Some(id.to_string());
        }
        self.app
            .channels
            .get(g)
            .into_iter()
            .flatten()
            .chain(self.app.threads.get(g).into_iter().flatten())
            .find(|c| {
                c.kind != Channel::CATEGORY
                    && c.name
                        .as_deref()
                        .is_some_and(|n| n.eq_ignore_ascii_case(name))
            })
            .map(|c| c.id.clone())
    }
}

impl App {
    fn current_search_scope(&self) -> Option<SearchScope> {
        match (&self.sel_guild, &self.sel_channel) {
            (Some(g), _) => Some(SearchScope::Guild(g.clone())),
            (None, Some(c)) => Some(SearchScope::Channel(c.clone())),
            (None, None) => None,
        }
    }

    /// 検索の枠を出す・閉じる
    pub fn toggle_search(&mut self) {
        if self.side == SidePanel::Search {
            self.close_side();
        } else {
            self.close_side();
            self.side = SidePanel::Search;
            if self.search.is_none() {
                self.search = Some(SearchView::default());
            }
        }
    }

    /// 検索欄が変わった: 手を止めたら検索する
    pub fn search_typed(&mut self) {
        let pending = !self.search_query.trim().is_empty();
        if let Some(s) = self.search.as_mut() {
            *s = SearchView {
                rev: s.rev + 1,
                loading: pending,
                ..Default::default()
            };
        }
        self.search_due = pending.then(|| std::time::Instant::now() + SEARCH_DEBOUNCE);
        if pending {
            self.repaint_after(SEARCH_DEBOUNCE);
        }
    }

    /// 毎回の描画で呼ぶ: 待ち時間が過ぎた検索・やり直しを送る
    pub fn tick_search(&mut self) {
        let now = std::time::Instant::now();
        if self.search_due.is_some_and(|t| now >= t) {
            self.search_due = None;
            self.run_search();
        }
        let scope = self.current_search_scope();
        let scope_changed = self.side == SidePanel::Search
            && !self.search_query.trim().is_empty()
            && self.search.as_ref().is_some_and(|s| {
                (s.scope.is_some() || (!s.loading && s.error.is_some())) && s.scope != scope
            });
        if scope_changed {
            self.run_search();
            return;
        }
        let Some(s) = self.search.as_mut() else {
            return;
        };
        if s.retry_at.is_some_and(|t| now >= t) {
            s.retry_at = None;
            self.send_search();
        }
    }

    /// 今の検索欄の中身で、今いる場所を探す（先頭のページから）
    pub fn run_search(&mut self) {
        self.search_due = None;
        let q = self.search_query.trim().to_string();
        let scope = match self.current_search_scope() {
            Some(scope) => scope,
            None => {
                if let Some(s) = self.search.as_mut() {
                    *s = SearchView {
                        rev: s.rev + 1,
                        error: (!q.is_empty()).then(|| {
                            tr!(
                                "検索するチャンネルを選択してください",
                                "Select a channel to search",
                                "请选择要搜索的频道"
                            )
                            .into()
                        }),
                        ..Default::default()
                    };
                }
                return;
            }
        };
        let parsed = parse_search(
            &q,
            matches!(scope, SearchScope::Guild(_)),
            &SearchNamesFor {
                app: self,
                guild: self.sel_guild.as_deref(),
            },
        );
        let s = self.search.get_or_insert_with(SearchView::default);
        *s = SearchView {
            rev: s.rev + 1,
            scope: Some(scope),
            ..Default::default()
        };
        match parsed {
            _ if q.is_empty() => {} // 初期化だけ行い、検索は送らない
            Err(e) => {
                s.loading = false;
                s.error = Some(e);
            }
            Ok(params) => {
                s.params = params;
                s.offset = 0;
                s.searched = true;
                self.send_search();
            }
        }
    }

    /// ページを移る
    pub fn search_page(&mut self, forward: bool) {
        let Some(s) = self
            .search
            .as_mut()
            .filter(|s| s.scope.is_some() && !s.loading)
        else {
            return;
        };
        let next = if forward {
            s.offset + SEARCH_PAGE
        } else {
            s.offset.saturating_sub(SEARCH_PAGE)
        };
        if next == s.offset || next > SEARCH_MAX_OFFSET || (forward && next >= s.total) {
            return;
        }
        s.offset = next;
        s.retries = 0;
        self.send_search();
    }

    /// 読み込めなかった検索を、同じ条件・ページからやり直す。
    pub fn retry_search(&mut self) {
        let scope = self.current_search_scope();
        let Some(s) = self.search.as_mut().filter(|s| {
            s.searched
                && !s.loading
                && (s.error.is_some() || s.partial)
                && s.scope == scope
                && scope.is_some()
        }) else {
            return;
        };
        s.retries = 0;
        self.send_search();
    }

    fn send_search(&mut self) {
        let Some(s) = self.search.as_mut() else {
            return;
        };
        let Some(scope) = s.scope.clone() else { return };
        s.loading = true;
        s.error = None;
        s.hits.clear();
        s.rev += 1;
        let mut query = s.params.clone();
        if s.offset > 0 {
            query.push(("offset".into(), s.offset.to_string()));
        }
        query.push(("limit".into(), SEARCH_PAGE.to_string()));
        let call = match scope {
            SearchScope::Guild(guild_id) => ApiCall::SearchGuild { guild_id, query },
            SearchScope::Channel(channel_id) => ApiCall::SearchChannel { channel_id, query },
        };
        self.next_api_id += 1;
        let id = self.next_api_id;
        let queued = self
            .backend
            .as_ref()
            .is_some_and(|b| b.cmd.send(Cmd::Api { id, call }).is_ok());
        if let Some(s) = self.search.as_mut() {
            if queued {
                s.request = id;
            } else {
                s.request = 0;
                s.loading = false;
                s.error = Some(
                    tr!(
                        "検索できませんでした。接続を確認して再試行してください",
                        "Couldn't search. Check your connection and try again",
                        "无法搜索。请检查连接后重试"
                    )
                    .into(),
                );
            }
        }
    }

    pub(super) fn on_search_result(&mut self, id: u64, result: Result<serde_json::Value, String>) {
        let scope = self.current_search_scope();
        let Some(s) = self
            .search
            .as_mut()
            .filter(|s| s.request == id && s.loading && s.scope == scope)
        else {
            return;
        };
        s.rev += 1;
        match result {
            Ok(v) => {
                match parse_results(&v) {
                    Ok((messages, total, partial)) => {
                        s.loading = false;
                        s.total = total;
                        s.partial = partial;
                        let hits: Vec<SearchHit> = messages
                            .into_iter()
                            .map(|m| SearchHit {
                                place: String::new(),
                                message: m,
                            })
                            .collect();
                        s.hits = hits;
                        self.name_search_hits(&v);
                    }
                    Err(Some(wait)) if s.retries < SEARCH_RETRIES => {
                        // 索引の準備中: 言われた時間だけ待ってやり直す
                        s.retries += 1;
                        let wait = std::time::Duration::from_secs_f64(wait.clamp(1.0, 30.0));
                        s.retry_at = Some(std::time::Instant::now() + wait);
                        self.repaint_after(wait);
                    }
                    Err(Some(_)) => {
                        s.loading = false;
                        s.error = Some(tr!("検索の準備に時間がかかっています。しばらくしてからもう一度お試しください", "Search is taking a while to prepare. Try again in a moment", "搜索准备耗时较长。请稍后再试").into());
                    }
                    Err(None) => {
                        s.loading = false;
                        s.error = Some(
                            tr!(
                                "検索結果を読めませんでした",
                                "Couldn't read the search results",
                                "无法读取搜索结果"
                            )
                            .into(),
                        );
                    }
                }
            }
            Err(e) => {
                eprintln!("検索に失敗しました: {e}");
                s.loading = false;
                s.error = Some(api_error_text(
                    &e,
                    tr!("検索できませんでした", "Couldn't search", "无法搜索"),
                ));
            }
        }
    }

    /// 結果に「#チャンネル名」を付ける（スレッドは結果に入っている名前を使う）
    fn name_search_hits(&mut self, v: &serde_json::Value) {
        let threads: HashMap<&str, &str> = v["threads"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|t| Some((t["id"].as_str()?, t["name"].as_str()?)))
            .collect();
        let Some(s) = self.search.as_ref() else {
            return;
        };
        let guild = match &s.scope {
            Some(SearchScope::Guild(g)) => Some(g.clone()),
            _ => None,
        };
        let names: Vec<String> = s
            .hits
            .iter()
            .map(|h| {
                let cid = h.message.channel_id.as_str();
                match &guild {
                    Some(g) => threads
                        .get(cid)
                        .map(|n| n.to_string())
                        .or_else(|| self.find_guild_channel(g, cid).and_then(|c| c.name.clone()))
                        .map(|n| format!("#{n}"))
                        .unwrap_or_default(),
                    None => self
                        .dms
                        .iter()
                        .find(|c| c.id == cid)
                        .map(|c| c.dm_label())
                        .unwrap_or_default(),
                }
            })
            .collect();
        if let Some(s) = self.search.as_mut() {
            for (h, n) in s.hits.iter_mut().zip(names) {
                h.place = n;
            }
        }
    }

    /// 検索結果の場所（サーバー、DM なら None）
    pub fn search_guild(&self) -> Option<String> {
        match self.search.as_ref()?.scope.as_ref()? {
            SearchScope::Guild(g) => Some(g.clone()),
            SearchScope::Channel(_) => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Names;
    impl SearchNames for Names {
        fn user_id(&self, name: &str) -> Option<String> {
            (name == "sakura").then(|| "2".into())
        }
        fn channel_id(&self, name: &str) -> Option<String> {
            (name == "#一般" || name == "一般").then(|| "10".into())
        }
    }

    #[test]
    fn filters_become_discord_params() {
        let p = parse_search(
            "こんにちは from:sakura in:#一般 has:画像 pinned:true",
            true,
            &Names,
        )
        .unwrap();
        assert_eq!(p[0], ("content".into(), "こんにちは".into()));
        assert!(p.contains(&("author_id".into(), "2".into())));
        assert!(p.contains(&("channel_id".into(), "10".into())));
        assert!(p.contains(&("has".into(), "image".into())));
        assert!(p.contains(&("pinned".into(), "true".into())));
        // URL などの知らない「xx:」は本文
        let p = parse_search("https://example.com", true, &Names).unwrap();
        assert_eq!(
            p,
            vec![("content".to_string(), "https://example.com".to_string())]
        );
    }

    #[test]
    fn bad_filters_explain_why() {
        assert!(parse_search("from:nobody", true, &Names)
            .unwrap_err()
            .contains("nobody"));
        assert!(parse_search("in:一般", false, &Names)
            .unwrap_err()
            .contains("DM"));
        assert!(parse_search("has:cat", true, &Names).is_err());
        assert!(parse_search("before:昨日", true, &Names).is_err());
    }

    #[test]
    fn dates_become_id_bounds() {
        let p = parse_search("during:2026-01-31", true, &Names).unwrap();
        let min: u64 = p
            .iter()
            .find(|(k, _)| k == "min_id")
            .unwrap()
            .1
            .parse()
            .unwrap();
        let max: u64 = p
            .iter()
            .find(|(k, _)| k == "max_id")
            .unwrap()
            .1
            .parse()
            .unwrap();
        // ちょうど1日分
        assert_eq!((max >> 22) - (min >> 22), 86_400_000);
        let before: u64 = parse_search("before:2026-01-31", true, &Names).unwrap()[0]
            .1
            .parse()
            .unwrap();
        let after: u64 = parse_search("after:2026-01-31", true, &Names).unwrap()[0]
            .1
            .parse()
            .unwrap();
        assert_eq!(before, min);
        assert_eq!(after, max);
        assert_eq!(
            snowflake_at(chrono::DateTime::from_timestamp_millis(1_420_070_400_000 + 1).unwrap()),
            1 << 22
        );
    }

    #[test]
    fn results_and_index_wait_are_read() {
        let m = serde_json::json!({ "id": "5", "channel_id": "c", "author": { "id": "u", "username": "u" }, "timestamp": "2026-01-01T00:00:00+00:00" });
        let (list, total, partial) =
            parse_results(&serde_json::json!({ "total_results": 120, "messages": [[m]] })).unwrap();
        assert_eq!((list.len(), total), (1, 120));
        assert!(!partial);
        assert_eq!(
            parse_results(&serde_json::json!({ "code": 110000, "retry_after": 3 })).unwrap_err(),
            Some(3.0)
        );
        assert_eq!(parse_results(&serde_json::json!({})).unwrap_err(), None);
    }

    #[test]
    fn search_summary_does_not_claim_empty_when_a_message_is_present() {
        let (mut app, tx, mut rx) = harness();
        app.toggle_search();
        app.search_query = "猫".into();
        app.run_search();
        let Cmd::Api { id, .. } = rx.try_recv().unwrap() else {
            panic!()
        };
        tx.send(reply(id, 0)).unwrap();
        app.poll();
        let s = app.search.as_ref().unwrap();
        assert_eq!(s.hits.len(), 1);
        assert!(!s.result_label().contains("ありません"));
        assert!(s.result_label().contains("表示中: 1 件"));
    }

    #[test]
    fn empty_page_with_nonzero_total_does_not_show_an_inverted_range() {
        let (mut app, tx, mut rx) = harness();
        app.toggle_search();
        app.search_query = "猫".into();
        app.run_search();
        let Cmd::Api { id, call } = rx.try_recv().unwrap() else {
            panic!()
        };
        tx.send(Event::Api {
            id,
            call,
            result: Ok(serde_json::json!({"total_results":60,"messages":[]})),
        })
        .unwrap();
        app.poll();
        let label = app.search.as_ref().unwrap().result_label();
        assert!(!label.contains("1–0"));
        assert!(label.contains("表示できる"));
    }

    #[test]
    fn result_count_shrinking_between_pages_does_not_show_a_contradictory_range() {
        let (mut app, tx, mut rx) = harness();
        app.toggle_search();
        app.search_query = "猫".into();
        app.run_search();
        let Cmd::Api { id, .. } = rx.try_recv().unwrap() else {
            panic!()
        };
        tx.send(reply(id, 60)).unwrap();
        app.poll();
        app.search_page(true);
        let Cmd::Api { id, .. } = rx.try_recv().unwrap() else {
            panic!()
        };
        tx.send(reply(id, 1)).unwrap();
        app.poll();
        let label = app.search.as_ref().unwrap().result_label();
        assert!(!label.contains("26–26 件目（全 1 件）"));
        assert!(label.contains("全件数は確認できません"));
    }

    #[test]
    fn malformed_search_items_do_not_look_like_a_complete_page() {
        let (mut app, tx, mut rx) = harness();
        app.toggle_search();
        app.search_query = "猫".into();
        app.run_search();
        let Cmd::Api { id, call } = rx.try_recv().unwrap() else {
            panic!()
        };
        let valid = serde_json::json!({ "id":"5", "channel_id":"c", "author":{"id":"u","username":"u"}, "timestamp":"2026-01-01T00:00:00+00:00" });
        tx.send(Event::Api {
            id,
            call,
            result: Ok(
                serde_json::json!({"total_results":2,"messages":[[{"id":"broken"}],[valid]]}),
            ),
        })
        .unwrap();
        app.poll();
        let s = app.search.as_ref().unwrap();
        assert_eq!(s.hits.len(), 1);
        assert!(
            s.result_label().contains("一部"),
            "表示できなかった検索結果を知らせる"
        );
        app.retry_search();
        let Cmd::Api { id, .. } = rx.try_recv().unwrap() else {
            panic!()
        };
        tx.send(reply(id, 1)).unwrap();
        app.poll();
        assert!(!app.search.as_ref().unwrap().result_label().contains("一部"));
    }

    #[test]
    fn entirely_malformed_search_page_is_a_retryable_error() {
        let (mut app, tx, mut rx) = harness();
        app.toggle_search();
        app.search_query = "猫".into();
        app.run_search();
        let Cmd::Api { id, call } = rx.try_recv().unwrap() else {
            panic!()
        };
        tx.send(Event::Api {
            id,
            call,
            result: Ok(serde_json::json!({"total_results":1,"messages":[[{"id":"broken"}]]})),
        })
        .unwrap();
        app.poll();
        let s = app.search.as_ref().unwrap();
        assert!(!s.loading && s.error.is_some() && s.hits.is_empty());
        app.retry_search();
        assert!(matches!(
            rx.try_recv(),
            Ok(Cmd::Api {
                call: ApiCall::SearchGuild { .. },
                ..
            })
        ));
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
        app.sel_guild = Some("g".into());
        app.sel_channel = Some("c".into());
        (app, tx, rx)
    }

    fn reply(id: u64, total: u32) -> Event {
        let m = serde_json::json!({ "id": "5", "channel_id": "c", "author": { "id": "u", "username": "u" }, "timestamp": "2026-01-01T00:00:00+00:00" });
        Event::Api {
            id,
            call: ApiCall::SearchGuild {
                guild_id: "g".into(),
                query: vec![],
            },
            result: Ok(serde_json::json!({ "total_results": total, "messages": [[m]] })),
        }
    }

    #[test]
    fn stale_results_are_dropped_and_pages_move() {
        let (mut app, tx, mut rx) = harness();
        app.toggle_search();
        app.search_query = "猫".into();
        app.run_search();
        let Cmd::Api { id: first, call } = rx.try_recv().unwrap() else {
            panic!()
        };
        assert!(
            matches!(&call, ApiCall::SearchGuild { guild_id, query } if guild_id == "g" && query.contains(&("content".into(), "猫".into())))
        );
        // 返事を待つ間に打ち直した: 前の結果は捨てる
        app.search_query = "犬".into();
        app.run_search();
        let Cmd::Api { id: second, .. } = rx.try_recv().unwrap() else {
            panic!()
        };
        assert_ne!(first, second);
        tx.send(reply(first, 1)).unwrap();
        app.poll();
        assert!(app.search.as_ref().unwrap().loading);
        tx.send(reply(second, 60)).unwrap();
        app.poll();
        let s = app.search.as_ref().unwrap();
        assert!(!s.loading && s.hits.len() == 1 && s.total == 60);
        assert_eq!(s.result_label(), "1–1 件目（全 60 件）");
        // 次のページは offset=25
        app.search_page(true);
        let Cmd::Api {
            call: ApiCall::SearchGuild { query, .. },
            ..
        } = rx.try_recv().unwrap()
        else {
            panic!()
        };
        assert!(query.contains(&("offset".into(), "25".into())));
        assert!(
            app.search.as_ref().unwrap().hits.is_empty(),
            "ページの取得中は前のページを表示しない"
        );
    }

    #[test]
    fn typing_a_new_query_immediately_discards_the_previous_response() {
        let (mut app, tx, mut rx) = harness();
        app.toggle_search();
        app.search_query = "猫".into();
        app.run_search();
        let Cmd::Api { id: old, .. } = rx.try_recv().unwrap() else {
            panic!()
        };
        app.search_query = "犬".into();
        app.search_typed();
        tx.send(reply(old, 1)).unwrap();
        app.poll();
        assert!(
            app.search.as_ref().unwrap().hits.is_empty(),
            "旧検索の結果を新しい検索語の下に表示しない"
        );
    }

    #[test]
    fn invalid_new_query_does_not_leave_old_results_visible() {
        let (mut app, tx, mut rx) = harness();
        app.toggle_search();
        app.search_query = "猫".into();
        app.run_search();
        let Cmd::Api { id, .. } = rx.try_recv().unwrap() else {
            panic!()
        };
        tx.send(reply(id, 1)).unwrap();
        app.poll();
        assert_eq!(app.search.as_ref().unwrap().hits.len(), 1);
        app.search_query = "has:cat".into();
        app.run_search();
        let s = app.search.as_ref().unwrap();
        assert!(s.error.is_some());
        assert!(s.hits.is_empty(), "入力エラーの下に以前の結果を残さない");
    }

    #[test]
    fn clearing_the_query_cancels_pending_results_and_loading() {
        let (mut app, tx, mut rx) = harness();
        app.toggle_search();
        app.search_query = "猫".into();
        app.run_search();
        let Cmd::Api { id, .. } = rx.try_recv().unwrap() else {
            panic!()
        };
        app.search_query.clear();
        app.search_typed();
        tx.send(reply(id, 1)).unwrap();
        app.poll();
        let s = app.search.as_ref().unwrap();
        assert!(!s.loading && s.hits.is_empty() && !s.searched);
        assert!(app.search_due.is_none());
    }

    #[test]
    fn leaving_the_search_scope_during_debounce_stops_loading() {
        let (mut app, _tx, mut rx) = harness();
        app.toggle_search();
        app.search_query = "猫".into();
        app.search_typed();
        app.sel_guild = None;
        app.sel_channel = None;
        app.search_due = Some(std::time::Instant::now());
        app.tick_search();
        let s = app.search.as_ref().unwrap();
        assert!(!s.loading && s.error.is_some());
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn switching_guild_discards_inflight_results_and_searches_the_new_scope() {
        let (mut app, tx, mut rx) = harness();
        app.toggle_search();
        app.search_query = "猫".into();
        app.run_search();
        let Cmd::Api { id: old, .. } = rx.try_recv().unwrap() else {
            panic!()
        };
        app.sel_guild = Some("別サーバー".into());
        tx.send(reply(old, 1)).unwrap();
        app.poll();
        assert!(
            app.search.as_ref().unwrap().hits.is_empty(),
            "別サーバーの見出しの下に古い結果を出さない"
        );
        app.tick_search();
        assert!(
            matches!(rx.try_recv(), Ok(Cmd::Api { call: ApiCall::SearchGuild { guild_id, .. }, .. }) if guild_id == "別サーバー")
        );
    }

    #[test]
    fn switching_scope_replaces_already_visible_results() {
        let (mut app, tx, mut rx) = harness();
        app.toggle_search();
        app.search_query = "猫".into();
        app.run_search();
        let Cmd::Api { id, .. } = rx.try_recv().unwrap() else {
            panic!()
        };
        tx.send(reply(id, 1)).unwrap();
        app.poll();
        assert_eq!(app.search.as_ref().unwrap().hits.len(), 1);
        app.sel_guild = None;
        app.sel_channel = Some("別DM".into());
        app.tick_search();
        assert!(app.search.as_ref().unwrap().hits.is_empty());
        assert!(
            matches!(rx.try_recv(), Ok(Cmd::Api { call: ApiCall::SearchChannel { channel_id, .. }, .. }) if channel_id == "別DM")
        );
    }

    #[test]
    fn switching_channels_within_one_guild_keeps_server_results() {
        let (mut app, tx, mut rx) = harness();
        app.toggle_search();
        app.search_query = "猫".into();
        app.run_search();
        let Cmd::Api { id, .. } = rx.try_recv().unwrap() else {
            panic!()
        };
        tx.send(reply(id, 1)).unwrap();
        app.poll();
        app.sel_channel = Some("別チャンネル".into());
        app.tick_search();
        assert_eq!(app.search.as_ref().unwrap().hits.len(), 1);
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn reopening_search_after_scope_change_refreshes_without_searching_in_the_background() {
        let (mut app, tx, mut rx) = harness();
        app.toggle_search();
        app.search_query = "猫".into();
        app.run_search();
        let Cmd::Api { id, .. } = rx.try_recv().unwrap() else {
            panic!()
        };
        tx.send(reply(id, 1)).unwrap();
        app.poll();
        app.toggle_search();
        app.sel_guild = Some("別サーバー".into());
        app.tick_search();
        assert!(rx.try_recv().is_err());
        app.toggle_search();
        app.tick_search();
        assert!(app.search.as_ref().unwrap().hits.is_empty());
        assert!(
            matches!(rx.try_recv(), Ok(Cmd::Api { call: ApiCall::SearchGuild { guild_id, .. }, .. }) if guild_id == "別サーバー")
        );
    }

    #[test]
    fn failed_search_page_retries_the_same_query_without_double_submit() {
        let (mut app, tx, mut rx) = harness();
        app.toggle_search();
        app.search_query = "猫".into();
        app.run_search();
        let Cmd::Api { id, .. } = rx.try_recv().unwrap() else {
            panic!()
        };
        tx.send(reply(id, 60)).unwrap();
        app.poll();
        app.search_page(true);
        let Cmd::Api { id, call } = rx.try_recv().unwrap() else {
            panic!()
        };
        tx.send(Event::Api {
            id,
            call,
            result: Err("network unavailable".into()),
        })
        .unwrap();
        app.poll();
        assert!(app.search.as_ref().unwrap().error.is_some());
        app.retry_search();
        let Cmd::Api {
            id,
            call: ApiCall::SearchGuild { query, .. },
        } = rx.try_recv().unwrap()
        else {
            panic!()
        };
        assert!(query.contains(&("content".into(), "猫".into())));
        assert!(query.contains(&("offset".into(), "25".into())));
        app.retry_search();
        assert!(rx.try_recv().is_err());
        tx.send(reply(id, 60)).unwrap();
        app.poll();
        let s = app.search.as_ref().unwrap();
        assert!(!s.loading && s.error.is_none() && s.offset == 25 && s.hits.len() == 1);
    }

    #[test]
    fn retry_ignores_invalid_input_and_an_outdated_scope() {
        let (mut app, tx, mut rx) = harness();
        app.toggle_search();
        app.search_query = "has:cat".into();
        app.run_search();
        app.retry_search();
        assert!(rx.try_recv().is_err());
        app.search_query = "猫".into();
        app.run_search();
        let Cmd::Api { id, call } = rx.try_recv().unwrap() else {
            panic!()
        };
        tx.send(Event::Api {
            id,
            call,
            result: Err("network unavailable".into()),
        })
        .unwrap();
        app.poll();
        app.sel_guild = Some("別サーバー".into());
        app.retry_search();
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn search_without_a_backend_reports_an_error_instead_of_loading_forever() {
        let (mut app, _tx, mut rx) = harness();
        let backend = app.backend.take();
        app.toggle_search();
        app.search_query = "猫".into();
        app.run_search();
        let s = app.search.as_ref().unwrap();
        assert!(!s.loading && s.error.is_some());
        app.backend = backend;
        app.retry_search();
        assert!(matches!(
            rx.try_recv(),
            Ok(Cmd::Api {
                call: ApiCall::SearchGuild { .. },
                ..
            })
        ));
        assert!(app.search.as_ref().unwrap().loading);
    }

    #[test]
    fn search_with_a_closed_backend_reports_an_error() {
        let (mut app, _tx, rx) = harness();
        drop(rx);
        app.toggle_search();
        app.search_query = "猫".into();
        app.run_search();
        let s = app.search.as_ref().unwrap();
        assert!(!s.loading && s.error.is_some());
    }

    #[test]
    fn index_not_ready_is_retried_later() {
        let (mut app, tx, mut rx) = harness();
        app.toggle_search();
        app.search_query = "猫".into();
        app.run_search();
        let Cmd::Api { id, .. } = rx.try_recv().unwrap() else {
            panic!()
        };
        tx.send(Event::Api {
            id,
            call: ApiCall::SearchGuild {
                guild_id: "g".into(),
                query: vec![],
            },
            result: Ok(serde_json::json!({ "code": 110000, "retry_after": 0 })),
        })
        .unwrap();
        app.poll();
        let s = app.search.as_mut().unwrap();
        assert!(s.retry_at.is_some() && s.loading);
        s.retry_at = Some(std::time::Instant::now());
        app.tick_search();
        assert!(matches!(
            rx.try_recv().unwrap(),
            Cmd::Api {
                call: ApiCall::SearchGuild { .. },
                ..
            }
        ));
    }

    #[test]
    fn typing_waits_before_searching() {
        let (mut app, _tx, mut rx) = harness();
        app.toggle_search();
        app.search_query = "猫".into();
        app.search_typed();
        app.tick_search();
        assert!(rx.try_recv().is_err());
        app.search_due = Some(std::time::Instant::now());
        app.tick_search();
        assert!(rx.try_recv().is_ok());
    }
}
