//! スタンプの取得とデコード（PNG / APNG / GIF / Lottie）。すべてRustのみで処理する。

use std::io::Cursor;

use anyhow::{bail, Result};
use image::{
    codecs::gif::GifDecoder, codecs::png::PngDecoder, AnimationDecoder, DynamicImage, RgbaImage,
};
use velato::{model::fixed, Composition, RenderSink, Renderer};
use vello_cpu::kurbo::{Affine, BezPath, Shape};
use vello_cpu::peniko::{BlendMode, Brush};
use vello_cpu::{Pixmap, RenderContext, Resources};

/// 保存しておく最大サイズ（px）とコマ数。メモリを食いすぎないように抑える
const MAX_PX: u32 = 200;
const MAX_FRAMES: usize = 120;
const LOTTIE_FPS: f64 = 30.0;

pub const LOTTIE: u8 = 3;
pub const GIF: u8 = 4;

pub struct Frames {
    pub size: [usize; 2],
    /// RGBA（非乗算）とそのコマの表示時間(ms)
    pub frames: Vec<(Vec<u8>, u32)>,
}

pub fn url(id: &str, format: u8) -> String {
    match format {
        LOTTIE => format!("https://cdn.discordapp.com/stickers/{id}.json"),
        GIF => format!("https://media.discordapp.net/stickers/{id}.gif"),
        // APNGはプロキシを通すと静止画になるのでCDNから原本を取る
        _ => format!("https://cdn.discordapp.com/stickers/{id}.png"),
    }
}

pub fn decode(format: u8, bytes: &[u8]) -> Result<Frames> {
    match format {
        LOTTIE => decode_lottie(bytes),
        GIF => from_anim(GifDecoder::new(Cursor::new(bytes))?.into_frames()),
        _ => {
            let dec = PngDecoder::new(Cursor::new(bytes))?;
            if dec.is_apng()? {
                from_anim(dec.apng()?.into_frames())
            } else {
                let img = DynamicImage::from_decoder(dec)?.to_rgba8();
                Ok(single(shrink(img)))
            }
        }
    }
}

fn shrink(img: RgbaImage) -> RgbaImage {
    let (w, h) = img.dimensions();
    if w <= MAX_PX && h <= MAX_PX {
        return img;
    }
    let s = MAX_PX as f32 / w.max(h) as f32;
    let (nw, nh) = (
        ((w as f32 * s) as u32).max(1),
        ((h as f32 * s) as u32).max(1),
    );
    image::imageops::resize(&img, nw, nh, image::imageops::FilterType::Triangle)
}

fn single(img: RgbaImage) -> Frames {
    let size = [img.width() as usize, img.height() as usize];
    Frames {
        size,
        frames: vec![(img.into_raw(), 0)],
    }
}

fn from_anim(frames: image::Frames<'_>) -> Result<Frames> {
    let mut out = Vec::new();
    let mut size = [0, 0];
    for f in frames.take(MAX_FRAMES) {
        let f = f?;
        let (n, d) = f.delay().numer_denom_ms();
        let ms = if d == 0 { 100 } else { (n / d).max(20) };
        let img = shrink(f.into_buffer());
        size = [img.width() as usize, img.height() as usize];
        out.push((img.into_raw(), ms));
    }
    if out.is_empty() {
        bail!("{}", tr!("コマがありません", "No frames", "没有帧",));
    }
    Ok(Frames { size, frames: out })
}

// ---- Lottie: velatoで解釈し、vello_cpuでCPU描画 ----

struct Sink<'a> {
    ctx: &'a mut RenderContext,
}

impl RenderSink for Sink<'_> {
    fn push_layer(
        &mut self,
        blend: impl Into<BlendMode>,
        alpha: f32,
        transform: Affine,
        shape: &impl Shape,
    ) {
        self.ctx.set_transform(Affine::IDENTITY);
        let p: BezPath = transform * shape.to_path(0.1);
        self.ctx
            .push_layer(Some(&p), Some(blend.into()), Some(alpha), None, None);
    }

    fn push_clip_layer(&mut self, transform: Affine, shape: &impl Shape) {
        self.ctx.set_transform(Affine::IDENTITY);
        let p: BezPath = transform * shape.to_path(0.1);
        self.ctx.push_clip_layer(&p);
    }

    fn pop_layer(&mut self) {
        self.ctx.pop_layer();
    }

    fn draw(
        &mut self,
        stroke: Option<&fixed::Stroke>,
        transform: Affine,
        brush: &fixed::Brush,
        shape: &impl Shape,
    ) {
        match brush {
            Brush::Solid(c) => self.ctx.set_paint(*c),
            Brush::Gradient(g) => self.ctx.set_paint(g.clone()),
            _ => return,
        }
        self.ctx.set_transform(transform);
        let p = shape.to_path(0.1);
        match stroke {
            Some(s) => {
                self.ctx.set_stroke(s.clone());
                self.ctx.stroke_path(&p);
            }
            None => self.ctx.fill_path(&p),
        }
    }

    fn draw_image(&mut self, _: &velato::model::ImageAsset, _: Affine, _: f64) {}
}

fn decode_lottie(bytes: &[u8]) -> Result<Frames> {
    let comp = Composition::from_slice(bytes)?;
    let (cw, ch) = (comp.width.max(1) as f64, comp.height.max(1) as f64);
    let scale = MAX_PX as f64 / cw.max(ch);
    let (w, h) = ((cw * scale).round() as u16, (ch * scale).round() as u16);

    let fps = if comp.frame_rate > 0.0 {
        comp.frame_rate
    } else {
        60.0
    };
    let step = (fps / LOTTIE_FPS).max(1.0);
    let ms = (1000.0 * step / fps).round() as u32;

    let mut renderer = Renderer::new();
    let mut ctx = RenderContext::new(w, h);
    let mut res = Resources::new();
    let mut frames = Vec::new();
    let mut f = comp.frames.start;
    while f < comp.frames.end && frames.len() < MAX_FRAMES {
        ctx.reset();
        renderer.append(
            &comp,
            f,
            Affine::scale(scale),
            1.0,
            &mut Sink { ctx: &mut ctx },
        );
        ctx.flush();
        let mut pm = Pixmap::new(w, h);
        ctx.render(&mut pm, &mut res);
        frames.push((pm.take_rgba8(vello_cpu::peniko::ImageAlphaType::Alpha), ms));
        f += step;
    }
    if frames.is_empty() {
        bail!("{}", tr!("コマがありません", "No frames", "没有帧",));
    }
    Ok(Frames {
        size: [w as usize, h as usize],
        frames,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn synthetic_static_and_animated_images_decode() {
        let red = image::Rgba([255, 0, 0, 255]);
        let blue = image::Rgba([0, 0, 255, 255]);
        let mut png = Cursor::new(Vec::new());
        DynamicImage::ImageRgba8(RgbaImage::from_pixel(4, 3, red))
            .write_to(&mut png, image::ImageFormat::Png)
            .unwrap();
        let decoded = decode(1, png.get_ref()).unwrap();
        assert_eq!(decoded.size, [4, 3]);
        assert_eq!(decoded.frames.len(), 1);
        assert_eq!(&decoded.frames[0].0[..4], &red.0);
        assert_eq!(decoded.frames[0].1, 0);

        let mut gif = Vec::new();
        {
            let mut encoder = image::codecs::gif::GifEncoder::new(&mut gif);
            for (color, ms) in [(red, 40), (blue, 60)] {
                encoder
                    .encode_frame(image::Frame::from_parts(
                        RgbaImage::from_pixel(4, 3, color),
                        0,
                        0,
                        image::Delay::from_numer_denom_ms(ms, 1),
                    ))
                    .unwrap();
            }
        }
        let decoded = decode(GIF, &gif).unwrap();
        assert_eq!(decoded.size, [4, 3]);
        assert_eq!(decoded.frames.len(), 2);
        assert_eq!(
            (&decoded.frames[0].0[..4], decoded.frames[0].1),
            (&red.0[..], 40)
        );
        assert_eq!(
            (&decoded.frames[1].0[..4], decoded.frames[1].1),
            (&blue.0[..], 60)
        );
        assert!(decode(GIF, b"not a gif").is_err());
        assert!(decode(1, b"not a png").is_err());
    }

    #[test]
    fn synthetic_lottie_frames_decode() {
        let data = br#"{"v":"5.5.2","fr":60,"ip":0,"op":6,"w":32,"h":32,"layers":[]}"#;
        let decoded = decode(LOTTIE, data).unwrap();
        assert_eq!(decoded.size, [200, 200]);
        assert_eq!(decoded.frames.len(), 3);
        assert!(decoded
            .frames
            .iter()
            .all(|(rgba, ms)| rgba.len() == 200 * 200 * 4 && *ms == 33));
        assert!(decode(LOTTIE, b"not json").is_err());
    }

    #[test]
    fn lottie_shape_produces_visible_pixels() {
        let data = br#"{
          "v":"5.5.2", "fr":30, "ip":0, "op":3, "w":32, "h":32,
          "layers":[{"ty":4,"ind":1,"ip":0,"op":3,"st":0,
            "ks":{"a":{"a":0,"k":[0,0]},"p":{"a":0,"k":[0,0]},
                  "s":{"a":0,"k":[100,100]},"r":{"a":0,"k":0},"o":{"a":0,"k":100}},
            "shapes":[{"ty":"el","p":{"a":0,"k":[16,16]},"s":{"a":0,"k":[16,16]}},
                      {"ty":"fl","c":{"a":0,"k":[1,0,0,1]},"o":{"a":0,"k":100},"r":1}]}]
        }"#;
        let decoded = decode(LOTTIE, data).unwrap();
        assert_eq!(decoded.frames.len(), 3);
        let rgba = &decoded.frames[0].0;
        let center = (100 * 200 + 100) * 4;
        assert!(rgba[center] > 200 && rgba[center + 3] > 200);
        assert_eq!(rgba[3], 0);
        assert!(decoded.frames.iter().all(|frame| frame.0 == *rgba));
    }

    /// STICKER_DIR に apng.png と lottie.json を置いて実行する
    #[test]
    fn decode_samples() {
        let Ok(dir) = std::env::var("STICKER_DIR") else {
            return;
        };
        let dir = std::path::PathBuf::from(dir);
        let a = decode(2, &std::fs::read(dir.join("apng.png")).unwrap()).unwrap();
        println!("apng: {:?} {} frames", a.size, a.frames.len());
        let l = decode(LOTTIE, &std::fs::read(dir.join("lottie.json")).unwrap()).unwrap();
        println!(
            "lottie: {:?} {} frames {}ms",
            l.size,
            l.frames.len(),
            l.frames[0].1
        );
        assert!(a.frames.len() > 1 && l.frames.len() > 1);
    }
}
