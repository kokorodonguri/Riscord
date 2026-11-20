//! 画面共有の映像を UDP で運ぶための RTP / RTCP の組み立てと分解。
//! 暗号は Discord の rtpsize 方式（aead_aes256_gcm_rtpsize / aead_xchacha20_poly1305_rtpsize）。
//! H264 は RFC 6184 の単一 NAL・FU-A で送り、受け取りは STAP-A も分解する。

use aes_gcm::aead::{AeadInPlace, KeyInit};
use aes_gcm::Aes256Gcm;
use chacha20poly1305::XChaCha20Poly1305;

/// 映像 (H264) の payload type。Discord 公式クライアントと同じ番号
pub const PT_H264: u8 = 101;
pub const PT_H264_RTX: u8 = 102;
pub const PT_OPUS: u8 = 120;
/// 1 パケットに入れる NAL の最大長（暗号の付加分と UDP の余裕を見て小さめ）
pub const MTU: usize = 1100;
const TAG: usize = 16;
const NONCE: usize = 4;

pub const MODE_AES: &str = "aead_aes256_gcm_rtpsize";
pub const MODE_XCHACHA: &str = "aead_xchacha20_poly1305_rtpsize";

/// 対応する暗号方式を選ぶ（AES を優先）
pub fn choose_mode(modes: &[String]) -> Option<&'static str> {
    if modes.iter().any(|m| m == MODE_AES) {
        Some(MODE_AES)
    } else if modes.iter().any(|m| m == MODE_XCHACHA) {
        Some(MODE_XCHACHA)
    } else {
        None
    }
}

/// 通信路の暗号（RTP / RTCP 共通）
pub enum Cipher {
    Aes(Box<Aes256Gcm>),
    XChaCha(XChaCha20Poly1305),
}

impl Cipher {
    pub fn new(mode: &str, key: &[u8]) -> Option<Cipher> {
        match mode {
            MODE_AES => Aes256Gcm::new_from_slice(key)
                .ok()
                .map(|c| Cipher::Aes(Box::new(c))),
            MODE_XCHACHA => XChaCha20Poly1305::new_from_slice(key)
                .ok()
                .map(Cipher::XChaCha),
            _ => None,
        }
    }

    /// `head` を認証だけ、`body` を暗号化して、head + 暗号文 + タグ + nonce(4) を返す
    pub fn seal(&self, head: &[u8], body: &[u8], counter: u32) -> Option<Vec<u8>> {
        let mut out = Vec::with_capacity(head.len() + body.len() + TAG + NONCE);
        out.extend_from_slice(head);
        let mut buf = body.to_vec();
        let tag: Vec<u8> = match self {
            Cipher::Aes(c) => {
                let mut n = aes_gcm::Nonce::default();
                n[..4].copy_from_slice(&counter.to_be_bytes());
                c.encrypt_in_place_detached(&n, head, &mut buf)
                    .ok()?
                    .to_vec()
            }
            Cipher::XChaCha(c) => {
                let mut n = chacha20poly1305::XNonce::default();
                n[..4].copy_from_slice(&counter.to_be_bytes());
                c.encrypt_in_place_detached(&n, head, &mut buf)
                    .ok()?
                    .to_vec()
            }
        };
        out.extend_from_slice(&buf);
        out.extend_from_slice(&tag);
        out.extend_from_slice(&counter.to_be_bytes());
        Some(out)
    }

    /// `seal` の逆。head_len バイトを認証データとして、残りを復号する
    pub fn open(&self, packet: &[u8], head_len: usize) -> Option<Vec<u8>> {
        if packet.len() < head_len + TAG + NONCE {
            return None;
        }
        let (head, rest) = packet.split_at(head_len);
        let (ct, tail) = rest.split_at(rest.len() - TAG - NONCE);
        let (tag, nonce) = tail.split_at(TAG);
        let mut buf = ct.to_vec();
        match self {
            Cipher::Aes(c) => {
                let mut n = aes_gcm::Nonce::default();
                n[..4].copy_from_slice(nonce);
                c.decrypt_in_place_detached(&n, head, &mut buf, tag.into())
                    .ok()?;
            }
            Cipher::XChaCha(c) => {
                let mut n = chacha20poly1305::XNonce::default();
                n[..4].copy_from_slice(nonce);
                c.decrypt_in_place_detached(&n, head, &mut buf, tag.into())
                    .ok()?;
            }
        }
        Some(buf)
    }
}

// ---- 送信 ----

/// H264 のアクセスユニット（Annex B）を RTP パケットに分けて暗号化する
pub struct Packetizer {
    pub ssrc: u32,
    seq: u16,
    counter: u32,
}

impl Packetizer {
    pub fn new(ssrc: u32) -> Self {
        Packetizer {
            ssrc,
            seq: rand::random(),
            counter: rand::random(),
        }
    }

    fn header(&mut self, marker: bool, timestamp: u32) -> [u8; 16] {
        let mut h = [0u8; 16];
        // V=2, X=1（拡張ヘッダあり）
        h[0] = 0x90;
        h[1] = PT_H264 | if marker { 0x80 } else { 0 };
        h[2..4].copy_from_slice(&self.seq.to_be_bytes());
        self.seq = self.seq.wrapping_add(1);
        h[4..8].copy_from_slice(&timestamp.to_be_bytes());
        h[8..12].copy_from_slice(&self.ssrc.to_be_bytes());
        // 一バイト形式の拡張 (0xBEDE)、長さ 1 ワード
        h[12] = 0xBE;
        h[13] = 0xDE;
        h[14] = 0;
        h[15] = 1;
        h
    }

    /// timestamp は 90kHz。frame は Annex B（開始コード付き）。暗号化済みのパケットを返す
    pub fn packetize(&mut self, cipher: &Cipher, frame: &[u8], timestamp: u32) -> Vec<Vec<u8>> {
        // 拡張の中身: playout-delay (id 5, 3 バイト) を 0 にして、すぐ表示してもらう
        const EXT: [u8; 4] = [0x52, 0, 0, 0];
        let nalus = split_annexb(frame);
        let mut out = Vec::new();
        for (i, nalu) in nalus.iter().enumerate() {
            let last_nal = i + 1 == nalus.len();
            if nalu.len() <= MTU {
                let head = self.header(last_nal, timestamp);
                let mut body = Vec::with_capacity(4 + nalu.len());
                body.extend_from_slice(&EXT);
                body.extend_from_slice(nalu);
                if let Some(p) = self.seal(cipher, &head, &body) {
                    out.push(p);
                }
            } else {
                let nal0 = nalu[0];
                let data = &nalu[1..];
                let chunks: Vec<&[u8]> = data.chunks(MTU).collect();
                for (j, chunk) in chunks.iter().enumerate() {
                    let first = j == 0;
                    let end = j + 1 == chunks.len();
                    let head = self.header(last_nal && end, timestamp);
                    let mut body = Vec::with_capacity(6 + chunk.len());
                    body.extend_from_slice(&EXT);
                    body.push((nal0 & 0xE0) | 28);
                    body.push(
                        if first { 0x80 } else { 0 } | if end { 0x40 } else { 0 } | (nal0 & 0x1F),
                    );
                    body.extend_from_slice(chunk);
                    if let Some(p) = self.seal(cipher, &head, &body) {
                        out.push(p);
                    }
                }
            }
        }
        out
    }

    fn seal(&mut self, cipher: &Cipher, head: &[u8], body: &[u8]) -> Option<Vec<u8>> {
        let c = self.counter;
        self.counter = self.counter.wrapping_add(1);
        cipher.seal(head, body, c)
    }

    /// RTCP（受信側から PLI を送るのにも使う）
    pub fn seal_rtcp(&mut self, cipher: &Cipher, rtcp: &[u8]) -> Option<Vec<u8>> {
        if rtcp.len() < 8 {
            return None;
        }
        let (head, body) = rtcp.split_at(8);
        self.seal(cipher, head, body)
    }
}

/// PLI（キーフレームを送ってほしい）。sender は自分の SSRC、media は相手の映像の SSRC
pub fn pli(sender: u32, media: u32) -> [u8; 12] {
    let mut p = [0u8; 12];
    p[0] = 0x81; // V=2, FMT=1
    p[1] = 206; // PSFB
    p[2..4].copy_from_slice(&2u16.to_be_bytes());
    p[4..8].copy_from_slice(&sender.to_be_bytes());
    p[8..12].copy_from_slice(&media.to_be_bytes());
    p
}

/// Annex B を NAL ごとに分ける（開始コードは含めない）
pub fn split_annexb(data: &[u8]) -> Vec<&[u8]> {
    let mut starts = Vec::new();
    let mut i = 0;
    while i + 3 <= data.len() {
        if data[i] == 0 && data[i + 1] == 0 && data[i + 2] == 1 {
            starts.push(i + 3);
            i += 3;
        } else {
            i += 1;
        }
    }
    if starts.is_empty() {
        return if data.is_empty() { vec![] } else { vec![data] };
    }
    let mut out = Vec::with_capacity(starts.len());
    for (k, &s) in starts.iter().enumerate() {
        let mut e = if k + 1 < starts.len() {
            starts[k + 1] - 3
        } else {
            data.len()
        };
        // 4 バイトの開始コード (00 00 00 01) の先頭の 0 は前の NAL に含めない
        if k + 1 < starts.len() && e > s && data[e - 1] == 0 {
            e -= 1;
        }
        if e > s {
            out.push(&data[s..e]);
        }
    }
    out
}

// ---- 受信 ----

/// 受け取った RTP の中身
pub struct Rtp<'a> {
    pub pt: u8,
    pub marker: bool,
    pub seq: u16,
    pub timestamp: u32,
    pub ssrc: u32,
    /// 認証だけされる先頭部分の長さ（固定ヘッダ + CSRC + 拡張ヘッダの 4 バイト）
    pub head_len: usize,
    pub has_ext: bool,
    pub raw: &'a [u8],
}

pub fn is_rtcp(packet: &[u8]) -> bool {
    packet.len() >= 2 && (192..=223).contains(&packet[1])
}

pub fn parse_rtp(packet: &[u8]) -> Option<Rtp<'_>> {
    if packet.len() < 12 || packet[0] >> 6 != 2 || is_rtcp(packet) {
        return None;
    }
    let cc = (packet[0] & 0x0F) as usize;
    let has_ext = packet[0] & 0x10 != 0;
    let mut head_len = 12 + cc * 4;
    if has_ext {
        head_len += 4;
    }
    if packet.len() < head_len {
        return None;
    }
    Some(Rtp {
        pt: packet[1] & 0x7F,
        marker: packet[1] & 0x80 != 0,
        seq: u16::from_be_bytes([packet[2], packet[3]]),
        timestamp: u32::from_be_bytes([packet[4], packet[5], packet[6], packet[7]]),
        ssrc: u32::from_be_bytes([packet[8], packet[9], packet[10], packet[11]]),
        head_len,
        has_ext,
        raw: packet,
    })
}

impl Rtp<'_> {
    /// 復号して、拡張の中身を除いた payload を返す
    pub fn decrypt(&self, cipher: &Cipher) -> Option<Vec<u8>> {
        let mut plain = cipher.open(self.raw, self.head_len)?;
        if self.has_ext {
            let words =
                u16::from_be_bytes([self.raw[self.head_len - 2], self.raw[self.head_len - 1]])
                    as usize;
            if plain.len() < words * 4 {
                return None;
            }
            plain.drain(..words * 4);
        }
        // パディング
        if self.raw[0] & 0x20 != 0 {
            let pad = *plain.last()? as usize;
            if pad == 0 || pad > plain.len() {
                return None;
            }
            plain.truncate(plain.len() - pad);
        }
        Some(plain)
    }
}

/// RTCP の中に、自分宛ての PLI / FIR（キーフレーム要求）があるか
pub fn rtcp_wants_keyframe(cipher: &Cipher, packet: &[u8]) -> bool {
    if packet.len() < 8 {
        return false;
    }
    let pt = packet[1];
    let fmt = packet[0] & 0x1F;
    // 先頭の RTCP の種類だけ見る（複合パケットの残りは暗号化されている）
    if pt == 206 && (fmt == 1 || fmt == 4) {
        return true;
    }
    // 先頭が SR/RR などの複合パケットなら中身を開いて探す
    let Some(plain) = cipher.open(packet, 8) else {
        return false;
    };
    let first_len = (u16::from_be_bytes([packet[2], packet[3]]) as usize + 1) * 4;
    let mut rest = &plain[first_len.saturating_sub(8).min(plain.len())..];
    while rest.len() >= 4 {
        let len = (u16::from_be_bytes([rest[2], rest[3]]) as usize + 1) * 4;
        if rest[1] == 206 && matches!(rest[0] & 0x1F, 1 | 4) {
            return true;
        }
        if len > rest.len() {
            break;
        }
        rest = &rest[len..];
    }
    false
}

/// A bounded jitter buffer for decrypted video RTP packets.
pub struct VideoPacket {
    pub seq: u16,
    pub timestamp: u32,
    pub marker: bool,
    pub payload: Vec<u8>,
}

#[derive(Default)]
pub struct Reorder {
    next: Option<u16>,
    pending: Vec<(std::time::Instant, VideoPacket)>,
    bytes: usize,
}

impl Reorder {
    const MAX_PACKETS: usize = 64;
    const MAX_BYTES: usize = 1024 * 1024;
    const WAIT: std::time::Duration = std::time::Duration::from_millis(40);

    pub fn push(&mut self, packet: VideoPacket, now: std::time::Instant) -> Vec<VideoPacket> {
        let next = *self.next.get_or_insert(packet.seq);
        let delta = packet.seq.wrapping_sub(next);
        if delta >= 0x8000 || self.pending.iter().any(|(_, p)| p.seq == packet.seq) {
            return Vec::new();
        }
        self.bytes = self.bytes.saturating_add(packet.payload.len());
        self.pending.push((now, packet));
        self.drain(now, delta as usize >= Self::MAX_PACKETS)
    }

    pub fn flush(&mut self, now: std::time::Instant) -> Vec<VideoPacket> {
        self.drain(now, false)
    }

    /// 配信者の SSRC が変わったときに、前の番号を忘れる
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    fn drain(&mut self, now: std::time::Instant, mut force: bool) -> Vec<VideoPacket> {
        let mut out = Vec::new();
        while !self.pending.is_empty() {
            let next = self.next.unwrap();
            let index = self
                .pending
                .iter()
                .enumerate()
                .min_by_key(|(_, (_, p))| p.seq.wrapping_sub(next))
                .unwrap()
                .0;
            let (_, packet) = &self.pending[index];
            if packet.seq != next {
                let expired = self
                    .pending
                    .iter()
                    .any(|(at, _)| now.saturating_duration_since(*at) >= Self::WAIT);
                if !force
                    && !expired
                    && self.pending.len() <= Self::MAX_PACKETS
                    && self.bytes <= Self::MAX_BYTES
                {
                    break;
                }
                self.next = Some(packet.seq);
            }
            let (_, packet) = self.pending.swap_remove(index);
            self.bytes -= packet.payload.len();
            self.next = Some(packet.seq.wrapping_add(1));
            out.push(packet);
            force = false;
        }
        out
    }
}

/// RTP の H264 を、アクセスユニット（Annex B）に組み立てる
#[derive(Default)]
pub struct Depacketizer {
    timestamp: Option<u32>,
    next_seq: Option<u16>,
    frame: Vec<u8>,
    /// FU-A を組み立て中
    in_fu: bool,
    /// 途中のパケットが抜けた（このフレームは捨てる）
    broken: bool,
}

/// 組み立て終わったフレーム
pub struct AccessUnit {
    pub data: Vec<u8>,
    pub keyframe: bool,
}

pub enum Push {
    None,
    Frame(AccessUnit),
    /// パケットが抜けた。キーフレームを頼むとよい
    Lost,
}

impl Depacketizer {
    /// 配信者の SSRC が変わったときに、組み立て途中のものと前の番号を捨てる
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    pub fn push(&mut self, seq: u16, timestamp: u32, marker: bool, payload: &[u8]) -> Push {
        let mut lost = false;
        if let Some(n) = self.next_seq {
            let d = seq.wrapping_sub(n);
            if d >= 0x8000 {
                // 古い（順番が入れ替わった・重複）パケットは捨てる
                return Push::None;
            }
            if d != 0 {
                lost = true;
                self.broken = true;
            }
        }
        self.next_seq = Some(seq.wrapping_add(1));
        if self.timestamp != Some(timestamp) {
            // 新しいフレームが始まった（前のフレームの最後が抜けていた）
            if self.timestamp.is_some() && !self.frame.is_empty() {
                lost = true;
            }
            self.timestamp = Some(timestamp);
            self.frame.clear();
            self.in_fu = false;
            // 欠けたパケットがこのフレームの先頭かもしれない。
            self.broken = lost;
        }
        // STAP-A can add at most two extra bytes per payload byte, plus a start code.
        // Stop accumulating immediately and discard the rest of this access unit.
        const MAX_ACCESS_UNIT: usize = 16 * 1024 * 1024;
        if !self.broken
            && self
                .frame
                .len()
                .saturating_add(payload.len().saturating_mul(3))
                .saturating_add(4)
                > MAX_ACCESS_UNIT
        {
            self.frame.clear();
            self.broken = true;
            self.in_fu = false;
            return Push::Lost;
        }
        if !self.broken && !payload.is_empty() {
            let kind = payload[0] & 0x1F;
            match kind {
                1..=23 => {
                    self.frame.extend_from_slice(&[0, 0, 0, 1]);
                    self.frame.extend_from_slice(payload);
                }
                24 => {
                    // STAP-A: [長さ(2) NAL]...
                    let mut p = &payload[1..];
                    while p.len() >= 2 {
                        let n = u16::from_be_bytes([p[0], p[1]]) as usize;
                        if n == 0 || p.len() < 2 + n {
                            self.broken = true;
                            break;
                        }
                        self.frame.extend_from_slice(&[0, 0, 0, 1]);
                        self.frame.extend_from_slice(&p[2..2 + n]);
                        p = &p[2 + n..];
                    }
                    if !p.is_empty() {
                        self.broken = true;
                    }
                }
                28 if payload.len() >= 2 => {
                    let start = payload[1] & 0x80 != 0;
                    if start {
                        if self.in_fu {
                            self.broken = true;
                        }
                        self.frame.extend_from_slice(&[0, 0, 0, 1]);
                        self.frame.push((payload[0] & 0xE0) | (payload[1] & 0x1F));
                        self.in_fu = true;
                    } else if !self.in_fu {
                        self.broken = true;
                    }
                    self.frame.extend_from_slice(&payload[2..]);
                    if payload[1] & 0x40 != 0 {
                        self.in_fu = false;
                    }
                }
                _ => {}
            }
        }
        if marker {
            let data = std::mem::take(&mut self.frame);
            let broken = std::mem::take(&mut self.broken);
            self.timestamp = None;
            if broken || self.in_fu || data.is_empty() {
                self.in_fu = false;
                return Push::Lost;
            }
            let keyframe = split_annexb(&data)
                .iter()
                .any(|n| matches!(n.first().map(|b| b & 0x1F), Some(5) | Some(7)));
            return Push::Frame(AccessUnit { data, keyframe });
        }
        if lost {
            Push::Lost
        } else {
            Push::None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reset_forgets_old_sequence_numbers() {
        let t = std::time::Instant::now();
        let packet = |seq| VideoPacket {
            seq,
            timestamp: 1,
            marker: true,
            payload: vec![0x65, 1, 2],
        };
        let mut r = Reorder::default();
        assert_eq!(r.push(packet(40000), t).len(), 1);
        // 新しい SSRC では番号が飛ぶ。前の番号のままだと古いものとして捨てられる
        assert!(r.push(packet(10000), t).is_empty());
        r.reset();
        assert_eq!(r.push(packet(10000), t).len(), 1);

        let mut d = Depacketizer::default();
        assert!(matches!(
            d.push(40000, 1, true, &[0x65, 1, 2]),
            Push::Frame(_)
        ));
        d.reset();
        assert!(matches!(
            d.push(10000, 2, true, &[0x65, 1, 2]),
            Push::Frame(_)
        ));
    }

    #[test]
    fn reorder_handles_gap_wrap_duplicates_and_timeout() {
        let mut r = Reorder::default();
        let t = std::time::Instant::now();
        let packet = |seq| VideoPacket {
            seq,
            timestamp: 1,
            marker: false,
            payload: vec![1],
        };
        assert_eq!(r.push(packet(65534), t).len(), 1);
        assert!(r.push(packet(0), t).is_empty());
        assert!(r.push(packet(0), t).is_empty());
        let ordered = r.push(packet(65535), t);
        assert_eq!(
            ordered.iter().map(|p| p.seq).collect::<Vec<_>>(),
            [65535, 0]
        );
        assert!(r.push(packet(2), t).is_empty());
        assert!(r.flush(t + std::time::Duration::from_millis(39)).is_empty());
        assert_eq!(r.flush(t + std::time::Duration::from_millis(40))[0].seq, 2);
        assert!(r.push(packet(1), t).is_empty());
    }

    #[test]
    fn reorder_bounds_memory_and_reports_sequence_gap() {
        let mut reorder = Reorder::default();
        let mut depack = Depacketizer::default();
        let now = std::time::Instant::now();
        let packet = |seq, size| VideoPacket {
            seq,
            timestamp: 1,
            marker: false,
            payload: vec![0x41; size],
        };
        let first = reorder.push(packet(1, 1), now).pop().unwrap();
        assert!(matches!(
            depack.push(first.seq, 1, false, &first.payload),
            Push::None
        ));
        assert!(reorder
            .push(packet(3, Reorder::MAX_BYTES / 2), now)
            .is_empty());
        let released = reorder.push(packet(4, Reorder::MAX_BYTES / 2 + 1), now);
        assert!(!released.is_empty());
        assert!(reorder.bytes <= Reorder::MAX_BYTES);
        assert!(matches!(
            depack.push(released[0].seq, 1, false, &released[0].payload),
            Push::Lost
        ));
    }

    #[test]
    fn reordering_reassembles_fragmented_frame_without_loss() {
        let t = std::time::Instant::now();
        let mut reorder = Reorder::default();
        let mut depack = Depacketizer::default();
        let packet = |seq, marker, payload: &[u8]| VideoPacket {
            seq,
            timestamp: 10,
            marker,
            payload: payload.to_vec(),
        };
        let first = reorder.push(packet(10, false, &[0x7c, 0x85, 1]), t);
        assert!(matches!(
            depack.push(first[0].seq, 10, false, &first[0].payload),
            Push::None
        ));
        assert!(reorder
            .push(packet(12, true, &[0x7c, 0x45, 3]), t)
            .is_empty());
        let ordered = reorder.push(packet(11, false, &[0x7c, 0x05, 2]), t);
        assert_eq!(ordered.len(), 2);
        assert!(matches!(
            depack.push(ordered[0].seq, 10, false, &ordered[0].payload),
            Push::None
        ));
        match depack.push(ordered[1].seq, 10, true, &ordered[1].payload) {
            Push::Frame(frame) => assert_eq!(frame.data, [0, 0, 0, 1, 0x65, 1, 2, 3]),
            _ => panic!("expected complete frame"),
        }
    }

    #[test]
    fn rejects_incomplete_or_malformed_nalus() {
        let mut d = Depacketizer::default();
        assert!(matches!(d.push(1, 1, true, &[0x7c, 0x85, 42]), Push::Lost));
        assert!(matches!(d.push(2, 2, true, &[24, 0, 4, 0x65]), Push::Lost));
        assert!(matches!(d.push(3, 3, true, &[0x65, 1]), Push::Frame(_)));
    }

    #[test]
    fn oversized_access_unit_is_discarded_and_next_frame_recovers() {
        let mut d = Depacketizer::default();
        let payload = vec![0x41; 1024 * 1024];
        let mut lost = false;
        for seq in 0..20 {
            lost |= matches!(d.push(seq, 1, false, &payload), Push::Lost);
        }
        assert!(lost);
        assert!(d.frame.len() <= 16 * 1024 * 1024);
        assert!(matches!(d.push(20, 1, true, &[0x41]), Push::Lost));
        assert!(matches!(d.push(21, 2, true, &[0x65, 1]), Push::Frame(_)));
    }

    #[test]
    fn lost_whole_packet_frame_requests_keyframe() {
        let mut d = Depacketizer::default();
        assert!(matches!(d.push(1, 1, true, &[0x65, 1]), Push::Frame(_)));
        assert!(matches!(d.push(3, 3, true, &[0x41, 2]), Push::Lost));
        assert!(matches!(d.push(4, 4, true, &[0x65, 3]), Push::Frame(_)));
    }

    fn cipher(mode: &str) -> Cipher {
        Cipher::new(mode, &[7u8; 32]).unwrap()
    }

    fn frame() -> Vec<u8> {
        let mut f = vec![
            0, 0, 0, 1, 0x67, 1, 2, 3, 0, 0, 1, 0x68, 4, 5, 0, 0, 0, 1, 0x65,
        ];
        f.extend((0..5000u32).map(|i| (i % 251) as u8 | 1));
        f
    }

    #[test]
    fn split_annexb_handles_3_and_4_byte_start_codes() {
        let f = [0, 0, 0, 1, 0x67, 9, 0, 0, 1, 0x68, 8, 0, 0, 0, 1, 0x65, 7];
        let n = split_annexb(&f);
        assert_eq!(n, vec![&[0x67, 9][..], &[0x68, 8][..], &[0x65, 7][..]]);
    }

    #[test]
    fn packetize_round_trips_through_depacketizer() {
        for mode in [MODE_AES, MODE_XCHACHA] {
            let c = cipher(mode);
            let mut p = Packetizer::new(1234);
            let f = frame();
            let packets = p.packetize(&c, &f, 9000);
            assert!(packets.len() > 3);
            let mut d = Depacketizer::default();
            let mut got = None;
            for pkt in &packets {
                assert!(pkt.len() <= MTU + 42, "{}", pkt.len());
                let r = parse_rtp(pkt).unwrap();
                assert_eq!(r.pt, PT_H264);
                assert_eq!(r.ssrc, 1234);
                let payload = r.decrypt(&c).unwrap();
                if let Push::Frame(au) = d.push(r.seq, r.timestamp, r.marker, &payload) {
                    got = Some(au);
                }
            }
            let au = got.expect("frame");
            assert!(au.keyframe);
            assert_eq!(split_annexb(&au.data), split_annexb(&f));
        }
    }

    #[test]
    fn lost_packet_drops_frame() {
        let c = cipher(MODE_AES);
        let mut p = Packetizer::new(1);
        let packets = p.packetize(&c, &frame(), 0);
        let mut d = Depacketizer::default();
        let mut lost = false;
        for (i, pkt) in packets.iter().enumerate() {
            if i == 2 {
                continue;
            }
            let r = parse_rtp(pkt).unwrap();
            match d.push(r.seq, r.timestamp, r.marker, &r.decrypt(&c).unwrap()) {
                Push::Frame(_) => panic!("壊れたフレームを出した"),
                Push::Lost => lost = true,
                Push::None => {}
            }
        }
        assert!(lost);
    }

    #[test]
    fn pli_is_recognized() {
        let c = cipher(MODE_AES);
        let mut p = Packetizer::new(1);
        let sealed = p.seal_rtcp(&c, &pli(1, 2)).unwrap();
        assert!(is_rtcp(&sealed));
        assert!(rtcp_wants_keyframe(&c, &sealed));
        assert_eq!(c.open(&sealed, 8).unwrap(), 2u32.to_be_bytes());
    }
}
