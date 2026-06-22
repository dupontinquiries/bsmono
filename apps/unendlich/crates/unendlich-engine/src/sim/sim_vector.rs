//! `sim::sim_vector` — the committed checkpoint-director that carries a region
//! from one deep-time checkpoint to the next. The unit the R2 planner emits and
//! the R1 fill consumes (`docs/design/sim_vectors_and_correction_deltas.md`).
//!
//! A `SimVector` is **committed data in the seed lineage**, not a live decision —
//! a replay re-fills the same vector deterministically rather than re-planning,
//! which is what makes using a (possibly searching) planner determinism-safe. It
//! carries:
//!   * **span** — the ticks it covers;
//!   * **seed key** — `hash(world_seed, region, tick, region_digest)`, so the
//!     statistical fill is conditioned on the actual state (state→seed, never
//!     outcome→seed); built on [`crate::sim::region_digest`];
//!   * **waypoint plan** — committed stage boundaries; the *count* is driven by
//!     the path-dependent (Class-C) channels (`hybrid_step_model.md` §3: you need
//!     enough stages that one exact sub-step lands per meaningful generation),
//!     while convergent channels ride the same fill for free;
//!   * **correction deltas** — sparse per-dimension corrections, applied only to
//!     the dimensions that actually responded to a new condition (§3: don't blur
//!     the axes that never change);
//!   * **event commitments** — conditional committed edits ("dragon still
//!     building → +structure increment"), applied at a checkpoint when a coarse
//!     predicate holds (§2: discrete marks, never statistical blur).
//!
//! Pure & deterministic: no clock/RNG/IO/panic. Fixed-point deltas so it's
//! bit-identical across machines.

use crate::sim::convergence::ConvergenceClass;
use crate::sim::fixed::Fixed;
use crate::sim::region_digest;

/// How many committed stages a path-dependent channel asks for, per channel —
/// the waypoint-density knob. More Class-C channels ⇒ more stages ⇒ the
/// trajectory's shape is preserved (genome/population sampled along the path).
pub const STAGES_PER_PATH_CHANNEL: u32 = 4;

/// A sparse correction applied to one state dimension over this vector's span.
/// `dim` indexes the region's coarse field vector (the same fixed order the
/// digest uses); `delta` is the Fixed-point adjustment. Only dimensions that
/// RESPONDED to a new condition get an entry — everything else rides its
/// convergent baseline untouched.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CorrectionDelta {
    pub dim: u16,
    pub delta: Fixed,
}

/// A conditional committed edit applied at a checkpoint. `kind` tags the edit
/// (e.g. structure-increment, population-stress); `at_tick` is when in the span
/// it applies; `magnitude` is its Fixed-point size. The *condition* is evaluated
/// by the caller against the committed coarse state (a pure predicate); this
/// records the edit that fires when it holds. Discrete + permanent — never blur.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EventCommitment {
    pub kind: u16,
    pub at_tick: u64,
    pub magnitude: Fixed,
}

/// The committed director for one checkpoint→checkpoint hop.
#[derive(Clone, Debug, PartialEq)]
pub struct SimVector {
    /// Tick the vector starts at (the from-checkpoint).
    pub start_tick: u64,
    /// Ticks the vector covers (span). `start_tick + span` is the to-checkpoint.
    pub span: u64,
    /// Region this vector advances.
    pub region: u32,
    /// State-conditioned seed lineage key for the statistical fill.
    pub seed_key: u64,
    /// Committed stage boundaries WITHIN the span (absolute ticks, ascending,
    /// each in `(start_tick, start_tick+span]`). The last is the to-checkpoint.
    /// Committed so a replay re-fills the same stages (exact, no re-planning).
    pub waypoints: Vec<u64>,
    /// Sparse corrections for the dimensions that responded this span.
    pub corrections: Vec<CorrectionDelta>,
    /// Conditional committed edits to apply at checkpoints.
    pub events: Vec<EventCommitment>,
}

/// Waypoint count for a span given how many active channels are path-dependent
/// (Class C). `0` Class-C channels ⇒ 1 stage (the convergent bulk needs no
/// intermediate resolution); each Class-C channel adds [`STAGES_PER_PATH_CHANNEL`]
/// stages, capped so a stage never covers fewer than 1 tick.
pub fn waypoint_count(span: u64, class_c_channels: u32) -> u32 {
    let want = 1 + class_c_channels.saturating_mul(STAGES_PER_PATH_CHANNEL);
    // never more stages than ticks (a stage must cover ≥1 tick)
    let cap = span.max(1).min(u32::MAX as u64) as u32;
    want.min(cap).max(1)
}

/// Count the path-dependent channels in a class set (drives waypoint density).
pub fn class_c_count(classes: &[ConvergenceClass]) -> u32 {
    classes
        .iter()
        .filter(|c| matches!(c, ConvergenceClass::PathDependent))
        .count() as u32
}

impl SimVector {
    /// Plan a committed vector: derive the state-conditioned seed key from the
    /// region's coarse digest, and lay out evenly-spaced committed waypoints
    /// whose count is driven by the path-dependent channels. Corrections + events
    /// start empty (the planner/fill add them).
    ///
    /// `quantized_state` is the region's coarse field vector (already bucketed,
    /// fixed order — see `region_digest`). `classes` are the active systems'
    /// convergence classes.
    pub fn plan(
        world_seed: u64,
        region: u32,
        start_tick: u64,
        span: u64,
        quantized_state: &[i64],
        classes: &[ConvergenceClass],
    ) -> SimVector {
        let digest = region_digest::region_digest(region, start_tick, quantized_state);
        // state-conditioned seed: fold world_seed into the digest (state→seed).
        let seed_key = region_digest::region_digest(
            region,
            start_tick ^ world_seed,
            &[digest as i64, span as i64],
        );
        let n = waypoint_count(span, class_c_count(classes));
        let waypoints = even_waypoints(start_tick, span, n);
        SimVector {
            start_tick,
            span,
            region,
            seed_key,
            waypoints,
            corrections: Vec::new(),
            events: Vec::new(),
        }
    }

    /// The to-checkpoint tick (end of the span).
    pub fn end_tick(&self) -> u64 {
        self.start_tick.saturating_add(self.span)
    }

    /// The deterministic seed for stage `i` of the fill — the per-sub-step PRNG
    /// key, derived from the vector's committed seed key so a replay reproduces
    /// every stage exactly.
    pub fn stage_seed(&self, i: usize) -> u64 {
        region_digest::region_digest(self.region, self.seed_key, &[i as i64])
    }

    /// Record a correction for a responding dimension (sparse).
    pub fn push_correction(&mut self, dim: u16, delta: Fixed) {
        self.corrections.push(CorrectionDelta { dim, delta });
    }

    /// Commit a conditional edit to apply at a checkpoint.
    pub fn push_event(&mut self, kind: u16, at_tick: u64, magnitude: Fixed) {
        self.events.push(EventCommitment {
            kind,
            at_tick,
            magnitude,
        });
    }
}

/// `n` evenly-spaced ascending tick boundaries in `(start, start+span]`, the last
/// being exactly `start+span` (the to-checkpoint). `n==0` is treated as 1.
fn even_waypoints(start: u64, span: u64, n: u32) -> Vec<u64> {
    let n = n.max(1) as u64;
    let mut out = Vec::with_capacity(n as usize);
    for k in 1..=n {
        // tick = start + round(span * k / n)
        let t = start + ((span as u128 * k as u128 + (n as u128 / 2)) / n as u128) as u64;
        out.push(t);
    }
    // guarantee the final boundary is exactly the end (rounding can't drift it,
    // but be explicit) and strictly ascending/deduped.
    if let Some(last) = out.last_mut() {
        *last = start + span;
    }
    out.dedup();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const PATH: ConvergenceClass = ConvergenceClass::PathDependent;
    const PROC: ConvergenceClass = ConvergenceClass::Procedural;
    fn conv(h: u64) -> ConvergenceClass {
        ConvergenceClass::Convergent { horizon_ticks: h }
    }

    #[test]
    fn convergent_only_gets_one_stage() {
        // no path-dependent channels → one stage (convergent bulk, no intermediate
        // resolution needed).
        assert_eq!(waypoint_count(1000, 0), 1);
        assert_eq!(class_c_count(&[PROC, conv(100), conv(50)]), 0);
    }

    #[test]
    fn path_channels_add_stages() {
        // 2 Class-C channels → 1 + 2*4 = 9 stages.
        assert_eq!(class_c_count(&[PATH, conv(100), PATH]), 2);
        assert_eq!(waypoint_count(1000, 2), 9);
    }

    #[test]
    fn stages_never_exceed_ticks() {
        // tiny span can't have more stages than ticks.
        assert_eq!(waypoint_count(3, 5), 3); // wants 21, capped to 3
        assert!(waypoint_count(1, 99) >= 1);
    }

    #[test]
    fn waypoints_span_and_end_exactly() {
        let v = SimVector::plan(0xABCD, 7, 100, 800, &[3, 9], &[PATH, conv(50)]);
        assert_eq!(*v.waypoints.last().unwrap(), 900, "last waypoint = end tick");
        assert_eq!(v.end_tick(), 900);
        // ascending
        for w in v.waypoints.windows(2) {
            assert!(w[1] > w[0], "waypoints strictly ascending");
        }
        // all within (start, end]
        assert!(v.waypoints.iter().all(|&t| t > 100 && t <= 900));
    }

    #[test]
    fn waypoint_count_matches_class_c() {
        // 1 Class-C channel → 1 + 4 = 5 stages.
        let v = SimVector::plan(1, 0, 0, 1000, &[5], &[PATH, conv(10), PROC]);
        assert_eq!(v.waypoints.len(), 5);
    }

    #[test]
    fn seed_key_is_state_conditioned() {
        // same region/tick/span but DIFFERENT coarse state → different seed key
        // (drought vs wet draw different outcomes).
        let dry = SimVector::plan(9, 2, 50, 400, &[1, 1], &[PATH]);
        let wet = SimVector::plan(9, 2, 50, 400, &[8, 7], &[PATH]);
        assert_ne!(dry.seed_key, wet.seed_key);
    }

    #[test]
    fn deterministic_plan_and_stage_seeds() {
        let a = SimVector::plan(7, 3, 10, 500, &[2, 4], &[PATH, conv(20)]);
        let b = SimVector::plan(7, 3, 10, 500, &[2, 4], &[PATH, conv(20)]);
        assert_eq!(a, b);
        assert_eq!(a.stage_seed(0), b.stage_seed(0));
        assert_ne!(a.stage_seed(0), a.stage_seed(1), "stages get distinct seeds");
    }

    #[test]
    fn corrections_and_events_are_sparse_and_committed() {
        let mut v = SimVector::plan(1, 0, 0, 100, &[0], &[conv(10)]);
        assert!(v.corrections.is_empty() && v.events.is_empty());
        v.push_correction(2, Fixed::from_frac(1, 4));
        v.push_event(5, 50, Fixed::from_int(3));
        assert_eq!(v.corrections.len(), 1);
        assert_eq!(v.corrections[0].dim, 2);
        assert_eq!(v.events[0].kind, 5);
        assert_eq!(v.events[0].at_tick, 50);
    }
}
