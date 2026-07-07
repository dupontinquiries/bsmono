//! **Player-sum mob sight** — a mob's perception as an *aggregation over players*
//! rather than a contested single value. Instead of clients competing to agree
//! "did the mob see the player," each player contributes a deterministic
//! perception term and the mob's sight is a **reduction** over them:
//!   - alertness = `Σ termᵢ` (more intruders ⇒ more noticeable)
//!   - target    = `argmax termᵢ` (the most-visible player; ties → lowest id)
//!
//! Because sum and max are commutative + associative, and the terms are keyed by
//! player (idempotent), this is a **CRDT** (a join-semilattice of per-player max
//! registers). That gives three properties consensus can't:
//!   - order-independent reduction → no iteration-order desync;
//!   - no separate consensus round for mobs — each term is a deterministic
//!     function of that player's already-agreed canonical position, so the mob is
//!     a *derived view*, not a contested entity;
//!   - a peer with only its area-of-interest players holds a partial field that
//!     converges monotonically to the true value as it [`merge`](SightField::merge)s
//!     more — interest management without a quorum.
//!
//! Determinism constraint: terms must be the **quantized/fixed-point** perceive
//! ([`quantize`] bridges the f32 [`crate::sim::perception_fog::perceive`]), never
//! the raw f32 — same render/sim boundary as everywhere else.

use crate::sim::gpu_bench::{bench, BenchResult};
use std::collections::BTreeMap;

pub type PlayerId = u64;

/// Quantization scale for perception terms (perception ∈ [0,1] → 0..=QSCALE).
pub const QSCALE: u32 = 1000;

/// Quantize an f32 perception into a deterministic integer term.
#[inline]
pub fn quantize(perception: f32) -> u32 {
    (perception.clamp(0.0, 1.0) * QSCALE as f32).round() as u32
}

/// A mob's player-keyed perception field: a join-semilattice (per-key max),
/// reduced by sum (alertness) and argmax (target). `BTreeMap` keeps iteration —
/// and therefore argmax tie-breaking — canonical.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SightField {
    terms: BTreeMap<PlayerId, u32>,
}

impl SightField {
    pub fn new() -> Self {
        SightField::default()
    }

    /// Record `player`'s perception term, keeping the max (the join). Idempotent:
    /// re-observing the same value is a no-op.
    pub fn observe(&mut self, player: PlayerId, term: u32) {
        let e = self.terms.entry(player).or_insert(0);
        if term > *e {
            *e = term;
        }
    }

    /// Merge another field in (per-key max). Commutative, associative, idempotent.
    pub fn merge(&mut self, other: &SightField) {
        for (&p, &t) in &other.terms {
            self.observe(p, t);
        }
    }

    /// Total stimulus — the alertness reduction (saturating).
    pub fn total_stimulus(&self) -> u32 {
        self.terms.values().fold(0u32, |a, &t| a.saturating_add(t))
    }

    /// The most-visible player and its term; ties resolve to the lowest player id.
    pub fn dominant(&self) -> Option<(PlayerId, u32)> {
        let mut best: Option<(PlayerId, u32)> = None;
        for (&p, &t) in &self.terms {
            match best {
                Some((_, bt)) if t <= bt => {}
                _ => best = Some((p, t)),
            }
        }
        best
    }

    /// Alerted if total stimulus clears a deterministic threshold. (Dither with
    /// `perception_fog::stable_noise` keyed on the mob if you want a graded
    /// probability instead of a hard cut.)
    pub fn alerted(&self, threshold: u32) -> bool {
        self.total_stimulus() > threshold
    }

    pub fn len(&self) -> usize {
        self.terms.len()
    }

    pub fn is_empty(&self) -> bool {
        self.terms.is_empty()
    }
}

/// Benchmark building + reducing a field over `iters` runs.
pub fn bench_sightfield(terms: &[(PlayerId, u32)], iters: usize) -> BenchResult {
    bench(iters, || {
        let mut f = SightField::new();
        for &(p, t) in terms {
            f.observe(p, t);
        }
        std::hint::black_box((f.total_stimulus(), f.dominant()));
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn field(pairs: &[(PlayerId, u32)]) -> SightField {
        let mut f = SightField::new();
        for &(p, t) in pairs {
            f.observe(p, t);
        }
        f
    }

    #[test]
    fn total_is_order_independent() {
        let a = field(&[(1, 100), (2, 200), (3, 50)]);
        let b = field(&[(3, 50), (1, 100), (2, 200)]);
        assert_eq!(a.total_stimulus(), b.total_stimulus());
        assert_eq!(a, b);
    }

    #[test]
    fn merge_is_commutative_associative_idempotent() {
        let a = field(&[(1, 10), (2, 20)]);
        let b = field(&[(2, 30), (3, 5)]);
        let c = field(&[(3, 50), (4, 1)]);
        let mut ab = a.clone();
        ab.merge(&b);
        let mut ba = b.clone();
        ba.merge(&a);
        assert_eq!(ab, ba, "commutative");
        let mut left = ab.clone();
        left.merge(&c);
        let mut bc = b.clone();
        bc.merge(&c);
        let mut right = a.clone();
        right.merge(&bc);
        assert_eq!(left, right, "associative");
        let mut idem = a.clone();
        idem.merge(&a);
        assert_eq!(idem, a, "idempotent");
    }

    #[test]
    fn partial_views_converge_to_full() {
        let full = field(&[(0, 100), (1, 200), (2, 300), (3, 400)]);
        let mut v1 = field(&[(0, 100), (1, 200)]);
        let v2 = field(&[(2, 300), (3, 400)]);
        assert!(v1.total_stimulus() <= full.total_stimulus());
        let t0 = v1.total_stimulus();
        v1.merge(&v2);
        assert!(v1.total_stimulus() >= t0, "monotone");
        assert_eq!(v1, full, "converges to full");
    }

    #[test]
    fn dominant_tie_breaks_to_lowest_id() {
        let f = field(&[(5, 900), (2, 900), (8, 300)]);
        assert_eq!(f.dominant(), Some((2, 900)));
    }

    #[test]
    fn alertness_scales_with_intruders() {
        assert!(!field(&[(1, 100)]).alerted(500));
        assert!(field(&[(1, 400), (2, 400), (3, 400)]).alerted(500));
    }

    #[test]
    fn empty_is_safe() {
        let f = SightField::new();
        assert_eq!(f.total_stimulus(), 0);
        assert_eq!(f.dominant(), None);
    }

    #[test]
    fn quantize_bridges_perception() {
        assert_eq!(quantize(0.0), 0);
        assert_eq!(quantize(1.0), QSCALE);
        assert_eq!(quantize(0.5), QSCALE / 2);
        assert_eq!(quantize(2.0), QSCALE); // clamped
    }

    #[test]
    fn bench_runs() {
        let br = bench_sightfield(&[(1, 100), (2, 200), (3, 300)], 3);
        assert_eq!(br.samples.len(), 3);
        assert!(br.metric_line("sight_field", None).contains("sight_field n=3"));
    }
}
