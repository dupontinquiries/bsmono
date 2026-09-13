//! Getting a video file INTO the arrangement, as separable picture and
//! sound from the moment it's imported — not a combined "video clip" that
//! gets split apart later.
//!
//! [`import`] does three things, and keeps their results in two different
//! places on purpose:
//!
//! 1. [`probe::probe`] — dimensions, frame rate, codec.
//! 2. [`probe::keyframes`] — the seek table [`super::decode`] needs.
//! 3. The audio track, decoded to the SAME `Sample` type an imported
//!    audio file or a captured beat already uses
//!    (`super::super::render::Sample`) — which is what "separable" means
//!    concretely: a video's sound goes straight into the arrangement's
//!    existing audio library, mixed and time-stretched by exactly the
//!    machinery in `render.rs` that already knows nothing about video.
//!    The picture stays in its own [`VideoAsset`], read lazily through
//!    [`super::decode`] rather than decoded whole up front — a two-hour
//!    4K interview does not get fully rasterised into memory just because
//!    someone dragged it onto the timeline.
//!
//! Audio extraction tries the CHEAP path first — `audio_io::decode_to_mono`
//! directly on the container — before falling back to `ffmpeg` extracting
//! the track to a temporary WAV. Said honestly: for THIS module's own
//! Matroska+AV1+Opus test fixture, on THIS crate's Symphonia build, the
//! direct path does not actually succeed (Symphonia's probe cannot open
//! this container here — `the_fixtures_own_symphonia_path_is_pinned_honestly`
//! proves and pins exactly this, so a future Symphonia or feature-flag
//! upgrade that starts opening it is noticed rather than silently
//! changing which code path every test below actually exercises). The
//! direct branch stays because other containers — a `.mov` with PCM
//! audio, for instance — genuinely can go straight through Symphonia in
//! this crate already; it is not dead code, it is just not what THIS
//! fixture proves. Every test in this file is exercising the `ffmpeg`
//! fallback, and says so.

use std::path::{Path, PathBuf};

use super::probe::{self, ProbeError, VideoProbe};
use super::super::render::Sample;

#[derive(Debug)]
pub enum ImportError {
    Probe(ProbeError),
    NoAudioTrack,
    /// Both the direct decode and the `ffmpeg`-extraction fallback failed;
    /// carries the direct attempt's error, since the fallback exists
    /// specifically to route around cases where that one doesn't apply
    /// (no audio track at all is handled separately, as `NoAudioTrack`,
    /// before either path runs).
    AudioDecode(String),
    Io(std::io::Error),
}

impl std::fmt::Display for ImportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ImportError::Probe(e) => write!(f, "probing the video failed: {e}"),
            ImportError::NoAudioTrack => write!(f, "this video has no audio track"),
            ImportError::AudioDecode(e) => write!(f, "could not decode the audio track: {e}"),
            ImportError::Io(e) => write!(f, "{e}"),
        }
    }
}
impl std::error::Error for ImportError {}
impl From<std::io::Error> for ImportError {
    fn from(e: std::io::Error) -> Self {
        ImportError::Io(e)
    }
}
impl From<ProbeError> for ImportError {
    fn from(e: ProbeError) -> Self {
        ImportError::Probe(e)
    }
}

/// The picture half of an imported video: enough to seek and decode
/// frames later ([`super::decode::decode_range`]), nothing decoded yet.
#[derive(Debug, Clone, PartialEq)]
pub struct VideoAsset {
    pub path: PathBuf,
    pub probe: VideoProbe,
    /// `(frame_index, pts_seconds)`, from `probe::keyframes`. Probing this
    /// once at import and holding it is what makes every later seek O(log
    /// n) instead of re-running `ffprobe`'s per-frame walk on every scrub.
    pub keyframes: Vec<(i64, f64)>,
    /// Total frame count, from `probe.duration_secs * probe.fps` rounded
    /// — `ffprobe`'s fast metadata pass (used here, not the slow
    /// `-count_frames` one) doesn't give an exact count, and an estimate
    /// is genuinely all `import` can afford without walking every frame
    /// the way `keyframes` already (unavoidably) does for the picture
    /// side. Held separately from `keyframes.last()` because a source
    /// can end on a run of frames after its last keyframe.
    pub estimated_frame_count: i64,
}

/// Import `path`: probe it, read its keyframe table, and decode its audio
/// track to `target_sr` as a plain mono `Sample` — the same type any
/// other audio import produces, ready for `Library::add_file` (or a
/// dedicated video-audio slot, at the caller's choice; this function
/// doesn't touch the library itself, matching how `render::Sample`
/// construction elsewhere in this crate stays separate from the library
/// that holds it).
pub fn import(path: &Path, target_sr: u32) -> Result<(VideoAsset, Sample), ImportError> {
    let probe = probe::probe(path)?;
    if !probe.has_audio {
        return Err(ImportError::NoAudioTrack);
    }
    let keyframes = probe::keyframes(path)?;
    let estimated_frame_count = (probe.duration_secs * probe.fps).round() as i64;

    let audio = decode_audio_track(path, target_sr)?;
    let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| "video".into());
    let sample = Sample { name, audio, sr: target_sr, planes: None, bpm: None };

    Ok((VideoAsset { path: path.to_path_buf(), probe, keyframes, estimated_frame_count }, sample))
}

fn decode_audio_track(path: &Path, target_sr: u32) -> Result<Vec<f32>, ImportError> {
    // Cheap path: Symphonia demuxes and decodes plenty of containers
    // directly, with no subprocess. See the module doc for which case
    // this module's own tests prove takes this path.
    if let Ok(samples) = crate::audio_io::decode_to_mono(path, target_sr) {
        if !samples.is_empty() {
            return Ok(samples);
        }
    }
    // Fallback: extract the audio track to a temporary WAV via `ffmpeg`,
    // then decode THAT — for containers or codecs Symphonia can't open
    // directly (this crate doesn't otherwise decode, say, AAC or an
    // uncommon container's demuxer).
    extract_audio_via_ffmpeg(path, target_sr)
}

fn extract_audio_via_ffmpeg(path: &Path, target_sr: u32) -> Result<Vec<f32>, ImportError> {
    use std::process::Command;
    let tmp = std::env::temp_dir().join(format!("beatspace-extract-{}-{}.wav", std::process::id(), fastrand_suffix()));
    let status = Command::new("ffmpeg")
        .args(["-y", "-hide_banner", "-loglevel", "error"])
        .args(["-i", &path.to_string_lossy()])
        .args(["-vn", "-ac", "1", "-ar", &target_sr.to_string()])
        .arg(&tmp)
        .status()
        .map_err(|e| ImportError::AudioDecode(format!("running ffmpeg: {e}")))?;
    if !status.success() {
        return Err(ImportError::AudioDecode("ffmpeg audio extraction exited non-zero".into()));
    }
    let result = crate::audio_io::decode_to_mono(&tmp, target_sr).map_err(|e| ImportError::AudioDecode(e.to_string()));
    let _ = std::fs::remove_file(&tmp);
    result
}

/// A short, dependency-free unique suffix (process-local; two calls in
/// the same process never collide because it's just a counter, not
/// randomness pretending to be one).
fn fastrand_suffix() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    COUNTER.fetch_add(1, Ordering::Relaxed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::probe::tests::{has_av1, make_test_fixture};

    fn tmp_dir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("bs-import-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// Says plainly, and permanently, which path the tests below actually
    /// exercise — see the module doc. If a future Symphonia upgrade (or a
    /// feature flag change) starts opening this container directly, THIS
    /// is the test that should start failing, which is the point: it
    /// forces the module doc's honesty claim to be updated deliberately
    /// rather than silently going stale the moment it stops being true.
    #[test]
    fn the_fixtures_own_symphonia_path_is_pinned_honestly() {
        if !has_av1() {
            return;
        }
        let dir = tmp_dir("pathcheck");
        let path = make_test_fixture(&dir, 2, 24);
        let direct = crate::audio_io::decode_to_mono(&path, 22_050);
        let symphonia_opens_it_directly = matches!(&direct, Ok(v) if !v.is_empty());
        assert!(
            !symphonia_opens_it_directly,
            "Symphonia now opens this Matroska+AV1+Opus fixture directly — update the module doc's \
claim (it currently says this does NOT happen) and consider whether the ffmpeg fallback path in \
the OTHER tests in this file is still being exercised the way their comments say it is"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn importing_the_real_fixture_gives_correct_metadata_and_a_playable_audio_track() {
        if !has_av1() {
            return;
        }
        let dir = tmp_dir("basic");
        let path = make_test_fixture(&dir, 5, 24);
        let (asset, sample) = import(&path, 22_050).unwrap();

        assert_eq!((asset.probe.width, asset.probe.height), (320, 180));
        assert!((asset.probe.fps - 24.0).abs() < 0.01);
        assert_eq!(asset.estimated_frame_count, 120, "5s at 24fps");
        assert_eq!(asset.keyframes.first(), Some(&(0, 0.0)));
        assert!(asset.keyframes.len() >= 5, "one per second, at least");

        // The audio: 5s of a 440Hz tone at 22050 Hz.
        assert!((sample.audio.len() as f64 / 22_050.0 - 5.0).abs() < 0.1, "{}", sample.audio.len());
        assert_eq!(sample.sr, 22_050);
        assert!(sample.audio.iter().any(|&x| x.abs() > 0.01), "the tone must not have decoded to silence");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_video_and_its_audio_are_independent_after_import() {
        // "Separable" proven concretely: nothing about VideoAsset
        // references the Sample or vice versa — they can be held,
        // dropped, or placed on different tracks independently. This
        // test exists to catch a future refactor that accidentally
        // couples them (e.g. VideoAsset growing an `audio: Sample`
        // field), which would silently break that property.
        if !has_av1() {
            return;
        }
        let dir = tmp_dir("separable");
        let path = make_test_fixture(&dir, 2, 24);
        let (asset, sample) = import(&path, 22_050).unwrap();
        drop(asset);
        assert!(!sample.audio.is_empty(), "the audio must survive independently of the video asset");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_video_with_no_audio_track_is_a_named_error_not_a_panic() {
        if !has_av1() {
            return;
        }
        let dir = tmp_dir("silent");
        let path = dir.join("silent.mkv");
        let status = std::process::Command::new("ffmpeg")
            .args(["-y", "-hide_banner", "-loglevel", "error"])
            .args(["-f", "lavfi", "-i", "testsrc2=size=64x64:rate=24:duration=1"])
            .args(["-c:v", "libaom-av1", "-cpu-used", "8", "-pix_fmt", "yuv420p"])
            .arg(&path)
            .status()
            .unwrap();
        assert!(status.success());
        assert!(matches!(import(&path, 22_050).unwrap_err(), ImportError::NoAudioTrack));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn importing_a_nonexistent_file_reports_the_probe_error() {
        if !has_av1() {
            return;
        }
        let err = import(Path::new("/nonexistent/beatspace-import.mkv"), 22_050).unwrap_err();
        assert!(matches!(err, ImportError::Probe(_)), "{err}");
    }
}

