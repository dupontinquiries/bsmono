//! `sim::flora_harvest` — what harvesting a grown plant yields.
//!
//! Composes the plant loop: R34 `germination` and R35 `canopy_field_shift` give a
//! plant a `strain_bias` and a `growth01`; this turns a *mature* plant into a
//! harvest — per-element **reagents** keyed to the plant's element (rare bloom
//! strains, via `strain_bias`, also drop a little of the neighbouring elements),
//! a little **essence** (feeding `essence_refine`), and a **regrow timer** that
//! lengthens with how much was taken. Immature plants yield ~nothing. This is the
//! flora mirror of the creature-side [`super::dragon_material_yield`].
//!
//! Element palette (mirrors `dragon_material_yield` / `artifact_imbue`):
//! `0=Wood 1=Water 2=Air 3=Dust 4=Fire 5=Storm`, read modulo 6.
//!
//! Mobile: yield is a pure function of the compact 3-field `PlantState` (`Copy`,
//! no heap, no `Vec`), so a field of thousands of plants harvests with **zero
//! allocation**. Because the result depends only on the descriptor
//! (`element`, `strain_bias`, `growth01`) and never on per-instance lifecycle
//! state, a DISTANT plant can be harvested straight from its descriptor without
//! running the full growth/canopy sim — a memory/CPU-tight 8 GB mobile SoC can
//! skip far-field plant simulation entirely (LOD-friendly) and still produce the
//! correct yield on demand.
//!
//! Pure, deterministic, saturating, no panic. No `crate::render`, no `ash`/`vk`,
//! no IO, no clock, no RNG, no `.unwrap()`/`.expect()` in non-test code; compiles
//! clean under `cargo check --release` (no `cfg(test)`/`cfg(debug_assertions)`
//! gated helpers).

/// A plant ready to (maybe) be harvested.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PlantState {
    /// Element `0..5` (palette above); read modulo 6 so out-of-range is safe.
    pub element: u8,
    /// Strain bias `0..1` — rare-bloom factor; boosts primary reagents and adds
    /// cross-element units. Clamped.
    pub strain_bias: f32,
    /// Growth `0..1`; below maturity the plant yields ~nothing. Clamped.
    pub growth01: f32,
}

/// The result of harvesting: per-element reagent units
/// `[Wood, Water, Air, Dust, Fire, Storm]`, a little essence, and a regrow time.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HarvestYield {
    pub reagent_units: [u32; 6],
    pub essence: u32,
    pub regrow_secs: f32,
}

/// Growth below this is immature → ~zero yield.
pub const MATURITY: f32 = 0.4;
/// Primary-slot reagent units at full maturity (before strain boost).
const PRIMARY_MAX: f32 = 20.0;
/// Cross-element (rare-bloom) units per neighbour at full maturity & strain.
const CROSS_MAX: f32 = 8.0;
/// Essence at full maturity (before strain boost).
const ESSENCE_MAX: f32 = 5.0;
/// Regrow floor (seconds) — a harvested-or-not plant always needs some regrow.
const REGROW_BASE: f32 = 30.0;
/// Extra regrow seconds per unit taken.
const REGROW_PER_UNIT: f32 = 1.5;

fn clamp01(v: f32) -> f32 {
    crate::util::mathx::saturate(v) // r343 dedup
}

/// Harvest a plant. See module docs. Saturating and panic-free; immature plants
/// yield zero reagents/essence (but still report a positive regrow time).
pub fn harvest(p: &PlantState) -> HarvestYield {
    let growth = clamp01(p.growth01);
    let strain = clamp01(p.strain_bias);
    let e = (p.element % 6) as usize;

    // Maturity ramp: 0 at/below MATURITY, rising to 1 at full growth.
    let g_eff = clamp01((growth - MATURITY) / (1.0 - MATURITY));

    let mut reagent_units = [0u32; 6];

    // Primary slot (governed by element): scales with maturity, boosted by strain.
    // f32→u32 `as` is a saturating cast (NaN→0, negative→0, huge→u32::MAX).
    let primary = (g_eff * PRIMARY_MAX * (1.0 + strain)) as u32;
    reagent_units[e] = primary;

    // Rare-bloom cross-element units in the two neighbouring slots (only with
    // strain). e±1 mod 6 are always distinct from e (6 elements), so the primary
    // slot is never overwritten and always dominates.
    let cross = (g_eff * strain * CROSS_MAX) as u32;
    if cross > 0 {
        let lo = (e + 5) % 6;
        let hi = (e + 1) % 6;
        reagent_units[lo] = reagent_units[lo].saturating_add(cross);
        reagent_units[hi] = reagent_units[hi].saturating_add(cross);
    }

    // A little essence, rising with maturity & strain.
    let essence = (g_eff * (1.0 + strain) * ESSENCE_MAX) as u32;

    // Regrow lengthens with the total taken; always positive.
    let mut total: u32 = essence;
    let mut i = 0;
    while i < 6 {
        total = total.saturating_add(reagent_units[i]);
        i += 1;
    }
    let regrow_secs = REGROW_BASE + (total as f32) * REGROW_PER_UNIT;

    HarvestYield { reagent_units, essence, regrow_secs }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lcg(seed: &mut u32) -> u32 {
        *seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
        *seed >> 8
    }
    fn plant(element: u8, strain: f32, growth: f32) -> PlantState {
        PlantState { element, strain_bias: strain, growth01: growth }
    }
    fn reagent_total(y: &HarvestYield) -> u64 {
        y.reagent_units.iter().map(|&u| u as u64).sum()
    }

    #[test]
    fn immature_yields_about_nothing() {
        for &g in &[0.0, 0.1, 0.3, MATURITY] {
            let y = harvest(&plant(2, 1.0, g));
            assert_eq!(y.reagent_units, [0; 6], "immature growth {} should give no reagents", g);
            assert_eq!(y.essence, 0);
            assert!(y.regrow_secs > 0.0, "regrow always positive");
        }
    }

    #[test]
    fn element_governs_primary_slot() {
        for e in 0u8..6 {
            let y = harvest(&plant(e, 0.5, 1.0));
            let ei = e as usize;
            assert!(y.reagent_units[ei] > 0, "element {} primary slot should be filled", e);
            for s in 0..6 {
                if s != ei {
                    assert!(
                        y.reagent_units[ei] > y.reagent_units[s],
                        "element {} primary {} should dominate slot {} ({})",
                        e, y.reagent_units[ei], s, y.reagent_units[s]
                    );
                }
            }
        }
        // out-of-range element folds mod 6
        let y = harvest(&plant(200, 0.0, 1.0));
        assert!(y.reagent_units[(200 % 6) as usize] > 0);
    }

    #[test]
    fn reagents_and_essence_monotone_in_growth() {
        // fixed strain, mature range, increasing growth ⇒ non-decreasing, and
        // strictly increasing across well-separated growths.
        let lo = harvest(&plant(4, 0.3, 0.55));
        let hi = harvest(&plant(4, 0.3, 0.95));
        assert!(reagent_total(&hi) > reagent_total(&lo), "reagents should rise with growth");
        assert!(hi.essence > lo.essence, "essence should rise with growth");

        let mut prev_r = 0u64;
        let mut prev_e = 0u32;
        let mut g = MATURITY;
        while g <= 1.0001 {
            let y = harvest(&plant(4, 0.3, g));
            assert!(reagent_total(&y) >= prev_r, "reagents must be non-decreasing in growth");
            assert!(y.essence >= prev_e, "essence must be non-decreasing in growth");
            prev_r = reagent_total(&y);
            prev_e = y.essence;
            g += 0.02;
        }
    }

    #[test]
    fn reagents_and_essence_monotone_in_strain() {
        let lo = harvest(&plant(1, 0.0, 1.0));
        let hi = harvest(&plant(1, 1.0, 1.0));
        assert!(reagent_total(&hi) > reagent_total(&lo), "more strain ⇒ more reagents");
        assert!(hi.essence > lo.essence, "more strain ⇒ more essence");
        // strain adds cross-element units (zero at strain 0)
        assert_eq!(harvest(&plant(1, 0.0, 1.0)).reagent_units.iter().filter(|&&u| u > 0).count(), 1,
            "no strain ⇒ only the primary slot");
        assert!(harvest(&plant(1, 1.0, 1.0)).reagent_units.iter().filter(|&&u| u > 0).count() >= 3,
            "rare bloom ⇒ primary + cross-element slots");

        // non-decreasing sweep
        let mut prev = 0u64;
        let mut s = 0.0;
        while s <= 1.0001 {
            let t = reagent_total(&harvest(&plant(1, s, 1.0)));
            assert!(t >= prev, "reagents non-decreasing in strain");
            prev = t;
            s += 0.05;
        }
    }

    #[test]
    fn regrow_positive_and_scales_with_amount() {
        let immature = harvest(&plant(0, 0.0, 0.1));
        let small = harvest(&plant(0, 0.0, 0.6));
        let big = harvest(&plant(0, 1.0, 1.0));
        assert!(immature.regrow_secs > 0.0);
        assert!(small.regrow_secs > immature.regrow_secs, "harvesting more ⇒ longer regrow");
        assert!(big.regrow_secs > small.regrow_secs, "harvesting much more ⇒ even longer regrow");
    }

    #[test]
    fn saturating_and_clamped_no_panic() {
        // out-of-range everything: huge growth/strain, negative, NaN, big element
        let cases = [
            plant(200, 5.0, 9.0),
            plant(7, -3.0, -1.0),
            plant(3, f32::NAN, f32::INFINITY),
            plant(5, 1.0, 1.0),
        ];
        for c in &cases {
            let y = harvest(c);
            for &u in &y.reagent_units {
                let _ = u; // u32, inherently no overflow/panic
            }
            assert!(y.regrow_secs.is_finite() && y.regrow_secs > 0.0);
        }
        // at the documented max, values are bounded & sane (no overflow)
        let maxed = harvest(&plant(4, 1.0, 1.0));
        assert_eq!(maxed.reagent_units[4], (PRIMARY_MAX * 2.0) as u32); // g_eff=1, strain=1
        assert!(maxed.essence == (ESSENCE_MAX * 2.0) as u32);
    }

    #[test]
    fn nan_growth_is_immature() {
        let y = harvest(&plant(2, 1.0, f32::NAN));
        assert_eq!(y.reagent_units, [0; 6]);
        assert_eq!(y.essence, 0);
    }

    #[test]
    fn deterministic() {
        let mut seed = 0xF10A_0000u32;
        for _ in 0..5000 {
            let e = (lcg(&mut seed) % 8) as u8;
            let strain = (lcg(&mut seed) % 1001) as f32 / 1000.0;
            let growth = (lcg(&mut seed) % 1001) as f32 / 1000.0;
            let p = plant(e, strain, growth);
            assert_eq!(harvest(&p), harvest(&p));
        }
    }
}
