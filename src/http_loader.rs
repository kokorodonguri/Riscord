//! http(s):// の画像を取ってくる BytesLoader。
//! egui_extras の http ローダー（ehttp）は1枚ごとにスレッドを作り、接続（DNS・TCP・TLS）も毎回作り直すので、
//! アバターやアイコンが多いと読み込みが遅く CPU も食う。ここでは接続を使い回す reqwest のクライアント1つで取る。
//! キャッシュの持ち方・結果・エラー文は ehttp ローダーと同じにしてある
use eframe::egui::{
    self,
    load::{Bytes, BytesLoadResult, BytesLoader, BytesPoll, LoadError},
    mutex::Mutex,
};
use std::{collections::HashMap, sync::Arc, sync::OnceLock, time::Duration};

#[derive(Clone)]
struct File {
    bytes: Arc<[u8]>,
    mime: Option<String>,
}

enum Entry {
    Pending(Arc<()>),
    Ready(Result<File, String>),
}

// Limit simultaneous downloads when a channel contains many images.
const MAX_DOWNLOADS: usize = 8;

/// 通信用のランタイムとクライアント。http の画像を初めて読むときに作る
struct Worker {
    rt: tokio::runtime::Runtime,
    http: reqwest::Client,
    permits: Arc<tokio::sync::Semaphore>,
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
                .timeout(Duration::from_secs(60))
                .build()
                .expect("http client"),
            permits: Arc::new(tokio::sync::Semaphore::new(MAX_DOWNLOADS)),
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

type Cache = Arc<Mutex<HashMap<String, Entry>>>;

fn is_pending(cache: &Cache, uri: &str, token: &Arc<()>) -> bool {
    matches!(cache.lock().get(uri), Some(Entry::Pending(current)) if Arc::ptr_eq(current, token))
}

fn complete(cache: &Cache, uri: &str, token: &Arc<()>, result: Result<File, String>) -> bool {
    let mut cache = cache.lock();
    if !matches!(cache.get(uri), Some(Entry::Pending(current)) if Arc::ptr_eq(current, token)) {
        return false;
    }
    cache.insert(uri.to_owned(), Entry::Ready(result));
    true
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
        if let Some(entry) = cache.get(uri) {
            return match entry {
                Entry::Ready(Ok(file)) => Ok(BytesPoll::Ready { size: None, bytes: Bytes::Shared(file.bytes.clone()), mime: file.mime.clone() }),
                Entry::Ready(Err(err)) => Err(LoadError::Loading(err.clone())),
                Entry::Pending(_) => Ok(BytesPoll::Pending { size: None }),
            };
        }
        let uri = uri.to_owned();
        let token = Arc::new(());
        cache.insert(uri.clone(), Entry::Pending(token.clone()));
        drop(cache);
        let w = self.worker();
        let (http, ctx, cache, permits) = (w.http.clone(), ctx.clone(), self.cache.clone(), w.permits.clone());
        w.rt.spawn(async move {
            let Ok(_permit) = permits.acquire().await else { return };
            // A forgotten request may have spent time waiting for a permit.
            if !is_pending(&cache, &uri, &token) { return; }
            let result = fetch(&http, &uri).await;
            if complete(&cache, &uri, &token, result) {
                ctx.request_repaint();
            }
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
                Entry::Ready(Ok(file)) => file.bytes.len() + file.mime.as_ref().map_or(0, |m| m.len()),
                Entry::Ready(Err(err)) => err.len(),
                _ => 0,
            })
            .sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn burst_limits_active_http_requests() {
        use std::{io::{Read, Write}, net::TcpListener, sync::atomic::{AtomicUsize, Ordering}, thread, time::Instant};

        const REQUESTS: usize = MAX_DOWNLOADS * 3;
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        listener.set_nonblocking(true).unwrap();
        let active = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));
        let server_active = active.clone();
        let server_peak = peak.clone();
        let server = thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(10);
            let mut handlers = Vec::new();
            while handlers.len() < REQUESTS && Instant::now() < deadline {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        let active = server_active.clone();
                        let peak = server_peak.clone();
                        handlers.push(thread::spawn(move || {
                            stream.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
                            let mut buf = [0; 1024];
                            let _ = stream.read(&mut buf).unwrap();
                            let count = active.fetch_add(1, Ordering::SeqCst) + 1;
                            peak.fetch_max(count, Ordering::SeqCst);
                            thread::sleep(Duration::from_millis(80));
                            active.fetch_sub(1, Ordering::SeqCst);
                            let _ = stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok");
                        }));
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => thread::sleep(Duration::from_millis(1)),
                    Err(e) => panic!("accept: {e}"),
                }
            }
            let count = handlers.len();
            for handler in handlers { handler.join().unwrap(); }
            count
        });
        let loader = HttpLoader::default();
        let ctx = egui::Context::default();
        let uris: Vec<_> = (0..REQUESTS).map(|i| format!("http://{addr}/{i}")).collect();
        for uri in &uris { assert!(matches!(loader.load(&ctx, uri), Ok(BytesPoll::Pending { .. }))); }
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let ready = uris.iter().filter(|uri| matches!(loader.load(&ctx, uri), Ok(BytesPoll::Ready { .. }))).count();
            if ready == REQUESTS { break; }
            assert!(Instant::now() < deadline, "only {ready} requests completed");
            thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(server.join().unwrap(), REQUESTS);
        let peak = peak.load(Ordering::SeqCst);
        assert!((2..=MAX_DOWNLOADS).contains(&peak), "peak active requests: {peak}");
    }

    #[test]
    fn forgotten_download_cannot_restore_or_overwrite_cache() {
        let loader = HttpLoader::default();
        let old = Arc::new(());
        let new = Arc::new(());
        loader.cache.lock().insert("url".into(), Entry::Pending(old.clone()));
        loader.forget("url");
        assert!(!complete(&loader.cache, "url", &old, Err("old".into())));
        loader.cache.lock().insert("url".into(), Entry::Pending(new.clone()));
        assert!(!complete(&loader.cache, "url", &old, Err("old".into())));
        assert!(is_pending(&loader.cache, "url", &new));
        assert!(complete(&loader.cache, "url", &new, Err("new".into())));
        assert_eq!(loader.byte_size(), 3);
    }
}
