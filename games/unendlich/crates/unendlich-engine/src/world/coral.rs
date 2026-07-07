//! `coral` — coral / aquatic-flora genome.
//!
//! Worker-03 / WORLD-PERF pure core, round 4. Mirrors the plant-genetics
//! pattern (heritable genes → expressed phenotype, breeding, mutation) for reef
//! flora that grows on the `aquatic_materials` coral substrate. Kept as a
//! self-contained haploid 3-gene model (hue / branchiness / spread) so it
//! compiles + tests standalone; the coordinator maps it onto the engine's
//! diploid `genetics` when wiring.
//!
//! Pure & deterministic (seed-driven, no rng); no clock/unwrap. Genes are kept
//! in [0,1]; breeding stays within the parents' range; mutation is bounded.
#![allow(dead_code)]

/// Maximum per-gene change a single mutation may apply (before clamping).
pub const MUT_MAX: f32 = 0.08;

/// A coral's heritable genes, each in [0,1].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CoralGenome {
    pub hue: f32,
    pub branchiness: f32,
    pub spread: f32,
}

/// Expressed, renderable traits.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CoralPhenotype {
    /// Number of primary branches (1..=12).
    pub branches: u8,
    /// Spread radius in cells (1..=8).
    pub radius: u8,
    /// RGB tint in [0,1]³ derived from hue.
    pub rgb: [f32; 3],
}

impl CoralGenome {
    /// A wild coral genome seeded deterministically.
    pub fn from_seed(seed: u64) -> Self {
        CoralGenome {
            hue: unit(mix(seed ^ 0x1111)),
            branchiness: unit(mix(seed ^ 0x2222)),
            spread: unit(mix(seed ^ 0x3333)),
        }
    }

    /// Express genes into phenotype (deterministic).
    pub fn express(&self) -> CoralPhenotype {
        let branches = 1 + (clamp01(self.branchiness) * 11.0 + 0.5) as u8; // 1..=12
        let radius = 1 + (clamp01(self.spread) * 7.0 + 0.5) as u8; // 1..=8
        CoralPhenotype {
            branches,
            radius,
            rgb: hue_to_rgb(clamp01(self.hue)),
        }
    }

    /// Breed two genomes. Each child gene is a deterministic interpolation
    /// between the parents' values, so it always lies within `[min, max]` of
    /// the two parents (no mutation here — see `mutate`).
    pub fn breed(&self, other: &CoralGenome, seed: u64) -> CoralGenome {
        CoralGenome {
            hue: lerp_within(self.hue, other.hue, unit(mix(seed ^ 0xA1))),
            branchiness: lerp_within(self.branchiness, other.branchiness, unit(mix(seed ^ 0xB2))),
            spread: lerp_within(self.spread, other.spread, unit(mix(seed ^ 0xC3))),
        }
    }

    /// Mutate each gene by at most `±MUT_MAX`, then clamp to [0,1]. The result
    /// differs from `self` by at most `MUT_MAX` per gene.
    pub fn mutate(&self, seed: u64) -> CoralGenome {
        CoralGenome {
            hue: clamp01(self.hue + signed(mix(seed ^ 0xD4)) * MUT_MAX),
            branchiness: clamp01(self.branchiness + signed(mix(seed ^ 0xE5)) * MUT_MAX),
            spread: clamp01(self.spread + signed(mix(seed ^ 0xF6)) * MUT_MAX),
        }
    }
}

// --- deterministic helpers (local splitmix) ---

fn mix(x: u64) -> u64 {
    let mut z = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// Hash → [0,1).
fn unit(h: u64) -> f32 {
    ((h >> 40) as f32) / ((1u64 << 24) as f32)
}

/// Hash → [-1,1).
fn signed(h: u64) -> f32 {
    unit(h) * 2.0 - 1.0
}

fn clamp01(v: f32) -> f32 {
    crate::util::mathx::clamp01(v) // r343 dedup
}

/// Interpolate between a and b by t∈[0,1]; result is within [min(a,b),max(a,b)].
fn lerp_within(a: f32, b: f32, t: f32) -> f32 {
    let a = clamp01(a);
    let b = clamp01(b);
    let t = clamp01(t);
    a + (b - a) * t
}

fn hue_to_rgb(h: f32) -> [f32; 3] {
    // Simple 6-sector HSV (S=V=1) → RGB; deterministic, finite.
    let x = h * 6.0;
    let i = x as i32 % 6;
    let f = x - (x.floor());
    let q = 1.0 - f;
    match i {
        0 => [1.0, f, 0.0],
        1 => [q, 1.0, 0.0],
        2 => [0.0, 1.0, f],
        3 => [0.0, q, 1.0],
        4 => [f, 0.0, 1.0],
        _ => [1.0, 0.0, q],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn genes(g: &CoralGenome) -> [f32; 3] {
        [g.hue, g.branchiness, g.spread]
    }

    #[test]
    fn prop_breed_within_parents_range() {
        for s in 0..200u64 {
            let a = CoralGenome::from_seed(s);
            let b = CoralGenome::from_seed(s ^ 0xDEAD_BEEF);
            let child = a.breed(&b, s);
            let (ga, gb, gc) = (genes(&a), genes(&b), genes(&child));
            for k in 0..3 {
                let lo = ga[k].min(gb[k]);
                let hi = ga[k].max(gb[k]);
                assert!(
                    gc[k] >= lo - 1e-6 && gc[k] <= hi + 1e-6,
                    "bred gene out of parents' range"
                );
            }
        }
    }

    #[test]
    fn prop_mutate_bounded() {
        for s in 0..200u64 {
            let g = CoralGenome::from_seed(s);
            let m = g.mutate(s ^ 0x55);
            let (gg, mm) = (genes(&g), genes(&m));
            for k in 0..3 {
                assert!((mm[k] - gg[k]).abs() <= MUT_MAX + 1e-6, "mutation exceeded bound");
                assert!(mm[k] >= 0.0 && mm[k] <= 1.0, "gene left [0,1]");
            }
        }
    }

    #[test]
    fn prop_deterministic() {
        for s in 0..64u64 {
            let a = CoralGenome::from_seed(s);
            let b = CoralGenome::from_seed(s);
            assert_eq!(a, b);
            assert_eq!(a.express(), b.express());
            let c = CoralGenome::from_seed(s ^ 7);
            assert_eq!(a.breed(&c, s), a.breed(&c, s));
            assert_eq!(a.mutate(s), a.mutate(s));
        }
    }

    #[test]
    fn express_in_documented_ranges() {
        for s in 0..256u64 {
            let p = CoralGenome::from_seed(s).express();
            assert!((1..=12).contains(&p.branches));
            assert!((1..=8).contains(&p.radius));
            for c in p.rgb {
                assert!(c >= 0.0 && c <= 1.0 && c.is_finite());
            }
        }
    }

    #[test]
    fn genes_stay_in_unit_range() {
        for s in 0..256u64 {
            let g = CoralGenome::from_seed(s);
            for v in genes(&g) {
                assert!(v >= 0.0 && v < 1.0001);
            }
        }
    }
}
