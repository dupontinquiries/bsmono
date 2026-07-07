//! Heritable temperament — the genome half of an adaptive brain.
//!
//! A creature's [`Disposition`] (aggression, territoriality, caution, curiosity,
//! greed) is already its behavioral genome: those five `[0,1]` traits steer every
//! goal choice. This adds inheritance — [`breed`] crosses two parents gene by
//! gene with a little mutation, so a population's temperament drifts and adapts
//! across generations instead of being fixed per seed. Pure and deterministic
//! (same parents + seed → same child), so it slots under the engine's rollback.
//!
//! Learned behavior (the cultural "watch a neighbor and copy" rule) layers on top
//! of this later; here we just establish heredity.

use crate::sim::fixed::Fixed;
use crate::sim::personality::Disposition;

/// Largest mutation nudge applied to an inherited gene, in hundredths (so ±0.10).
const MUT_MAX_PCT: i64 = 10;

fn splitmix(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

fn clamp01(x: Fixed) -> Fixed {
    if x < Fixed::ZERO {
        Fixed::ZERO
    } else if x > Fixed::ONE {
        Fixed::ONE
    } else {
        x
    }
}

/// Inherit one gene: cross (pick a parent 50/50) then mutate by ±`MUT_MAX_PCT`.
fn inherit(state: &mut u64, a: Fixed, b: Fixed) -> Fixed {
    let pick = if splitmix(state) & 1 == 0 { a } else { b };
    // Mutation in [-MUT_MAX_PCT, +MUT_MAX_PCT] hundredths.
    let m = (splitmix(state) % (2 * MUT_MAX_PCT as u64 + 1)) as i64 - MUT_MAX_PCT;
    clamp01(pick + Fixed::from_frac(m, 100))
}

/// Cross two parents into a child genome, gene by gene, with mutation.
/// Deterministic in `seed`.
pub fn breed(a: &Disposition, b: &Disposition, seed: u64) -> Disposition {
    let mut s = seed.wrapping_mul(0x2545_F491_4F6C_DD1D).wrapping_add(0x9E37);
    Disposition {
        aggression: inherit(&mut s, a.aggression, b.aggression),
        territoriality: inherit(&mut s, a.territoriality, b.territoriality),
        caution: inherit(&mut s, a.caution, b.caution),
        curiosity: inherit(&mut s, a.curiosity, b.curiosity),
        greed: inherit(&mut s, a.greed, b.greed),
    }
}

/// Blend (average) two parents gene by gene, with mutation — a smoother
/// alternative to crossover when you want children to converge on the mean.
pub fn blend(a: &Disposition, b: &Disposition, seed: u64) -> Disposition {
    let mut s = seed.wrapping_mul(0xD1B5_4A32_D192_ED03).wrapping_add(0x1234);
    let half = Fixed::from_frac(1, 2);
    let gene = |s: &mut u64, ga: Fixed, gb: Fixed| {
        let avg = (ga + gb) * half;
        let m = (splitmix(s) % (2 * MUT_MAX_PCT as u64 + 1)) as i64 - MUT_MAX_PCT;
        clamp01(avg + Fixed::from_frac(m, 100))
    };
    Disposition {
        aggression: gene(&mut s, a.aggression, b.aggression),
        territoriality: gene(&mut s, a.territoriality, b.territoriality),
        caution: gene(&mut s, a.caution, b.caution),
        curiosity: gene(&mut s, a.curiosity, b.curiosity),
        greed: gene(&mut s, a.greed, b.greed),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(ag: i64, te: i64, ca: i64, cu: i64, gr: i64) -> Disposition {
        Disposition {
            aggression: Fixed::from_frac(ag, 100),
            territoriality: Fixed::from_frac(te, 100),
            caution: Fixed::from_frac(ca, 100),
            curiosity: Fixed::from_frac(cu, 100),
            greed: Fixed::from_frac(gr, 100),
        }
    }
    fn genes(x: &Disposition) -> [Fixed; 5] {
        [x.aggression, x.territoriality, x.caution, x.curiosity, x.greed]
    }

    #[test]
    fn children_genes_stay_in_unit_range() {
        // Parents at the extremes; mutation must never push a child out of [0,1].
        let lo = d(0, 0, 0, 0, 0);
        let hi = d(100, 100, 100, 100, 100);
        for seed in 0..200u64 {
            for g in genes(&breed(&lo, &hi, seed)) {
                assert!(g >= Fixed::ZERO && g <= Fixed::ONE, "gene out of range at seed {seed}");
            }
        }
    }

    #[test]
    fn a_gene_resembles_a_parent_plus_small_mutation() {
        // With both parents sharing a gene value, the child's must be within the
        // mutation bound of it.
        let a = d(50, 50, 50, 50, 50);
        let b = d(50, 50, 50, 50, 50);
        let bound = Fixed::from_frac(MUT_MAX_PCT, 100);
        for seed in 0..100u64 {
            for g in genes(&breed(&a, &b, seed)) {
                let lo = Fixed::from_frac(50, 100) - bound;
                let hi = Fixed::from_frac(50, 100) + bound;
                assert!(g >= lo && g <= hi, "gene drifted too far at seed {seed}");
            }
        }
    }

    #[test]
    fn breeding_is_deterministic_but_varies_across_seeds() {
        let a = d(80, 20, 10, 60, 40);
        let b = d(10, 70, 90, 20, 50);
        assert_eq!(genes(&breed(&a, &b, 7)), genes(&breed(&a, &b, 7)), "same seed → same child");
        // Different seeds should produce at least some different children.
        let distinct: std::collections::HashSet<_> =
            (0..32u64).map(|s| genes(&breed(&a, &b, s)).map(|g| g.to_f32().to_bits())).collect();
        assert!(distinct.len() > 8, "breeding should yield variety, got {}", distinct.len());
    }

    #[test]
    fn blend_lands_near_the_parental_mean() {
        let a = d(80, 80, 80, 80, 80);
        let b = d(20, 20, 20, 20, 20);
        for seed in 0..100u64 {
            for g in genes(&blend(&a, &b, seed)) {
                let v = g.to_f32();
                assert!((0.39..=0.61).contains(&v), "blend should sit near the 0.50 mean, got {v} at seed {seed}");
            }
        }
    }
}
