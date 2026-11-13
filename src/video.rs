//! Inline video playback. Linux uses GStreamer; other platforms keep the download link.
#[cfg(target_os = "linux")]
mod platform {
    use crate::{images, wake::Waker};
    use gpui::RenderImage;
    use gst::prelude::*;
    use gst_video::VideoFrameExt;
    use gstreamer as gst;
    use gstreamer_app as gst_app;
    use gstreamer_video as gst_video;
    use std::{
        collections::{HashMap, VecDeque},
        sync::{mpsc, Arc, Mutex, OnceLock},
        time::Duration,
    };

    struct Frame {
        width: u32,
        height: u32,
        data: Vec<u8>,
    }
    fn frame_from_sample(sample: &gst::Sample) -> Option<Frame> {
        let info = gst_video::VideoInfo::from_caps(sample.caps()?).ok()?;
        let frame =
            gst_video::VideoFrameRef::from_buffer_ref_readable(sample.buffer()?, &info).ok()?;
        let data = frame.plane_data(0).ok()?;
        let stride = usize::try_from(frame.plane_stride()[0]).ok()?;
        let row = info.width() as usize * 4;
        if stride < row || data.len() < stride.checked_mul(info.height() as usize)? {
            return None;
        }
        let mut pixels = Vec::with_capacity(row * info.height() as usize);
        for line in data.chunks_exact(stride).take(info.height() as usize) {
            pixels.extend_from_slice(&line[..row]);
        }
        Some(Frame {
            width: info.width(),
            height: info.height(),
            data: pixels,
        })
    }
    fn render_frame(frame: Frame) -> Option<Arc<RenderImage>> {
        let buffer = image::RgbaImage::from_raw(frame.width, frame.height, frame.data)?;
        Some(Arc::new(RenderImage::new(vec![image::Frame::from_parts(
            buffer,
            0,
            0,
            image::Delay::from_saturating_duration(Duration::ZERO),
        )])))
    }
    enum ThumbnailEntry {
        Pending(Arc<()>),
        Ready(Arc<RenderImage>),
        Failed(std::time::Instant),
    }
    #[derive(Default)]
    struct ThumbnailCache {
        entries: HashMap<String, ThumbnailEntry>,
        order: VecDeque<String>,
    }
    struct ThumbnailRequest {
        url: String,
        width: u32,
        height: u32,
        waker: Waker,
        token: Arc<()>,
    }
    struct ThumbnailStore {
        cache: Arc<Mutex<ThumbnailCache>>,
        queue: mpsc::SyncSender<ThumbnailRequest>,
    }
    fn thumbnails() -> &'static ThumbnailStore {
        static STORE: OnceLock<ThumbnailStore> = OnceLock::new();
        STORE.get_or_init(|| {
            let cache = Arc::new(Mutex::new(ThumbnailCache::default()));
            let (queue, receiver) = mpsc::sync_channel::<ThumbnailRequest>(24);
            let receiver = Arc::new(Mutex::new(receiver));
            for _ in 0..2 {
                let receiver = receiver.clone();
                let cache = cache.clone();
                std::thread::spawn(move || loop {
                    let Ok(request) = receiver.lock().unwrap().recv() else { break };
                    let image = thumbnail_frame(&request.url, request.width, request.height).and_then(render_frame);
                    let mut cache = cache.lock().unwrap();
                    if matches!(cache.entries.get(&request.url), Some(ThumbnailEntry::Pending(token)) if Arc::ptr_eq(token, &request.token)) {
                        cache.entries.insert(request.url, image.map_or_else(|| ThumbnailEntry::Failed(std::time::Instant::now()), ThumbnailEntry::Ready));
                        drop(cache);
                        request.waker.wake();
                    }
                });
            }
            ThumbnailStore { cache, queue }
        })
    }
    /// Return a cached preview, scheduling a bounded background request when necessary.
    pub fn thumbnail(
        url: &str,
        width: u32,
        height: u32,
        waker: &Waker,
    ) -> Option<Arc<RenderImage>> {
        if !(url.starts_with("https://") || url.starts_with("http://")) {
            return None;
        }
        let store = thumbnails();
        let mut cache = store.cache.lock().unwrap();
        match cache.entries.get(url) {
            Some(ThumbnailEntry::Ready(image)) => return Some(image.clone()),
            Some(ThumbnailEntry::Pending(_)) => return None,
            Some(ThumbnailEntry::Failed(at)) if at.elapsed() < Duration::from_secs(30) => {
                return None
            }
            Some(ThumbnailEntry::Failed(_)) => {
                cache.entries.remove(url);
                cache.order.retain(|entry| entry != url);
            }
            None => (),
        }
        while cache.entries.len() >= 32 {
            let Some(old) = cache.order.pop_front() else {
                break;
            };
            if let Some(ThumbnailEntry::Ready(image)) = cache.entries.remove(&old) {
                images::release(image);
            }
        }
        let token = Arc::new(());
        cache
            .entries
            .insert(url.to_owned(), ThumbnailEntry::Pending(token.clone()));
        cache.order.push_back(url.to_owned());
        let request = ThumbnailRequest {
            url: url.to_owned(),
            width,
            height,
            waker: waker.clone(),
            token,
        };
        if store.queue.try_send(request).is_err() {
            cache.entries.remove(url);
            cache.order.retain(|entry| entry != url);
            waker.wake_after(Duration::from_millis(500));
        }
        None
    }
    fn thumbnail_frame(url: &str, width: u32, height: u32) -> Option<Frame> {
        gst::init().ok()?;
        let (width, height) = dimensions(width, height);
        let caps = gst::Caps::builder("video/x-raw")
            .field("format", "BGRA")
            .field("width", width as i32)
            .field("height", height as i32)
            .build();
        let sink = gst_app::AppSink::builder()
            .caps(&caps)
            .max_buffers(1)
            .drop(true)
            .build();
        let audio = gst::ElementFactory::make("fakesink").build().ok()?;
        let pipeline = gst::ElementFactory::make("playbin")
            .property("uri", url)
            .property("video-sink", &sink)
            .property("audio-sink", &audio)
            .build()
            .ok()?;
        if pipeline.set_state(gst::State::Paused).is_err() {
            let _ = pipeline.set_state(gst::State::Null);
            return None;
        }
        let frame = sink
            .try_pull_preroll(gst::ClockTime::from_seconds(10))
            .and_then(|sample| frame_from_sample(&sample));
        let _ = pipeline.set_state(gst::State::Null);
        frame
    }
    pub struct Player {
        pub channel: String,
        pub url: String,
        pub image: Option<Arc<RenderImage>>,
        pub error: Option<String>,
        pub paused: bool,
        pub ended: bool,
        pipeline: gst::Element,
        latest: Arc<Mutex<Option<Frame>>>,
    }
    impl Player {
        pub fn new(
            channel: String,
            url: String,
            width: u32,
            height: u32,
            waker: Waker,
        ) -> Result<Self, String> {
            if !(url.starts_with("https://") || url.starts_with("http://")) {
                return Err(tr!(
                    "対応していない動画 URL です",
                    "Unsupported video URL",
                    "不支持的视频 URL"
                )
                .into());
            }
            Self::new_inner(channel, url, width, height, waker)
        }
        fn new_inner(
            channel: String,
            url: String,
            width: u32,
            height: u32,
            waker: Waker,
        ) -> Result<Self, String> {
            gst::init().map_err(|e| {
                tr!(
                    "動画再生を初期化できません: {e}",
                    "Can't initialize video playback: {e}",
                    "无法初始化视频播放：{e}",
                )
            })?;
            let (width, height) = dimensions(width, height);
            let caps = gst::Caps::builder("video/x-raw")
                .field("format", "BGRA")
                .field("width", width as i32)
                .field("height", height as i32)
                .build();
            let latest = Arc::new(Mutex::new(None));
            let dest = latest.clone();
            let sink = gst_app::AppSink::builder()
                .caps(&caps)
                .max_buffers(1)
                .drop(true)
                .build();
            sink.set_callbacks(
                gst_app::AppSinkCallbacks::builder()
                    .new_sample(move |sink| {
                        let sample = sink.pull_sample().map_err(|_| gst::FlowError::Eos)?;
                        *dest.lock().unwrap() =
                            Some(frame_from_sample(&sample).ok_or(gst::FlowError::Error)?);
                        waker.wake();
                        Ok(gst::FlowSuccess::Ok)
                    })
                    .build(),
            );
            let pipeline = gst::ElementFactory::make("playbin")
                .property("uri", &url)
                .property("video-sink", &sink)
                .build()
                .map_err(|e| {
                    tr!(
                        "動画プレイヤーを作れません: {e}",
                        "Can't create the video player: {e}",
                        "无法创建视频播放器：{e}",
                    )
                })?;
            #[cfg(test)]
            pipeline.set_property(
                "audio-sink",
                gst::ElementFactory::make("fakesink")
                    .build()
                    .expect("fakesink"),
            );
            pipeline.set_state(gst::State::Playing).map_err(|e| {
                tr!(
                    "動画を再生できません: {e}",
                    "Can't play the video: {e}",
                    "无法播放视频：{e}",
                )
            })?;
            Ok(Self {
                channel,
                url,
                image: None,
                error: None,
                paused: false,
                ended: false,
                pipeline,
                latest,
            })
        }
        pub fn poll(&mut self) {
            if let Some(frame) = self.latest.lock().unwrap().take() {
                if let Some(image) = render_frame(frame) {
                    if let Some(old) = self.image.replace(image) {
                        images::release(old);
                    }
                }
            }
            if let Some(bus) = self.pipeline.bus() {
                while let Some(message) = bus.pop() {
                    match message.view() {
                        gst::MessageView::Error(e) => {
                            self.error = Some(tr!(
                                "動画を再生できません: {}",
                                "Can't play the video: {}",
                                "无法播放视频：{}",
                                e.error()
                            ));
                            self.paused = true;
                            let _ = self.pipeline.set_state(gst::State::Null);
                        }
                        gst::MessageView::Eos(..) => {
                            self.ended = true;
                            self.paused = true;
                        }
                        _ => (),
                    }
                }
            }
        }
        pub fn toggle(&mut self) {
            if self.error.is_some() {
                return;
            }
            if self.ended {
                if self
                    .pipeline
                    .seek_simple(
                        gst::SeekFlags::FLUSH | gst::SeekFlags::KEY_UNIT,
                        gst::ClockTime::ZERO,
                    )
                    .is_err()
                {
                    self.error = Some(
                        tr!(
                            "動画を先頭に戻せませんでした",
                            "Couldn't rewind the video",
                            "无法将视频倒回开头"
                        )
                        .into(),
                    );
                    return;
                }
                self.ended = false;
            }
            self.paused = !self.paused;
            if self
                .pipeline
                .set_state(if self.paused {
                    gst::State::Paused
                } else {
                    gst::State::Playing
                })
                .is_err()
            {
                self.error = Some(
                    tr!(
                        "動画の再生状態を変更できませんでした",
                        "Couldn't change the video's playback state",
                        "无法更改视频的播放状态"
                    )
                    .into(),
                );
            }
        }
    }
    impl Drop for Player {
        fn drop(&mut self) {
            let _ = self.pipeline.set_state(gst::State::Null);
            if let Some(img) = self.image.take() {
                images::release(img);
            }
        }
    }
    fn dimensions(width: u32, height: u32) -> (u32, u32) {
        let (width, height) = if width == 0 || height == 0 {
            (640, 360)
        } else {
            (width, height)
        };
        let scale = (840.0 / width as f64).min(640.0 / height as f64).min(1.0);
        (
            ((width as f64 * scale) as u32).clamp(1, 840),
            ((height as f64 * scale) as u32).clamp(1, 640),
        )
    }
    #[cfg(test)]
    mod tests {
        use super::*;
        #[test]
        fn plays_real_video_and_can_pause() {
            let path =
                std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/video.mp4");
            let url = reqwest::Url::from_file_path(path).unwrap().to_string();
            let mut player =
                Player::new_inner("channel".into(), url, 64, 48, Waker::default()).unwrap();
            let deadline = std::time::Instant::now() + Duration::from_secs(8);
            while player.image.is_none()
                && player.error.is_none()
                && std::time::Instant::now() < deadline
            {
                std::thread::sleep(Duration::from_millis(20));
                player.poll();
            }
            assert!(player.error.is_none(), "{:?}", player.error);
            assert!(player.image.is_some(), "動画フレームが届きませんでした");
            player.toggle();
            assert!(player.paused);
            player.toggle();
            assert!(!player.paused);
        }
        #[test]
        fn streams_video_over_http() {
            use std::io::{Read, Write};
            use std::sync::atomic::{AtomicBool, Ordering};
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            listener.set_nonblocking(true).unwrap();
            let address = listener.local_addr().unwrap();
            let stop = Arc::new(AtomicBool::new(false));
            let stop_server = stop.clone();
            let server = std::thread::spawn(move || {
                let data = include_bytes!("../tests/fixtures/video.mp4");
                while !stop_server.load(Ordering::Relaxed) {
                    match listener.accept() {
                        Ok((mut stream, _)) => {
                            let _ = stream.set_read_timeout(Some(Duration::from_secs(1)));
                            let mut request = [0u8; 4096];
                            let Ok(n) = stream.read(&mut request) else {
                                continue;
                            };
                            let request = String::from_utf8_lossy(&request[..n]);
                            let start = request
                                .lines()
                                .find_map(|line| {
                                    line.to_ascii_lowercase()
                                        .strip_prefix("range: bytes=")
                                        .and_then(|value| value.split('-').next())
                                        .and_then(|value| value.parse::<usize>().ok())
                                })
                                .unwrap_or(0)
                                .min(data.len());
                            let partial =
                                start > 0 || request.to_ascii_lowercase().contains("range: bytes=");
                            let status = if partial {
                                "206 Partial Content"
                            } else {
                                "200 OK"
                            };
                            let range = if partial {
                                format!(
                                    "Content-Range: bytes {}-{}/{}\r\n",
                                    start,
                                    data.len() - 1,
                                    data.len()
                                )
                            } else {
                                String::new()
                            };
                            let header = format!("HTTP/1.1 {status}\r\nContent-Type: video/mp4\r\nContent-Length: {}\r\nAccept-Ranges: bytes\r\n{range}Connection: close\r\n\r\n", data.len() - start);
                            let _ = stream.write_all(header.as_bytes());
                            if !request.starts_with("HEAD ") {
                                let _ = stream.write_all(&data[start..]);
                            }
                        }
                        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                            std::thread::sleep(Duration::from_millis(5))
                        }
                        Err(_) => break,
                    }
                }
            });
            let url = format!("http://{address}/video.mp4");
            let awakened = Arc::new(AtomicBool::new(false));
            let wake_flag = awakened.clone();
            let waker = Waker::new(move |_| {
                wake_flag.store(true, Ordering::Relaxed);
            });
            assert!(thumbnail(&url, 64, 48, &waker).is_none());
            let until = std::time::Instant::now() + Duration::from_secs(8);
            let mut preview = None;
            while preview.is_none() && std::time::Instant::now() < until {
                std::thread::sleep(Duration::from_millis(20));
                preview = thumbnail(&url, 64, 48, &waker);
            }
            assert!(preview.is_some(), "HTTP 動画のサムネイルが届きませんでした");
            assert!(awakened.load(Ordering::Relaxed));
            let mut player = Player::new("channel".into(), url, 64, 48, Waker::default()).unwrap();
            let deadline = std::time::Instant::now() + Duration::from_secs(8);
            while player.image.is_none()
                && player.error.is_none()
                && std::time::Instant::now() < deadline
            {
                std::thread::sleep(Duration::from_millis(20));
                player.poll();
            }
            let error = player.error.clone();
            let has_image = player.image.is_some();
            drop(player);
            stop.store(true, Ordering::Relaxed);
            server.join().unwrap();
            assert!(error.is_none(), "{:?}", error);
            assert!(has_image, "HTTP 動画フレームが届きませんでした");
        }
        #[test]
        fn detects_video_attachments() {
            let attachment = |name: &str, content_type: Option<&str>| crate::model::Attachment {
                filename: name.into(),
                url: "https://example.com/a".into(),
                proxy_url: None,
                content_type: content_type.map(str::to_owned),
                width: None,
                height: None,
                size: None,
            };
            assert!(attachment("clip.mp4", Some("video/mp4")).is_video());
            assert!(attachment("CLIP.WEBM", None).is_video());
            assert!(attachment("clip.mp4", Some("application/octet-stream")).is_video());
            assert!(!attachment("image.png", Some("image/png")).is_video());
            assert!(!attachment("clip.mp4", Some("text/plain")).is_video());
        }
        #[test]
        fn bounded_dimensions() {
            assert_eq!(dimensions(1920, 1080), (840, 472));
            assert_eq!(dimensions(0, 0), (640, 360));
            assert_eq!(dimensions(100, 100), (100, 100));
        }
    }
}
#[cfg(not(target_os = "linux"))]
mod platform {
    use crate::wake::Waker;
    use gpui::RenderImage;
    pub fn thumbnail(_: &str, _: u32, _: u32, _: &Waker) -> Option<Arc<RenderImage>> {
        None
    }
    use std::sync::Arc;
    pub struct Player {
        pub channel: String,
        pub url: String,
        pub image: Option<Arc<RenderImage>>,
        pub error: Option<String>,
        pub paused: bool,
        pub ended: bool,
    }
    impl Player {
        pub fn new(_: String, _: String, _: u32, _: u32, _: Waker) -> Result<Self, String> {
            Err(tr!(
                "この OS では動画の埋め込み再生に対応していません",
                "Embedded video playback isn't supported on this OS",
                "此操作系统不支持内嵌播放视频"
            )
            .into())
        }
        pub fn poll(&mut self) {}
        pub fn toggle(&mut self) {}
    }
}
pub use platform::{thumbnail, Player};
