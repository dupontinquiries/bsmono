//! `world::plant_gene_traits` — heritable plant traits + expression, mirroring
//! the dragon genome→phenotype pattern.
//!
//! A plant carries a [`Genotype`] of genes normalised to `0..1`; [`express`]
//! maps it deterministically to a [`Phenotype`] (also `0..1`) the lifecycle
//! reads, and [`crossover`] breeds two parents into a child genotype reusing the
//! same per-gene blend as `sim::gene_crossover` (continuous traits blend by a
//! seed-derived mix; the quantised trait picks a parent). This module produces
//! only the trait values — the blueprint/lifecycle consumption is integration.
//!
//! Pure, standalone `rustc --test`-able (R1): it mirrors the gene-crossover
//! blend locally rather than importing it, so it has no crate-internal deps.
//! Deterministic, no clock, no IO, no `unwrap`/`expect` (R1/R6/R8/R11).

/// A plant's heritable genes, each normalised `0..1`.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Genotype {
    /// Genetic height cap (0 = shortest lineage, 1 = tallest).
    pub height_scale: f32,
    /// Foliage thickness. **Quantised** trait (expresses to discrete steps).
    pub leaf_density: f32,
    /// Tolerance to cold (1 = survives the coldest).
    pub cold_tolerance: f32,
    /// Growth-rate gene (1 = fastest).
    pub growth_rate: f32,
}

impl Genotype {
    /// Construct, clamping every gene into `0..1`.
    pub fn new(height_scale: f32, leaf_density: f32, cold_tolerance: f32, growth_rate: f32) -> Genotype {
        Genotype {
            height_scale: clamp01(height_scale),
            leaf_density: clamp01(leaf_density),
            cold_tolerance: clamp01(cold_tolerance),
            growth_rate: clamp01(growth_rate),
        }
    }
}

/// The expressed traits a plant presents, each `0..1`.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Phenotype {
    pub height_scale: f32,
    pub leaf_density: f32,
    pub cold_tolerance: f32,
    pub growth_rate: f32,
}

/// Number of discrete steps the quantised `leaf_density` trait expresses into.
pub const LEAF_DENSITY_STEPS: u32 = 5;

fn clamp01(v: f32) -> f32 {
    crate::util::mathx::saturate(v) // r343 dedup
}

/// Local mirror of `sim::gene_crossover::crossover`: blend `a→b` by `mix`
/// (`0` = all A, `1` = all B), inputs and output clamped to `0..1`.
fn blend(a: f32, b: f32, mix: f32) -> f32 {
    let a = clamp01(a);
    let b = clamp01(b);
    // identical parents breed true exactly (avoids a*(1-m)+a*m float drift)
    if a == b {
        return a;
    }
    let m = clamp01(mix);
    clamp01(a * (1.0 - m) + b * m)
}

/// Deterministic `[0,1)` hash of `(seed, salt)` (splitmix64-style).
// r193 (guide 4.1): salt-fold + splitmix-to-unit == mix64_unit. Identical.
fn hash01(seed: u64, salt: u64) -> f32 {
    crate::util::hash_util::mix64_unit(seed.wrapping_add(salt.wrapping_mul(0x9E37_79B9_7F4A_7C15)))
}

/// Quantise `g` into one of [`LEAF_DENSITY_STEPS`] evenly-spaced levels in
/// `0..1` (e.g. 5 steps → {0, 0.25, 0.5, 0.75, 1}).
fn quantise(g: f32, steps: u32) -> f32 {
    let n = steps.max(2) - 1; // number of intervals
    let level = (clamp01(g) * n as f32 + 0.5) as u32; // round to nearest level
    let level = if level > n { n } else { level };
    level as f32 / n as f32
}

/// Express a genotype into its phenotype (deterministic). Continuous traits pass
/// through (clamped), `growth_rate` gets a smoothstep regulatory curve, and
/// `leaf_density` is quantised to discrete foliage steps. All outputs `0..1`.
pub fn express(genotype: Genotype) -> Phenotype {
    let gr = clamp01(genotype.growth_rate);
    Phenotype {
        height_scale: clamp01(genotype.height_scale),
        leaf_density: quantise(genotype.leaf_density, LEAF_DENSITY_STEPS),
        cold_tolerance: clamp01(genotype.cold_tolerance),
        // smoothstep: a mild regulatory non-linearity, still monotone in 0..1
        growth_rate: clamp01(gr * gr * (3.0 - 2.0 * gr)),
    }
}

/// Breed two parents into a child genotype, deterministic in `seed`. Continuous
/// traits blend by a per-trait seed-derived mix (reusing the gene-crossover
/// approach); the quantised `leaf_density` picks one parent's value. Every gene
/// stays in `0..1`.
pub fn crossover(parent_a: Genotype, parent_b: Genotype, seed: u64) -> Genotype {
    let m_height = hash01(seed, 1);
    let m_cold = hash01(seed, 2);
    let m_growth = hash01(seed, 3);
    // quantised trait: pick a parent by a seed bit rather than blending
    let leaf = if hash01(seed, 4) < 0.5 {
        clamp01(parent_a.leaf_density)
    } else {
        clamp01(parent_b.leaf_density)
    };
    Genotype {
        height_scale: blend(parent_a.height_scale, parent_b.height_scale, m_height),
        leaf_density: leaf,
        cold_tolerance: blend(parent_a.cold_tolerance, parent_b.cold_tolerance, m_cold),
        growth_rate: blend(parent_a.growth_rate, parent_b.growth_rate, m_growth),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lcg(seed: &mut u64) -> f32 {
        *seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        (*seed >> 40) as f32 / ((1u64 << 24) as f32)
    }

    fn in01(v: f32) -> bool {
        v.is_finite() && (0.0..=1.0).contains(&v)
    }
    fn pheno_in01(p: Phenotype) -> bool {
        in01(p.height_scale) && in01(p.leaf_density) && in01(p.cold_tolerance) && in01(p.growth_rate)
    }
    fn geno_in01(g: Genotype) -> bool {
        in01(g.height_scale) && in01(g.leaf_density) && in01(g.cold_tolerance) && in01(g.growth_rate)
    }

    #[test]
    fn crossover_stays_in_range_even_for_bad_inputs() {
        let mut s = 0x50FA_u64; // documented seed (R20)
        for _ in 0..5000 {
            // deliberately out-of-range and NaN-ish parents
            let a = Genotype {
                height_scale: lcg(&mut s) * 4.0 - 2.0,
                leaf_density: lcg(&mut s) * 4.0 - 2.0,
                cold_tolerance: lcg(&mut s),
                growth_rate: lcg(&mut s),
            };
            let b = Genotype {
                height_scale: lcg(&mut s),
                leaf_density: lcg(&mut s) * 4.0 - 2.0,
                cold_tolerance: lcg(&mut s) * 4.0 - 2.0,
                growth_rate: lcg(&mut s),
            };
            let child = crossover(a, b, s);
            assert!(geno_in01(child), "child gene out of range: {:?}", child);
        }
    }

    #[test]
    fn expression_is_deterministic() {
        let g = Genotype::new(0.3, 0.62, 0.9, 0.4);
        assert_eq!(express(g), express(g));
    }

    #[test]
    fn crossover_is_deterministic_per_seed() {
        let a = Genotype::new(0.2, 0.7, 0.5, 0.9);
        let b = Genotype::new(0.8, 0.1, 0.4, 0.3);
        assert_eq!(crossover(a, b, 12345), crossover(a, b, 12345));
        // a different seed generally gives a different child
        assert!(crossover(a, b, 1) != crossover(a, b, 2) || a == b);
    }

    #[test]
    fn no_nan_inf_for_bounded_inputs() {
        let mut s = 0x9911_u64;
        for _ in 0..3000 {
            let g = Genotype::new(lcg(&mut s), lcg(&mut s), lcg(&mut s), lcg(&mut s));
            assert!(pheno_in01(express(g)), "express produced non-finite/oob");
            // NaN genes are sanitised to 0
            let nan_g = Genotype { height_scale: f32::NAN, leaf_density: f32::INFINITY, cold_tolerance: 0.5, growth_rate: 0.5 };
            assert!(pheno_in01(express(nan_g)));
        }
    }

    #[test]
    fn identical_parents_breed_true() {
        // crossover of a genome with itself returns that genome (blend & pick both
        // collapse to the shared value) regardless of seed.
        let g = Genotype::new(0.31, 0.66, 0.42, 0.78);
        for seed in 0..50u64 {
            assert_eq!(crossover(g, g, seed), g, "self-cross drifted at seed {}", seed);
        }
    }

    #[test]
    fn quantised_trait_picks_a_parent_and_expresses_to_steps() {
        let a = Genotype::new(0.1, 0.20, 0.5, 0.5);
        let b = Genotype::new(0.9, 0.95, 0.5, 0.5);
        for seed in 0..64u64 {
            let child = crossover(a, b, seed);
            // leaf_density must be exactly one parent's value (picked, not blended)
            assert!(
                child.leaf_density == a.leaf_density || child.leaf_density == b.leaf_density,
                "leaf_density was blended, not picked: {}",
                child.leaf_density
            );
        }
        // expressed leaf density lands on a discrete step
        let step = 1.0 / (LEAF_DENSITY_STEPS - 1) as f32;
        let mut s = 0x1357_u64;
        for _ in 0..1000 {
            let lv = express(Genotype::new(0.0, lcg(&mut s), 0.0, 0.0)).leaf_density;
            let k = (lv / step).round();
            assert!((lv - k * step).abs() < 1e-6, "leaf_density not on a step: {}", lv);
        }
    }

    #[test]
    fn growth_rate_endpoints_and_monotone() {
        assert_eq!(express(Genotype::new(0.0, 0.0, 0.0, 0.0)).growth_rate, 0.0);
        assert_eq!(express(Genotype::new(0.0, 0.0, 0.0, 1.0)).growth_rate, 1.0);
        let mut prev = -1.0f32;
        let mut x = 0.0f32;
        while x <= 1.0 {
            let v = express(Genotype::new(0.0, 0.0, 0.0, x)).growth_rate;
            assert!(v >= prev - 1e-6, "growth_rate not monotone at {}", x);
            prev = v;
            x += 0.02;
        }
    }
}
