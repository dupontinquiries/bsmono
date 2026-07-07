//! Feeding & breeding — the energy economy that ties eating to reproduction.
//! A creature carries [`Vitality`] (an energy reserve in `[0,1]`) that drains
//! over time and is replenished by eating grass or food. Breeding is *earned*:
//! only well-fed creatures can do it, and each act spends a chunk of the
//! parents' reserves, so a population that can't feed itself can't multiply —
//! and a fed one breeds, with offspring genomes that are Mendelian crosses of
//! the parents (see [`crate::genetics`]). This couples the genetics to the
//! ecosystem: scarce food selects, abundant food lets traits spread.
//!
//! Pure + deterministic. The world wiring (eat the grass block under a creature,
//! pair nearby well-fed creatures, spawn the offspring) sits on top of this.

use crate::genetics::{Genome, Sex};
use crate::world::biomes::Biome;

/// Energy lost per second just being alive (at rest). Eating must outpace it.
pub const STARVE_DECAY_PER_SEC: f32 = 0.01;
/// Energy gained from eating one grass/food.
pub const GRASS_FOOD_VALUE: f32 = 0.25;
/// A creature must be at least this well-fed to breed.
pub const BREED_THRESHOLD: f32 = 0.8;
/// Below this energy a creature stops idling and starts actively seeking food.
pub const HUNGER_THRESHOLD: f32 = 0.5;
/// Energy each parent spends on a breeding act (so it can't immediately repeat).
pub const BREED_COST: f32 = 0.5;

/// Energy a grazing creature gains per second from the biome it stands in. Lush,
/// vegetated biomes feed it faster than [`STARVE_DECAY_PER_SEC`] (net gain);
/// sparse ones barely keep pace; barren ground (desert, rock, ocean, caves) gives
/// nothing, so a creature lingering there slowly starves. This is what makes
/// *habitat* matter — fauna thrive where there's grass and struggle where there
/// isn't, without needing per-block grass bookkeeping yet.
pub fn graze_rate(biome: Biome) -> f32 {
    use Biome::*;
    match biome {
        // Lush — comfortably above the starve rate.
        Plains | TemperateForest | BirchForest | MapleForest | CloudForest | BloomingCanyon | Swamp => 0.05,
        // Lush new lands — rich grazing.
        Savanna | Jungle | Mangrove => 0.05,
        // Vegetated but leaner.
        TemperateHighlands | MistyMountains | AlpineHighlands | Taiga | HighlandSteppe => 0.03,
        // Sparse — barely keeps a creature alive.
        Beach | Tundra => 0.015,
        // Barren — no grazing; a creature here is on borrowed time.
        Desert | RedSandstoneCanyon | Ocean | Skylands | IceCave | FungalCave | ThermalCave | CrystalCave | LushCave | Mesa | VolcanicWastes | FrozenOcean => 0.0,
    }
}

/// A creature's energy reserve.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Vitality {
    /// `[0,1]`: 0 = starving, 1 = full.
    pub energy: f32,
}

impl Vitality {
    pub fn new(energy: f32) -> Vitality {
        Vitality { energy: energy.clamp(0.0, 1.0) }
    }
    /// A full reserve.
    pub fn full() -> Vitality {
        Vitality::new(1.0)
    }

    /// Drain energy for `dt` seconds at `decay_per_sec` (clamped at empty).
    pub fn tick(&mut self, dt: f32, decay_per_sec: f32) {
        self.energy = (self.energy - decay_per_sec * dt).max(0.0);
    }

    /// Eat: add `amount` energy (clamped at full).
    pub fn feed(&mut self, amount: f32) {
        self.energy = (self.energy + amount).min(1.0);
    }

    /// Out of energy.
    pub fn is_starving(&self) -> bool {
        self.energy <= 0.0
    }

    /// Well-fed enough to reproduce.
    pub fn can_breed(&self) -> bool {
        self.energy >= BREED_THRESHOLD
    }

    /// Hungry enough to break off idling and go seek food.
    pub fn is_hungry(&self) -> bool {
        self.energy < HUNGER_THRESHOLD
    }
}

/// Attempt to breed two creatures. If BOTH are well-fed enough, each pays
/// [`BREED_COST`] and an offspring genome — a Mendelian cross of the parents — is
/// returned. Otherwise `None`, and no energy is spent. The energy cost is what
/// stops a fed pair from breeding every tick: after one act they fall below the
/// threshold and must eat again.
pub fn try_breed(
    a: &mut Vitality,
    b: &mut Vitality,
    genome_a: &Genome,
    genome_b: &Genome,
    seed: u64,
) -> Option<Genome> {
    if a.can_breed() && b.can_breed() {
        a.energy = (a.energy - BREED_COST).max(0.0);
        b.energy = (b.energy - BREED_COST).max(0.0);
        Some(genome_a.breed(genome_b, seed))
    } else {
        None
    }
}

/// Whether two creatures can mate: opposite [`Sex`] and both well-fed enough.
/// This is the partner check fauna use to *pick* a mate.
pub fn can_mate(sex_a: Sex, sex_b: Sex, a: &Vitality, b: &Vitality) -> bool {
    sex_a != sex_b && a.can_breed() && b.can_breed()
}

/// Like [`try_breed`] but also requires the pair be opposite sexes — the actual
/// mating path. No energy is spent unless the mating goes ahead.
pub fn try_mate(
    sex_a: Sex,
    sex_b: Sex,
    a: &mut Vitality,
    b: &mut Vitality,
    genome_a: &Genome,
    genome_b: &Genome,
    seed: u64,
) -> Option<Genome> {
    if can_mate(sex_a, sex_b, a, b) {
        try_breed(a, b, genome_a, genome_b, seed)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn energy_drains_to_starving_and_eating_refills() {
        let mut v = Vitality::full();
        // At rest it slowly drains.
        v.tick(10.0, STARVE_DECAY_PER_SEC); // 0.1 lost
        assert!((v.energy - 0.9).abs() < 1e-5);
        // Drain past empty clamps at 0 → starving.
        v.tick(1000.0, STARVE_DECAY_PER_SEC);
        assert_eq!(v.energy, 0.0);
        assert!(v.is_starving());
        // Eating refills, capped at full.
        v.feed(GRASS_FOOD_VALUE);
        assert!((v.energy - GRASS_FOOD_VALUE).abs() < 1e-5 && !v.is_starving());
        v.feed(10.0);
        assert_eq!(v.energy, 1.0);
    }

    #[test]
    fn only_the_well_fed_can_breed() {
        assert!(Vitality::full().can_breed());
        assert!(Vitality::new(BREED_THRESHOLD).can_breed());
        assert!(!Vitality::new(BREED_THRESHOLD - 0.01).can_breed());
        assert!(!Vitality::new(0.0).can_breed());
    }

    #[test]
    fn hunger_kicks_in_below_the_threshold() {
        assert!(!Vitality::full().is_hungry());
        assert!(!Vitality::new(HUNGER_THRESHOLD).is_hungry());
        assert!(Vitality::new(HUNGER_THRESHOLD - 0.01).is_hungry());
        // A creature can be hungry (seeking food) well before it would starve.
        assert!(Vitality::new(0.3).is_hungry() && !Vitality::new(0.3).is_starving());
    }

    #[test]
    fn habitat_decides_whether_grazing_outpaces_starvation() {
        use crate::world::biomes::Biome;
        // Lush biomes net-positive (graze beats decay); barren ones net-negative.
        assert!(graze_rate(Biome::Plains) > STARVE_DECAY_PER_SEC, "plains feed a grazer");
        assert!(graze_rate(Biome::TemperateForest) > STARVE_DECAY_PER_SEC);
        assert_eq!(graze_rate(Biome::Desert), 0.0, "desert offers nothing to graze");
        assert_eq!(graze_rate(Biome::Ocean), 0.0);
        assert_eq!(graze_rate(Biome::ThermalCave), 0.0);
        // Sparse biomes barely sustain (positive but slim).
        let tundra = graze_rate(Biome::Tundra);
        assert!(tundra > 0.0 && tundra <= STARVE_DECAY_PER_SEC + 0.01, "tundra is lean ({tundra})");
    }

    #[test]
    fn breeding_costs_energy_so_a_pair_cant_spam_it() {
        let mut a = Vitality::full();
        let mut b = Vitality::full();
        let ga = Genome::from_seed(11);
        let gb = Genome::from_seed(22);
        // First act succeeds and both pay the cost.
        let child = try_breed(&mut a, &mut b, &ga, &gb, 7);
        assert!(child.is_some(), "two full creatures breed");
        assert!((a.energy - (1.0 - BREED_COST)).abs() < 1e-5);
        assert!((b.energy - (1.0 - BREED_COST)).abs() < 1e-5);
        // They're now below threshold → a second immediate attempt fails and
        // spends nothing.
        let before = (a.energy, b.energy);
        assert!(try_breed(&mut a, &mut b, &ga, &gb, 8).is_none(), "can't breed again until refed");
        assert_eq!((a.energy, b.energy), before, "a failed attempt costs no energy");
        // After eating back up, they can breed again.
        a.feed(1.0);
        b.feed(1.0);
        assert!(try_breed(&mut a, &mut b, &ga, &gb, 9).is_some(), "refed pair breeds again");
    }

    #[test]
    fn a_hungry_partner_blocks_breeding_at_no_cost() {
        let mut fed = Vitality::full();
        let mut hungry = Vitality::new(0.2);
        let ga = Genome::from_seed(1);
        let gb = Genome::from_seed(2);
        assert!(try_breed(&mut fed, &mut hungry, &ga, &gb, 1).is_none());
        assert_eq!(fed.energy, 1.0, "the fed one keeps its energy when the partner can't breed");
    }

    #[test]
    fn mating_needs_opposite_sexes_and_two_fed_partners() {
        let (m, f) = (Sex::Male, Sex::Female);
        let full = Vitality::full();
        let lean = Vitality::new(0.3);
        // Opposite sexes, both fed → can mate.
        assert!(can_mate(m, f, &full, &full));
        // Same sex → never, however well-fed.
        assert!(!can_mate(m, m, &full, &full));
        assert!(!can_mate(f, f, &full, &full));
        // Opposite sexes but one underfed → no.
        assert!(!can_mate(m, f, &full, &lean));

        // try_mate spends nothing on a same-sex pair...
        let (mut a, mut b) = (Vitality::full(), Vitality::full());
        let (ga, gb) = (Genome::from_seed(3), Genome::from_seed(4));
        assert!(try_mate(m, m, &mut a, &mut b, &ga, &gb, 1).is_none());
        assert_eq!((a.energy, b.energy), (1.0, 1.0));
        // ...but produces an offspring for a fed opposite-sex pair (and charges them).
        assert!(try_mate(m, f, &mut a, &mut b, &ga, &gb, 1).is_some());
        assert!(a.energy < 1.0 && b.energy < 1.0);
    }
}
