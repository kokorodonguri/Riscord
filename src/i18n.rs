//! 表示言語（日本語・英語・中国語）。デスクトップ版と riscord-core（Android）で共有する。
//! 選んだ言語の保存はデスクトップ側（app.rs の init_language / choose_language）で行う。
//!
//! 文字列は `tr!("日本語", "English", "中文")` のように 3 つをその場に書く（中国語は簡体字）。
//! `tr!("{n} 件", "{n} items", "{n} 项", n = 3)` のように引数を付けると `format!` と同じく String を返す。

use std::sync::atomic::{AtomicU8, Ordering};

static LANG: AtomicU8 = AtomicU8::new(Lang::Ja as u8);

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum Lang {
    Ja = 0,
    En = 1,
    Zh = 2,
}

/// 設定に保存する値
pub const AUTO: &str = "auto";
pub const JA: &str = "ja";
pub const EN: &str = "en";
pub const ZH: &str = "zh";

/// 設定画面で選んだ値（"auto" / "ja" / "en" / "zh"）。全アカウントで共通
static SETTING: std::sync::Mutex<&'static str> = std::sync::Mutex::new(AUTO);

pub fn setting() -> &'static str {
    *SETTING.lock().unwrap_or_else(|e| e.into_inner())
}

/// 設定の値（"auto" / "ja" / "en" / "zh"）を覚えて、言語を切り替える
pub fn set_setting(setting: &'static str) {
    *SETTING.lock().unwrap_or_else(|e| e.into_inner()) = setting;
    apply(setting);
}

/// 保存した値を読む。知らない値なら None
pub fn parse_setting(s: &str) -> Option<&'static str> {
    match s.trim() {
        JA => Some(JA),
        EN => Some(EN),
        ZH => Some(ZH),
        AUTO => Some(AUTO),
        _ => None,
    }
}

pub fn lang() -> Lang {
    match LANG.load(Ordering::Relaxed) {
        1 => Lang::En,
        2 => Lang::Zh,
        _ => Lang::Ja,
    }
}

/// 今の言語の短い名前（"ja" / "en" / "zh"）。プラグインに渡す
pub fn code() -> &'static str {
    match lang() {
        Lang::Ja => JA,
        Lang::En => EN,
        Lang::Zh => ZH,
    }
}

/// 3 つの訳から今の言語のものを選ぶ（`tr!` / `Tr` の中身）
#[inline]
pub fn pick<T>(ja: T, en: T, zh: T) -> T {
    match lang() {
        Lang::Ja => ja,
        Lang::En => en,
        Lang::Zh => zh,
    }
}

pub fn set(lang: Lang) {
    LANG.store(lang as u8, Ordering::Relaxed);
}

/// 設定の値（"auto" / "ja" / "en" / "zh"）から言語を決めて切り替える
pub fn apply(setting: &str) {
    set(resolve(setting, &os_locale()));
}

pub fn resolve(setting: &str, locale: &str) -> Lang {
    match setting {
        JA => Lang::Ja,
        EN => Lang::En,
        ZH => Lang::Zh,
        _ => from_locale(locale),
    }
}

/// "ja_JP.UTF-8" などの OS のロケールから決める。ja・zh 以外は英語
pub fn from_locale(locale: &str) -> Lang {
    let l = locale.to_ascii_lowercase();
    if l.starts_with("ja") {
        Lang::Ja
    } else if l.starts_with("zh") {
        Lang::Zh
    } else {
        Lang::En
    }
}

fn os_locale() -> String {
    for key in ["LC_ALL", "LC_MESSAGES", "LANG"] {
        if let Ok(v) = std::env::var(key) {
            if !v.is_empty() {
                return v;
            }
        }
    }
    String::new()
}

/// const や static に置く訳（`trc!` で作り、使うときに `.s()`）
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Tr(pub &'static str, pub &'static str, pub &'static str);

impl Tr {
    pub fn s(self) -> &'static str {
        pick(self.0, self.1, self.2)
    }
}

#[macro_export]
macro_rules! trc {
    ($ja:literal, $en:literal, $zh:literal $(,)?) => {
        $crate::i18n::Tr($ja, $en, $zh)
    };
}

#[macro_export]
macro_rules! tr {
    // 末尾にカンマだけ付けたときは今まで通り format! の側（`{e}` などを埋める）に回す
    ($ja:literal, $en:literal, $zh:literal) => {
        $crate::i18n::pick($ja, $en, $zh)
    };
    ($ja:literal, $en:literal, $zh:literal, $($arg:tt)*) => {
        match $crate::i18n::lang() {
            $crate::i18n::Lang::Ja => format!($ja, $($arg)*),
            $crate::i18n::Lang::En => format!($en, $($arg)*),
            $crate::i18n::Lang::Zh => format!($zh, $($arg)*),
        }
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locale_detection() {
        assert_eq!(from_locale("ja_JP.UTF-8"), Lang::Ja);
        assert_eq!(from_locale("en_US.UTF-8"), Lang::En);
        assert_eq!(from_locale(""), Lang::En);
        assert_eq!(from_locale("zh_CN.UTF-8"), Lang::Zh);
        assert_eq!(from_locale("zh_TW.UTF-8"), Lang::Zh);
        assert_eq!(resolve(ZH, "en_US.UTF-8"), Lang::Zh);
        assert_eq!(parse_setting("zh"), Some(ZH));
        assert_eq!(resolve(JA, "en_US.UTF-8"), Lang::Ja);
        assert_eq!(resolve(EN, "ja_JP.UTF-8"), Lang::En);
        assert_eq!(resolve(AUTO, "ja_JP.UTF-8"), Lang::Ja);
    }
}
