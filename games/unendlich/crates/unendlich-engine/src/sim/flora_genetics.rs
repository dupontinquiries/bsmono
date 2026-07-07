//! `sim::flora_genetics` — the diploid genome every tree carries, its expression
//! rules, and seed inheritance (spec §C), **plus** a colour-genetics extension
//! (pigment + hue) that the base doc doesn't cover but which ties naturally into
//! the thermal (R) and biotic (B) fields.
//!
//! ## Documented loci (§C.1–C.2)
//! * **FR** fire resistance — codominant, expressed as the allele average
//!   (`FR-1/FR-3 → FR-2`).
//! * **WR** water requirement — codominant, the optimal-humidity range blends.
//! * **SR** sun requirement — codominant, optimal-light range blends.
//! * **SP** size potential — *limiting recessive*: the lower allele expresses
//!   unless the site fully satisfies both (`SP-2/SP-4 → SP-2` off-optimum).
//!
//! Inheritance (§C.4): one allele drawn from each parent per locus, with a 5%
//! chance per allele to mutate one step. Same-genus only.
//!
//! ## Colour extension (new)
//! A **pigment** locus (pale→dark, codominant) drives how much sun a plant's
//! leaves absorb, and therefore how hot they run — dark foliage is a thermal
//! advantage in the cold and a liability in heat. A **hue** locus (categorical,
//! dominance-ordered) gives the visible colour family. Both feed
//! [`crypsis`]: fauna whose colour matches the local foliage hide far better in
//! *dense* foliage. And [`linked_trait_bonus`] lets a species tie its pigment to
//! a real trait (dark bark ⇒ fire-hardened; dark leaves ⇒ cold-hardy), so colour
//! isn't cosmetic.
//!
//! Pure/std-only. Inheritance uses a caller-seeded splitmix step so crosses are
//! deterministic and testable.

/// A diploid allele pair at one locus (allele *levels*, e.g. FR-0..FR-3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pair {
    pub a: u8,
    pub b: u8,
}

impl Pair {
    pub fn new(a: u8, b: u8) -> Self {
        Pair { a, b }
    }
    pub fn homozygous(v: u8) -> Self {
        Pair { a: v, b: v }
    }
    /// Codominant expression: the average of the two alleles (§C.2). FR/WR/SR.
    pub fn codominant(self) -> f32 {
        (self.a as f32 + self.b as f32) * 0.5
    }
    /// Limiting-recessive expression (SP): the lower allele, unless the site is
    /// fully optimal, in which case the higher allele can express.
    pub fn limiting(self, site_fully_optimal: bool) -> u8 {
        if site_fully_optimal {
            self.a.max(self.b)
        } else {
            self.a.min(self.b)
        }
    }
}

/// The visible colour family (hue locus). Dominance order for expression is the
/// declaration order: earlier = more dominant.
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
    fn rank(self) -> u8 {
        Hue::ALL.iter().position(|&h| h == self).unwrap() as u8
    }
    /// Dominant expression of two hue alleles (lower rank wins).
    pub fn dominant(a: Hue, b: Hue) -> Hue {
        if a.rank() <= b.rank() {
            a
        } else {
            b
        }
    }
}

/// A hue allele pair.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HuePair {
    pub a: Hue,
    pub b: Hue,
}

impl HuePair {
    pub fn expressed(self) -> Hue {
        Hue::dominant(self.a, self.b)
    }
}

/// Highest pigment allele level (dark end of the scale), used to normalise shade.
pub const PIGMENT_MAX: u8 = 4;

/// A tree's full genome.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Genome {
    pub fr: Pair,
    pub wr: Pair,
    pub sr: Pair,
    pub sp: Pair,
    /// Colour extension: pigment level 0 (pale) .. PIGMENT_MAX (dark), codominant.
    pub pigment: Pair,
    pub hue: HuePair,
}

impl Genome {
    /// Expressed fire resistance (0..3), codominant average rounded.
    pub fn fire_resistance(&self) -> f32 {
        self.fr.codominant()
    }

    /// Expressed size class, limiting-recessive on site optimality.
    pub fn size_class(&self, site_fully_optimal: bool) -> u8 {
        self.sp.limiting(site_fully_optimal)
    }

    /// Expressed foliage darkness, 0.0 (pale) .. 1.0 (dark).
    pub fn shade(&self) -> f32 {
        (self.pigment.codominant() / PIGMENT_MAX as f32).clamp(0.0, 1.0)
    }

    /// Expressed hue.
    pub fn hue(&self) -> Hue {
        self.hue.expressed()
    }
}

// ---------------------------------------------------------------------------
// Colour → physics (extension)
// ---------------------------------------------------------------------------

/// Fraction of incident sunlight a leaf absorbs, from its shade. Pale leaves
/// reflect (≈0.35), dark leaves absorb (≈0.95).
pub fn sun_absorption(shade: f32) -> f32 {
    (0.35 + 0.6 * shade.clamp(0.0, 1.0)).clamp(0.0, 1.0)
}

/// How many °C above ambient a leaf runs in sun: absorption × luminosity ×
/// radiant heat × a solar-heating constant. Dark foliage in bright hot sun runs
/// hottest; at night (L→0) there's no solar gain.
pub const SOLAR_LEAF_HEATING_C: f32 = 12.0;
pub fn leaf_temp_rise_c(shade: f32, luminosity: f32, radiant: f32) -> f32 {
    sun_absorption(shade) * luminosity.clamp(0.0, 1.0) * radiant.clamp(0.0, 1.0) * SOLAR_LEAF_HEATING_C
}

/// How a species ties its pigment to a real trait (pleiotropy). `None` = purely
/// cosmetic colour.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorLinkage {
    None,
    /// Dark bark is char-adapted → bonus fire resistance.
    FireResist,
    /// Dark leaves run warm → survive cold better.
    ColdTolerance,
    /// Pale leaves reflect → tolerate harsh sun/heat.
    HeatTolerance,
}

/// The trait bonus (0..1) a plant's shade grants under its species linkage.
/// Dark helps fire/cold linkage; pale helps heat linkage; `None` → 0.
pub fn linked_trait_bonus(linkage: ColorLinkage, shade: f32) -> f32 {
    let s = shade.clamp(0.0, 1.0);
    match linkage {
        ColorLinkage::None => 0.0,
        ColorLinkage::FireResist | ColorLinkage::ColdTolerance => s,
        ColorLinkage::HeatTolerance => 1.0 - s,
    }
}

/// How well a fauna's colouring lets it hide against local foliage. Matching hue
/// and shade in *dense* foliage → near-total concealment; a mismatch, or sparse
/// cover, offers little. `foliage_density` 0..1. Returns 0..1 (1 = hidden).
pub fn crypsis(fauna: (Hue, f32), foliage: (Hue, f32), foliage_density: f32) -> f32 {
    let (fh, fs) = fauna;
    let (gh, gs) = foliage;
    let shade_match = 1.0 - (fs - gs).abs().clamp(0.0, 1.0);
    let hue_match = if fh == gh { 1.0 } else { 0.4 };
    (0.5 * shade_match + 0.5 * hue_match) * foliage_density.clamp(0.0, 1.0)
}

/// Biotic-density amplification from local genetic diversity: a stand with more
/// than 8 distinct genomes in radius gets +0.10 to B (§ biodiversity mechanic).
pub fn biodiversity_bonus(distinct_genomes: usize) -> f32 {
    if distinct_genomes > 8 {
        0.10
    } else {
        0.0
    }
}

// ---------------------------------------------------------------------------
// Inheritance (§C.4)
// ---------------------------------------------------------------------------

/// Mutation chance per allele per pollination event, in the deterministic RNG's
/// 0..1 space.
pub const MUTATION_RATE: f32 = 0.05;

/// A tiny deterministic PRNG (splitmix64) so crosses are reproducible + testable.
#[derive(Debug, Clone, Copy)]
pub struct Rng(pub u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Rng(seed)
    }
    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E3779B97F4A7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58476D1CE4E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D049BB133111EB);
        z ^ (z >> 31)
    }
    /// Uniform in [0,1).
    fn unit(&mut self) -> f32 {
        (self.next_u64() >> 40) as f32 / (1u64 << 24) as f32
    }
    fn coin(&mut self) -> bool {
        self.next_u64() & 1 == 0
    }
}

/// Draw one allele level for a locus: pick maternal or paternal allele at random,
/// then mutate one step (up or down) with [`MUTATION_RATE`], clamped to `[0,max]`.
fn inherit_pair(m: Pair, p: Pair, max: u8, rng: &mut Rng) -> Pair {
    // one allele from the maternal side, one from the paternal side (§C.4)
    let a = {
        let from_m = if rng.coin() { m.a } else { m.b };
        mutate(from_m, max, rng)
    };
    let b = {
        let from_p = if rng.coin() { p.a } else { p.b };
        mutate(from_p, max, rng)
    };
    Pair { a, b }
}

fn mutate(mut v: u8, max: u8, rng: &mut Rng) -> u8 {
    if rng.unit() < MUTATION_RATE {
        if rng.coin() {
            v = v.saturating_add(1).min(max);
        } else {
            v = v.saturating_sub(1);
        }
    }
    v
}

fn inherit_hue(m: HuePair, p: HuePair, rng: &mut Rng) -> HuePair {
    let a = if rng.coin() { m.a } else { m.b };
    let b = if rng.coin() { p.a } else { p.b };
    HuePair { a, b }
}

/// Cross two parent genomes into a seed genome (§C.4): each locus takes one
/// allele from the maternal and one from the paternal parent, each with a small
/// mutation chance. Deterministic for a given `rng`. (Same-genus is the caller's
/// responsibility — cross-genus pollination is rejected upstream.)
pub fn cross(maternal: &Genome, paternal: &Genome, rng: &mut Rng) -> Genome {
    Genome {
        fr: inherit_pair(maternal.fr, paternal.fr, 3, rng),
        wr: inherit_pair(maternal.wr, paternal.wr, 2, rng),
        sr: inherit_pair(maternal.sr, paternal.sr, 2, rng),
        sp: inherit_pair(maternal.sp, paternal.sp, 4, rng),
        pigment: inherit_pair(maternal.pigment, paternal.pigment, PIGMENT_MAX, rng),
        hue: inherit_hue(maternal.hue, paternal.hue, rng),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approx(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-4
    }

    fn genome(fr: (u8, u8), sp: (u8, u8), pig: (u8, u8), hue: (Hue, Hue)) -> Genome {
        Genome {
            fr: Pair::new(fr.0, fr.1),
            wr: Pair::homozygous(1),
            sr: Pair::homozygous(1),
            sp: Pair::new(sp.0, sp.1),
            pigment: Pair::new(pig.0, pig.1),
            hue: HuePair { a: hue.0, b: hue.1 },
        }
    }

    #[test]
    fn codominant_fr_averages() {
        // FR-1/FR-3 → FR-2 (the spec's example)
        assert!(approx(Pair::new(1, 3).codominant(), 2.0));
        assert!(approx(Pair::new(0, 2).codominant(), 1.0));
    }

    #[test]
    fn sp_is_limiting_recessive() {
        // SP-2/SP-4 → SP-2 off-optimum, SP-4 only when the site is fully optimal
        let sp = Pair::new(2, 4);
        assert_eq!(sp.limiting(false), 2);
        assert_eq!(sp.limiting(true), 4);
    }

    #[test]
    fn shade_and_absorption_and_leaf_heat() {
        let dark = genome((0, 0), (2, 2), (4, 4), (Hue::Green, Hue::Green));
        let pale = genome((0, 0), (2, 2), (0, 0), (Hue::Pale, Hue::Pale));
        assert!(approx(dark.shade(), 1.0) && approx(pale.shade(), 0.0));
        assert!(sun_absorption(dark.shade()) > sun_absorption(pale.shade()));
        // in bright hot sun the dark canopy runs hotter; at night neither gains
        assert!(leaf_temp_rise_c(dark.shade(), 1.0, 1.0) > leaf_temp_rise_c(pale.shade(), 1.0, 1.0));
        assert!(approx(leaf_temp_rise_c(dark.shade(), 0.0, 1.0), 0.0), "no sun → no solar gain");
    }

    #[test]
    fn color_can_be_linked_to_a_real_trait() {
        // a fire-linked species: darker bark ⇒ more fire resistance
        assert!(linked_trait_bonus(ColorLinkage::FireResist, 1.0) > linked_trait_bonus(ColorLinkage::FireResist, 0.0));
        // heat-linked: pale reflects ⇒ better in heat
        assert!(linked_trait_bonus(ColorLinkage::HeatTolerance, 0.0) > linked_trait_bonus(ColorLinkage::HeatTolerance, 1.0));
        assert_eq!(linked_trait_bonus(ColorLinkage::None, 0.7), 0.0, "cosmetic colour → no bonus");
    }

    #[test]
    fn crypsis_rewards_matching_dense_foliage() {
        let green_dark = (Hue::Green, 0.8);
        // a matching creature deep in dense green canopy is nearly invisible
        let hidden = crypsis(green_dark, green_dark, 1.0);
        // the same creature on sparse cover, or a mismatched one, stands out
        let sparse = crypsis(green_dark, green_dark, 0.2);
        let mismatch = crypsis((Hue::Crimson, 0.2), green_dark, 1.0);
        assert!(hidden > 0.9);
        assert!(hidden > sparse && hidden > mismatch);
        assert!(mismatch < 0.6, "wrong colour → poor concealment even when dense");
    }

    #[test]
    fn biodiversity_threshold() {
        assert_eq!(biodiversity_bonus(8), 0.0);
        assert_eq!(biodiversity_bonus(9), 0.10);
    }

    #[test]
    fn cross_draws_alleles_from_parents_and_is_deterministic() {
        let mum = genome((3, 3), (4, 4), (4, 4), (Hue::Green, Hue::Green)); // dark, fire-hardy, mega
        let dad = genome((0, 0), (2, 2), (0, 0), (Hue::Crimson, Hue::Crimson)); // pale, flammable, standard
        // deterministic for a fixed seed
        let mut r1 = Rng::new(42);
        let mut r2 = Rng::new(42);
        assert_eq!(cross(&mum, &dad, &mut r1), cross(&mum, &dad, &mut r2));
        // over many crosses, every child allele is a parent allele ±1 mutation
        let mut rng = Rng::new(7);
        for _ in 0..500 {
            let kid = cross(&mum, &dad, &mut rng);
            for lvl in [kid.fr.a, kid.fr.b] {
                assert!(lvl <= 3, "clamped to FR max");
            }
            // pigment child alleles come from {4 (mum), 0 (dad)} ± 1 → in {0,1,3,4} mostly, always ≤ max
            assert!(kid.pigment.a <= PIGMENT_MAX && kid.pigment.b <= PIGMENT_MAX);
            // hue is always one of the parents' hues
            assert!(matches!(kid.hue.a, Hue::Green | Hue::Crimson));
            assert!(matches!(kid.hue.b, Hue::Green | Hue::Crimson));
        }
    }

    #[test]
    fn mega_tree_needs_both_parents_to_carry_sp4() {
        // SP-4/SP-4 (mega) can only arise if both parents carry SP-4 — a pale/std
        // parent (SP-2/SP-2) can never contribute a 4, so no SP-4/SP-4 offspring
        // absent mutation.
        let std_a = genome((0, 0), (2, 2), (1, 1), (Hue::Green, Hue::Green));
        let std_b = genome((0, 0), (2, 2), (1, 1), (Hue::Green, Hue::Green));
        let mut rng = Rng::new(99);
        let mut saw_mega = false;
        for _ in 0..200 {
            let kid = cross(&std_a, &std_b, &mut rng);
            if kid.sp == Pair::new(4, 4) {
                saw_mega = true;
            }
        }
        assert!(!saw_mega, "two standard trees don't spontaneously make a mega (barring double mutation)");
    }
}
