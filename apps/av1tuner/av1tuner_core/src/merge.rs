//! # merge — stitch per-clip encoded outputs into one file with chapters
//!
//! A merge group encodes each clip independently (reusing the normal
//! job → chunk → scheduler → concat pipeline) into per-clip MKVs, then
//! this module stitches those MKVs into a single output, inserting a
//! chapter marker at each clip boundary so the merged file is navigable
//! back to its sources.
//!
//! Two responsibilities:
//! 1. `build_chapters_metadata` — generate an ffmpeg metadata file with
//!    one chapter per source clip, named after the clip.
//! 2. `stitch` — ffmpeg concat-demuxer over the per-clip MKVs + apply
//!    the chapter metadata → final output.
//!
//! Resolution policy (scale-to-smallest etc) is applied during the
//! per-clip encode (the scale filter is added to that clip's ffmpeg
//! command), NOT here — by stitch time all clips already match.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::io::Write;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum MergeError {
    #[error("ffmpeg not found")]
    FfmpegNotFound,
    #[error("ffmpeg failed: {0}")]
    Failed(String),
    #[error("no clips to merge")]
    NoClips,
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}

/// A single encoded clip ready to be stitched: its on-disk path + the
/// display name for its chapter + its duration in seconds.
#[derive(Clone, Debug)]
pub struct MergeClip {
    pub encoded_path: PathBuf,
    pub chapter_name: String,
    pub duration_s: f64,
}

pub struct MergeResult {
    pub output_bytes: u64,
    pub clips_merged: usize,
    pub total_duration_s: f64,
}

/// Build an ffmpeg metadata file describing chapters — one per clip,
/// back to back. Chapter timestamps use a 1/1000 timebase (ms).
/// Returns the path to the written metadata file (inside `work_dir`).
pub fn build_chapters_metadata(
    clips: &[MergeClip],
    work_dir: &Path,
) -> Result<PathBuf, MergeError> {
    let meta_path = work_dir.join("chapters.txt");
    let mut f = std::fs::File::create(&meta_path)?;

    writeln!(f, ";FFMETADATA1")?;

    let mut cursor_ms: u64 = 0;
    for clip in clips {
        let start_ms = cursor_ms;
        let dur_ms = (clip.duration_s * 1000.0).round() as u64;
        let end_ms = start_ms + dur_ms.saturating_sub(1).max(1);
        cursor_ms = start_ms + dur_ms;

        writeln!(f, "[CHAPTER]")?;
        writeln!(f, "TIMEBASE=1/1000")?;
        writeln!(f, "START={}", start_ms)?;
        writeln!(f, "END={}", end_ms)?;
        writeln!(f, "title={}", escape_ffmetadata(&clip.chapter_name))?;
    }

    Ok(meta_path)
}

/// Escape a string for an ffmetadata value (=, ;, #, \\, newline).
fn escape_ffmetadata(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '=' | ';' | '#' | '\\' => { out.push('\\'); out.push(c); }
            '\n' => out.push_str("\\n"),
            _ => out.push(c),
        }
    }
    out
}

/// Stitch the per-clip encoded MKVs into a single output with chapters.
///
/// Uses ffmpeg's concat demuxer with stream copy (`-c copy`) so the
/// already-encoded video isn't re-encoded — fast and lossless. Requires
/// all clips share codec + resolution + pixel format, which the
/// per-clip encode guarantees.
pub fn stitch(
    clips: &[MergeClip],
    output: &Path,
    work_dir: &Path,
) -> Result<MergeResult, MergeError> {
    if clips.is_empty() {
        return Err(MergeError::NoClips);
    }
    let ffmpeg = which::which("ffmpeg").map_err(|_| MergeError::FfmpegNotFound)?;

    std::fs::create_dir_all(work_dir)?;

    // 1. Concat list file.
    let list_path = work_dir.join("merge_list.txt");
    {
        let mut f = std::fs::File::create(&list_path)?;
        for clip in clips {
            // ffmpeg concat demuxer requires single-quote escaping.
            let p = clip.encoded_path.to_string_lossy().replace('\'', "'\\''");
            writeln!(f, "file '{}'", p)?;
        }
    }

    // 2. Chapter metadata.
    let chapters_path = build_chapters_metadata(clips, work_dir)?;

    // 3. ffmpeg: concat demuxer input + chapter metadata input → copy.
    let result = Command::new(&ffmpeg)
        .args([
            "-hide_banner", "-loglevel", "error", "-nostdin", "-y",
            "-f", "concat", "-safe", "0",
            "-i",
        ])
        .arg(&list_path)
        .arg("-i")
        .arg(&chapters_path)
        .args([
            "-map", "0",
            "-map_metadata", "1",
            "-map_chapters", "1",
            "-c", "copy",
        ])
        .arg(output)
        .output()?;

    if !result.status.success() {
        return Err(MergeError::Failed(
            String::from_utf8_lossy(&result.stderr).trim().to_string()
        ));
    }

    let output_bytes = std::fs::metadata(output).map(|m| m.len()).unwrap_or(0);
    let total_duration_s = clips.iter().map(|c| c.duration_s).sum();

    Ok(MergeResult {
        output_bytes,
        clips_merged: clips.len(),
        total_duration_s,
    })
}

/// The ffmpeg scale-filter string for a target resolution, or None if
/// no scaling needed. Used by the per-clip encode when a resolution
/// policy forces clips to a common size. `force_original_aspect` pads
/// rather than stretches when aspect ratios differ.
pub fn scale_filter(target: (u32, u32)) -> String {
    // scale=W:H:force_original_aspect_ratio=decrease then pad to exact
    // target so mismatched aspect ratios letterbox instead of stretch.
    format!(
        "scale={w}:{h}:force_original_aspect_ratio=decrease,\
         pad={w}:{h}:(ow-iw)/2:(oh-ih)/2",
        w = target.0, h = target.1
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escapes_metadata_specials() {
        assert_eq!(escape_ffmetadata("a=b"), "a\\=b");
        assert_eq!(escape_ffmetadata("a;b"), "a\\;b");
        assert_eq!(escape_ffmetadata("normal title"), "normal title");
    }

    #[test]
    fn chapters_metadata_layout() {
        let clips = vec![
            MergeClip {
                encoded_path: PathBuf::from("a.mkv"),
                chapter_name: "Intro".into(),
                duration_s: 10.0,
            },
            MergeClip {
                encoded_path: PathBuf::from("b.mkv"),
                chapter_name: "Main".into(),
                duration_s: 20.0,
            },
        ];
        let dir = std::env::temp_dir().join(format!("merge_test_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let meta = build_chapters_metadata(&clips, &dir).unwrap();
        let content = std::fs::read_to_string(&meta).unwrap();
        assert!(content.contains(";FFMETADATA1"));
        assert!(content.contains("START=0"));
        assert!(content.contains("START=10000")); // second chapter at 10s
        assert!(content.contains("title=Intro"));
        assert!(content.contains("title=Main"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn scale_filter_format() {
        let f = scale_filter((1280, 720));
        assert!(f.contains("1280:720"));
        assert!(f.contains("force_original_aspect_ratio=decrease"));
        assert!(f.contains("pad="));
    }

    #[test]
    fn stitch_empty_errors() {
        let r = stitch(&[], Path::new("/tmp/out.mkv"), Path::new("/tmp"));
        assert!(matches!(r, Err(MergeError::NoClips)));
    }
}
