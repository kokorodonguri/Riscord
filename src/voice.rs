//! ボイスチャット。接続・暗号化(DAVE)・Opusは songbird の driver に任せ、
//! マイクとスピーカーは cpal で扱う。UI とは `VoiceCtl` を共有してやり取りする。

use std::collections::{HashMap, HashSet, VecDeque};
use std::io::{Read, Seek, SeekFrom};
use std::num::NonZeroU64;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering::Relaxed};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use anyhow::{anyhow, Result};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use songbird::driver::{Channels, DecodeConfig, DecodeMode, SampleRate as SbRate};
use songbird::events::{CoreEvent, Event as SbEvent, EventContext, EventHandler};
use songbird::input::{Input, RawAdapter};
use songbird::{Config, ConnectionInfo, Driver};

const RATE: u32 = 48_000;
/// 20ms 分のステレオサンプル数
const FRAME: usize = 960 * 2;
/// マイク側に溜めておく上限（遅延を抑える）
const MIC_MAX: usize = FRAME * 5;
/// スピーカー側に溜めておく上限
const OUT_MAX: usize = FRAME * 10;
/// サウンドボードの音をためておく上限（48 kHz ステレオで 12 秒）
const EFFECT_MAX: usize = RATE as usize * 2 * 12;
/// サウンドボードの音の長さの上限（Discord は 5.2 秒まで）
const SOUND_MAX_SECS: usize = 10;
/// 音声検出で閉じるまでの猶予
const VAD_HANG: Duration = Duration::from_millis(300);

fn f32_load(a: &AtomicU32) -> f32 {
    f32::from_bits(a.load(Relaxed))
}
fn f32_store(a: &AtomicU32, v: f32) {
    a.store(v.to_bits(), Relaxed)
}

/// UI と音声処理が共有する設定・状態
pub struct VoiceCtl {
    pub mute: AtomicBool,
    pub deaf: AtomicBool,
    /// true = プッシュトゥトーク
    pub ptt: AtomicBool,
    pub ptt_down: AtomicBool,
    vad_threshold: AtomicU32,
    in_gain: AtomicU32,
    out_gain: AtomicU32,
    /// 直近のマイク入力レベル (0..1)
    level: AtomicU32,
    vad_open: AtomicBool,
    /// マイクのノイズ抑制
    noise_suppression: AtomicBool,
    /// 再生待ちのサウンドボードの音（48 kHz ステレオ）。受信の音声に混ぜて流す
    effects: Mutex<VecDeque<f32>>,
    /// 実際に送信中か
    pub transmitting: AtomicBool,
    pub input_device: Mutex<Option<String>>,
    pub output_device: Mutex<Option<String>>,
    /// デバイス変更などで音声ストリームを作り直す
    pub restart: AtomicBool,
    /// 自分以外の話している人 (user_id)
    pub speaking: Mutex<HashSet<String>>,
    /// 視聴中の画面共有の最新の 1 枚（src/golive）
    #[cfg(not(target_os = "android"))]
    pub screen: Arc<Mutex<Option<crate::golive::Frame>>>,
    repaint: Mutex<Option<crate::wake::Waker>>,
}

impl Default for VoiceCtl {
    fn default() -> Self {
        VoiceCtl {
            mute: AtomicBool::new(false),
            deaf: AtomicBool::new(false),
            ptt: AtomicBool::new(false),
            ptt_down: AtomicBool::new(false),
            vad_threshold: AtomicU32::new(0.02f32.to_bits()),
            in_gain: AtomicU32::new(1.0f32.to_bits()),
            out_gain: AtomicU32::new(1.0f32.to_bits()),
            level: AtomicU32::new(0),
            vad_open: AtomicBool::new(false),
            noise_suppression: AtomicBool::new(false),
            effects: Mutex::new(VecDeque::new()),
            transmitting: AtomicBool::new(false),
            input_device: Mutex::new(None),
            output_device: Mutex::new(None),
            restart: AtomicBool::new(false),
            speaking: Mutex::new(HashSet::new()),
            #[cfg(not(target_os = "android"))]
            screen: Arc::default(),
            repaint: Mutex::new(None),
        }
    }
}

impl VoiceCtl {
    pub fn set_ctx(&self, ctx: crate::wake::Waker) {
        *self.repaint.lock().unwrap() = Some(ctx);
    }
    pub fn set_vad_threshold(&self, v: f32) {
        f32_store(&self.vad_threshold, v)
    }
    pub fn set_in_gain(&self, v: f32) {
        f32_store(&self.in_gain, v)
    }
    /// サウンドボードの音を鳴らす。すでに鳴っている音には重ねる
    pub fn play_effect(&self, pcm: &[f32], volume: f32) {
        let volume = volume.clamp(0.0, 1.0);
        let mut q = self.effects.lock().unwrap();
        let overlap = q.len().min(pcm.len());
        for (v, &s) in q.iter_mut().zip(&pcm[..overlap]) {
            *v = (*v + s * volume).clamp(-1.0, 1.0);
        }
        let room = EFFECT_MAX.saturating_sub(q.len());
        q.extend(pcm[overlap..].iter().take(room).map(|&s| s * volume));
    }

    /// 20 ms 分の効果音を mix に足す。足したら true
    fn mix_effects(&self, mix: &mut [f32]) -> bool {
        let mut q = self.effects.lock().unwrap();
        if q.is_empty() {
            return false;
        }
        let n = q.len().min(mix.len());
        for (m, s) in mix.iter_mut().zip(q.drain(..n)) {
            *m += s;
        }
        true
    }

    pub fn set_noise_suppression(&self, on: bool) {
        self.noise_suppression.store(on, Relaxed)
    }
    pub fn set_out_gain(&self, v: f32) {
        f32_store(&self.out_gain, v)
    }
    pub fn level(&self) -> f32 {
        f32_load(&self.level)
    }
    /// いま送信すべきか
    pub fn gate(&self) -> bool {
        if self.mute.load(Relaxed) || self.deaf.load(Relaxed) {
            return false;
        }
        if self.ptt.load(Relaxed) {
            self.ptt_down.load(Relaxed)
        } else {
            self.vad_open.load(Relaxed)
        }
    }
    /// 画面を描き直させる口（未設定なら何もしない）
    pub fn waker(&self) -> crate::wake::Waker {
        self.repaint.lock().unwrap().clone().unwrap_or_default()
    }
    fn repaint(&self) {
        if let Some(c) = &*self.repaint.lock().unwrap() {
            c.wake();
        }
    }
}

// ---- デバイス一覧 ----

pub fn input_devices() -> Vec<String> {
    let host = cpal::default_host();
    host.input_devices()
        .map(|it| it.filter_map(|d| dev_name(&d)).collect())
        .unwrap_or_default()
}

pub fn output_devices() -> Vec<String> {
    let host = cpal::default_host();
    host.output_devices()
        .map(|it| it.filter_map(|d| dev_name(&d)).collect())
        .unwrap_or_default()
}

fn dev_name(d: &cpal::Device) -> Option<String> {
    d.description().ok().map(|x| x.name().to_string())
}

fn find_device(input: bool, name: Option<&str>) -> Option<cpal::Device> {
    let host = cpal::default_host();
    if let Some(name) = name {
        let found = if input {
            host.input_devices().ok()
        } else {
            host.output_devices().ok()
        }
        .and_then(|mut it| it.find(|d| dev_name(d).as_deref() == Some(name)));
        if found.is_some() {
            return found;
        }
    }
    if input {
        host.default_input_device()
    } else {
        host.default_output_device()
    }
}

// ---- 共有バッファ ----

#[derive(Default)]
struct Ring {
    buf: Mutex<VecDeque<f32>>,
    cv: Condvar,
}

impl Ring {
    fn push(&self, data: &[f32], max: usize) {
        let mut b = self.buf.lock().unwrap();
        b.extend(data);
        let over = b.len().saturating_sub(max);
        b.drain(..over);
        drop(b);
        self.cv.notify_one();
    }
    fn clear(&self) {
        self.buf.lock().unwrap().clear();
    }
}

/// 簡単な線形補間リサンプラ（インターリーブ、チャンネル数は保つ）
struct Resampler {
    ch: usize,
    step: f64,
    pos: f64,
    prev: Vec<f32>,
}

impl Resampler {
    fn new(from: u32, to: u32, ch: usize) -> Self {
        Resampler {
            ch,
            step: from as f64 / to as f64,
            pos: 0.0,
            prev: vec![0.0; ch],
        }
    }
    fn run(&mut self, input: &[f32], out: &mut Vec<f32>) {
        if (self.step - 1.0).abs() < 1e-9 {
            out.extend_from_slice(input);
            return;
        }
        let ch = self.ch;
        let n = input.len() / ch;
        // pos は「prev を -1 番目とした」入力フレーム位置
        while self.pos < n as f64 {
            let i = self.pos.floor() as isize - 1;
            let t = (self.pos - self.pos.floor()) as f32;
            for c in 0..ch {
                let a = if i < 0 {
                    self.prev[c]
                } else {
                    input[i as usize * ch + c]
                };
                let b = input[(i + 1) as usize * ch + c];
                out.push(a + (b - a) * t);
            }
            self.pos += self.step;
        }
        self.pos -= n as f64;
        if n > 0 {
            self.prev.copy_from_slice(&input[(n - 1) * ch..n * ch]);
        }
    }
}

/// 任意チャンネル数 → ステレオ
/// RNNoise でマイクの雑音を抑える。48 kHz のモノラルを 10 ms ずつ処理するので、10 ms 遅れる
struct Denoiser {
    state: Box<nnnoiseless::DenoiseState<'static>>,
    /// まだ 10 ms に満たないモノラルの入力（i16 の幅）
    pending: Vec<f32>,
    frame: Vec<f32>,
}

impl Denoiser {
    fn new() -> Self {
        Self {
            state: nnnoiseless::DenoiseState::new(),
            pending: Vec::with_capacity(nnnoiseless::DenoiseState::FRAME_SIZE * 2),
            frame: vec![0.0; nnnoiseless::DenoiseState::FRAME_SIZE],
        }
    }

    /// 48 kHz ステレオ（交互）を受け取り、処理し終わった分をステレオで返す（長さは呼ぶたびに変わる）
    fn run(&mut self, stereo: &[f32], out: &mut Vec<f32>) {
        const N: usize = nnnoiseless::DenoiseState::FRAME_SIZE;
        out.clear();
        self.pending.extend(
            (0..stereo.len() / 2).map(|i| (stereo[2 * i] + stereo[2 * i + 1]) * 0.5 * 32768.0),
        );
        let mut used = 0;
        while self.pending.len() - used >= N {
            self.state
                .process_frame(&mut self.frame, &self.pending[used..used + N]);
            used += N;
            for &s in &self.frame {
                let s = (s / 32768.0).clamp(-1.0, 1.0);
                out.extend_from_slice(&[s, s]);
            }
        }
        self.pending.drain(..used);
    }
}

fn to_stereo(data: &[f32], ch: usize, out: &mut Vec<f32>) {
    for f in data.chunks_exact(ch) {
        let (l, r) = if ch == 1 { (f[0], f[0]) } else { (f[0], f[1]) };
        out.push(l);
        out.push(r);
    }
}

// ---- 音声入出力スレッド ----

/// cpal のストリームは Send でないので専用スレッドで持つ
pub struct AudioIo {
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Drop for AudioIo {
    fn drop(&mut self) {
        self.stop.store(true, Relaxed);
        if let Some(t) = self.thread.take() {
            t.thread().unpark();
            let _ = t.join();
        }
    }
}

impl AudioIo {
    /// output=false ならマイクだけ開く（マイクテスト用）
    fn start(ctl: Arc<VoiceCtl>, mic: Arc<Ring>, out: Option<Arc<Ring>>) -> AudioIo {
        let stop = Arc::new(AtomicBool::new(false));
        let stop2 = stop.clone();
        let thread = std::thread::spawn(move || {
            while !stop2.load(Relaxed) {
                ctl.restart.store(false, Relaxed);
                let streams = open_streams(&ctl, &mic, out.as_ref());
                if let Err(e) = &streams {
                    eprintln!("audio: {e:#}");
                }
                while !stop2.load(Relaxed) && !ctl.restart.load(Relaxed) {
                    std::thread::park_timeout(Duration::from_millis(200));
                }
                drop(streams);
            }
            f32_store(&ctl.level, 0.0);
            ctl.vad_open.store(false, Relaxed);
        });
        AudioIo {
            stop,
            thread: Some(thread),
        }
    }
}

fn open_streams(
    ctl: &Arc<VoiceCtl>,
    mic: &Arc<Ring>,
    out: Option<&Arc<Ring>>,
) -> Result<Vec<cpal::Stream>> {
    let mut streams = Vec::new();

    // マイク
    let in_name = ctl.input_device.lock().unwrap().clone();
    let dev = find_device(true, in_name.as_deref()).ok_or_else(|| {
        anyhow!(
            "{}",
            tr!(
                "入力デバイスがありません",
                "No input device",
                "没有输入设备",
            )
        )
    })?;
    let cfg = dev.default_input_config()?;
    let ch = cfg.channels() as usize;
    let fmt = cfg.sample_format();
    let config: cpal::StreamConfig = cfg.into();
    let mut rs = Resampler::new(config.sample_rate, RATE, 2);
    let (ctl2, mic2) = (ctl.clone(), mic.clone());
    let mut last_loud = Instant::now() - VAD_HANG;
    let mut st = Vec::new();
    let mut rsd = Vec::new();
    let mut denoised = Vec::new();
    let mut denoiser: Option<Denoiser> = None;
    let mut on_input = move |data: &[f32]| {
        let gain = f32_load(&ctl2.in_gain);
        st.clear();
        to_stereo(data, ch, &mut st);
        st.iter_mut()
            .for_each(|s| *s = (*s * gain).clamp(-1.0, 1.0));
        rsd.clear();
        rs.run(&st, &mut rsd);
        // 雑音を消してから音量を測る（雑音で音声検出が開かないように）
        if ctl2.noise_suppression.load(Relaxed) {
            denoiser
                .get_or_insert_with(Denoiser::new)
                .run(&rsd, &mut denoised);
            std::mem::swap(&mut rsd, &mut denoised);
        } else {
            denoiser = None;
        }
        let rms = if rsd.is_empty() {
            0.0
        } else {
            (rsd.iter().map(|s| s * s).sum::<f32>() / rsd.len() as f32).sqrt()
        };
        let prev = f32_load(&ctl2.level);
        f32_store(&ctl2.level, rms.max(prev * 0.85));
        if rms >= f32_load(&ctl2.vad_threshold) {
            last_loud = Instant::now();
        }
        let open = last_loud.elapsed() < VAD_HANG;
        if ctl2.vad_open.swap(open, Relaxed) != open {
            ctl2.repaint();
        }
        mic2.push(&rsd, MIC_MAX);
    };
    let err = |e| eprintln!("audio input: {e}");
    let s = match fmt {
        cpal::SampleFormat::F32 => {
            dev.build_input_stream::<f32, _, _>(config, move |d, _| on_input(d), err, None)?
        }
        cpal::SampleFormat::I16 => {
            let mut tmp = Vec::new();
            dev.build_input_stream::<i16, _, _>(
                config,
                move |d, _| {
                    tmp.clear();
                    tmp.extend(d.iter().map(|&s| s as f32 / 32768.0));
                    on_input(&tmp)
                },
                err,
                None,
            )?
        }
        f => {
            return Err(anyhow!(
                "{}",
                tr!(
                    "未対応の入力形式 {f:?}",
                    "Unsupported input format {f:?}",
                    "不支持的输入格式 {f:?}",
                )
            ))
        }
    };
    s.play()?;
    streams.push(s);

    // スピーカー
    if let Some(out) = out {
        let out_name = ctl.output_device.lock().unwrap().clone();
        let dev = find_device(false, out_name.as_deref()).ok_or_else(|| {
            anyhow!(
                "{}",
                tr!(
                    "出力デバイスがありません",
                    "No output device",
                    "没有输出设备",
                )
            )
        })?;
        let cfg = dev.default_output_config()?;
        let ch = cfg.channels() as usize;
        let fmt = cfg.sample_format();
        let config: cpal::StreamConfig = cfg.into();
        let mut rs = Resampler::new(RATE, config.sample_rate, 2);
        let (ctl2, out2) = (ctl.clone(), out.clone());
        let mut pending: VecDeque<f32> = VecDeque::with_capacity(FRAME * 4);
        let mut tmp = Vec::with_capacity(FRAME * 2);
        let mut chunk: Vec<f32> = Vec::with_capacity(FRAME);
        let mut fill = move |frames: usize, write: &mut dyn FnMut(usize, f32)| {
            // 必要なだけ 48kHz 側から取り出して変換
            while pending.len() < frames * 2 {
                let mut b = out2.buf.lock().unwrap();
                if b.is_empty() {
                    break;
                }
                let n = b.len().min(FRAME);
                chunk.clear();
                chunk.extend(b.drain(..n));
                drop(b);
                tmp.clear();
                rs.run(&chunk, &mut tmp);
                pending.extend(tmp.iter());
            }
            let gain = f32_load(&ctl2.out_gain);
            let deaf = ctl2.deaf.load(Relaxed);
            for f in 0..frames {
                let (l, r) = if pending.len() >= 2 {
                    (pending.pop_front().unwrap(), pending.pop_front().unwrap())
                } else {
                    (0.0, 0.0)
                };
                let (l, r) = if deaf {
                    (0.0, 0.0)
                } else {
                    (l * gain, r * gain)
                };
                for c in 0..ch {
                    write(f * ch + c, if c % 2 == 0 { l } else { r });
                }
            }
        };
        let err = |e| eprintln!("audio output: {e}");
        let s = match fmt {
            cpal::SampleFormat::F32 => dev.build_output_stream::<f32, _, _>(
                config,
                move |d: &mut [f32], _| {
                    let n = d.len() / ch;
                    fill(n, &mut |i, v| d[i] = v.clamp(-1.0, 1.0));
                },
                err,
                None,
            )?,
            cpal::SampleFormat::I16 => dev.build_output_stream::<i16, _, _>(
                config,
                move |d: &mut [i16], _| {
                    let n = d.len() / ch;
                    fill(n, &mut |i, v| d[i] = (v.clamp(-1.0, 1.0) * 32767.0) as i16);
                },
                err,
                None,
            )?,
            f => {
                return Err(anyhow!(
                    "{}",
                    tr!(
                        "未対応の出力形式 {f:?}",
                        "Unsupported output format {f:?}",
                        "不支持的输出格式 {f:?}",
                    )
                ))
            }
        };
        s.play()?;
        streams.push(s);
    }
    Ok(streams)
}

/// 設定画面のマイクテスト。持っている間だけマイクを開いてレベルを測る
pub struct MicTest {
    _io: AudioIo,
}

impl MicTest {
    pub fn start(ctl: Arc<VoiceCtl>) -> MicTest {
        MicTest {
            _io: AudioIo::start(ctl, Arc::new(Ring::default()), None),
        }
    }
}

// ---- songbird へ渡すマイク入力 ----

/// マイクのリングバッファを f32 LE のバイト列として読ませる
struct MicSource {
    ring: Arc<Ring>,
}

impl Read for MicSource {
    fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
        let want = (out.len() / 4).min(FRAME);
        if want == 0 {
            return Ok(0);
        }
        let mut b = self.ring.buf.lock().unwrap();
        if b.is_empty() {
            // 少しだけ待つ。来なければ無音で埋めて止まらないようにする
            b = self
                .ring
                .cv
                .wait_timeout(b, Duration::from_millis(25))
                .unwrap()
                .0;
        }
        let n = if b.is_empty() {
            want
        } else {
            b.len().min(want)
        };
        for i in 0..n {
            let s = b.pop_front().unwrap_or(0.0);
            out[i * 4..i * 4 + 4].copy_from_slice(&s.to_le_bytes());
        }
        Ok(n * 4)
    }
}

impl Seek for MicSource {
    fn seek(&mut self, _: SeekFrom) -> std::io::Result<u64> {
        Err(std::io::ErrorKind::Unsupported.into())
    }
}

impl symphonia_core::io::MediaSource for MicSource {
    fn is_seekable(&self) -> bool {
        false
    }
    fn byte_len(&self) -> Option<u64> {
        None
    }
}

/// サウンドボードの音（MP3・Ogg Opus など）を 48 kHz ステレオにする
pub fn decode_sound(bytes: Vec<u8>) -> Result<Vec<f32>> {
    use symphonia_core::audio::SampleBuffer;
    use symphonia_core::codecs::DecoderOptions;
    use symphonia_core::errors::Error as SymErr;
    use symphonia_core::formats::FormatOptions;
    use symphonia_core::io::MediaSourceStream;
    use symphonia_core::meta::MetadataOptions;
    use symphonia_core::probe::Hint;

    let mss = MediaSourceStream::new(Box::new(std::io::Cursor::new(bytes)), Default::default());
    let mut format = songbird::input::codecs::get_probe()
        .format(
            &Hint::new(),
            mss,
            &FormatOptions::default(),
            &MetadataOptions::default(),
        )
        .map_err(|e| {
            anyhow!(
                "{}",
                tr!(
                    "形式を読めません: {e}",
                    "Can't read the format: {e}",
                    "无法读取格式：{e}",
                )
            )
        })?
        .format;
    let track = format
        .default_track()
        .ok_or_else(|| anyhow!("{}", tr!("音声がありません", "No audio", "没有音频",)))?;
    let track_id = track.id;
    let mut dec = songbird::input::codecs::get_codec_registry()
        .make(&track.codec_params, &DecoderOptions::default())
        .map_err(|e| {
            anyhow!(
                "{}",
                tr!("復号できません: {e}", "Can't decode: {e}", "无法解码：{e}",)
            )
        })?;
    // コンテナの値は当てにならないこともあるので、復号した音のレートで変換する
    let mut rs: Option<(u32, Resampler)> = None;
    let limit = RATE as usize * 2 * SOUND_MAX_SECS;
    let (mut st, mut out, mut part) = (Vec::new(), Vec::new(), Vec::new());
    loop {
        let packet = match format.next_packet() {
            Ok(p) => p,
            Err(SymErr::IoError(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
            Err(SymErr::ResetRequired) => break,
            Err(e) => {
                return Err(anyhow!(
                    "{}",
                    tr!(
                        "読み込みに失敗: {e}",
                        "Failed to load: {e}",
                        "加载失败：{e}",
                    )
                ))
            }
        };
        if packet.track_id() != track_id {
            continue;
        }
        let buf = match dec.decode(&packet) {
            Ok(b) => b,
            // 壊れた 1 パケットは飛ばす
            Err(SymErr::DecodeError(_)) => continue,
            Err(e) => {
                return Err(anyhow!(
                    "{}",
                    tr!("復号に失敗: {e}", "Failed to decode: {e}", "解码失败：{e}",)
                ))
            }
        };
        let spec = *buf.spec();
        let mut sb = SampleBuffer::<f32>::new(buf.capacity() as u64, spec);
        sb.copy_interleaved_ref(buf);
        st.clear();
        to_stereo(sb.samples(), spec.channels.count().max(1), &mut st);
        part.clear();
        if rs.as_ref().is_none_or(|(r, _)| *r != spec.rate) {
            rs = Some((spec.rate, Resampler::new(spec.rate, RATE, 2)));
        }
        rs.as_mut().unwrap().1.run(&st, &mut part);
        out.extend_from_slice(&part);
        if out.len() >= limit {
            out.truncate(limit);
            break;
        }
    }
    if out.is_empty() {
        return Err(anyhow!(
            "{}",
            tr!("音声がありません", "No audio", "没有音频",)
        ));
    }
    Ok(out)
}

// ---- 受信 ----

struct Receiver {
    out: Arc<Ring>,
    ctl: Arc<VoiceCtl>,
    users: Arc<Mutex<HashMap<u32, String>>>,
    /// 20msごとの作業領域（毎回確保しない）
    scratch: Mutex<(Vec<f32>, HashSet<String>)>,
}

#[async_trait::async_trait]
impl EventHandler for Receiver {
    async fn act(&self, ctx: &EventContext<'_>) -> Option<SbEvent> {
        match ctx {
            EventContext::SpeakingStateUpdate(s) => {
                if let Some(u) = s.user_id {
                    self.users.lock().unwrap().insert(s.ssrc, u.0.to_string());
                }
            }
            EventContext::VoiceTick(t) => {
                let mut guard = self.scratch.lock().unwrap();
                let (mix, talking) = &mut *guard;
                mix.clear();
                mix.resize(FRAME, 0.0);
                talking.clear();
                let users = self.users.lock().unwrap();
                for (ssrc, data) in &t.speaking {
                    let Some(pcm) = &data.decoded_voice else {
                        continue;
                    };
                    let mut energy = 0f32;
                    for (m, &s) in mix.iter_mut().zip(pcm.iter()) {
                        let v = s as f32 / 32768.0;
                        *m += v;
                        energy += v * v;
                    }
                    if !pcm.is_empty() && (energy / pcm.len() as f32).sqrt() > 0.003 {
                        if let Some(u) = users.get(ssrc) {
                            talking.insert(u.clone());
                        }
                    }
                }
                drop(users);
                let effects = self.ctl.mix_effects(mix);
                if !t.speaking.is_empty() || effects {
                    self.out.push(mix, OUT_MAX);
                }
                // 変化したときだけ UI を再描画する
                let mut cur = self.ctl.speaking.lock().unwrap();
                if *cur != *talking {
                    std::mem::swap(&mut *cur, talking);
                    drop(cur);
                    self.ctl.repaint();
                }
            }
            EventContext::ClientDisconnect(d) => {
                let id = d.user_id.0.to_string();
                self.users.lock().unwrap().retain(|_, u| *u != id);
            }
            _ => {}
        }
        None
    }
}

// ---- セッション ----

pub struct VoiceSession {
    driver: Driver,
    ctl: Arc<VoiceCtl>,
    mic: Arc<Ring>,
    out: Arc<Ring>,
    io: Option<AudioIo>,
    muted: bool,
}

impl VoiceSession {
    pub fn new(ctl: Arc<VoiceCtl>) -> Self {
        let config = Config::default().decode_mode(DecodeMode::Decode(DecodeConfig::new(
            Channels::Stereo,
            SbRate::Hz48000,
        )));
        let mut driver = Driver::new(config);
        let mic = Arc::new(Ring::default());
        let out = Arc::new(Ring::default());
        let users = Arc::new(Mutex::new(HashMap::new()));
        for ev in [
            CoreEvent::SpeakingStateUpdate,
            CoreEvent::VoiceTick,
            CoreEvent::ClientDisconnect,
        ] {
            driver.add_global_event(
                SbEvent::Core(ev),
                Receiver {
                    out: out.clone(),
                    ctl: ctl.clone(),
                    users: users.clone(),
                    scratch: Mutex::new((Vec::with_capacity(FRAME), HashSet::new())),
                },
            );
        }
        driver.mute(true);
        VoiceSession {
            driver,
            ctl,
            mic,
            out,
            io: None,
            muted: true,
        }
    }

    /// guild_id が無い（DM通話）ときは channel_id をサーバーIDとして使う
    pub async fn connect(
        &mut self,
        guild_id: Option<&str>,
        channel_id: &str,
        endpoint: &str,
        session_id: &str,
        token: &str,
        user_id: &str,
    ) -> Result<()> {
        let id = |s: &str| -> Result<NonZeroU64> {
            s.parse::<NonZeroU64>().map_err(|_| {
                anyhow!(
                    "{}",
                    tr!("不正なID {s}", "Invalid ID {s}", "无效的 ID {s}",)
                )
            })
        };
        let endpoint = endpoint
            .trim_start_matches("wss://")
            .trim_end_matches('/')
            .to_string();
        let info = ConnectionInfo {
            channel_id: id(channel_id)?.into(),
            endpoint,
            guild_id: id(guild_id.unwrap_or(channel_id))?.into(),
            session_id: session_id.into(),
            token: token.into(),
            user_id: id(user_id)?.into(),
        };
        self.driver.connect(info).await?;
        self.mic.clear();
        self.out.clear();
        if self.io.is_none() {
            self.io = Some(AudioIo::start(
                self.ctl.clone(),
                self.mic.clone(),
                Some(self.out.clone()),
            ));
        }
        let input: Input = RawAdapter::new(
            MicSource {
                ring: self.mic.clone(),
            },
            RATE,
            2,
        )
        .into();
        self.driver.play_only_input(input);
        self.muted = true;
        self.driver.mute(true);
        Ok(())
    }

    pub fn leave(&mut self) {
        self.driver.stop();
        self.driver.leave();
        self.io = None;
        self.ctl.speaking.lock().unwrap().clear();
        self.ctl.transmitting.store(false, Relaxed);
    }

    /// 20msごとに呼び、送信のオンオフを反映する
    pub fn tick(&mut self) {
        if self.io.is_none() {
            return;
        }
        let open = self.ctl.gate();
        if open == self.muted {
            self.muted = !open;
            self.driver.mute(!open);
            self.ctl.transmitting.store(open, Relaxed);
            self.ctl.repaint();
        }
        if !open {
            // 送っていない間のマイク音は捨てる（再開時に古い音が出ないように）
            self.mic.clear();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore]
    fn bench_resampler() {
        for rate in [48_000, 44_100] {
            let input: Vec<f32> = (0..(rate as usize / 50 * 2))
                .map(|i| ((i % 100) as f32 / 50.0) - 1.0)
                .collect();
            let mut resampler = Resampler::new(rate, RATE, 2);
            let mut output = Vec::with_capacity(FRAME * 2);
            let start = Instant::now();
            for _ in 0..10_000 {
                output.clear();
                resampler.run(std::hint::black_box(&input), &mut output);
                std::hint::black_box(&output);
            }
            println!(
                "resample {rate} Hz, 20 ms stereo: {:.3} µs/frame",
                start.elapsed().as_secs_f64() * 100.0
            );
        }
    }

    #[test]
    #[ignore]
    fn bench_voice_mix() {
        for speakers in [1, 10, 50] {
            let pcm: Vec<Vec<i16>> = (0..speakers)
                .map(|speaker| {
                    (0..FRAME)
                        .map(|i| ((i * 17 + speaker * 97) % 16_000) as i16)
                        .collect()
                })
                .collect();
            let mut mix = vec![0.0_f32; FRAME];
            let start = Instant::now();
            for _ in 0..1000 {
                mix.fill(0.0);
                for samples in &pcm {
                    let mut energy = 0.0_f32;
                    for (m, &s) in mix.iter_mut().zip(std::hint::black_box(samples).iter()) {
                        let v = s as f32 / 32768.0;
                        *m += v;
                        energy += v * v;
                    }
                    std::hint::black_box((energy / samples.len() as f32).sqrt() > 0.003);
                }
                std::hint::black_box(&mix);
            }
            println!(
                "mix {speakers} speakers, 20 ms stereo: {:.3} µs/tick",
                start.elapsed().as_secs_f64() * 1000.0
            );
        }
    }

    #[test]
    fn effects_overlap_and_drain_into_mix() {
        let ctl = VoiceCtl::default();
        ctl.play_effect(&[0.5; 4], 1.0);
        ctl.play_effect(&[0.25; 6], 0.5);
        let mut mix = [0.0f32; 4];
        assert!(ctl.mix_effects(&mut mix));
        assert_eq!(mix, [0.625; 4]);
        let mut mix = [0.0f32; 4];
        assert!(ctl.mix_effects(&mut mix));
        assert_eq!(mix, [0.125, 0.125, 0.0, 0.0]);
        assert!(!ctl.mix_effects(&mut mix));
    }

    #[test]
    fn decode_sound_reads_mp3_and_ogg_opus_at_48k_stereo() {
        for (name, bytes) in [
            (
                "mp3",
                &include_bytes!("../tests/fixtures/soundboard-sine.mp3")[..],
            ),
            (
                "ogg",
                &include_bytes!("../tests/fixtures/soundboard-sine.ogg")[..],
            ),
        ] {
            let pcm = decode_sound(bytes.to_vec()).unwrap_or_else(|e| panic!("{name}: {e}"));
            // 0.5 秒の 440 Hz（ステレオ 48 kHz で約 48000 サンプル）
            let secs = pcm.len() as f32 / (RATE as f32 * 2.0);
            assert!((0.4..0.7).contains(&secs), "{name}: {secs}");
            let peak = pcm.iter().fold(0f32, |m, s| m.max(s.abs()));
            // ffmpeg の sine は振幅 1/8
            assert!(peak > 0.05, "{name}: {peak}");
        }
    }

    #[test]
    fn decode_sound_rejects_garbage() {
        assert!(decode_sound(b"not audio at all".to_vec()).is_err());
    }

    #[test]
    fn denoiser_keeps_length_and_quiets_noise() {
        let mut d = Denoiser::new();
        let mut out = Vec::new();
        let mut total_in = 0;
        let mut total_out = 0;
        let mut energy_in = 0.0f32;
        let mut energy_out = 0.0f32;
        let mut seed = 1u32;
        // 決まった長さでない塊で渡しても、10 ms ずつ処理して全部返す
        for (i, len) in [100usize, 333, 960, 7, 2000]
            .iter()
            .cycle()
            .take(200)
            .enumerate()
        {
            let input: Vec<f32> = (0..*len * 2)
                .map(|_| {
                    seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12345);
                    ((seed >> 16) as f32 / 32768.0 - 1.0) * 0.05
                })
                .collect();
            total_in += input.len();
            d.run(&input, &mut out);
            total_out += out.len();
            if i > 50 {
                energy_in += input.iter().map(|s| s * s).sum::<f32>();
                energy_out += out.iter().map(|s| s * s).sum::<f32>();
            }
        }
        assert!(total_in - total_out < 480 * 2);
        assert_eq!(total_out % 2, 0);
        assert!(energy_out < energy_in * 0.5, "{energy_out} vs {energy_in}");
    }

    #[test]
    fn resample_identity_and_ratio() {
        let mut r = Resampler::new(48_000, 48_000, 2);
        let mut o = Vec::new();
        r.run(&[1.0, 2.0, 3.0, 4.0], &mut o);
        assert_eq!(o, [1.0, 2.0, 3.0, 4.0]);

        let mut r = Resampler::new(44_100, 48_000, 2);
        let input: Vec<f32> = (0..44_100 * 2).map(|i| (i / 2) as f32).collect();
        let mut o = Vec::new();
        for c in input.chunks(882) {
            r.run(c, &mut o);
        }
        let frames = o.len() / 2;
        assert!((frames as i64 - 48_000).abs() <= 2, "{frames}");
    }
}
