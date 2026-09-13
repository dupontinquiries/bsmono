//! The real pixel path: `ffmpeg`-backed, GOP-aware, scaled by `ffmpeg`'s
//! own filter rather than decode-then-downsample in Rust.
//!
//! ## Why seeking works the way it does here
//!
//! An AV1 (or any long-GOP codec) frame that isn't itself a keyframe can
//! only be produced by decoding forward from its most recent keyframe —
//! there is no shortcut. So "decode frame 50" is really "find the
//! keyframe at or before 50 (`probe::keyframes` gives the true positions;
//! see that module for why a regular-GOP assumption is wrong even on a
//! deliberately-regular encode), seek the DEMUXER there with `-ss` before
//! `-i` (fast: no decoding happens to skip to it), then decode forward
//! and throw away the frames before 50."
//!
//! This was proven byte-for-byte, not assumed: decoding the fixture this
//! module's tests build from frame 0 and comparing frame 50 against
//! seeking to keyframe 48's timestamp and decoding 3 frames forward gave
//! IDENTICAL bytes on this ffmpeg build. `decode_range`'s tests re-run
//! that same comparison as a regression check.
//!
//! ## Why scaling happens in the `-vf scale=` filter, not after
//!
//! `ffmpeg`'s own scaler runs as part of the same pipeline that's already
//! decoding — a Thumbnail-tier request decodes the same AV1 bitstream at
//! the same cost, then scales down a cheap operation, rather than this
//! crate decoding a full-resolution frame into memory and downsampling it
//! itself. The `-ss`/decode-forward/discard dance above still has to
//! happen at whatever tier is requested — scaling doesn't change how many
//! frames must be decoded to reach a target, only how much each one costs
//! once decoded (see `cache_policy`'s module doc for the fuller version
//! of this point, which is the whole reason a spatial tier is a separate
//! knob from a temporal stride).

use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};

#[derive(Debug)]
pub enum DecodeError {
    MissingBinary(&'static str),
    Io(std::io::Error),
    CommandFailed { stderr: String },
    /// Asked to decode a source with no known keyframes at all — either
    /// `probe::keyframes` found none (a corrupt or all-intra-less file,
    /// which shouldn't happen for real video) or an empty slice was
    /// passed in by mistake.
    NoKeyframes,
}

impl std::fmt::Display for DecodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DecodeError::MissingBinary(b) => write!(f, "'{b}' is not on PATH"),
            DecodeError::Io(e) => write!(f, "could not run ffmpeg: {e}"),
            DecodeError::CommandFailed { stderr } => write!(f, "ffmpeg failed: {}", stderr.trim()),
            DecodeError::NoKeyframes => write!(f, "no keyframes known for this source"),
        }
    }
}
impl std::error::Error for DecodeError {}
impl From<std::io::Error> for DecodeError {
    fn from(e: std::io::Error) -> Self {
        DecodeError::Io(e)
    }
}

/// One decoded frame: `w * h * 4` bytes, RGBA, row-major.
#[derive(Debug, Clone, PartialEq)]
pub struct Frame {
    pub w: u32,
    pub h: u32,
    pub rgba: Vec<u8>,
}

/// The keyframe at or before `target`, by presentation time. `keyframes`
/// is `(frame_index, pts_seconds)` as `probe::keyframes` returns it —
/// assumed sorted by both index and time ascending, which is what
/// `ffprobe`'s frame order already guarantees and this function does not
/// re-check (a caller feeding it out-of-order data gets an out-of-order
/// answer, not a panic).
pub fn nearest_at_or_before(keyframes: &[(i64, f64)], target: i64) -> Option<(i64, f64)> {
    keyframes.iter().rev().find(|&&(i, _)| i <= target).copied()
}

/// Decode `count` frames starting at source frame `start_frame`, scaled to
/// `(out_w, out_h)`. Internally seeks to the nearest keyframe at or before
/// `start_frame`, decodes forward, and discards the leading frames before
/// `start_frame` — see the module doc for why this is the only correct
/// way to reach a non-keyframe at all, and why it was proven rather than
/// assumed correct.
///
/// Returns fewer than `count` frames, never an error, if the source ends
/// first — reaching the end of the material is not a failure.
pub fn decode_range(
    path: &Path,
    keyframes: &[(i64, f64)],
    start_frame: i64,
    count: i64,
    out_w: u32,
    out_h: u32,
) -> Result<Vec<Frame>, DecodeError> {
    if count <= 0 {
        return Ok(Vec::new());
    }
    let (kf_index, kf_time) = nearest_at_or_before(keyframes, start_frame).ok_or(DecodeError::NoKeyframes)?;
    let leading = (start_frame - kf_index).max(0);
    let total = leading + count;

    let mut child = Command::new("ffmpeg")
        .args(["-y", "-hide_banner", "-loglevel", "error"])
        .args(["-ss", &format!("{kf_time:.6}")])
        .args(["-i", &path.to_string_lossy()])
        .args(["-frames:v", &total.to_string()])
        .args(["-vf", &format!("scale={out_w}:{out_h}")])
        .args(["-pix_fmt", "rgba", "-f", "rawvideo"])
        .arg("pipe:1")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| if e.kind() == std::io::ErrorKind::NotFound { DecodeError::MissingBinary("ffmpeg") } else { e.into() })?;

    let frame_bytes = out_w as usize * out_h as usize * 4;
    let mut stdout = child.stdout.take().expect("piped stdout");
    let mut all = Vec::new();
    // Read to EOF rather than trusting the exit status alone: ffmpeg can
    // still have written every frame it was going to write and THEN hit
    // the source's real end before `-frames:v` was satisfied, which is
    // the normal "asked past the end of the material" case this function
    // promises not to treat as an error.
    stdout.read_to_end(&mut all)?;
    let status = child.wait()?;

    let n_complete = all.len() / frame_bytes;
    let usable = &all[..n_complete * frame_bytes];
    if !status.success() && n_complete == 0 {
        let mut stderr = String::new();
        if let Some(mut e) = child.stderr.take() {
            let _ = e.read_to_string(&mut stderr);
        }
        return Err(DecodeError::CommandFailed { stderr });
    }

    let frames: Vec<Frame> = usable
        .chunks_exact(frame_bytes)
        .skip(leading as usize)
        .map(|c| Frame { w: out_w, h: out_h, rgba: c.to_vec() })
        .collect();
    Ok(frames)
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::probe::{self, tests::{has_av1, make_test_fixture}};

    #[test]
    fn nearest_at_or_before_picks_the_last_keyframe_not_past_the_target() {
        let kf = vec![(0, 0.0), (24, 1.0), (48, 2.0), (72, 3.0)];
        assert_eq!(nearest_at_or_before(&kf, 50), Some((48, 2.0)));
        assert_eq!(nearest_at_or_before(&kf, 24), Some((24, 1.0)), "exactly on a keyframe returns itself");
        assert_eq!(nearest_at_or_before(&kf, 0), Some((0, 0.0)));
        assert_eq!(nearest_at_or_before(&kf, -1), None, "before the first keyframe: nothing to seek to");
        assert_eq!(nearest_at_or_before(&[], 10), None);
    }

    fn tmp_dir(tag: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("bs-decode-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn decode_range_at_full_resolution_is_byte_identical_to_decoding_from_the_start() {
        // The exact property proven by hand against this fixture before
        // writing any of this file: seeking to a keyframe and decoding
        // forward must give the SAME bytes as decoding from frame 0 and
        // discarding everything before the target. Re-run here as a
        // permanent regression check, not just a one-off proof.
        if !has_av1() {
            return;
        }
        let dir = tmp_dir("identical");
        let path = make_test_fixture(&dir, 5, 24);
        let kf = probe::keyframes(&path).unwrap();

        // Ground truth: decode everything from frame 0 at native size,
        // enough to cover both compared indices below (50 and 51).
        let baseline = decode_range(&path, &[(0, 0.0)], 0, 52, 320, 180).unwrap();
        assert_eq!(baseline.len(), 52);

        // Via a real seek: frame 50's nearest keyframe should NOT be 0
        // (this is exercising the seek path, not accidentally the
        // trivial case where they coincide).
        let (kf_index, _) = nearest_at_or_before(&kf, 50).unwrap();
        assert_ne!(kf_index, 0, "test setup: expected frame 50 to need a real seek");
        let via_seek = decode_range(&path, &kf, 50, 3, 320, 180).unwrap();
        assert_eq!(via_seek.len(), 3);
        assert_eq!(via_seek[0], baseline[50], "frame 50 via seek must match frame 50 via full decode");
        assert_eq!(via_seek[1], baseline[51], "sanity: the following frame also lines up");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn decode_range_scales_via_ffmpegs_own_filter_and_reports_the_requested_size() {
        if !has_av1() {
            return;
        }
        let dir = tmp_dir("scale");
        let path = make_test_fixture(&dir, 2, 24);
        let kf = probe::keyframes(&path).unwrap();
        let thumbs = decode_range(&path, &kf, 0, 5, 80, 45).unwrap();
        assert_eq!(thumbs.len(), 5);
        for f in &thumbs {
            assert_eq!((f.w, f.h), (80, 45));
            assert_eq!(f.rgba.len(), 80 * 45 * 4);
        }
        // Different frames of a moving test pattern must not all decode
        // to the identical buffer — a scale filter mistake that always
        // returned frame 0 (e.g. a wrong pipe read) would still pass a
        // dimensions-only check.
        assert!(thumbs.windows(2).any(|w| w[0].rgba != w[1].rgba), "frames should actually differ");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn decode_range_past_the_end_of_the_material_returns_fewer_frames_not_an_error() {
        if !has_av1() {
            return;
        }
        let dir = tmp_dir("eof");
        let path = make_test_fixture(&dir, 1, 24); // exactly 24 frames
        let kf = probe::keyframes(&path).unwrap();
        let got = decode_range(&path, &kf, 20, 20, 64, 36).unwrap();
        assert!(got.len() < 20 && !got.is_empty(), "{}", got.len());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn decode_range_with_no_keyframes_or_zero_count_is_handled_without_a_subprocess() {
        assert!(matches!(decode_range(Path::new("/x"), &[], 0, 5, 4, 4), Err(DecodeError::NoKeyframes)));
        // count <= 0 short-circuits before ever touching the path, so an
        // otherwise-invalid path is fine here.
        assert_eq!(decode_range(Path::new("/does/not/exist"), &[(0, 0.0)], 0, 0, 4, 4).unwrap(), Vec::new());
    }
}
