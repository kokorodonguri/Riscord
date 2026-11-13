//! 画面の取り込み（xdg-desktop-portal + PipeWire）と H264 のエンコード・デコード。
//! Linux は GStreamer を使う。ほかの OS ではまだ使えない。

use super::Frame;

/// エンコード済みの 1 フレーム（Annex B のアクセスユニット）
pub struct Encoded {
    pub data: Vec<u8>,
    /// 90kHz の時刻
    pub rtp_ts: u32,
    pub keyframe: bool,
}

/// 画面共有を始める前に選ぶ画質。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum QualityPreset {
    P360_30,
    P360_60,
    P720_60,
    P1080_30,
    P1080_60,
    #[default]
    #[serde(other)]
    P720_30,
}

impl QualityPreset {
    pub fn fit(self, source: Option<(i32, i32)>) -> Quality {
        let (width, height, fps) = match self {
            Self::P360_30 => (640, 360, 30),
            Self::P360_60 => (640, 360, 60),
            Self::P720_30 => (1280, 720, 30),
            Self::P720_60 => (1280, 720, 60),
            Self::P1080_30 => (1920, 1080, 30),
            Self::P1080_60 => (1920, 1080, 60),
        };
        Quality::fit(source, width, height, fps)
    }
}

/// 配信の画質
#[derive(Clone, Copy, Debug)]
pub struct Quality {
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub bitrate: u32,
}

impl Quality {
    /// 元の大きさを、縦横比を保って max_w×max_h に収める（偶数にそろえる）
    pub fn fit(src: Option<(i32, i32)>, max_w: u32, max_h: u32, fps: u32) -> Quality {
        let (sw, sh) = src
            .filter(|(w, h)| *w > 0 && *h > 0)
            .map(|(w, h)| (w as f64, h as f64))
            .unwrap_or((1920.0, 1080.0));
        let s = (max_w as f64 / sw).min(max_h as f64 / sh).min(1.0);
        let even = |v: f64| ((v as u32) & !1).max(16);
        let (width, height) = (even(sw * s), even(sh * s));
        // 720p30 で 2.5Mbps 程度。画素数に比例させる
        let bitrate = ((width * height) as f64 * fps as f64 * 0.09) as u32;
        Quality {
            width,
            height,
            fps,
            bitrate: bitrate.clamp(500_000, 6_000_000),
        }
    }
}

#[cfg(target_os = "linux")]
mod platform {
    use super::{Encoded, Frame, Quality};
    use crate::wake::Waker;
    use gst::prelude::*;
    use gstreamer as gst;
    use gstreamer_app as gst_app;
    use gstreamer_video as gst_video;
    use std::os::fd::{AsRawFd, OwnedFd};
    use std::sync::{Arc, Mutex};

    /// ポータルで選んだ画面・ウィンドウ
    pub struct Source {
        fd: OwnedFd,
        node: u32,
        pub size: Option<(i32, i32)>,
        session: Option<
            ashpd::desktop::Session<'static, ashpd::desktop::screencast::Screencast<'static>>,
        >,
    }

    impl Drop for Source {
        fn drop(&mut self) {
            // ポータル側の「共有中」を終わらせる
            if let Some(s) = self.session.take() {
                if let Ok(rt) = tokio::runtime::Handle::try_current() {
                    rt.spawn(async move {
                        let _ = s.close().await;
                    });
                }
            }
        }
    }

    /// 共有する画面を選んでもらう（ポータルのダイアログが出る）
    pub async fn pick_source() -> Result<Source, String> {
        use ashpd::desktop::screencast::{CursorMode, Screencast, SourceType};
        use ashpd::desktop::PersistMode;
        let err = |e: ashpd::Error| match e {
            ashpd::Error::Response(ashpd::desktop::ResponseError::Cancelled) => tr!(
                "画面の選択が取り消されました",
                "Screen selection was canceled"
            )
            .to_string(),
            e => tr!(
                "画面共有のポータルを使えません: {e}",
                "Can't use the screen sharing portal: {e}",
            ),
        };
        let proxy = Screencast::new().await.map_err(err)?;
        let session = proxy.create_session().await.map_err(err)?;
        let cursor = match proxy.available_cursor_modes().await {
            Ok(m) if m.contains(CursorMode::Embedded) => CursorMode::Embedded,
            _ => CursorMode::Hidden,
        };
        let types = proxy
            .available_source_types()
            .await
            .unwrap_or(SourceType::Monitor.into());
        proxy
            .select_sources(
                &session,
                cursor,
                types & (SourceType::Monitor | SourceType::Window),
                false,
                None,
                PersistMode::DoNot,
            )
            .await
            .map_err(err)?;
        let streams = proxy
            .start(&session, None)
            .await
            .map_err(err)?
            .response()
            .map_err(err)?;
        let stream = streams.streams().first().ok_or(tr!(
            "共有する画面が選ばれませんでした",
            "No screen was chosen to share"
        ))?;
        let node = stream.pipe_wire_node_id();
        let size = stream.size();
        let fd = proxy.open_pipe_wire_remote(&session).await.map_err(err)?;
        Ok(Source {
            fd,
            node,
            size,
            session: Some(session),
        })
    }

    fn have(name: &str) -> bool {
        gst::ElementFactory::find(name).is_some()
    }

    fn encoder(q: &Quality) -> Result<String, String> {
        let gop = q.fps * 2;
        if have("x264enc") {
            Ok(format!(
                "x264enc tune=zerolatency speed-preset=ultrafast bitrate={} key-int-max={gop} bframes=0 ! video/x-h264,profile=constrained-baseline",
                q.bitrate / 1000
            ))
        } else if have("openh264enc") {
            Ok(format!(
                "openh264enc bitrate={} gop-size={gop} complexity=low usage-type=screen",
                q.bitrate
            ))
        } else {
            Err(tr!("H264 のエンコーダーが見つかりません（gst-plugins-ugly の x264enc か、gst-plugins-bad の openh264enc を入れてください）", "No H264 encoder found (install x264enc from gst-plugins-ugly or openh264enc from gst-plugins-bad)").into())
        }
    }

    /// 画面を取り込んで H264 にする
    pub struct Capture {
        pipeline: gst::Element,
        sink: gst_app::AppSink,
        _source: Option<Source>,
    }

    impl Capture {
        pub fn start(
            source: Source,
            q: Quality,
            out: tokio::sync::mpsc::Sender<Encoded>,
        ) -> Result<Capture, String> {
            let src = format!(
                "pipewiresrc fd={} path={} do-timestamp=true keepalive-time=500 always-copy=true",
                source.fd.as_raw_fd(),
                source.node
            );
            Self::with_source(&src, Some(source), q, out)
        }

        /// テスト用などに、任意の映像元から作る
        pub fn with_source(
            src: &str,
            source: Option<Source>,
            q: Quality,
            out: tokio::sync::mpsc::Sender<Encoded>,
        ) -> Result<Capture, String> {
            gst::init().map_err(|e| {
                tr!(
                    "GStreamer を初期化できません: {e}",
                    "Can't initialize GStreamer: {e}",
                )
            })?;
            let desc = format!(
                "{src} ! queue leaky=downstream max-size-buffers=2 ! videoconvert ! videoscale ! videorate ! video/x-raw,format=I420,width={w},height={h},framerate={fps}/1,pixel-aspect-ratio=1/1 ! {enc} ! h264parse config-interval=-1 ! video/x-h264,stream-format=byte-stream,alignment=au ! appsink name=sink max-buffers=8 drop=true sync=false",
                w = q.width,
                h = q.height,
                fps = q.fps,
                enc = encoder(&q)?,
            );
            let pipeline = gst::parse::launch(&desc).map_err(|e| {
                tr!(
                    "映像の処理を組み立てられません: {e}",
                    "Can't build the video pipeline: {e}",
                )
            })?;
            let bin = pipeline
                .clone()
                .downcast::<gst::Bin>()
                .map_err(|_| tr!("パイプラインが不正です", "Invalid pipeline"))?;
            let sink = bin
                .by_name("sink")
                .and_then(|s| s.downcast::<gst_app::AppSink>().ok())
                .ok_or(tr!("appsink がありません", "Missing appsink"))?;
            let offset: u32 = rand::random();
            sink.set_callbacks(
                gst_app::AppSinkCallbacks::builder()
                    .new_sample(move |sink| {
                        let sample = sink.pull_sample().map_err(|_| gst::FlowError::Eos)?;
                        let Some(buf) = sample.buffer() else {
                            return Ok(gst::FlowSuccess::Ok);
                        };
                        let Ok(map) = buf.map_readable() else {
                            return Ok(gst::FlowSuccess::Ok);
                        };
                        let ns = buf.pts().map(|t| t.nseconds()).unwrap_or(0);
                        let rtp_ts =
                            ((ns as u128 * 90_000 / 1_000_000_000) as u32).wrapping_add(offset);
                        let keyframe = !buf.flags().contains(gst::BufferFlags::DELTA_UNIT);
                        // 送るのが追いつかないときは捨てる（遅延をためない）
                        let _ = out.try_send(Encoded {
                            data: map.to_vec(),
                            rtp_ts,
                            keyframe,
                        });
                        Ok(gst::FlowSuccess::Ok)
                    })
                    .build(),
            );
            pipeline.set_state(gst::State::Playing).map_err(|e| {
                tr!(
                    "画面の取り込みを始められません: {e}",
                    "Can't start screen capture: {e}",
                )
            })?;
            Ok(Capture {
                pipeline,
                sink,
                _source: source,
            })
        }

        /// 次のフレームをキーフレームにする（視聴者が増えた・PLI が来た）
        pub fn force_keyframe(&self) {
            let ev = gst_video::UpstreamForceKeyUnitEvent::builder()
                .all_headers(true)
                .build();
            self.sink.send_event(ev);
        }

        /// 取り込みが止まったら理由を返す
        pub fn error(&self) -> Option<String> {
            let bus = self.pipeline.bus()?;
            while let Some(m) = bus.pop() {
                match m.view() {
                    gst::MessageView::Error(e) => {
                        return Some(tr!(
                            "画面の取り込みが止まりました: {}",
                            "Screen capture stopped: {}",
                            e.error()
                        ))
                    }
                    gst::MessageView::Eos(..) => {
                        return Some(
                            tr!(
                                "共有していた画面が閉じられました",
                                "The shared screen was closed"
                            )
                            .into(),
                        )
                    }
                    _ => {}
                }
            }
            None
        }
    }

    impl Drop for Capture {
        fn drop(&mut self) {
            let _ = self.pipeline.set_state(gst::State::Null);
        }
    }

    /// 受け取った H264 を BGRA の画像にする
    pub struct Decoder {
        pipeline: gst::Element,
        src: gst_app::AppSrc,
    }

    impl Decoder {
        pub fn new(latest: Arc<Mutex<Option<Frame>>>, waker: Waker) -> Result<Decoder, String> {
            gst::init().map_err(|e| {
                tr!(
                    "GStreamer を初期化できません: {e}",
                    "Can't initialize GStreamer: {e}",
                )
            })?;
            let dec = ["avdec_h264", "openh264dec", "v4l2h264dec"]
                .into_iter()
                .find(|d| have(d))
                .ok_or(tr!(
                    "H264 のデコーダーが見つかりません（gst-libav か openh264 を入れてください）",
                    "No H264 decoder found (install gst-libav or openh264)"
                ))?;
            let desc = format!(
                "appsrc name=src is-live=true do-timestamp=true format=time caps=video/x-h264,stream-format=byte-stream,alignment=au ! h264parse ! {dec} ! videoconvert ! videoscale ! video/x-raw,format=BGRA,width=[16,1920],height=[16,1080],pixel-aspect-ratio=1/1 ! appsink name=sink max-buffers=1 drop=true sync=false"
            );
            let pipeline = gst::parse::launch(&desc).map_err(|e| {
                tr!(
                    "映像の処理を組み立てられません: {e}",
                    "Can't build the video pipeline: {e}",
                )
            })?;
            let bin = pipeline
                .clone()
                .downcast::<gst::Bin>()
                .map_err(|_| tr!("パイプラインが不正です", "Invalid pipeline"))?;
            let src = bin
                .by_name("src")
                .and_then(|s| s.downcast::<gst_app::AppSrc>().ok())
                .ok_or(tr!("appsrc がありません", "Missing appsrc"))?;
            let sink = bin
                .by_name("sink")
                .and_then(|s| s.downcast::<gst_app::AppSink>().ok())
                .ok_or(tr!("appsink がありません", "Missing appsink"))?;
            sink.set_callbacks(
                gst_app::AppSinkCallbacks::builder()
                    .new_sample(move |sink| {
                        let sample = sink.pull_sample().map_err(|_| gst::FlowError::Eos)?;
                        if let Some(f) = frame_from_sample(&sample) {
                            *latest.lock().unwrap() = Some(f);
                            waker.wake();
                        }
                        Ok(gst::FlowSuccess::Ok)
                    })
                    .build(),
            );
            pipeline
                .set_state(gst::State::Playing)
                .map_err(|e| tr!("映像を再生できません: {e}", "Can't play the video: {e}",))?;
            Ok(Decoder { pipeline, src })
        }

        pub fn push(&self, au: Vec<u8>) {
            let _ = self.src.push_buffer(gst::Buffer::from_mut_slice(au));
        }

        pub fn error(&self) -> Option<String> {
            let bus = self.pipeline.bus()?;
            while let Some(m) = bus.pop() {
                if let gst::MessageView::Error(e) = m.view() {
                    return Some(tr!(
                        "映像を再生できません: {}",
                        "Can't play the video: {}",
                        e.error()
                    ));
                }
            }
            None
        }
    }

    impl Drop for Decoder {
        fn drop(&mut self) {
            let _ = self.src.end_of_stream();
            let _ = self.pipeline.set_state(gst::State::Null);
        }
    }

    fn frame_from_sample(sample: &gst::Sample) -> Option<Frame> {
        use gst_video::VideoFrameExt;
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

    #[cfg(test)]
    mod tests {
        use super::*;
        use crate::golive::rtp;

        /// テスト画像をエンコード → RTP に分けて暗号化 → 組み立て → デコード まで通す
        #[test]
        fn encode_packetize_decode_round_trip() {
            if gst::init().is_err()
                || !(have("x264enc") || have("openh264enc"))
                || !(have("avdec_h264") || have("openh264dec"))
                || !have("videotestsrc")
            {
                eprintln!("GStreamer のプラグインが足りないので飛ばします");
                return;
            }
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            rt.block_on(async {
                let q = Quality::fit(Some((640, 360)), 640, 360, 15);
                let (tx, mut rx) = tokio::sync::mpsc::channel(32);
                let cap =
                    Capture::with_source("videotestsrc is-live=true num-buffers=30", None, q, tx)
                        .unwrap();
                let latest = Arc::new(Mutex::new(None));
                let dec = Decoder::new(latest.clone(), Waker::default()).unwrap();
                let cipher = rtp::Cipher::new(rtp::MODE_AES, &[3u8; 32]).unwrap();
                let mut pk = rtp::Packetizer::new(5);
                let mut dp = rtp::Depacketizer::default();
                let mut frames = 0;
                let mut first_key = None;
                while let Ok(Some(e)) =
                    tokio::time::timeout(std::time::Duration::from_secs(10), rx.recv()).await
                {
                    first_key.get_or_insert(e.keyframe);
                    for p in pk.packetize(&cipher, &e.data, e.rtp_ts) {
                        let r = rtp::parse_rtp(&p).unwrap();
                        if let rtp::Push::Frame(au) =
                            dp.push(r.seq, r.timestamp, r.marker, &r.decrypt(&cipher).unwrap())
                        {
                            dec.push(au.data);
                            frames += 1;
                        }
                    }
                    if frames >= 10 {
                        break;
                    }
                }
                drop(cap);
                assert_eq!(first_key, Some(true), "最初のフレームはキーフレーム");
                assert!(frames >= 10);
                for _ in 0..50 {
                    if latest.lock().unwrap().is_some() {
                        break;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(100));
                }
                assert!(dec.error().is_none(), "{:?}", dec.error());
                let f = latest.lock().unwrap().take().expect("デコードされた画像");
                assert_eq!((f.width, f.height), (640, 360));
                assert_eq!(f.data.len(), 640 * 360 * 4);
            });
        }
    }
}

#[cfg(not(target_os = "linux"))]
mod platform {
    use super::{Encoded, Frame, Quality};
    use crate::wake::Waker;
    use std::sync::{Arc, Mutex};

    const UNSUPPORTED: &str = tr!(
        "この OS では画面共有にまだ対応していません",
        "Screen sharing isn't supported on this OS yet"
    );

    pub struct Source {
        pub size: Option<(i32, i32)>,
    }
    pub async fn pick_source() -> Result<Source, String> {
        Err(UNSUPPORTED.into())
    }
    pub struct Capture;
    impl Capture {
        pub fn start(
            _: Source,
            _: Quality,
            _: tokio::sync::mpsc::Sender<Encoded>,
        ) -> Result<Capture, String> {
            Err(UNSUPPORTED.into())
        }
        pub fn force_keyframe(&self) {}
        pub fn error(&self) -> Option<String> {
            None
        }
    }
    pub struct Decoder;
    impl Decoder {
        pub fn new(_: Arc<Mutex<Option<Frame>>>, _: Waker) -> Result<Decoder, String> {
            Err(tr!(
                "この OS では配信の視聴にまだ対応していません",
                "Watching streams isn't supported on this OS yet"
            )
            .into())
        }
        pub fn push(&self, _: Vec<u8>) {}
        pub fn error(&self) -> Option<String> {
            None
        }
    }
}

pub use platform::*;
