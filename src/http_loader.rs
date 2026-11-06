//! http(s):// の画像を取ってくる BytesLoader。
//! egui_extras の http ローダー（ehttp）は1枚ごとにスレッドを作り、接続（DNS・TCP・TLS）も毎回作り直すので、
//! アバターやアイコンが多いと読み込みが遅く CPU も食う。ここでは接続を使い回す reqwest のクライアント1つで取る。
//! キャッシュの持ち方・結果・エラー文は ehttp ローダーと同じにしてある
use eframe::egui::{
    self,
    load::{Bytes, BytesLoadResult, BytesLoader, BytesPoll, LoadError},
    mutex::Mutex,
};
use std::{collections::HashMap, sync::Arc, sync::OnceLock, task::Poll, time::Duration};

#[derive(Clone)]
struct File {
    bytes: Arc<[u8]>,
    mime: Option<String>,
}

type Entry = Poll<Result<File, String>>;

/// 通信用のランタイムとクライアント。http の画像を初めて読むときに作る
struct Worker {
    rt: tokio::runtime::Runtime,
    http: reqwest::Client,
}

#[derive(Default)]
pub struct HttpLoader {
    cache: Arc<Mutex<HashMap<String, Entry>>>,
    worker: OnceLock<Worker>,
}

impl HttpLoader {
    pub const ID: &'static str = egui::generate_loader_id!(HttpLoader);

    fn worker(&self) -> &Worker {
        self.worker.get_or_init(|| Worker {
            // 待ち時間がほとんどなので少数のスレッドで足りる
            rt: tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .thread_name("http-image")
                .enable_all()
                .build()
                .expect("tokio"),
            http: reqwest::Client::builder()
                .user_agent("Mozilla/5.0 (X11; Linux x86_64) riscord/0.1")
                // ureq（ehttp）の既定と同じ
                .connect_timeout(Duration::from_secs(30))
                .build()
                .expect("http client"),
        })
    }
}

impl Drop for HttpLoader {
    fn drop(&mut self) {
        // 終了時に名前解決などの途中で待たされないようにする
        if let Some(w) = self.worker.take() {
            w.rt.shutdown_background();
        }
    }
}

async fn fetch(http: &reqwest::Client, uri: &str) -> Result<File, String> {
    let failed = || format!("Failed to load {uri:?}");
    let resp = http.get(uri).send().await.map_err(|_| failed())?;
    let status = resp.status();
    let mime = resp.headers().get(reqwest::header::CONTENT_TYPE).and_then(|v| v.to_str().ok()).map(str::to_owned);
    let bytes = resp.bytes().await.map_err(|_| failed())?;
    // ehttp と同じく 400 未満を成功とし、失敗時は本文も添える
    if status.as_u16() >= 400 {
        let reason = status.canonical_reason().unwrap_or("");
        return Err(match std::str::from_utf8(&bytes) {
            Ok(text) => format!("failed to load {uri:?}: {} {reason} {text}", status.as_u16()),
            Err(_) => format!("failed to load {uri:?}: {} {reason}", status.as_u16()),
        });
    }
    Ok(File { bytes: Arc::from(bytes.as_ref()), mime })
}

impl BytesLoader for HttpLoader {
    fn id(&self) -> &str {
        Self::ID
    }

    fn load(&self, ctx: &egui::Context, uri: &str) -> BytesLoadResult {
        if !(uri.starts_with("http://") || uri.starts_with("https://")) {
            return Err(LoadError::NotSupported);
        }
        let mut cache = self.cache.lock();
        if let Some(entry) = cache.get(uri).cloned() {
            return match entry {
                Poll::Ready(Ok(file)) => Ok(BytesPoll::Ready { size: None, bytes: Bytes::Shared(file.bytes), mime: file.mime }),
                Poll::Ready(Err(err)) => Err(LoadError::Loading(err)),
                Poll::Pending => Ok(BytesPoll::Pending { size: None }),
            };
        }
        let uri = uri.to_owned();
        cache.insert(uri.clone(), Poll::Pending);
        drop(cache);
        let w = self.worker();
        let (http, ctx, cache) = (w.http.clone(), ctx.clone(), self.cache.clone());
        w.rt.spawn(async move {
            let result = fetch(&http, &uri).await;
            cache.lock().insert(uri, Poll::Ready(result));
            ctx.request_repaint();
        });
        Ok(BytesPoll::Pending { size: None })
    }

    fn forget(&self, uri: &str) {
        let _ = self.cache.lock().remove(uri);
    }

    fn forget_all(&self) {
        self.cache.lock().clear();
    }

    fn byte_size(&self) -> usize {
        self.cache
            .lock()
            .values()
            .map(|entry| match entry {
                Poll::Ready(Ok(file)) => file.bytes.len() + file.mime.as_ref().map_or(0, |m| m.len()),
                Poll::Ready(Err(err)) => err.len(),
                _ => 0,
            })
            .sum()
    }
}
