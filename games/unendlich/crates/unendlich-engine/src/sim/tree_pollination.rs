//! `sim::tree_pollination` — tree genetics crossing and pollination viability.
//!
//! Spec: `docs/specs/flora_update_genetics.docx` §C (Tree Genetics & Pollination).
//! Every tree carries a small genome; pollen carried between trees (by wind /
//! pollinators) merges two parent genomes into a seed's genome. This module is
//! the pure math: [`cross`] merges two parents into an offspring, and
//! [`pollination_chance`] / [`viable`] model whether a pollination event takes.
//! Composes with the `plant_*` / `plant_tree_genetics` modules (the phenotype
//! traits here are the legible, expressed values those modules consume).
//!
//! ## Inheritance (spec §C.1: codominance)
//! The spec expresses each gene by **codominance** — "the expressed phenotype is
//! the average of the two alleles." So every continuous trait of the child is the
//! mean of the two parents' traits plus a small seeded mutation (variation scales
//! with how different the parents are, so diverse crosses vary more). `species`
//! is discrete and cannot be averaged, so it is **inherited wholesale from one
//! parent**, picked by a seeded fair coin from `seed`.
//!
//! ## Pollination viability
//! [`pollination_chance`] falls with distance (0 at/beyond `max_range`), rises
//! with wind blowing toward the recipient, and scales with species
//! `compatibility01` — so a cross-species attempt (compatibility 0) is ~0.
//! [`viable`] turns a chance into a deterministic yes/no via a seeded threshold.
//!
//! Pure, deterministic, finite, panic-free. Seeded `splitmix64` only — no real
//! RNG. No `crate::render`, no `ash`/`vk`, no IO, no clock, no `.unwrap()`/
//! `.expect()` in non-test code; compiles clean under `cargo check --release`
//! (no `cfg(test)`/`cfg(debug_assertions)` gated helpers).

/// A tree's expressed genome (phenotype traits + discrete species).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TreeGenome {
    pub height: f32,
    pub canopy: f32,
    pub trunk: f32,
    pub leaf_hue: f32,
    pub hardiness: f32,
    pub species: u8,
}

/// Base mutation magnitude added to each averaged trait.
const MUT_BASE: f32 = 0.05;
/// Extra mutation magnitude proportional to the parents' trait difference.
const MUT_SPREAD: f32 = 0.10;
/// Floor on the wind contribution (pollen can drift even with poor alignment).
const WIND_FLOOR: f32 = 0.30;

/// splitmix64 — deterministic, seedable, no real RNG.
fn mix(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// Uniform `[0, 1)` from the stream.
fn unit(state: &mut u64) -> f32 {
    // top 24 bits → [0,1)
    ((mix(state) >> 40) as f32) / ((1u32 << 24) as f32)
}

/// Signed `[-1, 1)` from the stream.
fn signed(state: &mut u64) -> f32 {
    unit(state) * 2.0 - 1.0
}

fn clamp01(v: f32) -> f32 {
    crate::util::mathx::saturate(v) // r343 dedup
}

/// Cross one trait: codominant average ± mutation that scales with the parents'
/// difference.
fn cross_trait(a: f32, b: f32, state: &mut u64) -> f32 {
    let avg = 0.5 * (a + b);
    let mag = MUT_BASE + MUT_SPREAD * (a - b).abs();
    avg + signed(state) * mag
}

/// Maximum mutation magnitude for a trait given the two parent values — the
/// amount by which a child trait may exceed the parents' min/max. Unconditional
/// (release-safe; usable from `debug_assert!`).
pub fn max_trait_deviation(a: f32, b: f32) -> f32 {
    MUT_BASE + MUT_SPREAD * (a - b).abs()
}

/// Cross two parent genomes into an offspring (see module docs). Deterministic
/// in `(a, b, seed)`.
pub fn cross(a: &TreeGenome, b: &TreeGenome, seed: u64) -> TreeGenome {
    let mut s = seed ^ 0xA5A5_5A5A_C3C3_3C3C; // spread the seed into the stream
    let height = cross_trait(a.height, b.height, &mut s);
    let canopy = cross_trait(a.canopy, b.canopy, &mut s);
    let trunk = cross_trait(a.trunk, b.trunk, &mut s);
    let leaf_hue = cross_trait(a.leaf_hue, b.leaf_hue, &mut s);
    let hardiness = cross_trait(a.hardiness, b.hardiness, &mut s);
    // species: discrete, inherited from one parent by a seeded fair coin.
    let species = if unit(&mut s) < 0.5 { a.species } else { b.species };
    TreeGenome { height, canopy, trunk, leaf_hue, hardiness, species }
}

/// Probability `[0,1]` that pollen from a donor reaches and fertilises a
/// recipient `distance` away, given the wind alignment toward the recipient and
/// the species compatibility. 0 at or beyond `max_range`.
pub fn pollination_chance(distance: f32, max_range: f32, wind_align01: f32, compatibility01: f32) -> f32 {
    if !max_range.is_finite() || max_range <= 0.0 || !distance.is_finite() {
        return 0.0;
    }
    let d = distance.max(0.0);
    if d >= max_range {
        return 0.0;
    }
    let falloff = 1.0 - d / max_range; // linear, 1 at 0 → 0 at max_range
    let wind = clamp01(wind_align01);
    let wind_factor = WIND_FLOOR + (1.0 - WIND_FLOOR) * wind; // [WIND_FLOOR, 1]
    let compat = clamp01(compatibility01);
    clamp01(falloff * wind_factor * compat)
}

/// Deterministic yes/no for a pollination of probability `chance01`, seeded.
/// `chance01 <= 0` ⇒ always false, `>= 1` ⇒ always true.
pub fn viable(seed: u64, chance01: f32) -> bool {
    let mut s = seed ^ 0x1234_5678_9ABC_DEF0;
    unit(&mut s) < clamp01(chance01)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lcg(seed: &mut u32) -> u32 {
        *seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
        *seed >> 8
    }
    fn randf(seed: &mut u32, lo: f32, hi: f32) -> f32 {
        lo + (lcg(seed) % 1_000_001) as f32 / 1_000_000.0 * (hi - lo)
    }
    fn rand_genome(seed: &mut u32, species: u8) -> TreeGenome {
        TreeGenome {
            height: randf(seed, 0.0, 1.0),
            canopy: randf(seed, 0.0, 1.0),
            trunk: randf(seed, 0.0, 1.0),
            leaf_hue: randf(seed, 0.0, 1.0),
            hardiness: randf(seed, 0.0, 1.0),
            species,
        }
    }

    #[test]
    fn cross_is_deterministic() {
        let a = TreeGenome { height: 0.4, canopy: 0.6, trunk: 0.5, leaf_hue: 0.2, hardiness: 0.7, species: 1 };
        let b = TreeGenome { height: 0.8, canopy: 0.2, trunk: 0.9, leaf_hue: 0.5, hardiness: 0.1, species: 3 };
        assert_eq!(cross(&a, &b, 42), cross(&a, &b, 42));
        // different seed generally differs
        assert_ne!(cross(&a, &b, 1), cross(&a, &b, 2));
    }

    #[test]
    fn child_traits_lie_within_or_near_parent_range() {
        let mut s = 0xBEEF_0000u32;
        for _ in 0..30000 {
            let a = rand_genome(&mut s, 1);
            let b = rand_genome(&mut s, 2);
            let seed = ((lcg(&mut s) as u64) << 32) | lcg(&mut s) as u64;
            let c = cross(&a, &b, seed);
            let pairs = [
                (a.height, b.height, c.height),
                (a.canopy, b.canopy, c.canopy),
                (a.trunk, b.trunk, c.trunk),
                (a.leaf_hue, b.leaf_hue, c.leaf_hue),
                (a.hardiness, b.hardiness, c.hardiness),
            ];
            for (pa, pb, child) in pairs {
                let lo = pa.min(pb);
                let hi = pa.max(pb);
                let bound = max_trait_deviation(pa, pb) + 1e-4;
                assert!(
                    child >= lo - bound && child <= hi + bound,
                    "child {} outside [{}, {}] ± {}", child, lo, hi, bound
                );
                assert!(child.is_finite());
            }
        }
    }

    #[test]
    fn species_inherited_from_a_parent() {
        let a = TreeGenome { height: 0.5, canopy: 0.5, trunk: 0.5, leaf_hue: 0.5, hardiness: 0.5, species: 7 };
        let b = TreeGenome { species: 9, ..a };
        let mut saw7 = false;
        let mut saw9 = false;
        for seed in 0..2000u64 {
            let sp = cross(&a, &b, seed).species;
            assert!(sp == 7 || sp == 9, "species must come from a parent, got {}", sp);
            if sp == 7 {
                saw7 = true;
            }
            if sp == 9 {
                saw9 = true;
            }
        }
        assert!(saw7 && saw9, "both parents' species should occur across seeds");
    }

    #[test]
    fn pollination_zero_beyond_range() {
        assert_eq!(pollination_chance(50.0, 40.0, 1.0, 1.0), 0.0, "beyond range");
        assert_eq!(pollination_chance(40.0, 40.0, 1.0, 1.0), 0.0, "exactly at range");
        assert_eq!(pollination_chance(1e9, 40.0, 1.0, 1.0), 0.0);
        // degenerate range
        assert_eq!(pollination_chance(1.0, 0.0, 1.0, 1.0), 0.0);
    }

    #[test]
    fn pollination_monotone_decreasing_with_distance() {
        let mut prev = 2.0f32;
        let mut d = 0.0;
        while d < 40.0 {
            let c = pollination_chance(d, 40.0, 0.8, 0.9);
            assert!(c <= prev + 1e-7, "should not increase with distance");
            if d > 0.0 {
                assert!(c < prev, "should strictly decrease while positive");
            }
            prev = c;
            d += 1.0;
        }
    }

    #[test]
    fn pollination_increases_with_wind_and_compatibility() {
        // wind
        let lo_w = pollination_chance(10.0, 40.0, 0.1, 0.9);
        let hi_w = pollination_chance(10.0, 40.0, 0.9, 0.9);
        assert!(hi_w > lo_w, "more wind alignment ⇒ higher chance");
        // compatibility
        let lo_c = pollination_chance(10.0, 40.0, 0.8, 0.2);
        let hi_c = pollination_chance(10.0, 40.0, 0.8, 0.9);
        assert!(hi_c > lo_c, "more compatibility ⇒ higher chance");
    }

    #[test]
    fn cross_species_is_zero_chance() {
        for &d in &[0.0f32, 5.0, 20.0] {
            for &w in &[0.0f32, 0.5, 1.0] {
                assert_eq!(pollination_chance(d, 40.0, w, 0.0), 0.0, "compat 0 ⇒ 0 chance");
            }
        }
    }

    #[test]
    fn pollination_clamped_and_finite() {
        let mut s = 0xBEEF_0001u32;
        for _ in 0..50000 {
            let d = randf(&mut s, -10.0, 200.0);
            let mr = randf(&mut s, -5.0, 100.0);
            let w = randf(&mut s, -0.5, 1.5);
            let comp = randf(&mut s, -0.5, 1.5);
            let c = pollination_chance(d, mr, w, comp);
            assert!(c.is_finite() && c >= 0.0 && c <= 1.0, "chance out of range: {}", c);
        }
        // perfect conditions reach 1
        assert!((pollination_chance(0.0, 40.0, 1.0, 1.0) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn viable_threshold_behaviour() {
        // chance 0 ⇒ never; chance 1 ⇒ always
        for seed in 0..1000u64 {
            assert!(!viable(seed, 0.0));
            assert!(viable(seed, 1.0));
            assert!(!viable(seed, -5.0)); // clamps
            assert!(viable(seed, 9.0)); // clamps
        }
        // deterministic per (seed, chance)
        assert_eq!(viable(123, 0.5), viable(123, 0.5));
        // higher chance ⇒ at least as many viable over a seed sample (monotone)
        let count = |ch: f32| (0..5000u64).filter(|&s| viable(s, ch)).count();
        let c2 = count(0.2);
        let c5 = count(0.5);
        let c8 = count(0.8);
        assert!(c2 <= c5 && c5 <= c8, "viability rate should rise with chance: {} {} {}", c2, c5, c8);
        // and roughly track the probability
        assert!((c5 as f32 / 5000.0 - 0.5).abs() < 0.05, "≈50% at chance 0.5");
    }
}
