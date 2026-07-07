//! `sim::essence_refine` — combine essence into higher-tier crafting reagents.
//!
//! This is the step AFTER `salvage_yield` (which extracts essence from items)
//! and BEFORE artifact imbuing (which consumes reagents). It converts raw
//! per-element essence units from an [`EssencePool`] into a single [`Reagent`]
//! of the requested element and tier.
//!
//! ## Cost model
//!
//! Refining element `e` at tier `t` costs:
//! - **Primary:** `cost(t)` units of element `e`.
//! - **Catalyst:** `catalyst_cost(t)` units spread across the two adjacent
//!   elements in the 6-cycle (left and right neighbours, mod 6), split evenly
//!   (floor), with the remainder taken from the left neighbour.
//!
//! If the pool cannot cover both the primary AND catalyst costs, `produced` is
//! `None` and nothing is consumed (the pool is unchanged).
//!
//! ## Quantity conservation
//!
//! `consumed[i] + leftover[i] == pool.units[i]` for all `i`, in every case.
//!
//! ## Purity
//!
//! Purity reflects how much of the pool's target element remained after cost:
//! `purity = (pool.units[e] - primary_cost) / pool.units[e]` if `pool.units[e]
//! > 0`, otherwise `0.0`. Clamped to `[0, 1]` and always finite.

// ---------------------------------------------------------------------------
// Cost constants
// ---------------------------------------------------------------------------

/// Primary cost base (tier 0).
const COST_BASE: u32 = 4;
/// Primary cost multiplier per tier (exponential: COST_BASE * COST_MULT^tier).
/// Using saturating multiply so very high tiers clamp to u32::MAX.
const COST_MULT: u32 = 3;

/// Catalyst cost (total across two neighbours) at tier 0.
const CATALYST_BASE: u32 = 2;
/// Catalyst cost multiplier per tier.
const CATALYST_MULT: u32 = 2;

/// Number of elements in the palette.
const N_ELEM: usize = 6;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Per-element essence pool output of salvage_yield. Index 0=Wood..5=Storm.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EssencePool {
    pub units: [u32; N_ELEM],
}

impl EssencePool {
    /// All-zero pool.
    pub fn empty() -> Self { Self { units: [0; N_ELEM] } }
}

/// A crafting reagent produced by refinement.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Reagent {
    /// Element index (0=Wood, 1=Water, 2=Air, 3=Dust, 4=Fire, 5=Storm).
    pub element: u8,
    /// Tier of this reagent (matches `target_tier`).
    pub tier: u8,
    /// Purity in `[0, 1]`. Higher = more surplus essence in the primary element.
    pub purity: f32,
}

/// Result of a refinement attempt.
#[derive(Debug, PartialEq)]
pub struct RefineResult {
    /// `Some(reagent)` if refinement succeeded; `None` if insufficient essence.
    pub produced: Option<Reagent>,
    /// How many units were consumed from each slot.
    pub consumed: [u32; N_ELEM],
    /// What remains in each slot after refinement.
    pub leftover: [u32; N_ELEM],
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Primary essence cost (units of the target element) for a given tier.
///
/// Cost is `COST_BASE * COST_MULT^tier`, computed with saturating arithmetic.
/// Tier 0 → 4, tier 1 → 12, tier 2 → 36, tier 3 → 108, …
pub fn cost(tier: u8) -> u32 {
    let mut c = COST_BASE;
    for _ in 0..tier {
        c = c.saturating_mul(COST_MULT);
    }
    c
}

/// Total catalyst cost (spread across the two neighbour elements) for a tier.
///
/// Tier 0 → 2, tier 1 → 4, tier 2 → 8, … Saturating.
pub fn catalyst_cost(tier: u8) -> u32 {
    let mut c = CATALYST_BASE;
    for _ in 0..tier {
        c = c.saturating_mul(CATALYST_MULT);
    }
    c
}

/// Plan and execute refinement of `target_element` at `target_tier` from `pool`.
///
/// - `target_element` is taken mod 6; any value is safe.
/// - If `target_tier` would make costs overflow u32 they are clamped to u32::MAX
///   (meaning the pool can never satisfy them — `produced` is `None`).
/// - Pure, deterministic, saturating. Never panics.
pub fn refine(pool: &EssencePool, target_element: u8, target_tier: u8) -> RefineResult {
    let e = (target_element % N_ELEM as u8) as usize;
    let primary = cost(target_tier);
    let cat_total = catalyst_cost(target_tier);

    // Split catalyst across left and right neighbours (mod 6).
    let left  = (e + N_ELEM - 1) % N_ELEM;
    let right = (e + 1) % N_ELEM;
    let cat_right = cat_total / 2;
    let cat_left  = cat_total - cat_right; // picks up odd remainder

    // When N_ELEM == 1 (degenerate), neighbours alias to e — guard against double-subtract.
    // With N_ELEM=6, left != e != right always, but keep the guard for safety.
    let left_is_self  = left  == e;
    let right_is_self = right == e;

    // Recompute: if a neighbour aliases to e, the primary pool must cover everything.
    let primary_covers_all = {
        let mut need_from_e = primary;
        if left_is_self  { need_from_e = need_from_e.saturating_add(cat_left); }
        if right_is_self { need_from_e = need_from_e.saturating_add(cat_right); }
        pool.units[e] >= need_from_e
            && (left_is_self  || pool.units[left]  >= cat_left)
            && (right_is_self || pool.units[right] >= cat_right)
    };

    if !primary_covers_all {
        // Not enough — return unchanged pool.
        return RefineResult {
            produced: None,
            consumed: [0; N_ELEM],
            leftover: pool.units,
        };
    }

    // Build consumed array.
    let mut consumed = [0u32; N_ELEM];
    consumed[e] = consumed[e].saturating_add(primary);
    if !left_is_self  { consumed[left]  = consumed[left].saturating_add(cat_left); }
    else              { consumed[e]     = consumed[e].saturating_add(cat_left); }
    if !right_is_self { consumed[right] = consumed[right].saturating_add(cat_right); }
    else              { consumed[e]     = consumed[e].saturating_add(cat_right); }

    // Leftover = pool - consumed (saturating; should be exact given the check above).
    let mut leftover = [0u32; N_ELEM];
    for i in 0..N_ELEM {
        leftover[i] = pool.units[i].saturating_sub(consumed[i]);
    }

    // Purity: how much of the primary element remains relative to what was there.
    let surplus = pool.units[e].saturating_sub(consumed[e]);
    let purity = if pool.units[e] > 0 {
        (surplus as f32 / pool.units[e] as f32).clamp(0.0, 1.0)
    } else {
        0.0
    };

    RefineResult {
        produced: Some(Reagent { element: e as u8, tier: target_tier, purity }),
        consumed,
        leftover,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn pool(units: [u32; 6]) -> EssencePool { EssencePool { units } }

    /// Conservation check: consumed + leftover == pool for all slots.
    fn assert_conserved(result: &RefineResult, pool: &EssencePool) {
        for i in 0..N_ELEM {
            assert_eq!(
                result.consumed[i].saturating_add(result.leftover[i]),
                pool.units[i],
                "conservation violated at slot {i}: consumed={} leftover={} pool={}",
                result.consumed[i], result.leftover[i], pool.units[i]
            );
        }
    }

    #[test]
    fn sufficient_units_produces_reagent() {
        // Fire (e=4), tier 0: costs 4 primary, catalyst=2 (1 left=Dust, 1 right=Storm)
        let p = pool([0, 0, 0, 10, 100, 10]); // Dust=10, Fire=100, Storm=10
        let r = refine(&p, 4, 0);
        assert!(r.produced.is_some(), "should produce a reagent");
        let reg = r.produced.unwrap();
        assert_eq!(reg.element, 4);
        assert_eq!(reg.tier, 0);
        assert!(reg.purity >= 0.0 && reg.purity <= 1.0);
        assert_conserved(&r, &p);
    }

    #[test]
    fn insufficient_primary_produces_none() {
        // Fire tier 0 needs 4 units of Fire — give only 2.
        let p = pool([0, 0, 0, 10, 2, 10]);
        let r = refine(&p, 4, 0);
        assert!(r.produced.is_none(), "should NOT produce (insufficient primary)");
        assert_eq!(r.consumed, [0; 6]);
        assert_conserved(&r, &p);
    }

    #[test]
    fn insufficient_catalyst_produces_none() {
        // Fire tier 0 needs 1 catalyst each from Dust(e=3) and Storm(e=5).
        // Give plenty of Fire but zero neighbours.
        let p = pool([0, 0, 0, 0, 100, 0]);
        let r = refine(&p, 4, 0);
        assert!(r.produced.is_none(), "should NOT produce (insufficient catalyst)");
        assert_eq!(r.consumed, [0; 6]);
        assert_conserved(&r, &p);
    }

    #[test]
    fn conservation_always_holds_on_success() {
        let p = pool([50, 50, 50, 50, 50, 50]);
        for e in 0u8..6 {
            for t in 0u8..4 {
                let r = refine(&p, e, t);
                assert_conserved(&r, &p);
            }
        }
    }

    #[test]
    fn conservation_always_holds_on_failure() {
        let p = pool([0, 0, 0, 0, 0, 0]);
        for e in 0u8..6 {
            let r = refine(&p, e, 3);
            assert!(r.produced.is_none());
            assert_conserved(&r, &p);
        }
    }

    #[test]
    fn higher_tier_costs_strictly_more() {
        for t in 0u8..6 {
            assert!(cost(t + 1) > cost(t),
                "cost(tier {}) not > cost(tier {}): {} vs {}",
                t + 1, t, cost(t + 1), cost(t));
        }
    }

    #[test]
    fn cost_tier0_is_base() {
        assert_eq!(cost(0), COST_BASE);
    }

    #[test]
    fn bad_element_handled_without_panic() {
        // element = 255 → 255 % 6 = 3 (Dust), should work without panic
        let p = pool([50, 50, 50, 50, 50, 50]);
        let r = refine(&p, 255, 0);
        assert_conserved(&r, &p);
    }

    #[test]
    fn very_high_tier_no_panic() {
        let p = pool([u32::MAX; 6]);
        let r = refine(&p, 0, u8::MAX);
        assert_conserved(&r, &p);
    }

    #[test]
    fn purity_in_range() {
        let p = pool([0, 0, 0, 50, 200, 50]);
        let r = refine(&p, 4, 0);
        if let Some(reg) = r.produced {
            assert!(reg.purity >= 0.0 && reg.purity <= 1.0,
                "purity {} out of [0,1]", reg.purity);
            assert!(reg.purity.is_finite(), "purity not finite");
        }
    }

    #[test]
    fn purity_decreases_with_smaller_surplus() {
        // Full pool vs barely-enough pool: full pool has more surplus → higher purity.
        let p_full = pool([50, 50, 50, 50, 1000, 50]);
        let p_bare = pool([50, 50, 50, 50, 4, 50]); // exactly cost(0)=4
        let r_full = refine(&p_full, 4, 0);
        let r_bare = refine(&p_bare, 4, 0);
        let pur_full = r_full.produced.map(|r| r.purity).unwrap_or(0.0);
        let pur_bare = r_bare.produced.map(|r| r.purity).unwrap_or(0.0);
        assert!(pur_full >= pur_bare, "more surplus should give >= purity");
    }

    #[test]
    fn produced_element_and_tier_match_request() {
        let p = pool([100; 6]);
        let r = refine(&p, 2, 1);
        let reg = r.produced.expect("should succeed with full pool");
        assert_eq!(reg.element, 2);
        assert_eq!(reg.tier, 1);
    }

    #[test]
    fn deterministic() {
        let p = pool([30, 20, 40, 15, 80, 25]);
        let a = refine(&p, 3, 2);
        let b = refine(&p, 3, 2);
        assert_eq!(a.produced, b.produced);
        assert_eq!(a.consumed, b.consumed);
        assert_eq!(a.leftover, b.leftover);
    }

    #[test]
    fn property_conservation_lcg_sweep() {
        // Seed 0xF1C3_E4A7 (documented, R20 style).
        let mut seed = 0xF1C3_E4A7u32;
        let mut lcg = || {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            seed
        };
        for _ in 0..3000 {
            let units = std::array::from_fn::<u32, 6, _>(|_| lcg() % 200);
            let p = EssencePool { units };
            let e = (lcg() % 6) as u8;
            let t = (lcg() % 8) as u8;
            let r = refine(&p, e, t);
            assert_conserved(&r, &p);
            // Purity in range when produced.
            if let Some(reg) = r.produced {
                assert!(reg.purity >= 0.0 && reg.purity <= 1.0);
                assert!(reg.purity.is_finite());
            }
        }
    }
}
