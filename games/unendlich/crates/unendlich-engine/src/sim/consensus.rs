//! **Region consensus** — quorum agreement over the per-region rollback digests
//! from [`crate::sim::sync_check`], for basic P2P sync + anti-cheat.
//!
//! Each region+frame collects one digest [`Vote`] per witnessing peer (a witness
//! = a peer whose area of interest covers that region). [`tally`] returns a
//! [`Verdict`]:
//!   - [`Verdict::Confirmed`] — a quorum agrees; the minority is rolled back to
//!     the agreed digest and [`suspects`] names them (sync + flag).
//!   - [`Verdict::Unverified`] — too few witnesses to vote (the lone-player
//!     region). Don't pay to validate it continuously; validate **lazily at the
//!     commit boundary** (replay/fraud-proof the inputs when its state enters a
//!     shared context) or recruit a nearby idle peer / the host as a second
//!     simulator *only if the region is high-value*.
//!   - [`Verdict::Contested`] — witnesses split with no quorum: a desync or a
//!     cheat. Surface it, roll the minority to the plurality (if any), flag.
//!
//! ## Strength & the collusion ceiling (be honest about this)
//! Quorum beats a *lone* cheater and catches accidental desync — the "basic"
//! tier. It does **not** beat collusion: a colluding majority in a region wins
//! the vote and the honest peer is mis-flagged (see the test). Real
//! collusion-proofing needs an authoritative host or fraud proofs. Pick
//! `min_agree` for the threat you actually face; `2f+1` witnesses tolerate `f`
//! cheaters.
//!
//! ## Cadence follows stakes
//! [`check_interval`] ties how often you *exchange/compare* digests to the
//! situation, not a fixed number: tight (≈ rollback window) for contested
//! multi-witness regions, loose (≈ a second) for quiet/solo ones. Local hashing
//! is one `u64`; it's the network compare you're rationing.

use crate::sim::gpu_bench::{bench, BenchResult};

pub type PeerId = u64;
pub type RegionId = u64;

/// One peer's digest for a region+frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Vote {
    pub peer: PeerId,
    pub digest: u64,
}

/// Quorum requirements. `min_witnesses` is how many votes are needed to even
/// attempt a verdict; `min_agree` is how many must share the winning digest.
#[derive(Clone, Copy, Debug)]
pub struct QuorumPolicy {
    pub min_witnesses: usize,
    pub min_agree: usize,
}

impl QuorumPolicy {
    /// "At least two peers agree" — the basic tier.
    pub fn two_of_any() -> Self {
        QuorumPolicy { min_witnesses: 2, min_agree: 2 }
    }

    /// Strict majority of `expected` witnesses (tolerates a minority of cheaters).
    pub fn majority_of(expected: usize) -> Self {
        QuorumPolicy { min_witnesses: expected, min_agree: expected / 2 + 1 }
    }
}

/// Outcome of a region+frame vote.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// A quorum agreed on `digest` (`agree` of `total` witnesses).
    Confirmed { digest: u64, agree: usize, total: usize },
    /// Fewer than `min_witnesses` voted — validate lazily / recruit if valuable.
    Unverified { witnesses: usize },
    /// No quorum: factions `(digest, count)` sorted by count desc then digest asc.
    Contested { factions: Vec<(u64, usize)> },
}

impl Verdict {
    /// The agreed digest, if this verdict confirmed one.
    pub fn confirmed_digest(&self) -> Option<u64> {
        match self {
            Verdict::Confirmed { digest, .. } => Some(*digest),
            _ => None,
        }
    }
}

/// Group votes into factions `(digest, count)`, sorted by count desc, digest asc
/// (deterministic — no hashing/iteration-order dependence).
fn factions(votes: &[Vote]) -> Vec<(u64, usize)> {
    let mut f: Vec<(u64, usize)> = Vec::new();
    for v in votes {
        if let Some(slot) = f.iter_mut().find(|s| s.0 == v.digest) {
            slot.1 += 1;
        } else {
            f.push((v.digest, 1));
        }
    }
    f.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    f
}

/// Tally one region+frame's votes into a [`Verdict`]. Expects at most one vote
/// per peer.
pub fn tally(votes: &[Vote], policy: &QuorumPolicy) -> Verdict {
    let total = votes.len();
    if total < policy.min_witnesses {
        return Verdict::Unverified { witnesses: total };
    }
    let f = factions(votes);
    let (top_digest, top_count) = f[0];
    let tie_at_top = f.len() > 1 && f[1].1 == top_count;
    if top_count >= policy.min_agree && !tie_at_top {
        Verdict::Confirmed { digest: top_digest, agree: top_count, total }
    } else {
        Verdict::Contested { factions: f }
    }
}

/// Peers whose digest differs from the canonical (agreed) one — the rollback +
/// flag list.
pub fn suspects(votes: &[Vote], canonical: u64) -> Vec<PeerId> {
    let mut out: Vec<PeerId> = votes.iter().filter(|v| v.digest != canonical).map(|v| v.peer).collect();
    out.sort_unstable();
    out
}

/// Adaptive compare cadence in ticks: tight (`base`) for contested multi-witness
/// regions, mid for quiet multi-witness, loose (`max`) for solo/quiet.
pub fn check_interval(witnesses: usize, contested_recently: bool, base: u32, max: u32) -> u32 {
    if witnesses >= 2 && contested_recently {
        base
    } else if witnesses >= 2 {
        base.max(max / 4)
    } else {
        max
    }
}

/// Benchmark a tally over `iters` runs (consistency with the harness; tiny).
pub fn bench_tally(votes: &[Vote], policy: &QuorumPolicy, iters: usize) -> BenchResult {
    bench(iters, || {
        let v = tally(votes, policy);
        std::hint::black_box(&v);
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(peer: u64, digest: u64) -> Vote {
        Vote { peer, digest }
    }

    #[test]
    fn all_agree_confirmed() {
        let votes = [v(1, 0xA), v(2, 0xA), v(3, 0xA)];
        assert_eq!(tally(&votes, &QuorumPolicy::two_of_any()), Verdict::Confirmed { digest: 0xA, agree: 3, total: 3 });
        assert!(suspects(&votes, 0xA).is_empty());
    }

    #[test]
    fn majority_confirmed_minority_flagged() {
        let votes = [v(1, 0xA), v(2, 0xA), v(3, 0xB)];
        assert_eq!(tally(&votes, &QuorumPolicy::two_of_any()), Verdict::Confirmed { digest: 0xA, agree: 2, total: 3 });
        assert_eq!(suspects(&votes, 0xA), vec![3]);
    }

    #[test]
    fn single_witness_unverified() {
        assert_eq!(tally(&[v(1, 0xA)], &QuorumPolicy::two_of_any()), Verdict::Unverified { witnesses: 1 });
    }

    #[test]
    fn split_is_contested() {
        assert!(matches!(tally(&[v(1, 0xA), v(2, 0xB)], &QuorumPolicy::two_of_any()), Verdict::Contested { .. }));
    }

    #[test]
    fn even_tie_is_contested() {
        let votes = [v(1, 0xA), v(2, 0xA), v(3, 0xB), v(4, 0xB)];
        match tally(&votes, &QuorumPolicy::two_of_any()) {
            Verdict::Contested { factions } => assert_eq!(factions, vec![(0xA, 2), (0xB, 2)]),
            other => panic!("expected Contested, got {other:?}"),
        }
    }

    #[test]
    fn three_one_confirms() {
        let votes = [v(1, 0xA), v(2, 0xA), v(3, 0xA), v(4, 0xB)];
        assert_eq!(tally(&votes, &QuorumPolicy::two_of_any()), Verdict::Confirmed { digest: 0xA, agree: 3, total: 4 });
    }

    #[test]
    fn collusion_majority_wins_known_limit() {
        // two colluding cheaters outvote one honest peer; the honest peer is mis-flagged.
        let votes = [v(1, 0xBAD), v(2, 0xBAD), v(3, 0x600D)];
        let verdict = tally(&votes, &QuorumPolicy::two_of_any());
        assert_eq!(verdict, Verdict::Confirmed { digest: 0xBAD, agree: 2, total: 3 });
        assert_eq!(suspects(&votes, verdict.confirmed_digest().unwrap()), vec![3]);
    }

    #[test]
    fn honest_majority_beats_scattered_cheaters() {
        let votes = [v(1, 0xA), v(2, 0xA), v(3, 0xA), v(4, 0xB), v(5, 0xC)];
        let p = QuorumPolicy { min_witnesses: 2, min_agree: 3 };
        assert_eq!(tally(&votes, &p), Verdict::Confirmed { digest: 0xA, agree: 3, total: 5 });
        assert_eq!(suspects(&votes, 0xA), vec![4, 5]);
    }

    #[test]
    fn adaptive_cadence() {
        assert_eq!(check_interval(3, true, 8, 60), 8); // contested multi-witness → tight
        assert_eq!(check_interval(3, false, 8, 60), 15); // quiet multi-witness → mid
        assert_eq!(check_interval(1, false, 8, 60), 60); // solo → loose
    }

    #[test]
    fn bench_runs() {
        let votes = [v(1, 0xA), v(2, 0xA), v(3, 0xB)];
        let br = bench_tally(&votes, &QuorumPolicy::two_of_any(), 3);
        assert_eq!(br.samples.len(), 3);
        assert!(br.metric_line("consensus_tally", None).contains("consensus_tally n=3"));
    }
}
