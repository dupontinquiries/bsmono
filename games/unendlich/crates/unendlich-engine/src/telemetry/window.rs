//! Rolling time-windowed sample aggregator.
//!
//! Feed it `(now_secs, value_ms)` each frame; it keeps only the samples inside
//! the trailing window and computes summary stats on demand. This is the data
//! source for the in-game "live 15s stats" overlay and for the per-window
//! snapshots in a [`crate::telemetry::SessionReport`].
//!
//! Pure `std` — no external crates. Percentiles use nearest-rank (lower),
//! matching the lightweight box-plot summaries the engine already prints.

use std::collections::VecDeque;

/// Summary of the samples currently inside a [`Window`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WindowStats {
    pub n: usize,
    pub mean: f64,
    pub p50: f64,
    pub p75: f64,
    pub p90: f64,
    pub p99: f64,
    pub min: f64,
    pub max: f64,
}

/// A trailing time-window of `f64` samples (typically milliseconds).
#[derive(Clone, Debug)]
pub struct Window {
    window_secs: f64,
    buf: VecDeque<(f64, f64)>, // (timestamp_secs, value)
    cap: usize,                // hard cap so a bad clock can't grow this unbounded
}

impl Window {
    /// `window_secs` is the trailing duration retained (e.g. `15.0`).
    pub fn new(window_secs: f64) -> Self {
        Self::with_cap(window_secs, 16_384)
    }

    pub fn with_cap(window_secs: f64, cap: usize) -> Self {
        Window {
            window_secs: if window_secs > 0.0 { window_secs } else { 1.0 },
            buf: VecDeque::new(),
            cap: cap.max(1),
        }
    }

    /// Append a sample taken at `now` (monotonic seconds) and evict anything
    /// older than `now - window_secs`. Non-finite values are ignored.
    pub fn push(&mut self, now: f64, value: f64) {
        if !value.is_finite() || !now.is_finite() {
            return;
        }
        self.buf.push_back((now, value));
        let cutoff = now - self.window_secs;
        while let Some(&(t, _)) = self.buf.front() {
            if t < cutoff {
                self.buf.pop_front();
            } else {
                break;
            }
        }
        // Safety valve against a non-monotonic / frozen clock.
        while self.buf.len() > self.cap {
            self.buf.pop_front();
        }
    }

    pub fn len(&self) -> usize {
        self.buf.len()
    }

    pub fn is_empty(&self) -> bool {
        self.buf.is_empty()
    }

    pub fn clear(&mut self) {
        self.buf.clear();
    }

    /// Stats over the samples currently in the window, or `None` if empty.
    pub fn stats(&self) -> Option<WindowStats> {
        let n = self.buf.len();
        if n == 0 {
            return None;
        }
        let mut vs: Vec<f64> = self.buf.iter().map(|&(_, v)| v).collect();
        vs.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let sum: f64 = vs.iter().sum();
        let pct = |p: f64| -> f64 {
            let mut idx = (p / 100.0 * n as f64) as usize;
            if idx >= n {
                idx = n - 1;
            }
            vs[idx]
        };
        Some(WindowStats {
            n,
            mean: sum / n as f64,
            p50: pct(50.0),
            p75: pct(75.0),
            p90: pct(90.0),
            p99: pct(99.0),
            min: vs[0],
            max: vs[n - 1],
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn evicts_to_trailing_window() {
        let mut w = Window::new(15.0);
        for i in 0..(30 * 60) {
            let t = i as f64 / 60.0;
            w.push(t, t);
        }
        let s = w.stats().unwrap();
        assert!((890..=905).contains(&s.n), "n={}", s.n);
        assert!(s.min >= 14.5 && s.min <= 15.5, "min={}", s.min);
    }

    #[test]
    fn percentiles_nearest_rank_lower() {
        let mut w = Window::new(100.0);
        for i in 1..=100 {
            w.push(i as f64 * 0.001, i as f64);
        }
        let s = w.stats().unwrap();
        assert_eq!(s.p50, 51.0);
        assert_eq!(s.p90, 91.0);
        assert_eq!(s.p99, 100.0);
        assert_eq!(s.mean, 50.5);
    }

    #[test]
    fn empty_is_none_and_nonfinite_ignored() {
        let mut w = Window::new(5.0);
        assert!(w.stats().is_none());
        w.push(1.0, f64::NAN);
        w.push(f64::INFINITY, 1.0);
        assert!(w.is_empty());
    }
}
