//! item_genetics — evolving items. An item carries a small genome of traits in
//! `[0, 1]` that `express()` into concrete stats (a blade's damage, weight,
//! durability, reach, worth). Items can `mutate` and `breed`, so a lineage of
//! crafted/found gear drifts and recombines across generations — the same genome
//! pattern the creature side uses, applied to loot. Self-contained and pure:
//! seeded deterministic mixing only, no clock, no rng crate.

/// Item genome: five heritable traits, each in `[0, 1]`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ItemGenes {
    /// Edge keenness → damage.
    pub sharpness: f32,
    /// Mass of the piece → weight (and a little damage).
    pub heft: f32,
    /// Point of balance → reach and handling.
    pub balance: f32,
    /// Material toughness → durability.
    pub resilience: f32,
    /// Ornament/craft → worth.
    pub flair: f32,
}

/// Concrete expressed stats.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ItemStats {
    pub damage: f32,
    pub weight: f32,
    pub durability: f32,
    pub reach: f32,
    pub value: f32,
}

/// SplitMix64 — a pure, seedable bit mixer (no rng crate, fully deterministic).
fn mix(x: u64) -> u64 {
    // r342 dedup: bit-identical to the canonical finalizer — delegate.
    crate::util::hash_util::mix64(x)
}
/// A `[0, 1)` float from a seed and a salt.
fn unit(seed: u64, salt: u64) -> f32 {
    let h = mix(seed ^ salt.wrapping_mul(0xD1B5_4A32_D192_ED03));
    ((h >> 40) as f32) / ((1u64 << 24) as f32)
}

fn clamp01(v: f32) -> f32 {
    v.clamp(0.0, 1.0)
}

impl ItemGenes {
    /// A neutral, mid-range item.
    pub fn balanced() -> ItemGenes {
        ItemGenes { sharpness: 0.5, heft: 0.5, balance: 0.5, resilience: 0.5, flair: 0.5 }
    }

    /// Map traits to concrete stats. Monotone in each trait so a "better" gene is
    /// never a worse item.
    pub fn express(&self) -> ItemStats {
        ItemStats {
            damage: 1.0 + self.sharpness * 9.0 + self.heft * 2.0, // 1..12
            weight: 0.5 + self.heft * 4.5,                        // 0.5..5
            durability: 10.0 + self.resilience * 90.0,            // 10..100
            reach: 0.8 + self.balance * 1.2,                      // 0.8..2.0
            value: (self.sharpness + self.resilience + self.flair) / 3.0 * 100.0, // 0..100
        }
    }

    /// A mutated copy: each trait jittered by up to ±`rate` (deterministic from
    /// `seed`), clamped to `[0, 1]`.
    pub fn mutate(&self, seed: u64, rate: f32) -> ItemGenes {
        let r = rate.clamp(0.0, 1.0);
        let j = |salt: u64, base: f32| clamp01(base + (unit(seed, salt) * 2.0 - 1.0) * r);
        ItemGenes {
            sharpness: j(0x01, self.sharpness),
            heft: j(0x02, self.heft),
            balance: j(0x03, self.balance),
            resilience: j(0x04, self.resilience),
            flair: j(0x05, self.flair),
        }
    }

    /// Breed two genomes: each child trait is a seeded blend of the parents', so
    /// it always lies between them (no mutation here — compose with [`mutate`]
    /// for drift). Deterministic in `seed`.
    pub fn breed(a: &ItemGenes, b: &ItemGenes, seed: u64) -> ItemGenes {
        let blend = |salt: u64, pa: f32, pb: f32| {
            let t = unit(seed, salt);
            pa + (pb - pa) * t
        };
        ItemGenes {
            sharpness: blend(0x11, a.sharpness, b.sharpness),
            heft: blend(0x12, a.heft, b.heft),
            balance: blend(0x13, a.balance, b.balance),
            resilience: blend(0x14, a.resilience, b.resilience),
            flair: blend(0x15, a.flair, b.flair),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lcg(seed: &mut u32) -> f32 {
        *seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
        ((*seed >> 8) as f32) / ((1u32 << 24) as f32)
    }
    fn rand_genes(seed: &mut u32) -> ItemGenes {
        ItemGenes {
            sharpness: lcg(seed),
            heft: lcg(seed),
            balance: lcg(seed),
            resilience: lcg(seed),
            flair: lcg(seed),
        }
    }
    fn traits(g: &ItemGenes) -> [f32; 5] {
        [g.sharpness, g.heft, g.balance, g.resilience, g.flair]
    }

    #[test]
    fn express_is_monotone_and_ordered() {
        let weak = ItemGenes { sharpness: 0.0, heft: 0.0, balance: 0.0, resilience: 0.0, flair: 0.0 };
        let strong = ItemGenes { sharpness: 1.0, heft: 1.0, balance: 1.0, resilience: 1.0, flair: 1.0 };
        let (w, s) = (weak.express(), strong.express());
        assert!(s.damage > w.damage && s.durability > w.durability && s.reach > w.reach);
    }

    #[test]
    fn breeding_is_deterministic_for_a_fixed_seed() {
        let a = ItemGenes { sharpness: 0.2, heft: 0.8, balance: 0.4, resilience: 0.6, flair: 0.1 };
        let b = ItemGenes { sharpness: 0.9, heft: 0.1, balance: 0.5, resilience: 0.3, flair: 0.7 };
        assert_eq!(ItemGenes::breed(&a, &b, 42), ItemGenes::breed(&a, &b, 42), "same seed → same child");
        assert!(ItemGenes::breed(&a, &b, 42) != ItemGenes::breed(&a, &b, 43), "different seed → varies");
    }

    #[test]
    fn prop_bred_traits_lie_between_parents() {
        let mut seed = 0x1234_5678u32;
        for _ in 0..5000 {
            let a = rand_genes(&mut seed);
            let b = rand_genes(&mut seed);
            let s = (lcg(&mut seed) * 1e9) as u64;
            let child = ItemGenes::breed(&a, &b, s);
            let (ta, tb, tc) = (traits(&a), traits(&b), traits(&child));
            for i in 0..5 {
                let lo = ta[i].min(tb[i]) - 1e-6;
                let hi = ta[i].max(tb[i]) + 1e-6;
                assert!(tc[i] >= lo && tc[i] <= hi, "trait {i} {} not between {lo}..{hi}", tc[i]);
                assert!(tc[i].is_finite());
            }
        }
    }

    #[test]
    fn prop_mutate_stays_in_bounds_and_finite() {
        let mut seed = 0x9E37_79B9u32;
        for _ in 0..5000 {
            let g = rand_genes(&mut seed);
            let rate = lcg(&mut seed);
            let s = (lcg(&mut seed) * 1e9) as u64;
            let m = g.mutate(s, rate);
            for t in traits(&m) {
                assert!(t >= 0.0 && t <= 1.0, "mutated trait in [0,1]: {t}");
                assert!(t.is_finite());
            }
            // expressed stats also finite
            let st = m.express();
            assert!(st.damage.is_finite() && st.weight.is_finite() && st.durability.is_finite());
        }
    }
}
