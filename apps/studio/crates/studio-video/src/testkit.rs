//! testkit — a tiny, deterministic film shoot, built on demand.
//!
//! The film pipeline's hard problems (pairing sound to picture, reading
//! slates, rendering an edit) only show up on media shaped like a real
//! shoot: several camera takes, a recorder that rolled first, a decoy file
//! of room tone, different gains and noise floors per machine. This builds
//! exactly that at toy size — seconds long, 64×36 pixels, 16 kHz — so a test
//! can run the real ffmpeg path end to end in well under a second per take.
//!
//! Nothing binary is committed. Every file is generated from a seed, so the
//! fixture is reviewable as code and identical on every machine; generated
//! files are cached in the system temp dir under a key of their parameters,
//! so a warm test run spawns no ffmpeg at all for fixtures.
//!
//! ## A missing ffmpeg must not look like a pass
//!
//! A fixture-gated test that `return`s when its tool is absent PASSES, and a
//! suite of them reports green while testing nothing (CLAUDE.md §5b records
//! 28 such sites hiding a whole loader). [`ffmpeg_or_skip`] skips only when
//! allowed: with `STUDIO_REQUIRE_FFMPEG=1` in the environment, a missing
//! ffmpeg is a test FAILURE. CI and anyone checking coverage set it.
//!
//! Compiled for this crate's tests, and for other crates with the `testkit`
//! feature (`studio-video = { path = "...", features = ["testkit"] }` as a
//! dev-dependency), so studio-app's integration tests use the same shoot.

use std::path::{Path, PathBuf};
use std::process::Command;

use studio_audio::SourceAudio;

/// The sample rate every fixture is built at.
pub const SR: u32 = 16_000;

/// True when ffmpeg can build fixtures. False means "skip" — but only when
/// skipping is allowed; with `STUDIO_REQUIRE_FFMPEG=1` this panics instead.
pub fn ffmpeg_or_skip(test: &str) -> bool {
    let ok = Command::new("ffmpeg").arg("-hide_banner").arg("-version").output()
        .map(|o| o.status.success()).unwrap_or(false);
    if !ok {
        if std::env::var("STUDIO_REQUIRE_FFMPEG").map(|v| v == "1").unwrap_or(false) {
            panic!("{test}: ffmpeg is required (STUDIO_REQUIRE_FFMPEG=1) and is not runnable");
        }
        eprintln!("SKIPPED {test}: no ffmpeg (set STUDIO_REQUIRE_FFMPEG=1 to make this a failure)");
    }
    ok
}

/// Deterministic noise source: splitmix-seeded xorshift. (Raw xorshift from
/// seeds 1, 2, 3 gives bit-shifted copies of one stream, which a correlator
/// hears as "the same room" — see `dualsync`'s tests.)
pub struct Rng(u64);
impl Rng {
    pub fn new(seed: u64) -> Rng {
        let mut z = seed.wrapping_add(0x9E37_79B9_7F4A_7C15);
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        Rng((z ^ (z >> 31)) | 1)
    }
    /// Uniform in [0, 1).
    pub fn next(&mut self) -> f64 {
        self.0 ^= self.0 << 13; self.0 ^= self.0 >> 7; self.0 ^= self.0 << 17;
        (self.0 >> 11) as f64 / (1u64 << 53) as f64
    }
}

/// A scene as it happened, in scene time: a clap, then syllables.
#[derive(Debug, Clone, PartialEq)]
pub struct Scene {
    /// (start seconds, duration seconds, fundamental Hz) per syllable.
    pub syllables: Vec<(f64, f64, f64)>,
    /// When the sticks closed.
    pub clap_at: f64,
    pub secs: f64,
}

impl Scene {
    /// A clap at 1 s, then speech-like voiced bursts to the end.
    pub fn new(seed: u64, secs: f64) -> Scene {
        let mut r = Rng::new(seed);
        let mut t = 1.6;
        let mut syllables = Vec::new();
        while t < secs - 0.3 {
            let d = 0.08 + 0.22 * r.next();
            syllables.push((t, d, 100.0 + 150.0 * r.next()));
            t += d + 0.05 + 0.5 * r.next();
        }
        Scene { syllables, clap_at: 1.0, secs }
    }

    /// What one machine heard: it started rolling at scene time `start`, ran
    /// `len` seconds, at `gain`, over its own noise floor.
    pub fn record(&self, start: f64, len: f64, gain: f32, noise: f32, seed: u64) -> SourceAudio {
        let n = (len * SR as f64) as usize;
        let mut r = Rng::new(seed);
        let mut s: Vec<f32> = (0..n).map(|_| noise * (r.next() as f32 * 2.0 - 1.0)).collect();
        let at = |t: f64| ((t - start) * SR as f64) as isize;
        // clap: 5 ms of full-band noise with a fast decay
        let c0 = at(self.clap_at);
        for k in 0..(0.03 * SR as f64) as isize {
            let i = c0 + k;
            if i >= 0 && (i as usize) < n {
                let env = (-(k as f64) / (0.004 * SR as f64)).exp();
                s[i as usize] += gain * 0.9 * (env * (r.next() * 2.0 - 1.0)) as f32;
            }
        }
        for &(t0, d, f) in &self.syllables {
            let (a, b) = (at(t0), at(t0 + d));
            for i in a.max(0)..b.min(n as isize) {
                let tt = (i - a) as f64 / SR as f64;
                let env = (std::f64::consts::PI * tt / d).sin();
                let mut v = 0.0;
                let mut h = 1.0;
                while h * f < 6000.0 {
                    v += (std::f64::consts::TAU * h * f * tt).sin() / h;
                    h += 1.0;
                }
                s[i as usize] += gain * (0.3 * env * v) as f32;
            }
        }
        SourceAudio { sample_rate: SR, channels: vec![s] }
    }
}

/// 16-bit PCM WAV bytes. Written by hand so the fixture needs no encoder
/// crate and the bytes are exactly what the samples say.
pub fn wav_bytes(a: &SourceAudio) -> Vec<u8> {
    let ch = a.channels.len() as u16;
    let frames = a.frames();
    let data_len = (frames * ch as usize * 2) as u32;
    let mut b = Vec::with_capacity(44 + data_len as usize);
    b.extend_from_slice(b"RIFF");
    b.extend_from_slice(&(36 + data_len).to_le_bytes());
    b.extend_from_slice(b"WAVEfmt ");
    b.extend_from_slice(&16u32.to_le_bytes());
    b.extend_from_slice(&1u16.to_le_bytes());
    b.extend_from_slice(&ch.to_le_bytes());
    b.extend_from_slice(&a.sample_rate.to_le_bytes());
    b.extend_from_slice(&(a.sample_rate * ch as u32 * 2).to_le_bytes());
    b.extend_from_slice(&(ch * 2).to_le_bytes());
    b.extend_from_slice(&16u16.to_le_bytes());
    b.extend_from_slice(b"data");
    b.extend_from_slice(&data_len.to_le_bytes());
    for f in 0..frames {
        for c in &a.channels {
            let v = (c[f].clamp(-1.0, 1.0) * 32767.0).round() as i16;
            b.extend_from_slice(&v.to_le_bytes());
        }
    }
    b
}

/// One camera take of the shoot: file, and the truth about it.
#[derive(Debug, Clone)]
pub struct Take {
    pub video: PathBuf,
    /// Which recorder file (index into [`Shoot::recorder`]) is its sound.
    pub truth_sound: usize,
    /// Scene time at which the camera rolled, minus the recorder's roll time:
    /// the dualsync offset is `-camera_late_by`.
    pub camera_late_by: f64,
    /// A flat colour per take, so a rendered edit can be checked by pixel.
    pub rgb: [u8; 3],
}

/// A card of camera takes and a folder of recorder files.
#[derive(Debug, Clone)]
pub struct Shoot {
    pub dir: PathBuf,
    pub takes: Vec<Take>,
    pub recorder: Vec<PathBuf>,
}

/// Distinct, easily-classified take colours.
pub const COLOURS: [[u8; 3]; 6] = [[200, 40, 40], [40, 180, 60], [50, 70, 210],
                                  [220, 200, 40], [180, 50, 190], [40, 190, 200]];

/// Build (or reuse) a shoot of `offsets.len()` takes, each `take_secs` long,
/// with the camera rolling `offsets[i]` seconds after the recorder, plus one
/// recorder file of room tone that belongs to no take. Recorder files are
/// listed in REVERSE take order so no test can pass by index coincidence.
pub fn shoot(offsets: &[f64], take_secs: f64) -> Shoot {
    let key = format!("studio-testkit-v1-{}-{}", take_secs,
        offsets.iter().map(|o| format!("{o:.3}")).collect::<Vec<_>>().join("_"));
    let dir = std::env::temp_dir().join(key);
    std::fs::create_dir_all(&dir).expect("temp dir");
    let n = offsets.len();
    let mut recorder = vec![PathBuf::new(); n + 1];
    let mut takes = Vec::new();
    for (k, &off) in offsets.iter().enumerate() {
        let scene = Scene::new(1000 + k as u64, off + take_secs + 1.0);
        let rec_path = dir.join(format!("STE-{:03}.wav", 100 + (n - k)));
        write_once(&rec_path, || wav_bytes(&scene.record(0.0, scene.secs, 0.5, 0.004, 2000 + k as u64)));
        let slot = n - k;  // reverse order in the recorder list
        recorder[slot] = rec_path;
        let cam_wav = dir.join(format!("cam-{k}.wav"));
        write_once(&cam_wav, || wav_bytes(&scene.record(off, take_secs, 0.25, 0.01, 3000 + k as u64)));
        let video = dir.join(format!("C{:04}.mkv", k));
        let rgb = COLOURS[k % COLOURS.len()];
        if !video.exists() {
            let colour = format!("color=c=0x{:02x}{:02x}{:02x}:s=64x36:r=12:d={take_secs}", rgb[0], rgb[1], rgb[2]);
            let out = Command::new("ffmpeg").args(["-y", "-hide_banner", "-loglevel", "error",
                    "-f", "lavfi", "-i", &colour, "-i"]).arg(&cam_wav)
                .args(["-c:v", "libx264", "-preset", "ultrafast", "-qp", "0", "-pix_fmt", "yuv444p",
                       "-c:a", "pcm_s16le", "-shortest"]).arg(&video).output()
                .expect("ffmpeg runs (call ffmpeg_or_skip first)");
            assert!(out.status.success(), "fixture encode failed: {}", String::from_utf8_lossy(&out.stderr));
        }
        takes.push(Take { video, truth_sound: slot, camera_late_by: off, rgb });
    }
    // slot 0: room tone that belongs to nobody
    let tone = dir.join("STE-099.wav");
    write_once(&tone, || {
        let mut r = Rng::new(9);
        let s: Vec<f32> = (0..(take_secs * 1.5 * SR as f64) as usize)
            .map(|_| 0.01 * (r.next() as f32 * 2.0 - 1.0)).collect();
        wav_bytes(&SourceAudio { sample_rate: SR, channels: vec![s] })
    });
    recorder[0] = tone;
    Shoot { dir, takes, recorder }
}

fn write_once(p: &Path, make: impl FnOnce() -> Vec<u8>) {
    if !p.exists() {
        let tmp = p.with_extension("partial");
        std::fs::write(&tmp, make()).expect("write fixture");
        std::fs::rename(&tmp, p).expect("publish fixture");  // no half-written fixture is ever read
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dualsync::{match_pool, Onsets, Verdict};

    #[test]
    fn wav_bytes_roundtrip_through_the_real_decoder() {
        let a = Scene::new(1, 3.0).record(0.0, 3.0, 0.5, 0.01, 2);
        let back = studio_media::decode_bytes(wav_bytes(&a), Some("wav")).expect("decodes");
        assert_eq!((back.sample_rate, back.frames()), (SR, a.frames()));
        let worst = a.channels[0].iter().zip(&back.channels[0])
            .map(|(x, y)| (x - y).abs()).fold(0.0f32, f32::max);
        assert!(worst <= 1.0 / 32767.0 + 1e-6, "worst sample error {worst}");
    }

    /// End to end through ffmpeg: camera files are encoded, their audio is
    /// pulled back out with `extract_audio`, and the pool pairs every take
    /// with its own recorder file at the right offset — and the room tone
    /// with nothing.
    #[test]
    fn a_shoot_pairs_itself_through_the_real_ffmpeg_path() {
        if !ffmpeg_or_skip("a_shoot_pairs_itself_through_the_real_ffmpeg_path") { return; }
        // 30 s takes: z grows with take length (see dualsync::SURE_Z), and
        // at 12 s the same pairs came out right but Unsure (z ~11).
        let offsets = [1.5, 3.0, 4.25];
        let s = shoot(&offsets, 30.0);
        let pics: Vec<Onsets> = s.takes.iter().map(|t| {
            let a = crate::extract_audio(&t.video).expect("extracts").expect("has audio");
            Onsets::of(&a).expect("onsets")
        }).collect();
        let snds: Vec<Onsets> = s.recorder.iter()
            .map(|p| Onsets::of(&studio_media::decode_file(p).expect("decodes")).expect("onsets"))
            .collect();
        let got = match_pool(&pics, &snds);
        assert_eq!(got.len(), offsets.len());
        for (t, m) in s.takes.iter().zip(got) {
            let m = m.expect("a match");
            assert_eq!(m.sound, t.truth_sound, "{m:?}");
            assert_ne!(m.sound, 0, "room tone must never be a take's sound");
            assert!((m.offset_seconds() + t.camera_late_by).abs() <= 0.02,
                    "offset {} for camera late by {}", m.offset_seconds(), t.camera_late_by);
            assert_eq!(m.verdict, Verdict::Sure, "{m:?}");
        }
    }

    /// The length dependence, pinned: a short take pairs correctly but is
    /// NOT called sure. If a threshold change ever makes 12 s takes Sure,
    /// that is a decision to make on real footage, not by this test drifting.
    #[test]
    fn a_short_take_pairs_right_but_is_not_sure() {
        if !ffmpeg_or_skip("a_short_take_pairs_right_but_is_not_sure") { return; }
        let s = shoot(&[1.5], 12.0);
        let a = crate::extract_audio(&s.takes[0].video).unwrap().unwrap();
        let snds: Vec<Onsets> = s.recorder.iter()
            .map(|p| Onsets::of(&studio_media::decode_file(p).unwrap()).unwrap()).collect();
        let m = match_pool(&[Onsets::of(&a).unwrap()], &snds).remove(0).unwrap();
        assert_eq!(m.sound, s.takes[0].truth_sound);
        assert!((m.offset_seconds() + 1.5).abs() <= 0.02, "{}", m.offset_seconds());
        assert_eq!(m.verdict, Verdict::Unsure, "{m:?}");
    }

    #[test]
    fn the_takes_are_the_colours_they_claim() {
        if !ffmpeg_or_skip("the_takes_are_the_colours_they_claim") { return; }
        let s = shoot(&[1.0, 2.0], 4.0);
        for t in &s.takes {
            // frame_rgb letterboxes to the requested box, so a 16:9 take in
            // an 8x8 box has black bars top and bottom: sample the centre.
            let px = crate::frame_rgb(&t.video, studio_core::Ticks::from_seconds(2.0), 8, 8).unwrap();
            let i = (4 * 8 + 4) * 3;
            for c in 0..3 {
                assert!((px[i + c] as i32 - t.rgb[c] as i32).abs() <= 6, "{:?} vs {:?}", &px[i..i + 3], t.rgb);
            }
        }
    }
}
