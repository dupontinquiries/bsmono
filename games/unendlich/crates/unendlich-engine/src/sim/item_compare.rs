//! `sim::item_compare` — equipped-vs-candidate tooltip comparison
//! (UI Tooltip doc §5). Produces signed per-affix and per-gene deltas plus a
//! rarity delta, so the tooltip can show up/down arrows — and so breeders see
//! heritable-gene deltas, not just combat stats.
//!
//! Mobile: every type here is a fixed-size, `Copy` stack struct and `compare`
//! does ZERO allocation, so it can run on every hover with no heap churn or GC
//! pressure on a memory-tight (8 GB) mobile SoC.
//!
//! Pure and deterministic: field-by-field arithmetic only, no GPU/IO/clock/RNG,
//! no `.unwrap()`/`.expect()`.
//!
//! ── DIMENSIONS / verified gap (read before merge) ────────────────────────────
//! The brief said to reuse `N_AFFIX` / `N_GENE` from `item_genetics.rs`, but
//! that module defines **no such constants** (and no affix concept): its genome
//! is five *named* traits (`ItemGenes` → `traits()` returns `[f32; 5]`), and
//! affixes are a *dynamic* count elsewhere (`sim::affix_count`, parameterised by
//! `max_affixes`, conventionally 6 across its tests). So the dimensions below
//! are declared here and **grounded** in the real code:
//!   * `N_GENE  = 5` — the five `ItemGenes` traits.
//!   * `N_AFFIX = 6` — the standard `max_affixes` cap used by `affix_count`.
//! If the coordinator establishes canonical constants (e.g. `item_genetics::
//! N_GENE`), re-point the two `pub const`s below to them — that is the only
//! change needed; the comparison logic is dimension-agnostic.

/// Number of heritable genes compared (the five `ItemGenes` traits).
pub const N_GENE: usize = 5;
/// Number of affix slots compared (the standard `affix_count` `max_affixes`).
pub const N_AFFIX: usize = 6;

/// A compact, `Copy` snapshot of an item for comparison.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ItemView {
    /// Integer affix magnitudes, one per slot.
    pub affixes: [i32; N_AFFIX],
    /// Heritable gene values (typically in `[0, 1]`), one per trait.
    pub genes: [f32; N_GENE],
    /// Rarity rank (higher = rarer).
    pub rarity: u8,
}

/// Signed `candidate - equipped` deltas for the tooltip's up/down arrows.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Delta {
    /// Per-affix `candidate - equipped`, **saturating** in `i32` (see below).
    pub affix_delta: [i32; N_AFFIX],
    /// Per-gene `candidate - equipped` (plain `f32` subtraction; finite in,
    /// finite out).
    pub gene_delta: [f32; N_GENE],
    /// Rarity `candidate - equipped`, **saturating** into `i8` (a `u8`
    /// difference spans `-255..=255`, wider than `i8`, so it clamps to
    /// `-128..=127`). Sign is always correct; `0` iff equal.
    pub rarity_delta: i8,
}

/// Field-by-field `candidate - equipped`.
///
/// Affix deltas use `i32::saturating_sub` — extreme inputs (e.g. `i32::MAX`
/// minus `i32::MIN`) saturate to the `i32` bound instead of panicking/wrapping.
/// Rarity is widened to `i16` then saturated into `i8`. Gene deltas are a plain
/// `f32` subtraction. No allocation, no panic.
pub fn compare(equipped: &ItemView, candidate: &ItemView) -> Delta {
    let mut affix_delta = [0i32; N_AFFIX];
    for i in 0..N_AFFIX {
        affix_delta[i] = candidate.affixes[i].saturating_sub(equipped.affixes[i]);
    }

    let mut gene_delta = [0.0f32; N_GENE];
    for i in 0..N_GENE {
        gene_delta[i] = candidate.genes[i] - equipped.genes[i];
    }

    let rarity_diff = candidate.rarity as i16 - equipped.rarity as i16;
    let rarity_delta = rarity_diff.clamp(i8::MIN as i16, i8::MAX as i16) as i8;

    Delta { affix_delta, gene_delta, rarity_delta }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn view(affix: i32, gene: f32, rarity: u8) -> ItemView {
        ItemView { affixes: [affix; N_AFFIX], genes: [gene; N_GENE], rarity }
    }

    #[test]
    fn identical_is_all_zero() {
        let a = view(7, 0.5, 3);
        let d = compare(&a, &a);
        assert_eq!(d.affix_delta, [0; N_AFFIX]);
        assert_eq!(d.gene_delta, [0.0; N_GENE]);
        assert_eq!(d.rarity_delta, 0);
    }

    #[test]
    fn strictly_better_is_positive() {
        let eq = view(2, 0.2, 1);
        let cand = view(9, 0.8, 5);
        let d = compare(&eq, &cand);
        assert!(d.affix_delta.iter().all(|&x| x > 0));
        assert!(d.gene_delta.iter().all(|&x| x > 0.0));
        assert!(d.rarity_delta > 0);
    }

    #[test]
    fn strictly_worse_is_negative() {
        let eq = view(9, 0.9, 7);
        let cand = view(1, 0.1, 2);
        let d = compare(&eq, &cand);
        assert!(d.affix_delta.iter().all(|&x| x < 0));
        assert!(d.gene_delta.iter().all(|&x| x < 0.0));
        assert!(d.rarity_delta < 0);
    }

    #[test]
    fn gene_delta_sign_and_magnitude() {
        let mut eq = view(0, 0.0, 0);
        let mut cand = view(0, 0.0, 0);
        eq.genes = [0.10, 0.50, 0.90, 0.25, 0.75];
        cand.genes = [0.40, 0.20, 0.90, 0.30, 0.05];
        let d = compare(&eq, &cand);
        let expect = [0.30f32, -0.30, 0.0, 0.05, -0.70];
        for i in 0..N_GENE {
            assert!((d.gene_delta[i] - expect[i]).abs() < 1e-6, "gene {} : {} vs {}", i, d.gene_delta[i], expect[i]);
        }
    }

    #[test]
    fn affix_delta_exact_values() {
        let mut eq = view(0, 0.0, 0);
        let mut cand = view(0, 0.0, 0);
        eq.affixes = [10, 0, -5, 100, 3, 7];
        cand.affixes = [15, -2, -5, 40, 9, 7];
        let d = compare(&eq, &cand);
        assert_eq!(d.affix_delta, [5, -2, 0, -60, 6, 0]);
    }

    #[test]
    fn rarity_delta_equal_is_zero_and_signs() {
        assert_eq!(compare(&view(0, 0.0, 4), &view(0, 0.0, 4)).rarity_delta, 0);
        assert!(compare(&view(0, 0.0, 2), &view(0, 0.0, 9)).rarity_delta > 0);
        assert!(compare(&view(0, 0.0, 9), &view(0, 0.0, 2)).rarity_delta < 0);
        // saturation: full u8 span clamps into i8 with correct sign
        assert_eq!(compare(&view(0, 0.0, 0), &view(0, 0.0, 255)).rarity_delta, 127);
        assert_eq!(compare(&view(0, 0.0, 255), &view(0, 0.0, 0)).rarity_delta, -128);
    }

    #[test]
    fn no_panic_on_extreme_i32() {
        let mut eq = view(0, 0.0, 0);
        let mut cand = view(0, 0.0, 0);
        eq.affixes = [i32::MIN, i32::MAX, 0, i32::MIN, i32::MAX, 0];
        cand.affixes = [i32::MAX, i32::MIN, i32::MAX, i32::MIN, 0, i32::MIN];
        let d = compare(&eq, &cand); // must not panic / wrap-around UB
        // saturating_sub: MAX-MIN -> MAX ; MIN-MAX -> MIN
        assert_eq!(d.affix_delta[0], i32::MAX);
        assert_eq!(d.affix_delta[1], i32::MIN);
        assert_eq!(d.affix_delta[3], 0); // MIN - MIN
    }

    #[test]
    fn antisymmetry_on_genes_and_affixes() {
        // deterministic seeded sweep (R20)
        let mut x: u64 = 0xC0FFEE_17_u64; // documented seed
        let mut next = || { x = x.wrapping_mul(6364136223846793005).wrapping_add(1); (x >> 33) as u32 };
        for _ in 0..3_000 {
            let mut a = view(0, 0.0, 0);
            let mut b = view(0, 0.0, 0);
            for i in 0..N_AFFIX {
                a.affixes[i] = (next() % 2001) as i32 - 1000;
                b.affixes[i] = (next() % 2001) as i32 - 1000;
            }
            for i in 0..N_GENE {
                a.genes[i] = (next() % 1000) as f32 / 1000.0;
                b.genes[i] = (next() % 1000) as f32 / 1000.0;
            }
            a.rarity = (next() % 256) as u8;
            b.rarity = (next() % 256) as u8;
            let d = compare(&a, &b);
            let dr = compare(&b, &a);
            for i in 0..N_AFFIX {
                // small magnitudes here, so plain negation holds (no saturation)
                assert_eq!(d.affix_delta[i], -dr.affix_delta[i]);
                assert!(d.affix_delta[i].is_positive() == (b.affixes[i] > a.affixes[i]) || d.affix_delta[i] == 0);
            }
            for i in 0..N_GENE {
                assert!((d.gene_delta[i] + dr.gene_delta[i]).abs() < 1e-6);
                assert!(d.gene_delta[i].is_finite());
            }
        }
    }

    #[test]
    fn deterministic() {
        let eq = view(3, 0.33, 2);
        let cand = view(8, 0.66, 6);
        assert_eq!(compare(&eq, &cand), compare(&eq, &cand));
    }
}
