//! Genome → **archetype** classification (R37, DRG) — the bridge from the
//! `dragon_forge_3d.html` Three.js authoring viewer to the game.
//!
//! `dragon_blueprint_from_genome.rs` expands a genome into the *continuous*
//! phenotype (8-part blueprint). This module is the *categorical* layer above
//! it: it classifies a viewer-authored genome into a recognizable
//! [`Archetype`] so the engine can spawn, name, and role a dialed-in dragon as
//! a typed entity. It composes with `dragon_elemental` / `dragon_signature` /
//! `dragon_attunement` (R34).
//!
//! Genome representation: `DRAGON_GENOME.md` declares the viewer's parametric
//! space to be the canonical genome. No single existing Rust type carries all
//! of BODY + SKULL + ELEMENT together (`dragon_blueprint_from_genome::BodyGenes`
//! is the body subset), so — as the brief permits — [`Genome`] here is a
//! *faithful named-field* mirror of that documented space (same gene names,
//! same normalised `[0,1]` ranges). It invents no new encoding: the body field
//! names match `BodyGenes`, the skull/element names match `DRAGON_GENOME.md`.
//!
//! Pure & deterministic: a dragon dialed in the viewer classifies identically
//! in-game, every call. No RNG, I/O, GPU, or wall-clock; out-of-range / non-
//! finite genes are clamped; never panics.

/// Number of discrete elements (`DRAGON_GENOME.md` ELEMENT order).
pub const NUM_ELEMENTS: usize = 6;
/// Canonical element index order: `0=Wood, 1=Water, 2=Air, 3=Dust, 4=Fire,
/// 5=Storm` — matches `DRAGON_GENOME.md` and `dragon_attunement::Element`.
pub const ELEMENT_NAMES: [&str; NUM_ELEMENTS] =
    ["Wood", "Water", "Air", "Dust", "Fire", "Storm"];

/// Number of morphological [`Build`] families.
pub const NUM_BUILDS: usize = 5;
/// Number of discrete `size_class` buckets (`0..=SIZE_CLASS_MAX`).
pub const SIZE_CLASS_MAX: u8 = 4;

// --- salient-trait flag bits (Archetype.flags) ---
/// At least one horn pair present.
pub const FLAG_HORNED: u32 = 1 << 0;
/// Pronounced dorsal crest.
pub const FLAG_CRESTED: u32 = 1 << 1;
/// Prominent teeth / fanged.
pub const FLAG_TOOTHED: u32 = 1 << 2;
/// Heavy brow ridge.
pub const FLAG_BROW_RIDGED: u32 = 1 << 3;
/// Broad wingspan (notably winged).
pub const FLAG_BROAD_WINGED: u32 = 1 << 4;
/// Elongated tail.
pub const FLAG_LONG_TAILED: u32 = 1 << 5;

/// Threshold above which a normalised skull/body gene counts as a "salient"
/// trait for flag purposes.
pub const TRAIT_THRESHOLD: f32 = 0.6;

/// Morphological family inferred from body proportions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Build {
    Serpentine,
    Bruiser,
    Flyer,
    Stalker,
    Sovereign,
}

impl Build {
    /// Stable index in declaration order (used in the archetype id and for
    /// deterministic tie-breaking).
    pub fn index(self) -> u8 {
        match self {
            Build::Serpentine => 0,
            Build::Bruiser => 1,
            Build::Flyer => 2,
            Build::Stalker => 3,
            Build::Sovereign => 4,
        }
    }
}

/// A viewer-authored genome, as the faithful named-field mirror of the
/// canonical `DRAGON_GENOME.md` space. All genes normalised `[0,1]`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Genome {
    // BODY — torso/limb proportions (names match `BodyGenes`, plus pose_bias).
    pub bulk: f32,
    pub neck_len: f32,
    pub neck_arch: f32,
    pub tail_len: f32,
    pub leg_len: f32,
    pub wing_span: f32,
    pub wing_spread: f32,
    pub pose_bias: f32,
    // SKULL — bone-box face.
    pub muzzle_len: f32,
    pub jaw_depth: f32,
    pub snout_curve: f32,
    pub brow: f32,
    pub cheek: f32,
    pub teeth: f32,
    pub crest: f32,
    /// Horns, normalised; expresses `0..4` pairs (see [`horn_pairs`]).
    pub horns: f32,
    pub horn_sweep: f32,
    // ELEMENT — per-element affinity; dominant element = argmax. Index order
    // per [`ELEMENT_NAMES`] (Wood..Storm).
    pub element: [f32; NUM_ELEMENTS],
}

impl Genome {
    /// A deterministic, varied genome from a seed. Every gene is drawn uniformly
    /// in `[0,1]` (the engine's normalised convention; neutral = 0.5), with one
    /// element channel made dominant so the dragon reads as a clear element.
    /// Pure: a `splitmix64` walk, no RNG state or wall-clock.
    pub fn from_seed(seed: u64) -> Genome {
        let mut s = seed ^ 0x9E37_79B9_7F4A_7C15;
        let mut next01 = || {
            // splitmix64
            s = s.wrapping_add(0x9E37_79B9_7F4A_7C15);
            let mut z = s;
            z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
            z ^= z >> 31;
            // top 24 bits → [0,1)
            ((z >> 40) as f32) / ((1u32 << 24) as f32)
        };
        let mut element = [0.0f32; NUM_ELEMENTS];
        for e in element.iter_mut() {
            *e = next01() * 0.35; // low background for every element
        }
        let dominant = (next01() * NUM_ELEMENTS as f32) as usize % NUM_ELEMENTS;
        element[dominant] = 0.7 + next01() * 0.3; // a clear dominant channel

        Genome {
            bulk: next01(),
            neck_len: next01(),
            neck_arch: next01(),
            tail_len: next01(),
            leg_len: next01(),
            wing_span: next01(),
            wing_spread: next01(),
            pose_bias: next01(),
            muzzle_len: next01(),
            jaw_depth: next01(),
            snout_curve: next01(),
            brow: next01(),
            cheek: next01(),
            teeth: next01(),
            crest: next01(),
            horns: next01(),
            horn_sweep: next01(),
            element,
        }
    }

    /// Like [`from_seed`] but forces a chosen dominant element (`0=Wood..5=Storm`)
    /// so a trait-selected egg breeds true — e.g. a vine/earthy dragon picks Wood
    /// and reads green. Other element channels are kept low. `element_idx` is
    /// taken modulo the element count, so it never panics.
    pub fn from_seed_element(seed: u64, element_idx: u8) -> Genome {
        let mut g = Genome::from_seed(seed);
        let idx = (element_idx as usize) % NUM_ELEMENTS;
        for (i, e) in g.element.iter_mut().enumerate() {
            *e = if i == idx { 0.85 + (*e) * 0.15 } else { (*e) * 0.25 };
        }
        g
    }

    /// The 17 morphological genes as a flat vector, for similarity / mate-compat
    /// scoring (e.g. `mate_select::compat`). Element affinities are excluded
    /// (they're compared separately); order matches the breeding cross.
    pub fn traits(&self) -> [f32; 17] {
        [
            self.bulk, self.neck_len, self.neck_arch, self.tail_len, self.leg_len,
            self.wing_span, self.wing_spread, self.pose_bias, self.muzzle_len, self.jaw_depth,
            self.snout_curve, self.brow, self.cheek, self.teeth, self.crest, self.horns,
            self.horn_sweep,
        ]
    }
}

/// A typed game archetype derived from a genome.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Archetype {
    /// Stable composite id = `element * NUM_BUILDS + build.index()` (`0..30`).
    pub id: u8,
    /// Dominant element index (`0=Wood..5=Storm`).
    pub element: u8,
    pub build: Build,
    /// Size bucket, monotone in `bulk` (`0..=SIZE_CLASS_MAX`).
    pub size_class: u8,
    /// Bit-set of salient traits (`FLAG_*`).
    pub flags: u32,
}

/// Non-finite → 0.0, then clamp to `[0,1]`. Defined unconditionally (used on
/// every classification path, including release builds).
#[inline]
fn g01(v: f32) -> f32 {
    if v.is_finite() {
        v.clamp(0.0, 1.0)
    } else {
        0.0
    }
}

/// Quantised horn-pair count (`0..=4`) from the normalised `horns` gene, per
/// `DRAGON_GENOME.md` ("horns — quantised 0..4 pairs").
pub fn horn_pairs(g: &Genome) -> u8 {
    (g01(g.horns) * 4.0).round() as u8
}

/// Dominant element index = argmax over the element affinities. Ties resolve to
/// the lowest index (deterministic).
fn dominant_element(g: &Genome) -> u8 {
    let mut best_i = 0usize;
    let mut best_v = g01(g.element[0]);
    let mut i = 1;
    while i < NUM_ELEMENTS {
        let v = g01(g.element[i]);
        if v > best_v {
            best_v = v;
            best_i = i;
        }
        i += 1;
    }
    best_i as u8
}

/// Size bucket, monotone non-decreasing in `bulk`.
fn size_class_of(g: &Genome) -> u8 {
    (g01(g.bulk) * SIZE_CLASS_MAX as f32).round() as u8
}

// ── r336-dragon-size ─────────────────────────────────────────────────────────
// Restore the small→huge size range. `size_class_of` buckets the genome into
// `0..=SIZE_CLASS_MAX`; this maps that bucket to a whole-body **rest**-dimension
// multiplier the forge applies to its base dimensions (BEFORE the r332
// anim-balloon clamp, which bounds per-axis *animation* growth relative to rest,
// not absolute size — see HANDOFF_dragon_size.md for the one forge call-site).

/// Smallest body-scale multiplier (size_class 0) — a small dragon.
pub const SIZE_SCALE_MIN: f32 = 0.45;
/// Largest body-scale multiplier (size_class `SIZE_CLASS_MAX`) — a huge dragon.
pub const SIZE_SCALE_MAX: f32 = 3.0;

/// Whole-body rest-scale for a `size_class` bucket, spanning a small→huge range
/// (`SIZE_SCALE_MIN`..`SIZE_SCALE_MAX`, ≈6.7× from smallest to largest).
///
/// Geometric ramp: each class is a constant multiplicative step over the last
/// (≈1.6×), so the progression reads as even size *classes* rather than equal
/// absolute jumps. Monotone increasing, finite, positive. `size_class` is clamped
/// to `0..=SIZE_CLASS_MAX`. Pure/deterministic; multiply this into the forge's
/// rest dimensions (the genome's `bulk` already adds within-class variation).
#[inline]
pub fn size_scale(size_class: u8) -> f32 {
    let c = size_class.min(SIZE_CLASS_MAX) as f32;
    let t = c / SIZE_CLASS_MAX as f32; // 0..1 across the buckets
    // MIN * (MAX/MIN)^t — geometric interpolation MIN→MAX.
    SIZE_SCALE_MIN * (SIZE_SCALE_MAX / SIZE_SCALE_MIN).powf(t)
}

/// "Balanced" body score in `[0,1]`: 1.0 when the core proportions all sit at
/// the neutral midpoint (0.5), falling off as they deviate. Feeds Sovereign.
fn balance_score(g: &Genome) -> f32 {
    let props = [
        g01(g.bulk),
        g01(g.neck_len),
        g01(g.tail_len),
        g01(g.leg_len),
        g01(g.wing_span),
    ];
    let mut dev = 0.0;
    for p in props {
        dev += (p - 0.5).abs();
    }
    let mean_dev = dev / props.len() as f32; // 0..0.5
    (1.0 - 2.0 * mean_dev).clamp(0.0, 1.0)
}

/// Per-build scores in roughly comparable `[0,1]` ranges (each a mean of its
/// driving genes). Order matches `Build::index`.
fn build_scores(g: &Genome) -> [f32; NUM_BUILDS] {
    let bulk = g01(g.bulk);
    let neck_len = g01(g.neck_len);
    let tail_len = g01(g.tail_len);
    let leg_len = g01(g.leg_len);
    let wing_span = g01(g.wing_span);
    let neck_arch = g01(g.neck_arch);
    let crest = g01(g.crest);
    // horns as a 0..1 fraction of the max pair count.
    let horns01 = horn_pairs(g) as f32 / 4.0;

    let serpentine = (neck_len + tail_len + (1.0 - leg_len)) / 3.0;
    let bruiser = (bulk + (1.0 - neck_len) + (1.0 - neck_arch)) / 3.0;
    let flyer = (wing_span + (1.0 - bulk)) / 2.0;
    let stalker = ((1.0 - bulk) + leg_len + (1.0 - tail_len)) / 3.0;
    let sovereign = (crest + horns01 + balance_score(g)) / 3.0;

    [serpentine, bruiser, flyer, stalker, sovereign]
}

/// Classify a genome's morphological family. Argmax of [`build_scores`];
/// ties resolve to the lowest `Build::index` (declaration order), so the result
/// is deterministic and stable under tiny perturbations away from a boundary.
pub fn build_of(g: &Genome) -> Build {
    let scores = build_scores(g);
    let builds = [
        Build::Serpentine,
        Build::Bruiser,
        Build::Flyer,
        Build::Stalker,
        Build::Sovereign,
    ];
    let mut best = 0usize;
    let mut i = 1;
    while i < NUM_BUILDS {
        if scores[i] > scores[best] {
            best = i;
        }
        i += 1;
    }
    builds[best]
}

/// Salient-trait flag bits for a genome.
fn flags_of(g: &Genome) -> u32 {
    let mut f = 0u32;
    if horn_pairs(g) >= 1 {
        f |= FLAG_HORNED;
    }
    if g01(g.crest) >= TRAIT_THRESHOLD {
        f |= FLAG_CRESTED;
    }
    if g01(g.teeth) >= TRAIT_THRESHOLD {
        f |= FLAG_TOOTHED;
    }
    if g01(g.brow) >= TRAIT_THRESHOLD {
        f |= FLAG_BROW_RIDGED;
    }
    if g01(g.wing_span) >= TRAIT_THRESHOLD {
        f |= FLAG_BROAD_WINGED;
    }
    if g01(g.tail_len) >= TRAIT_THRESHOLD {
        f |= FLAG_LONG_TAILED;
    }
    f
}

/// Classify a viewer-authored genome into a canonical game archetype.
/// Deterministic and stable: same genome → same archetype, every call.
pub fn classify(g: &Genome) -> Archetype {
    let element = dominant_element(g);
    let build = build_of(g);
    let id = element * NUM_BUILDS as u8 + build.index();
    Archetype {
        id,
        element,
        build,
        size_class: size_class_of(g),
        flags: flags_of(g),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A neutral genome: every gene at the midpoint, flat element affinities.
    fn neutral() -> Genome {
        Genome {
            bulk: 0.5,
            neck_len: 0.5,
            neck_arch: 0.5,
            tail_len: 0.5,
            leg_len: 0.5,
            wing_span: 0.5,
            wing_spread: 0.5,
            pose_bias: 0.5,
            muzzle_len: 0.5,
            jaw_depth: 0.5,
            snout_curve: 0.5,
            brow: 0.5,
            cheek: 0.5,
            teeth: 0.5,
            crest: 0.5,
            horns: 0.0,
            horn_sweep: 0.5,
            element: [0.0; NUM_ELEMENTS],
        }
    }

    #[test]
    fn high_wingspan_low_bulk_is_flyer() {
        let mut g = neutral();
        g.wing_span = 0.95;
        g.bulk = 0.05;
        assert_eq!(build_of(&g), Build::Flyer);
        assert_eq!(classify(&g).build, Build::Flyer);
    }

    #[test]
    fn high_bulk_short_neck_is_bruiser() {
        let mut g = neutral();
        g.bulk = 0.95;
        g.neck_len = 0.05;
        g.neck_arch = 0.1;
        assert_eq!(build_of(&g), Build::Bruiser);
    }

    #[test]
    fn long_neck_long_tail_low_leg_is_serpentine() {
        let mut g = neutral();
        g.neck_len = 0.95;
        g.tail_len = 0.95;
        g.leg_len = 0.05;
        assert_eq!(build_of(&g), Build::Serpentine);
    }

    #[test]
    fn lithe_long_leg_is_stalker() {
        let mut g = neutral();
        g.bulk = 0.05;
        g.leg_len = 0.95;
        g.tail_len = 0.1;
        g.neck_len = 0.4;
        assert_eq!(build_of(&g), Build::Stalker);
    }

    #[test]
    fn balanced_crest_horns_is_sovereign() {
        let mut g = neutral(); // all proportions at 0.5 → balance = 1.0
        g.crest = 1.0;
        g.horns = 1.0; // 4 pairs
        assert_eq!(build_of(&g), Build::Sovereign);
    }

    #[test]
    fn element_is_argmax_of_element_genes() {
        for dom in 0..NUM_ELEMENTS {
            let mut g = neutral();
            for k in 0..NUM_ELEMENTS {
                g.element[k] = if k == dom { 0.9 } else { 0.1 };
            }
            assert_eq!(classify(&g).element as usize, dom, "{} should dominate", ELEMENT_NAMES[dom]);
        }
    }

    #[test]
    fn element_ties_resolve_to_lowest_index() {
        let mut g = neutral();
        g.element = [0.7; NUM_ELEMENTS];
        assert_eq!(classify(&g).element, 0);
    }

    #[test]
    fn size_class_is_monotone_in_bulk() {
        let mut prev = 0u8;
        let mut g = neutral();
        for i in 0..=1000 {
            g.bulk = i as f32 / 1000.0;
            let sc = classify(&g).size_class;
            assert!(sc <= SIZE_CLASS_MAX);
            assert!(sc >= prev, "size_class dropped as bulk rose");
            prev = sc;
        }
    }

    #[test]
    fn horned_flag_iff_horns_present() {
        let mut g = neutral();
        g.horns = 0.0;
        assert_eq!(classify(&g).flags & FLAG_HORNED, 0, "0 pairs → not horned");
        g.horns = 0.9; // ~4 pairs
        assert_ne!(classify(&g).flags & FLAG_HORNED, 0, "high horns → horned");
        g.horns = 0.1; // rounds to 0 pairs
        assert_eq!(classify(&g).flags & FLAG_HORNED, 0, "negligible horns → not horned");
    }

    #[test]
    fn crested_flag_tracks_crest_gene() {
        let mut g = neutral();
        g.crest = 0.9;
        assert_ne!(classify(&g).flags & FLAG_CRESTED, 0);
        g.crest = 0.1;
        assert_eq!(classify(&g).flags & FLAG_CRESTED, 0);
    }

    #[test]
    fn classification_stable_under_tiny_perturbations() {
        let mut g = neutral();
        g.wing_span = 0.95;
        g.bulk = 0.05;
        let base = classify(&g);
        // Perturb each body gene by ±1e-4; well away from any score tie, the
        // archetype must not flip.
        let deltas = [-1e-4f32, 1e-4];
        for d in deltas {
            let mut p = g;
            p.wing_span = (p.wing_span + d).clamp(0.0, 1.0);
            p.bulk = (p.bulk + d).clamp(0.0, 1.0);
            p.neck_len = (p.neck_len + d).clamp(0.0, 1.0);
            p.tail_len = (p.tail_len + d).clamp(0.0, 1.0);
            p.leg_len = (p.leg_len + d).clamp(0.0, 1.0);
            assert_eq!(classify(&p), base, "tiny perturbation flipped the archetype");
        }
    }

    #[test]
    fn deterministic() {
        let mut g = neutral();
        g.wing_span = 0.8;
        g.element[4] = 0.9; // Fire
        let first = classify(&g);
        for _ in 0..100 {
            assert_eq!(classify(&g), first);
        }
    }

    #[test]
    fn out_of_range_and_non_finite_genes_clamp_without_panic() {
        let mut g = neutral();
        g.bulk = f32::NAN;
        g.wing_span = 5.0;
        g.leg_len = -3.0;
        g.element[2] = f32::INFINITY;
        g.horns = 9.0;
        let a = classify(&g); // must not panic
        assert!(a.size_class <= SIZE_CLASS_MAX);
        assert!((a.element as usize) < NUM_ELEMENTS);
        assert!(a.id < (NUM_ELEMENTS * NUM_BUILDS) as u8);
    }

    #[test]
    fn id_encodes_element_and_build() {
        let mut g = neutral();
        g.wing_span = 0.95;
        g.bulk = 0.05; // Flyer (index 2)
        g.element[5] = 0.9; // Storm (5)
        let a = classify(&g);
        assert_eq!(a.element, 5);
        assert_eq!(a.build, Build::Flyer);
        assert_eq!(a.id, 5 * NUM_BUILDS as u8 + Build::Flyer.index());
    }
}

#[cfg(test)]
mod from_seed_tests {
    use super::*;

    #[test]
    fn from_seed_is_deterministic() {
        let a = Genome::from_seed(12345);
        let b = Genome::from_seed(12345);
        assert_eq!(a.bulk, b.bulk);
        assert_eq!(a.wing_span, b.wing_span);
        assert_eq!(a.element, b.element);
    }

    #[test]
    fn all_genes_in_unit_range() {
        for seed in 0..200u64 {
            let g = Genome::from_seed(seed.wrapping_mul(2_654_435_761));
            for v in [
                g.bulk, g.neck_len, g.neck_arch, g.tail_len, g.leg_len, g.wing_span,
                g.wing_spread, g.pose_bias, g.muzzle_len, g.jaw_depth, g.snout_curve,
                g.brow, g.cheek, g.teeth, g.crest, g.horns, g.horn_sweep,
            ] {
                assert!((0.0..=1.0).contains(&v), "gene {v} out of [0,1] for seed {seed}");
            }
            for e in g.element {
                assert!((0.0..=1.0).contains(&e));
            }
        }
    }

    #[test]
    fn different_seeds_usually_differ() {
        let a = Genome::from_seed(1);
        let b = Genome::from_seed(2);
        assert!(a.bulk != b.bulk || a.wing_span != b.wing_span || a.element != b.element);
    }

    #[test]
    fn has_a_clear_dominant_element() {
        for seed in 0..50u64 {
            let g = Genome::from_seed(seed.wrapping_mul(0x100_0001));
            let max = g.element.iter().cloned().fold(0.0f32, f32::max);
            assert!(max >= 0.7, "seed {seed} should have a dominant element, got {max}");
        }
    }

    #[test]
    fn from_seed_element_forces_dominant() {
        for idx in 0..NUM_ELEMENTS as u8 {
            let g = Genome::from_seed_element(123, idx);
            let max_i = g.element.iter().enumerate()
                .max_by(|a, b| a.1.partial_cmp(b.1).unwrap()).unwrap().0;
            assert_eq!(max_i, idx as usize, "element {idx} should dominate");
            assert!(g.element[idx as usize] >= 0.85);
        }
    }

    #[test]
    fn seeded_genomes_classify_without_panic() {
        for seed in 0..100u64 {
            let g = Genome::from_seed(seed.wrapping_mul(2_246_822_519));
            let a = classify(&g); // must not panic
            assert!(a.element < NUM_ELEMENTS as u8);
        }
    }
}

// r336-dragon-size: tests for the small→huge size_scale ramp.
#[cfg(test)]
mod r336_size_scale_tests {
    use super::*;

    #[test]
    fn endpoints_small_and_huge() {
        assert!((size_scale(0) - SIZE_SCALE_MIN).abs() < 1e-6, "class 0 → smallest");
        assert!((size_scale(SIZE_CLASS_MAX) - SIZE_SCALE_MAX).abs() < 1e-6, "max → largest");
    }

    #[test]
    fn strictly_monotone_increasing() {
        let mut prev = size_scale(0);
        for c in 1..=SIZE_CLASS_MAX {
            let s = size_scale(c);
            assert!(s > prev, "size_scale must increase: {} -> {} at class {}", prev, s, c);
            prev = s;
        }
    }

    #[test]
    fn smallest_to_largest_ratio_at_least_5x() {
        let ratio = size_scale(SIZE_CLASS_MAX) / size_scale(0);
        assert!(ratio >= 5.0, "small→huge ratio {} should be >= 5x", ratio);
    }

    #[test]
    fn out_of_domain_clamps_to_max() {
        assert_eq!(size_scale(SIZE_CLASS_MAX), size_scale(SIZE_CLASS_MAX + 1));
        assert_eq!(size_scale(SIZE_CLASS_MAX), size_scale(250));
    }

    #[test]
    fn finite_positive_all_classes() {
        for c in 0..=SIZE_CLASS_MAX {
            let s = size_scale(c);
            assert!(s.is_finite() && s > 0.0, "class {} → {}", c, s);
        }
    }

    #[test]
    fn mid_class_between_endpoints() {
        let mid = size_scale(SIZE_CLASS_MAX / 2);
        assert!(mid > size_scale(0) && mid < size_scale(SIZE_CLASS_MAX));
    }

    #[test]
    fn deterministic() {
        for c in 0..=SIZE_CLASS_MAX {
            assert_eq!(size_scale(c), size_scale(c));
        }
    }
}
