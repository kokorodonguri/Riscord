//! GIF / アニメーション WebP を UI スレッドの外でデコードする ImageLoader。
//! egui_extras のローダーは描画中に全フレームをその場でデコードするため、大きな GIF があると
//! 数秒間画面が止まる（Hyprland などでは「応答なし」になる）。ここでは専用スレッドでデコードし、
//! 終わるまでは読み込み中として扱う。大きすぎるものは縮小・フレームの間引きをしてメモリも抑える
use eframe::egui::{
    self, decode_animated_image_uri, has_gif_magic_header, has_webp_header,
    load::{BytesPoll, ImageLoadResult, ImageLoader, ImagePoll, LoadError, SizeHint},
    mutex::Mutex,
    ColorImage, FrameDurations, Id,
};
use image::{codecs::gif::GifDecoder, codecs::webp::WebPDecoder, AnimationDecoder as _, ImageDecoder as _, RgbaImage};
use std::{collections::HashMap, io::Cursor, sync::mpsc, sync::Arc, sync::OnceLock, time::Duration};

/// 1フレームの最大画素数。表示は最大 420×320（高DPI で倍）なので、これより大きいものは縮める
const MAX_FRAME_PIXELS: u32 = 840 * 640;
/// 1枚のアニメーションに使うメモリの上限。超えるならフレームを間引く
const MAX_BYTES: usize = 64 * 1024 * 1024;
/// デコードするスレッドの数（CPU を食い尽くさないよう少なめ）
const WORKERS: usize = 2;

struct Anim {
    frames: Vec<Arc<ColorImage>>,
}

enum Entry {
    Pending,
    Ready(Arc<Anim>),
    Failed(String),
}

type Cache = Arc<Mutex<HashMap<String, Entry>>>;

struct Job {
    uri: String,
    bytes: Arc<[u8]>,
    gif: bool,
    ctx: egui::Context,
    cache: Cache,
}

#[derive(Default)]
pub struct AnimLoader {
    cache: Cache,
    queue: OnceLock<Mutex<mpsc::Sender<Job>>>,
}

impl AnimLoader {
    pub const ID: &'static str = egui::generate_loader_id!(AnimLoader);

    fn submit(&self, job: Job) {
        let queue = self.queue.get_or_init(|| {
            let (tx, rx) = mpsc::channel::<Job>();
            let rx = Arc::new(std::sync::Mutex::new(rx));
            for i in 0..WORKERS {
                let rx = rx.clone();
                std::thread::Builder::new()
                    .name(format!("anim-decode-{i}"))
                    .spawn(move || loop {
                        let job = match rx.lock().unwrap().recv() {
                            Ok(j) => j,
                            Err(_) => return,
                        };
                        run(job);
                    })
                    .expect("decode thread");
            }
            Mutex::new(tx)
        });
        let _ = queue.lock().send(job);
    }
}

fn run(job: Job) {
    // 待っている間に捨てられた（チャンネルを閉じた等）ならデコードしない
    if !matches!(job.cache.lock().get(&job.uri), Some(Entry::Pending)) {
        return;
    }
    let result = decode(&job.bytes, job.gif);
    let mut cache = job.cache.lock();
    if !matches!(cache.get(&job.uri), Some(Entry::Pending)) {
        return;
    }
    match result {
        Ok((frames, durations)) => {
            if durations.len() > 1 {
                job.ctx.data_mut(|d| d.insert_temp(Id::new(job.uri.as_str()), FrameDurations::new(durations)));
            }
            cache.insert(job.uri, Entry::Ready(Arc::new(Anim { frames })));
        }
        Err(e) => {
            cache.insert(job.uri, Entry::Failed(e));
        }
    }
    drop(cache);
    job.ctx.request_repaint();
}

/// ブラウザと同じく、極端に短い表示時間（0〜10ms）は 100ms とみなす
fn frame_delay(d: Duration) -> Duration {
    if d < Duration::from_millis(20) {
        Duration::from_millis(100)
    } else {
        d
    }
}

fn shrink(img: RgbaImage) -> RgbaImage {
    let (w, h) = img.dimensions();
    if w * h <= MAX_FRAME_PIXELS {
        return img;
    }
    let k = (MAX_FRAME_PIXELS as f64 / (w as f64 * h as f64)).sqrt();
    let (nw, nh) = (((w as f64 * k) as u32).max(1), ((h as f64 * k) as u32).max(1));
    image::imageops::resize(&img, nw, nh, image::imageops::FilterType::Triangle)
}

fn to_color(img: &RgbaImage) -> Arc<ColorImage> {
    Arc::new(ColorImage::from_rgba_unmultiplied([img.width() as usize, img.height() as usize], img.as_raw()))
}

type Decoded = (Vec<Arc<ColorImage>>, Vec<Duration>);

/// 全フレームをデコードする。メモリの上限を超えそうなら、表示時間を足し合わせながらフレームを間引く
pub fn decode(bytes: &[u8], gif: bool) -> Result<Decoded, String> {
    let (frames, (w, h)): (image::Frames<'_>, (u32, u32)) = if gif {
        let d = GifDecoder::new(Cursor::new(bytes)).map_err(|e| format!("GIF decode failure ({e})"))?;
        let dims = d.dimensions();
        (d.into_frames(), dims)
    } else {
        let mut d = WebPDecoder::new(Cursor::new(bytes)).map_err(|e| format!("WebP decode failure ({e})"))?;
        let dims = d.dimensions();
        if !d.has_animation() {
            let img = image::DynamicImage::from_decoder(d).map_err(|e| format!("WebP decode failure ({e})"))?.to_rgba8();
            return Ok((vec![to_color(&shrink(img))], vec![Duration::ZERO]));
        }
        d.set_background_color(image::Rgba([0, 0, 0, 0])).map_err(|e| format!("WebP decode failure ({e})"))?;
        (d.into_frames(), dims)
    };
    let frame_bytes = (w.max(1) as usize * h.max(1) as usize).min(MAX_FRAME_PIXELS as usize) * 4;
    let max_frames = (MAX_BYTES / frame_bytes).max(1);
    let mut images = Vec::new();
    let mut durations: Vec<Duration> = Vec::new();
    // 最初は全部残し、上限に達したら1つおきに減らして間隔を倍にする（総フレーム数を先に知らなくて済む）
    let mut step = 1usize;
    for (i, f) in frames.enumerate() {
        let f = f.map_err(|e| format!("frame decode failure ({e})"))?;
        let delay = frame_delay(f.delay().into());
        if i % step == 0 {
            images.push(to_color(&shrink(f.into_buffer())));
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
    Ok((images, durations))
}

impl ImageLoader for AnimLoader {
    fn id(&self) -> &str {
        Self::ID
    }

    fn load(&self, ctx: &egui::Context, frame_uri: &str, _: SizeHint) -> ImageLoadResult {
        let (uri, index) = decode_animated_image_uri(frame_uri).map_err(|_| LoadError::NotSupported)?;
        if let Some(e) = self.cache.lock().get(uri) {
            return match e {
                Entry::Ready(a) => Ok(ImagePoll::Ready { image: a.frames[index % a.frames.len()].clone() }),
                Entry::Pending => Ok(ImagePoll::Pending { size: None }),
                Entry::Failed(err) => Err(LoadError::Loading(err.clone())),
            };
        }
        match ctx.try_load_bytes(uri) {
            Ok(BytesPoll::Ready { bytes, .. }) => {
                let gif = has_gif_magic_header(&bytes);
                if !gif && !has_webp_header(&bytes) {
                    // 拡張子だけ .gif / .webp で中身が別形式なら、他のローダーに任せる
                    return Err(LoadError::NotSupported);
                }
                self.cache.lock().insert(uri.to_string(), Entry::Pending);
                let bytes: Arc<[u8]> = Arc::from(&bytes[..]);
                self.submit(Job { uri: uri.to_string(), bytes, gif, ctx: ctx.clone(), cache: self.cache.clone() });
                Ok(ImagePoll::Pending { size: None })
            }
            Ok(BytesPoll::Pending { size }) => Ok(ImagePoll::Pending { size }),
            Err(e) => Err(e),
        }
    }

    fn forget(&self, uri: &str) {
        let base = decode_animated_image_uri(uri).map_or(uri, |(u, _)| u);
        self.cache.lock().remove(base);
    }

    fn forget_all(&self) {
        self.cache.lock().clear();
    }

    fn byte_size(&self) -> usize {
        self.cache
            .lock()
            .values()
            .map(|e| match e {
                Entry::Ready(a) => a.frames.iter().map(|f| f.pixels.len() * 4).sum(),
                _ => 0,
            })
            .sum()
    }
}
