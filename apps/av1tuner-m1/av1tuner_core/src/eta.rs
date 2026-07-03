//! # ETA — encode throughput and time-remaining estimation
//!
//! Watches completed chunks, computes recent throughput (frames/second 
//! and bytes/second of encoded output), projects ETA for the remaining 
//! pending chunks.
//!
//! Why "recent" not "since-start": encode rate varies a lot with content
//! complexity. A high-motion section near the start might finish at 
//! 30fps; a static section near the end might run at 200fps. The 
//! since-start average lags real throughput. We use the last 10 chunks 
//! by default — short enough to follow rate changes, long enough to 
//! smooth out per-chunk noise.

use std::collections::VecDeque;
use std::time::Instant;

use crate::chunk::ChunkState;

/// Rolling stats over the N most recent completed chunks.
///
/// Uses **wall-clock time** between completions, not per-chunk encode 
/// duration. This is critical for parallel encoding: if N workers each 
/// take 10 seconds, the wall-clock throughput is N× a single worker, 
/// and the ETA must reflect that. Per-chunk encode_ms is also tracked 
/// for display purposes but isn't used for ETA math.
#[derive(Debug)]
pub struct EtaTracker {
    /// Window size — how many recent chunks to average over.
    window: usize,
    /// Recent samples: (frames_in_chunk, encode_ms, wall_clock_time_of_completion).
    samples: VecDeque<(u64, u64, Instant)>,
    /// When the tracker started.
    started: Instant,
    /// Frames recorded since the tracker started.
    total_frames_seen: u64,
}

impl EtaTracker {
    pub fn new() -> Self {
        Self::with_window(10)
    }
    
    pub fn with_window(window: usize) -> Self {
        Self {
            window: window.max(1),
            samples: VecDeque::with_capacity(window.max(1)),
            started: Instant::now(),
            total_frames_seen: 0,
        }
    }
    
    /// Record a chunk that just finished. Wall-clock completion time 
    /// is captured here (Instant::now()) so the rolling rate reflects 
    /// actual throughput across all parallel workers.
    pub fn record_chunk(&mut self, frames: u64, encode_ms: u64) {
        self.total_frames_seen = self.total_frames_seen.saturating_add(frames);
        if self.samples.len() == self.window {
            self.samples.pop_front();
        }
        self.samples.push_back((frames, encode_ms, Instant::now()));
    }
    
    /// Frames per second based on wall-clock progress over the rolling 
    /// window. Correctly accounts for parallelism: if 4 workers each 
    /// take 10s and produce 4 chunks in that 10s, fps reflects 4× 
    /// one-worker throughput.
    pub fn frames_per_second(&self) -> f64 {
        // Need at least 2 samples to measure wall-clock delta.
        if self.samples.len() < 2 {
            // Fallback: since-tracker-start rate.
            let elapsed_s = self.started.elapsed().as_secs_f64().max(0.001);
            return self.total_frames_seen as f64 / elapsed_s;
        }
        
        // Wall-clock window: from the OLDEST sample's completion time 
        // to the NEWEST sample's completion time. The frames produced 
        // in that window are all samples EXCEPT the oldest (which marks 
        // the start, not a frame produced in the window).
        let oldest_time = self.samples.front().expect("len >= 2").2;
        let newest_time = self.samples.back().expect("len >= 2").2;
        let wall_clock_s = newest_time.duration_since(oldest_time).as_secs_f64();
        
        if wall_clock_s <= 0.0 {
            return 0.0;
        }
        
        // Frames produced in the window = sum of frames excluding the 
        // oldest sample (whose completion bounds the window's start).
        let frames_in_window: u64 = self.samples.iter()
            .skip(1)
            .map(|(f, _, _)| f)
            .sum();
        
        frames_in_window as f64 / wall_clock_s
    }
    
    /// Estimate seconds remaining for `remaining_frames` at current rate.
    /// Returns None if no samples yet (avoid showing "0:00:00 remaining" 
    /// before we know anything).
    pub fn eta_seconds(&self, remaining_frames: u64) -> Option<f64> {
        if self.samples.is_empty() {
            return None;
        }
        let fps = self.frames_per_second();
        if fps <= 0.0 {
            return None;
        }
        Some(remaining_frames as f64 / fps)
    }
    
    /// Returns `Hh:MM:SS` for the ETA, or "—" if not computable.
    pub fn eta_string(&self, remaining_frames: u64) -> String {
        match self.eta_seconds(remaining_frames) {
            Some(s) => format_duration(s as u64),
            None => "—".to_string(),
        }
    }
    
    /// Number of samples currently in the window. Useful for UI to 
    /// show "warming up" until samples >= 2 or 3.
    pub fn sample_count(&self) -> usize {
        self.samples.len()
    }
}

impl Default for EtaTracker {
    fn default() -> Self { Self::new() }
}

/// Build a fresh EtaTracker. Previously pre-seeded from chunk history, 
/// but the tracker uses wall-clock time between completions for fps math 
/// — pre-seeding from chunks whose completion times are unknown would 
/// give a wrong reading. The tracker rebuilds quickly once new chunks 
/// start completing.
///
/// `chunks` is kept as a parameter for API stability (and so the caller 
/// can still pass it without conditional logic); we just don't use it.
pub fn from_chunks(_chunks: &[ChunkState], window: usize) -> EtaTracker {
    EtaTracker::with_window(window)
}

/// "1h 23m 45s" or "23m 45s" or "45s" depending on magnitude.
pub fn format_duration(total_seconds: u64) -> String {
    let h = total_seconds / 3600;
    let m = (total_seconds % 3600) / 60;
    let s = total_seconds % 60;
    if h > 0 {
        format!("{}h {:02}m {:02}s", h, m, s)
    } else if m > 0 {
        format!("{}m {:02}s", m, s)
    } else {
        format!("{}s", s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_tracker_has_no_eta() {
        let t = EtaTracker::new();
        assert_eq!(t.eta_seconds(1000), None);
        assert_eq!(t.eta_string(1000), "—");
    }

    #[test]
    fn single_sample_uses_fallback() {
        // With only 1 sample we don't have a wall-clock window, so 
        // frames_per_second uses the since-start fallback.
        let mut t = EtaTracker::new();
        t.record_chunk(300, 10_000);
        let fps = t.frames_per_second();
        // Since-start fallback. The exact number depends on test timing, 
        // but it should be very high (tracker just started, frames were 
        // recorded ~immediately).
        assert!(fps > 0.0);
    }

    #[test]
    fn wall_clock_rate_with_two_samples() {
        // Two samples bracket a wall-clock window. Use a known sleep 
        // to make the math predictable.
        let mut t = EtaTracker::new();
        t.record_chunk(100, 5000);
        std::thread::sleep(std::time::Duration::from_millis(100));
        t.record_chunk(100, 5000);
        // Window: 100ms, frames in window (excluding oldest) = 100.
        // fps = 100 / 0.1 = 1000.
        let fps = t.frames_per_second();
        assert!(fps > 800.0 && fps < 1200.0, "got fps {}", fps);
    }

    #[test]
    fn duration_format_variants() {
        assert_eq!(format_duration(0), "0s");
        assert_eq!(format_duration(45), "45s");
        assert_eq!(format_duration(75), "1m 15s");
        assert_eq!(format_duration(3661), "1h 01m 01s");
    }

    #[test]
    fn from_chunks_returns_fresh_tracker() {
        // After the wall-clock-based ETA rework, from_chunks no longer 
        // pre-seeds — see the function's rationale comment.
        let mut chunks = Vec::new();
        for i in 0..5 {
            let mut c = ChunkState::new(i, i as u64 * 100, (i + 1) as u64 * 100);
            c.encode_ms = Some(1000);
            chunks.push(c);
        }
        let t = from_chunks(&chunks, 10);
        assert_eq!(t.sample_count(), 0);
    }
}
