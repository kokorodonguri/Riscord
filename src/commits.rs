//! GitHub の main に、このビルドより新しいコミットがあるかを見る。
//! 起動時と、その後は 30 分おきに compare API（ビルド元...main）を呼び、進んでいれば設定画面と歯車のバッジで知らせる。
//! 読むだけなので、開発中のビルドでも RISCORD_NO_UPDATE でも動かす。ビルド元が分からなければ何もしない

use std::sync::{Mutex, Once};
use std::time::Duration;

use anyhow::Result;
use serde::Deserialize;

use crate::wake::Waker;

const REPO: &str = "kokorodonguri/Riscord";
const BRANCH: &str = "main";
/// build.rs が埋め込むビルド元のコミット（分からなければ空）
const BUILD: &str = env!("RISCORD_GIT_SHA");
const RECHECK: Duration = Duration::from_secs(30 * 60);
/// 設定画面に並べる最大件数
const SHOW: usize = 10;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Commit {
    pub sha: String,
    /// メッセージの 1 行目
    pub title: String,
    /// "2026-11-11" の形（UTC）
    pub date: String,
    pub url: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Status {
    /// 確認していない（ビルド元が分からない）
    Off,
    Checking,
    UpToDate,
    /// main がこのビルドより進んでいる。commits は新しい順
    Behind {
        ahead_by: u32,
        commits: Vec<Commit>,
        compare_url: String,
    },
    /// 比べられない（GitHub にないコミットからのビルドなど）
    Unknown(String),
    Failed(String),
}

static STATUS: Mutex<Status> = Mutex::new(Status::Off);
static START: Once = Once::new();
/// 設定画面で最後に見た main の先頭
static SEEN: Mutex<Option<String>> = Mutex::new(None);

pub fn status() -> Status {
    STATUS.lock().unwrap().clone()
}

/// ビルド元の短い sha（分からなければ None）
pub fn build_short() -> Option<&'static str> {
    (!BUILD.is_empty()).then(|| &BUILD[..BUILD.len().min(7)])
}

fn head(s: &Status) -> Option<&str> {
    match s {
        Status::Behind { commits, .. } => commits.first().map(|c| c.sha.as_str()),
        _ => None,
    }
}

/// 歯車にバッジを付けるか（新しいコミットがあり、まだ設定画面で見ていない）
pub fn badge() -> bool {
    let s = STATUS.lock().unwrap();
    head(&s).is_some_and(|h| SEEN.lock().unwrap().as_deref() != Some(h))
}

/// 設定画面で見た
pub fn mark_seen() {
    let s = STATUS.lock().unwrap();
    if let Some(h) = head(&s) {
        *SEEN.lock().unwrap() = Some(h.to_string());
    }
}

fn set(s: Status, waker: &Waker) {
    *STATUS.lock().unwrap() = s;
    waker.wake();
}

/// 起動時に一度だけ呼ぶ（2回目以降は何もしない）
pub fn start(waker: &Waker) {
    START.call_once(|| {
        if BUILD.is_empty() || cfg!(test) {
            return;
        }
        let waker = waker.clone();
        std::thread::spawn(move || {
            set(Status::Checking, &waker);
            let rt = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(rt) => rt,
                Err(e) => return set(Status::Failed(e.to_string()), &waker),
            };
            loop {
                match rt.block_on(fetch()) {
                    Ok(s) => set(s, &waker),
                    Err(e) => {
                        eprintln!("GitHub のコミットを確認できませんでした: {e:#}");
                        // 一度取れていれば、通信の失敗で一覧を消さない
                        if !matches!(status(), Status::Behind { .. } | Status::UpToDate) {
                            set(Status::Failed(format!("{e:#}")), &waker);
                        }
                    }
                }
                std::thread::sleep(RECHECK);
            }
        });
    });
}

async fn fetch() -> Result<Status> {
    let http = reqwest::Client::builder()
        .user_agent(format!("riscord/{}", env!("CARGO_PKG_VERSION")))
        .connect_timeout(Duration::from_secs(10))
        .build()?;
    let res = http
        .get(format!(
            "https://api.github.com/repos/{REPO}/compare/{BUILD}...{BRANCH}"
        ))
        .header("Accept", "application/vnd.github+json")
        .timeout(Duration::from_secs(30))
        .send()
        .await?;
    // 404 = GitHub にないコミット（まだプッシュしていない手元のビルドなど）
    if matches!(res.status().as_u16(), 404 | 422) {
        return Ok(Status::Unknown(
            "このビルドのコミットは GitHub にありません".into(),
        ));
    }
    parse_compare(&res.error_for_status()?.text().await?)
}

#[derive(Deserialize)]
struct Compare {
    status: String,
    ahead_by: u32,
    html_url: String,
    commits: Vec<RawCommit>,
}

#[derive(Deserialize)]
struct RawCommit {
    sha: String,
    html_url: String,
    commit: RawInner,
}

#[derive(Deserialize)]
struct RawInner {
    message: String,
    author: Option<RawAuthor>,
}

#[derive(Deserialize)]
struct RawAuthor {
    date: String,
}

fn parse_compare(json: &str) -> Result<Status> {
    let c: Compare = serde_json::from_str(json)?;
    if !matches!(c.status.as_str(), "ahead" | "diverged") || c.ahead_by == 0 {
        return Ok(Status::UpToDate);
    }
    // API は古い順に返すので、新しい順にする
    let commits = c
        .commits
        .into_iter()
        .rev()
        .take(SHOW)
        .map(|r| Commit {
            title: r
                .commit
                .message
                .lines()
                .next()
                .unwrap_or_default()
                .to_string(),
            date: r
                .commit
                .author
                .map(|a| a.date.chars().take(10).collect())
                .unwrap_or_default(),
            sha: r.sha,
            url: r.html_url,
        })
        .collect();
    Ok(Status::Behind {
        ahead_by: c.ahead_by,
        commits,
        compare_url: c.html_url,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raw(sha: &str, msg: &str) -> String {
        format!(
            r#"{{"sha":"{sha}","html_url":"https://github.com/x/commit/{sha}","commit":{{"message":{},"author":{{"date":"2026-11-11T12:52:39Z"}}}}}}"#,
            serde_json::to_string(msg).unwrap()
        )
    }

    #[test]
    fn identical_is_up_to_date() {
        let json =
            r#"{"status":"identical","ahead_by":0,"behind_by":0,"html_url":"u","commits":[]}"#;
        assert_eq!(parse_compare(json).unwrap(), Status::UpToDate);
        let json = r#"{"status":"behind","ahead_by":0,"behind_by":2,"html_url":"u","commits":[]}"#;
        assert_eq!(parse_compare(json).unwrap(), Status::UpToDate);
    }

    #[test]
    fn ahead_lists_newest_first_with_first_line() {
        let json = format!(
            r#"{{"status":"ahead","ahead_by":2,"html_url":"https://github.com/cmp","commits":[{},{}]}}"#,
            raw("aaa", "old one"),
            raw("bbb", "new one\n\ndetails")
        );
        let Status::Behind {
            ahead_by,
            commits,
            compare_url,
        } = parse_compare(&json).unwrap()
        else {
            panic!()
        };
        assert_eq!(ahead_by, 2);
        assert_eq!(compare_url, "https://github.com/cmp");
        assert_eq!(commits[0].sha, "bbb");
        assert_eq!(commits[0].title, "new one");
        assert_eq!(commits[0].date, "2026-11-11");
        assert_eq!(commits[1].sha, "aaa");
    }

    #[test]
    fn diverged_counts_as_behind() {
        let json = format!(
            r#"{{"status":"diverged","ahead_by":1,"behind_by":1,"html_url":"u","commits":[{}]}}"#,
            raw("ccc", "x")
        );
        assert!(matches!(
            parse_compare(&json).unwrap(),
            Status::Behind { ahead_by: 1, .. }
        ));
    }

    #[test]
    fn badge_until_seen_and_again_for_newer() {
        let behind = |sha: &str| Status::Behind {
            ahead_by: 1,
            commits: vec![Commit {
                sha: sha.into(),
                title: String::new(),
                date: String::new(),
                url: String::new(),
            }],
            compare_url: String::new(),
        };
        *STATUS.lock().unwrap() = behind("a");
        assert!(badge());
        mark_seen();
        assert!(!badge());
        *STATUS.lock().unwrap() = behind("b");
        assert!(badge());
        *STATUS.lock().unwrap() = Status::Off;
        *SEEN.lock().unwrap() = None;
    }
}
