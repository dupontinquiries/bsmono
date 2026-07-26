//! metricspanel.rs — the GPU phase breakdown, rendered for the chat page.
//!
//! Reads the engine's live counters and attributes them to `logscope::gpumetrics::Phase`, so the panel
//! answers "where did this token's time go" rather than showing raw numbers a reader has to interpret.
//!
//! ## What is real here and what is not
//!
//! The phase totals come from counters the engine genuinely maintains (`GPU_SYNC_NS`, `GPU_SETUP_NS`,
//! `MOE_BYTES`, cache hits/misses). The per-layer HEATMAP does not: nothing in the decode loop currently
//! records a span per (layer, phase), so there is no per-layer data to draw. The panel says so instead of
//! rendering a plausible-looking grid from data it does not have — an invented heatmap is worse than none,
//! because it looks authoritative.

use logscope::gpumetrics::{Phase, Span, Timeline};

/// A snapshot of the engine's counters at one instant.
#[derive(Clone, Copy, Default, PartialEq, Debug)]
pub struct Counters {
    pub submits: u64,
    pub polls: u64,
    pub setup_ns: u64,
    pub sync_ns: u64,
    pub moe_fills: u64,
    pub moe_hits: u64,
    pub moe_bytes: u64,
    pub bg_hits: u64,
    pub bg_misses: u64,
    pub pool_hits: u64,
    pub pool_misses: u64,
    pub tokens: u64,
}

/// Read the engine's live counters.
///
/// Returns zeros when built without the engine, so the panel degrades to "nothing measured" rather than
/// needing a feature check at every call site.
pub fn snapshot(tokens: u64) -> Counters {
    #[cfg(feature = "wgpu-engine")]
    {
        use std::sync::atomic::Ordering::Relaxed;
        Counters {
            submits: gpu_kernels::GPU_SUBMITS.load(Relaxed),
            polls: gpu_kernels::GPU_POLLS.load(Relaxed),
            setup_ns: gpu_kernels::GPU_SETUP_NS.load(Relaxed),
            sync_ns: gpu_kernels::GPU_SYNC_NS.load(Relaxed),
            moe_fills: gpu_kernels::MOE_FILLS.load(Relaxed),
            moe_hits: gpu_kernels::MOE_HITS.load(Relaxed),
            moe_bytes: gpu_kernels::MOE_BYTES.load(Relaxed),
            bg_hits: gpu_kernels::BG_HITS.load(Relaxed),
            bg_misses: gpu_kernels::BG_MISSES.load(Relaxed),
            pool_hits: gpu_kernels::POOL_HITS.load(Relaxed),
            pool_misses: gpu_kernels::POOL_MISSES.load(Relaxed),
            tokens,
        }
    }
    #[cfg(not(feature = "wgpu-engine"))]
    { Counters { tokens, ..Default::default() } }
}

impl Counters {
    /// Difference between two snapshots, for per-generation rather than per-process figures.
    pub fn since(&self, base: &Counters) -> Counters {
        Counters {
            submits: self.submits.saturating_sub(base.submits),
            polls: self.polls.saturating_sub(base.polls),
            setup_ns: self.setup_ns.saturating_sub(base.setup_ns),
            sync_ns: self.sync_ns.saturating_sub(base.sync_ns),
            moe_fills: self.moe_fills.saturating_sub(base.moe_fills),
            moe_hits: self.moe_hits.saturating_sub(base.moe_hits),
            moe_bytes: self.moe_bytes.saturating_sub(base.moe_bytes),
            bg_hits: self.bg_hits.saturating_sub(base.bg_hits),
            bg_misses: self.bg_misses.saturating_sub(base.bg_misses),
            pool_hits: self.pool_hits.saturating_sub(base.pool_hits),
            pool_misses: self.pool_misses.saturating_sub(base.pool_misses),
            tokens: self.tokens.saturating_sub(base.tokens),
        }
    }

    /// Attribute the counters to phases. `wall_ns` is the measured generation time.
    ///
    /// UPLOAD IS DELIBERATELY NOT GIVEN A DURATION. The engine times sync and setup; it does not time
    /// host→device traffic. The first version of this estimated upload time from bytes at an assumed
    /// bandwidth, and on real Scout numbers that estimate consumed ALL the unaccounted time, leaving zero
    /// compute — a fabricated number crowding out a real one. Bytes moved is reported as the fact it is,
    /// and everything the engine did not time is Compute, labelled so the reader knows untimed upload is
    /// inside it.
    pub fn to_timeline(&self, wall_ns: u64) -> Timeline {
        let mut t = Timeline::new(16);
        let mut push = |p: Phase, ns: u64, bytes: u64| {
            if ns > 0 { t.record(Span { token: 0, layer: 0, phase: p, ns, bytes }); }
        };
        push(Phase::Sync, self.sync_ns, 0);
        push(Phase::Submit, self.setup_ns, 0);
        let accounted = self.sync_ns + self.setup_ns;
        push(Phase::Compute, wall_ns.saturating_sub(accounted), self.moe_bytes);
        t
    }

    fn rate(hits: u64, misses: u64) -> Option<f32> {
        let n = hits + misses;
        if n == 0 { None } else { Some(hits as f32 / n as f32) }
    }

    /// Lines a reader can act on. Empty when nothing has been measured yet.
    pub fn report(&self, wall_ns: u64) -> Vec<String> {
        if wall_ns == 0 || self.tokens == 0 { return Vec::new(); }
        let mut out = self.to_timeline(wall_ns).summary();
        out.push("(compute includes host→device upload, which the engine does not time)".into());
        out.push(format!("{} submits ({}/tok), {} polls",
            self.submits, self.submits / self.tokens.max(1), self.polls));
        if let Some(r) = Self::rate(self.bg_hits, self.bg_misses) {
            out.push(format!("bind-group reuse {:.0}% ({} / {})",
                r * 100.0, self.bg_hits, self.bg_hits + self.bg_misses));
        }
        if let Some(r) = Self::rate(self.moe_hits, self.moe_fills) {
            out.push(format!("expert cache {:.0}% resident ({} hits / {} fills, {:.1} GiB moved)",
                r * 100.0, self.moe_hits, self.moe_fills,
                self.moe_bytes as f64 / (1u64 << 30) as f64));
        }
        if let Some(r) = Self::rate(self.pool_hits, self.pool_misses) {
            out.push(format!("buffer pool reuse {:.0}%", r * 100.0));
        }
        // The ceiling comes LAST and is the line worth reading: it says whether any of the numbers above
        // are worth acting on. A cache hit rate is interesting only if CPU time is recoverable at all.
        if let Some(h) = logscope::gpumetrics::headroom(
            wall_ns, self.setup_ns, self.sync_ns, self.tokens, self.submits) {
            out.push(h.verdict());
        }
        out
    }

    /// Why the per-layer heatmap is unavailable, or None once spans are recorded.
    ///
    /// Stated rather than silently omitted: a panel that just shows nothing invites the reader to assume
    /// there is nothing to see.
    pub fn heatmap_unavailable(&self) -> Option<&'static str> {
        Some("per-layer heatmap needs span recording in the decode loop — the engine reports totals only, \
so there is no (layer × token) data to draw yet")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c() -> Counters {
        Counters { submits: 4604, polls: 193, setup_ns: 7_000_000, sync_ns: 3_025_000_000,
                   moe_fills: 500, moe_hits: 100, moe_bytes: 20 << 30,
                   bg_hits: 456, bg_misses: 2914, pool_hits: 900, pool_misses: 100, tokens: 4 }
    }

    #[test]
    fn unaccounted_time_becomes_compute_and_upload_gets_no_invented_duration() {
        // The engine times sync and setup and nothing else, so the remainder is compute — including
        // untimed upload. An earlier version estimated upload time from bytes at an assumed bandwidth;
        // on these very numbers that estimate ate the entire remainder and left compute at zero, which
        // is a fabricated figure displacing a real one.
        let t = c().to_timeline(3_792_000_000);
        let total: u64 = t.totals().values().sum();
        assert_eq!(total, 3_792_000_000, "phases must account for the whole wall time");
        assert!(t.totals().contains_key(&Phase::Compute));
        assert!(!t.totals().contains_key(&Phase::Upload), "no timer for upload, so no Upload phase");
        // the bytes are still reported, as a fact rather than a duration
        assert_eq!(t.bytes(Phase::Compute), 20 << 30);
        assert!(c().report(3_792_000_000).iter().any(|s| s.contains("does not time")));
    }

    #[test]
    fn the_dominant_phase_matches_the_engines_own_verdict() {
        // The engine logged "GPU-sync-bound, 80.0% accounted" for these numbers; the panel must not
        // disagree with the log line sitting next to it.
        let t = c().to_timeline(3_792_000_000);
        let (p, share) = t.dominant().unwrap();
        assert_eq!(p, Phase::Sync);
        assert!(share > 0.7, "sync share {share}");
    }

    #[test]
    fn a_delta_gives_per_generation_numbers_not_per_process() {
        let base = Counters { submits: 4000, tokens: 2, sync_ns: 1_000_000_000, ..Default::default() };
        let d = c().since(&base);
        assert_eq!(d.submits, 604);
        assert_eq!(d.tokens, 2);
        assert_eq!(d.sync_ns, 2_025_000_000);
        // saturating: a counter that reset must not wrap into a huge number
        let d2 = base.since(&c());
        assert_eq!(d2.submits, 0);
    }

    #[test]
    fn nothing_measured_reports_nothing_rather_than_zeroes() {
        assert!(Counters::default().report(0).is_empty());
        assert!(Counters { tokens: 0, ..c() }.report(1_000_000).is_empty());
    }

    #[test]
    fn rates_are_absent_not_zero_when_there_is_no_data() {
        // A 0% cache rate and "no lookups happened" are different facts; showing the first for the second
        // would send someone optimising a cache that was never consulted.
        let mut z = c();
        z.bg_hits = 0; z.bg_misses = 0;
        let r = z.report(3_792_000_000);
        assert!(!r.iter().any(|s| s.contains("bind-group reuse")));
        assert!(r.iter().any(|s| s.contains("expert cache")), "other rates still shown");
    }

    #[test]
    fn the_missing_heatmap_explains_itself() {
        let why = c().heatmap_unavailable().expect("not yet instrumented");
        assert!(why.contains("span recording"));
    }
}
