//! `sim::dragon_growth_stage` — age + maturation genes -> growth stage + size/stat scales.
//!
//! Maps a creature's **age** (in ticks) and two genome **maturation genes** to a
//! discrete [`GrowthStage`] plus continuous [`GrowthState`] multipliers that
//! scale the blueprint size and combat stats. Feeds `dragon_blueprint_from_genome`
//! (multiply its half-extents by `GrowthState::size_mul`) and the stat system.
//!
//! ## Stage thresholds
//!
//! Thresholds are in ticks. The two maturation genes shift the Hatchling→Juvenile
//! and Juvenile→Adult boundaries by up to ±25 % so fast-maturing dragons grow up
//! sooner and slow-maturing ones linger in juvenile form longer.
//!
//! | Stage     | Entry age (neutral genes)   |
//! |-----------|------------------------------|
//! | Egg       | 0                            |
//! | Hatchling | EGG_TICKS       = 1 200      |
//! | Juvenile  | HATCHLING_TICKS = 12 000     |
//! | Adult     | JUVENILE_TICKS  = 72 000     |
//!
//! Size multiplier rises monotonically from [`SIZE_EGG`] (0.08) to 1.0 (Adult).
//! Stat multipliers follow the same monotone curve.
//!
//! ## Gene contract
//! - `maturation_rate` in `[0,1]`: 0 = slow (thresholds ×1.25), 1 = fast (×0.75).
//! - `early_growth`    in `[0,1]`: shifts only the Hatchling→Juvenile boundary.
//! Out-of-range genes are clamped silently.

// ---------------------------------------------------------------------------
// Stage thresholds (neutral-gene tick values)
// ---------------------------------------------------------------------------

/// Ticks from age 0 until the egg hatches (Egg -> Hatchling).
/// Not affected by maturation genes — the egg incubation period is fixed.
pub const EGG_TICKS: u64 = 1_200;

/// Ticks from hatch until Hatchling -> Juvenile (neutral genes).
pub const HATCHLING_TICKS: u64 = 12_000;

/// Ticks from juvenile onset until Juvenile -> Adult (neutral genes).
pub const JUVENILE_TICKS: u64 = 72_000;

/// Minimum size multiplier (egg stage). In (0, 1].
pub const SIZE_EGG: f32 = 0.08;
/// Size multiplier at hatch.
pub const SIZE_HATCHLING: f32 = 0.22;
/// Size multiplier at juvenile onset.
pub const SIZE_JUVENILE: f32 = 0.55;
/// Size multiplier at adult (full size).
pub const SIZE_ADULT: f32 = 1.0;

/// Gene multiplier range for maturation thresholds (slow..fast).
const MAT_SLOW: f32 = 1.25;
const MAT_FAST: f32 = 0.75;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Discrete life stage of a dragon.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum GrowthStage {
    Egg,
    Hatchling,
    Juvenile,
    Adult,
}

/// Maturation genes that influence growth thresholds.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MaturationGenes {
    /// Overall maturation rate, `[0,1]`: 0 = slow, 1 = fast.
    pub maturation_rate: f32,
    /// Early growth boost, `[0,1]`: shifts Hatchling->Juvenile threshold earlier.
    pub early_growth: f32,
}

impl MaturationGenes {
    /// Neutral genes — both at midpoint.
    pub fn neutral() -> Self {
        Self { maturation_rate: 0.5, early_growth: 0.5 }
    }
}

/// The result of growth evaluation: stage + continuous multipliers.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GrowthState {
    pub stage: GrowthStage,
    /// Isotropic size multiplier in `(0, 1]`. Multiply blueprint half-extents by this.
    pub size_mul: f32,
    /// HP/max-HP stat multiplier in `(0, 1]`.
    pub hp_mul: f32,
    /// Damage output multiplier in `(0, 1]`.
    pub damage_mul: f32,
    /// Speed multiplier in `(0, 1]`.
    pub speed_mul: f32,
}

// ---------------------------------------------------------------------------
// Core function
// ---------------------------------------------------------------------------

/// Clamped linear interpolation.
#[inline(always)]
fn lerp(a: f32, b: f32, t: f32) -> f32 {
    let t = t.max(0.0).min(1.0);
    a + (b - a) * t
}

/// Compute thresholds (absolute tick ages) from genes.
/// Returns `(egg_end, hatch_end, juv_end)`.
fn thresholds(g: MaturationGenes) -> (u64, u64, u64) {
    let mr = g.maturation_rate.max(0.0).min(1.0);
    let eg = g.early_growth.max(0.0).min(1.0);

    // maturation_rate scales both post-hatch thresholds
    let mat_mul = lerp(MAT_SLOW, MAT_FAST, mr);
    // early_growth additionally shifts Hatchling->Juvenile
    let early_mul = lerp(MAT_SLOW, MAT_FAST, eg);

    let egg_end  = EGG_TICKS; // fixed
    let hatch_dur = (HATCHLING_TICKS as f32 * mat_mul * early_mul).round() as u64;
    let juv_dur   = (JUVENILE_TICKS  as f32 * mat_mul).round() as u64;

    (egg_end, egg_end + hatch_dur, egg_end + hatch_dur + juv_dur)
}

/// Map `age_ticks` + maturation genes to a [`GrowthState`].
///
/// - Age 0 is always [`GrowthStage::Egg`]; very large ages are always
///   [`GrowthStage::Adult`].
/// - Stage is monotone non-decreasing in age.
/// - Size/stat multipliers are monotone non-decreasing in age.
/// - Out-of-range genes (< 0 or > 1) are silently clamped.
/// - Deterministic: same `(age_ticks, genes)` -> same output every call.
pub fn dragon_growth_stage(age_ticks: u64, genes: MaturationGenes) -> GrowthState {
    let (egg_end, hatch_end, juv_end) = thresholds(genes);

    let (stage, size_mul, stat_t) = if age_ticks < egg_end {
        // Egg: age rises from 0 to egg_end
        let t = age_ticks as f32 / egg_end.max(1) as f32;
        let sz = lerp(SIZE_EGG, SIZE_HATCHLING * 0.9, t);
        (GrowthStage::Egg, sz, t * 0.15)
    } else if age_ticks < hatch_end {
        // Hatchling
        let span = (hatch_end - egg_end).max(1);
        let t = (age_ticks - egg_end) as f32 / span as f32;
        let sz = lerp(SIZE_HATCHLING, SIZE_JUVENILE * 0.95, t);
        (GrowthStage::Hatchling, sz, lerp(0.15, 0.45, t))
    } else if age_ticks < juv_end {
        // Juvenile
        let span = (juv_end - hatch_end).max(1);
        let t = (age_ticks - hatch_end) as f32 / span as f32;
        let sz = lerp(SIZE_JUVENILE, SIZE_ADULT * 0.97, t);
        (GrowthStage::Juvenile, sz, lerp(0.45, 0.85, t))
    } else {
        // Adult — fully grown
        (GrowthStage::Adult, SIZE_ADULT, 1.0)
    };

    // HP and damage scale with a power curve for more dramatic juvenile→adult jump
    let hp_mul     = lerp(0.08, 1.0, stat_t * stat_t + stat_t * (1.0 - stat_t) * 0.5).max(0.01);
    let damage_mul = lerp(0.05, 1.0, stat_t).max(0.01);
    let speed_mul  = lerp(0.55, 1.0, stat_t).max(0.01);

    GrowthState {
        stage,
        size_mul:   size_mul.max(0.001).min(1.0),
        hp_mul:     hp_mul.min(1.0),
        damage_mul: damage_mul.min(1.0),
        speed_mul:  speed_mul.min(1.0),
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn neutral() -> MaturationGenes { MaturationGenes::neutral() }

    #[test]
    fn age_zero_is_egg() {
        assert_eq!(dragon_growth_stage(0, neutral()).stage, GrowthStage::Egg);
    }

    #[test]
    fn very_large_age_is_adult() {
        assert_eq!(dragon_growth_stage(u64::MAX / 2, neutral()).stage, GrowthStage::Adult);
        assert_eq!(dragon_growth_stage(1_000_000_000, neutral()).stage, GrowthStage::Adult);
    }

    #[test]
    fn stage_monotone_non_decreasing_in_age() {
        let genes = neutral();
        let max_age = JUVENILE_TICKS * 3 + EGG_TICKS;
        let step = max_age / 200;
        let mut prev_stage = GrowthStage::Egg;
        let mut age = 0u64;
        while age <= max_age {
            let s = dragon_growth_stage(age, genes).stage;
            assert!(s >= prev_stage, "stage went backwards at age {age}: {prev_stage:?} -> {s:?}");
            prev_stage = s;
            age += step;
        }
    }

    #[test]
    fn size_mul_monotone_non_decreasing_in_age() {
        let genes = neutral();
        let max_age = JUVENILE_TICKS * 2 + EGG_TICKS;
        let step = max_age / 500;
        let mut prev = 0.0f32;
        let mut age = 0u64;
        while age <= max_age {
            let sz = dragon_growth_stage(age, genes).size_mul;
            assert!(sz >= prev - 1e-5, "size_mul decreased at age {age}: {prev} -> {sz}");
            prev = sz;
            age += step;
        }
    }

    #[test]
    fn size_mul_in_range() {
        for age in [0u64, 600, EGG_TICKS, 50_000, 200_000, 1_000_000] {
            let gs = dragon_growth_stage(age, neutral());
            assert!(gs.size_mul > 0.0,  "size_mul not positive at age {age}");
            assert!(gs.size_mul <= 1.0, "size_mul > 1 at age {age}: {}", gs.size_mul);
            assert!(gs.size_mul.is_finite(), "size_mul not finite at age {age}");
        }
    }

    #[test]
    fn adult_size_mul_is_one() {
        let gs = dragon_growth_stage(1_000_000, neutral());
        assert_eq!(gs.stage, GrowthStage::Adult);
        assert!((gs.size_mul - 1.0).abs() < 1e-5, "adult size_mul != 1.0: {}", gs.size_mul);
    }

    #[test]
    fn out_of_range_genes_clamp() {
        let hi = MaturationGenes { maturation_rate: 5.0, early_growth: 9.0 };
        let lo = MaturationGenes { maturation_rate: -3.0, early_growth: -1.0 };
        let max_g = MaturationGenes { maturation_rate: 1.0, early_growth: 1.0 };
        let min_g = MaturationGenes { maturation_rate: 0.0, early_growth: 0.0 };
        for age in [0u64, 1_000, 10_000, 500_000] {
            assert_eq!(dragon_growth_stage(age, hi), dragon_growth_stage(age, max_g));
            assert_eq!(dragon_growth_stage(age, lo), dragon_growth_stage(age, min_g));
        }
    }

    #[test]
    fn deterministic() {
        let g = neutral();
        for age in [0u64, 5_000, 50_000, 200_000] {
            assert_eq!(dragon_growth_stage(age, g), dragon_growth_stage(age, g));
        }
    }

    #[test]
    fn fast_maturation_reaches_adult_earlier_than_slow() {
        let fast = MaturationGenes { maturation_rate: 1.0, early_growth: 1.0 };
        let slow = MaturationGenes { maturation_rate: 0.0, early_growth: 0.0 };
        // At a mid-juvenile age for neutral genes, fast should be Adult, slow still not.
        let mid = EGG_TICKS + (HATCHLING_TICKS as f32 * MAT_FAST * MAT_FAST) as u64
                + (JUVENILE_TICKS as f32 * MAT_FAST) as u64 + 1;
        assert_eq!(dragon_growth_stage(mid, fast).stage, GrowthStage::Adult,
            "fast gene should be Adult by tick {mid}");
        assert_ne!(dragon_growth_stage(mid, slow).stage, GrowthStage::Adult,
            "slow gene should not yet be Adult by tick {mid}");
    }

    #[test]
    fn all_stat_muls_positive_and_finite() {
        for age in [0u64, 600, 6_000, 50_000, 200_000, 1_000_000] {
            let gs = dragon_growth_stage(age, neutral());
            for (name, v) in [
                ("hp_mul",     gs.hp_mul),
                ("damage_mul", gs.damage_mul),
                ("speed_mul",  gs.speed_mul),
            ] {
                assert!(v > 0.0,   "{name} not positive at age {age}: {v}");
                assert!(v <= 1.0,  "{name} > 1 at age {age}: {v}");
                assert!(v.is_finite(), "{name} not finite at age {age}");
            }
        }
    }

    #[test]
    fn hatchling_stage_exists_at_expected_age() {
        // Just past egg_end, should be Hatchling (not Egg or Juvenile).
        let age = EGG_TICKS + 1;
        assert_eq!(dragon_growth_stage(age, neutral()).stage, GrowthStage::Hatchling);
    }

    #[test]
    fn egg_stage_ends_at_egg_ticks() {
        assert_eq!(dragon_growth_stage(EGG_TICKS - 1, neutral()).stage, GrowthStage::Egg);
        assert_eq!(dragon_growth_stage(EGG_TICKS,     neutral()).stage, GrowthStage::Hatchling);
    }
}
