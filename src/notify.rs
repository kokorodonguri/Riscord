//! デスクトップ通知（メンションされたときなど）。
//! Linux は D-Bus の通知サーバー、Windows はトースト。押されたら、そのチャンネルを開く
use std::sync::{Arc, Mutex};

use crate::wake::Waker;

/// 押された通知の行き先（サーバー, チャンネル）
pub type Clicked = Arc<Mutex<Option<(Option<String>, String)>>>;

/// 通知を出す（通知サーバーとのやり取りで待たないよう別スレッドで）
pub fn show(
    title: String,
    body: String,
    target: (Option<String>, String),
    clicked: Clicked,
    waker: Waker,
) {
    if cfg!(test) {
        return;
    }
    std::thread::spawn(move || {
        #[cfg(all(unix, not(target_os = "macos")))]
        let body = escape_markup(&body);
        let mut n = notify_rust::Notification::new();
        n.appname("Riscord").summary(&title).body(&body);
        #[cfg(all(unix, not(target_os = "macos")))]
        {
            n.icon("riscord")
                .hint(notify_rust::Hint::Category("im.received".into()))
                .hint(notify_rust::Hint::DesktopEntry("riscord".into()))
                .action("default", tr!("開く", "Open", "打开"));
        }
        let handle = match n.show() {
            Ok(h) => h,
            Err(e) => {
                eprintln!("通知を出せませんでした: {e}");
                return;
            }
        };
        #[cfg(all(unix, not(target_os = "macos")))]
        handle.wait_for_action(|action| {
            if action == "default" {
                *clicked.lock().unwrap() = Some(target);
                waker.wake();
            }
        });
        #[cfg(not(all(unix, not(target_os = "macos"))))]
        {
            let _ = (handle, target, clicked, waker);
        }
    });
}

#[cfg(any(test, all(unix, not(target_os = "macos"))))]
fn escape_markup(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// 通知の本文を短くする（1行・長すぎない）
pub fn excerpt(s: &str, max: usize) -> String {
    let one: String = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if one.chars().count() <= max {
        one
    } else {
        let mut t: String = one.chars().take(max).collect();
        t.push('…');
        t
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn escapes_markup_in_notification_body() {
        assert_eq!(
            super::escape_markup("<a href='x'>&"),
            "&lt;a href='x'&gt;&amp;"
        );
    }

    #[test]
    fn excerpt_shortens() {
        assert_eq!(super::excerpt("a\n b", 10), "a b");
        assert_eq!(super::excerpt("あいうえお", 3), "あいう…");
    }
}
