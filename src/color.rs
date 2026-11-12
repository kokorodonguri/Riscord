//! 色（sRGB・非乗算の RGBA）。UI フレームワークに依存しない形で持ち、描くときに gpui の色へ直す
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Color32 {
    pub rgba: [u8; 4],
}

#[allow(dead_code)]
impl Color32 {
    pub const TRANSPARENT: Color32 = Color32 { rgba: [0, 0, 0, 0] };
    pub const WHITE: Color32 = Color32 {
        rgba: [255, 255, 255, 255],
    };
    pub const BLACK: Color32 = Color32 {
        rgba: [0, 0, 0, 255],
    };

    pub const fn from_rgb(r: u8, g: u8, b: u8) -> Self {
        Color32 {
            rgba: [r, g, b, 255],
        }
    }
    pub const fn from_rgba_unmultiplied(r: u8, g: u8, b: u8, a: u8) -> Self {
        Color32 { rgba: [r, g, b, a] }
    }
    pub const fn from_gray(v: u8) -> Self {
        Self::from_rgb(v, v, v)
    }
    pub const fn from_black_alpha(a: u8) -> Self {
        Color32 { rgba: [0, 0, 0, a] }
    }
    pub const fn from_white_alpha(a: u8) -> Self {
        Color32 {
            rgba: [255, 255, 255, a],
        }
    }
    pub fn r(self) -> u8 {
        self.rgba[0]
    }
    pub fn g(self) -> u8 {
        self.rgba[1]
    }
    pub fn b(self) -> u8 {
        self.rgba[2]
    }
    pub fn a(self) -> u8 {
        self.rgba[3]
    }
    pub fn to_srgba_unmultiplied(self) -> [u8; 4] {
        self.rgba
    }
    /// 透明度を掛ける（egui の gamma_multiply と同じく、見た目の濃さを factor 倍にする）
    pub fn gamma_multiply(self, factor: f32) -> Self {
        let [r, g, b, a] = self.rgba;
        Color32 {
            rgba: [r, g, b, (a as f32 * factor.clamp(0.0, 1.0)).round() as u8],
        }
    }
    /// other へ t (0..1) だけ近づける
    pub fn lerp_to_gamma(self, other: Color32, t: f32) -> Self {
        let mut out = [0u8; 4];
        for i in 0..4 {
            out[i] = (self.rgba[i] as f32 + (other.rgba[i] as f32 - self.rgba[i] as f32) * t)
                .round() as u8;
        }
        Color32 { rgba: out }
    }
}

impl From<Color32> for gpui::Rgba {
    fn from(c: Color32) -> Self {
        let [r, g, b, a] = c.rgba;
        gpui::Rgba {
            r: r as f32 / 255.0,
            g: g as f32 / 255.0,
            b: b as f32 / 255.0,
            a: a as f32 / 255.0,
        }
    }
}

impl From<Color32> for gpui::Hsla {
    fn from(c: Color32) -> Self {
        gpui::Rgba::from(c).into()
    }
}

impl From<Color32> for gpui::Fill {
    fn from(c: Color32) -> Self {
        gpui::Hsla::from(c).into()
    }
}

impl From<Color32> for gpui::Background {
    fn from(c: Color32) -> Self {
        gpui::Rgba::from(c).into()
    }
}
