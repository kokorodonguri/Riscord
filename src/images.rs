//! 画像の取得とデコード。http(s):// は接続を使い回す reqwest のクライアント1つで取り、file:// はファイルを読む。
//! デコード（GIF・アニメーション WebP の全フレームを含む）は UI スレッドの外で行い、
//! 終わるまでは読み込み中として扱う。大きすぎるものは縮小・フレームの間引きをしてメモリも抑える。
//! 出来上がった画像は gpui の RenderImage（BGRA）で持ち、`img()` にそのまま渡せる（アニメも gpui が進める）
use gpui::RenderImage;
use image::{codecs::gif::GifDecoder, codecs::png::PngDecoder, codecs::webp::WebPDecoder, AnimationDecoder as _, Delay, Frame, ImageDecoder as _, RgbaImage};
use std::collections::HashMap;
use std::io::Cursor;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use crate::wake::Waker;

/// 1フレームの最大画素数。表示は最大 420×320（高DPI で倍）なので、これより大きいものは縮める
const MAX_FRAME_PIXELS: u32 = 840 * 640;
/// 背景画像（file://）はウィンドウいっぱいに敷くので大きめまで許す
const MAX_BACKGROUND_PIXELS: u32 = 3840 * 2160;
/// 1枚のアニメーションに使うメモリの上限。超えるならフレームを間引く
const MAX_BYTES: usize = 64 * 1024 * 1024;
/// 同時にデコードする数（CPU を食い尽くさないよう少なめ）
const DECODERS: usize = 2;

#[derive(Clone)]
pub enum Img {
    Ready(Arc<RenderImage>),
    Pending,
    Failed,
}

enum Entry {
    Pending(Arc<()>),
    Ready(Arc<RenderImage>),
    Failed,
}

// 同じ URI を捨てて再取得した場合、古い非同期処理は新しい要求を完了できない。
fn is_current_request(entry: Option<&Entry>, request: &Arc<()>) -> bool {
    matches!(entry, Some(Entry::Pending(current)) if Arc::ptr_eq(current, request))
}

struct Store {
    cache: Mutex<HashMap<String, Entry>>,
    /// 捨てた画像。描画側で GPU のテクスチャも解放する
    dropped: Mutex<Vec<Arc<RenderImage>>>,
    waker: Mutex<Waker>,
    rt: tokio::runtime::Runtime,
    http: reqwest::Client,
    decode: Arc<tokio::sync::Semaphore>,
}

fn store() -> &'static Store {
    static S: OnceLock<Store> = OnceLock::new();
    S.get_or_init(|| Store {
        cache: Mutex::new(HashMap::new()),
        dropped: Mutex::new(Vec::new()),
        waker: Mutex::new(Waker::default()),
        // 待ち時間がほとんどなので少数のスレッドで足りる
        rt: tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("image")
            .enable_all()
            .build()
            .expect("tokio"),
        http: image_client(Duration::from_secs(30)).expect("http client"),
        decode: Arc::new(tokio::sync::Semaphore::new(DECODERS)),
    })
}

fn image_client(read_timeout: Duration) -> Result<reqwest::Client, reqwest::Error> {
    reqwest::Client::builder()
        .user_agent("Mozilla/5.0 (X11; Linux x86_64) riscord/0.1")
        .connect_timeout(Duration::from_secs(30))
        .read_timeout(read_timeout)
        .build()
}

/// 読み込みが終わったときに画面を描き直してもらう口
pub fn set_waker(w: Waker) {
    *store().waker.lock().unwrap() = w;
}

/// 画像を返す。まだなら読み込みを始めて Pending
pub fn get(uri: &str) -> Img {
    let s = store();
    let mut cache = s.cache.lock().unwrap();
    match cache.get(uri) {
        Some(Entry::Ready(i)) => return Img::Ready(i.clone()),
        Some(Entry::Pending(_)) => return Img::Pending,
        Some(Entry::Failed) => return Img::Failed,
        None => {}
    }
    let request = Arc::new(());
    cache.insert(uri.to_string(), Entry::Pending(request.clone()));
    drop(cache);
    let uri = uri.to_string();
    s.rt.spawn(async move {
        let s = store();
        let bytes = load_bytes(&s.http, &uri).await;
        let result = match bytes {
            Ok(b) => {
                let _permit = s.decode.clone().acquire_owned().await;
                // 待っている間に捨てられた（チャンネルを閉じた等）ならデコードしない
                if !is_current_request(s.cache.lock().unwrap().get(&uri), &request) {
                    return;
                }
                let max = if uri.starts_with("file://") { MAX_BACKGROUND_PIXELS } else { MAX_FRAME_PIXELS };
                tokio::task::spawn_blocking(move || decode(&b, max)).await.unwrap_or_else(|_| Err("decode panicked".into()))
            }
            Err(e) => Err(e),
        };
        let mut cache = s.cache.lock().unwrap();
        if !is_current_request(cache.get(&uri), &request) {
            return;
        }
        match result {
            Ok(frames) => {
                cache.insert(uri, Entry::Ready(Arc::new(RenderImage::new(frames))));
            }
            Err(e) => {
                eprintln!("画像の読み込み失敗 {uri}: {e}");
                cache.insert(uri, Entry::Failed);
            }
        }
        drop(cache);
        s.waker.lock().unwrap().wake();
    });
    Img::Pending
}

/// 読み込み済みなら返す（読み込みは始めない）。テストで結果を待つのに使う
#[cfg(test)]
pub fn peek(uri: &str) -> Img {
    match store().cache.lock().unwrap().get(uri) {
        Some(Entry::Ready(i)) => Img::Ready(i.clone()),
        Some(Entry::Pending(_)) => Img::Pending,
        Some(Entry::Failed) => Img::Failed,
        None => Img::Pending,
    }
}

/// キャッシュから捨てる（次に get したら読み直す）
pub fn forget(uri: &str) {
    let s = store();
    if let Some(Entry::Ready(i)) = s.cache.lock().unwrap().remove(uri) {
        s.dropped.lock().unwrap().push(i);
    }
}

/// キャッシュの外で作った画像（スタンプなど）を捨てる。描画側で GPU のテクスチャも解放する
pub fn release(img: Arc<RenderImage>) {
    store().dropped.lock().unwrap().push(img);
}

/// 捨てた画像（GPU のテクスチャを解放するため）
pub fn take_dropped() -> Vec<Arc<RenderImage>> {
    std::mem::take(&mut *store().dropped.lock().unwrap())
}

#[cfg(test)]
pub(crate) fn release_queued(image: &Arc<RenderImage>) -> bool {
    store().dropped.lock().unwrap().iter().any(|item| Arc::ptr_eq(item, image))
}

async fn load_bytes(http: &reqwest::Client, uri: &str) -> Result<Vec<u8>, String> {
    if let Some(path) = uri.strip_prefix("file://") {
        return tokio::fs::read(path).await.map_err(|e| format!("{path}: {e}"));
    }
    if !(uri.starts_with("http://") || uri.starts_with("https://")) {
        return Err("対応していない URI".into());
    }
    let disk = disk_path(uri);
    if let Some(p) = &disk {
        if let Ok(b) = tokio::fs::read(p).await {
            return Ok(b);
        }
    }
    let resp = http.get(uri).send().await.map_err(|e| e.to_string())?;
    let status = resp.status();
    let bytes = resp.bytes().await.map_err(|e| e.to_string())?;
    if status.as_u16() >= 400 {
        return Err(format!("{}", status.as_u16()));
    }
    if let Some(p) = disk {
        let b = bytes.to_vec();
        tokio::task::spawn_blocking(move || {
            let _ = std::fs::create_dir_all(p.parent().unwrap());
            let tmp = p.with_extension("tmp");
            if crate::cache::write_private(&tmp, &b).is_ok() {
                let _ = std::fs::rename(&tmp, &p);
            }
        });
    }
    Ok(bytes.to_vec())
}

/// アイコン（ユーザー・サーバー・グループ DM）はディスクにも控え、次の起動ですぐ出す。
/// URL にハッシュが入っていて中身が変わらないので、期限なしで使ってよい
fn disk_path(uri: &str) -> Option<std::path::PathBuf> {
    const KEPT: [&str; 6] = [
        "https://cdn.discordapp.com/avatars/",
        "https://cdn.discordapp.com/embed/avatars/",
        "https://cdn.discordapp.com/icons/",
        "https://cdn.discordapp.com/channel-icons/",
        "https://cdn.discordapp.com/avatar-decoration-presets/",
        "https://cdn.discordapp.com/guilds/",
    ];
    if !KEPT.iter().any(|k| uri.starts_with(k)) {
        return None;
    }
    use sha2::Digest as _;
    let name = crate::cache::hex(&sha2::Sha256::digest(uri.as_bytes()));
    Some(crate::cache::image_dir().join(name))
}

/// ブラウザと同じく、極端に短い表示時間（0〜10ms）は 100ms とみなす
fn frame_delay(d: Duration) -> Duration {
    if d < Duration::from_millis(20) {
        Duration::from_millis(100)
    } else {
        d
    }
}

fn shrink(img: RgbaImage, max: u32) -> RgbaImage {
    let (w, h) = img.dimensions();
    if (w as u64) * (h as u64) <= max as u64 {
        return img;
    }
    let k = (max as f64 / (w as f64 * h as f64)).sqrt();
    let (nw, nh) = (((w as f64 * k) as u32).max(1), ((h as f64 * k) as u32).max(1));
    image::imageops::resize(&img, nw, nh, image::imageops::FilterType::Triangle)
}

/// RGBA（非乗算）を gpui の BGRA のフレームにする
pub fn bgra_frame(mut img: RgbaImage, delay: Duration) -> Frame {
    for p in img.chunks_exact_mut(4) {
        p.swap(0, 2);
    }
    Frame::from_parts(img, 0, 0, Delay::from_saturating_duration(delay))
}

/// RGBA の1枚絵から RenderImage を作る（貼り付けた画像のサムネイルなど）
pub fn render_image(img: RgbaImage) -> Arc<RenderImage> {
    Arc::new(RenderImage::new(vec![bgra_frame(img, Duration::ZERO)]))
}

fn is_gif(b: &[u8]) -> bool {
    b.starts_with(b"GIF8")
}

fn is_webp(b: &[u8]) -> bool {
    b.len() >= 12 && &b[0..4] == b"RIFF" && &b[8..12] == b"WEBP"
}

fn is_png(b: &[u8]) -> bool {
    b.starts_with(b"\x89PNG\r\n\x1a\n")
}

/// 全フレームをデコードする。メモリの上限を超えそうなら、表示時間を足し合わせながらフレームを間引く
pub fn decode(bytes: &[u8], max_pixels: u32) -> Result<Vec<Frame>, String> {
    let (frames, (w, h)): (image::Frames<'_>, (u32, u32)) = if is_gif(bytes) {
        let d = GifDecoder::new(Cursor::new(bytes)).map_err(|e| format!("GIF decode failure ({e})"))?;
        let dims = d.dimensions();
        (d.into_frames(), dims)
    } else if is_webp(bytes) {
        let mut d = WebPDecoder::new(Cursor::new(bytes)).map_err(|e| format!("WebP decode failure ({e})"))?;
        let dims = d.dimensions();
        if !d.has_animation() {
            let img = image::DynamicImage::from_decoder(d).map_err(|e| format!("WebP decode failure ({e})"))?.to_rgba8();
            return Ok(vec![bgra_frame(shrink(img, max_pixels), Duration::ZERO)]);
        }
        d.set_background_color(image::Rgba([0, 0, 0, 0])).map_err(|e| format!("WebP decode failure ({e})"))?;
        (d.into_frames(), dims)
    } else if is_png(bytes) {
        let d = PngDecoder::new(Cursor::new(bytes)).map_err(|e| format!("PNG decode failure ({e})"))?;
        let dims = d.dimensions();
        if !d.is_apng().unwrap_or(false) {
            let img = image::DynamicImage::from_decoder(d).map_err(|e| format!("PNG decode failure ({e})"))?.to_rgba8();
            return Ok(vec![bgra_frame(shrink(img, max_pixels), Duration::ZERO)]);
        }
        (d.apng().map_err(|e| format!("APNG decode failure ({e})"))?.into_frames(), dims)
    } else {
        let img = image::load_from_memory(bytes).map_err(|e| format!("decode failure ({e})"))?.to_rgba8();
        return Ok(vec![bgra_frame(shrink(img, max_pixels), Duration::ZERO)]);
    };
    let frame_bytes = (w.max(1) as usize * h.max(1) as usize).min(max_pixels as usize) * 4;
    let max_frames = (MAX_BYTES / frame_bytes).max(1);
    let mut images: Vec<RgbaImage> = Vec::new();
    let mut durations: Vec<Duration> = Vec::new();
    // 最初は全部残し、上限に達したら1つおきに減らして間隔を倍にする（総フレーム数を先に知らなくて済む）
    let mut step = 1usize;
    for (i, f) in frames.enumerate() {
        let f = f.map_err(|e| format!("frame decode failure ({e})"))?;
        let delay = frame_delay(f.delay().into());
        if i % step == 0 {
            images.push(shrink(f.into_buffer(), max_pixels));
            durations.push(delay);
            if images.len() > max_frames {
                let mut j = 0;
                images.retain(|_| {
                    j += 1;
                    j % 2 == 1
                });
                durations = durations.chunks(2).map(|c| c.iter().sum()).collect();
                step *= 2;
            }
        } else if let Some(last) = durations.last_mut() {
            *last += delay;
        }
    }
    if images.is_empty() {
        return Err("no frames".into());
    }
    Ok(images.into_iter().zip(durations).map(|(img, d)| bgra_frame(img, d)).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gif(frames: usize, w: u16, h: u16) -> Vec<u8> {
        let mut out = Vec::new();
        {
            let mut enc = image::codecs::gif::GifEncoder::new(&mut out);
            for i in 0..frames {
                let img = RgbaImage::from_pixel(w as u32, h as u32, image::Rgba([i as u8, 0, 0, 255]));
                enc.encode_frame(Frame::from_parts(img, 0, 0, Delay::from_numer_denom_ms(50, 1))).unwrap();
            }
        }
        out
    }

    #[test]
    fn decodes_all_gif_frames_as_bgra() {
        let frames = decode(&gif(3, 4, 4), MAX_FRAME_PIXELS).unwrap();
        assert_eq!(frames.len(), 3);
        // 赤（R=2）が BGRA の3バイト目に来る
        assert_eq!(frames[2].buffer().as_raw()[2], 2);
    }

    #[test]
    fn huge_animations_are_thinned_out() {
        // 1フレーム 840x640 の上限ぎりぎり → 64MB に収まるよう間引く
        let frames = decode(&gif(40, 840, 640), MAX_FRAME_PIXELS).unwrap();
        assert!(frames.len() * 840 * 640 * 4 <= MAX_BYTES, "{}", frames.len());
        assert!(frames.len() < 40, "frames should be thinned to fit the memory budget");
        let total: Duration = frames.iter().map(|frame| Duration::from(frame.delay())).sum();
        assert_eq!(total, Duration::from_millis(2000));
    }

    #[test]
    fn animated_webp_preserves_multiple_frames_and_timing() {
        let frames = decode(include_bytes!("../tests/fixtures/animated.webp"), MAX_FRAME_PIXELS).unwrap();
        assert!(frames.len() > 1);
        assert!(frames.iter().all(|frame| Duration::from(frame.delay()) >= Duration::from_millis(20)));
    }

    #[test]
    fn static_png_decodes_and_shrinks() {
        let img = RgbaImage::from_pixel(2000, 2000, image::Rgba([1, 2, 3, 255]));
        let mut png = Vec::new();
        img.write_to(&mut Cursor::new(&mut png), image::ImageFormat::Png).unwrap();
        let frames = decode(&png, MAX_FRAME_PIXELS).unwrap();
        assert_eq!(frames.len(), 1);
        let (w, h) = frames[0].buffer().dimensions();
        assert!(w * h <= MAX_FRAME_PIXELS);
    }
}

#[cfg(test)]
mod request_tests {
    use super::*;

    #[tokio::test]
    async fn stalled_image_response_times_out() {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let (release, done) = std::sync::mpsc::channel::<()>();
        let server = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            socket.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
            let mut request = [0; 1024];
            socket.read(&mut request).unwrap();
            socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\n\r\nx").unwrap();
            let _ = done.recv_timeout(Duration::from_secs(5));
        });
        let client = image_client(Duration::from_millis(100)).unwrap();
        let start = std::time::Instant::now();
        let result = load_bytes(&client, &format!("http://{addr}/image")).await;
        let elapsed = start.elapsed();
        let _ = release.send(());
        server.join().unwrap();
        assert!(result.is_err(), "stalled body must not remain pending");
        assert!(elapsed >= Duration::from_millis(60) && elapsed < Duration::from_secs(3), "{elapsed:?}");
    }

    #[test]
    fn stale_request_cannot_complete_a_new_request_for_the_same_uri() {
        let old = Arc::new(());
        let new = Arc::new(());
        let mut cache = HashMap::new();
        cache.insert("image", Entry::Pending(old.clone()));
        assert!(is_current_request(cache.get("image"), &old));
        cache.remove("image");
        assert!(!is_current_request(cache.get("image"), &old));
        cache.insert("image", Entry::Pending(new.clone()));
        assert!(!is_current_request(cache.get("image"), &old));
        assert!(is_current_request(cache.get("image"), &new));
        cache.insert("image", Entry::Failed);
        assert!(!is_current_request(cache.get("image"), &new));
    }
}
