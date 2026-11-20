//! Translation requests run off the UI thread and never receive Discord credentials.
use crate::wake::Waker;
use std::{
    collections::{HashMap, VecDeque},
    sync::mpsc::{self, Receiver, Sender},
    time::Duration,
};

#[derive(Clone, Debug)]
pub enum Target {
    Message {
        channel: String,
        id: String,
        original: String,
        generation: u64,
        /// 翻訳ボタンから頼んだもの（自動翻訳ではない）
        manual: bool,
    },
    Outgoing {
        channel: String,
        request_id: u64,
        generation: u64,
    },
}
pub struct ResultItem {
    pub target: Target,
    pub result: Result<String, String>,
    /// 原文がもともと翻訳先の言語だった
    pub same_language: bool,
}
struct Job {
    target: Target,
    text: String,
    language: String,
    waker: Waker,
}
const MAX_RUNNING: usize = 4;
const MAX_QUEUED: usize = 64;
pub struct Translator {
    tx: Sender<ResultItem>,
    rx: Receiver<ResultItem>,
    pub pending: usize,
    queue: VecDeque<Job>,
    pub next_request_id: u64,
    pub outgoing: HashMap<String, u64>,
    pub messages: HashMap<(String, String), (String, String)>,
}
impl Default for Translator {
    fn default() -> Self {
        let (tx, rx) = mpsc::channel();
        Self {
            tx,
            rx,
            pending: 0,
            queue: VecDeque::new(),
            next_request_id: 0,
            outgoing: HashMap::new(),
            messages: HashMap::new(),
        }
    }
}
pub fn language(code: &str) -> Option<String> {
    const CODES: &str = "af am ar az be bg bn bs ca ceb co cs cy da de el en eo es et eu fa fi fr fy ga gd gl gu ha haw hi hmn hr ht hu hy id ig is it iw he ja jw ka kk km kn ko ku ky la lb lo lt lv mg mi mk ml mn mr ms mt my ne nl no ny or pa pl ps pt ro ru rw sd si sk sl sm sn so sq sr st su sv sw ta te tg th tk tl tr tt ug uk ur uz vi xh yi yo zh-CN zh-TW zu";
    let code = if code == "jp" { "ja" } else { code };
    CODES
        .split_ascii_whitespace()
        .any(|valid| valid == code)
        .then(|| code.to_owned())
}
pub fn prefix(text: &str) -> Option<(String, &str)> {
    let (code, body) = text.strip_prefix('$')?.split_once(char::is_whitespace)?;
    let code = language(code)?;
    let body = body.trim_start();
    (!body.is_empty()).then_some((code, body))
}
fn parse_response(bytes: &[u8]) -> Result<(String, Option<String>), String> {
    let data: serde_json::Value = serde_json::from_slice(bytes).map_err(|_| {
        tr!(
            "翻訳サービスの応答を読み取れませんでした",
            "Couldn't read the translation service's response",
            "无法读取翻译服务的响应"
        )
        .to_string()
    })?;
    let sentences = data.get("sentences").and_then(|s| s.as_array()).ok_or(tr!(
        "翻訳サービスの応答を読み取れませんでした",
        "Couldn't read the translation service's response",
        "无法读取翻译服务的响应"
    ))?;
    let text: String = sentences
        .iter()
        .filter_map(|s| s.get("trans").and_then(|v| v.as_str()))
        .collect();
    if text.is_empty() {
        Err(tr!(
            "翻訳結果が空でした",
            "The translation was empty",
            "翻译结果为空"
        )
        .into())
    } else {
        let source = data.get("src").and_then(|v| v.as_str()).map(str::to_owned);
        Ok((text, source))
    }
}
/// 検出された原文の言語が翻訳先と同じか（zh-CN と zh-TW は別扱い、he と iw は同じ扱い）
fn same_language(source: &str, target: &str) -> bool {
    let norm = |code: &str| {
        let code = code.to_ascii_lowercase();
        match code.as_str() {
            "iw" => "he".to_owned(),
            "jw" => "jv".to_owned(),
            "zh" => "zh-cn".to_owned(),
            _ => code,
        }
    };
    norm(source) == norm(target)
}
impl Translator {
    pub fn request(
        &mut self,
        target: Target,
        text: String,
        language: String,
        waker: Waker,
    ) -> Result<(), String> {
        if text.is_empty() || text.len() > 8000 {
            return Err(tr!(
                "翻訳する文章または言語が無効です",
                "Invalid text or language to translate",
                "要翻译的文字或语言无效"
            )
            .into());
        }
        let language = crate::translator::language(&language).ok_or(tr!(
            "翻訳する文章または言語が無効です",
            "Invalid text or language to translate",
            "要翻译的文字或语言无效"
        ))?;
        let job = Job {
            target,
            text,
            language,
            waker,
        };
        if self.pending < MAX_RUNNING {
            self.start(job);
        } else if self.queue.len() < MAX_QUEUED {
            self.queue.push_back(job);
        } else {
            return Err(tr!(
                "翻訳中のリクエストが多すぎます",
                "Too many translation requests in progress",
                "正在进行的翻译请求过多"
            )
            .into());
        }
        Ok(())
    }
    fn start(&mut self, job: Job) {
        let Job {
            target,
            text,
            language,
            waker,
        } = job;
        self.pending += 1;
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let result = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|_| {
                    tr!(
                        "翻訳処理を開始できませんでした",
                        "Couldn't start translating",
                        "无法开始翻译"
                    )
                    .to_string()
                })
                .and_then(|rt| {
                    rt.block_on(async {
                        let client = reqwest::Client::builder()
                            .timeout(Duration::from_secs(15))
                            .redirect(reqwest::redirect::Policy::none())
                            .build()
                            .map_err(|_| {
                                tr!(
                                    "翻訳処理を開始できませんでした",
                                    "Couldn't start translating",
                                    "无法开始翻译"
                                )
                                .to_string()
                            })?;
                        // 本文は URL に載せず POST の本文で送る（URL はログや履歴に残りやすく、長さの上限もある）
                        let response = client
                            .post("https://translate.googleapis.com/translate_a/single")
                            .query(&[
                                ("client", "gtx"),
                                ("dt", "t"),
                                ("dj", "1"),
                                ("sl", "auto"),
                                ("tl", language.as_str()),
                            ])
                            .form(&[("q", text.as_str())])
                            .send()
                            .await
                            .map_err(|_| {
                                tr!(
                                    "翻訳サービスに接続できませんでした",
                                    "Couldn't connect to the translation service",
                                    "无法连接翻译服务"
                                )
                                .to_string()
                            })?;
                        if !response.status().is_success() {
                            return Err(tr!(
                                "翻訳サービスがエラーを返しました ({})",
                                "The translation service returned an error ({})",
                                "翻译服务返回了错误（{}）",
                                response.status().as_u16()
                            ));
                        }
                        let mut response = response;
                        let mut bytes = Vec::new();
                        while let Some(chunk) = response.chunk().await.map_err(|_| {
                            tr!(
                                "翻訳サービスの応答を受信できませんでした",
                                "Couldn't receive the translation service's response",
                                "无法接收翻译服务的响应"
                            )
                            .to_string()
                        })? {
                            if bytes.len() + chunk.len() > 256 * 1024 {
                                return Err(tr!(
                                    "翻訳結果が大きすぎます",
                                    "The translation is too large",
                                    "翻译结果过大"
                                )
                                .into());
                            }
                            bytes.extend_from_slice(&chunk);
                        }
                        parse_response(&bytes)
                    })
                });
            let same = matches!(&result, Ok((_, Some(source))) if same_language(source, &language));
            let _ = tx.send(ResultItem {
                target,
                result: result.map(|(text, _)| text),
                same_language: same,
            });
            waker.wake();
        });
    }
    #[cfg(test)]
    pub fn inject(&mut self, item: ResultItem) {
        self.pending += 1;
        self.tx.send(item).unwrap();
    }
    pub fn drain(&mut self) -> Vec<ResultItem> {
        let items: Vec<_> = self.rx.try_iter().collect();
        self.pending = self.pending.saturating_sub(items.len());
        while self.pending < MAX_RUNNING {
            let Some(job) = self.queue.pop_front() else {
                break;
            };
            self.start(job);
        }
        items
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn language_prefix_is_validated() {
        assert_eq!(
            prefix("$en こんにちは"),
            Some(("en".to_string(), "こんにちは"))
        );
        assert_eq!(prefix("$jp hello"), Some(("ja".to_string(), "hello")));
        assert_eq!(prefix("$zh-CN hello"), Some(("zh-CN".to_string(), "hello")));
        assert_eq!(prefix("$unknown hello"), None);
        assert_eq!(prefix("$en "), None);
    }
    #[test]
    fn parses_segmented_response_and_rejects_invalid() {
        assert_eq!(
            parse_response(br#"{"sentences":[{"trans":"hello "},{"trans":"world"}],"src":"en"}"#)
                .unwrap(),
            ("hello world".to_string(), Some("en".to_string()))
        );
        assert!(parse_response(b"not json").is_err());
        assert!(parse_response(br#"{"sentences":[]}"#).is_err());
    }
    #[test]
    fn detects_source_already_in_target_language() {
        assert!(same_language("ja", "ja"));
        assert!(same_language("iw", "he"));
        assert!(same_language("zh-CN", "zh-CN"));
        assert!(!same_language("zh-TW", "zh-CN"));
        assert!(!same_language("en", "ja"));
    }
}
#[cfg(test)]
mod queue_tests {
    use super::*;
    #[test]
    fn busy_requests_wait_instead_of_being_dropped() {
        let mut t = Translator::default();
        t.pending = MAX_RUNNING;
        let target = || Target::Outgoing {
            channel: "c".into(),
            request_id: 0,
            generation: 0,
        };
        for _ in 0..MAX_QUEUED {
            assert!(t
                .request(target(), "hi".into(), "ja".into(), Waker::default())
                .is_ok());
        }
        assert_eq!(t.queue.len(), MAX_QUEUED);
        assert!(t
            .request(target(), "hi".into(), "ja".into(), Waker::default())
            .is_err());
        assert!(t.drain().is_empty());
        assert_eq!(t.queue.len(), MAX_QUEUED);
    }
}
