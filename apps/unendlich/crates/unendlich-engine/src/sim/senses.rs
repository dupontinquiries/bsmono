//! Senses — a creature's *realized* perception, derived from its species base
//! and its genome. This is where the genetics, vision, and hearing models meet:
//! the `eyesight` and `hearing` genes finally drive something. A dragon starts
//! from keen aerial-hunter eyes, a lauscher from near-blind ones and the sharpest
//! ears; then the individual's genome sharpens or dulls each, so a poorly-sighted
//! fauna and a keen-eyed one of the same species genuinely differ — and it's
//! heritable.
//!
//! Pure + deterministic.

use crate::genetics::{EnvSwitches, Genome};
use crate::item::CreatureKind;
use crate::sim::eyes::EyeProfile;
use crate::sim::sound::{noticed_over_ambient, Loudness};

/// A creature's working senses for this individual, in this environment.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Senses {
    /// The eye it sees with (already scaled by its eyesight gene).
    pub eyes: EyeProfile,
    /// The faintest perceived loudness it will notice (lower = sharper ears).
    pub hearing_threshold: Loudness,
}

impl Senses {
    /// Whether this creature notices a sound it perceives at `perceived`
    /// loudness, given the local biome `ambient_floor` (see
    /// [`crate::sim::sound::biome_ambient_loudness`]). It must clear both this
    /// creature's hearing threshold and the ambient floor.
    pub fn notices(&self, perceived: i32, ambient_floor: Loudness) -> bool {
        noticed_over_ambient(perceived, ambient_floor, self.hearing_threshold)
    }
}

/// The species' baseline eye, before the genome adjusts it.
fn base_eye(kind: CreatureKind) -> EyeProfile {
    match kind {
        CreatureKind::Dragon => EyeProfile::AERIAL_HUNTER,
        CreatureKind::Dinosaur => EyeProfile::DINO,
        CreatureKind::Lauscher => EyeProfile::LAUSCHER, // near-blind; hunts by ear
        CreatureKind::Spaeher | CreatureKind::Fauna => EyeProfile::FAUNA,
    }
}

/// The species' baseline hearing threshold (faintest loudness noticed), before
/// the genome adjusts it. The lauscher hears best; the dragon leans on sight.
fn base_hearing_threshold(kind: CreatureKind) -> Loudness {
    match kind {
        CreatureKind::Lauscher => 6,  // sharpest ears in the world
        CreatureKind::Spaeher => 10,  // a vigilant watcher
        CreatureKind::Dinosaur => 16,
        CreatureKind::Fauna => 18,
        CreatureKind::Dragon => 22, // relies on its eyes
    }
}

/// Realize a creature's senses from its `kind`, `genome`, individual
/// `entity_seed`, and `env`. Eyesight gene sharpens/extends the eye; hearing gene
/// lowers (sharpens) the hearing threshold.
pub fn senses_for(kind: CreatureKind, genome: &Genome, entity_seed: u64, env: EnvSwitches) -> Senses {
    let ph = genome.express(entity_seed, env);
    let eyes = base_eye(kind).with_eyesight(ph.eyesight);
    // hearing gene 1.0 -> 0.5x threshold (sharper), 0.0 -> 1.5x (duller).
    let base = base_hearing_threshold(kind) as f32;
    let hearing_threshold = (base * (1.5 - ph.hearing)).round().max(1.0) as Loudness;
    Senses { eyes, hearing_threshold }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sim::eyes::{sees, Detection};

    #[test]
    fn species_set_the_baseline_dragon_sees_far_lauscher_barely() {
        let g = Genome::from_seed(1);
        let dragon = senses_for(CreatureKind::Dragon, &g, 1, 0);
        let lauscher = senses_for(CreatureKind::Lauscher, &g, 1, 0);
        assert!(dragon.eyes.range > lauscher.eyes.range * 3.0, "a dragon outsees a lauscher by far");
        // The lauscher has the sharpest ears (lowest threshold) of any species.
        let kinds = [CreatureKind::Dragon, CreatureKind::Spaeher, CreatureKind::Fauna, CreatureKind::Dinosaur];
        for k in kinds {
            assert!(lauscher.hearing_threshold <= senses_for(k, &g, 1, 0).hearing_threshold,
                "lauscher hears at least as faintly as {k:?}");
        }
    }

    #[test]
    fn the_genome_makes_individuals_of_one_species_differ() {
        // Two fauna genomes engineered to be keen- vs poor-sighted via the
        // eyesight gene (index 4). Same species, different reach.
        use crate::genetics::{Allele, Gene};
        let mut keen = Genome::from_seed(5);
        keen.eyesight = Gene::new(Allele::new(1.0, 0.5, 0), Allele::new(1.0, 0.5, 0)); // homozygous sharp
        let mut poor = Genome::from_seed(5);
        poor.eyesight = Gene::new(Allele::new(0.0, 0.5, 0), Allele::new(0.0, 0.5, 0)); // homozygous blurry
        let sk = senses_for(CreatureKind::Fauna, &keen, 1, 0);
        let sp = senses_for(CreatureKind::Fauna, &poor, 1, 0);
        assert!(sk.eyes.range > sp.eyes.range && sk.eyes.acuity_ratio < sp.eyes.acuity_ratio,
            "keen-eyed fauna outsees its poor-sighted cousin");
        // And it tells in practice: a target the keen one resolves, the poor misses.
        let target = [sp.eyes.range + 4.0, 0.0, 0.0];
        assert!(sees(sk.eyes, [0.0; 3], [1.0, 0.0, 0.0], target, 0.5, 0.0).is_detected());
        assert_eq!(sees(sp.eyes, [0.0; 3], [1.0, 0.0, 0.0], target, 0.5, 0.0), Detection::Unseen);
    }

    #[test]
    fn sharper_hearing_gene_notices_fainter_sounds() {
        use crate::genetics::{Allele, Gene};
        let mut sharp = Genome::from_seed(8);
        sharp.hearing = Gene::new(Allele::new(1.0, 0.5, 0), Allele::new(1.0, 0.5, 0));
        let mut dull = Genome::from_seed(8);
        dull.hearing = Gene::new(Allele::new(0.0, 0.5, 0), Allele::new(0.0, 0.5, 0));
        let ss = senses_for(CreatureKind::Fauna, &sharp, 1, 0);
        let sd = senses_for(CreatureKind::Fauna, &dull, 1, 0);
        assert!(ss.hearing_threshold < sd.hearing_threshold, "sharp ears notice fainter ({} < {})", ss.hearing_threshold, sd.hearing_threshold);
        // A faint footstep in a quiet biome (floor 2): the sharp-eared one notices,
        // the dull one doesn't.
        let faint = ((ss.hearing_threshold + sd.hearing_threshold) / 2).max(ss.hearing_threshold + 1);
        assert!(ss.notices(faint, 2), "sharp ears catch the faint step");
        assert!(!sd.notices(faint, 2), "dull ears miss it");
    }
}
