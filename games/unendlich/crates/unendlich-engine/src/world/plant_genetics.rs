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

/// The visible colour family of foliage.
///
/// Recovered with `pigment` from the orphaned `sim::flora_genetics`. Declaration
/// order was its dominance ladder; here it is the order of the unit interval,
/// which is what quantising a continuous gene gives. Green is widest because a
/// forest that is mostly not green is a different game.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Hue {
    Green,
    Gold,
    Crimson,
    Violet,
    Pale,
}

impl Hue {
    pub const ALL: [Hue; 5] = [Hue::Green, Hue::Gold, Hue::Crimson, Hue::Violet, Hue::Pale];

    /// Quantise an expressed gene value into a colour family.
    ///
    /// GREEN TAKES HALF THE RANGE and the other four split the rest. An even
    /// split would make one wild plant in five violet, which is not a forest,
    /// it is a fruit bowl. The rarities are the point of the trait: a crimson
    /// stand should be worth walking to.
    pub fn from_unit(v: f32) -> Hue {
        match v.clamp(0.0, 1.0) {
            x if x < 0.50 => Hue::Green,
            x if x < 0.70 => Hue::Gold,
            x if x < 0.85 => Hue::Crimson,
            x if x < 0.95 => Hue::Violet,
            _ => Hue::Pale,
        }
    }
}

/// How well a creature's colouring hides it against local foliage.
///
/// Ported from the orphan, and the reason the colour traits were worth
/// recovering at all: they connect plants to the creatures in them. Matching
/// hue and shade in DENSE foliage is near-total concealment; a mismatch, or
/// sparse cover, offers little.
///
/// Density multiplies rather than adds, which is the load-bearing part: a
/// perfectly-matched creature standing in the open is still standing in the
/// open. Adding would let colour alone hide something on bare ground.
pub fn crypsis(creature: (Hue, f32), foliage: (Hue, f32), foliage_density: f32) -> f32 {
    let (ch, cs) = creature;
    let (fh, fs) = foliage;
    let shade_match = 1.0 - (cs - fs).abs().clamp(0.0, 1.0);
    // A mismatch is 0.4 rather than 0: a brown creature in green leaves is
    // still harder to see than one on stone, because a broken outline does
    // most of the work.
    let hue_match = if ch == fh { 1.0 } else { 0.4 };
    (0.5 * shade_match + 0.5 * hue_match) * foliage_density.clamp(0.0, 1.0)
}

/// How much a dark plant benefits from its pigment, given how cold it is.
///
/// `cold01` is 0 at the heat end and 1 at the cold end. Dark foliage absorbs
/// more sun, so it is an advantage in the cold and a LIABILITY in heat — the
/// sign flips, which is what makes pigment a cline across latitude rather than
/// a strictly better trait with a colour attached.
pub fn pigment_thermal_bonus(pigment: f32, cold01: f32) -> f32 {
    let p = pigment.clamp(0.0, 1.0);
    let c = cold01.clamp(0.0, 1.0);
    // -1..1 scaled by how dark it is: pale plants care about neither end.
    (c * 2.0 - 1.0) * p
}

/// Trait order — the index each gene occupies for `activator_for`. Stable.
pub const PLANT_TRAITS: [&str; 8] = [
    "max_height", "growth_rate", "drought_tolerance", "branchiness", "resonance_gain",
    // APPENDED, never inserted. The index IS the identity here -- `activator_for`
    // keys epigenetic switches off it, and popgen indexes `genes()` directly --
    // so putting a new trait anywhere but the end would silently re-point every
    // existing plant's switches at a different gene.
    "fire_resistance",
    // Appended for the same reason, and in this order: pigment then hue, so a
    // species that only wants shade can stop reading at index 6.
    "pigment",
    "hue",
];

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
    /// How well the plant survives fire.
    ///
    /// Recovered from `sim::flora_genetics`, a PARALLEL diploid engine that
    /// this module's own header says should not exist ("one Mendelian machine
    /// across the whole world, not a parallel one") and which nothing calls.
    /// The trait it carried is worth having and the second machine is not, so
    /// the trait moves here and rides the shared engine like every other.
    ///
    /// It matters because this world has dragons that breathe fire: without
    /// it, a Fire dragon's pass over a forest is uniform, and a forest that
    /// burns uniformly has no survivors to reseed from and no reason for one
    /// stand to differ from the next.
    pub fire_resistance: Gene,
    /// Leaf/bark darkness, 0 pale to 1 dark.
    ///
    /// Not cosmetic. Dark foliage absorbs more sun, so it runs hotter: an
    /// advantage in the cold and a liability in heat, which makes the same
    /// species' colour a real cline across latitude rather than a skin.
    pub pigment: Gene,
    /// Colour family, quantised from a continuous gene into [`Hue`].
    ///
    /// Categorical traits usually want their own allele type, and the orphan
    /// gave them one. Quantising a normal `Gene` instead is the price of
    /// keeping ONE Mendelian machine — which this module's header is explicit
    /// about, and which is worth more than an exact dominance ladder for a
    /// colour. The cost is that hue blends where the orphan had strict
    /// dominance; see [`Hue::from_unit`].
    pub hue: Gene,
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
            fire_resistance: roll_gene(seed ^ 0x06),
            pigment: roll_gene(seed ^ 0x07),
            hue: roll_gene(seed ^ 0x08),
        }
    }

    /// The genes in [`PLANT_TRAITS`] order (so the index matches `activator_for`).
    pub fn genes(&self) -> [Gene; 8] {
        [
            self.max_height,
            self.growth_rate,
            self.drought_tolerance,
            self.branchiness,
            self.resonance_gain,
            self.fire_resistance,
            self.pigment,
            self.hue,
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
            fire_resistance: p(5),
            pigment: p(6),
            hue: Hue::from_unit(p(7)),
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
            fire_resistance: cross(&self.fire_resistance, &other.fire_resistance, s(0x06)),
            pigment: cross(&self.pigment, &other.pigment, s(0x07)),
            hue: cross(&self.hue, &other.hue, s(0x08)),
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
    /// Fire survival, `0..=1`. 0 burns like tinder, 1 is fire-hardened bark.
    pub fire_resistance: f32,
    /// Darkness, `0..=1`. Drives sun absorption and therefore leaf temperature.
    pub pigment: f32,
    /// Visible colour family.
    pub hue: Hue,
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
            fire_resistance: g,
            pigment: g,
            hue: g,
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
            fire_resistance: 0.5,
            pigment: 0.5,
            hue: Hue::Green,
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
            fire_resistance: 0.5,
            pigment: 0.5,
            hue: Hue::Green,
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
            fire_resistance: 0.5,
            pigment: 0.5,
            hue: Hue::Green,
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
            fire_resistance: 0.5,
            pigment: 0.5,
            hue: Hue::Green,
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
            fire_resistance: g,
            pigment: g,
            hue: g,
        };
        let off = genome.express(5, 0b0000_0000).growth_rate;
        let on = genome.express(5, 0b0000_0001).growth_rate;
        assert!(on < off, "env switch should up-regulate the low-value allele");
    }

    /// FIRE RESISTANCE, recovered from the orphaned parallel engine. It rides
    /// the shared diploid machine like every other plant trait -- which is the
    /// whole reason it moved here rather than the second machine being revived.
    #[test]
    fn fire_resistance_is_heritable_like_any_other_trait() {
        let hardy = Gene::new(Allele::new(1.0, 0.5, 0), Allele::new(1.0, 0.5, 0));
        let tinder = Gene::new(Allele::new(0.0, 0.5, 0), Allele::new(0.0, 0.5, 0));
        let mut a = uniform(0.5); a.fire_resistance = hardy;
        let mut b = uniform(0.5); b.fire_resistance = tinder;
        // Hardy x hardy breeds true.
        let child = a.breed(&a, 7);
        assert!(child.express(1, 0).fire_resistance > 0.9,
                "got {}", child.express(1, 0).fire_resistance);
        // Hardy x tinder lands between, as a codominant cross should.
        let mixed = a.breed(&b, 7).express(1, 0).fire_resistance;
        assert!(mixed > 0.05 && mixed < 0.95, "got {mixed}");
    }

    /// THE INDEX IS THE IDENTITY. `activator_for` keys epigenetic switches off
    /// a gene's position and popgen indexes `genes()` directly, so a new trait
    /// must be APPENDED. Inserting one would silently re-point every existing
    /// plant's switches at a different gene.
    #[test]
    fn the_new_trait_is_appended_so_existing_indices_do_not_move() {
        assert_eq!(PLANT_TRAITS[0], "max_height");
        assert_eq!(PLANT_TRAITS[4], "resonance_gain");
        assert_eq!(PLANT_TRAITS[5], "fire_resistance", "appended, not inserted");
        assert_eq!(PLANT_TRAITS.len(), PlantGenome::from_seed(1).genes().len(),
                   "the trait names and the gene array must stay the same length");
    }

    /// Two plants from different seeds differ in fire resistance, or a forest
    /// burns uniformly and no stand differs from the next.
    #[test]
    fn wild_plants_vary_in_fire_resistance() {
        let vals: Vec<f32> = (1..40u64)
            .map(|s| PlantGenome::from_seed(s).express(s, 0).fire_resistance)
            .collect();
        let lo = vals.iter().cloned().fold(f32::MAX, f32::min);
        let hi = vals.iter().cloned().fold(f32::MIN, f32::max);
        assert!(hi - lo > 0.3, "wild stands barely vary: {lo}..{hi}");
        assert!(vals.iter().all(|v| (0.0..=1.0).contains(v)));
    }

    /// GREEN TAKES HALF THE RANGE. An even split would make one wild plant in
    /// five violet, which is a fruit bowl and not a forest.
    #[test]
    fn most_wild_foliage_is_green_and_pale_is_rare() {
        let mut counts = std::collections::BTreeMap::new();
        for seed in 1..600u64 {
            let h = PlantGenome::from_seed(seed).express(seed, 0).hue;
            *counts.entry(h).or_insert(0usize) += 1;
        }
        let green = counts.get(&Hue::Green).copied().unwrap_or(0);
        let pale = counts.get(&Hue::Pale).copied().unwrap_or(0);
        assert!(green > 599 / 3, "a forest should be mostly green, got {green}/599");
        assert!(pale < green, "pale must be the rare one: {pale} vs {green}");
    }

    /// DENSITY MULTIPLIES. A perfectly-matched creature standing in the open
    /// is still standing in the open -- adding would let colour alone hide
    /// something on bare ground.
    #[test]
    fn colour_cannot_hide_a_creature_with_no_cover() {
        let matched = (Hue::Green, 0.5);
        assert_eq!(crypsis(matched, matched, 0.0), 0.0);
        assert!(crypsis(matched, matched, 1.0) > 0.99, "and hides it completely in cover");
    }

    /// A mismatch is worse but not useless: a broken outline does most of the
    /// work, so a brown creature in green leaves still beats bare stone.
    #[test]
    fn a_colour_mismatch_still_beats_no_cover_at_all() {
        let dense = 1.0;
        let matched = crypsis((Hue::Green, 0.5), (Hue::Green, 0.5), dense);
        let mismatched = crypsis((Hue::Crimson, 0.5), (Hue::Green, 0.5), dense);
        assert!(mismatched < matched);
        assert!(mismatched > 0.5, "still substantial: {mismatched}");
        // Shade matters independently of hue.
        let wrong_shade = crypsis((Hue::Green, 0.0), (Hue::Green, 1.0), dense);
        assert!(wrong_shade < matched);
    }

    /// THE SIGN FLIPS. Dark foliage is an advantage in the cold and a
    /// liability in heat -- which is what makes pigment a cline across
    /// latitude rather than a strictly better trait wearing a colour.
    #[test]
    fn dark_foliage_helps_in_the_cold_and_hurts_in_the_heat() {
        assert!(pigment_thermal_bonus(1.0, 1.0) > 0.9, "dark plant, cold place");
        assert!(pigment_thermal_bonus(1.0, 0.0) < -0.9, "dark plant, hot place");
        // A pale plant is indifferent at both ends.
        assert!(pigment_thermal_bonus(0.0, 1.0).abs() < 1e-6);
        assert!(pigment_thermal_bonus(0.0, 0.0).abs() < 1e-6);
    }

    /// Colour is heritable through the same machine as everything else, which
    /// is the whole reason it came here instead of the orphan being revived.
    #[test]
    fn pigment_is_inherited_like_any_other_trait() {
        let dark = Gene::new(Allele::new(1.0, 0.5, 0), Allele::new(1.0, 0.5, 0));
        let mut a = uniform(0.5); a.pigment = dark;
        assert!(a.breed(&a, 3).express(1, 0).pigment > 0.9);
    }

    /// Eight traits, eight genes, and the earlier indices unmoved.
    #[test]
    fn the_trait_table_and_the_gene_array_stay_in_step() {
        assert_eq!(PLANT_TRAITS.len(), 8);
        assert_eq!(PLANT_TRAITS.len(), PlantGenome::from_seed(1).genes().len());
        assert_eq!(PLANT_TRAITS[5], "fire_resistance");
        assert_eq!(PLANT_TRAITS[6], "pigment");
        assert_eq!(PLANT_TRAITS[7], "hue");
    }
}
