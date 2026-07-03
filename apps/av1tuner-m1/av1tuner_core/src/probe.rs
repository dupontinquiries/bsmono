//! # Probe — extract source video metadata via ffprobe
//!
//! `ffprobe -v error -count_frames -select_streams v:0 -show_entries 
//! stream=width,height,nb_read_frames,r_frame_rate,duration 
//! -of json input.mkv`
//!
//! We could parse ourselves with `mp4parse` etc. but ffprobe handles 
//! every container + codec already. Subprocess is fine here — probe 
//! runs once per job, not per frame.

use std::path::Path;
use std::process::Command;
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ProbeError {
    #[error("ffprobe not found on PATH")]
    NotFound,
    #[error("ffprobe failed: {0}")]
    Failed(String),
    #[error("ffprobe output parse: {0}")]
    Parse(String),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}

/// What the scheduler needs to know about a source video.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct VideoInfo {
    pub width: u32,
    pub height: u32,
    /// Total frame count. `nb_read_frames` from ffprobe counts every frame
    /// (not a header estimate), so this is exact even for VFR sources.
    pub frame_count: u64,
    /// Frames per second. For VFR content this is the average; chunk 
    /// boundaries should still be aligned to keyframes when present.
    pub fps: f64,
    /// Duration in seconds.
    pub duration_s: f64,
    pub pixel_format: String,
    pub codec: String,
    /// Audio tracks present in the source. Empty if there are none. 
    /// First entry is what audio-encode mode will operate on.
    #[serde(default)]
    pub audio_streams: Vec<AudioStreamInfo>,
}

/// Per-audio-track metadata read at probe time. Used by the UI to show 
/// "source has 2 audio tracks" + by preflight to flag issues.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AudioStreamInfo {
    pub codec: String,
    pub channels: u32,
    pub sample_rate: u32,
    /// Language tag from container metadata (e.g. "eng", "jpn"). Empty 
    /// when not set.
    pub language: String,
}

/// Two-stage probe: try fast first (container metadata only, instant), 
/// fall back to exact frame counting (slow but always correct).
///
/// Fast probe reads `nb_frames` from the container header. Most modern 
/// containers (MP4, MKV, WebM) carry an accurate count; some (raw 
/// streams, repaired files, certain VFR sources) don't and we need to 
/// count frames the slow way.
pub fn probe(input: &Path) -> Result<VideoInfo, ProbeError> {
    let mut info = match probe_fast(input) {
        Ok(info) if info.frame_count > 0 => info,
        Ok(_) | Err(_) => probe_exact(input)?,
    };
    // Audio streams are read in a separate probe so the video-side 
    // logic stays simple. This call is fast (no -count_frames).
    info.audio_streams = probe_audio_streams(input).unwrap_or_default();
    Ok(info)
}

/// Read all audio streams' metadata from the source. Separate from the 
/// video probe so we can run it fast (no frame counting). Returns an 
/// empty vec if the source has no audio.
pub fn probe_audio_streams(input: &Path) -> Result<Vec<AudioStreamInfo>, ProbeError> {
    let ffprobe = which::which("ffprobe").map_err(|_| ProbeError::NotFound)?;
    
    let output = Command::new(&ffprobe)
        .args([
            "-v", "error",
            "-select_streams", "a",
            "-show_entries",
            "stream=codec_name,channels,sample_rate:stream_tags=language",
            "-of", "json",
        ])
        .arg(input)
        .output()?;
    
    if !output.status.success() {
        return Err(ProbeError::Failed(
            String::from_utf8_lossy(&output.stderr).to_string()
        ));
    }
    
    let stdout = String::from_utf8_lossy(&output.stdout);
    parse_audio_streams_json(&stdout)
}

#[derive(Debug, Deserialize)]
struct AudioProbeOutput {
    streams: Vec<AudioStreamRaw>,
}

#[derive(Debug, Deserialize)]
struct AudioStreamRaw {
    codec_name: Option<String>,
    channels: Option<u32>,
    /// ffprobe returns sample_rate as a string in JSON.
    sample_rate: Option<String>,
    tags: Option<AudioTags>,
}

#[derive(Debug, Deserialize)]
struct AudioTags {
    language: Option<String>,
}

fn parse_audio_streams_json(json: &str) -> Result<Vec<AudioStreamInfo>, ProbeError> {
    let parsed: AudioProbeOutput = serde_json::from_str(json)
        .map_err(|e| ProbeError::Parse(e.to_string()))?;
    Ok(parsed.streams.into_iter().map(|s| AudioStreamInfo {
        codec: s.codec_name.unwrap_or_else(|| "unknown".to_string()),
        channels: s.channels.unwrap_or(0),
        sample_rate: s.sample_rate
            .as_deref()
            .and_then(|s| s.parse::<u32>().ok())
            .unwrap_or(0),
        language: s.tags.and_then(|t| t.language).unwrap_or_default(),
    }).collect())
}

/// Fast probe — container-metadata only. ~100ms regardless of source length.
/// May return frame_count == 0 if the container doesn't carry one (raw 
/// streams, malformed files); caller falls back to probe_exact in that case.
pub fn probe_fast(input: &Path) -> Result<VideoInfo, ProbeError> {
    let ffprobe = which::which("ffprobe").map_err(|_| ProbeError::NotFound)?;
    
    let output = Command::new(&ffprobe)
        .args([
            "-v", "error",
            "-select_streams", "v:0",
            "-show_entries",
            "stream=width,height,nb_frames,r_frame_rate,duration,pix_fmt,codec_name",
            "-of", "json",
        ])
        .arg(input)
        .output()?;
    
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(ProbeError::Failed(stderr.to_string()));
    }
    
    let stdout = String::from_utf8_lossy(&output.stdout);
    parse_ffprobe_json_fast(&stdout)
}

/// Exact probe — walks the stream and counts frames. Slow but accurate. 
/// 20-60 seconds on a 1-hour 1080p source.
pub fn probe_exact(input: &Path) -> Result<VideoInfo, ProbeError> {
    let ffprobe = which::which("ffprobe").map_err(|_| ProbeError::NotFound)?;
    
    let output = Command::new(&ffprobe)
        .args([
            "-v", "error",
            "-count_frames",
            "-select_streams", "v:0",
            "-show_entries",
            "stream=width,height,nb_read_frames,r_frame_rate,duration,pix_fmt,codec_name",
            "-of", "json",
        ])
        .arg(input)
        .output()?;
    
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(ProbeError::Failed(stderr.to_string()));
    }
    
    let stdout = String::from_utf8_lossy(&output.stdout);
    parse_ffprobe_json(&stdout)
}

#[derive(Debug, Deserialize)]
struct FfprobeOutput {
    streams: Vec<FfprobeStream>,
}

#[derive(Debug, Deserialize)]
struct FfprobeStream {
    width: u32,
    height: u32,
    /// From `-count_frames` — exact, slow.
    nb_read_frames: Option<String>,
    /// From container metadata — fast, may be missing/wrong.
    nb_frames: Option<String>,
    r_frame_rate: String,
    duration: Option<String>,
    pix_fmt: Option<String>,
    codec_name: Option<String>,
}

fn parse_ffprobe_json(json: &str) -> Result<VideoInfo, ProbeError> {
    let parsed: FfprobeOutput = serde_json::from_str(json)
        .map_err(|e| ProbeError::Parse(e.to_string()))?;
    let s = parsed.streams.into_iter().next()
        .ok_or_else(|| ProbeError::Parse("no video streams".to_string()))?;
    
    let fps = parse_rational(&s.r_frame_rate)
        .ok_or_else(|| ProbeError::Parse(format!("bad r_frame_rate: {}", s.r_frame_rate)))?;
    
    let frame_count = s.nb_read_frames
        .and_then(|s| s.parse::<u64>().ok())
        .ok_or_else(|| ProbeError::Parse("missing nb_read_frames".to_string()))?;
    
    let duration_s = s.duration
        .as_deref()
        .and_then(|s| s.parse::<f64>().ok())
        .unwrap_or_else(|| frame_count as f64 / fps);
    
    Ok(VideoInfo {
        width: s.width, height: s.height, frame_count, fps, duration_s,
        pixel_format: s.pix_fmt.unwrap_or_else(|| "unknown".to_string()),
        codec: s.codec_name.unwrap_or_else(|| "unknown".to_string()),
        audio_streams: Vec::new(),
    })
}

/// Parse for fast probe — uses container nb_frames (may be 0 / missing).
fn parse_ffprobe_json_fast(json: &str) -> Result<VideoInfo, ProbeError> {
    let parsed: FfprobeOutput = serde_json::from_str(json)
        .map_err(|e| ProbeError::Parse(e.to_string()))?;
    let s = parsed.streams.into_iter().next()
        .ok_or_else(|| ProbeError::Parse("no video streams".to_string()))?;
    
    let fps = parse_rational(&s.r_frame_rate)
        .ok_or_else(|| ProbeError::Parse(format!("bad r_frame_rate: {}", s.r_frame_rate)))?;
    
    // 0 frames = "container doesn't know"; signal that to the caller 
    // so it falls back to exact probe.
    let frame_count = s.nb_frames
        .as_deref()
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(0);
    
    // If we have duration but no frame_count, compute frame_count from 
    // duration * fps. Approximate but usually correct.
    let duration_s = s.duration
        .as_deref()
        .and_then(|s| s.parse::<f64>().ok())
        .unwrap_or(0.0);
    
    let frame_count = if frame_count > 0 {
        frame_count
    } else if duration_s > 0.0 && fps > 0.0 {
        (duration_s * fps).round() as u64
    } else {
        0
    };
    
    Ok(VideoInfo {
        width: s.width, height: s.height, frame_count, fps, duration_s,
        pixel_format: s.pix_fmt.unwrap_or_else(|| "unknown".to_string()),
        codec: s.codec_name.unwrap_or_else(|| "unknown".to_string()),
        audio_streams: Vec::new(),
    })
}

fn parse_rational(s: &str) -> Option<f64> {
    let (num, den) = s.split_once('/')?;
    let num: f64 = num.parse().ok()?;
    let den: f64 = den.parse().ok()?;
    if den == 0.0 { return None; }
    Some(num / den)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_ntsc_framerate() {
        assert!((parse_rational("30000/1001").unwrap() - 29.97003).abs() < 0.001);
    }

    #[test]
    fn parse_24fps() {
        assert!((parse_rational("24/1").unwrap() - 24.0).abs() < 0.001);
    }

    #[test]
    fn parse_zero_denom_rejected() {
        assert!(parse_rational("30/0").is_none());
    }

    #[test]
    fn parse_minimal_ffprobe_json() {
        let json = r#"{
            "streams": [{
                "width": 1920,
                "height": 1080,
                "nb_read_frames": "12345",
                "r_frame_rate": "30/1",
                "duration": "411.5",
                "pix_fmt": "yuv420p",
                "codec_name": "h264"
            }]
        }"#;
        let info = parse_ffprobe_json(json).unwrap();
        assert_eq!(info.width, 1920);
        assert_eq!(info.height, 1080);
        assert_eq!(info.frame_count, 12345);
        assert!((info.fps - 30.0).abs() < 0.001);
        assert_eq!(info.codec, "h264");
    }

    #[test]
    fn parse_missing_streams_errors() {
        let json = r#"{"streams": []}"#;
        assert!(parse_ffprobe_json(json).is_err());
    }

    #[test]
    fn fast_parse_uses_nb_frames() {
        let json = r#"{
            "streams": [{
                "width": 1280, "height": 720,
                "nb_frames": "600",
                "r_frame_rate": "30/1",
                "duration": "20.0",
                "pix_fmt": "yuv420p", "codec_name": "h264"
            }]
        }"#;
        let info = parse_ffprobe_json_fast(json).unwrap();
        assert_eq!(info.frame_count, 600);
    }

    #[test]
    fn fast_parse_falls_back_to_duration_when_nb_frames_missing() {
        // No nb_frames, but duration + fps lets us estimate.
        let json = r#"{
            "streams": [{
                "width": 1280, "height": 720,
                "r_frame_rate": "30/1",
                "duration": "20.0",
                "pix_fmt": "yuv420p", "codec_name": "h264"
            }]
        }"#;
        let info = parse_ffprobe_json_fast(json).unwrap();
        assert_eq!(info.frame_count, 600);
    }

    #[test]
    fn fast_parse_returns_zero_when_no_count_info() {
        // Neither nb_frames nor duration — caller should fall back to exact.
        let json = r#"{
            "streams": [{
                "width": 1280, "height": 720,
                "r_frame_rate": "30/1",
                "pix_fmt": "yuv420p", "codec_name": "h264"
            }]
        }"#;
        let info = parse_ffprobe_json_fast(json).unwrap();
        assert_eq!(info.frame_count, 0);
    }
}
