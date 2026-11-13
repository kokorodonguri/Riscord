//! Attachment downloads run outside the UI thread and replace the target only on success.
use crate::wake::Waker;
use std::{
    io::Write,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicU64, Ordering},
        mpsc::{self, Receiver},
    },
    time::Duration,
};

pub enum Event {
    Started,
    Finished(PathBuf),
    Failed(String),
    Cancelled,
}
static NEXT: AtomicU64 = AtomicU64::new(1);

/// 何を保存するか（保存ダイアログの題・お知らせの言い回し・名前が分からないときのファイル名に使う）
#[derive(Clone, Copy)]
pub enum Kind {
    Video,
    Image,
    File,
}

impl Kind {
    pub fn label(self) -> &'static str {
        match self {
            Kind::Video => tr!("動画", "video"),
            Kind::Image => tr!("画像", "image"),
            Kind::File => tr!("ファイル", "file"),
        }
    }

    fn default_name(self) -> &'static str {
        match self {
            Kind::Video => "video.mp4",
            Kind::Image => "image.png",
            Kind::File => "file",
        }
    }
}

/// URL の最後の部分をファイル名にする（? 以降は除く）。取れなければ空
pub fn name_from_url(url: &str) -> String {
    let rest = url.split(['?', '#']).next().unwrap_or("");
    // ホスト名をファイル名にしないよう、スキーム://ホスト の後ろだけを見る
    let path = rest
        .split_once("://")
        .map_or(rest, |(_, r)| r)
        .split_once('/')
        .map_or("", |(_, p)| p);
    path.rsplit('/').next().unwrap_or("").to_string()
}

pub fn start(url: String, filename: String, kind: Kind, waker: Waker) -> Receiver<Event> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let Ok(rt) = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        else {
            let _ = tx.send(Event::Failed(
                tr!(
                    "ダウンロードを開始できませんでした",
                    "Couldn't start the download"
                )
                .into(),
            ));
            waker.wake();
            return;
        };
        rt.block_on(async {
            let name = filename
                .rsplit(['/', '\\'])
                .next()
                .filter(|s| !s.is_empty() && *s != "." && *s != "..")
                .unwrap_or(kind.default_name());
            let mut dialog = rfd::AsyncFileDialog::new()
                .set_title(tr!("{}を保存", "Save {}", kind.label()))
                .set_file_name(name);
            if let Some(dir) = dirs::download_dir() {
                dialog = dialog.set_directory(dir);
            }
            let Some(file) = dialog.save_file().await else {
                let _ = tx.send(Event::Cancelled);
                waker.wake();
                return;
            };
            let path = file.path().to_path_buf();
            let _ = tx.send(Event::Started);
            waker.wake();
            let event = match download_to(&url, &path, kind.label()).await {
                Ok(()) => Event::Finished(path),
                Err(error) => Event::Failed(error),
            };
            let _ = tx.send(event);
            waker.wake();
        });
    });
    rx
}

/// what = 保存するものの呼び名（「動画」「画像」。失敗したときのお知らせに使う）
async fn download_to(url: &str, destination: &Path, what: &str) -> Result<(), String> {
    let parsed = reqwest::Url::parse(url)
        .map_err(|_| tr!("{what}の URL が無効です", "Invalid {what} URL",))?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err(tr!(
            "対応していない{what}の URL です",
            "Unsupported {what} URL",
        ));
    }
    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(30))
        .read_timeout(Duration::from_secs(30))
        .build()
        .map_err(|_| {
            tr!(
                "通信を開始できませんでした",
                "Couldn't start the connection"
            )
            .to_string()
        })?;
    let mut response = client
        .get(parsed)
        .send()
        .await
        .map_err(|_| tr!("{what}を取得できませんでした", "Couldn't fetch the {what}",))?
        .error_for_status()
        .map_err(|e| {
            tr!(
                "{what}を取得できませんでした ({})",
                "Couldn't fetch the {what} ({})",
                e.status().map(|s| s.as_u16()).unwrap_or(0)
            )
        })?;
    let parent = destination
        .parent()
        .ok_or(tr!("保存先が無効です", "Invalid save location"))?;
    let mut temp = None;
    for _ in 0..10 {
        let name = format!(
            ".riscord-download-{}-{}.tmp",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        );
        let path = parent.join(name);
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(file) => {
                temp = Some((path, file));
                break;
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(_) => {
                return Err(tr!(
                    "保存先にファイルを作成できませんでした",
                    "Couldn't create the file at the save location"
                )
                .into())
            }
        }
    }
    let (path, mut file) = temp.ok_or(tr!(
        "一時ファイルを作成できませんでした",
        "Couldn't create a temporary file"
    ))?;
    let result = async {
        while let Some(chunk) = response.chunk().await.map_err(|_| {
            tr!(
                "{what}のダウンロードが途中で失敗しました",
                "The {what} download failed partway",
            )
        })? {
            file.write_all(&chunk)
                .map_err(|_| tr!("{what}を保存できませんでした", "Couldn't save the {what}",))?;
        }
        file.flush()
            .map_err(|_| tr!("{what}を保存できませんでした", "Couldn't save the {what}",))?;
        drop(file);
        std::fs::rename(&path, destination).map_err(|_| {
            tr!(
                "保存先へ移動できませんでした",
                "Couldn't move it to the save location"
            )
            .to_string()
        })?;
        Ok::<(), String>(())
    }
    .await;
    if result.is_err() {
        let _ = std::fs::remove_file(&path);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;
    fn server(response: &'static [u8]) -> String {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0; 2048];
            let _ = stream.read(&mut request);
            let _ = stream.write_all(response);
        });
        format!("http://{address}/video")
    }
    fn test_dir() -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "riscord-download-test-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&path).unwrap();
        path
    }
    #[tokio::test]
    async fn saves_download_and_replaces_existing_file() {
        let dir = test_dir();
        let path = dir.join("video.mp4");
        std::fs::write(&path, b"old").unwrap();
        let url = server(b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\nConnection: close\r\n\r\nvideo");
        download_to(&url, &path, "動画").await.unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"video");
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
        std::fs::remove_dir_all(dir).unwrap();
    }
    #[tokio::test]
    async fn incomplete_response_preserves_existing_file() {
        let dir = test_dir();
        let path = dir.join("video.mp4");
        std::fs::write(&path, b"old").unwrap();
        let url =
            server(b"HTTP/1.1 200 OK\r\nContent-Length: 50\r\nConnection: close\r\n\r\nshort");
        assert!(download_to(&url, &path, "動画").await.is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"old");
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
        std::fs::remove_dir_all(dir).unwrap();
    }
    #[tokio::test]
    async fn http_error_does_not_create_file() {
        let dir = test_dir();
        let path = dir.join("video.mp4");
        let url =
            server(b"HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
        assert!(download_to(&url, &path, "動画").await.is_err());
        assert!(!path.exists());
        std::fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn file_name_is_taken_from_url_path() {
        assert_eq!(
            name_from_url("https://cdn.example.com/a/b/cat.png?ex=1&is=2"),
            "cat.png"
        );
        assert_eq!(
            name_from_url("https://example.com/photo.jpg#x"),
            "photo.jpg"
        );
        assert_eq!(name_from_url("https://example.com/"), "");
        assert_eq!(name_from_url("https://example.com"), "");
    }
}
