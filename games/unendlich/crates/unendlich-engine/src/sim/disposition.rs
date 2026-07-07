//! `sim::disposition` — express a creature's genome into the **behavioural +
//! perceptual disposition** the mechanism modules consume: `poi::PoiWeights`, a
//! boldness/aggression baseline for the arbiter + witnessed reaction, hearing
//! acuity + sight range for `acoustics` / detection, and the innate sound-class set.
//!
//! This is Step 1 of `docs/design/genome_loci_for_behavior.md`: one
//! `express_disposition(genome, entity_seed)` that turns genes into the inputs
//! those modules already take, so wiring it later changes nothing in them.
//!
//! Genome status (honest): the diploid `Genome` already carries `eyesight` and
//! `hearing` — those are read through its real `express(...).{eyesight,hearing}`
//! phenotypes. The purely-behavioural loci (boldness, sociality, caution,
//! foraging_drive, aggression_base) are NOT in the genome yet, so they're derived
//! deterministically from `entity_seed` here — a per-individual, replayable stand-in
//! that selection can't yet act on. When those genes are added (§1 of the doc), swap
//! each derivation for a phenotype read; the `Disposition` interface stays fixed.
//!
//! Pure & deterministic: genome + seed in, disposition out. No clock/RNG/IO/panic.

use crate::genetics::{EnvSwitches, Genome};
use crate::sim::acoustics::SoundClass;
use crate::sim::poi::PoiWeights;

/// The expressed disposition — exactly the inputs the mechanism modules take.
#[derive(Clone, Debug, PartialEq)]
pub struct Disposition {
    /// Per-category POI pull weights for `poi::to_drives` / `poi::poi_pull`.
    pub poi_weights: PoiWeights,
    /// Resting aggression the aggro decay relaxes *toward* (instead of 0) — a
    /// naturally hostile lineage idles aggressive. `[0,1]`.
    pub aggression_base: f32,
    /// Fight-vs-flee genetic tip in `[0,1]`: high → the arbiter's Fight needs less
    /// provocation and a witnessed attack tips toward help; low → flees/scatters.
    pub boldness: f32,
    /// Hearing acuity for `acoustics::effective_hearing` `[0,1]`.
    pub hearing_acuity: f32,
    /// Perception/sight range in blocks (witness + detection radius), from eyesight.
    pub sight_range: f32,
    /// Sounds this creature recognises from birth (the "sound DNA" it's born with).
    pub innate_classes: Vec<SoundClass>,
}

/// Smallest + largest sight range (blocks) the eyesight phenotype maps onto.
pub const SIGHT_MIN: f32 = 10.0;
pub const SIGHT_MAX: f32 = 24.0;

/// splitmix64 finaliser — derive an independent `[0,1]` locus from the entity seed
/// for a behavioural trait not yet in the genome (replayable per individual).
#[inline]
fn derived_locus(entity_seed: u64, salt: u64) -> f32 {
    let mut x = entity_seed ^ salt.wrapping_mul(0x9E37_79B9_7F4A_7C15);
    x = (x ^ (x >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    x ^= x >> 31;
    ((x >> 40) as f32) / ((1u64 << 24) as f32) // [0,1)
}

/// Express `genome` for the individual `entity_seed` into its [`Disposition`].
///
/// `eyesight` + `hearing` come from the genome's real phenotypes; the behavioural
/// loci are derived from `entity_seed` (see module docs — placeholders until those
/// genes exist). The POI weights are built so that a cautious individual values
/// shelter + treats foes as more salient, a social one flocks, a foraging-driven
/// one chases food — the heritable disposition the design calls for.
pub fn express_disposition(genome: &Genome, entity_seed: u64) -> Disposition {
    let pheno = genome.express(entity_seed, EnvSwitches::default());
    let eyesight = pheno.eyesight.clamp(0.0, 1.0);
    let hearing_acuity = pheno.hearing.clamp(0.0, 1.0);

    // behavioural loci (derived placeholders, distinct salts so they're independent)
    let boldness = derived_locus(entity_seed, 0xB01D);
    let sociality = derived_locus(entity_seed, 0x50C1);
    let caution = derived_locus(entity_seed, 0xCA07);
    let foraging = derived_locus(entity_seed, 0xF00D);
    // resting aggression: bold lineages idle a little hostile, but capped low.
    let aggression_base = (boldness * 0.3).clamp(0.0, 0.5);

    let poi_weights = PoiWeights {
        food: 0.5 + foraging,        // [0.5, 1.5]
        shelter: 0.5 + caution,      // cautious → seeks cover
        friend: 0.5 + sociality,     // social → flocks
        foe: 0.5 + caution,          // cautious → foes more salient
    };

    let sight_range = SIGHT_MIN + eyesight * (SIGHT_MAX - SIGHT_MIN);

    Disposition {
        poi_weights,
        aggression_base,
        boldness,
        hearing_acuity,
        sight_range,
        innate_classes: Vec::new(), // populated when the innate-class genes land
    }
}

/// Combine a creature's current aggro with its genetic floor: aggression never
/// relaxes below `aggression_base`. Used at the aggro-decay site so a hostile
/// lineage stays primed. (Helper so the wiring is one call.)
#[inline]
pub fn aggro_with_floor(current: f32, aggression_base: f32) -> f32 {
    current.max(aggression_base).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disposition_is_deterministic() {
        let g = Genome::from_seed(1234);
        let a = express_disposition(&g, 77);
        let b = express_disposition(&g, 77);
        assert_eq!(a, b, "same genome+seed → same disposition");
    }

    #[test]
    fn different_individuals_differ() {
        let g = Genome::from_seed(1234);
        let a = express_disposition(&g, 1);
        let b = express_disposition(&g, 2);
        // the derived behavioural loci should differ across individuals
        assert!(
            a.boldness != b.boldness || a.poi_weights != b.poi_weights,
            "different entity seeds → different disposition"
        );
    }

    #[test]
    fn sight_range_tracks_eyesight_bounds() {
        // whatever the genome, sight range stays within [SIGHT_MIN, SIGHT_MAX].
        for s in 0..50u64 {
            let g = Genome::from_seed(s * 13 + 1);
            let d = express_disposition(&g, s);
            assert!(d.sight_range >= SIGHT_MIN - 1e-3 && d.sight_range <= SIGHT_MAX + 1e-3,
                "sight {} out of bounds", d.sight_range);
        }
    }

    #[test]
    fn poi_weights_in_expected_range() {
        let g = Genome::from_seed(42);
        let d = express_disposition(&g, 9);
        for w in [d.poi_weights.food, d.poi_weights.shelter, d.poi_weights.friend, d.poi_weights.foe] {
            assert!((0.5..=1.5).contains(&w), "weight {w} out of [0.5,1.5]");
        }
    }

    #[test]
    fn aggression_base_capped() {
        for s in 0..50u64 {
            let g = Genome::from_seed(s + 7);
            let d = express_disposition(&g, s * 3);
            assert!((0.0..=0.5).contains(&d.aggression_base), "agg_base {} too high", d.aggression_base);
        }
    }

    #[test]
    fn aggro_floor_holds() {
        assert_eq!(aggro_with_floor(0.1, 0.4), 0.4, "floor raises low aggro");
        assert_eq!(aggro_with_floor(0.7, 0.4), 0.7, "high aggro unaffected");
        assert_eq!(aggro_with_floor(0.0, 0.0), 0.0);
    }

    #[test]
    fn boldness_and_acuity_in_unit_range() {
        let g = Genome::from_seed(555);
        let d = express_disposition(&g, 21);
        assert!((0.0..=1.0).contains(&d.boldness));
        assert!((0.0..=1.0).contains(&d.hearing_acuity));
    }
}
