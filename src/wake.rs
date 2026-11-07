//! 別スレッド（通信・音声・ログイン・ファイル選択など）から画面に「描き直して」と知らせる口。
//! UI フレームワークに依存しないよう、ただの関数として持つ（gpui 側で受け口を作って渡す）
use std::sync::Arc;
use std::time::Duration;

#[derive(Clone, Default)]
pub struct Waker(Option<Arc<dyn Fn(Option<Duration>) + Send + Sync>>);

impl Waker {
    pub fn new(f: impl Fn(Option<Duration>) + Send + Sync + 'static) -> Self {
        Waker(Some(Arc::new(f)))
    }

    /// すぐに描き直す
    pub fn wake(&self) {
        if let Some(f) = &self.0 {
            f(None)
        }
    }

    /// 急がない更新。d 以内にまとめて1回描き直せばよい
    pub fn wake_after(&self, d: Duration) {
        if let Some(f) = &self.0 {
            f(Some(d))
        }
    }
}
