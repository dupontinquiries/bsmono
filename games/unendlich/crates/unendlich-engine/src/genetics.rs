//! Diploid genetics — dominant/recessive, crosses, drift, and environment-driven
//! expression, as a pure deterministic model.
//!
//! The design (from playtest musings) makes "dominance" a *number on a scale*
//! rather than a fixed label:
//!
//! - A **trait** is carried by a [`Gene`] = two [`Allele`]s (one per parent;
//!   diploid). Each allele has a numeric `value` (the trait it codes for, e.g. a
//!   colour channel or a height factor in `[0,1]`), a `locus` — its position on
//!   the genome strand `[0,1]`, which can **drift** across generations — and a
//!   set of epigenetic `switches` (bits) that can be turned on by the
//!   environment.
//! - Each individual has a per-gene **activator** point on the strand. An
//!   allele's **dominance is how close its locus sits to that activator**: close
//!   = dominant, far = recessive (with a small floor so a recessive still leaks).
//!   Because loci drift and activators are per-entity, *which* allele is dominant
//!   varies between individuals and shifts over generations — no allele is
//!   permanently king.
//! - The expressed **phenotype** is the dominance-weighted blend of the two
//!   allele values. Lopsided dominance ⇒ the dominant value wins (classic
//!   dominant/recessive); near-equal dominance ⇒ a blend (incomplete dominance,
//!   the pink-flower case).
//! - **Expression** is further scaled by how many of an allele's switches the
//!   current environment has flipped on — so cold / heat / hunger can up- or
//!   down-regulate a gene, giving cheap adaptation gameplay.
//! - **Crossing** two parents takes one allele from each (Mendelian), so a
//!   heterozygous cross yields the textbook 1:2:1 genotype spread, with drift
//!   applied to the child's loci/switches.

/// A per-gene environment state: a bitmask of which epigenetic switches the
/// world is currently asserting (cold, heat, wet, hunger, …). The meaning of
/// each bit is up to the caller; only the overlap with an allele's `switches`
/// matters.
pub type EnvSwitches = u8;

/// The smallest dominance a maximally-recessive allele keeps — it never fully
/// vanishes, so a recessive trait can still leak into the blend.
pub const DOMINANCE_FLOOR: f32 = 0.05;

fn mix(seed: u64) -> u64 {
    let mut z = seed.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// A `[0,1)` draw from a seed.
fn unit(seed: u64) -> f32 {
    (mix(seed) >> 40) as f32 / (1u64 << 24) as f32
}

/// One copy of a trait, inherited from a single parent.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Allele {
    /// The trait value this allele codes for, conventionally `[0,1]`.
    pub value: f32,
    /// Position on the genome strand `[0,1]`. Drifts across generations; its
    /// distance to the entity's activator sets this allele's dominance.
    pub locus: f32,
    /// Epigenetic switches (bits). Overlap with the environment up-regulates
    /// this allele's expression.
    pub switches: u8,
}

impl Allele {
    pub fn new(value: f32, locus: f32, switches: u8) -> Allele {
        Allele { value: value.clamp(0.0, 1.0), locus: locus.clamp(0.0, 1.0), switches }
    }

    /// Dominance in `[DOMINANCE_FLOOR, 1]`: 1 when the locus sits on the
    /// activator, falling off linearly with distance along the strand.
    pub fn dominance(&self, activator: f32) -> f32 {
        let d = (self.locus - activator).abs();
        (1.0 - d).max(DOMINANCE_FLOOR)
    }

    /// Expression multiplier in `[0.5, 1]`: a baseline plus a bonus for the
    /// fraction of this allele's switches the environment has turned on. An
    /// allele with no switches sits at the baseline regardless of environment.
    pub fn expression(&self, env: EnvSwitches) -> f32 {
        let total = self.switches.count_ones();
        if total == 0 {
            return 0.75; // no epigenetic switches: steady mid expression
        }
        let on = (self.switches & env).count_ones();
        0.5 + 0.5 * (on as f32 / total as f32)
    }
}

/// A diploid gene: the two allele copies an individual carries for one trait.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Gene {
    pub a: Allele,
    pub b: Allele,
}

impl Gene {
    pub fn new(a: Allele, b: Allele) -> Gene {
        Gene { a, b }
    }

    /// Effective weight of each allele = dominance × expression.
    fn weights(&self, activator: f32, env: EnvSwitches) -> (f32, f32) {
        (
            self.a.dominance(activator) * self.a.expression(env),
            self.b.dominance(activator) * self.b.expression(env),
        )
    }

    /// The expressed trait value: the weight-blended allele values. Lopsided
    /// weights ⇒ the dominant allele wins; equal weights ⇒ a 50/50 blend.
    pub fn phenotype(&self, activator: f32, env: EnvSwitches) -> f32 {
        let (wa, wb) = self.weights(activator, env);
        let sum = wa + wb;
        if sum <= 1e-6 {
            return (self.a.value + self.b.value) * 0.5;
        }
        (self.a.value * wa + self.b.value * wb) / sum
    }

    /// Whether the two alleles code for the (near) same value — homozygous.
    pub fn is_homozygous(&self) -> bool {
        (self.a.value - self.b.value).abs() < 1e-3
    }
}

/// Nudge an allele for inheritance: its locus drifts a little along the strand,
/// and (rarely) one switch bit flips. Value is conserved — alleles are passed on
/// intact; it's *where they sit* and *how they're regulated* that evolve.
pub fn drift(allele: Allele, seed: u64) -> Allele {
    let dl = (unit(seed) - 0.5) * 0.12; // ±0.06 strand drift
    let mut switches = allele.switches;
    if unit(seed ^ 0xA5A5_A5A5) < 0.08 {
        let bit = (mix(seed ^ 0x1357) % 8) as u8;
        switches ^= 1 << bit;
    }
    Allele::new(allele.value, (allele.locus + dl).clamp(0.0, 1.0), switches)
}

/// A Mendelian cross: the child inherits one allele from each parent (chosen by
/// `seed`), each carried over with [`drift`]. Crossing two heterozygotes
/// therefore reconstructs the 1:2:1 genotype spread.
pub fn cross(p1: &Gene, p2: &Gene, seed: u64) -> Gene {
    let from1 = if mix(seed) & 1 == 0 { p1.a } else { p1.b };
    let from2 = if mix(seed ^ 0xDEAD) & 1 == 0 { p2.a } else { p2.b };
    Gene::new(drift(from1, seed ^ 0x11), drift(from2, seed ^ 0x22))
}

/// The per-gene activator point for an individual, derived from its seed and a
/// gene index. (Each gene gets its own activator, so dominance is independent
/// trait-to-trait.)
pub fn activator_for(entity_seed: u64, gene_index: u32) -> f32 {
    unit(entity_seed ^ ((gene_index as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15)))
}

/// A creature's full genotype: one [`Gene`] per visible/sensory trait. Colour is
/// three genes (one per channel) so a creature can be heterozygous for, say, red
/// but not blue — which is how mixed coat colours arise.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Genome {
    pub color_r: Gene,
    pub color_g: Gene,
    pub color_b: Gene,
    /// Body-size factor.
    pub height: Gene,
    /// Visual acuity (drives eye range/resolution later).
    pub eyesight: Gene,
    /// Auditory acuity (drives hearing threshold later).
    pub hearing: Gene,
    /// Movement-speed factor — a slightly faster or slower individual.
    pub speed: Gene,
    /// Turn-rate factor — how fast it can pivot (prevents instant flipping).
    pub turn_rate: Gene,
}

/// The trait names in genome order, lining up with [`Genome::genes`] and the
/// gene index passed to [`activator_for`].
pub const GENOME_TRAITS: [&str; 8] = ["color R", "color G", "color B", "height", "eyesight", "hearing", "speed", "turn rate"];

/// The expressed traits of an individual in a given environment.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Phenotype {
    pub color: [f32; 3],
    pub height: f32,
    pub eyesight: f32,
    pub hearing: f32,
    pub speed: f32,
    pub turn_rate: f32,
}

/// A per-gene readout for display: both allele values, both dominances, the
/// expressed phenotype, and whether the individual is homozygous for the trait.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GeneReadout {
    pub value_a: f32,
    pub value_b: f32,
    pub dom_a: f32,
    pub dom_b: f32,
    pub phenotype: f32,
    pub homozygous: bool,
}

impl Gene {
    /// A display readout of this gene for the given activator/environment.
    pub fn readout(&self, activator: f32, env: EnvSwitches) -> GeneReadout {
        let total = self.a.dominance(activator) * self.a.expression(env)
            + self.b.dominance(activator) * self.b.expression(env);
        let total = total.max(1e-6);
        GeneReadout {
            value_a: self.a.value,
            value_b: self.b.value,
            dom_a: self.a.dominance(activator) * self.a.expression(env) / total,
            dom_b: self.b.dominance(activator) * self.b.expression(env) / total,
            phenotype: self.phenotype(activator, env),
            homozygous: self.is_homozygous(),
        }
    }
}

fn roll_allele(seed: u64) -> Allele {
    Allele::new(unit(seed), unit(seed ^ 0x9999), (mix(seed ^ 0x7777) & 0x0F) as u8)
}

fn roll_gene(seed: u64) -> Gene {
    Gene::new(roll_allele(seed ^ 0xA), roll_allele(seed ^ 0xB))
}

impl Genome {
    /// Roll a fresh genotype from a seed (a wild creature's genome).
    pub fn from_seed(seed: u64) -> Genome {
        Genome {
            color_r: roll_gene(seed ^ 0x01),
            color_g: roll_gene(seed ^ 0x02),
            color_b: roll_gene(seed ^ 0x03),
            height: roll_gene(seed ^ 0x04),
            eyesight: roll_gene(seed ^ 0x05),
            hearing: roll_gene(seed ^ 0x06),
            speed: roll_gene(seed ^ 0x07),
            turn_rate: roll_gene(seed ^ 0x08),
        }
    }

    /// The genes in [`GENOME_TRAITS`] order.
    pub fn genes(&self) -> [Gene; 8] {
        [self.color_r, self.color_g, self.color_b, self.height, self.eyesight, self.hearing, self.speed, self.turn_rate]
    }

    /// Express this genome for the individual `entity_seed` in environment `env`.
    pub fn express(&self, entity_seed: u64, env: EnvSwitches) -> Phenotype {
        let g = self.genes();
        let p = |i: usize| g[i].phenotype(activator_for(entity_seed, i as u32), env);
        Phenotype {
            color: [p(0), p(1), p(2)],
            height: p(3),
            eyesight: p(4),
            hearing: p(5),
            speed: p(6),
            turn_rate: p(7),
        }
    }

    /// Per-gene readouts in [`GENOME_TRAITS`] order, for the viewer panel.
    pub fn readouts(&self, entity_seed: u64, env: EnvSwitches) -> [GeneReadout; 8] {
        let g = self.genes();
        std::array::from_fn(|i| g[i].readout(activator_for(entity_seed, i as u32), env))
    }

    /// Breed two genomes: each child gene is a Mendelian [`cross`] of the
    /// parents' corresponding genes.
    pub fn breed(&self, other: &Genome, seed: u64) -> Genome {
        let s = |salt: u64| seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ salt;
        Genome {
            color_r: cross(&self.color_r, &other.color_r, s(0x01)),
            color_g: cross(&self.color_g, &other.color_g, s(0x02)),
            color_b: cross(&self.color_b, &other.color_b, s(0x03)),
            height: cross(&self.height, &other.height, s(0x04)),
            eyesight: cross(&self.eyesight, &other.eyesight, s(0x05)),
            hearing: cross(&self.hearing, &other.hearing, s(0x06)),
            speed: cross(&self.speed, &other.speed, s(0x07)),
            turn_rate: cross(&self.turn_rate, &other.turn_rate, s(0x08)),
        }
    }
}

/// Biological sex, for mate selection. Assigned per individual at birth and
/// fixed thereafter (it is not one of the blended quantitative genes — it's a
/// coin flip seeded by the creature, ~50/50 across a population).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Sex {
    Male,
    Female,
}

impl Sex {
    pub fn label(self) -> &'static str {
        match self {
            Sex::Male => "male",
            Sex::Female => "female",
        }
    }
}

/// Deterministic ~50/50 sex from a creature's seed.
pub fn sex_from_seed(seed: u64) -> Sex {
    let mut x = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ 0xA5A5_A5A5_A5A5_A5A5;
    x ^= x >> 33;
    x = x.wrapping_mul(0xFF51_AFD7_ED55_8CCD);
    x ^= x >> 33;
    if x & 1 == 0 {
        Sex::Male
    } else {
        Sex::Female
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sex_is_deterministic_and_roughly_balanced() {
        // Deterministic per seed.
        assert_eq!(sex_from_seed(42), sex_from_seed(42));
        // Roughly 50/50 across a population.
        let males = (0..2000u64).filter(|&s| sex_from_seed(s) == Sex::Male).count();
        assert!((800..1200).contains(&males), "≈balanced sexes, got {males} males of 2000");
    }

    #[test]
    fn a_close_locus_dominates_a_far_one() {
        // Activator at one end of the strand; the dominant allele sits on it
        // (distance 0 → full dominance) and the recessive at the far end
        // (distance 1 → floor). This is a *fully* dominant configuration — note
        // it depends on where this individual's activator landed, which is the
        // point: the same genotype expresses differently between individuals.
        let activator = 0.0;
        let gene = Gene::new(Allele::new(1.0, 0.0, 0), Allele::new(0.0, 1.0, 0));
        let p = gene.phenotype(activator, 0);
        assert!(p > 0.85, "the dominant (close) allele wins: {p}");
        // Swap which allele is close → the other value dominates.
        let gene2 = Gene::new(Allele::new(1.0, 1.0, 0), Allele::new(0.0, 0.0, 0));
        assert!(gene2.phenotype(activator, 0) < 0.15, "now the 0.0 allele dominates");
    }

    #[test]
    fn equal_dominance_blends_incomplete_dominance() {
        // Both alleles equidistant from the activator → equal dominance → the
        // phenotype is the average (red × white = pink).
        let activator = 0.5;
        let gene = Gene::new(Allele::new(1.0, 0.3, 0), Allele::new(0.0, 0.7, 0));
        let p = gene.phenotype(activator, 0);
        assert!((p - 0.5).abs() < 1e-3, "incomplete dominance blends to the mean: {p}");
    }

    #[test]
    fn a_heterozygous_cross_gives_the_textbook_one_two_one() {
        // Parents are both Aa: allele value 1.0 = A, 0.0 = a. Count child
        // genotypes over many crosses; expect ~25% AA, ~50% Aa, ~25% aa.
        let big_a = Allele::new(1.0, 0.4, 0);
        let little_a = Allele::new(0.0, 0.6, 0);
        let p1 = Gene::new(big_a, little_a);
        let p2 = Gene::new(big_a, little_a);
        let (mut aa, mut het, mut rr) = (0u32, 0u32, 0u32);
        let n = 20_000u64;
        for s in 0..n {
            let child = cross(&p1, &p2, s.wrapping_mul(0x9E37_79B9_7F4A_7C15));
            let a_is_big = child.a.value > 0.5;
            let b_is_big = child.b.value > 0.5;
            match (a_is_big, b_is_big) {
                (true, true) => aa += 1,
                (false, false) => rr += 1,
                _ => het += 1,
            }
        }
        let frac = |c: u32| c as f32 / n as f32;
        assert!((frac(aa) - 0.25).abs() < 0.03, "AA ~25%, got {:.3}", frac(aa));
        assert!((frac(het) - 0.50).abs() < 0.03, "Aa ~50%, got {:.3}", frac(het));
        assert!((frac(rr) - 0.25).abs() < 0.03, "aa ~25%, got {:.3}", frac(rr));
    }

    #[test]
    fn drift_moves_loci_so_dominance_can_flip_over_generations() {
        // Repeatedly drift an allele; its locus should wander measurably.
        let start = Allele::new(0.7, 0.5, 0b0101);
        let mut a = start;
        let mut max_move = 0.0f32;
        for g in 0..50u64 {
            a = drift(a, g.wrapping_mul(0x1000_0001));
            max_move = max_move.max((a.locus - start.locus).abs());
            assert_eq!(a.value, start.value, "drift conserves the coded value");
        }
        assert!(max_move > 0.1, "loci drift enough to reorder dominance ({max_move})");
    }

    #[test]
    fn environment_up_regulates_a_matching_allele_adaptation() {
        // Allele A codes a high value and carries a "cold" switch (bit 0); allele
        // B codes low with no switches. With dominance equal, turning on the cold
        // environment boosts A's expression and pulls the phenotype up.
        let activator = 0.5;
        let cold_gene = Gene::new(Allele::new(1.0, 0.4, 0b0001), Allele::new(0.0, 0.6, 0));
        let warm = cold_gene.phenotype(activator, 0b0000); // cold switch off
        let cold = cold_gene.phenotype(activator, 0b0001); // cold switch on
        assert!(cold > warm, "cold environment up-regulates the cold-adapted allele ({warm} -> {cold})");
    }

    #[test]
    fn a_genome_expresses_traits_in_range_and_breeds() {
        let g = Genome::from_seed(42);
        let ph = g.express(42, 0);
        for c in ph.color {
            assert!((0.0..=1.0).contains(&c), "colour channel in range: {c}");
        }
        assert!((0.0..=1.0).contains(&ph.height));
        // Readouts line up with the trait list and their phenotypes match express().
        let r = g.readouts(42, 0);
        assert_eq!(r.len(), GENOME_TRAITS.len());
        assert!((r[3].phenotype - ph.height).abs() < 1e-5, "readout matches expressed height");
        // Each readout's two dominances sum to 1 (they're the normalized weights).
        for row in r {
            assert!((row.dom_a + row.dom_b - 1.0).abs() < 1e-4);
        }
        // Breeding two genomes is deterministic and yields a valid genome.
        let h = Genome::from_seed(99);
        let child1 = g.breed(&h, 7);
        let child2 = g.breed(&h, 7);
        assert_eq!(child1, child2, "breeding is deterministic in the seed");
        let cph = child1.express(7, 0);
        assert!((0.0..=1.0).contains(&cph.color[0]));
    }

    #[test]
    fn two_homozygous_parents_breed_a_predictable_child() {
        // Both parents homozygous "tall" (height alleles all value 1.0): every
        // child is tall too (no recessive to surface), whatever the activator.
        let tall = Allele::new(1.0, 0.5, 0);
        let mut g = Genome::from_seed(1);
        g.height = Gene::new(tall, tall);
        let mut h = Genome::from_seed(2);
        h.height = Gene::new(tall, tall);
        for s in 0..200u64 {
            let child = g.breed(&h, s.wrapping_mul(0x9E37_79B9_7F4A_7C15));
            assert!(child.height.a.value > 0.99 && child.height.b.value > 0.99, "tall × tall stays tall");
            // And it expresses tall under any individual's activators.
            assert!(child.express(s, 0).height > 0.95);
        }
    }
}
