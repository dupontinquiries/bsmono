//! # Scene detection — produce chunk boundaries for the scheduler
//!
//! For M1 we go simple: **fixed-length chunks**. Each chunk is N seconds 
//! of source, with N tunable but defaulting to 10 seconds. This trades 
//! away some compression efficiency (chunk boundaries that fall mid-
//! scene cost SVT-AV1 a fresh keyframe + lookahead reset) but it's:
//! - Predictable (no scene-detector dependency)
//! - Fast (no extra pass)
//! - Easy to reason about (chunk N covers seconds N*10 .. (N+1)*10)
//!
//! Scene-aware splitting is M2 work. The hook is in place: `boundaries()`
//! returns a Vec<u64> of frame indices and that's what the scheduler 
//! consumes — swap out the implementation later without touching the 
//! scheduler.

use crate::probe::VideoInfo;

/// Strategy for picking chunk boundaries.
#[derive(Clone, Debug)]
pub enum SceneStrategy {
    /// Fixed length in seconds. The last chunk may be shorter.
    FixedSeconds(f64),
    /// Fixed length in frames. The last chunk may be shorter.
    FixedFrames(u64),
    // Future: SceneAware { min_seconds, max_seconds, threshold }
}

impl Default for SceneStrategy {
    fn default() -> Self {
        // 10 seconds is a sweet spot for AV1: short enough that crashes 
        // don't cost much wallclock, long enough that the encoder's 
        // lookahead amortizes. av1an default is similar.
        SceneStrategy::FixedSeconds(10.0)
    }
}

/// Compute chunk boundary frame indices for a video.
///
/// Returns a Vec of frame indices `[0, f1, f2, ..., last_frame_exclusive]`.
/// Chunks are `[boundaries[i], boundaries[i+1])`. So N chunks → 
/// boundaries.len() == N+1.
///
/// Guarantees:
/// - boundaries[0] == 0
/// - boundaries.last() == frame_count
/// - boundaries is strictly increasing
/// - every chunk has at least 1 frame (we won't emit zero-length chunks)
pub fn boundaries(info: &VideoInfo, strategy: &SceneStrategy) -> Vec<u64> {
    let total = info.frame_count;
    if total == 0 {
        return vec![0];
    }

    let chunk_frames = match strategy {
        SceneStrategy::FixedSeconds(s) => {
            let n = (s * info.fps).round() as u64;
            n.max(1)
        }
        SceneStrategy::FixedFrames(n) => (*n).max(1),
    };

    let mut out = Vec::with_capacity((total / chunk_frames + 2) as usize);
    let mut cur = 0u64;
    while cur < total {
        out.push(cur);
        cur = cur.saturating_add(chunk_frames);
    }
    out.push(total);
    out
}

/// Convenience: turn boundaries into (start, end) pairs ready to hand to
/// ChunkState::new.
pub fn ranges(boundaries: &[u64]) -> Vec<(u64, u64)> {
    boundaries.windows(2).map(|w| (w[0], w[1])).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info(frames: u64, fps: f64) -> VideoInfo {
        VideoInfo {
            width: 1920, height: 1080,
            frame_count: frames, fps,
            duration_s: frames as f64 / fps,
            pixel_format: "yuv420p".into(),
            codec: "h264".into(),
        }
    }

    #[test]
    fn fixed_seconds_30fps_10s_chunks() {
        let v = info(900, 30.0);  // 30 seconds
        let b = boundaries(&v, &SceneStrategy::FixedSeconds(10.0));
        // 30 seconds / 10s chunks = 3 chunks → 4 boundaries
        assert_eq!(b, vec![0, 300, 600, 900]);
        assert_eq!(ranges(&b), vec![(0, 300), (300, 600), (600, 900)]);
    }

    #[test]
    fn last_chunk_shorter() {
        let v = info(950, 30.0);  // 31.67s
        let b = boundaries(&v, &SceneStrategy::FixedSeconds(10.0));
        // 3 full chunks of 300 frames, then a partial of 50
        assert_eq!(b, vec![0, 300, 600, 900, 950]);
        let r = ranges(&b);
        assert_eq!(r.last().unwrap(), &(900, 950));
    }

    #[test]
    fn fixed_frames_explicit() {
        let v = info(1000, 30.0);
        let b = boundaries(&v, &SceneStrategy::FixedFrames(250));
        assert_eq!(b, vec![0, 250, 500, 750, 1000]);
    }

    #[test]
    fn empty_video_single_boundary() {
        let v = info(0, 30.0);
        let b = boundaries(&v, &SceneStrategy::FixedSeconds(10.0));
        assert_eq!(b, vec![0]);
        assert!(ranges(&b).is_empty());
    }

    #[test]
    fn single_frame_video() {
        let v = info(1, 30.0);
        let b = boundaries(&v, &SceneStrategy::FixedSeconds(10.0));
        assert_eq!(b, vec![0, 1]);
        assert_eq!(ranges(&b), vec![(0, 1)]);
    }

    #[test]
    fn boundaries_strictly_increasing() {
        let v = info(10_000, 23.976);
        let b = boundaries(&v, &SceneStrategy::FixedSeconds(7.5));
        for w in b.windows(2) {
            assert!(w[0] < w[1], "boundary {} should be < {}", w[0], w[1]);
        }
        assert_eq!(*b.last().unwrap(), 10_000);
    }
}
