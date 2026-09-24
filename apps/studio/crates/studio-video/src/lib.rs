//! studio-video — the README's other half of Media I/O: "`ffmpeg` via CLI
//! for video, keeping the core codec-free." `studio-media`'s own module
//! doc named this gap directly and deliberately left it unbuilt, for the
//! same reason it's its own crate here rather than folded into
//! `studio-media`: shelling out to an external binary is a genuinely
//! different technology and failure mode (a missing binary vs. an
//! unsupported codec feature) from Symphonia's pure-Rust decode.
//!
//! ## What this does and doesn't do
//!
//! This crate never decodes a video FRAME into pixels Studio's own model
//! understands — there is no `VideoFrame` type anywhere in this codebase
//! yet, and inventing one is real, separate work this crate doesn't
//! attempt. What an editor's timeline actually needs first, and what this
//! crate provides:
//!
//! 1. [`probe`] — duration, resolution, frame rate, and whether an audio
//!    track exists, via `ffprobe`. This is what fills in a `studio_core::
//!    Source`'s fields correctly for a video file, the same way decoding
//!    a WAV's header already does for audio.
//! 2. [`thumbnail_png`] — a single frame at a given position, encoded as
//!    PNG bytes by `ffmpeg` itself (not decoded further here — a caller
//!    that wants pixels decodes the PNG with whatever image crate its own
//!    UI layer already uses; this crate doesn't take that dependency just
//!    to prove the bytes are real PNG). [`thumbnail_strip`] is several of
//!    these at evenly-spaced positions, for a scrubber.
//! 3. [`extract_audio`] — a video's audio track, piped through `ffmpeg` as
//!    WAV and handed straight to [`studio_media::decode_bytes`] rather
//!    than round-tripping through a temp file. This is the genuinely
//!    reusable piece: once extracted, a video's audio is a `SourceAudio`
//!    exactly like a plain audio file's, a recorded take's, or a bounced
//!    pattern's — `mixdown` doesn't know or care which.
//!
//! ## The film-production half, which shells out to nothing
//!
//! Two things a set needs that have no ffmpeg in them at all, so they are pure and tested:
//!
//! * [`timecode`] — SMPTE, including drop-frame. Studio could print a wall clock
//!   (`H:MM:SS.mmm`); a timecode names a FRAME, which is what a camera stamps and what a
//!   shot list refers to. Drop-frame skips two frame LABELS a minute (no pictures) so that
//!   `01:00:00;00` lands on the real hour at 29.97, which means some timecodes name no
//!   frame at all and a parser that accepts them cuts two frames off.
//! * [`slate`] — dual-system sound. Sound is recorded on its own machine with its own
//!   clock, and the clapperboard exists to put one shared transient in both files.
//!   [`slate::align`] finds it in each and correlates the ENVELOPES around it — different
//!   microphones disagree about waveforms and agree about energy — reporting a confidence,
//!   because a periodic signal correlates with itself at many lags and the peak is then
//!   noise.
//!
//! Every function here shells out and inherits `ffmpeg`/`ffprobe`'s own
//! failure modes: a missing binary is [`VideoError::MissingBinary`], a
//! subprocess that exits non-zero is [`VideoError::CommandFailed`] carrying
//! its stderr, and a video with no audio track is `Ok(None)` from
//! `extract_audio`, not an error — same "this file genuinely doesn't have
//! that" pattern `probe`'s own `has_audio: false` already uses.

/// SMPTE timecode, including drop-frame. Pure — no ffmpeg in it.
pub mod timecode;
pub mod fingerprint;
pub mod provenance;
/// Dual-system sound: aligning a separately-recorded take to picture by the clap. Pure.
pub mod slate;
pub mod slate_ocr;
/// The slate as SPOKEN: scene/shot/take from a transcript's head, into `slate_ocr::Board`.
pub mod spoken_slate;
/// A tiny deterministic shoot (camera takes + recorder files) for tests, here and downstream.
#[cfg(any(test, feature = "testkit"))]
pub mod testkit;
/// Warp stabilization: measure the camera path once, solve it per flavour (still/smooth/dejerk/track).
pub mod stabilize;
/// Touch-up: a ticket for one shot becomes a fixed version of it, with handles and a report.
pub mod touchup;
/// Reverse-engineer an edit: its cuts, short transitions and fades.
pub mod cuts;
/// Operator signal lanes: weighted inputs, hard marks, cut candidates that say why.
pub mod signals;
#[cfg(test)]
mod abundance_tests;
/// Particles: dirt, dust and smoke, deterministic and driven by audio.
pub mod particles;
/// Match-move rig: track points sharing one rigid transform, surviving points that disappear.
pub mod tracker;
/// Clip style transfer: fit one clip's look onto another, baked into a 3D LUT for live use.
pub mod style;
/// Cut on action: the performer's movement accents within a shot, camera motion cancelled.
pub mod action;
/// Comic: an auto-mode layout planner and a page compositor (wide, page, inset, split).
pub mod comic;
/// Kaleidoscope clip effect: one formula for the preview (Rust) and the export (ffmpeg geq).
pub mod kaleido;
/// Temporal clean plate: remove something that passes through a shot, using the frames around it.
pub mod cleanplate;
/// Frames out of ffmpeg, through Rust, back into ffmpeg: the one place per-frame pixel work happens.
pub mod framepipe;
/// Multiview grid geometry: tiles that cover the canvas exactly, and what they cost in detail.
pub mod grid;
/// Multicam: cut between synced angles from a control channel's level and/or beats.
pub mod multicam;
/// Many camera takes against many recorder files: which sound is whose, by listening.
pub mod dualsync;
/// The timeline's video clips cut into one mp4 over the mixdown — ffmpeg's edit list.
pub mod edit;
pub mod vfx_survey;

use std::path::Path;
use std::process::{Command, Stdio};

use studio_audio::SourceAudio;
use studio_core::Ticks;

#[derive(Debug)]
pub enum VideoError {
    /// `ffmpeg` or `ffprobe` isn't on `PATH` — checked once, distinctly
    /// from a general IO error, so a caller can tell "not installed" from
    /// "this specific file/command failed" and message the user
    /// accordingly.
    MissingBinary(&'static str),
    Io(std::io::Error),
    /// The subprocess ran but exited non-zero; carries its stderr, which
    /// for both `ffmpeg` and `ffprobe` is where the actually useful error
    /// message lives (a missing codec, a corrupt file, an unreadable path).
    CommandFailed { program: &'static str, stderr: String },
    /// `ffprobe`'s own JSON came back malformed or missing a field this
    /// crate needs — distinct from `CommandFailed`, since the process
    /// itself succeeded; the output just wasn't shaped as expected.
    UnexpectedOutput(String),
    /// The container has no video stream at all — a caller asked
    /// `probe`/`thumbnail_png` of something that is, per `ffprobe` itself,
    /// not a video.
    NoVideoStream,
    /// Asked for the sound of something that has none.
    NoAudioStream,
    Decode(studio_media::DecodeError),
}

impl std::fmt::Display for VideoError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            VideoError::MissingBinary(bin) => write!(f, "'{bin}' is not on PATH"),
            VideoError::Io(e) => write!(f, "could not run the subprocess: {e}"),
            VideoError::CommandFailed { program, stderr } => write!(f, "{program} failed: {}", stderr.trim()),
            VideoError::UnexpectedOutput(msg) => write!(f, "unexpected ffprobe output: {msg}"),
            VideoError::NoVideoStream => write!(f, "no video stream in this file"),
            VideoError::NoAudioStream => write!(f, "no audio stream in this file"),
            VideoError::Decode(e) => write!(f, "decoding the extracted audio track failed: {e}"),
        }
    }
}
impl std::error::Error for VideoError {}
impl From<std::io::Error> for VideoError {
    fn from(e: std::io::Error) -> Self { VideoError::Io(e) }
}
impl From<studio_media::DecodeError> for VideoError {
    fn from(e: studio_media::DecodeError) -> Self { VideoError::Decode(e) }
}

/// What [`probe`] reports about a video file.
#[derive(Debug, Clone, PartialEq)]
pub struct VideoProbe {
    pub duration: Ticks,
    pub width: u32,
    pub height: u32,
    /// Frames per second — kept as `f64` rather than a `(num, den)` pair
    /// like `ffprobe`'s own `r_frame_rate`, since every consumer of this
    /// (a thumbnail spacing calculation, a UI label) wants a number, not a
    /// fraction to reduce itself; the exact NTSC rationals `studio_core::
    /// Ticks::from_frames` cares about are a `studio-core` concern for
    /// timeline math, not this crate's — `probe` describes the file, it
    /// doesn't place anything on a timeline.
    pub fps: f64,
    pub has_audio: bool,
    pub audio_sample_rate: Option<u32>,
    pub audio_channels: Option<u16>,
}

fn run(program: &'static str, args: &[&str]) -> Result<Vec<u8>, VideoError> {
    let output = Command::new(program)
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound { VideoError::MissingBinary(program) } else { VideoError::Io(e) }
        })?;
    if !output.status.success() {
        return Err(VideoError::CommandFailed { program, stderr: String::from_utf8_lossy(&output.stderr).into_owned() });
    }
    Ok(output.stdout)
}

/// Probe a video file's format and stream metadata via `ffprobe`.
impl VideoProbe {
    /// The named frame rate this file is at, if it is one anybody shoots. `None` for an odd
    /// rate — which is a real answer: a timecode at a rate with no integer nominal count is
    /// not a thing, and guessing the nearest would mislabel every frame.
    pub fn rate(&self) -> Option<timecode::Rate> {
        timecode::Rate::nearest(self.fps)
    }

    /// The timecode of a position in this file, counted from `00:00:00:00`.
    ///
    /// Drop-frame when the rate has a drop standard, because that is what an NTSC camera
    /// stamps and what an NLE shows unless told otherwise.
    pub fn timecode_at(&self, at: Ticks) -> Option<timecode::Timecode> {
        let r = self.rate()?;
        Some(timecode::Timecode::from_ticks(at, r, r.can_drop()))
    }
}

pub fn probe(path: &Path) -> Result<VideoProbe, VideoError> {
    let path_str = path.to_string_lossy();
    let stdout = run("ffprobe", &["-v", "error", "-print_format", "json", "-show_format", "-show_streams", &path_str])?;
    let json: serde_json::Value = serde_json::from_slice(&stdout)
        .map_err(|e| VideoError::UnexpectedOutput(e.to_string()))?;

    let streams = json.get("streams").and_then(|s| s.as_array())
        .ok_or_else(|| VideoError::UnexpectedOutput("no \"streams\" array".into()))?;

    let video = streams.iter().find(|s| s.get("codec_type").and_then(|c| c.as_str()) == Some("video"))
        .ok_or(VideoError::NoVideoStream)?;
    let width = video.get("width").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
    let height = video.get("height").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
    let fps = video.get("r_frame_rate").and_then(|v| v.as_str())
        .and_then(parse_rational)
        .unwrap_or(0.0);

    let audio = streams.iter().find(|s| s.get("codec_type").and_then(|c| c.as_str()) == Some("audio"));
    let audio_sample_rate = audio.and_then(|a| a.get("sample_rate")).and_then(|v| v.as_str()).and_then(|s| s.parse::<u32>().ok());
    let audio_channels = audio.and_then(|a| a.get("channels")).and_then(|v| v.as_u64()).map(|c| c as u16);

    let duration_s: f64 = json.get("format").and_then(|f| f.get("duration")).and_then(|v| v.as_str())
        .and_then(|s| s.parse::<f64>().ok())
        .unwrap_or(0.0);

    Ok(VideoProbe {
        duration: Ticks::from_seconds(duration_s),
        width, height, fps,
        has_audio: audio.is_some(),
        audio_sample_rate,
        audio_channels,
    })
}

/// Parse an `ffprobe`-style rational like `"30000/1001"` or `"25/1"` into a
/// plain `f64`. `"0/0"` (what `ffprobe` reports for a stream with no frame
/// rate — e.g. an audio-only stream, never a real video stream this
/// function is actually called on, but worth not dividing by zero over)
/// returns `None` rather than `NaN`.
fn parse_rational(s: &str) -> Option<f64> {
    let (num, den) = s.split_once('/')?;
    let (num, den): (f64, f64) = (num.parse().ok()?, den.parse().ok()?);
    if den == 0.0 { None } else { Some(num / den) }
}

/// Extract a single frame at `at` as PNG bytes, via `ffmpeg`. Returns the
/// raw encoded bytes — decoding them into pixels is a caller concern (a UI
/// layer already has an image-decoding dependency; this crate doesn't take
/// one just to prove its own output is well-formed, though the tests below
/// do check the PNG magic bytes directly).
/// The audio between `from` and `to` of a video, as WAV bytes any decoder
/// in this repo reads. The headless half of "sample a video": a shot's
/// dialogue, a room tone, the thump of something falling — material that
/// lives in the footage library and belongs in the rack.
///
/// Trimming happens in ffmpeg (`-ss` before `-i` to seek, `-t` for the
/// span) rather than after decoding, so pulling two seconds out of a
/// forty-minute rush costs two seconds of work. A video with no audio
/// stream is [`VideoError::NoAudioStream`] — a named fact, not an empty
/// buffer that would look like silence.
pub fn audio_segment(path: &Path, from: Ticks, to: Ticks) -> Result<Vec<u8>, VideoError> {
    let path_str = path.to_string_lossy();
    let start = format!("{:.6}", from.to_seconds().max(0.0));
    let span = (to.to_seconds() - from.to_seconds()).max(0.0);
    if span <= 0.0 {
        return Err(VideoError::UnexpectedOutput(
            "audio_segment: the out point is not after the in point".into()));
    }
    let dur = format!("{span:.6}");
    let out = match run("ffmpeg", &[
        "-y", "-ss", &start, "-i", &path_str, "-t", &dur,
        "-vn", "-map", "a:0?", "-f", "wav", "-acodec", "pcm_s16le", "-",
    ]) {
        Ok(bytes) => bytes,
        // With no audio stream ffmpeg may either write a bare header or
        // fail outright depending on build and container; both mean the
        // same thing to a caller, and "which stream is missing" is more
        // useful than the raw stderr.
        Err(VideoError::CommandFailed { .. }) => return Err(VideoError::NoAudioStream),
        Err(e) => return Err(e),
    };
    // A bare 44-byte header is the other shape of the same fact.
    if out.len() <= 64 { return Err(VideoError::NoAudioStream); }
    Ok(out)
}

/// One frame at `at` as raw RGB24 at EXACTLY `w`×`h` — aspect preserved by
/// scale-then-pad, so the caller can hand the bytes straight to a texture
/// with no image decoder in the loop. Built for the timeline's viewport:
/// a preview must be a fixed-size buffer, not a PNG to parse per frame.
pub fn frame_rgb(path: &Path, at: Ticks, w: u32, h: u32) -> Result<Vec<u8>, VideoError> {
    let path_str = path.to_string_lossy();
    let seek = format!("{:.6}", at.to_seconds());
    let vf = format!(
        "scale={w}:{h}:force_original_aspect_ratio=decrease,pad={w}:{h}:(ow-iw)/2:(oh-ih)/2");
    let out = run("ffmpeg", &[
        "-y", "-ss", &seek, "-i", &path_str,
        "-frames:v", "1", "-vf", &vf,
        "-f", "rawvideo", "-pix_fmt", "rgb24", "-",
    ])?;
    let want = (w * h * 3) as usize;
    if out.len() != want {
        return Err(VideoError::UnexpectedOutput(format!(
            "frame_rgb: expected {want} bytes for {w}x{h}, got {}", out.len())));
    }
    Ok(out)
}

pub fn thumbnail_png(path: &Path, at: Ticks) -> Result<Vec<u8>, VideoError> {
    let path_str = path.to_string_lossy();
    let seek = format!("{:.6}", at.to_seconds());
    run("ffmpeg", &[
        "-y", "-ss", &seek, "-i", &path_str,
        "-frames:v", "1", "-f", "image2", "-c:v", "png", "-",
    ])
}

/// A strip of thumbnails at `count` evenly-spaced positions across the
/// video's own duration, for a timeline scrubber — the natural next thing
/// on top of [`thumbnail_png`] once a UI needs more than one frame.
/// Returns `(position, PNG bytes)` pairs in time order.
///
/// Each slot is centred rather than left-aligned (`(i + 0.5) / count` of
/// the duration, not `i / count`): the very first frame of a video is
/// often black or mid-fade-in, and always landing the first thumbnail
/// exactly there would make every scrubber built on this look broken in
/// the same way on the same kind of source.
///
/// Honest about the cost: this is `count` separate `ffmpeg` invocations
/// (`thumbnail_png` called in a loop), not the single-process batch
/// extraction (one `-vf fps=...` decode pass producing every frame from
/// one process) a genuinely optimized version would do. Simpler, correct,
/// and honestly slower for a large `count` — the batch version is real,
/// separate work this function doesn't attempt, the same "prove the shape
/// first" trade this session's other new code already made more than once.
pub fn thumbnail_strip(path: &Path, count: usize) -> Result<Vec<(Ticks, Vec<u8>)>, VideoError> {
    if count == 0 {
        return Ok(Vec::new());
    }
    let info = probe(path)?;
    let total_s = info.duration.to_seconds().max(0.0);
    let mut out = Vec::with_capacity(count);
    for i in 0..count {
        let frac = (i as f64 + 0.5) / count as f64;
        let at = Ticks::from_seconds(total_s * frac);
        let png = thumbnail_png(path, at)?;
        out.push((at, png));
    }
    Ok(out)
}

/// Extract a video's audio track as a [`SourceAudio`], via `ffmpeg` piping
/// WAV bytes straight to [`studio_media::decode_bytes`] — no temp file.
/// `Ok(None)` when the file genuinely has no audio track (checked via
/// `probe` first), not an error: a silent video is a normal video, not a
/// failure.
/// A lossy audio deliverable through ffmpeg: `wav` transcoded to `out`
/// with `codec` (`libmp3lame` for an mp3, `aac` for an m4a) at
/// `bitrate` bits per second. The phone's share sheet and the group
/// chat want an mp3 or an m4a, and this codebase encodes neither
/// itself — Opus is the in-house format; these are the ones everyone
/// else's player opens. A build of ffmpeg without the encoder fails
/// with its own stderr, which names the missing encoder.
pub fn transcode_audio(wav: &Path, out: &Path, codec: &str, bitrate: u32) -> Result<(), VideoError> {
    let (w, o) = (wav.to_string_lossy(), out.to_string_lossy());
    let br = format!("{bitrate}");
    run("ffmpeg", &["-y", "-hide_banner", "-loglevel", "error", "-i", &w, "-vn",
                    "-c:a", codec, "-b:a", &br, &o]).map(|_| ())
}

pub fn extract_audio(path: &Path) -> Result<Option<SourceAudio>, VideoError> {
    let info = probe(path)?;
    if !info.has_audio {
        return Ok(None);
    }
    let path_str = path.to_string_lossy();
    let wav_bytes = run("ffmpeg", &[
        "-y", "-i", &path_str,
        "-vn", "-f", "wav", "-acodec", "pcm_f32le", "-",
    ])?;
    Ok(Some(studio_media::decode_bytes(wav_bytes, Some("wav"))?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::OnceLock;

    /// Real ffmpeg/ffprobe, invoked for real — this crate's whole reason to
    /// exist is shelling out to them, so a test that fakes the subprocess
    /// would be testing a different, easier crate than the one that ships.
    /// Skips (not fails) if either binary genuinely isn't present, the same
    /// "hardware stage, not the authoring sandbox" bargain `studio-io`'s
    /// `device` feature already makes for real audio hardware — except
    /// here the tests DO run in this sandbox, because both binaries are
    /// actually installed.
    /// Can this machine build the test fixture?
    ///
    /// It is not enough that `ffmpeg` and `ffprobe` EXIST. `make_test_video`
    /// encodes with libx264 and aac, and many distribution ffmpeg builds ship
    /// without them (Debian/Ubuntu's `ffmpeg` from main historically has, and
    /// the minimal container images usually do). On such a box the binaries
    /// answer `-version` happily, this guard passed, and then the fixture's
    /// `.expect("generating the test fixture itself must succeed")` panicked —
    /// which is how the fleet reported three studio-video failures that look
    /// like probe/thumbnail bugs and are neither. All 13 pass here against a
    /// full build.
    ///
    /// This is the "a guard that watches one failure mode on a device that
    /// produces another" shape that this codebase has already been bitten by
    /// and calls out by name elsewhere. The guard now checks the capability
    /// the tests actually need, not the binary's presence.
    pub(crate) fn has_ffmpeg() -> bool {
        static CHECKED: OnceLock<bool> = OnceLock::new();
        *CHECKED.get_or_init(|| {
            let ok = |c: &str| Command::new(c).arg("-version").output()
                .map(|o| o.status.success()).unwrap_or(false);
            if !ok("ffmpeg") || !ok("ffprobe") { return false; }
            // `-encoders` rather than `-codecs`: a build can know OF a codec it
            // cannot encode with, and encoding is what the fixture does.
            let enc = match Command::new("ffmpeg").arg("-hide_banner").arg("-encoders").output() {
                Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).into_owned(),
                _ => return false,
            };
            let have = |n: &str| enc.lines().any(|l| l.split_whitespace().nth(1) == Some(n));
            if !have("libx264") {
                eprintln!("studio-video tests: ffmpeg is present but cannot encode libx264 — \
skipping the fixture-backed tests. Install a full ffmpeg build to get real coverage here.");
                return false;
            }
            if !have("aac") {
                eprintln!("studio-video tests: ffmpeg cannot encode aac — skipping the \
fixture-backed tests (the audio fixture needs it).");
                return false;
            }
            true
        })
    }

    /// Build a real short MP4 (10fps, 64x48 video + 440Hz mono tone,
    /// 1 second) via ffmpeg's own `lavfi` test sources — no bundled fixture
    /// file needed, and every test gets an independently-generated file in
    /// a fresh temp path so tests can run concurrently without clashing.
    #[test]
    fn a_video_can_be_sampled_for_its_sound_and_trimmed_while_it_is_read() {
        // Guarded like every other fixture-backed test here. These two were not,
        // so on a box whose ffmpeg cannot encode libx264 they panicked inside
        // make_test_video while the rest skipped cleanly — which is why the fleet
        // saw a handful of studio-video failures rather than a whole-module skip.
        if !has_ffmpeg() { eprintln!("skipping: ffmpeg cannot build the test fixture"); return; }
        // The sampler's headless half: half a second out of the middle of
        // a one-second clip, decoded back to prove it is real audio and
        // really that long.
        let path = make_test_video(true);
        let wav = audio_segment(&path, Ticks::from_seconds(0.25),
                                Ticks::from_seconds(0.75)).expect("a segment");
        let back = studio_media::decode_bytes(wav, Some("wav")).expect("decodes");
        let secs = back.frames() as f64 / back.sample_rate as f64;
        assert!((secs - 0.5).abs() < 0.05, "trimmed to the ask, got {secs}s");
        let energy: f32 = back.channels[0].iter().map(|s| s.abs()).sum();
        assert!(energy > 1.0, "a 440 Hz tone is in there ({energy})");
        // Backwards or empty spans are refused rather than returning
        // nothing that looks like silence.
        assert!(audio_segment(&path, Ticks::from_seconds(0.5),
                              Ticks::from_seconds(0.5)).is_err());
        // A video with no audio says which thing is missing.
        let silent = make_test_video(false);
        assert!(matches!(audio_segment(&silent, Ticks::ZERO, Ticks::from_seconds(0.5)),
                         Err(VideoError::NoAudioStream)));
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(&silent);
    }

    #[test]
    fn a_frame_arrives_as_exactly_the_rgb_buffer_a_texture_wants() {
        // Guarded like every other fixture-backed test here. These two were not,
        // so on a box whose ffmpeg cannot encode libx264 they panicked inside
        // make_test_video while the rest skipped cleanly — which is why the fleet
        // saw a handful of studio-video failures rather than a whole-module skip.
        if !has_ffmpeg() { eprintln!("skipping: ffmpeg cannot build the test fixture"); return; }
        let path = make_test_video(false);
        let px = frame_rgb(&path, Ticks::from_seconds(0.5), 160, 90)
            .expect("a frame at half a second");
        assert_eq!(px.len(), 160 * 90 * 3, "fixed size is the whole contract");
        // testsrc is colour bars: a real decoded frame is not all one value.
        let (min, max) = px.iter().fold((255u8, 0u8), |(lo, hi), b| (lo.min(*b), hi.max(*b)));
        assert!(max > min, "the frame has content, not a flat fill");
        let _ = std::fs::remove_file(&path);
    }

    fn make_test_video(with_audio: bool) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!("studio-video-test-{}-{}.mp4", std::process::id(), rand_suffix()));
        let mut args = vec![
            "-y".to_string(),
            "-f".into(), "lavfi".into(), "-i".into(), "testsrc=duration=1:size=64x48:rate=10".into(),
        ];
        if with_audio {
            args.extend(["-f".into(), "lavfi".into(), "-i".into(), "sine=frequency=440:duration=1".into(), "-shortest".into()]);
        }
        args.extend(["-c:v".into(), "libx264".into(), "-pix_fmt".into(), "yuv420p".into()]);
        if with_audio {
            args.extend(["-c:a".into(), "aac".into()]);
        } else {
            args.push("-an".into());
        }
        args.push(path.to_string_lossy().into_owned());
        let arg_refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
        run("ffmpeg", &arg_refs).expect("generating the test fixture itself must succeed");
        path
    }

    fn rand_suffix() -> u64 {
        use std::time::{SystemTime, UNIX_EPOCH};
        SystemTime::now().duration_since(UNIX_EPOCH).unwrap().subsec_nanos() as u64
    }

    #[test]
    fn parse_rational_handles_ntsc_and_whole_number_rates() {
        assert!((parse_rational("30000/1001").unwrap() - 29.97).abs() < 0.01);
        // and that rational is what a probe turns into a NAMED rate, so a timecode can be
        // drawn: 29.97 is drop-frame capable and 23.976 is not, and the difference decides
        // whether `01:00:00;00` is even a legal label.
        let p = VideoProbe { duration: Ticks::ZERO, width: 1920, height: 1080,
                             fps: parse_rational("30000/1001").unwrap(), has_audio: true,
                             audio_channels: Some(2), audio_sample_rate: Some(48_000) };
        assert_eq!(p.rate(), Some(timecode::Rate::Fps29_97));
        assert_eq!(p.timecode_at(Ticks::from_seconds(3600.0)).unwrap().format(), "01:00:00;00");
        let film = VideoProbe { fps: 24.0, ..p };
        assert_eq!(film.timecode_at(Ticks::from_seconds(3600.0)).unwrap().format(), "01:00:00:00");
        // an odd rate is not rounded onto a real one — every frame would then be mislabelled
        let odd = VideoProbe { fps: 48.0, ..p };
        assert_eq!(odd.rate(), None);
        assert!(odd.timecode_at(Ticks::ZERO).is_none());
        assert_eq!(parse_rational("25/1"), Some(25.0));
        assert_eq!(parse_rational("0/0"), None, "must not divide by zero into NaN");
        assert_eq!(parse_rational("not-a-fraction"), None);
    }

    #[test]
    fn probe_reports_real_dimensions_duration_and_audio_presence() {
        if !has_ffmpeg() { eprintln!("skipping: ffmpeg/ffprobe not available"); return; }
        let path = make_test_video(true);
        let info = probe(&path).unwrap();
        let _ = std::fs::remove_file(&path);

        assert_eq!(info.width, 64);
        assert_eq!(info.height, 48);
        assert!((info.fps - 10.0).abs() < 0.01, "fps was {}", info.fps);
        assert!((info.duration.to_seconds() - 1.0).abs() < 0.05, "duration was {:?}", info.duration);
        assert!(info.has_audio);
        assert_eq!(info.audio_sample_rate, Some(44_100));
        assert_eq!(info.audio_channels, Some(1));
    }

    #[test]
    fn probe_reports_no_audio_for_a_silent_video() {
        if !has_ffmpeg() { eprintln!("skipping: ffmpeg/ffprobe not available"); return; }
        let path = make_test_video(false);
        let info = probe(&path).unwrap();
        let _ = std::fs::remove_file(&path);
        assert!(!info.has_audio);
        assert_eq!(info.audio_sample_rate, None);
    }

    #[test]
    fn probe_of_a_missing_file_is_a_clean_error() {
        if !has_ffmpeg() { eprintln!("skipping: ffmpeg/ffprobe not available"); return; }
        let err = probe(Path::new("/nonexistent/studio-video-test-does-not-exist.mp4")).unwrap_err();
        assert!(matches!(err, VideoError::CommandFailed { .. }), "ffprobe itself should report the missing file, not this crate guessing");
    }

    #[test]
    fn thumbnail_png_returns_real_png_bytes() {
        if !has_ffmpeg() { eprintln!("skipping: ffmpeg/ffprobe not available"); return; }
        let path = make_test_video(true);
        let png = thumbnail_png(&path, Ticks::from_seconds(0.5)).unwrap();
        let _ = std::fs::remove_file(&path);

        assert!(png.len() > 100, "a real 64x48 PNG should be more than a header's worth of bytes");
        assert_eq!(&png[..8], &[0x89, b'P', b'N', b'G', b'\r', b'\n', 0x1a, b'\n'], "must start with the real PNG magic bytes");
    }

    #[test]
    fn thumbnail_strip_returns_evenly_spaced_valid_thumbnails_in_time_order() {
        if !has_ffmpeg() { eprintln!("skipping: ffmpeg/ffprobe not available"); return; }
        let path = make_test_video(true);
        let strip = thumbnail_strip(&path, 4).unwrap();
        let _ = std::fs::remove_file(&path);

        assert_eq!(strip.len(), 4);
        for (_, png) in &strip {
            assert_eq!(&png[..8], &[0x89, b'P', b'N', b'G', b'\r', b'\n', 0x1a, b'\n'], "every entry must be a real PNG");
        }
        let positions: Vec<f64> = strip.iter().map(|(t, _)| t.to_seconds()).collect();
        for w in positions.windows(2) {
            assert!(w[1] > w[0], "positions must be strictly increasing: {positions:?}");
        }
        // Centred slots of a 1-second video split into 4: (0.5)/4, (1.5)/4,
        // (2.5)/4, (3.5)/4 = 0.125, 0.375, 0.625, 0.875.
        assert!((positions[0] - 0.125).abs() < 0.05, "first slot should be centred near 0.125s, got {}", positions[0]);
        assert!((positions[3] - 0.875).abs() < 0.05, "last slot should be centred near 0.875s, got {}", positions[3]);
    }

    #[test]
    fn thumbnail_strip_of_zero_is_an_empty_no_op_not_an_error() {
        // No ffmpeg call at all needed to answer "give me zero thumbnails" —
        // worth pinning as a real fast path, not just an edge case that
        // happens to work.
        let strip = thumbnail_strip(Path::new("/nonexistent/does-not-matter.mp4"), 0).unwrap();
        assert!(strip.is_empty());
    }

    #[test]
    fn thumbnail_strip_of_one_lands_near_the_midpoint() {
        if !has_ffmpeg() { eprintln!("skipping: ffmpeg/ffprobe not available"); return; }
        let path = make_test_video(true);
        let strip = thumbnail_strip(&path, 1).unwrap();
        let _ = std::fs::remove_file(&path);
        assert_eq!(strip.len(), 1);
        assert!((strip[0].0.to_seconds() - 0.5).abs() < 0.1, "a single centred slot of a 1s video should land near 0.5s, got {}", strip[0].0.to_seconds());
    }

    #[test]
    fn extract_audio_returns_a_real_decodable_source_audio() {
        if !has_ffmpeg() { eprintln!("skipping: ffmpeg/ffprobe not available"); return; }
        let path = make_test_video(true);
        let audio = extract_audio(&path).unwrap().expect("this fixture has an audio track");
        let _ = std::fs::remove_file(&path);

        assert_eq!(audio.sample_rate, 44_100);
        assert!(audio.frames() > 40_000, "a 1-second 44.1kHz track should decode to close to 44,100 frames, got {}", audio.frames());
        assert!(audio.channels[0].iter().any(|&s| s != 0.0), "the 440Hz test tone must actually be audible in the extracted track");
    }

    #[test]
    fn extract_audio_returns_none_for_a_silent_video_not_an_error() {
        if !has_ffmpeg() { eprintln!("skipping: ffmpeg/ffprobe not available"); return; }
        let path = make_test_video(false);
        let audio = extract_audio(&path).unwrap();
        let _ = std::fs::remove_file(&path);
        assert!(audio.is_none(), "a video with no audio track is a normal video, not a failure");
    }

    #[test]
    fn video_error_display_is_human_readable() {
        assert_eq!(format!("{}", VideoError::MissingBinary("ffmpeg")), "'ffmpeg' is not on PATH");
        assert_eq!(format!("{}", VideoError::NoVideoStream), "no video stream in this file");
        let msg = format!("{}", VideoError::CommandFailed { program: "ffprobe", stderr: "  no such file  \n".into() });
        assert_eq!(msg, "ffprobe failed: no such file", "stderr must be trimmed, not echoed with its own whitespace");
    }
}
