//! Plant genetics — heritable plant traits + grower resonance.
//!
//! Plants ride the *same* diploid engine creatures and dragon eggs use
//! ([`crate::genetics`]: [`Allele`]/[`Gene`]/[`cross`]/[`activator_for`]), so
//! there is one Mendelian machine across the whole world, not a parallel one.
//! What differs is the **trait set**: a plant carries genes for the things a
//! plant cares about, expressed into a [`PlantPhenotype`] the lifecycle reads.
//!
//! # Two new capabilities this models
//!
//! 1. **Grow taller genetically.** [`PlantGenome::max_height`] is a heritable
//!    cap. A tall lineage tends to beget tall offspring ([`PlantGenome::breed`]
//!    is a per-gene [`cross`], so values pass on intact while loci/regulation
//!    drift). Wild plants get a deterministic genome from their position seed
//!    ([`PlantGenome::from_seed`]) so nothing has to change to give *every*
//!    existing plant a genome for free.
//!
//! 2. **Grower resonance.** A "grower" — a Wood-dragon of the Keystone-grower
//!    morph, or a player holding a growth item — deposits a Wood-growth field
//!    into the world (the `emission`/Discharge substrate, Wood payload). Plants
//!    nearby read the local resonance level the same way the lifecycle already
//!    reads the moisture field, and respond via [`growth_response`]: they grow
//!    faster and reach taller, scaled by their *heritable* [`resonance_gain`].
//!    A plant deaf to resonance (`resonance_gain ≈ 0`) ignores growers entirely.
//!
//! [`resonance_gain`]: PlantPhenotype::resonance_gain
//!
//! # Perf note (this is a mobile-first engine)
//!
//! Taller plants and resonance both mean *more structural growth*, and every
//! wood block a plant adds flips its chunk `structural_dirty` → octree rebuild +
//! GPU upload + persist. So the model is built to **bound** that churn:
//!   - height is genetically capped, and resonance adds only a bounded bonus up
//!     to a hard ceiling — growth converges and then *stops* dirtying;
//!   - resonance is local and rare (only near a grower);
//!   - the growth-rate phenotype + the lifecycle's stage-duration constants gate
//!     edits/second — the cheapest lever on a weak SoC is simply slower growth.
//! Resonance should make a plant reach its cap *sooner*, not edit forever.

use crate::genetics::{activator_for, cross, Allele, EnvSwitches, Gene};

/// Genetic height range, in blocks, before any resonance bonus.
pub const MIN_HEIGHT: u8 = 3;
pub const MAX_HEIGHT: u8 = 24;
/// Absolute ceiling including resonance overgrowth. Nothing exceeds this.
pub const HARD_MAX_HEIGHT: u8 = 32;

/// Growth-rate multiplier range (applied to stage durations: >1 = faster).
pub const GROWTH_RATE_MIN: f32 = 0.5;
pub const GROWTH_RATE_MAX: f32 = 2.0;

/// Drought threshold range: the moisture level below which a plant risks
/// drying. A *tolerant* lineage (high gene value) sits at the low end.
pub const DROUGHT_THRESHOLD_WET: f32 = 0.8; // gene 0.0 → needs it wet
pub const DROUGHT_THRESHOLD_DRY: f32 = 0.2; // gene 1.0 → survives dry

/// At full resonance × full gain, growth rate is multiplied by `1 + this`…
pub const RESONANCE_RATE_BOOST: f32 = 2.0;
/// …and height gains up to this many extra blocks (capped at [`HARD_MAX_HEIGHT`]).
pub const RESONANCE_HEIGHT_BONUS: u8 = 8;

/// Trait order — the index each gene occupies for `activator_for`. Stable.
pub const PLANT_TRAITS: [&str; 5] =
    ["max_height", "growth_rate", "drought_tolerance", "branchiness", "resonance_gain"];

#[inline]
fn mix(seed: u64) -> u64 {
    let mut z = seed.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// A `[0,1)` draw from a seed (matches the genetics module's own hash so plant
/// and creature rolls are statistically identical).
#[inline]
fn unit(seed: u64) -> f32 {
    (mix(seed) >> 40) as f32 / (1u64 << 24) as f32
}

#[inline]
fn lerp(a: f32, b: f32, t: f32) -> f32 {
    crate::util::mathx::lerp(a, b, t) // r342 dedup
}

#[inline]
fn lerp_u8(a: u8, b: u8, t: f32) -> u8 {
    (a as f32 + (b as f32 - a as f32) * t.clamp(0.0, 1.0)).round() as u8
}

/// Roll a fresh gene from a seed: two independent alleles (one per "parent" of a
/// wild plant), each with a seed-drawn value, strand locus, and sparse switches.
fn roll_gene(seed: u64) -> Gene {
    let a = Allele::new(unit(seed ^ 0xA1), unit(seed ^ 0xA2), (mix(seed ^ 0xA3) & 0b11) as u8);
    let b = Allele::new(unit(seed ^ 0xB1), unit(seed ^ 0xB2), (mix(seed ^ 0xB3) & 0b11) as u8);
    Gene::new(a, b)
}

/// A plant's full genotype: one diploid [`Gene`] per heritable trait.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PlantGenome {
    /// Genetic height cap — the "grow taller genetically" trait.
    pub max_height: Gene,
    /// Vigor: how fast the plant advances through its stages.
    pub growth_rate: Gene,
    /// Drought tolerance: shifts the moisture threshold for drying out.
    pub drought_tolerance: Gene,
    /// Bushiness: branch / blade density of the rendered form.
    pub branchiness: Gene,
    /// Sensitivity to grower resonance (Wood dragons / growth items).
    pub resonance_gain: Gene,
}

impl PlantGenome {
    /// Deterministic genome for a wild plant from its position/plant seed.
    pub fn from_seed(seed: u64) -> PlantGenome {
        PlantGenome {
            max_height: roll_gene(seed ^ 0x01),
            growth_rate: roll_gene(seed ^ 0x02),
            drought_tolerance: roll_gene(seed ^ 0x03),
            branchiness: roll_gene(seed ^ 0x04),
            resonance_gain: roll_gene(seed ^ 0x05),
        }
    }

    /// The genes in [`PLANT_TRAITS`] order (so the index matches `activator_for`).
    pub fn genes(&self) -> [Gene; 5] {
        [
            self.max_height,
            self.growth_rate,
            self.drought_tolerance,
            self.branchiness,
            self.resonance_gain,
        ]
    }

    /// Express this genome for the individual `plant_seed` in environment `env`
    /// (epigenetic switches: wet/dry/cold/heat… meaning is the caller's).
    pub fn express(&self, plant_seed: u64, env: EnvSwitches) -> PlantPhenotype {
        let g = self.genes();
        let p = |i: usize| g[i].phenotype(activator_for(plant_seed, i as u32), env);
        PlantPhenotype {
            max_height: lerp_u8(MIN_HEIGHT, MAX_HEIGHT, p(0)),
            growth_rate: lerp(GROWTH_RATE_MIN, GROWTH_RATE_MAX, p(1)),
            drought_threshold: lerp(DROUGHT_THRESHOLD_WET, DROUGHT_THRESHOLD_DRY, p(2)),
            branchiness: p(3),
            resonance_gain: p(4),
        }
    }

    /// Breed two plants: each child gene is a Mendelian [`cross`] of the parents'
    /// corresponding genes. Tall × tall ⇒ tall (values pass on intact); a
    /// heterozygous cross reconstructs the 1:2:1 spread, with loci drift.
    pub fn breed(&self, other: &PlantGenome, seed: u64) -> PlantGenome {
        let s = |salt: u64| seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ salt;
        PlantGenome {
            max_height: cross(&self.max_height, &other.max_height, s(0x01)),
            growth_rate: cross(&self.growth_rate, &other.growth_rate, s(0x02)),
            drought_tolerance: cross(&self.drought_tolerance, &other.drought_tolerance, s(0x03)),
            branchiness: cross(&self.branchiness, &other.branchiness, s(0x04)),
            resonance_gain: cross(&self.resonance_gain, &other.resonance_gain, s(0x05)),
        }
    }
}

/// The expressed, concrete traits the lifecycle consumes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PlantPhenotype {
    /// Genetic height cap in blocks ([`MIN_HEIGHT`]..=[`MAX_HEIGHT`]).
    pub max_height: u8,
    /// Stage-duration multiplier (>1 = faster), [`GROWTH_RATE_MIN`]..=[`GROWTH_RATE_MAX`].
    pub growth_rate: f32,
    /// Moisture level below which the plant risks drying. Lower = more tolerant.
    pub drought_threshold: f32,
    /// Branch / blade density, `0..=1`.
    pub branchiness: f32,
    /// Resonance sensitivity, `0..=1`.
    pub resonance_gain: f32,
}

/// What a plant does this tick under a local grower-resonance level.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GrowthResponse {
    /// Growth-rate multiplier after resonance (≥ the phenotype's base rate).
    pub rate_mul: f32,
    /// Height to grow toward, in blocks (≥ genetic cap, ≤ [`HARD_MAX_HEIGHT`]).
    pub target_height: u8,
}

/// Combine a plant's phenotype with the local grower-resonance level (`0..=1`,
/// sampled from the Wood-growth field a grower deposits). Resonance speeds the
/// plant up and lets it reach taller — both scaled by its heritable
/// `resonance_gain`, so a deaf plant ignores growers and the effect is bounded.
pub fn growth_response(pheno: &PlantPhenotype, resonance: f32) -> GrowthResponse {
    let boost = resonance.clamp(0.0, 1.0) * pheno.resonance_gain.clamp(0.0, 1.0);
    let rate_mul = pheno.growth_rate * (1.0 + boost * RESONANCE_RATE_BOOST);
    let bonus = (boost * RESONANCE_HEIGHT_BONUS as f32).round() as u16;
    let target_height = (pheno.max_height as u16 + bonus).min(HARD_MAX_HEIGHT as u16) as u8;
    GrowthResponse { rate_mul, target_height }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A genome where every gene is homozygous for `h` (locus 0.5, no switches),
    /// so every expressed trait equals `lerp(.., h)` regardless of activator/env.
    fn uniform(h: f32) -> PlantGenome {
        let g = Gene::new(Allele::new(h, 0.5, 0), Allele::new(h, 0.5, 0));
        PlantGenome {
            max_height: g,
            growth_rate: g,
            drought_tolerance: g,
            branchiness: g,
            resonance_gain: g,
        }
    }

    #[test]
    fn from_seed_and_express_are_deterministic() {
        assert_eq!(PlantGenome::from_seed(42), PlantGenome::from_seed(42));
        let g = PlantGenome::from_seed(7);
        assert_eq!(g.express(7, 0), g.express(7, 0));
        assert_eq!(g.breed(&g, 9), g.breed(&g, 9));
    }

    #[test]
    fn expressed_traits_stay_in_range() {
        for seed in 0..256u64 {
            let p = PlantGenome::from_seed(seed).express(seed, 0);
            assert!(p.max_height >= MIN_HEIGHT && p.max_height <= MAX_HEIGHT);
            assert!(p.growth_rate >= GROWTH_RATE_MIN - 1e-3 && p.growth_rate <= GROWTH_RATE_MAX + 1e-3);
            assert!(p.drought_threshold >= DROUGHT_THRESHOLD_DRY - 1e-3
                && p.drought_threshold <= DROUGHT_THRESHOLD_WET + 1e-3);
            assert!((0.0..=1.0).contains(&p.branchiness));
            assert!((0.0..=1.0).contains(&p.resonance_gain));
        }
    }

    #[test]
    fn tall_lineage_begets_tall_offspring() {
        // Homozygous tall × tall keeps the value intact through the cross, so the
        // child expresses the genetic max; short × short stays at the floor.
        let tall = uniform(1.0);
        let short = uniform(0.0);
        let child_tall = tall.breed(&tall, 0x7A11);
        let child_short = short.breed(&short, 0x7A11);
        assert_eq!(child_tall.express(1, 0).max_height, MAX_HEIGHT);
        assert_eq!(child_short.express(1, 0).max_height, MIN_HEIGHT);
        assert!(child_tall.express(1, 0).max_height > child_short.express(1, 0).max_height);
    }

    #[test]
    fn drought_tolerance_maps_monotonically() {
        // Higher gene value = more tolerant = lower moisture threshold.
        let tolerant = uniform(1.0).express(1, 0).drought_threshold;
        let thirsty = uniform(0.0).express(1, 0).drought_threshold;
        assert!(tolerant < thirsty);
        assert!((tolerant - DROUGHT_THRESHOLD_DRY).abs() < 1e-3);
        assert!((thirsty - DROUGHT_THRESHOLD_WET).abs() < 1e-3);
    }

    #[test]
    fn zero_resonance_is_the_baseline() {
        let pheno = PlantPhenotype {
            max_height: 10,
            growth_rate: 1.0,
            drought_threshold: 0.5,
            branchiness: 0.5,
            resonance_gain: 1.0,
        };
        let r = growth_response(&pheno, 0.0);
        assert_eq!(r.rate_mul, 1.0);
        assert_eq!(r.target_height, 10);
    }

    #[test]
    fn resonance_speeds_up_and_grows_taller_scaled_by_gain() {
        let pheno = PlantPhenotype {
            max_height: 10,
            growth_rate: 1.0,
            drought_threshold: 0.5,
            branchiness: 0.5,
            resonance_gain: 1.0,
        };
        let base = growth_response(&pheno, 0.0);
        let full = growth_response(&pheno, 1.0);
        // rate 1.0 * (1 + 1*2) = 3.0 ; height 10 + round(1*8) = 18
        assert_eq!(full.rate_mul, 3.0);
        assert_eq!(full.target_height, 18);
        assert!(full.rate_mul > base.rate_mul);
        assert!(full.target_height > base.target_height);
    }

    #[test]
    fn a_deaf_plant_ignores_growers() {
        let deaf = PlantPhenotype {
            max_height: 12,
            growth_rate: 1.5,
            drought_threshold: 0.5,
            branchiness: 0.5,
            resonance_gain: 0.0,
        };
        let r = growth_response(&deaf, 1.0);
        assert_eq!(r.rate_mul, 1.5);
        assert_eq!(r.target_height, 12);
    }

    #[test]
    fn resonance_height_is_hard_capped() {
        let pheno = PlantPhenotype {
            max_height: MAX_HEIGHT,
            growth_rate: 1.0,
            drought_threshold: 0.5,
            branchiness: 0.5,
            resonance_gain: 1.0,
        };
        let r = growth_response(&pheno, 1.0); // 24 + 8 = 32 = HARD_MAX
        assert!(r.target_height <= HARD_MAX_HEIGHT);
        assert_eq!(r.target_height, HARD_MAX_HEIGHT);
    }

    #[test]
    fn env_switches_change_expression() {
        // a: value 0 with a switch bit; b: value 1, no switches. Turning a's
        // switch on (env) up-regulates a → blend pulls toward 0 → lower trait.
        let a = Allele::new(0.0, 0.2, 0b0000_0001);
        let b = Allele::new(1.0, 0.8, 0b0000_0000);
        let g = Gene::new(a, b);
        let genome = PlantGenome {
            max_height: g,
            growth_rate: g,
            drought_tolerance: g,
            branchiness: g,
            resonance_gain: g,
        };
        let off = genome.express(5, 0b0000_0000).growth_rate;
        let on = genome.express(5, 0b0000_0001).growth_rate;
        assert!(on < off, "env switch should up-regulate the low-value allele");
    }
}
