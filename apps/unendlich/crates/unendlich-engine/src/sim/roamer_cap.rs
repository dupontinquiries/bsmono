//! `sim::roamer_cap` — roamer **spawn-admission** cap (WID r330-mobs).
//!
//! Grounds `NEXT_SESSION_GUIDE.md §4d` ("roamer **overpopulation** … saw 73 vs
//! cap 8") as a **pure admission gate the spawn director consults so spawning
//! never pushes population past the configured cap**. It answers one question —
//! *given the live count and the cap, how many of a requested batch may I spawn?*
//! — and a diagnostic — *by how much am I currently over?*.
//!
//! **Spawn-side ONLY — this module never despawns or evicts.** Per the r300
//! ledger (`docs/WIRING_STATUS.md`), the operator **declined forced population
//! caps / eviction**: the herd stays intact and per-creature work was made
//! cheaper instead (graze gate, disposition cache). So an over-cap herd is
//! corrected by *admitting nothing further* — [`admit`] returns `0` while
//! `current >= cap` — and the population settles back down naturally as
//! creatures die or stream out. There is no removal path here by construction:
//! every public fn returns a non-negative **spawn** count or a read-only
//! diagnostic, and none of them can shrink the live population.
//!
//! Consistency: [`admit`] is the cap-only specialization of the existing
//! spawn-budget rule `crate::sim::batch_spawn_budget::allowed_one`
//! (`min(pending, budget, cap - active)`) with `budget` unbounded — i.e.
//! `admit(current, cap, requested) == allowed_one(requested, u32::MAX, current,
//! cap)`. Keeping it derivable from that rule means the global roamer gate and
//! the per-kind director clamp can't disagree.
//!
//! Pure: plain `u32` in, `u32` out. No allocation (it's on the spawn path), no
//! IO/clock/RNG, no `.unwrap()`/`.expect()`/`panic!`; **saturating** arithmetic
//! throughout, so no overflow path exists even at `u32::MAX`. Release-safe.

/// Headroom under the cap: how many more may exist before the cap is reached.
///
/// `cap.saturating_sub(current)` — `0` once `current >= cap`. Pure, alloc-free.
/// This is the quantity [`admit`] clamps a request against.
#[inline]
pub fn headroom(current: u32, cap: u32) -> u32 {
    cap.saturating_sub(current)
}

/// How many of `requested` new roamers the spawn gate may admit so that the
/// resulting population never exceeds `cap`.
///
/// Returns `min(requested, cap - current)`:
/// - `0` when `current >= cap` (already at/over the cap — admit nothing; the
///   herd settles down on its own, we never remove anyone);
/// - the full `requested` when there is room for all of them;
/// - otherwise exactly the headroom, so `current + admit(..) == cap`.
///
/// Guarantees (see tests): `admit(c, cap, r) <= r`, `admit(c, cap, r) <=
/// headroom(c, cap)`, and therefore `c + admit(c, cap, r) <= cap` for all
/// inputs (saturating; correct even when `current > cap` or near `u32::MAX`).
/// Alloc-free and deterministic — safe to call per spawn tick.
#[inline]
pub fn admit(current: u32, cap: u32, requested: u32) -> u32 {
    requested.min(headroom(current, cap))
}

/// **Diagnostic only** — how far the live population is *over* the cap right
/// now (`current.saturating_sub(cap)`; `0` when at or under).
///
/// This exists to *log the stutter cause* (e.g. "[roamer-cap] 73 > cap 8,
/// over by 65; admitting 0 until it settles"). It is **NOT a despawn list and
/// must never drive eviction** — see the module header and the r300 decision.
/// Pure, alloc-free, read-only.
#[inline]
pub fn overflow_count(current: u32, cap: u32) -> u32 {
    current.saturating_sub(cap)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Core invariant over a deterministic fuzz: admitting can never push the
    /// resulting population past the cap, for any (current, cap, requested) —
    /// including current already over cap and values near `u32::MAX`.
    #[test]
    fn admit_never_exceeds_cap() {
        let mut x: u64 = 0xD06F_ACE5;
        let mut next = || {
            x = x.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            (x >> 32) as u32
        };
        // include the boundary values explicitly alongside random draws.
        let edges = [0u32, 1, 2, 7, 8, 73, u32::MAX - 1, u32::MAX];
        for _ in 0..50_000 {
            let current = if next() & 1 == 0 { edges[(next() as usize) % edges.len()] } else { next() };
            let cap = if next() & 1 == 0 { edges[(next() as usize) % edges.len()] } else { next() };
            let requested = if next() & 1 == 0 { edges[(next() as usize) % edges.len()] } else { next() };

            let a = admit(current, cap, requested);
            // never admits more than asked, never more than the headroom,
            // and the post-spawn population (saturating) never exceeds cap.
            assert!(a <= requested, "admitted {} > requested {}", a, requested);
            assert!(a <= headroom(current, cap));
            assert!(current.saturating_add(a) <= cap.max(current),
                "current {} + admitted {} overshot cap {}", current, a, cap);
            if current < cap {
                assert!(current + a <= cap, "current {} + {} > cap {}", current, a, cap);
            }
        }
    }

    /// A cap of zero admits nothing, whatever is requested.
    #[test]
    fn cap_zero_admits_nothing() {
        for r in [0u32, 1, 5, 100, u32::MAX] {
            assert_eq!(admit(0, 0, r), 0);
            assert_eq!(admit(10, 0, r), 0);
        }
    }

    /// At or over the cap, admit is always 0 — the spawn-side-only behaviour:
    /// we stop spawning, we do not remove anyone.
    #[test]
    fn at_or_over_cap_admits_zero() {
        assert_eq!(admit(8, 8, 5), 0); // exactly at cap
        assert_eq!(admit(9, 8, 5), 0); // one over
        assert_eq!(admit(73, 8, 1000), 0); // the §4d scenario: way over
        // requesting zero is always zero, and a huge request at-cap is still zero.
        assert_eq!(admit(8, 8, 0), 0);
        assert_eq!(admit(100, 8, u32::MAX), 0);
    }

    /// With room to spare, the full request is granted; with partial room it is
    /// clamped exactly to the headroom so the cap is filled but not exceeded.
    #[test]
    fn fills_available_headroom() {
        assert_eq!(admit(3, 8, 2), 2); // room for 5, asked 2 → 2
        assert_eq!(admit(0, 8, 8), 8); // empty, fill to cap
        assert_eq!(admit(6, 8, 5), 2); // headroom 2, asked 5 → 2 (current+2==cap)
        assert_eq!(admit(6, 8, 5) + 6, 8);
        assert_eq!(admit(7, 8, 100), 1); // last slot
    }

    /// Saturating arithmetic: no overflow/panic even at the integer ceiling.
    #[test]
    fn saturating_no_overflow() {
        assert_eq!(admit(u32::MAX, u32::MAX, 10), 0); // headroom 0
        assert_eq!(admit(0, u32::MAX, u32::MAX), u32::MAX); // whole range fits
        assert_eq!(headroom(5, u32::MAX), u32::MAX - 5);
        assert_eq!(overflow_count(u32::MAX, 0), u32::MAX);
        assert_eq!(headroom(u32::MAX, 3), 0);
    }

    /// `overflow_count` is a pure read-only diagnostic: positive only when over,
    /// 0 at/under, and it equals current-cap when over.
    #[test]
    fn overflow_count_is_pure_diagnostic() {
        assert_eq!(overflow_count(73, 8), 65); // §4d
        assert_eq!(overflow_count(8, 8), 0); // exactly at cap → not "over"
        assert_eq!(overflow_count(3, 8), 0); // under
        assert_eq!(overflow_count(0, 0), 0);
        // pure: repeated calls never change anything, same answer every time.
        for _ in 0..3 {
            assert_eq!(overflow_count(50, 8), 42);
        }
    }

    /// `admit` is exactly the cap-only spawn-budget rule, and is deterministic.
    #[test]
    fn matches_spawn_budget_rule_and_deterministic() {
        // allowed_one(pending, budget, active, cap) = min(pending, budget, cap-active);
        // with budget unbounded this is admit(active=current, cap, pending=requested).
        let reference = |requested: u32, current: u32, cap: u32| -> u32 {
            requested.min(u32::MAX).min(cap.saturating_sub(current))
        };
        let mut x: u64 = 0x5A11_0076;
        let mut next = || { x = x.wrapping_mul(6364136223846793005).wrapping_add(1); (x >> 33) as u32 };
        for _ in 0..10_000 {
            let (c, cap, r) = (next() % 200, next() % 200, next() % 200);
            assert_eq!(admit(c, cap, r), reference(r, c, cap));
            assert_eq!(admit(c, cap, r), admit(c, cap, r)); // deterministic
        }
    }

    /// Belt-and-braces: nothing here can shrink a population — every fn returns
    /// a count `>= 0` and `admit` never exceeds the request, so a caller can
    /// only ever *add* `admit(..)` creatures (or zero). There is no despawn API.
    #[test]
    fn never_implies_removal() {
        for &(c, cap, r) in &[(73u32, 8u32, 9u32), (8, 8, 4), (0, 8, 3), (100, 0, 7)] {
            let a = admit(c, cap, r);
            assert!(a <= r, "admit can only admit up to the request, never negative/over");
            // applying the admission only grows or holds the population.
            assert!(c.saturating_add(a) >= c, "population must not shrink");
        }
    }
}
