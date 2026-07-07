//! Population genetics analyzer — breed a population across generations and
//! measure how a trait spreads or recedes, using the diploid model in
//! [`crate::genetics`]. This is the genome-analysis counterpart to the world
//! census: it confirms, with numbers, that the genetics behaves like real
//! population genetics — a dominant allele shows up in *more* individuals than
//! its raw frequency (it masks the recessive), selection raises a favored
//! allele's frequency over generations, and selecting against one makes it
//! recede.
//!
//! Pure + deterministic. Each member carries its own `entity_seed` so it
//! expresses with its own activators (dominance varies individual-to-individual,
//! exactly as in the base model).

use crate::genetics::{activator_for, EnvSwitches, Genome};

fn mix(seed: u64) -> u64 {
    // r341 dedup: bit-identical to the canonical splitmix64 finalizer — delegate.
    crate::util::hash_util::mix64(seed)
}
fn unit(seed: u64) -> f32 {
    crate::util::hash_util::mix64_unit(seed)
}

/// A breeding population: each member is a genome plus the seed it expresses with.
pub struct Population {
    pub members: Vec<(Genome, u64)>,
}

/// A measurement of one trait across a population.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TraitReport {
    /// `p` — fraction of *alleles* (2 per member) above the midpoint (the high
    /// allele's frequency in the gene pool).
    pub allele_frequency: f32,
    /// Fraction of *individuals* whose expressed phenotype is above the midpoint.
    pub phenotype_frequency: f32,
    /// Mean expressed phenotype across the population.
    pub mean_phenotype: f32,
    /// `phenotype_frequency - allele_frequency`: positive ⇒ the high allele
    /// *dominates* (shows up more than its gene frequency); negative ⇒ it's
    /// recessive (hidden below its frequency).
    pub dominance_index: f32,
}

impl Population {
    /// Found a population of `n` random individuals.
    pub fn founders(n: usize, seed: u64) -> Population {
        let members = (0..n)
            .map(|i| {
                let s = mix(seed ^ (i as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15));
                (Genome::from_seed(s), s ^ 0xAC71_5EED)
            })
            .collect();
        Population { members }
    }

    pub fn size(&self) -> usize {
        self.members.len()
    }

    /// High-allele frequency `p` for gene `gene_index` (`0..7`): fraction of all
    /// alleles whose value exceeds `mid`.
    pub fn allele_frequency(&self, gene_index: usize, mid: f32) -> f32 {
        let (mut hi, mut tot) = (0u32, 0u32);
        for (g, _) in &self.members {
            let gene = g.genes()[gene_index];
            for v in [gene.a.value, gene.b.value] {
                tot += 1;
                if v > mid {
                    hi += 1;
                }
            }
        }
        hi as f32 / tot.max(1) as f32
    }

    /// Fraction of individuals whose expressed phenotype exceeds `mid`.
    pub fn phenotype_frequency(&self, gene_index: usize, env: EnvSwitches, mid: f32) -> f32 {
        let n = self.members.len().max(1);
        let hi = self
            .members
            .iter()
            .filter(|(g, seed)| {
                g.genes()[gene_index].phenotype(activator_for(*seed, gene_index as u32), env) > mid
            })
            .count();
        hi as f32 / n as f32
    }

    /// Mean expressed phenotype for a gene across the population.
    pub fn mean_phenotype(&self, gene_index: usize, env: EnvSwitches) -> f32 {
        let n = self.members.len().max(1) as f32;
        self.members
            .iter()
            .map(|(g, seed)| g.genes()[gene_index].phenotype(activator_for(*seed, gene_index as u32), env))
            .sum::<f32>()
            / n
    }

    /// A full [`TraitReport`] for one gene.
    pub fn analyze(&self, gene_index: usize, env: EnvSwitches, mid: f32) -> TraitReport {
        let allele_frequency = self.allele_frequency(gene_index, mid);
        let phenotype_frequency = self.phenotype_frequency(gene_index, env, mid);
        TraitReport {
            allele_frequency,
            phenotype_frequency,
            mean_phenotype: self.mean_phenotype(gene_index, env),
            dominance_index: phenotype_frequency - allele_frequency,
        }
    }

    /// Breed the next generation at the same size: parents are sampled by
    /// `fitness` (roulette), each child is a [`Genome::breed`] of two parents with
    /// a fresh expression seed. Flat fitness ⇒ neutral drift; a fitness that
    /// tracks a trait ⇒ selection on it.
    pub fn next_generation(&self, seed: u64, fitness: impl Fn(&Genome) -> f32) -> Population {
        let weights: Vec<f32> = self.members.iter().map(|(g, _)| fitness(g).max(0.0)).collect();
        let total: f32 = weights.iter().sum();
        let n = self.members.len();
        let pick = |r: f32| -> usize {
            if total <= 0.0 {
                return (r * n as f32) as usize % n.max(1);
            }
            let target = r * total;
            let mut acc = 0.0;
            for (i, w) in weights.iter().enumerate() {
                acc += w;
                if target <= acc {
                    return i;
                }
            }
            n - 1
        };
        let members = (0..n)
            .map(|i| {
                let s = mix(seed ^ (i as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15));
                let p1 = pick(unit(s));
                let p2 = pick(unit(s ^ 0xBEEF_F00D));
                let child = self.members[p1].0.breed(&self.members[p2].0, s);
                (child, s ^ 0xAC71_5EED)
            })
            .collect();
        Population { members }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::genetics::{Allele, Gene};

    const HEIGHT: usize = 3; // gene index of the height trait

    #[test]
    fn a_dominant_allele_shows_up_more_than_its_frequency() {
        // Every member is heterozygous: a high allele (value 1.0) sitting at a
        // central locus (usually close to an activator → dominant) and a low
        // allele (0.0) parked at the far end (usually recessive). The gene pool is
        // exactly 50% high, yet the high trait should EXPRESS in well over half.
        let big = Allele::new(1.0, 0.5, 0);
        let small = Allele::new(0.0, 0.97, 0);
        let members = (0..400u64)
            .map(|i| {
                let mut g = Genome::from_seed(i);
                g.height = Gene::new(big, small);
                (g, mix(i ^ 0x5151))
            })
            .collect();
        let pop = Population { members };
        let r = pop.analyze(HEIGHT, 0, 0.5);
        assert!((r.allele_frequency - 0.5).abs() < 0.01, "gene pool is 50/50 ({})", r.allele_frequency);
        assert!(r.phenotype_frequency > 0.6, "the central allele dominates: shows in {} of individuals", r.phenotype_frequency);
        assert!(r.dominance_index > 0.1, "positive dominance index = it dominates ({})", r.dominance_index);
    }

    #[test]
    fn selection_spreads_a_favored_trait_and_starves_a_disfavored_one() {
        // Select FOR tall: fitness tracks expressed height. The tall allele's
        // frequency should climb over generations.
        let mut pop = Population::founders(400, 7);
        let start = pop.allele_frequency(HEIGHT, 0.5);
        for gen in 0..12u64 {
            pop = pop.next_generation(0x1000 + gen, |g| {
                g.height.phenotype(activator_for(0, HEIGHT as u32), 0).powi(2) + 0.02
            });
        }
        let after_up = pop.allele_frequency(HEIGHT, 0.5);
        assert!(after_up > start + 0.05, "selecting for tall raised the tall allele ({start:.3} -> {after_up:.3})");

        // Now select AGAINST tall on a fresh population: the allele recedes.
        let mut pop2 = Population::founders(400, 7);
        let start2 = pop2.allele_frequency(HEIGHT, 0.5);
        for gen in 0..12u64 {
            pop2 = pop2.next_generation(0x2000 + gen, |g| {
                let h = g.height.phenotype(activator_for(0, HEIGHT as u32), 0);
                (1.0 - h).powi(2) + 0.02
            });
        }
        let after_down = pop2.allele_frequency(HEIGHT, 0.5);
        assert!(after_down < start2 - 0.05, "selecting against tall made it recede ({start2:.3} -> {after_down:.3})");
    }

    #[test]
    fn neutral_drift_keeps_a_large_population_roughly_stable_over_a_generation() {
        let pop = Population::founders(800, 99);
        let before = pop.allele_frequency(HEIGHT, 0.5);
        let after = pop.next_generation(123, |_| 1.0).allele_frequency(HEIGHT, 0.5);
        assert!((after - before).abs() < 0.08, "no selection => only mild drift ({before:.3} -> {after:.3})");
    }
}
