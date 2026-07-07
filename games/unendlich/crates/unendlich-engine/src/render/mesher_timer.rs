//! Per-LOD timing for mesh-pool worker threads (v0.23.1).
//!
//! # Why this exists
//!
//! The v0.23.0 CPU timer revealed that the render-thread bottleneck
//! is `wait_fences` (mean 5ms, max 857ms) — bursty GPU work, not
//! steady CPU upload. `record` mean is 0.22ms, so the render thread
//! is doing essentially no upload work per frame.
//!
//! This points at the meshing-pool worker threads as the candidate
//! culprit: if Near-LOD greedy meshes routinely take 50-200ms each,
//! the chunk pipeline backs up, `mesh_q` saturates (we observed
//! `mesh_q: 179/128` in the playtest), and chunks land in bursts
//! that produce slow GPU frames (the `wait_fences` spikes).
//!
//! This module instruments the worker dispatch with per-LOD timing.
//! Per-second window log + lifetime summary at shutdown, mirroring
//! the existing `cpu_timer` and `gpu_timer` shape.
//!
//! # Design notes
//!
//! Shared `Arc<Mutex<MesherTimer>>` across all workers. One lock
//! acquire per chunk meshing call. With ~1-200ms per call, contention
//! is irrelevant. Could go lock-free with per-thread accumulators
//! merged at flush, but YAGNI — the simpler design is enough.
//!
//! The same `ZoneStats` accumulator the GPU and CPU timers use.
//! Indexed by `LodLevel` enum (4 tiers).

use std::sync::Mutex;

use unendlich_shared::zone_stats::ZoneStats;
use crate::world::lod::LodLevel;

/// Per-LOD mesher timing accumulator.
///
/// One instance shared across all worker threads via
/// `Arc<MesherTimer>`. Internal mutability via Mutex so the public
/// API is `&self` (workers don't need a mut reference).
pub struct MesherTimer {
    inner: Mutex<MesherTimerInner>,
}

struct MesherTimerInner {
    /// Lifetime stats per LOD tier, indexed by `LodLevel::index()`.
    lifetime: [ZoneStats; LodLevel::COUNT],
    /// 1-second window: total μs and count per tier. Reset by
    /// `flush_log`.
    window_us:    [u64; LodLevel::COUNT],
    window_count: [u64; LodLevel::COUNT],
}

impl MesherTimer {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(MesherTimerInner {
                lifetime:     std::array::from_fn(|_| ZoneStats::default()),
                window_us:    [0; LodLevel::COUNT],
                window_count: [0; LodLevel::COUNT],
            }),
        }
    }

    /// Record one mesh-call observation. `elapsed_us` is the wall-
    /// clock time the worker spent meshing this chunk (excludes
    /// channel send/receive, which are negligible).
    pub fn record(&self, lod: LodLevel, elapsed_us: u64) {
        let mut g = self.inner.lock().expect("MesherTimer mutex poisoned");
        let i = lod.index();
        g.lifetime[i].push(elapsed_us);
        g.window_us[i] += elapsed_us;
        g.window_count[i] += 1;
    }

    /// Format and reset the per-LOD window stats. Returns a one-line
    /// string suitable for the FPS log block. Empty tiers are
    /// omitted to keep the line short.
    pub fn flush_log(&self) -> String {
        let mut g = self.inner.lock().expect("MesherTimer mutex poisoned");
        let mut s = String::from("  mesher:");
        let mut any = false;
        for lod in LodLevel::iter() {
            let i = lod.index();
            let count = g.window_count[i];
            if count == 0 { continue; }
            any = true;
            let mean_ms = (g.window_us[i] as f64) / (count as f64) / 1000.0;
            s.push_str(&format!(" {}={:.2}ms×{}", lod.short_name(), mean_ms, count));
        }
        if !any {
            s.push_str(" <no chunks meshed this window>");
        }
        // Reset window.
        for i in 0..LodLevel::COUNT {
            g.window_us[i] = 0;
            g.window_count[i] = 0;
        }
        s
    }

    /// Format the per-LOD lifetime stats. Multi-line; intended
    /// for shutdown output. v0.24.2c.2: includes percentiles.
    /// No "over-budget" framing here — meshing isn't on the frame
    /// budget critical path (it's per-chunk on worker threads),
    /// so percentile spread is the metric that matters.
    pub fn flush_lifetime_summary(&self) -> String {
        let g = self.inner.lock().expect("MesherTimer mutex poisoned");
        let mut out = String::from("Mesher timer lifetime summary (per LOD):\n");
        let mut any = false;
        let label_width = LodLevel::iter()
            .map(|l| l.short_name().len())
            .max()
            .unwrap_or(8);
        for lod in LodLevel::iter() {
            let i = lod.index();
            let st = &g.lifetime[i];
            if st.n == 0 {
                out.push_str(&format!("  {:<width$}  (no chunks meshed)\n",
                    lod.short_name(), width = label_width));
                continue;
            }
            any = true;
            out.push_str(&unendlich_shared::zone_stats::render_zone_rich(
                lod.short_name(), label_width, st, None,
            ));
            out.push('\n');
        }
        if !any {
            return "Mesher timer lifetime summary: no chunks meshed".to_string();
        }

        out.push_str("  ── box plots ──────────────────────────────────────────────────\n");
        for lod in LodLevel::iter() {
            let i = lod.index();
            let st = &g.lifetime[i];
            if st.n == 0 { continue; }
            out.push_str(&unendlich_shared::zone_stats::render_zone_boxplot(
                lod.short_name(), label_width, st,
            ));
            out.push('\n');
        }

        out.truncate(out.trim_end().len());
        out
    }
}

impl Default for MesherTimer {
    fn default() -> Self { Self::new() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_timer_summary_is_sentinel() {
        let t = MesherTimer::new();
        let s = t.flush_lifetime_summary();
        assert!(s.contains("no chunks"), "expected sentinel, got: {}", s);
    }

    /// Single record produces one observation in the right tier.
    #[test]
    fn single_record_lands_in_correct_tier() {
        let t = MesherTimer::new();
        t.record(LodLevel::Mid, 5_000);
        let g = t.inner.lock().unwrap();
        assert_eq!(g.lifetime[LodLevel::Mid.index()].n, 1);
        assert_eq!(g.lifetime[LodLevel::Mid.index()].mean_us, 5_000.0);
        assert_eq!(g.lifetime[LodLevel::Near.index()].n, 0);
        assert_eq!(g.lifetime[LodLevel::Far.index()].n, 0);
        assert_eq!(g.lifetime[LodLevel::VeryFar.index()].n, 0);
    }

    /// Multi-tier records accumulate independently.
    #[test]
    fn multi_tier_records_independent() {
        let t = MesherTimer::new();
        t.record(LodLevel::Near, 50_000);    // 50ms
        t.record(LodLevel::Near, 100_000);   // 100ms
        t.record(LodLevel::Far, 1_000);      // 1ms
        let g = t.inner.lock().unwrap();
        assert_eq!(g.lifetime[LodLevel::Near.index()].n, 2);
        assert_eq!(g.lifetime[LodLevel::Near.index()].mean_us, 75_000.0);
        assert_eq!(g.lifetime[LodLevel::Far.index()].n, 1);
        assert_eq!(g.lifetime[LodLevel::Far.index()].mean_us, 1_000.0);
        assert_eq!(g.lifetime[LodLevel::Mid.index()].n, 0);
    }

    /// `flush_log` clears window but preserves lifetime.
    #[test]
    fn flush_log_clears_window_preserves_lifetime() {
        let t = MesherTimer::new();
        t.record(LodLevel::Near, 10_000);
        let _ = t.flush_log();
        let g = t.inner.lock().unwrap();
        assert_eq!(g.window_us[LodLevel::Near.index()], 0);
        assert_eq!(g.window_count[LodLevel::Near.index()], 0);
        assert_eq!(g.lifetime[LodLevel::Near.index()].n, 1, "lifetime should survive flush");
    }

    /// `flush_log` with no records returns a sentinel.
    #[test]
    fn flush_log_empty_returns_sentinel() {
        let t = MesherTimer::new();
        let s = t.flush_log();
        assert!(s.contains("no chunks meshed"), "expected sentinel, got: {}", s);
    }

    /// Lifetime summary mentions every tier that received an
    /// observation, in the iter() order.
    #[test]
    fn lifetime_summary_orders_by_iter() {
        let t = MesherTimer::new();
        for lod in LodLevel::iter() {
            t.record(lod, 1_000);
        }
        let summary = t.flush_lifetime_summary();
        let mut last_pos: usize = 0;
        for lod in LodLevel::iter() {
            let pos = summary.find(lod.short_name())
                .expect(&format!("tier {} missing from summary", lod.short_name()));
            assert!(pos >= last_pos,
                "tier {} appeared out of iter order in: {}", lod.short_name(), summary);
            last_pos = pos;
        }
    }

    /// Concurrent records from multiple "threads" (simulated)
    /// produce correct totals. Welford via the shared accumulator
    /// requires the Mutex to be held for the whole record() call;
    /// this test catches a hypothetical regression that did `lock,
    /// read, unlock, modify, re-lock`.
    #[test]
    fn concurrent_records_accumulate_correctly() {
        use std::sync::Arc;
        use std::thread;
        let t = Arc::new(MesherTimer::new());
        let mut handles = Vec::new();
        for _ in 0..4 {
            let t2 = Arc::clone(&t);
            handles.push(thread::spawn(move || {
                for _ in 0..100 {
                    t2.record(LodLevel::Near, 1_000);
                }
            }));
        }
        for h in handles { h.join().unwrap(); }
        let g = t.inner.lock().unwrap();
        assert_eq!(g.lifetime[LodLevel::Near.index()].n, 400);
        assert_eq!(g.lifetime[LodLevel::Near.index()].mean_us, 1_000.0);
    }
}
