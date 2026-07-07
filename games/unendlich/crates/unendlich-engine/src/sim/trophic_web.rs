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
    pub const ALL: [Species; 12] = [
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
            // apex / base species have none
            _ => &[],
        }
    }

    /// Species `self` hunts (inverse of the graph).
    pub fn prey(self) -> Vec<Species> {
        Species::ALL.iter().copied().filter(|&p| p.predators().contains(&self)).collect()
    }

    /// No predators — top of its chain.
    pub fn is_apex(self) -> bool {
        self.predators().is_empty()
    }

    /// Intrinsic per-step growth rate. Small, fast-breeding species (rabbit,
    /// firefly, shrimp) recover quickly; apex predators breed slowly.
    pub fn growth_rate(self) -> f32 {
        use Species::*;
        match self {
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
    pub biomass: [f32; 12],
}

impl Default for Populations {
    fn default() -> Self {
        Populations { biomass: [0.0; 12] }
    }
}

impl Populations {
    pub fn uniform(v: f32) -> Self {
        Populations { biomass: [v.clamp(0.0, 1.0); 12] }
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
        if n <= 0.0 && food_availability(s, pops) <= 0.0 {
            // extinct with no way to feed in → stays extinct (no spontaneous life)
            out.set(s, 0.0);
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
}
