//! soundbank -- the game's loaded audio: music, sound effects, character
//! voices, exported from a studio as `.opus` (or `.wav`).
//!
//! ## Layout
//!
//! Everything under one root (`CLAWBOUND_AUDIO_DIR`, else `assets/audio`
//! next to the executable, else `assets/audio` in the working directory).
//! A clip's KEY is its path under the root without the extension, and a
//! trailing `.N` makes it take N of that key:
//!
//! ```text
//! music/pass.opus                      key "music/pass"
//! sfx/hit.opus                         key "sfx/hit"
//! voice/goldenmane/cast.1.opus         key "voice/goldenmane/cast", take 1
//! voice/goldenmane/cast.2.opus         ...take 2 (a random take plays,
//!                                         never the same one twice running)
//! announcer/firstblood.opus
//! ```
//!
//! Keys the game asks for are in `cues`; a key with no file is silence (the
//! synthesized ambience and fanfare still play), so audio can be added a
//! line at a time.
//!
//! ## No hangs
//!
//! [`Loader::start`] decodes on a background thread; the game polls
//! ([`Bank::poll`]) and hands each finished clip to the mixer. Nothing is
//! read or decoded on the frame thread or the audio thread, whatever the
//! format -- which is what keeps any format latency-safe. Opus decodes at
//! 48 kHz (its native rate) and is resampled once, at load, if the device
//! runs at another.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::sync::Arc;

use crate::mixer::{Cmd, MixerHandle};

/// A decoded clip: interleaved f32, 1 or 2 channels, at the device rate.
#[derive(Debug, Clone)]
pub struct Clip {
    pub pcm: Arc<[f32]>,
    pub channels: u8,
}

impl Clip {
    pub fn frames(&self) -> usize {
        self.pcm.len() / self.channels.max(1) as usize
    }
}

#[derive(Debug)]
pub enum LoadError {
    Io(std::io::Error),
    Format(String),
}

impl std::fmt::Display for LoadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LoadError::Io(e) => write!(f, "{e}"),
            LoadError::Format(s) => write!(f, "{s}"),
        }
    }
}

/// Decode an Ogg Opus file to interleaved f32 at `rate`.
pub fn decode_opus(bytes: &[u8], rate: u32) -> Result<Clip, LoadError> {
    use opus_pure::{OggOpusReader, Trim, MAX_PACKET_SAMPLES};
    let fmt = |e: opus_pure::Error| LoadError::Format(format!("opus: {e}"));
    let mut reader = OggOpusReader::new(std::io::Cursor::new(bytes)).map_err(fmt)?;
    let channels = reader.head().channel_count as usize;
    if !(1..=2).contains(&channels) {
        return Err(LoadError::Format(format!("opus: {channels} channels (mono or stereo only)")));
    }
    let mut decoder = reader.head().decoder(48_000).map_err(fmt)?;
    let mut trim = Trim::new(reader.head(), 48_000, channels).map_err(fmt)?;
    let mut block = vec![0.0f32; MAX_PACKET_SAMPLES * channels];
    let mut pcm = Vec::new();
    for packet in reader.packets() {
        let packet = packet.map_err(fmt)?;
        let n = decoder.decode(&packet.data, MAX_PACKET_SAMPLES, &mut block).map_err(fmt)?;
        pcm.extend_from_slice(trim.keep(&packet, &block[..n * channels]));
    }
    Ok(Clip { pcm: resample(&pcm, channels, 48_000, rate).into(), channels: channels as u8 })
}

/// Decode a RIFF WAV (PCM 16-bit, or IEEE float 32-bit; mono or stereo).
pub fn decode_wav(bytes: &[u8], rate: u32) -> Result<Clip, LoadError> {
    let bad = |s: &str| LoadError::Format(format!("wav: {s}"));
    if bytes.len() < 12 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Err(bad("not RIFF/WAVE"));
    }
    let (mut fmt, mut data) = (None, None);
    let mut i = 12;
    while i + 8 <= bytes.len() {
        let id = &bytes[i..i + 4];
        let len = u32::from_le_bytes([bytes[i + 4], bytes[i + 5], bytes[i + 6], bytes[i + 7]]) as usize;
        let body = bytes.get(i + 8..i + 8 + len).ok_or_else(|| bad("truncated chunk"))?;
        match id {
            b"fmt " => fmt = Some(body),
            b"data" => data = Some(body),
            _ => {}
        }
        i += 8 + len + (len & 1);
    }
    let fmt = fmt.ok_or_else(|| bad("no fmt chunk"))?;
    let data = data.ok_or_else(|| bad("no data chunk"))?;
    if fmt.len() < 16 {
        return Err(bad("short fmt"));
    }
    let tag = u16::from_le_bytes([fmt[0], fmt[1]]);
    let channels = u16::from_le_bytes([fmt[2], fmt[3]]) as usize;
    let src_rate = u32::from_le_bytes([fmt[4], fmt[5], fmt[6], fmt[7]]);
    let bits = u16::from_le_bytes([fmt[14], fmt[15]]);
    if !(1..=2).contains(&channels) {
        return Err(bad("mono or stereo only"));
    }
    let pcm: Vec<f32> = match (tag, bits) {
        (1, 16) => data.chunks_exact(2).map(|b| i16::from_le_bytes([b[0], b[1]]) as f32 / 32768.0).collect(),
        (3, 32) => data.chunks_exact(4).map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]])).collect(),
        _ => return Err(bad(&format!("format {tag}/{bits}-bit (PCM16 or float32 only)"))),
    };
    Ok(Clip { pcm: resample(&pcm, channels, src_rate, rate).into(), channels: channels as u8 })
}

/// Linear resampling, done once at load. Identity when the rates match.
pub fn resample(pcm: &[f32], channels: usize, from: u32, to: u32) -> Vec<f32> {
    if from == to || from == 0 || to == 0 || pcm.is_empty() {
        return pcm.to_vec();
    }
    let frames = pcm.len() / channels;
    let out_frames = ((frames as u64 * to as u64) / from as u64) as usize;
    let step = from as f64 / to as f64;
    let mut out = Vec::with_capacity(out_frames * channels);
    for f in 0..out_frames {
        let src = f as f64 * step;
        let i = src as usize;
        let t = (src - i as f64) as f32;
        let j = (i + 1).min(frames - 1);
        for c in 0..channels {
            let a = pcm[i * channels + c];
            let b = pcm[j * channels + c];
            out.push(a + (b - a) * t);
        }
    }
    out
}

/// A file's key and take from its path under the root:
/// `voice/goldenmane/cast.2.opus` -> ("voice/goldenmane/cast", 2).
pub fn key_of(root: &Path, file: &Path) -> Option<(String, u32)> {
    let rel = file.strip_prefix(root).ok()?;
    let ext = rel.extension()?.to_str()?.to_ascii_lowercase();
    if ext != "opus" && ext != "wav" {
        return None;
    }
    let stem = rel.with_extension("");
    let mut key = stem.to_str()?.replace('\\', "/");
    let mut take = 0;
    if let Some((base, n)) = key.rsplit_once('.') {
        if let Ok(n) = n.parse::<u32>() {
            take = n;
            key = base.to_string();
        }
    }
    Some((key.to_ascii_lowercase(), take))
}

/// Where the audio lives: `CLAWBOUND_AUDIO_DIR`, else `assets/audio` by the
/// executable, else `assets/audio` in the working directory.
pub fn default_root() -> PathBuf {
    if let Some(d) = std::env::var_os("CLAWBOUND_AUDIO_DIR") {
        return PathBuf::from(d);
    }
    if let Some(exe_dir) = std::env::current_exe().ok().and_then(|p| p.parent().map(Path::to_path_buf)) {
        let p = exe_dir.join("assets/audio");
        if p.is_dir() {
            return p;
        }
    }
    PathBuf::from("assets/audio")
}

/// One decoded file, from the loader thread.
pub struct Loaded {
    pub key: String,
    pub take: u32,
    pub result: Result<Clip, LoadError>,
    pub path: PathBuf,
}

/// The background decoder.
pub struct Loader;

impl Loader {
    /// Walk `root` on a new thread, decode every `.opus`/`.wav` at `rate`,
    /// and send each as it finishes. A missing root sends nothing.
    pub fn start(root: PathBuf, rate: u32) -> mpsc::Receiver<Loaded> {
        let (tx, rx) = mpsc::channel();
        std::thread::Builder::new()
            .name("audio-loader".into())
            .spawn(move || {
                let mut files = Vec::new();
                walk(&root, &mut files);
                files.sort();
                for path in files {
                    let Some((key, take)) = key_of(&root, &path) else { continue };
                    let result = std::fs::read(&path).map_err(LoadError::Io).and_then(|b| {
                        if path.extension().is_some_and(|e| e.eq_ignore_ascii_case("wav")) {
                            decode_wav(&b, rate)
                        } else {
                            decode_opus(&b, rate)
                        }
                    });
                    if tx.send(Loaded { key, take, result, path }).is_err() {
                        return;
                    }
                }
            })
            .ok();
        rx
    }
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            walk(&p, out);
        } else {
            out.push(p);
        }
    }
}

/// The game's view of the loaded audio: keys to clip ids (the mixer's
/// table), with takes.
#[derive(Default)]
pub struct Bank {
    keys: HashMap<String, Vec<u32>>,
    /// Every clip, kept alive here so the audio thread never frees one.
    clips: Vec<Clip>,
    last: HashMap<String, u32>,
    rng: u32,
    pub errors: Vec<String>,
    rx: Option<mpsc::Receiver<Loaded>>,
}

impl Bank {
    pub fn loading(rx: mpsc::Receiver<Loaded>) -> Bank {
        Bank { rx: Some(rx), rng: 0x9E37_79B9, ..Default::default() }
    }

    /// Take what the loader has finished (never waits) and give it to the
    /// mixer. Returns how many clips arrived.
    pub fn poll(&mut self, mixer: &mut MixerHandle) -> usize {
        let mut n = 0;
        let Some(rx) = self.rx.as_ref() else { return 0 };
        while let Ok(l) = rx.try_recv() {
            match l.result {
                Ok(clip) => {
                    let id = self.clips.len() as u32;
                    if id as usize >= crate::mixer::MAX_CLIPS {
                        self.errors.push(format!("{}: over {} clips", l.path.display(), crate::mixer::MAX_CLIPS));
                        continue;
                    }
                    mixer.send(Cmd::Load { id, pcm: clip.pcm.clone(), channels: clip.channels });
                    self.clips.push(clip);
                    self.keys.entry(l.key).or_default().push(id);
                    n += 1;
                }
                Err(e) => self.errors.push(format!("{}: {e}", l.path.display())),
            }
        }
        n
    }

    /// Add a clip directly (tests; generated audio).
    pub fn insert(&mut self, key: &str, clip: Clip, mixer: &mut MixerHandle) -> u32 {
        let id = self.clips.len() as u32;
        mixer.send(Cmd::Load { id, pcm: clip.pcm.clone(), channels: clip.channels });
        self.clips.push(clip);
        self.keys.entry(key.to_string()).or_default().push(id);
        id
    }

    /// How long clip `id` plays, in seconds, at `rate`.
    pub fn secs(&self, id: u32, rate: u32) -> f32 {
        self.clips.get(id as usize).map_or(0.0, |c| c.frames() as f32 / rate.max(1) as f32)
    }

    pub fn has(&self, key: &str) -> bool {
        self.keys.contains_key(key)
    }

    pub fn len(&self) -> usize {
        self.clips.len()
    }

    /// A take of `key`: random, but not the one played last time.
    pub fn pick(&mut self, key: &str) -> Option<u32> {
        let ids = self.keys.get(key)?;
        if ids.len() == 1 {
            return Some(ids[0]);
        }
        let mut x = self.rng;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.rng = x;
        let last = self.last.get(key).copied();
        let mut i = (x as usize) % ids.len();
        if Some(ids[i]) == last {
            i = (i + 1) % ids.len();
        }
        self.last.insert(key.to_string(), ids[i]);
        Some(ids[i])
    }

    /// The first key of `keys` that has audio, and a take of it.
    pub fn first(&mut self, keys: &[String]) -> Option<u32> {
        keys.iter().find(|k| self.has(k)).cloned().and_then(|k| self.pick(&k))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wav16(samples: &[i16], channels: u16, rate: u32) -> Vec<u8> {
        let data: Vec<u8> = samples.iter().flat_map(|s| s.to_le_bytes()).collect();
        let mut b = Vec::new();
        b.extend_from_slice(b"RIFF");
        b.extend_from_slice(&(36 + data.len() as u32).to_le_bytes());
        b.extend_from_slice(b"WAVEfmt ");
        b.extend_from_slice(&16u32.to_le_bytes());
        b.extend_from_slice(&1u16.to_le_bytes());
        b.extend_from_slice(&channels.to_le_bytes());
        b.extend_from_slice(&rate.to_le_bytes());
        b.extend_from_slice(&(rate * channels as u32 * 2).to_le_bytes());
        b.extend_from_slice(&(channels * 2).to_le_bytes());
        b.extend_from_slice(&16u16.to_le_bytes());
        b.extend_from_slice(b"data");
        b.extend_from_slice(&(data.len() as u32).to_le_bytes());
        b.extend_from_slice(&data);
        b
    }

    /// Encode with the same pure-Rust codec a studio's `opusenc` output is
    /// read with, as a studio export would arrive.
    fn opus_file(pcm: &[f32], channels: usize) -> Vec<u8> {
        use opus_pure::{Application, OggOpusWriter, OpusEncoder, OpusHead, MAX_PACKET_BYTES};
        let frame = 960;
        let mut enc = OpusEncoder::new(48_000, channels, Application::Audio).unwrap();
        enc.bitrate_bps = 96_000;
        let head = OpusHead::for_encoder(&enc, 48_000);
        let mut out = Vec::new();
        {
            let mut w = OggOpusWriter::new(&mut out, head).unwrap();
            let mut packet = vec![0u8; MAX_PACKET_BYTES];
            let mut padded = pcm.to_vec();
            padded.resize(pcm.len().div_ceil(frame * channels) * frame * channels + frame * channels, 0.0);
            for block in padded.chunks_exact(frame * channels) {
                let n = enc.encode(block, frame, &mut packet).unwrap();
                w.write_packet(&packet[..n]).unwrap();
            }
            w.finish().unwrap();
        }
        out
    }

    #[test]
    fn a_wav_decodes_exactly_and_resamples() {
        let s: Vec<i16> = (0..480).map(|i| ((i as f32 * 0.1).sin() * 16000.0) as i16).collect();
        let c = decode_wav(&wav16(&s, 1, 48_000), 48_000).unwrap();
        assert_eq!(c.frames(), 480);
        assert!((c.pcm[10] - s[10] as f32 / 32768.0).abs() < 1e-6);
        let c = decode_wav(&wav16(&s, 1, 24_000), 48_000).unwrap();
        assert_eq!(c.frames(), 960, "24 kHz doubled to 48");
        assert!(decode_wav(b"not a wav at all", 48_000).is_err());
    }

    /// A real Opus round trip: the game's own fanfare, encoded to .opus at
    /// 96 kbps, decoded back -- the right length, and faithful (SNR).
    #[test]
    fn an_opus_file_decodes_faithfully() {
        let src = crate::brass::Fanfare::render_all(48_000.0);
        let file = opus_file(&src, 1);
        assert!(file.len() < src.len() * 4 / 8, "opus should be far smaller than f32 PCM: {} vs {}", file.len(), src.len() * 4);
        let clip = decode_opus(&file, 48_000).unwrap();
        assert_eq!(clip.channels, 1);
        let n = src.len().min(clip.frames());
        assert!(clip.frames() >= src.len() && clip.frames() < src.len() + 2 * 960, "length {} vs {}", clip.frames(), src.len());
        let (sig, err) = (0..n).fold((0.0f64, 0.0f64), |(s, e), i| (s + (src[i] as f64).powi(2), e + (src[i] as f64 - clip.pcm[i] as f64).powi(2)));
        let snr = 10.0 * (sig / err.max(1e-12)).log10();
        println!("opus round trip: {} bytes for {:.2} s; SNR {snr:.1} dB", file.len(), src.len() as f32 / 48_000.0);
        assert!(snr > 15.0, "decoded audio does not match: SNR {snr:.1} dB");
    }

    #[test]
    fn keys_and_takes_come_from_the_path() {
        let root = Path::new("/a/audio");
        assert_eq!(key_of(root, Path::new("/a/audio/voice/Goldenmane/cast.2.opus")), Some(("voice/goldenmane/cast".into(), 2)));
        assert_eq!(key_of(root, Path::new("/a/audio/sfx/hit.wav")), Some(("sfx/hit".into(), 0)));
        assert_eq!(key_of(root, Path::new("/a/audio/notes.txt")), None);
    }

    /// The loader decodes a folder on its own thread; the bank takes what is
    /// done without waiting, and the mixer gets every clip.
    #[test]
    fn a_folder_loads_in_the_background() {
        let dir = std::env::temp_dir().join(format!("cb_audio_{}", std::process::id()));
        std::fs::create_dir_all(dir.join("voice/goldenmane")).unwrap();
        let s: Vec<i16> = vec![1000; 4800];
        std::fs::write(dir.join("sfx_hit.wav"), wav16(&s, 1, 48_000)).unwrap();
        std::fs::write(dir.join("voice/goldenmane/cast.1.wav"), wav16(&s, 1, 48_000)).unwrap();
        std::fs::write(dir.join("voice/goldenmane/cast.2.opus"), opus_file(&vec![0.1; 4800], 1)).unwrap();
        std::fs::write(dir.join("broken.opus"), b"nope").unwrap();
        let (mut h, _m) = crate::mixer::new();
        let mut bank = Bank::loading(Loader::start(dir.clone(), 48_000));
        let t0 = std::time::Instant::now();
        while bank.len() < 3 && t0.elapsed().as_secs() < 10 {
            bank.poll(&mut h);
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert_eq!(bank.len(), 3);
        assert_eq!(bank.errors.len(), 1, "the broken file reported: {:?}", bank.errors);
        // takes alternate, never the same twice running
        let mut prev = bank.pick("voice/goldenmane/cast").unwrap();
        for _ in 0..20 {
            let next = bank.pick("voice/goldenmane/cast").unwrap();
            assert_ne!(prev, next, "the same take twice running");
            prev = next;
        }
        assert_eq!(bank.first(&["voice/none".into(), "sfx_hit".into()]), bank.pick("sfx_hit"));
        std::fs::remove_dir_all(&dir).ok();
    }
}
