//! `sim::sim_scheme` — the adaptive scheme selector for traversing a deep-time
//! span: decide whether to just simulate it, jump it with one sim vector, or
//! drive cheaply then sim-prefill the landing.
//!
//! Background: `docs/design/adaptive_scheme_selection.md` +
//! `vector_chaining_and_timewarp.md`. A sim vector has a *fidelity length* =
//! `min` over its modelled systems' [`ConvergenceClass::fidelity_horizon`]s — the
//! most path-dependent active system caps how far one vector stays accurate. The
//! selector then picks the cheapest scheme that meets tolerance:
//!   * tiny span        → just SIM it (no vector overhead);
//!   * within one vector → a SINGLE vector;
//!   * large span        → DRIVE cheaply to just before the target, then
//!     SIM-PREFILL exactly into it (so building/growth is materialised, not
//!     estimated) — and if the drive itself exceeds `VECTOR_MAX_SPAN`, it CHAINS.
//!
//! The whole decision is a pure, deterministic function of (span, the active
//! classes, the thresholds) so every machine picks the same scheme — the choice
//! is committable into the sim vector. No clock/RNG/IO, no panics.

use crate::sim::convergence::ConvergenceClass;

/// How to traverse the requested span. Ticks throughout.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scheme {
    /// Span small enough that exact tick-by-tick is both simplest and cheapest;
    /// the vector machinery's setup isn't worth it.
    SimIt,
    /// One committed sim vector covers the whole span within tolerance.
    SingleVector,
    /// Drive cheaply (one or more chained vectors) across the bulk to
    /// `drive_to`, then sim exactly for `prefill_ticks` into the target so the
    /// landing's building/growth is materialised. `drive_chained` is true when
    /// the drive span exceeds `VECTOR_MAX_SPAN` and must be split into a chain.
    DriveThenPrefill {
        /// Tick to drive (approximately) up to before the exact prefill begins.
        drive_to: u64,
        /// Exact ticks simulated into the target (the landing window).
        prefill_ticks: u64,
        /// Whether the drive phase needs >1 vector (chaining + convergence merge).
        drive_chained: bool,
    },
}

/// The fidelity length of a vector modelling `classes`: the `min` over each
/// class's [`ConvergenceClass::fidelity_horizon`]. An all-`Procedural`/empty set
/// has unbounded length (`u64::MAX`); any `PathDependent` member drives it to `0`
/// (no free horizon — must be simulated). This is the "most path-dependent active
/// system caps the vector length" rule.
pub fn fidelity_len(classes: &[ConvergenceClass]) -> u64 {
    classes
        .iter()
        .map(|c| c.fidelity_horizon())
        .min()
        .unwrap_or(u64::MAX)
}

/// Choose the cheapest scheme that meets tolerance for a `span`-tick jump over a
/// region whose active systems have the given `classes`.
///
/// - `span`            — ticks from now to the target.
/// - `classes`         — the convergence classes of the systems active here.
/// - `sim_it_threshold`— spans at or below this just SIM (no vector overhead).
/// - `prefill_ticks`   — exact ticks needed before the target so building/growth
///   lands materialised (the slowest such system's requirement); clamped to span.
/// - `vector_max_span` — hard per-vector cap; a drive longer than this chains.
pub fn choose_scheme(
    span: u64,
    classes: &[ConvergenceClass],
    sim_it_threshold: u64,
    prefill_ticks: u64,
    vector_max_span: u64,
) -> Scheme {
    if span <= sim_it_threshold {
        return Scheme::SimIt; // A
    }
    let flen = fidelity_len(classes);
    if span <= flen {
        return Scheme::SingleVector; // B
    }
    // C — drive then prefill. The prefill can't exceed the span; drive covers the
    // rest. If the drive span is longer than one vector's cap, it must chain.
    let plen = prefill_ticks.min(span);
    let drive_to = span - plen;
    let drive_chained = vector_max_span > 0 && drive_to > vector_max_span;
    Scheme::DriveThenPrefill {
        drive_to,
        prefill_ticks: plen,
        drive_chained,
    }
}

/// How many vectors a `drive_span`-tick drive needs at a per-vector cap of
/// `vector_max_span` (the chain length before convergence-merge collapses
/// quiescent links). `0` cap or `0` span ⇒ 1 (a single vector / nothing to do).
pub fn chain_len(drive_span: u64, vector_max_span: u64) -> u64 {
    if vector_max_span == 0 || drive_span == 0 {
        return 1;
    }
    // ceil(drive_span / vector_max_span)
    drive_span.div_ceil(vector_max_span)
}

#[cfg(test)]
mod tests {
    use super::*;

    const PROC: ConvergenceClass = ConvergenceClass::Procedural;
    const PATH: ConvergenceClass = ConvergenceClass::PathDependent;
    fn conv(h: u64) -> ConvergenceClass {
        ConvergenceClass::Convergent { horizon_ticks: h }
    }

    #[test]
    fn fidelity_len_is_min_horizon() {
        assert_eq!(fidelity_len(&[PROC, conv(100), conv(50)]), 50);
        assert_eq!(fidelity_len(&[PROC]), u64::MAX);
        assert_eq!(fidelity_len(&[]), u64::MAX);
        assert_eq!(fidelity_len(&[conv(100), PATH]), 0); // path-dependent → 0
    }

    #[test]
    fn tiny_span_just_sims() {
        assert_eq!(choose_scheme(5, &[conv(100)], 10, 4, 1000), Scheme::SimIt);
    }

    #[test]
    fn medium_span_single_vector() {
        // span 80 ≤ flen 100 → one vector.
        assert_eq!(
            choose_scheme(80, &[conv(100)], 10, 4, 1000),
            Scheme::SingleVector
        );
    }

    #[test]
    fn large_span_drives_then_prefills() {
        // span 500 > flen 100 → drive to 500-8=492, prefill 8, no chain (492<1000).
        assert_eq!(
            choose_scheme(500, &[conv(100)], 10, 8, 1000),
            Scheme::DriveThenPrefill {
                drive_to: 492,
                prefill_ticks: 8,
                drive_chained: false,
            }
        );
    }

    #[test]
    fn very_large_span_chains_the_drive() {
        // span 5000, flen 100, prefill 50, cap 1000 → drive_to 4950 > 1000 → chained.
        match choose_scheme(5000, &[conv(100)], 10, 50, 1000) {
            Scheme::DriveThenPrefill {
                drive_to,
                prefill_ticks,
                drive_chained,
            } => {
                assert_eq!(drive_to, 4950);
                assert_eq!(prefill_ticks, 50);
                assert!(drive_chained);
            }
            other => panic!("expected DriveThenPrefill, got {other:?}"),
        }
    }

    #[test]
    fn path_dependent_forces_vectoring_even_for_small_spans() {
        // a PathDependent system → flen 0 → any span above the sim-it threshold
        // can't be a single vector; it drives+prefills.
        let s = choose_scheme(50, &[PATH], 10, 5, 1000);
        assert!(matches!(s, Scheme::DriveThenPrefill { .. }));
    }

    #[test]
    fn prefill_clamped_to_span() {
        // prefill 9999 but span 30 (> threshold 10) → prefill = 30, drive_to = 0.
        assert_eq!(
            choose_scheme(30, &[PATH], 10, 9999, 1000),
            Scheme::DriveThenPrefill {
                drive_to: 0,
                prefill_ticks: 30,
                drive_chained: false,
            }
        );
    }

    #[test]
    fn chain_len_is_ceil_div() {
        assert_eq!(chain_len(5000, 1000), 5);
        assert_eq!(chain_len(5001, 1000), 6);
        assert_eq!(chain_len(1000, 1000), 1);
        assert_eq!(chain_len(0, 1000), 1);
        assert_eq!(chain_len(500, 0), 1); // no cap → single vector
    }

    #[test]
    fn determinism() {
        let a = choose_scheme(5000, &[conv(100), PROC], 10, 50, 1000);
        let b = choose_scheme(5000, &[conv(100), PROC], 10, 50, 1000);
        assert_eq!(a, b);
    }
}
