//! **Desync monitor** — two facets of watching sync health:
//!
//!  1. [`Scrutiny`] — a per-entity/peer/region adaptive cadence controller. It
//!     escalates toward tight checking when a peer causes problems and decays
//!     toward relaxed checking when it stays in sync, so you don't spam
//!     always-synced peers and you focus effort on troublemakers. **The relaxed
//!     end is hard-capped at the rollback window**: the cheap digest compare
//!     never backs off past the horizon you could still roll back to (the rail
//!     that prevents the lazy-merge failure). [`Scrutiny::wants_deep_audit`]
//!     gates the *expensive* work (full audits, recruiting validators, alerting)
//!     behind high scrutiny — that's what actually goes quiet for synced peers,
//!     while the one-`u64` compare keeps ticking for correctness.
//!
//!  2. [`DesyncEvent`] + [`annotate_frametime`] — fold desync/resim cost into the
//!     frametime series so the dashboard can mark *when* desyncs happened and how
//!     much resim they cost, right on the frametime chart. [`DesyncEvent::event_line`]
//!     emits the `events.jsonl` record the dashboard ingests.

use crate::sim::gpu_bench::{bench, BenchResult};

/// Tunables for the adaptive cadence controller.
#[derive(Clone, Copy, Debug)]
pub struct ScrutinyCfg {
    /// Scrutiny added per problem observation.
    pub escalate: f32,
    /// Scrutiny removed per clean observation (set `< escalate` for hysteresis).
    pub decay: f32,
    /// Tightest check interval (ticks) at full scrutiny.
    pub min_tick: u32,
    /// Loosest check interval (ticks) at zero scrutiny — the rollback horizon.
    /// The compare cadence is never allowed past this.
    pub rollback_window: u32,
    /// Scrutiny level at/above which expensive deep audits switch on.
    pub audit_threshold: f32,
}

impl Default for ScrutinyCfg {
    fn default() -> Self {
        ScrutinyCfg { escalate: 0.5, decay: 0.1, min_tick: 1, rollback_window: 8, audit_threshold: 0.6 }
    }
}

/// Adaptive scrutiny state for one peer/entity/region.
#[derive(Clone, Copy, Debug)]
pub struct Scrutiny {
    /// 0 = fully trusted (loosest cadence), 1 = max scrutiny (tightest).
    pub level: f32,
}

impl Scrutiny {
    pub fn new(level: f32) -> Self {
        Scrutiny { level: level.clamp(0.0, 1.0) }
    }

    /// Update after a verdict: `problem` = a Contested verdict or suspect flag.
    pub fn observe(&mut self, problem: bool, cfg: &ScrutinyCfg) {
        let delta = if problem { cfg.escalate } else { -cfg.decay };
        self.level = (self.level + delta).clamp(0.0, 1.0);
    }

    /// Cheap-compare interval in ticks: tight at high scrutiny, loose at low —
    /// clamped to `[min_tick, rollback_window]`, so it never relaxes past the
    /// horizon you could still roll back to.
    pub fn interval(&self, cfg: &ScrutinyCfg) -> u32 {
        let rw = cfg.rollback_window as f32;
        let mt = cfg.min_tick as f32;
        let iv = (rw - self.level * (rw - mt)).round();
        iv.clamp(mt, rw) as u32
    }

    /// Whether to run the *expensive* validation (deep audit / validator
    /// recruitment / alerting) — only for troublemakers, so synced peers stay
    /// quiet.
    pub fn wants_deep_audit(&self, cfg: &ScrutinyCfg) -> bool {
        self.level >= cfg.audit_threshold
    }
}

/// What kind of sync problem occurred.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DesyncKind {
    /// Witnesses split with no quorum.
    Contested,
    /// A minority peer disagreed with the quorum.
    Suspect,
    /// A hard resync (snapshot transfer) was required.
    Resync,
}

impl DesyncKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            DesyncKind::Contested => "contested",
            DesyncKind::Suspect => "suspect",
            DesyncKind::Resync => "resync",
        }
    }
}

/// A logged desync, with the resim cost it imposed on the frame.
#[derive(Clone, Debug)]
pub struct DesyncEvent {
    pub tick: u64,
    pub region: u64,
    /// How far back the rollback reached (ticks).
    pub rollback_ticks: u32,
    /// Milliseconds the rollback+resim added to this frame.
    pub resim_ms: f32,
    pub kind: DesyncKind,
    pub suspects: Vec<u64>,
}

impl DesyncEvent {
    /// `events.jsonl`-style record for the dashboard.
    pub fn event_line(&self) -> String {
        let sus = self.suspects.iter().map(|s| s.to_string()).collect::<Vec<_>>().join(",");
        format!(
            "{{\"type\":\"desync\",\"tick\":{},\"region\":{},\"kind\":\"{}\",\"rollback\":{},\"resim_ms\":{:.3},\"suspects\":[{}]}}",
            self.tick, self.region, self.kind.as_str(), self.rollback_ticks, self.resim_ms, sus
        )
    }
}

/// One point of the annotated frametime series.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FramePoint {
    pub tick: u64,
    /// Baseline frame time (ms).
    pub frame_ms: f32,
    /// Resim cost attributed to desyncs on this frame (ms) — drawn as an overlay.
    pub resim_ms: f32,
    /// Whether a desync was logged on this frame (chart marker).
    pub desync: bool,
}

impl FramePoint {
    /// Total wall time the frame actually took (baseline + resim).
    pub fn total_ms(&self) -> f32 {
        self.frame_ms + self.resim_ms
    }
}

/// Fold desync events into a frametime series. `base_ms[i]` is the baseline cost
/// of frame `start_tick + i`; each event adds its `resim_ms` to its frame and
/// marks it. Events outside the range are ignored.
pub fn annotate_frametime(base_ms: &[f32], start_tick: u64, events: &[DesyncEvent]) -> Vec<FramePoint> {
    let mut out: Vec<FramePoint> = base_ms
        .iter()
        .enumerate()
        .map(|(i, &ms)| FramePoint { tick: start_tick + i as u64, frame_ms: ms, resim_ms: 0.0, desync: false })
        .collect();
    for e in events {
        if e.tick < start_tick {
            continue;
        }
        let idx = (e.tick - start_tick) as usize;
        if idx < out.len() {
            out[idx].resim_ms += e.resim_ms;
            out[idx].desync = true;
        }
    }
    out
}

/// Benchmark the annotation over `iters` runs (consistency with the harness).
pub fn bench_annotate(base_ms: &[f32], start_tick: u64, events: &[DesyncEvent], iters: usize) -> BenchResult {
    bench(iters, || {
        let v = annotate_frametime(base_ms, start_tick, events);
        std::hint::black_box(&v);
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> ScrutinyCfg {
        ScrutinyCfg::default()
    }

    #[test]
    fn escalates_on_problems_to_tight_cadence() {
        let mut s = Scrutiny::new(0.25);
        for _ in 0..10 {
            s.observe(true, &cfg());
        }
        assert!((s.level - 1.0).abs() < 1e-6);
        assert_eq!(s.interval(&cfg()), cfg().min_tick);
        assert!(s.wants_deep_audit(&cfg()));
    }

    #[test]
    fn backs_off_clean_peer_to_window_cap() {
        let mut s = Scrutiny::new(1.0);
        for _ in 0..30 {
            s.observe(false, &cfg());
        }
        assert!(s.level.abs() < 1e-6);
        assert_eq!(s.interval(&cfg()), cfg().rollback_window, "loosest is the rollback window, never beyond");
        assert!(!s.wants_deep_audit(&cfg()));
    }

    #[test]
    fn interval_monotone_and_capped() {
        let c = ScrutinyCfg { min_tick: 2, rollback_window: 16, ..cfg() };
        let mut prev = u32::MAX;
        for k in 0..=100 {
            let s = Scrutiny::new(k as f32 / 100.0);
            let iv = s.interval(&c);
            assert!(iv >= 2 && iv <= 16, "interval {iv} outside cap");
            assert!(iv <= prev, "interval rose with scrutiny");
            prev = iv;
        }
    }

    #[test]
    fn focuses_on_troublemakers_with_hysteresis() {
        let mut clean = Scrutiny::new(0.25);
        clean.observe(false, &cfg());
        let mut trouble = Scrutiny::new(0.25);
        trouble.observe(true, &cfg());
        assert!(trouble.level > clean.level);
        trouble.observe(false, &cfg()); // one clean after a problem
        assert!(trouble.level > clean.level, "should stay elevated (escalate>decay)");
    }

    #[test]
    fn never_exceeds_window_even_fully_trusted() {
        let c = ScrutinyCfg { min_tick: 1, rollback_window: 4, ..cfg() };
        assert_eq!(Scrutiny::new(0.0).interval(&c), 4);
    }

    #[test]
    fn annotation_maps_and_sums() {
        let base = vec![16.6f32; 10];
        let ev = vec![
            DesyncEvent { tick: 103, region: 1, rollback_ticks: 5, resim_ms: 12.0, kind: DesyncKind::Contested, suspects: vec![3] },
            DesyncEvent { tick: 103, region: 1, rollback_ticks: 2, resim_ms: 3.0, kind: DesyncKind::Suspect, suspects: vec![3] },
            DesyncEvent { tick: 999, region: 2, rollback_ticks: 1, resim_ms: 50.0, kind: DesyncKind::Resync, suspects: vec![] },
        ];
        let fr = annotate_frametime(&base, 100, &ev);
        assert!(fr[3].desync && (fr[3].resim_ms - 15.0).abs() < 1e-6);
        assert!((fr[3].total_ms() - (16.6 + 15.0)).abs() < 1e-4);
        assert!(!fr[0].desync && fr[0].resim_ms == 0.0);
        assert_eq!(fr.iter().filter(|f| f.desync).count(), 1, "out-of-range event ignored");
    }

    #[test]
    fn event_line_has_fields() {
        let e = DesyncEvent { tick: 42, region: 7, rollback_ticks: 5, resim_ms: 9.5, kind: DesyncKind::Contested, suspects: vec![2, 4] };
        let line = e.event_line();
        assert!(line.contains("\"type\":\"desync\""));
        assert!(line.contains("\"tick\":42"));
        assert!(line.contains("\"kind\":\"contested\""));
        assert!(line.contains("\"suspects\":[2,4]"));
    }

    #[test]
    fn bench_runs() {
        let base = vec![16.6f32; 64];
        let ev = vec![DesyncEvent { tick: 110, region: 0, rollback_ticks: 4, resim_ms: 8.0, kind: DesyncKind::Suspect, suspects: vec![1] }];
        let br = bench_annotate(&base, 100, &ev, 3);
        assert_eq!(br.samples.len(), 3);
        assert!(br.metric_line("desync_annotate", None).contains("desync_annotate n=3"));
    }
}
