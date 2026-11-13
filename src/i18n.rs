//! 表示言語（日本語・英語）。デスクトップ版と riscord-core（Android）で共有する。
//! 選んだ言語の保存はデスクトップ側（app.rs の init_language / choose_language）で行う。
//!
//! 文字列は `tr!("日本語", "English")` のように両方をその場に書く。
//! `tr!("{n} 件", "{n} items", n = 3)` のように引数を付けると `format!` と同じく String を返す。

use std::sync::atomic::{AtomicBool, Ordering};

static ENGLISH: AtomicBool = AtomicBool::new(false);

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Lang {
    Ja,
    En,
}

/// 設定に保存する値
pub const AUTO: &str = "auto";
pub const JA: &str = "ja";
pub const EN: &str = "en";

/// 設定画面で選んだ値（"auto" / "ja" / "en"）。全アカウントで共通
static SETTING: std::sync::Mutex<&'static str> = std::sync::Mutex::new(AUTO);

pub fn setting() -> &'static str {
    *SETTING.lock().unwrap_or_else(|e| e.into_inner())
}

/// 設定の値（"auto" / "ja" / "en"）を覚えて、言語を切り替える
pub fn set_setting(setting: &'static str) {
    *SETTING.lock().unwrap_or_else(|e| e.into_inner()) = setting;
    apply(setting);
}

/// 保存した値を読む。知らない値なら None
pub fn parse_setting(s: &str) -> Option<&'static str> {
    match s.trim() {
        JA => Some(JA),
        EN => Some(EN),
        AUTO => Some(AUTO),
        _ => None,
    }
}

pub fn is_en() -> bool {
    ENGLISH.load(Ordering::Relaxed)
}

pub fn set(lang: Lang) {
    ENGLISH.store(lang == Lang::En, Ordering::Relaxed);
}

/// 設定の値（"auto" / "ja" / "en"）から言語を決めて切り替える
pub fn apply(setting: &str) {
    set(resolve(setting, &os_locale()));
}

pub fn resolve(setting: &str, locale: &str) -> Lang {
    match setting {
        JA => Lang::Ja,
        EN => Lang::En,
        _ => from_locale(locale),
    }
}

/// "ja_JP.UTF-8" などの OS のロケールから決める。ja 以外は英語
pub fn from_locale(locale: &str) -> Lang {
    if locale.to_ascii_lowercase().starts_with("ja") {
        Lang::Ja
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
pub struct Tr(pub &'static str, pub &'static str);

impl Tr {
    pub fn s(self) -> &'static str {
        if is_en() {
            self.1
        } else {
            self.0
        }
    }
}

#[macro_export]
macro_rules! trc {
    ($ja:literal, $en:literal) => {
        $crate::i18n::Tr($ja, $en)
    };
}

#[macro_export]
macro_rules! tr {
    ($ja:literal, $en:literal) => {
        if $crate::i18n::is_en() {
            $en
        } else {
            $ja
        }
    };
    ($ja:literal, $en:literal, $($arg:tt)*) => {
        if $crate::i18n::is_en() {
            format!($en, $($arg)*)
        } else {
            format!($ja, $($arg)*)
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
        assert_eq!(resolve(JA, "en_US.UTF-8"), Lang::Ja);
        assert_eq!(resolve(EN, "ja_JP.UTF-8"), Lang::En);
        assert_eq!(resolve(AUTO, "ja_JP.UTF-8"), Lang::Ja);
    }
}
