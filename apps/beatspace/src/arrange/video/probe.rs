//! Real file metadata, via `ffprobe` — the same "shell out, keep the core
//! codec-free" bargain `apps/studio/crates/studio-video` already makes,
//! independently arrived at here because AV1 decode is exactly the kind
//! of "genuinely different technology and failure mode" that crate's own
//! docs give as the reason to shell out rather than link a decoder.
//!
//! Two pieces of information, deliberately split so each is testable on
//! its own:
//!
//! * [`probe`] — dimensions, frame rate, frame count, and whether an
//!   audio track exists. This fills in the same fields a decoded WAV
//!   header already gives an audio [`super::super::render::Sample`].
//! * [`keyframes`] — the exact frame indices `ffprobe` reports as key
//!   frames. This is what [`super::decode`] needs to seek correctly: an
//!   AV1 (or any long-GOP codec) P-frame can only be decoded by starting
//!   from its most recent keyframe and decoding forward, so a caller that
//!   wants frame 47 needs to know THAT number, not guess it from a
//!   assumed-regular GOP size — real encoders (as the fixture this module
//!   is tested against demonstrates) do not always place keyframes on a
//!   perfectly regular grid.
//!
//! The JSON/CSV parsing in both is a free function taking already-read
//! text, so the parsing logic is tested against literal fixtures with no
//! subprocess involved, and only a handful of integration tests actually
//! invoke `ffprobe` for real (guarded, see `decode`'s test module doc).

use std::path::Path;
use std::process::{Command, Stdio};

#[derive(Debug)]
pub enum ProbeError {
    MissingBinary(&'static str),
    Io(std::io::Error),
    CommandFailed { program: &'static str, stderr: String },
    UnexpectedOutput(String),
    NoVideoStream,
}

impl std::fmt::Display for ProbeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ProbeError::MissingBinary(b) => write!(f, "'{b}' is not on PATH"),
            ProbeError::Io(e) => write!(f, "could not run ffprobe: {e}"),
            ProbeError::CommandFailed { program, stderr } => write!(f, "{program} failed: {}", stderr.trim()),
            ProbeError::UnexpectedOutput(m) => write!(f, "unexpected ffprobe output: {m}"),
            ProbeError::NoVideoStream => write!(f, "no video stream in this file"),
        }
    }
}
impl std::error::Error for ProbeError {}
impl From<std::io::Error> for ProbeError {
    fn from(e: std::io::Error) -> Self {
        ProbeError::Io(e)
    }
}

fn run(args: &[&str]) -> Result<Vec<u8>, ProbeError> {
    let output = Command::new("ffprobe")
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .map_err(|e| if e.kind() == std::io::ErrorKind::NotFound { ProbeError::MissingBinary("ffprobe") } else { e.into() })?;
    if !output.status.success() {
        return Err(ProbeError::CommandFailed { program: "ffprobe", stderr: String::from_utf8_lossy(&output.stderr).into_owned() });
    }
    Ok(output.stdout)
}

/// What [`probe`] reports.
#[derive(Debug, Clone, PartialEq)]
pub struct VideoProbe {
    pub width: u32,
    pub height: u32,
    pub fps: f64,
    /// From the container's own frame count field when present; `None`
    /// when `ffprobe` doesn't report one without a slower `-count_frames`
    /// pass (which this function deliberately does not make — see
    /// `frame_count` for the pass that does, on request).
    pub duration_secs: f64,
    pub has_audio: bool,
    pub audio_sample_rate: Option<u32>,
    pub audio_channels: Option<u16>,
    pub video_codec: String,
}

/// `"30000/1001"` or `"25/1"` -> a plain `f64`. `"0/0"` (an audio-only
/// stream's frame rate field) returns `None` rather than `NaN`.
fn parse_rational(s: &str) -> Option<f64> {
    let (num, den) = s.split_once('/')?;
    let (num, den): (f64, f64) = (num.parse().ok()?, den.parse().ok()?);
    if den == 0.0 {
        None
    } else {
        Some(num / den)
    }
}

fn parse_probe_json(json: &serde_json::Value) -> Result<VideoProbe, ProbeError> {
    let streams = json.get("streams").and_then(|s| s.as_array()).ok_or_else(|| ProbeError::UnexpectedOutput("no \"streams\" array".into()))?;
    let video = streams.iter().find(|s| s.get("codec_type").and_then(|c| c.as_str()) == Some("video")).ok_or(ProbeError::NoVideoStream)?;
    let width = video.get("width").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
    let height = video.get("height").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
    let fps = video.get("r_frame_rate").and_then(|v| v.as_str()).and_then(parse_rational).unwrap_or(0.0);
    let video_codec = video.get("codec_name").and_then(|v| v.as_str()).unwrap_or("unknown").to_string();

    let audio = streams.iter().find(|s| s.get("codec_type").and_then(|c| c.as_str()) == Some("audio"));
    let audio_sample_rate = audio.and_then(|a| a.get("sample_rate")).and_then(|v| v.as_str()).and_then(|s| s.parse::<u32>().ok());
    let audio_channels = audio.and_then(|a| a.get("channels")).and_then(|v| v.as_u64()).map(|c| c as u16);

    let duration_secs = json.get("format").and_then(|f| f.get("duration")).and_then(|v| v.as_str()).and_then(|s| s.parse::<f64>().ok()).unwrap_or(0.0);

    Ok(VideoProbe { width, height, fps, duration_secs, has_audio: audio.is_some(), audio_sample_rate, audio_channels, video_codec })
}

/// Duration, resolution, frame rate, codec, and whether an audio track
/// exists. One `ffprobe` call, fast (`-show_format -show_streams`, no
/// per-frame pass).
pub fn probe(path: &Path) -> Result<VideoProbe, ProbeError> {
    let path_str = path.to_string_lossy();
    let stdout = run(&["-v", "error", "-print_format", "json", "-show_format", "-show_streams", &path_str])?;
    let json: serde_json::Value = serde_json::from_slice(&stdout).map_err(|e| ProbeError::UnexpectedOutput(e.to_string()))?;
    parse_probe_json(&json)
}

/// Exact keyframe positions, as (0-based frame index, presentation time
/// in seconds). This DOES walk every frame (`-show_entries
/// frame=key_frame,pts_time`), which costs roughly what a full decode
/// costs (`ffprobe` still has to decode headers/motion vectors to find
/// frame boundaries even without producing pixels) — worth it once per
/// source, cached, never repeated per seek. [`super::decode::decode_range`]
/// is what actually uses these to seek correctly.
pub fn keyframes(path: &Path) -> Result<Vec<(i64, f64)>, ProbeError> {
    let path_str = path.to_string_lossy();
    let stdout = run(&[
        "-v", "error", "-select_streams", "v", "-show_entries", "frame=key_frame,pts_time",
        "-of", "csv=p=0", &path_str,
    ])?;
    let text = String::from_utf8(stdout).map_err(|e| ProbeError::UnexpectedOutput(e.to_string()))?;
    Ok(parse_keyframes_csv(&text))
}

fn parse_keyframes_csv(text: &str) -> Vec<(i64, f64)> {
    text.lines()
        .enumerate()
        .filter_map(|(i, line)| {
            let mut parts = line.splitn(2, ',');
            let is_key = parts.next()? == "1";
            if !is_key {
                return None;
            }
            let pts = parts.next()?.trim();
            // A frame with no pts (rare, some containers/codecs) is
            // skipped rather than defaulted to 0.0 — a wrong seek target
            // is worse than one missing keyframe entry, since
            // `nearest_at_or_before` still has every OTHER real one to
            // fall back to.
            let t: f64 = pts.parse().ok()?;
            Some((i as i64, t))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::OnceLock;

    // ---------------------------------------------------- pure parsing

    #[test]
    fn parse_rational_handles_ntsc_integer_and_the_audio_only_zero_case() {
        assert_eq!(parse_rational("30000/1001"), Some(30000.0 / 1001.0));
        assert_eq!(parse_rational("25/1"), Some(25.0));
        assert_eq!(parse_rational("0/0"), None);
        assert_eq!(parse_rational("garbage"), None);
    }

    #[test]
    fn probe_json_parses_a_literal_ffprobe_fixture_with_video_and_audio() {
        let json: serde_json::Value = serde_json::from_str(r#"{
            "streams": [
                {"codec_type": "video", "codec_name": "av1", "width": 320, "height": 180, "r_frame_rate": "24/1"},
                {"codec_type": "audio", "codec_name": "opus", "sample_rate": "48000", "channels": 1}
            ],
            "format": {"duration": "5.008000"}
        }"#).unwrap();
        let p = parse_probe_json(&json).unwrap();
        assert_eq!(p, VideoProbe {
            width: 320, height: 180, fps: 24.0, duration_secs: 5.008,
            has_audio: true, audio_sample_rate: Some(48_000), audio_channels: Some(1),
            video_codec: "av1".into(),
        });
    }

    #[test]
    fn probe_json_without_audio_or_a_video_stream_is_handled_distinctly() {
        let audio_only: serde_json::Value = serde_json::from_str(r#"{
            "streams": [{"codec_type": "audio", "codec_name": "opus", "sample_rate": "48000", "channels": 2}],
            "format": {"duration": "1.0"}
        }"#).unwrap();
        assert!(matches!(parse_probe_json(&audio_only), Err(ProbeError::NoVideoStream)));

        let no_audio: serde_json::Value = serde_json::from_str(r#"{
            "streams": [{"codec_type": "video", "codec_name": "av1", "width": 4, "height": 4, "r_frame_rate": "1/1"}],
            "format": {"duration": "1.0"}
        }"#).unwrap();
        let p = parse_probe_json(&no_audio).unwrap();
        assert!(!p.has_audio);
        assert_eq!(p.audio_sample_rate, None);
    }

    #[test]
    fn keyframes_csv_parses_only_the_key_rows_with_their_frame_index() {
        let csv = "1,0.000000\n0,0.041667\n1,1.000000\n0,1.041667\n";
        assert_eq!(parse_keyframes_csv(csv), vec![(0, 0.0), (2, 1.0)]);
        assert_eq!(parse_keyframes_csv(""), vec![]);
        // A malformed row (no comma, or unparsable pts) is skipped, not a
        // panic — `ffprobe` output is trusted but not blindly indexed.
        assert_eq!(parse_keyframes_csv("garbage\n1,not_a_number\n1,2.0\n"), vec![(2, 2.0)]);
    }

    // ------------------------------------------------ real ffprobe, guarded

    /// Same discipline as `apps/studio/crates/studio-video`'s own
    /// `has_ffmpeg`: check the SPECIFIC capability the fixture needs
    /// (encoding AV1 + Opus), not just that the binaries answer
    /// `-version` — a build can have ffmpeg without libaom or libopus,
    /// and that failure looks nothing like a probe bug when it happens
    /// three function calls downstream.
    pub(crate) fn has_av1() -> bool {
        static CHECKED: OnceLock<bool> = OnceLock::new();
        *CHECKED.get_or_init(|| {
            let ok = |c: &str| Command::new(c).arg("-version").output().map(|o| o.status.success()).unwrap_or(false);
            if !ok("ffmpeg") || !ok("ffprobe") {
                return false;
            }
            let enc = match Command::new("ffmpeg").args(["-hide_banner", "-encoders"]).output() {
                Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).into_owned(),
                _ => return false,
            };
            let have = |n: &str| enc.lines().any(|l| l.split_whitespace().nth(1) == Some(n));
            if !have("libaom-av1") {
                eprintln!("beatspace video tests: ffmpeg cannot encode libaom-av1 — skipping fixture-backed tests");
                return false;
            }
            if !have("libopus") {
                eprintln!("beatspace video tests: ffmpeg cannot encode libopus — skipping fixture-backed tests");
                return false;
            }
            true
        })
    }

    /// A short, real AV1+Opus file via ffmpeg's own `lavfi` test sources —
    /// no bundled binary fixture. `-g` and `-force_key_frames` set a
    /// mostly-regular GOP so tests can assert roughly where keyframes
    /// land, without assuming they are PERFECTLY regular (real encoders,
    /// including this one, insert extra ones on detected scene changes —
    /// `testsrc2`'s own motion pattern is enough to trigger that, which
    /// is discovered fact about this exact command, not a guess).
    pub(crate) fn make_test_fixture(dir: &Path, secs: u32, gop: u32) -> std::path::PathBuf {
        let path = dir.join("fixture.mkv");
        let status = Command::new("ffmpeg")
            .args(["-y", "-hide_banner", "-loglevel", "error"])
            .args(["-f", "lavfi", "-i", &format!("testsrc2=size=320x180:rate=24:duration={secs}")])
            .args(["-f", "lavfi", "-i", &format!("sine=frequency=440:duration={secs}")])
            .args(["-c:v", "libaom-av1", "-cpu-used", "8", "-g", &gop.to_string()])
            .args(["-force_key_frames", &format!("expr:eq(mod(n,{gop}),0)")])
            .args(["-c:a", "libopus", "-b:a", "64k", "-pix_fmt", "yuv420p"])
            .arg(&path)
            .status()
            .expect("running ffmpeg to build the test fixture must succeed");
        assert!(status.success(), "ffmpeg exited non-zero building the test fixture");
        path
    }

    fn tmp_dir(tag: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("bs-video-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn probe_reads_a_real_av1_opus_fixtures_metadata_correctly() {
        if !has_av1() {
            return;
        }
        let dir = tmp_dir("probe");
        let path = make_test_fixture(&dir, 5, 24);
        let p = probe(&path).unwrap();
        assert_eq!((p.width, p.height), (320, 180));
        assert!((p.fps - 24.0).abs() < 0.01, "{}", p.fps);
        assert!((p.duration_secs - 5.0).abs() < 0.1, "{}", p.duration_secs);
        assert!(p.has_audio);
        assert_eq!(p.audio_sample_rate, Some(48_000));
        assert_eq!(p.audio_channels, Some(1));
        assert_eq!(p.video_codec, "av1");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn probe_of_a_nonexistent_file_reports_command_failed_not_a_panic() {
        if !has_av1() {
            return;
        }
        let err = probe(Path::new("/nonexistent/beatspace-video-fixture.mkv")).unwrap_err();
        assert!(matches!(err, ProbeError::CommandFailed { .. }), "{err}");
    }

    #[test]
    fn keyframes_of_a_real_fixture_start_at_frame_zero_and_land_near_every_gop() {
        if !has_av1() {
            return;
        }
        let dir = tmp_dir("keyframes");
        let path = make_test_fixture(&dir, 5, 24);
        let kf = keyframes(&path).unwrap();
        assert_eq!(kf.first().map(|(i, _)| *i), Some(0), "the first frame is always a keyframe");
        // One keyframe at or shortly after every multiple of 24 (real
        // encoders don't always land exactly on the requested boundary —
        // see the module doc), across all five one-second GOPs.
        for boundary in [0i64, 24, 48, 72, 96] {
            assert!(kf.iter().any(|&(i, _)| (i - boundary).abs() <= 2), "no keyframe near {boundary} in {kf:?}");
        }
        // Presentation times are non-decreasing and start at (or very
        // near) zero.
        assert!(kf[0].1 < 0.05, "{:?}", kf[0]);
        assert!(kf.windows(2).all(|w| w[1].1 >= w[0].1));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
