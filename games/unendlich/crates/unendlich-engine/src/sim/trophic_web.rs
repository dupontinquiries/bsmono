//! `sim::trophic_web` — the predator-prey graph that links the animal roster into
//! one self-rebalancing ecology (§6.1), spanning land and sea.
//!
//! Every species sits in a "is hunted by" graph. From that graph two per-species
//! signals fall out each step: **predation pressure** (how much predator biomass
//! is bearing down on it) and **food availability** (how much prey it has to eat).
//! A bounded logistic/Lotka-Volterra step turns those into population change, so
//! the doc's emergent story works on its own: wipe the wolves in a region and the
//! rabbits explode, which in turn pulls migratory predators (bears) toward the
//! high rabbit density — the system re-levels without scripting.
//!
//! Populations are normalized biomass in `0..1` per species (a region's carrying
//! fraction). The step is pure and deterministic — no clock, no RNG — so it's
//! safe to run inside the deep-time statistical sim and rewind.
//!
//! Pure/std-only.

/// The animal roster that participates in the food web.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Species {
    /// The primary producer, and the base the whole web was missing.
    ///
    /// Without it `food_availability` returns 0 for every herbivore, because
    /// it sums the biomass of a species' PREY and a rabbit has none. So the
    /// feeding half of the Lotka-Volterra step was dead for the entire base of
    /// the web: rabbits grew at their intrinsic rate and were limited only by
    /// predation, and a region could never be overgrazed. "Wipe the wolves and
    /// the rabbits explode" worked; "the rabbits then eat the meadow bare and
    /// crash" could not.
    Vegetation,
    // aquatic
    CaveFish,
    CaveTroll,
    BioShrimp,
    Anglerfish,
    // surface small
    Rabbit,
    Fox,
    Firefly,
    // surface large
    Deer,
    Moose,
    Wolf,
    Bear,
    Harpy,
}

impl Species {
    pub const ALL: [Species; 13] = [
        Species::Vegetation,
        Species::CaveFish,
        Species::CaveTroll,
        Species::BioShrimp,
        Species::Anglerfish,
        Species::Rabbit,
        Species::Fox,
        Species::Firefly,
        Species::Deer,
        Species::Moose,
        Species::Wolf,
        Species::Bear,
        Species::Harpy,
    ];

    pub fn index(self) -> usize {
        Species::ALL.iter().position(|&s| s == self).unwrap()
    }

    /// Species that hunt `self` ("is hunted by", §6.1).
    pub fn predators(self) -> &'static [Species] {
        use Species::*;
        match self {
            CaveFish => &[CaveTroll],
            BioShrimp => &[Anglerfish],
            Rabbit => &[Fox, Wolf, Harpy],
            Deer => &[Wolf, Bear],
            // A moose had NO predators, which made `is_apex` call it one — a
            // large herbivore filed as a top carnivore. Wolves and bears take
            // calves and winter-weakened adults, which is the real relation
            // and the reason a moose population is not unbounded.
            Moose => &[Wolf, Bear],
            // Everything that eats plants presses on the producer.
            Vegetation => &[Rabbit, Deer, Moose],
            // apex species have none
            _ => &[],
        }
    }

    /// Species `self` hunts (inverse of the graph).
    pub fn prey(self) -> Vec<Species> {
        Species::ALL.iter().copied().filter(|&p| p.predators().contains(&self)).collect()
    }

    /// A primary producer: eats nothing, and the base of every chain.
    pub fn is_producer(self) -> bool {
        matches!(self, Species::Vegetation)
    }

    /// No predators AND not a producer — top of its chain.
    ///
    /// The producer clause matters: vegetation is eaten by three species, so
    /// it is not apex, but before `Vegetation` existed a firefly was — nothing
    /// hunts one — and so was a moose, which was simply a missing edge.
    pub fn is_apex(self) -> bool {
        !self.is_producer() && self.predators().is_empty()
    }

    /// Intrinsic per-step growth rate. Small, fast-breeding species (rabbit,
    /// firefly, shrimp) recover quickly; apex predators breed slowly.
    pub fn growth_rate(self) -> f32 {
        use Species::*;
        match self {
            // Plants regrow fastest of anything: a meadow comes back in a
            // season where a wolf pack takes years.
            Vegetation => 0.50,
            Firefly | BioShrimp | CaveFish => 0.35,
            Rabbit => 0.30,
            Fox | Deer => 0.14,
            Moose => 0.10,
            Wolf | Harpy | CaveTroll | Anglerfish => 0.06,
            Bear => 0.05,
        }
    }

    /// Baseline mortality per step (starvation/age independent of predation).
    pub fn mortality(self) -> f32 {
        0.04
    }
}

/// Per-species normalized biomass `0..1`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Populations {
    pub biomass: [f32; 13],
}

impl Default for Populations {
    fn default() -> Self {
        Populations { biomass: [0.0; 13] }
    }
}

impl Populations {
    pub fn uniform(v: f32) -> Self {
        Populations { biomass: [v.clamp(0.0, 1.0); 13] }
    }
    pub fn get(&self, s: Species) -> f32 {
        self.biomass[s.index()]
    }
    pub fn set(&mut self, s: Species, v: f32) {
        self.biomass[s.index()] = v.clamp(0.0, 1.0);
    }
}

/// Total predator biomass pressing on `s` (sum over its predators).
pub fn predation_pressure(s: Species, pops: &Populations) -> f32 {
    s.predators().iter().map(|&p| pops.get(p)).sum()
}

/// Total prey biomass available to `s` to eat (sum over its prey).
pub fn food_availability(s: Species, pops: &Populations) -> f32 {
    s.prey().iter().map(|&q| pops.get(q)).sum()
}

/// The trace a producer recovers from after being wiped out. See `step`.
///
/// Small enough that recovery is slow and visible, large enough that the
/// logistic term has something to multiply — growth is `n * (...)`, so a
/// literal zero can never leave zero however good the conditions.
pub const SEED_BANK: f32 = 0.01;

/// Coupling coefficients for the population step (tunable).
#[derive(Debug, Clone, Copy)]
pub struct WebParams {
    /// Step size.
    pub dt: f32,
    /// How hard predation drives prey down.
    pub predation: f32,
    /// How much available prey feeds predator growth.
    pub feeding: f32,
}

impl Default for WebParams {
    fn default() -> Self {
        WebParams { dt: 1.0, predation: 0.6, feeding: 0.5 }
    }
}

/// Advance the whole web one step. For each species:
/// `Δ = pop · (growth·(1-pop)  +  feeding·food  −  predation·pressure  −  mortality)`,
/// clamped to `0..1`. Logistic self-limit keeps biomass bounded; the predation
/// and feeding couplings are what make the graph self-rebalance. Deterministic.
pub fn step(pops: &Populations, p: &WebParams) -> Populations {
    let mut out = *pops;
    for &s in &Species::ALL {
        let n = pops.get(s);
        if n <= 0.0 && food_availability(s, pops) <= 0.0 && !s.is_producer() {
            // extinct with no way to feed in → stays extinct (no spontaneous life)
            out.set(s, 0.0);
            continue;
        }
        if s.is_producer() && n <= 0.0 {
            // THE SEED BANK. A producer's food is sunlight, so
            // `food_availability` is always 0 for it and the guard above would
            // make burnt ground permanently barren — and with the base of the
            // web gone, every herbivore above it starves with no recovery
            // path. That is a dead region no fire should be able to create.
            //
            // Real ground regrows from a seed bank and from its edges, so a
            // producer at zero comes back from a trace rather than from
            // nothing. Deliberately tiny: a burnt meadow should take a long
            // while to be a meadow again, which is the point of burning it.
            out.set(s, SEED_BANK);
            continue;
        }
        let logistic = s.growth_rate() * (1.0 - n);
        let feed = p.feeding * food_availability(s, pops);
        let pressure = p.predation * predation_pressure(s, pops);
        let delta = n * (logistic + feed - pressure - s.mortality());
        out.set(s, (n + p.dt * delta).clamp(0.0, 1.0));
    }
    out
}

/// How strongly a (migratory) predator is pulled toward this region: high when
/// its prey is abundant here but it is itself scarce — the mechanism behind
/// "bears migrate into the rabbit-rich region." `0..1`.
pub fn migration_pull(predator: Species, pops: &Populations) -> f32 {
    let food = food_availability(predator, pops).clamp(0.0, 1.0);
    let scarcity = 1.0 - pops.get(predator);
    food * scarcity
}

#[cfg(test)]
mod tests {
    use super::*;
    use Species::*;

    #[test]
    fn graph_matches_the_doc() {
        assert_eq!(Rabbit.predators(), &[Fox, Wolf, Harpy]);
        assert_eq!(Deer.predators(), &[Wolf, Bear]);
        assert_eq!(CaveFish.predators(), &[CaveTroll]);
        assert_eq!(BioShrimp.predators(), &[Anglerfish]);
        assert!(Firefly.predators().is_empty(), "firefly at the ecological base");
        assert!(Bear.is_apex() && Anglerfish.is_apex());
    }

    #[test]
    fn prey_is_the_graph_inverse() {
        let wolf_prey = Wolf.prey();
        assert!(wolf_prey.contains(&Rabbit) && wolf_prey.contains(&Deer));
        assert_eq!(Anglerfish.prey(), vec![BioShrimp]);
        assert!(Firefly.prey().is_empty());
    }

    #[test]
    fn pressure_and_availability_sum_the_graph() {
        let mut pops = Populations::default();
        pops.set(Fox, 0.2);
        pops.set(Wolf, 0.3);
        pops.set(Harpy, 0.1);
        assert!((predation_pressure(Rabbit, &pops) - 0.6).abs() < 1e-5, "fox+wolf+harpy");
        pops.set(Rabbit, 0.5);
        pops.set(Deer, 0.4);
        assert!((food_availability(Wolf, &pops) - 0.9).abs() < 1e-5, "rabbit+deer");
    }

    #[test]
    fn base_species_grows_toward_carrying_capacity() {
        let mut pops = Populations::default();
        pops.set(Firefly, 0.1);
        let p = WebParams::default();
        for _ in 0..50 {
            pops = step(&pops, &p);
        }
        assert!(pops.get(Firefly) > 0.5, "unpreyed base species recovers, got {}", pops.get(Firefly));
        assert!(pops.get(Firefly) <= 1.0, "logistic cap");
    }

    #[test]
    fn removing_the_predator_lets_prey_explode() {
        // The doc's headline: kill the wolves and rabbits surge. Same start, one
        // world keeps wolves, the other has none.
        let p = WebParams::default();
        let mut with_wolves = Populations::default();
        with_wolves.set(Rabbit, 0.3);
        with_wolves.set(Wolf, 0.4);
        with_wolves.set(Deer, 0.3);
        let mut no_wolves = with_wolves;
        no_wolves.set(Wolf, 0.0);

        for _ in 0..40 {
            with_wolves = step(&with_wolves, &p);
            no_wolves = step(&no_wolves, &p);
        }
        assert!(
            no_wolves.get(Rabbit) > with_wolves.get(Rabbit),
            "no wolves ⇒ more rabbits ({} vs {})",
            no_wolves.get(Rabbit),
            with_wolves.get(Rabbit)
        );
    }

    #[test]
    fn migration_pull_targets_rich_scarce_regions() {
        // bears (prey: deer) pulled toward deer-rich regions where bears are scarce
        let mut rich = Populations::default();
        rich.set(Deer, 0.9);
        rich.set(Bear, 0.05);
        let mut saturated = Populations::default();
        saturated.set(Deer, 0.9);
        saturated.set(Bear, 0.95);
        assert!(migration_pull(Bear, &rich) > migration_pull(Bear, &saturated));
        // no prey → no pull
        let empty = Populations::default();
        assert_eq!(migration_pull(Bear, &empty), 0.0);
    }

    #[test]
    fn everything_stays_bounded_and_deterministic() {
        let p = WebParams::default();
        let mut a = Populations::uniform(0.5);
        let mut b = Populations::uniform(0.5);
        for _ in 0..200 {
            a = step(&a, &p);
            b = step(&b, &p);
        }
        assert_eq!(a.biomass, b.biomass, "deterministic");
        for v in a.biomass {
            assert!((0.0..=1.0).contains(&v), "biomass {v} escaped [0,1]");
        }
    }

    #[test]
    fn extinct_with_no_food_stays_extinct() {
        // an apex predator with zero prey and zero population doesn't spawn from nothing
        let pops = Populations::default(); // all zero
        let next = step(&pops, &WebParams::default());
        assert_eq!(next.get(Anglerfish), 0.0);
    }

    /// THE MISSING BASE. Before `Vegetation`, `food_availability` summed a
    /// herbivore's prey and a rabbit has none — so the feeding half of the
    /// step was dead for the entire bottom of the web.
    #[test]
    fn herbivores_finally_have_something_to_eat() {
        let mut pops = Populations::default();
        pops.set(Species::Vegetation, 0.8);
        pops.set(Species::Rabbit, 0.3);
        assert!(food_availability(Species::Rabbit, &pops) > 0.5,
                "a rabbit in a meadow should be well fed");
        assert!(food_availability(Species::Deer, &pops) > 0.5);
    }

    /// OVERGRAZING, which is what the base makes possible: rabbits with no
    /// predators eat the meadow down rather than growing forever.
    #[test]
    fn unchecked_herbivores_eat_the_meadow_down() {
        let mut pops = Populations::default();
        pops.set(Species::Vegetation, 1.0);
        pops.set(Species::Rabbit, 0.6);
        let p = WebParams::default();
        for _ in 0..60 {
            pops = step(&pops, &p);
        }
        assert!(pops.get(Species::Vegetation) < 1.0,
                "the meadow should be grazed: {}", pops.get(Species::Vegetation));
    }

    /// A moose was filed as an APEX PREDATOR, because nothing hunted it and
    /// `is_apex` meant "no predators".
    #[test]
    fn a_moose_is_prey_not_an_apex_predator() {
        assert!(!Species::Moose.is_apex());
        assert!(Species::Moose.predators().contains(&Species::Wolf));
        assert!(Species::Bear.is_apex(), "a bear still is one");
    }

    /// A producer is not apex either, though nothing it eats hunts it.
    #[test]
    fn vegetation_is_a_producer_rather_than_an_apex_species() {
        assert!(Species::Vegetation.is_producer());
        assert!(!Species::Vegetation.is_apex());
        assert!(!Species::Wolf.is_producer());
        // It is eaten by every herbivore.
        assert_eq!(Species::Vegetation.predators().len(), 3);
    }

    /// THE SEED BANK. Burnt ground must not be permanently barren, or a fire
    /// starves everything above it forever.
    #[test]
    fn burnt_ground_regrows_rather_than_staying_dead() {
        let mut pops = Populations::default();
        pops.set(Species::Vegetation, 0.0);
        let p = WebParams::default();
        pops = step(&pops, &p);
        assert!(pops.get(Species::Vegetation) > 0.0, "a trace comes back");
        for _ in 0..200 {
            pops = step(&pops, &p);
        }
        assert!(pops.get(Species::Vegetation) > 0.3,
                "and grows back over time: {}", pops.get(Species::Vegetation));
    }

    /// ...but GRAZERS SUPPRESS THE RECOVERY, and that is the interesting half.
    /// My first draft of the test above left rabbits on the burnt ground and
    /// asserted it regrew anyway; simulating it showed 0.02 after 200 steps,
    /// not the 0.3 I had assumed. Burnt ground with hungry animals on it stays
    /// bare, which is what actually happens and is worth asserting rather than
    /// designing around.
    #[test]
    fn grazers_hold_burnt_ground_bare() {
        let mut grazed = Populations::default();
        grazed.set(Species::Vegetation, 0.0);
        grazed.set(Species::Rabbit, 0.2);
        let mut clear = Populations::default();
        clear.set(Species::Vegetation, 0.0);
        let p = WebParams::default();
        for _ in 0..200 {
            grazed = step(&grazed, &p);
            clear = step(&clear, &p);
        }
        assert!(grazed.get(Species::Vegetation) < clear.get(Species::Vegetation) * 0.5,
                "grazed {} vs clear {}",
                grazed.get(Species::Vegetation), clear.get(Species::Vegetation));
    }

    /// ...but animals still do not appear from nothing. The seed bank is a
    /// producer rule, not a general one.
    #[test]
    fn extinct_animals_stay_extinct() {
        let mut pops = Populations::default();
        pops.set(Species::Vegetation, 1.0);
        pops.set(Species::Wolf, 0.0);
        let p = WebParams::default();
        for _ in 0..50 {
            pops = step(&pops, &p);
        }
        assert_eq!(pops.get(Species::Wolf), 0.0, "no spontaneous wolves");
    }
}
