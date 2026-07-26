//! gpumetrics.rs — where a token's time actually goes: upload vs stall vs sync vs compute.
//!
//! The engine already counts submits, polls, setup/sync nanoseconds, MoE fills and bytes moved. What it
//! could not do is say WHERE in a token those went — a single "GPU-sync-bound, 79.9% accounted" line
//! cannot distinguish a layer that stalled on a router readback from one that stalled uploading 68 MiB of
//! expert weights. Those have opposite fixes.
//!
//! Two outputs, deliberately:
//!
//! * A **heatmap** over (layer x token), which is the view that makes streaming behaviour obvious — a
//!   pinned expert set shows as a few bright rows, thrashing shows as uniform heat.
//! * **Chrome Trace Event JSON**, so the same data opens in `chrome://tracing` or Perfetto with their
//!   flame graphs, filtering and measurement tools already built. Producing the format costs a few dozen
//!   lines and is strictly better than reimplementing a timeline viewer.
//!
//! Sampling is CPU-side wall time. GPU timestamp queries (`wgpu::Features::TIMESTAMP_QUERY`) would give
//! true device-side durations and are the obvious upgrade — but they are a device feature that Honeykrisp
//! may not expose, whereas this works everywhere and needs no capability check.

use std::collections::BTreeMap;

/// What a span of time was spent on. Kept small: these are the categories with DIFFERENT fixes.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Phase {
    /// Host→device weight traffic (expert fills, staging writes).
    Upload,
    /// GPU-side format conversion (Q4_K → q4).
    Convert,
    /// Actual matmul/attention dispatch time.
    Compute,
    /// Blocking readback: the CPU waiting on the GPU (router logits, logits).
    Sync,
    /// Queue submission and command recording overhead.
    Submit,
}

impl Phase {
    pub fn label(self) -> &'static str {
        match self {
            Phase::Upload => "upload", Phase::Convert => "convert", Phase::Compute => "compute",
            Phase::Sync => "sync", Phase::Submit => "submit",
        }
    }
    pub const ALL: [Phase; 5] = [Phase::Upload, Phase::Convert, Phase::Compute, Phase::Sync, Phase::Submit];

    /// The action a dominant phase implies. This is the point of separating them.
    pub fn remedy(self) -> &'static str {
        match self {
            Phase::Upload => "expert working set exceeds resident capacity — pin more or widen the ring",
            Phase::Convert => "converting on every fill — cache converted experts or convert at load",
            Phase::Compute => "genuinely compute-bound; kernel or quantization work",
            Phase::Sync => "blocking readbacks — routing on the CPU forces one stall per layer",
            Phase::Submit => "too many submits per token — batch passes into fewer submissions",
        }
    }
}

/// One measured span.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Span {
    pub token: u32,
    pub layer: u32,
    pub phase: Phase,
    pub ns: u64,
    /// Bytes moved, for Upload/Convert. 0 elsewhere.
    pub bytes: u64,
}

/// A bounded recording of spans.
///
/// Bounded because this runs in the decode loop: an unbounded log would grow without limit on a long
/// generation and change the thing it is measuring.
#[derive(Clone, Debug)]
pub struct Timeline {
    spans: Vec<Span>,
    cap: usize,
    dropped: usize,
}

impl Timeline {
    pub fn new(cap: usize) -> Self { Timeline { spans: Vec::new(), cap: cap.max(1), dropped: 0 } }

    pub fn record(&mut self, s: Span) {
        if self.spans.len() >= self.cap { self.dropped += 1; return; }
        self.spans.push(s);
    }

    pub fn spans(&self) -> &[Span] { &self.spans }
    /// Spans discarded because the buffer was full — reported, never silently lost.
    pub fn dropped(&self) -> usize { self.dropped }
    pub fn is_empty(&self) -> bool { self.spans.is_empty() }

    /// Total nanoseconds per phase.
    pub fn totals(&self) -> BTreeMap<Phase, u64> {
        let mut m = BTreeMap::new();
        for s in &self.spans { *m.entry(s.phase).or_insert(0) += s.ns; }
        m
    }

    /// The phase consuming the most time, with its share. None when nothing was recorded.
    pub fn dominant(&self) -> Option<(Phase, f32)> {
        let t = self.totals();
        let total: u64 = t.values().sum();
        if total == 0 { return None; }
        t.into_iter().max_by_key(|(_, ns)| *ns).map(|(p, ns)| (p, ns as f32 / total as f32))
    }

    /// Heat grid for one phase: `rows[layer][token]` in nanoseconds.
    ///
    /// A (layer x token) grid is the view that makes streaming legible — a stable pin set is a few bright
    /// rows against a dark field, while thrashing is uniform heat.
    pub fn heatmap(&self, phase: Phase, n_layers: usize, n_tokens: usize) -> Vec<Vec<u64>> {
        let mut g = vec![vec![0u64; n_tokens]; n_layers];
        for s in &self.spans {
            if s.phase != phase { continue; }
            let (l, t) = (s.layer as usize, s.token as usize);
            if l < n_layers && t < n_tokens { g[l][t] += s.ns; }
        }
        g
    }

    /// Rows whose heat is far above the median — the layers actually costing you something.
    ///
    /// Returns (layer, ns) sorted hottest first. `factor` is how many times the median counts as hot.
    pub fn hot_layers(&self, phase: Phase, n_layers: usize, factor: f32) -> Vec<(u32, u64)> {
        let mut per: Vec<u64> = vec![0; n_layers];
        for s in &self.spans {
            if s.phase == phase && (s.layer as usize) < n_layers { per[s.layer as usize] += s.ns; }
        }
        // Median over ALL layers, including the silent ones. Taking it over only the active layers
        // makes a single hot layer its own baseline — it can never exceed factor x itself, so the
        // detector reports nothing exactly when there IS something. (That is what the first version did,
        // and the heatmap test caught it.)
        let mut sorted = per.clone();
        if sorted.is_empty() { return Vec::new(); }
        sorted.sort_unstable();
        let median = sorted[sorted.len() / 2];
        let mut out: Vec<(u32, u64)> = per.iter().enumerate()
            .filter(|(_, ns)| **ns > 0 && **ns as f32 > median as f32 * factor)
            .map(|(i, ns)| (i as u32, *ns)).collect();
        out.sort_by_key(|(_, ns)| std::cmp::Reverse(*ns));
        out
    }

    /// Bytes moved per phase, for the phases where that is meaningful.
    pub fn bytes(&self, phase: Phase) -> u64 {
        self.spans.iter().filter(|s| s.phase == phase).map(|s| s.bytes).sum()
    }

    /// Chrome Trace Event JSON — opens in chrome://tracing and Perfetto.
    ///
    /// Emitting the format rather than building a timeline viewer: those tools already have flame graphs,
    /// filtering, and measurement. Each PHASE becomes a process and each LAYER a thread, so the default
    /// view is "phases stacked, layers down the side", which is the arrangement being looked for.
    /// Timestamps are microseconds (`ts`/`dur`), as the format requires.
    pub fn to_chrome_trace(&self) -> String {
        let mut out = String::from("[\n");
        let mut cursor: BTreeMap<(Phase, u32), u64> = BTreeMap::new();
        for (i, s) in self.spans.iter().enumerate() {
            let key = (s.phase, s.layer);
            let ts = cursor.entry(key).or_insert(0);
            let dur_us = (s.ns / 1000).max(1);
            if i > 0 { out.push_str(",\n"); }
            out.push_str(&format!(
                "  {{\"name\":\"{} L{}\",\"cat\":\"{}\",\"ph\":\"X\",\"pid\":{},\"tid\":{},\"ts\":{},\"dur\":{},\
\"args\":{{\"token\":{},\"bytes\":{}}}}}",
                s.phase.label(), s.layer, s.phase.label(),
                s.phase as u8, s.layer, *ts, dur_us, s.token, s.bytes));
            *ts += dur_us;
        }
        out.push_str("\n]\n");
        out
    }

    /// Render the heatmap as text: one row per layer, one cell per token.
    ///
    /// Text rather than only a GUI widget because the thing being debugged usually shows up in a LOG that
    /// gets pasted somewhere, and a picture that only exists inside a running app cannot be pasted. Scale
    /// is per-call relative to the hottest cell, since absolute nanoseconds vary by orders of magnitude
    /// between phases and a fixed scale would make every phase but one look empty.
    pub fn render_heatmap(&self, phase: Phase, n_layers: usize, n_tokens: usize) -> Vec<String> {
        const RAMP: [char; 8] = [' ', '.', ':', '-', '=', '+', '*', '#'];
        let g = self.heatmap(phase, n_layers, n_tokens);
        let peak = g.iter().flatten().copied().max().unwrap_or(0);
        let mut out = vec![format!("{} heatmap — rows=layer, cols=token, peak {:.2} ms/cell",
            phase.label(), peak as f64 / 1e6)];
        if peak == 0 { out.push("  (nothing recorded for this phase)".into()); return out; }
        for (l, row) in g.iter().enumerate() {
            let bar: String = row.iter().map(|&v| {
                // index by proportion of peak; only an exactly-zero cell gets blank, so a faint-but-real
                // cell is never indistinguishable from silence
                if v == 0 { RAMP[0] }
                else { RAMP[(1 + (v * 6 / peak.max(1)) as usize).min(7)] }
            }).collect();
            out.push(format!("  L{l:<3}|{bar}|"));
        }
        out
    }

    /// A short operator-facing summary: what dominated, and what to do about it.
    pub fn summary(&self) -> Vec<String> {
        let mut out = Vec::new();
        let totals = self.totals();
        let total: u64 = totals.values().sum();
        if total == 0 { return vec!["no spans recorded".into()]; }
        for p in Phase::ALL {
            let ns = totals.get(&p).copied().unwrap_or(0);
            if ns == 0 { continue; }
            let by = self.bytes(p);
            let b = if by > 0 { format!(", {:.1} GiB", by as f64 / (1u64 << 30) as f64) } else { String::new() };
            out.push(format!("{:>7}: {:>5.1}% ({:.1} ms{b})",
                p.label(), ns as f32 * 100.0 / total as f32, ns as f64 / 1e6));
        }
        if let Some((p, share)) = self.dominant() {
            out.push(format!("dominant: {} at {:.0}% — {}", p.label(), share * 100.0, p.remedy()));
        }
        if self.dropped > 0 {
            out.push(format!("NOTE: {} spans dropped (buffer full) — percentages are of what was recorded",
                self.dropped));
        }
        out
    }
}

/// What is the MOST a change to CPU-side work could win?
///
/// Amdahl, applied to the engine's own profile line. Exists because optimisation effort is routinely
/// spent on the wrong half: caching, batching and dispatch-count work can only ever recover the time the
/// CPU actually holds, and on a compute-bound run that is a rounding error. Measuring the ceiling first
/// costs one division and can save a week.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Headroom {
    /// Best achievable speedup if ALL CPU setup and ALL sync stalls vanished.
    pub ceiling: f32,
    pub cpu_share: f32,
    pub sync_share: f32,
    pub submits_per_token: u64,
}

impl Headroom {
    /// Below this, a change is not worth making: run-to-run variance on this class of hardware is a few
    /// percent, so a 1.05x ceiling cannot be distinguished from noise even if perfectly realised.
    pub const WORTH_DOING: f32 = 1.15;

    pub fn is_worth_optimising(&self) -> bool { self.ceiling >= Self::WORTH_DOING }

    /// Where the recoverable time actually is — which decides WHICH change to make.
    pub fn verdict(&self) -> String {
        if !self.is_worth_optimising() {
            return format!("compute-bound: CPU holds {:.1}% and sync {:.1}%, so removing ALL of it gives {:.2}x. Caching, batching and submit-count work cannot help here — the GPU is the wall.",
                self.cpu_share * 100.0, self.sync_share * 100.0, self.ceiling);
        }
        if self.sync_share > self.cpu_share * 4.0 {
            format!("sync-bound: {:.0}% of the step is the CPU WAITING on the GPU ({} submits/token). The win is removing readbacks, not caching — a bind-group cache does not shorten a stall. Ceiling {:.2}x.",
                self.sync_share * 100.0, self.submits_per_token, self.ceiling)
        } else {
            format!("dispatch-bound: {:.0}% is CPU setup at {} submits/token. Caching and batching are the right tools here. Ceiling {:.2}x.", self.cpu_share * 100.0, self.submits_per_token, self.ceiling)
        }
    }
}

/// Compute the headroom from one generation's measurements.
pub fn headroom(wall_ns: u64, setup_ns: u64, sync_ns: u64, tokens: u64, submits: u64) -> Option<Headroom> {
    if wall_ns == 0 { return None; }
    let overhead = setup_ns.saturating_add(sync_ns).min(wall_ns);
    let remaining = wall_ns - overhead;
    // All overhead removed and the GPU work unchanged: that is the bound, and it is not reachable.
    let ceiling = if remaining == 0 { f32::INFINITY } else { wall_ns as f32 / remaining as f32 };
    Some(Headroom {
        ceiling,
        cpu_share: setup_ns as f32 / wall_ns as f32,
        sync_share: sync_ns as f32 / wall_ns as f32,
        submits_per_token: submits / tokens.max(1),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tl() -> Timeline {
        let mut t = Timeline::new(1000);
        // 4 layers, 3 tokens. Layer 2 is a streaming hot spot; every layer pays a router sync.
        for tok in 0..3u32 {
            for l in 0..4u32 {
                t.record(Span { token: tok, layer: l, phase: Phase::Compute, ns: 1_000_000, bytes: 0 });
                t.record(Span { token: tok, layer: l, phase: Phase::Sync, ns: 3_000_000, bytes: 0 });
                if l == 2 {
                    t.record(Span { token: tok, layer: l, phase: Phase::Upload, ns: 20_000_000, bytes: 68 << 20 });
                }
            }
        }
        t
    }

    #[test]
    fn the_dominant_phase_carries_its_remedy() {
        let t = tl();
        let (p, share) = t.dominant().expect("spans recorded");
        // 3 tokens x 1 layer x 20ms upload = 60ms, vs 36ms sync and 12ms compute
        assert_eq!(p, Phase::Upload);
        assert!(share > 0.5, "share {share}");
        // the point of separating phases is that they imply DIFFERENT actions
        assert!(p.remedy().contains("resident capacity"));
        assert!(Phase::Sync.remedy().contains("readback"));
        assert_ne!(Phase::Upload.remedy(), Phase::Sync.remedy());
    }

    #[test]
    fn the_heatmap_localises_the_hot_layer() {
        let t = tl();
        let g = t.heatmap(Phase::Upload, 4, 3);
        assert_eq!(g.len(), 4);
        assert_eq!(g[2], vec![20_000_000; 3], "layer 2 carries all the upload");
        for l in [0usize, 1, 3] { assert_eq!(g[l], vec![0; 3], "layer {l} uploads nothing"); }
        // and hot_layers names it without the caller reading the grid
        let hot = t.hot_layers(Phase::Upload, 4, 2.0);
        assert_eq!(hot.len(), 1);
        assert_eq!(hot[0].0, 2);
    }

    #[test]
    fn a_uniform_phase_has_no_hot_layers() {
        // Sync is even across layers (one router readback each), so nothing should be flagged — a
        // detector that fires on a flat distribution would point at the wrong thing every time.
        let t = tl();
        assert!(t.hot_layers(Phase::Sync, 4, 2.0).is_empty());
    }

    #[test]
    fn the_trace_is_well_formed_chrome_json() {
        let t = tl();
        let j = t.to_chrome_trace();
        assert!(j.starts_with('[') && j.trim_end().ends_with(']'));
        // one event per span, and the required Chrome Trace keys present
        assert_eq!(j.matches("\"ph\":\"X\"").count(), t.spans().len());
        assert!(j.contains("\"cat\":\"upload\"") && j.contains("\"dur\":"));
        assert!(j.contains("\"token\":") && j.contains("\"bytes\":"));
        // durations are MICROseconds, as the format requires: 20ms -> 20000us
        assert!(j.contains("\"dur\":20000"), "{j}");
    }

    #[test]
    fn the_text_heatmap_shows_the_hot_row_and_never_hides_a_faint_one() {
        let mut t = tl();
        // a faint but REAL cell: 1/1000th of the hot one. It must not render as blank, or "quiet" and
        // "absent" become indistinguishable, which is the failure mode of every naive intensity ramp.
        t.record(Span { token: 0, layer: 0, phase: Phase::Upload, ns: 20_000, bytes: 1 });
        let r = t.render_heatmap(Phase::Upload, 4, 3);
        assert!(r[0].contains("rows=layer"));
        let l0 = r.iter().find(|s| s.starts_with("  L0  ")).expect("layer 0 row");
        let l2 = r.iter().find(|s| s.starts_with("  L2  ")).expect("layer 2 row");
        assert!(l2.contains('#'), "the hot layer peaks: {l2}");
        assert!(l0.contains('.'), "a faint cell is visible, not blank: {l0}");
        assert!(l0.contains(' '), "and a truly empty cell IS blank: {l0}");
    }

    #[test]
    fn a_heatmap_for_an_unrecorded_phase_says_so() {
        let t = tl();
        let r = t.render_heatmap(Phase::Convert, 4, 3);
        assert!(r.iter().any(|s| s.contains("nothing recorded")));
    }

    #[test]
    fn a_full_buffer_reports_what_it_dropped() {
        // Silently losing spans would make every percentage a lie, so the count is carried and surfaced.
        let mut t = Timeline::new(2);
        for i in 0..5 { t.record(Span { token: 0, layer: i, phase: Phase::Compute, ns: 1, bytes: 0 }); }
        assert_eq!(t.spans().len(), 2);
        assert_eq!(t.dropped(), 3);
        assert!(t.summary().iter().any(|s| s.contains("3 spans dropped")));
    }

    // The three cases below use REAL measurements from hardware runs, not invented numbers, so the
    // thresholds are calibrated against workloads that actually occur.

    #[test]
    fn a_compute_bound_run_has_no_headroom_and_says_so_plainly() {
        // DeepSeek-R1-Distill-8B: 369 tokens in 75.627 s, setup 179 ms, sync 0 ms, 5395 submits.
        // 99.8% of the step is GPU execution. This is the case where caching work is wasted effort.
        let h = headroom(75_627_000_000, 179_000_000, 0, 369, 5395).unwrap();
        assert!(h.ceiling < 1.01, "ceiling {:.3}", h.ceiling);
        assert!(!h.is_worth_optimising());
        let v = h.verdict();
        assert!(v.contains("compute-bound") && v.contains("cannot help here"), "{v}");
    }

    #[test]
    fn a_sync_bound_run_points_at_readbacks_not_caching() {
        // Llama-4 Scout: 4 tokens in 3.792 s, setup 7 ms, sync 3.025 s, 4604 submits.
        let h = headroom(3_792_000_000, 7_000_000, 3_025_000_000, 4, 4604).unwrap();
        assert!(h.ceiling > 4.0, "5x on the table: {:.2}", h.ceiling);
        assert!(h.is_worth_optimising());
        let v = h.verdict();
        assert!(v.contains("sync-bound"), "{v}");
        assert!(v.contains("does not shorten a stall"),
            "the point is that the OBVIOUS fix is the wrong one here");
        assert_eq!(h.submits_per_token, 1151);
    }

    #[test]
    fn a_marginal_run_is_left_alone() {
        // Qwen3-30B: 10 tokens in 3.106 s, setup 12 ms, sync 340 ms. 1.13x is below the threshold —
        // run-to-run variance on this hardware is a few percent, so even a perfect fix is unmeasurable.
        let h = headroom(3_106_000_000, 12_000_000, 340_000_000, 10, 310).unwrap();
        assert!(h.ceiling < Headroom::WORTH_DOING, "ceiling {:.2}", h.ceiling);
        assert!(!h.is_worth_optimising());
    }

    #[test]
    fn zero_wall_time_yields_no_verdict_rather_than_infinity() {
        assert!(headroom(0, 0, 0, 1, 1).is_none());
        // and overhead exceeding the wall clock (clock skew between counters) must not go negative
        let h = headroom(1_000_000, 900_000, 900_000, 1, 1).unwrap();
        assert!(h.ceiling.is_infinite() || h.ceiling > 1.0);
    }

    #[test]
    fn an_empty_timeline_says_so_rather_than_dividing_by_zero() {
        let t = Timeline::new(10);
        assert!(t.is_empty());
        assert!(t.dominant().is_none());
        assert_eq!(t.summary(), vec!["no spans recorded".to_string()]);
        assert!(t.hot_layers(Phase::Upload, 4, 2.0).is_empty());
    }
}
