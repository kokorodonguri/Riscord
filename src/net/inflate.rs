//! ゲートウェイの zlib-stream 展開（デスクトップ版とスマホ版で共用）
use anyhow::{bail, Result};

/// ゲートウェイの zlib-stream 展開。1接続で1つの圧縮文脈を使い続け、
/// 1件のメッセージは 00 00 FF FF で終わる（途中で分割されて届くこともある）
pub(crate) struct Inflater {
    z: flate2::Decompress,
    pub(crate) buf: Vec<u8>,
    pub(crate) out: Vec<u8>,
}

impl Inflater {
    pub(crate) const SUFFIX: [u8; 4] = [0x00, 0x00, 0xff, 0xff];
    /// READY 後などに大きくなった作業領域は、次のメッセージで小さく取り直す
    pub(crate) const KEEP: usize = 1 << 20;
    // 非圧縮時の tungstenite のメッセージ上限と同じ。
    pub(crate) const MAX_MESSAGE: usize = 64 << 20;

    pub(crate) fn new() -> Self {
        Inflater {
            z: flate2::Decompress::new(true),
            buf: Vec::new(),
            out: Vec::new(),
        }
    }

    /// 受け取ったバイナリを足し、1件そろったら展開した JSON 文字列を返す
    pub(crate) fn push(&mut self, data: &[u8]) -> Result<Option<&str>> {
        self.push_with_limit(data, Self::MAX_MESSAGE)
    }

    pub(crate) fn push_with_limit(&mut self, data: &[u8], limit: usize) -> Result<Option<&str>> {
        if data.len() > limit.saturating_sub(self.buf.len()) {
            bail!(
                "{}",
                tr!(
                    "圧縮ゲートウェイメッセージが上限を超えました",
                    "Compressed gateway message exceeded the limit",
                    "压缩的网关消息超出上限",
                )
            );
        }
        if self.buf.is_empty() && self.buf.capacity() > Self::KEEP {
            self.buf = Vec::new();
        }
        self.buf.extend_from_slice(data);
        if !self.buf.ends_with(&Self::SUFFIX) {
            return Ok(None);
        }
        if self.out.capacity() > Self::KEEP {
            self.out = Vec::new();
        }
        self.out.clear();
        let mut input = &self.buf[..];
        loop {
            if self.out.len() == self.out.capacity() {
                let target = self
                    .out
                    .capacity()
                    .saturating_mul(2)
                    .max(64 * 1024)
                    .min(limit + 1);
                if target == limit + 1 {
                    // 上限判定の1バイトのために、確保容量をさらに倍増させない。
                    self.out.reserve_exact(target - self.out.len());
                } else {
                    self.out.reserve(target - self.out.len());
                }
            }
            let (in0, out0) = (self.z.total_in(), self.z.total_out());
            self.z
                .decompress_vec(input, &mut self.out, flate2::FlushDecompress::Sync)?;
            let used = (self.z.total_in() - in0) as usize;
            let made = self.z.total_out() - out0;
            if self.out.len() > limit {
                bail!(
                    "{}",
                    tr!(
                        "展開済みゲートウェイメッセージが上限を超えました",
                        "Decompressed gateway message exceeded the limit",
                        "解压后的网关消息超出上限",
                    )
                );
            }
            input = &input[used..];
            if input.is_empty() && self.out.len() < self.out.capacity() {
                break;
            }
            if used == 0 && made == 0 && self.out.len() < self.out.capacity() {
                bail!(
                    "{}",
                    tr!(
                        "zlib-stream の展開が進みません",
                        "zlib-stream decompression is stuck",
                        "zlib-stream 解压停滞",
                    )
                );
            }
        }
        if self.buf.capacity() > Self::KEEP {
            self.buf = Vec::new();
        } else {
            self.buf.clear();
        }
        Ok(Some(std::str::from_utf8(&self.out)?))
    }
}
