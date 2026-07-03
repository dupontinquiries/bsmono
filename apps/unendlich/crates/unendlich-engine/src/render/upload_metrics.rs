//! `render::upload_metrics` — Tier 1c of the operator's upload-throughput plan:
//! "report per-frame `uploaded_chunks / uploaded_bytes / memcpy_ms / gpu_copy_ms`
//! as a metric line so every tier below has a before/after." Nothing measured
//! this before — the r596 off-thread packing and copy-coalescing work landed
//! with no way to confirm they actually helped. This closes that loop.
//!
//! Lives on `WorldRenderer` (single-threaded — called only from the frame
//! thread), so unlike `mesher_timer` (used from worker threads) this needs no
//! internal `Mutex`; plain fields behind `&mut self`.

use std::time::{Duration, Instant};

/// Accumulates upload activity over a reporting window, then resets.
#[derive(Debug)]
pub struct UploadMetrics {
    window_start: Instant,
    chunks: u64,
    vert_bytes: u64,
    idx_bytes: u64,
    memcpy_time: Duration,
    /// Cached `UNENDLICH_UPLOAD_METRICS=1` check (env reads are cheap but this
    /// runs every frame; read once).
    enabled: bool,
    /// How often to flush a log line.
    period: Duration,
}

impl UploadMetrics {
    pub fn new() -> Self {
        let enabled = std::env::var("UNENDLICH_UPLOAD_METRICS")
            .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
            .unwrap_or(false);
        Self {
            window_start: Instant::now(),
            chunks: 0,
            vert_bytes: 0,
            idx_bytes: 0,
            memcpy_time: Duration::ZERO,
            enabled,
            period: Duration::from_secs(1),
        }
    }

    /// Whether metrics collection is worth doing at all this frame — callers can
    /// skip the (cheap but non-zero) `Instant::now()` bracketing when this is
    /// `false`, so the feature costs nothing when not requested.
    #[inline]
    pub fn enabled(&self) -> bool {
        self.enabled
    }

    /// Record one chunk's upload: byte counts plus how long the CPU spent
    /// packing/copying it into staging (the L1 memcpy cost from the upload plan).
    pub fn record_chunk(&mut self, vert_bytes: u64, idx_bytes: u64, memcpy_time: Duration) {
        if !self.enabled {
            return;
        }
        self.chunks += 1;
        self.vert_bytes += vert_bytes;
        self.idx_bytes += idx_bytes;
        self.memcpy_time += memcpy_time;
    }

    /// If a full reporting window has elapsed, format + reset it and return the
    /// line to log; otherwise `None`. Call once per frame.
    pub fn maybe_flush(&mut self) -> Option<String> {
        if !self.enabled {
            return None;
        }
        let elapsed = self.window_start.elapsed();
        if elapsed < self.period {
            return None;
        }
        let secs = elapsed.as_secs_f64().max(1e-6);
        let line = format!(
            "[upload] {:.1} chunks/s, {:.2} MB/s vert+idx, memcpy {:.2} ms/s CPU ({} chunks, \
             {:.1} KB avg/chunk this window)",
            self.chunks as f64 / secs,
            (self.vert_bytes + self.idx_bytes) as f64 / secs / 1_000_000.0,
            self.memcpy_time.as_secs_f64() * 1000.0 / secs,
            self.chunks,
            if self.chunks > 0 {
                (self.vert_bytes + self.idx_bytes) as f64 / self.chunks as f64 / 1000.0
            } else {
                0.0
            },
        );
        self.chunks = 0;
        self.vert_bytes = 0;
        self.idx_bytes = 0;
        self.memcpy_time = Duration::ZERO;
        self.window_start = Instant::now();
        Some(line)
    }
}

impl Default for UploadMetrics {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disabled_by_default_records_nothing() {
        // (Assumes UNENDLICH_UPLOAD_METRICS isn't set in the test environment.)
        let mut m = UploadMetrics { enabled: false, ..UploadMetrics::new() };
        m.record_chunk(100, 50, Duration::from_millis(1));
        assert_eq!(m.chunks, 0, "disabled metrics must not accumulate");
        assert!(m.maybe_flush().is_none());
    }

    #[test]
    fn enabled_accumulates_and_flushes_after_period() {
        let mut m = UploadMetrics {
            enabled: true,
            period: Duration::from_millis(0), // flush immediately for the test
            ..UploadMetrics::new()
        };
        m.record_chunk(1000, 200, Duration::from_micros(500));
        m.record_chunk(2000, 400, Duration::from_micros(500));
        assert_eq!(m.chunks, 2);
        assert_eq!(m.vert_bytes, 3000);
        assert_eq!(m.idx_bytes, 600);
        let line = m.maybe_flush();
        assert!(line.is_some());
        assert!(line.unwrap().contains("chunks/s"));
        // window reset after flush
        assert_eq!(m.chunks, 0);
    }

    #[test]
    fn enabled_flag_gates_recording() {
        assert!(!UploadMetrics { enabled: false, ..UploadMetrics::new() }.enabled());
        assert!(UploadMetrics { enabled: true, ..UploadMetrics::new() }.enabled());
    }
}
