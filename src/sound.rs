//! 通知音などの短い効果音と読み上げ（通話とは別の出力で鳴らす）。
//! 設定フォルダの riscord/sounds/<名前>.(ogg|mp3) があればそれを、無ければ作った音を鳴らす
use std::sync::atomic::{AtomicUsize, Ordering};

/// 同時に鳴らす数の上限（キー音などで出力を開きすぎない）
static PLAYING: AtomicUsize = AtomicUsize::new(0);

pub fn sounds_dir() -> std::path::PathBuf {
    crate::app::config_root().join("riscord/sounds")
}

/// 名前の効果音を鳴らす（volume は 0〜2 程度の倍率）
pub fn play(name: &str, volume: f32) {
    if cfg!(test) || PLAYING.load(Ordering::Relaxed) >= 4 {
        return;
    }
    let name: String = name
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .take(40)
        .collect();
    PLAYING.fetch_add(1, Ordering::Relaxed);
    std::thread::spawn(move || {
        if let Err(e) = play_blocking(&name, volume) {
            eprintln!("効果音を鳴らせませんでした: {e:#}");
        }
        PLAYING.fetch_sub(1, Ordering::Relaxed);
    });
}

fn play_blocking(name: &str, volume: f32) -> anyhow::Result<()> {
    use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
    let dev = cpal::default_host()
        .default_output_device()
        .ok_or_else(|| anyhow::anyhow!("no output device"))?;
    let cfg = dev.default_output_config()?;
    let ch = cfg.channels() as usize;
    let fmt = cfg.sample_format();
    let config: cpal::StreamConfig = cfg.into();
    let rate = config.sample_rate as f32;
    let samples = load_custom(name, rate).unwrap_or_else(|| synth(name, rate));
    let len = samples.len();
    let duration = std::time::Duration::from_secs_f32(len as f32 / rate + 0.15);
    let volume = volume.clamp(0.0, 3.0);
    let mut pos = 0usize;
    let mut next = move || {
        let v = samples.get(pos).copied().unwrap_or(0.0) * volume;
        pos += 1;
        v.clamp(-1.0, 1.0)
    };
    let err = |e| eprintln!("sound output: {e}");
    let stream = match fmt {
        cpal::SampleFormat::F32 => dev.build_output_stream::<f32, _, _>(
            config,
            move |d: &mut [f32], _| {
                for frame in d.chunks_mut(ch) {
                    let v = next();
                    frame.fill(v);
                }
            },
            err,
            None,
        )?,
        cpal::SampleFormat::I16 => dev.build_output_stream::<i16, _, _>(
            config,
            move |d: &mut [i16], _| {
                for frame in d.chunks_mut(ch) {
                    let v = (next() * 32767.0) as i16;
                    frame.fill(v);
                }
            },
            err,
            None,
        )?,
        f => anyhow::bail!("unsupported format {f:?}"),
    };
    stream.play()?;
    std::thread::sleep(duration);
    Ok(())
}

/// 作った音（モノラル、rate Hz）
pub fn synth(name: &str, rate: f32) -> Vec<f32> {
    // (周波数, 長さ秒) の並び
    let notes: &[(f32, f32)] = match name {
        "mention" => &[(880.0, 0.09), (1318.5, 0.16)],
        "join" => &[(523.3, 0.08), (784.0, 0.14)],
        "leave" => &[(784.0, 0.08), (523.3, 0.14)],
        "mute" => &[(330.0, 0.12)],
        "unmute" => &[(660.0, 0.10)],
        "deafen" => &[(262.0, 0.15)],
        "key" => &[(1800.0, 0.012)],
        "key2" => &[(1200.0, 0.015)],
        "key3" => &[(2400.0, 0.010)],
        "ptt" => &[(440.0, 0.05)],
        "reminder" => &[(659.3, 0.1), (659.3, 0.1), (880.0, 0.2)],
        "friend" => &[(587.3, 0.08), (880.0, 0.12)],
        "error" => &[(200.0, 0.2)],
        _ => &[(987.8, 0.07), (1318.5, 0.12)],
    };
    let mut out = Vec::new();
    for &(f, secs) in notes {
        let n = (secs * rate) as usize;
        for i in 0..n {
            let t = i as f32 / rate;
            // 立ち上がりを少しなだらかにし、指数で減衰させる
            let env = (i as f32 / (rate * 0.004)).min(1.0) * (-t * 18.0 / secs.max(0.05)).exp();
            out.push((t * f * std::f32::consts::TAU).sin() * env * 0.35);
        }
    }
    out
}

/// sounds フォルダの音声ファイルを読む（モノラルにして rate へ雑に合わせる）
fn load_custom(name: &str, rate: f32) -> Option<Vec<f32>> {
    use symphonia::core::{
        audio::SampleBuffer, codecs::DecoderOptions, formats::FormatOptions, io::MediaSourceStream,
        meta::MetadataOptions, probe::Hint,
    };
    let dir = sounds_dir();
    let path = ["ogg", "mp3"]
        .iter()
        .map(|ext| dir.join(format!("{name}.{ext}")))
        .find(|p| p.is_file())?;
    let file = std::fs::File::open(&path).ok()?;
    if file.metadata().ok()?.len() > 4 * 1024 * 1024 {
        return None;
    }
    let mss = MediaSourceStream::new(Box::new(file), Default::default());
    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }
    let probed = symphonia::default::get_probe()
        .format(
            &hint,
            mss,
            &FormatOptions::default(),
            &MetadataOptions::default(),
        )
        .ok()?;
    let mut format = probed.format;
    let track = format.default_track()?;
    let track_id = track.id;
    let mut decoder = symphonia::default::get_codecs()
        .make(&track.codec_params, &DecoderOptions::default())
        .ok()?;
    let mut mono = Vec::new();
    let mut src_rate = 48_000.0;
    while let Ok(packet) = format.next_packet() {
        if packet.track_id() != track_id {
            continue;
        }
        let Ok(decoded) = decoder.decode(&packet) else {
            continue;
        };
        let spec = *decoded.spec();
        src_rate = spec.rate as f32;
        let channels = spec.channels.count().max(1);
        let mut buf = SampleBuffer::<f32>::new(decoded.capacity() as u64, spec);
        buf.copy_interleaved_ref(decoded);
        for frame in buf.samples().chunks(channels) {
            mono.push(frame.iter().sum::<f32>() / channels as f32);
        }
        if mono.len() > 48_000 * 10 {
            break;
        }
    }
    resample_nearest(&mono, src_rate, rate)
}

/// 受け付けるサンプリングレート。外れた値だと step がほぼ 0 になり、巨大な確保をしてしまう
const RATE_RANGE: std::ops::RangeInclusive<f32> = 8_000.0..=192_000.0;
/// 鳴らす音の長さの上限（秒）
const MAX_SECS: f32 = 10.0;

/// mono（src_rate）を rate へ一番近いサンプルで合わせる。レートがおかしければ None
fn resample_nearest(mono: &[f32], src_rate: f32, rate: f32) -> Option<Vec<f32>> {
    if mono.is_empty() || !RATE_RANGE.contains(&src_rate) || !RATE_RANGE.contains(&rate) {
        return None;
    }
    let step = src_rate / rate;
    let n = ((mono.len() as f32 / step) as usize).min((rate * MAX_SECS) as usize);
    Some(
        (0..n)
            .map(|i| mono[((i as f32 * step) as usize).min(mono.len() - 1)])
            .collect(),
    )
}

/// 文字列を読み上げる（Linux は spd-say、macOS は say、Windows は SAPI）
pub fn speak(text: &str) {
    if cfg!(test) {
        return;
    }
    let text: String = text.chars().filter(|c| !c.is_control()).take(300).collect();
    if text.trim().is_empty() {
        return;
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    let r = std::process::Command::new("spd-say")
        .arg("--")
        .arg(&text)
        .spawn();
    #[cfg(target_os = "macos")]
    let r = std::process::Command::new("say")
        .arg("--")
        .arg(&text)
        .spawn();
    #[cfg(windows)]
    let r = {
        std::process::Command::new("powershell")
            .args([
                "-NoProfile",
                "-Command",
                "Add-Type -AssemblyName System.Speech; (New-Object System.Speech.Synthesis.SpeechSynthesizer).Speak($env:RISCORD_SPEECH_TEXT)",
            ])
            .env("RISCORD_SPEECH_TEXT", &text)
            .spawn()
    };
    if r.is_err() {
        play("message", 1.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resample_nearest_rejects_bad_rates_and_caps_length() {
        let mono = vec![0.5; 1000];
        assert!(resample_nearest(&mono, 0.0, 48_000.0).is_none());
        assert!(resample_nearest(&mono, 1.0, 48_000.0).is_none());
        assert!(resample_nearest(&mono, f32::NAN, 48_000.0).is_none());
        assert!(resample_nearest(&[], 48_000.0, 48_000.0).is_none());
        assert_eq!(
            resample_nearest(&mono, 24_000.0, 48_000.0).unwrap().len(),
            2000
        );
        let long = vec![0.0; 48_000 * 10];
        let out = resample_nearest(&long, 8_000.0, 192_000.0).unwrap();
        assert_eq!(out.len(), 192_000 * 10);
    }
}
