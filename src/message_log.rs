//! 削除・編集されたメッセージの記録。同梱プラグイン MessageLogger が有効なときだけ使う。
//! 記録は設定フォルダの message-log.json に残し、再起動後も削除済み・編集前の本文を表示する
use crate::model::{LogMark, Message, PastEdit};
use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::time::{Duration, Instant};

/// 削除・編集に備えて覚えておく、最近受け取ったメッセージの数（開いていないチャンネルの分も含む）
const CACHE_MAX: usize = 10_000;
/// 残す記録の数（削除・編集それぞれ）
const LOG_MAX: usize = 3_000;
/// 1つのメッセージに残す編集前の本文の数
const EDITS_MAX: usize = 50;
/// 変更から保存までの間隔（続けて届いても書き込みは1回にまとめる）
const SAVE_DELAY: Duration = Duration::from_secs(3);

#[derive(Clone, Serialize, Deserialize)]
pub struct Deleted {
    pub message: Message,
    /// 削除を受け取った時刻（RFC 3339）
    pub at: String,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Edited {
    /// 最新の本文のメッセージ
    pub message: Message,
    /// 編集前の本文（古い順）
    pub history: Vec<PastEdit>,
}

#[derive(Default, Serialize, Deserialize)]
struct Stored {
    #[serde(default)]
    deleted: Vec<Deleted>,
    #[serde(default)]
    edited: Vec<Edited>,
}

/// 記録するかどうかの条件（プラグインのページで切り替える）
#[derive(Clone, Copy, Default)]
pub struct Options {
    pub ignore_self: bool,
    pub ignore_bots: bool,
}

#[derive(Default)]
pub struct MessageLog {
    pub deleted: HashMap<String, Deleted>,
    pub edited: HashMap<String, Edited>,
    cache: HashMap<String, Message>,
    order: VecDeque<String>,
    loaded: bool,
    dirty_since: Option<Instant>,
    /// 記録が変わるたびに増える（一覧画面の描き直し用）
    pub revision: u64,
    /// 記録を置くファイル（アカウントのフォルダの message-log.json）。空なら保存しない
    pub path: PathBuf,
}

pub fn path_for(profile: Option<&str>) -> PathBuf {
    crate::profile::account_dir(profile).join("message-log.json")
}

fn now() -> String {
    chrono::Utc::now().to_rfc3339()
}

/// Discord の ID（数字の文字列）を数として比べる
pub fn snowflake_cmp(a: &str, b: &str) -> Ordering {
    (a.len(), a).cmp(&(b.len(), b))
}

impl MessageLog {
    /// 初めて使うときにファイルから読む
    pub fn ensure_loaded(&mut self) {
        if self.loaded {
            return;
        }
        self.loaded = true;
        let Some(stored) = std::fs::read(&self.path).ok().and_then(|b| serde_json::from_slice::<Stored>(&b).ok()) else { return };
        self.deleted = stored.deleted.into_iter().map(|d| (d.message.id.clone(), d)).collect();
        self.edited = stored.edited.into_iter().map(|e| (e.message.id.clone(), e)).collect();
        self.revision = self.revision.wrapping_add(1);
    }

    /// 受け取ったメッセージを覚える（後で削除・編集されたときに本文が分かるように）
    pub fn remember(&mut self, m: &Message) {
        if let Some(old) = self.cache.get_mut(&m.id) {
            *old = m.clone();
            old.log = None;
            return;
        }
        let mut copy = m.clone();
        copy.log = None;
        self.cache.insert(m.id.clone(), copy);
        self.order.push_back(m.id.clone());
        while self.order.len() > CACHE_MAX {
            if let Some(id) = self.order.pop_front() {
                self.cache.remove(&id);
            }
        }
    }

    pub fn cached(&self, id: &str) -> Option<&Message> {
        self.cache.get(id)
    }

    pub fn wanted(m: &Message, me: Option<&str>, opt: Options) -> bool {
        !(opt.ignore_self && me == Some(m.author.id.as_str())) && !(opt.ignore_bots && m.author.bot) && !m.is_call()
    }

    /// 削除を記録する
    pub fn on_delete(&mut self, m: &Message) {
        self.ensure_loaded();
        let mut message = m.clone();
        message.log = None;
        self.deleted.insert(m.id.clone(), Deleted { message, at: now() });
        self.cache.remove(&m.id);
        trim(&mut self.deleted, |d| d.at.clone());
        self.changed();
    }

    /// 編集を記録する（old は編集前のメッセージ）
    pub fn on_edit(&mut self, old: &Message, new_content: &str) {
        self.ensure_loaded();
        let entry = self.edited.entry(old.id.clone()).or_insert_with(|| {
            let mut message = old.clone();
            message.log = None;
            Edited { message, history: Vec::new() }
        });
        entry.history.push(PastEdit { content: old.content.clone(), at: now() });
        if entry.history.len() > EDITS_MAX {
            entry.history.remove(0);
        }
        entry.message.content = new_content.to_string();
        if let Some(c) = self.cache.get_mut(&old.id) {
            c.content = new_content.to_string();
        }
        trim(&mut self.edited, |e| e.history.last().map(|h| h.at.clone()).unwrap_or_default());
        self.changed();
    }

    /// このメッセージに付ける印（記録が無ければ None）
    pub fn mark_for(&self, id: &str) -> Option<Box<LogMark>> {
        let deleted_at = self.deleted.get(id).map(|d| d.at.clone());
        let edits = self.edited.get(id).map(|e| e.history.clone()).unwrap_or_default();
        (deleted_at.is_some() || !edits.is_empty()).then(|| Box::new(LogMark { deleted_at, edits }))
    }

    /// 読み込んだ一覧に印を付け、記録にある削除済みメッセージを元の位置へ戻す。
    /// from_start が false なら、一覧の先頭より新しいものだけ戻す（それより前はまだ読んでいない）
    pub fn restore(&mut self, channel: &str, list: &mut Vec<Message>, from_start: bool) {
        self.ensure_loaded();
        for m in list.iter_mut() {
            m.log = self.mark_for(&m.id);
        }
        let first = list.first().map(|m| m.id.clone());
        let have: HashSet<&str> = list.iter().map(|m| m.id.as_str()).collect();
        let extra: Vec<Message> = self
            .deleted
            .values()
            .filter(|d| d.message.channel_id == channel && !have.contains(d.message.id.as_str()))
            .filter(|d| from_start || first.as_deref().is_some_and(|f| snowflake_cmp(&d.message.id, f).is_ge()))
            .map(|d| {
                let mut m = d.message.clone();
                m.log = self.mark_for(&m.id);
                m
            })
            .collect();
        if extra.is_empty() {
            return;
        }
        list.extend(extra);
        list.sort_by(|a, b| snowflake_cmp(&a.id, &b.id));
    }

    pub fn clear(&mut self) {
        self.ensure_loaded();
        self.deleted.clear();
        self.edited.clear();
        self.changed();
    }

    fn changed(&mut self) {
        self.revision = self.revision.wrapping_add(1);
        self.dirty_since.get_or_insert_with(Instant::now);
    }

    /// 変更から少し経っていれば保存する。次に保存すべきまでの時間を返す
    pub fn save_if_due(&mut self) -> Option<Duration> {
        let since = self.dirty_since?;
        let elapsed = since.elapsed();
        if elapsed < SAVE_DELAY {
            return Some(SAVE_DELAY - elapsed);
        }
        self.flush();
        None
    }

    pub fn flush(&mut self) {
        if self.dirty_since.take().is_none() {
            return;
        }
        let stored = Stored { deleted: self.deleted.values().cloned().collect(), edited: self.edited.values().cloned().collect() };
        let Ok(bytes) = serde_json::to_vec(&stored) else { return };
        if self.path.as_os_str().is_empty() {
            return;
        }
        let p = self.path.clone();
        if let Some(dir) = p.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        // 書きかけで落ちても前の記録を壊さない
        let tmp = p.with_extension("json.tmp");
        if let Err(e) = std::fs::write(&tmp, bytes).and_then(|_| std::fs::rename(&tmp, &p)) {
            eprintln!("メッセージの記録を保存できませんでした: {e}");
        }
    }
}

impl Drop for MessageLog {
    fn drop(&mut self) {
        self.flush();
    }
}

/// 上限を超えたら古い記録から捨てる
fn trim<T>(map: &mut HashMap<String, T>, at: impl Fn(&T) -> String) {
    if map.len() <= LOG_MAX {
        return;
    }
    let mut v: Vec<(String, String)> = map.iter().map(|(k, x)| (at(x), k.clone())).collect();
    v.sort();
    for (_, k) in v.into_iter().take(map.len() - LOG_MAX) {
        map.remove(&k);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn msg(id: &str, content: &str) -> Message {
        serde_json::from_value(serde_json::json!({
            "id": id, "channel_id": "c", "author": { "id": "u", "username": "u" },
            "content": content, "timestamp": "2026-01-01T00:00:00+00:00",
        }))
        .unwrap()
    }

    fn log() -> MessageLog {
        let mut l = MessageLog::default();
        l.loaded = true;
        l
    }

    #[test]
    fn deleted_message_is_restored_in_order() {
        let mut l = log();
        l.on_delete(&msg("15", "消えた"));
        l.on_delete(&msg("5", "まだ読んでいない所"));
        let mut list = vec![msg("10", "a"), msg("20", "b")];
        l.restore("c", &mut list, false);
        let ids: Vec<_> = list.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(ids, ["10", "15", "20"]);
        assert!(list[1].log.as_ref().unwrap().deleted_at.is_some());
        l.restore("c", &mut list, true);
        assert_eq!(list.len(), 4);
        assert_eq!(list[0].id, "5");
        l.dirty_since = None;
    }

    #[test]
    fn edits_are_kept_oldest_first() {
        let mut l = log();
        let m = msg("1", "最初");
        l.on_edit(&m, "二回目");
        let mut m2 = m.clone();
        m2.content = "二回目".into();
        l.on_edit(&m2, "三回目");
        let mark = l.mark_for("1").unwrap();
        let old: Vec<_> = mark.edits.iter().map(|e| e.content.as_str()).collect();
        assert_eq!(old, ["最初", "二回目"]);
        assert_eq!(l.edited["1"].message.content, "三回目");
        l.dirty_since = None;
    }

    #[test]
    fn snowflakes_compare_numerically() {
        assert_eq!(snowflake_cmp("9", "10"), Ordering::Less);
        assert_eq!(snowflake_cmp("100", "99"), Ordering::Greater);
    }
}
