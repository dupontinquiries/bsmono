//! `sim::dragon_breeding` — cross two dragon genomes into an offspring genome,
//! decide a clutch size, and time the hatch (composing `hatch_time`). The egg's
//! genome then drives the forge model + anatomy at hatch like any other dragon,
//! and `incubation_imprint` can run over the incubation window for bonding.
//! Pure, deterministic, panic-free.

use crate::sim::dragon_archetype::{Genome, NUM_ELEMENTS};
use crate::sim::hatch_time::hatch_time;

/// Base hatch duration (game-seconds) at zero complexity / zero warmth.
pub const BASE_HATCH: f32 = 120.0;
pub const MAX_CLUTCH: u8 = 4;

fn splitmix64(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}
#[inline]
fn unit(state: &mut u64) -> f32 {
    (splitmix64(state) >> 40) as f32 / (1u64 << 24) as f32 // [0,1)
}
#[inline]
fn mix(dom: f32, rec: f32, jit: f32) -> f32 {
    (dom * 0.75 + rec * 0.25 + jit).clamp(0.0, 1.0)
}

/// Mendelian-ish cross: each gene takes mostly one parent (seeded coin) with a
/// touch of the other and a small mutation; element affinities blend per channel.
/// Deterministic in `(a, b, seed)`.
pub fn cross(a: &Genome, b: &Genome, seed: u64) -> Genome {
    let mut s = seed ^ 0xD3A6_0_u64.wrapping_mul(0x9E37_79B9);
    let mut gene = |x: f32, y: f32, s: &mut u64| {
        let coin = unit(s);
        let (dom, rec) = if coin < 0.5 { (x, y) } else { (y, x) };
        let jit = (unit(s) - 0.5) * 0.06; // ±0.03
        mix(dom, rec, jit)
    };
    let mut out = Genome::from_seed(seed); // shape; fields overwritten below
    out.bulk = gene(a.bulk, b.bulk, &mut s);
    out.neck_len = gene(a.neck_len, b.neck_len, &mut s);
    out.neck_arch = gene(a.neck_arch, b.neck_arch, &mut s);
    out.tail_len = gene(a.tail_len, b.tail_len, &mut s);
    out.leg_len = gene(a.leg_len, b.leg_len, &mut s);
    out.wing_span = gene(a.wing_span, b.wing_span, &mut s);
    out.wing_spread = gene(a.wing_spread, b.wing_spread, &mut s);
    out.pose_bias = gene(a.pose_bias, b.pose_bias, &mut s);
    out.muzzle_len = gene(a.muzzle_len, b.muzzle_len, &mut s);
    out.jaw_depth = gene(a.jaw_depth, b.jaw_depth, &mut s);
    out.snout_curve = gene(a.snout_curve, b.snout_curve, &mut s);
    out.brow = gene(a.brow, b.brow, &mut s);
    out.cheek = gene(a.cheek, b.cheek, &mut s);
    out.teeth = gene(a.teeth, b.teeth, &mut s);
    out.crest = gene(a.crest, b.crest, &mut s);
    out.horns = gene(a.horns, b.horns, &mut s);
    out.horn_sweep = gene(a.horn_sweep, b.horn_sweep, &mut s);
    for i in 0..NUM_ELEMENTS {
        let jit = (unit(&mut s) - 0.5) * 0.04;
        out.element[i] = ((a.element[i] + b.element[i]) * 0.5 + jit).clamp(0.0, 1.0);
    }
    out
}

/// Eggs in a clutch: leaner (low-bulk) dragons lay more, bulkier fewer. 1..=MAX_CLUTCH.
pub fn clutch_size(mother: &Genome, seed: u64) -> u8 {
    let mut s = seed ^ 0x0E66_C107_u64;
    let base = 1.0 + (1.0 - mother.bulk.clamp(0.0, 1.0)) * (MAX_CLUTCH as f32 - 1.0);
    let jit = (unit(&mut s) - 0.5) * 0.8;
    (base + jit).round().clamp(1.0, MAX_CLUTCH as f32) as u8
}

/// Morphological complexity (0..~2): more expressed ornament/structure ⇒ longer
/// to develop in the egg. Feeds `hatch_time`.
pub fn genome_complexity(g: &Genome) -> f32 {
    let sum = g.horns.clamp(0.0, 1.0)
        + g.crest.clamp(0.0, 1.0)
        + g.teeth.clamp(0.0, 1.0)
        + g.brow.clamp(0.0, 1.0)
        + g.wing_span.clamp(0.0, 1.0);
    (sum / 5.0) * 2.0
}

/// Hatch duration (game-seconds) for an egg given nest warmth (0..1). Warmer
/// nests hatch faster; more complex dragons take longer.
pub fn hatch_seconds(egg: &Genome, warmth01: f32) -> f32 {
    hatch_time(BASE_HATCH, genome_complexity(egg), warmth01)
}

/// Breed a full clutch from a mated pair: `clutch_size(mother, seed)` eggs, each an
/// independent [`cross`] of both parents under a distinct per-egg seed so siblings
/// vary while the whole clutch stays reproducible from `(mother, father, seed)`.
/// Pairs the previously-disjoint `clutch_size` and `cross` into one lay event; each
/// returned genome then drives hatch/anatomy like any dragon. Deterministic, panic-free.
pub fn breed_clutch(mother: &Genome, father: &Genome, seed: u64) -> Vec<Genome> {
    let n = clutch_size(mother, seed) as usize;
    let mut eggs = Vec::with_capacity(n);
    for i in 0..n {
        // distinct, stable per-egg seed so siblings differ yet the clutch reproduces.
        let egg_seed = seed ^ 0x51A3_C001_5EED_0001_u64.wrapping_mul(i as u64 + 1);
        eggs.push(cross(mother, father, egg_seed));
    }
    eggs
}

#[cfg(test)]
mod tests {
    use super::*;

    fn arr(g: &Genome) -> [f32; 17] {
        [
            g.bulk, g.neck_len, g.neck_arch, g.tail_len, g.leg_len, g.wing_span, g.wing_spread,
            g.pose_bias, g.muzzle_len, g.jaw_depth, g.snout_curve, g.brow, g.cheek, g.teeth,
            g.crest, g.horns, g.horn_sweep,
        ]
    }

    #[test]
    fn cross_in_range_and_deterministic() {
        let a = Genome::from_seed(11);
        let b = Genome::from_seed(22);
        let c1 = cross(&a, &b, 7);
        let c2 = cross(&a, &b, 7);
        for v in arr(&c1) {
            assert!((0.0..=1.0).contains(&v) && v.is_finite());
        }
        for v in c1.element {
            assert!((0.0..=1.0).contains(&v));
        }
        assert_eq!(arr(&c1), arr(&c2), "deterministic");
        assert_eq!(c1.element, c2.element);
    }

    #[test]
    fn identical_parents_breed_true() {
        let p = Genome::from_seed(9);
        let c = cross(&p, &p, 3);
        for (cv, pv) in arr(&c).iter().zip(arr(&p).iter()) {
            assert!((cv - pv).abs() <= 0.04, "child of identical parents ≈ parent ({cv} vs {pv})");
        }
    }

    #[test]
    fn element_blends_between_parents() {
        let mut a = Genome::from_seed(1);
        let mut b = Genome::from_seed(1);
        for i in 0..NUM_ELEMENTS { a.element[i] = 0.2; b.element[i] = 0.8; }
        let c = cross(&a, &b, 5);
        for i in 0..NUM_ELEMENTS {
            assert!(c.element[i] > 0.2 - 0.05 && c.element[i] < 0.8 + 0.05, "element between parents");
        }
    }

    #[test]
    fn clutch_size_in_range_and_leaner_lays_more() {
        let mut lean = Genome::from_seed(4); lean.bulk = 0.0;
        let mut hefty = Genome::from_seed(4); hefty.bulk = 1.0;
        let mut lean_total = 0u32;
        let mut hefty_total = 0u32;
        for s in 0..50u64 {
            let l = clutch_size(&lean, s);
            let h = clutch_size(&hefty, s);
            assert!((1..=MAX_CLUTCH).contains(&l) && (1..=MAX_CLUTCH).contains(&h));
            lean_total += l as u32;
            hefty_total += h as u32;
        }
        assert!(lean_total > hefty_total, "leaner dragons lay larger clutches on average");
    }

    #[test]
    fn hatch_faster_when_warm_longer_when_complex() {
        let egg = Genome::from_seed(8);
        assert!(hatch_seconds(&egg, 1.0) < hatch_seconds(&egg, 0.0), "warm nest hatches sooner");
        let mut simple = Genome::from_seed(8);
        simple.horns = 0.0; simple.crest = 0.0; simple.teeth = 0.0; simple.brow = 0.0; simple.wing_span = 0.0;
        let mut ornate = Genome::from_seed(8);
        ornate.horns = 1.0; ornate.crest = 1.0; ornate.teeth = 1.0; ornate.brow = 1.0; ornate.wing_span = 1.0;
        assert!(hatch_seconds(&ornate, 0.5) > hatch_seconds(&simple, 0.5), "ornate dragons take longer");
        assert!(hatch_seconds(&egg, 0.5).is_finite());
    }

    #[test]
    fn clutch_matches_clutch_size_and_is_deterministic() {
        let m = Genome::from_seed(100);
        let f = Genome::from_seed(200);
        let n = clutch_size(&m, 77) as usize;
        let c1 = breed_clutch(&m, &f, 77);
        let c2 = breed_clutch(&m, &f, 77);
        assert_eq!(c1.len(), n, "clutch length follows clutch_size(mother, seed)");
        assert!((1..=MAX_CLUTCH as usize).contains(&c1.len()));
        for (e1, e2) in c1.iter().zip(c2.iter()) {
            assert_eq!(arr(e1), arr(e2), "same (mother,father,seed) reproduces the clutch");
            assert_eq!(e1.element, e2.element);
        }
    }

    #[test]
    fn clutch_siblings_differ_and_stay_in_range() {
        let mut m = Genome::from_seed(3);
        m.bulk = 0.0; // leaner mother → a full clutch
        let f = Genome::from_seed(4);
        let clutch = breed_clutch(&m, &f, 555);
        assert!(clutch.len() >= 2, "lean mother should lay multiple eggs");
        let differ = arr(&clutch[0]) != arr(&clutch[1]) || clutch[0].element != clutch[1].element;
        assert!(differ, "distinct per-egg seeds should vary siblings");
        for egg in &clutch {
            for v in arr(egg) {
                assert!((0.0..=1.0).contains(&v) && v.is_finite());
            }
            for v in egg.element {
                assert!((0.0..=1.0).contains(&v));
            }
        }
    }
}
