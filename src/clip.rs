//! 貼り付け・ドロップされた画像やファイルを送信用の添付にする
//!
//! 画像は arboard で直接クリップボードから読む（Wayland のデータコントロールにも対応させるため）。

use std::path::{Path, PathBuf};

use std::sync::Arc;

/// 送信待ちの添付ファイル
pub struct Upload {
    pub name: String,
    pub data: Vec<u8>,
    /// 画像ならサムネイル
    pub thumb: Option<Arc<gpui::RenderImage>>,
}

// GPUI の画像は GPU 側のキャッシュも明示的に解放する。
impl Drop for Upload {
    fn drop(&mut self) {
        if let Some(image) = self.thumb.take() {
            crate::images::release(image);
        }
    }
}

const THUMB: u32 = 160;

/// 別スレッドで読み込んだ添付（読めたもの, 読めなかったときのお知らせ）
pub struct LoadedFiles {
    pub loaded: Vec<Upload>,
    pub errors: Vec<String>,
}

/// クリップボードの画像を取り出す（PNG への変換は重いので encode_image で別スレッドに回す）。画像がなければ None
pub fn clipboard_rgba() -> Option<image::RgbaImage> {
    let mut cb = arboard::Clipboard::new().ok()?;
    let img = cb.get_image().ok()?;
    let (w, h) = (img.width as u32, img.height as u32);
    image::RgbaImage::from_raw(w, h, img.bytes.into_owned())
}

/// クリップボードの画像を PNG にして添付にする
pub fn encode_image(rgba: image::RgbaImage) -> Result<Upload, String> {
    let mut png = Vec::new();
    if let Err(e) = rgba.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png) {
        eprintln!("貼り付け画像の変換失敗: {e}");
        return Err(tr!(
            "クリップボードの画像を読み込めませんでした。",
            "Couldn't read the image from the clipboard.",
            "无法读取剪贴板中的图片。"
        )
        .into());
    }
    let name = format!("image-{}.png", chrono::Local::now().format("%Y%m%d-%H%M%S"));
    Ok(Upload {
        thumb: Some(thumb_texture(&rgba)),
        name,
        data: png,
    })
}

/// ファイルを読み込む。画像ならサムネイルも作る
pub fn load_file(path: &Path) -> Result<Upload, String> {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "file".into());
    let data = std::fs::read(path).map_err(|e| {
        eprintln!("{} を読めません: {e}", path.display());
        tr!(
            "「{name}」を読み込めませんでした。ファイルを確認して、もう一度お試しください。",
            "Couldn't read \"{name}\". Check the file and try again.",
            "无法读取“{name}”。请检查文件后重试。",
        )
    })?;
    // 縦横が極端に大きい画像（展開すると巨大になるもの）はサムネイルを作らない
    let thumb = image::ImageReader::new(std::io::Cursor::new(&data))
        .with_guessed_format()
        .ok()
        .and_then(|r| r.into_decoder().ok())
        .filter(|d| {
            let (w, h) = image::ImageDecoder::dimensions(d);
            crate::images::check_decode_dimensions(w, h).is_ok()
        })
        .and_then(|d| image::DynamicImage::from_decoder(d).ok())
        .map(|img| thumb_texture(&img.to_rgba8()));
    Ok(Upload { name, data, thumb })
}

fn thumb_texture(img: &image::RgbaImage) -> Arc<gpui::RenderImage> {
    // 縦横比を保って THUMB に収める
    let k = (THUMB as f32 / img.width().max(1) as f32)
        .min(THUMB as f32 / img.height().max(1) as f32)
        .min(1.0);
    let (w, h) = (
        ((img.width() as f32 * k) as u32).max(1),
        ((img.height() as f32 * k) as u32).max(1),
    );
    crate::images::render_image(image::imageops::thumbnail(img, w, h))
}

/// ファイルマネージャーでコピーしたファイルは「file:///…」や絶対パスの文字として貼り付けられる。
/// 全行が実在するファイルならそのパスを返す（普通の文章なら None で、そのまま入力欄に入れる）。
/// file:// の付かない素のパスは、相談などで文字として貼ることも多い。隠しフォルダ（~/.ssh など）や
/// 設定フォルダ（トークンがある）の中を指すものは添付にせず、文字として貼る
pub fn pasted_paths(text: &str) -> Option<Vec<PathBuf>> {
    let mut paths = Vec::new();
    for l in text
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
    {
        let path = match l.strip_prefix("file://") {
            // file://host/path の host は省略されている前提（ローカルのみ）
            Some(rest) => PathBuf::from(percent_decode(rest)),
            None => {
                let p = PathBuf::from(l);
                if sensitive_path(&p) {
                    return None;
                }
                p
            }
        };
        paths.push(path);
    }
    (!paths.is_empty() && paths.iter().all(|p| p.is_absolute() && p.is_file())).then_some(paths)
}

/// 素のパスとして貼られても添付にしない場所（隠しフォルダ・隠しファイル・設定フォルダ）
fn sensitive_path(p: &std::path::Path) -> bool {
    let hidden = p.components().any(|c| match c {
        std::path::Component::Normal(n) => n.to_string_lossy().starts_with('.'),
        _ => false,
    });
    hidden
        || [
            crate::app::config_root(),
            dirs::config_dir().unwrap_or_default(),
        ]
        .iter()
        .any(|dir| !dir.as_os_str().is_empty() && p.starts_with(dir))
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let Some(v) = std::str::from_utf8(&b[i + 1..i + 3])
                .ok()
                .and_then(|h| u8::from_str_radix(h, 16).ok())
            {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dropping_upload_releases_its_thumbnail() {
        let thumbnail = crate::images::render_image(image::RgbaImage::new(1, 1));
        let upload = Upload {
            name: "test.png".into(),
            data: vec![],
            thumb: Some(thumbnail.clone()),
        };
        drop(upload);
        assert!(crate::images::release_queued(&thumbnail));
    }

    #[test]
    fn decodes_percent_escapes() {
        assert_eq!(percent_decode("/tmp/a%20b%E3%81%82.png"), "/tmp/a bあ.png");
        assert_eq!(percent_decode("/x%2"), "/x%2");
        assert_eq!(percent_decode("100%"), "100%");
    }

    #[test]
    fn plain_text_is_not_paths() {
        assert!(pasted_paths("hello world").is_none());
        assert!(pasted_paths("/definitely/not/here.png").is_none());
        let f = std::env::temp_dir().join("riscord clip test.txt");
        std::fs::write(&f, b"x").unwrap();
        let uri = format!("file://{}", f.to_string_lossy().replace(' ', "%20"));
        assert_eq!(pasted_paths(&uri), Some(vec![f.clone()]));
        assert_eq!(pasted_paths(&f.to_string_lossy()), Some(vec![f.clone()]));
        let _ = std::fs::remove_file(f);
    }

    #[test]
    fn plain_paths_to_hidden_or_config_files_stay_text() {
        let dir = std::env::temp_dir().join(".riscord-clip-hidden");
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("token");
        std::fs::write(&f, b"secret").unwrap();
        assert!(
            pasted_paths(&f.to_string_lossy()).is_none(),
            "素のパスは文字のまま"
        );
        // ファイルマネージャーからのコピー（file://）は添付にする
        let uri = format!("file://{}", f.to_string_lossy());
        assert_eq!(pasted_paths(&uri), Some(vec![f.clone()]));
        let cfg = crate::app::config_root().join("riscord");
        std::fs::create_dir_all(&cfg).unwrap();
        let token = cfg.join("clip-test-token");
        std::fs::write(&token, b"secret").unwrap();
        assert!(pasted_paths(&token.to_string_lossy()).is_none());
        let _ = std::fs::remove_file(token);
        let _ = std::fs::remove_dir_all(dir);
    }
}
