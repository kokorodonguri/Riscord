//! Translation requests run off the UI thread and never receive Discord credentials.
use crate::wake::Waker;
use std::{
    collections::HashMap,
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
}
pub struct Translator {
    tx: Sender<ResultItem>,
    rx: Receiver<ResultItem>,
    pub pending: usize,
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
fn parse_response(bytes: &[u8]) -> Result<String, String> {
    let data: serde_json::Value = serde_json::from_slice(bytes)
        .map_err(|_| "翻訳サービスの応答を読み取れませんでした".to_string())?;
    let sentences = data
        .get("sentences")
        .and_then(|s| s.as_array())
        .ok_or("翻訳サービスの応答を読み取れませんでした")?;
    let text: String = sentences
        .iter()
        .filter_map(|s| s.get("trans").and_then(|v| v.as_str()))
        .collect();
    if text.is_empty() {
        Err("翻訳結果が空でした".into())
    } else {
        Ok(text)
    }
}
impl Translator {
    pub fn request(
        &mut self,
        target: Target,
        text: String,
        language: String,
        waker: Waker,
    ) -> Result<(), String> {
        if self.pending >= 4 {
            return Err("翻訳中のリクエストが多すぎます".into());
        }
        if text.is_empty() || text.len() > 8000 {
            return Err("翻訳する文章または言語が無効です".into());
        }
        let language =
            crate::translator::language(&language).ok_or("翻訳する文章または言語が無効です")?;
        self.pending += 1;
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let result =
                tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .map_err(|_| "翻訳処理を開始できませんでした".to_string())
                    .and_then(|rt| {
                        rt.block_on(async {
                            let client = reqwest::Client::builder()
                                .timeout(Duration::from_secs(15))
                                .redirect(reqwest::redirect::Policy::none())
                                .build()
                                .map_err(|_| "翻訳処理を開始できませんでした".to_string())?;
                            let response = client
                                .get("https://translate.googleapis.com/translate_a/single")
                                .query(&[
                                    ("client", "gtx"),
                                    ("dt", "t"),
                                    ("dj", "1"),
                                    ("sl", "auto"),
                                    ("tl", language.as_str()),
                                    ("q", text.as_str()),
                                ])
                                .send()
                                .await
                                .map_err(|_| "翻訳サービスに接続できませんでした".to_string())?;
                            if !response.status().is_success() {
                                return Err(format!(
                                    "翻訳サービスがエラーを返しました ({})",
                                    response.status().as_u16()
                                ));
                            }
                            let mut response = response;
                            let mut bytes = Vec::new();
                            while let Some(chunk) = response.chunk().await.map_err(|_| {
                                "翻訳サービスの応答を受信できませんでした".to_string()
                            })? {
                                if bytes.len() + chunk.len() > 256 * 1024 {
                                    return Err("翻訳結果が大きすぎます".into());
                                }
                                bytes.extend_from_slice(&chunk);
                            }
                            parse_response(&bytes)
                        })
                    });
            let _ = tx.send(ResultItem { target, result });
            waker.wake();
        });
        Ok(())
    }
    #[cfg(test)]
    pub fn inject(&mut self, item: ResultItem) {
        self.pending += 1;
        self.tx.send(item).unwrap();
    }
    pub fn drain(&mut self) -> Vec<ResultItem> {
        let items: Vec<_> = self.rx.try_iter().collect();
        self.pending = self.pending.saturating_sub(items.len());
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
            parse_response(br#"{"sentences":[{"trans":"hello "},{"trans":"world"}]}"#).unwrap(),
            "hello world"
        );
        assert!(parse_response(b"not json").is_err());
        assert!(parse_response(br#"{"sentences":[]}"#).is_err());
    }
}
