//! Ambient wildlife that makes the map feel alive.
//!
//! Each biome has its own fauna. Critters wander and graze with a small
//! sense→transition→act FSM (adapted from early-fps's enemy state machine),
//! and bolt when the hunter or a cat comes near. Populations are seeded by
//! per-biome habitat weights and a day/night activity curve, both borrowed
//! from unendlich's ecosystem model.

use crate::world;

/// The ambient species. Each keeps to the biomes that suit it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Species { Hare, Deer, Squirrel, Duck, Frog, Heron, Owl, Grouse }

pub const SPECIES: [Species; 8] = [
    Species::Hare, Species::Deer, Species::Squirrel, Species::Duck,
    Species::Frog, Species::Heron, Species::Owl, Species::Grouse,
];

impl Species {
    pub fn name(self) -> &'static str {
        match self {
            Species::Hare => "hare", Species::Deer => "deer", Species::Squirrel => "squirrel",
            Species::Duck => "duck", Species::Frog => "frog", Species::Heron => "heron",
            Species::Owl => "owl", Species::Grouse => "grouse",
        }
    }
    /// How suited a biome is to this species (0 = never lives here).
    pub fn habitat_weight(self, biome: &'static str) -> u16 {
        use Species::*;
        match self {
            Hare     => match biome { "Moor Plains" => 8, "Stony Mountains" => 6, "Oak Tree Forest" => 3, "Lake Shore" => 2, _ => 1 },
            Deer     => match biome { "Oak Tree Forest" => 8, "Moor Plains" => 4, "Shadowy Forest" => 3, _ => 1 },
            Squirrel => match biome { "Oak Tree Forest" => 9, "Shadowy Forest" => 5, _ => 0 },
            Duck     => match biome { "Lake Shore" => 9, "River Reeds" => 6, _ => 0 },
            Frog     => match biome { "River Reeds" => 9, "Lake Shore" => 5, _ => 0 },
            Heron    => match biome { "River Reeds" => 8, "Lake Shore" => 4, _ => 0 },
            Owl      => match biome { "Shadowy Forest" => 9, "Oak Tree Forest" => 3, "Stony Mountains" => 2, _ => 0 },
            Grouse   => match biome { "Moor Plains" => 9, "Stony Mountains" => 4, _ => 1 },
        }
    }
    /// 0 = strictly diurnal, 1 = strictly nocturnal.
    pub fn nocturnality(self) -> f32 {
        match self { Species::Owl => 1.0, Species::Frog => 0.6, Species::Deer => 0.4, _ => 0.15 }
    }
    /// Distance at which a threat sends it fleeing.
    pub fn flee_range(self) -> f32 {
        match self {
            Species::Deer => 17.0, Species::Heron => 15.0, Species::Hare => 12.0,
            Species::Owl => 12.0, Species::Grouse => 11.0, Species::Duck => 10.0,
            Species::Squirrel => 8.0, Species::Frog => 6.0,
        }
    }
    /// Panic run speed (units/sec).
    pub fn speed(self) -> f32 {
        match self {
            Species::Deer => 10.0, Species::Hare => 9.0, Species::Owl => 9.0, Species::Squirrel => 7.5,
            Species::Duck => 5.0, Species::Heron => 6.5, Species::Grouse => 6.0, Species::Frog => 6.0,
        }
    }
    pub fn is_bird(self) -> bool { matches!(self, Species::Duck | Species::Heron | Species::Owl | Species::Grouse) }
    /// Land species that won't wade into deep water (ducks/herons/frogs are fine there).
    pub fn avoids_water(self) -> bool { !matches!(self, Species::Duck | Species::Heron | Species::Frog) }
    /// Body colour (paper tint).
    pub fn color(self) -> u32 {
        match self {
            Species::Hare => 0x9a7b52, Species::Deer => 0x8a6a44, Species::Squirrel => 0x9a5334,
            Species::Duck => 0x5f7150, Species::Frog => 0x5f7a38, Species::Heron => 0xb8b2a4,
            Species::Owl => 0x7d6d57, Species::Grouse => 0x7a5636,
        }
    }
    /// Rough body scale.
    pub fn size(self) -> f32 {
        match self {
            Species::Deer => 1.0, Species::Heron => 0.85, Species::Owl => 0.42, Species::Hare => 0.38,
            Species::Grouse => 0.34, Species::Duck => 0.32, Species::Squirrel => 0.24, Species::Frog => 0.16,
        }
    }
}

/// Activity level 0..1 for a species given the time of day. Nocturnal animals
/// are livelier at night, diurnal ones by day, with a resting baseline.
pub fn activity(nocturnality: f32, is_night: bool) -> f32 {
    let night_factor = if is_night { nocturnality } else { 1.0 - nocturnality };
    (0.3 + 0.7 * night_factor).clamp(0.0, 1.0)
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CritterState { Graze, Wander, Flee, Alert }

/// One ambient animal with its FSM state.
#[derive(Clone, Copy, Debug)]
pub struct Critter {
    pub species: Species,
    pub x: f32,
    pub z: f32,
    pub heading: f32,
    pub state: CritterState,
    pub timer: f32,   // seconds left in the current state's intent
    pub phase: f32,   // animation phase (bob / hop)
    pub vx: f32,
    pub vz: f32,
    seed: u32,
}

fn hashf(mut s: u32) -> f32 {
    // cheap deterministic LCG-ish scramble → [0,1)
    s = s.wrapping_mul(747796405).wrapping_add(2891336453);
    let w = ((s >> ((s >> 28).wrapping_add(4))) ^ s).wrapping_mul(277803737);
    ((w >> 22) ^ w) as f32 / u32::MAX as f32
}

impl Critter {
    pub fn new(species: Species, x: f32, z: f32, seed: u32) -> Self {
        Critter {
            species, x, z, heading: hashf(seed) * std::f32::consts::TAU,
            state: CritterState::Graze, timer: 1.0 + hashf(seed ^ 0x9e3) * 2.0,
            phase: hashf(seed ^ 0x55) * 6.28, vx: 0.0, vz: 0.0, seed,
        }
    }

    /// Advance one tick. `threats` are the positions the animal is scared of
    /// (the hunter and any cats). This is the sense→transition→act loop.
    pub fn step(&mut self, dt: f32, threats: &[(f32, f32)]) {
        self.timer -= dt;
        self.phase += dt * if self.state == CritterState::Flee { 14.0 } else { 4.0 };
        self.seed = self.seed.wrapping_add(1);
        let r = hashf(self.seed);

        // ── sense: nearest threat ──
        let mut nd = f32::MAX;
        let mut nx = 0.0; let mut nz = 0.0;
        for &(tx, tz) in threats {
            let d = (tx - self.x).hypot(tz - self.z);
            if d < nd { nd = d; nx = tx; nz = tz; }
        }
        let fr = self.species.flee_range();

        // ── transition ──
        match self.state {
            CritterState::Flee => {
                if nd > fr * 1.7 || self.timer <= 0.0 {
                    self.state = CritterState::Alert;
                    self.timer = 0.8 + r * 1.2;
                }
            }
            CritterState::Alert => {
                if nd < fr { self.state = CritterState::Flee; self.timer = 1.4 + r * 1.4; }
                else if self.timer <= 0.0 { self.state = CritterState::Graze; self.timer = 1.5 + r * 2.5; }
            }
            CritterState::Graze => {
                if nd < fr { self.state = CritterState::Flee; self.timer = 1.4 + r * 1.4; }
                else if nd < fr * 1.8 { self.state = CritterState::Alert; self.timer = 0.6 + r; }
                else if self.timer <= 0.0 { self.state = CritterState::Wander; self.timer = 1.0 + r * 2.0; self.heading = r * std::f32::consts::TAU; }
            }
            CritterState::Wander => {
                if nd < fr { self.state = CritterState::Flee; self.timer = 1.4 + r * 1.4; }
                else if self.timer <= 0.0 { self.state = CritterState::Graze; self.timer = 1.5 + r * 2.5; }
            }
        }

        // ── act ──
        let (tvx, tvz) = match self.state {
            CritterState::Flee => {
                // sprint directly away from the threat
                let dx = self.x - nx; let dz = self.z - nz;
                let l = dx.hypot(dz).max(0.001);
                self.heading = dz.atan2(dx);
                (dx / l * self.species.speed(), dz / l * self.species.speed())
            }
            CritterState::Alert => (0.0, 0.0), // freeze and watch
            CritterState::Graze => {
                // tiny nibbling drift
                (self.heading.cos() * 0.4, self.heading.sin() * 0.4)
            }
            CritterState::Wander => {
                let amble = self.species.speed() * 0.28;
                (self.heading.cos() * amble, self.heading.sin() * amble)
            }
        };
        // ease toward target velocity, integrate
        self.vx += (tvx - self.vx) * (dt * 6.0).min(1.0);
        self.vz += (tvz - self.vz) * (dt * 6.0).min(1.0);
        let (px, pz) = (self.x, self.z);
        self.x += self.vx * dt;
        self.z += self.vz * dt;

        // land species won't wade into deep water — turn back at the shoreline.
        // Only blocks a land→water crossing; a critter already in water can still
        // move (so it can climb back out) rather than getting stuck.
        if self.species.avoids_water()
            && world::height_at(px, pz) >= world::WATER_Y
            && world::height_at(self.x, self.z) < world::WATER_Y {
            self.x = px; self.z = pz;
            self.heading += std::f32::consts::PI;
            self.vx = -self.vx * 0.5; self.vz = -self.vz * 0.5;
        }

        // keep on the map, turn back at the edge
        let lim = world::SIZE * 0.9;
        if self.x.abs() > lim { self.x = self.x.clamp(-lim, lim); self.heading += std::f32::consts::PI; }
        if self.z.abs() > lim { self.z = self.z.clamp(-lim, lim); self.heading += std::f32::consts::PI; }
    }

    pub fn y(&self) -> f32 { world::height_at(self.x, self.z) }
}

/// Seed a population across the map, weighted by each biome's habitat suitability
/// and the species' day/night activity. `count` candidate sites are tried.
pub fn populate(count: usize, is_night: bool) -> Vec<Critter> {
    let mut out = Vec::new();
    for i in 0..count {
        let s = i as u32;
        let x = (hashf(s ^ 0x1111) - 0.5) * world::SIZE * 2.6;
        let z = (hashf(s ^ 0x2222) - 0.5) * world::SIZE * 2.6;
        let y = world::height_at(x, z);
        if y < world::WATER_Y { continue; }                    // no critters mid-lake
        let biome = world::biome_at(x, z).name;

        // pick the species best suited here (weighted by habitat × activity)
        let mut best = None; let mut best_score = 0.0f32;
        for (k, &sp) in SPECIES.iter().enumerate() {
            let w = sp.habitat_weight(biome) as f32;
            if w <= 0.0 { continue; }
            let act = activity(sp.nocturnality(), is_night);
            let jitter = 0.5 + hashf(s ^ (0x777 + k as u32 * 131)) ;
            let score = w * act * jitter;
            if score > best_score { best_score = score; best = Some(sp); }
        }
        if let Some(sp) = best {
            // suitability gate: sparse where the land barely supports life
            if hashf(s ^ 0x9999) < (best_score / 12.0).clamp(0.05, 0.95) {
                out.push(Critter::new(sp, x, z, s ^ 0xABCD));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flees_when_threat_close() {
        let mut c = Critter::new(Species::Hare, 0.0, 0.0, 1);
        c.step(0.1, &[(3.0, 0.0)]);   // threat well within flee range
        assert_eq!(c.state, CritterState::Flee);
    }

    #[test]
    fn flee_runs_away_from_threat() {
        let mut c = Critter::new(Species::Hare, 0.0, 0.0, 2);
        for _ in 0..10 { c.step(0.1, &[(5.0, 0.0)]); }   // threat to the +x side
        assert!(c.x < 0.0, "hare should flee to -x, got {}", c.x);
    }

    #[test]
    fn calms_down_when_safe() {
        let mut c = Critter::new(Species::Deer, 0.0, 0.0, 3);
        c.step(0.1, &[(2.0, 0.0)]);
        assert_eq!(c.state, CritterState::Flee);
        // no threats nearby now; after enough time it should stop fleeing
        for _ in 0..80 { c.step(0.1, &[(999.0, 999.0)]); }
        assert_ne!(c.state, CritterState::Flee);
    }

    #[test]
    fn habitat_matches_biome() {
        assert!(Species::Duck.habitat_weight("Lake Shore") > Species::Duck.habitat_weight("Stony Mountains"));
        assert!(Species::Squirrel.habitat_weight("Oak Tree Forest") > Species::Squirrel.habitat_weight("Moor Plains"));
        assert_eq!(Species::Duck.habitat_weight("Stony Mountains"), 0);
    }

    #[test]
    fn nocturnal_activity_flips_with_night() {
        assert!(activity(1.0, true) > activity(1.0, false));   // owl livelier at night
        assert!(activity(0.0, false) > activity(0.0, true));   // hare livelier by day
    }

    #[test]
    fn land_critter_stays_out_of_deep_water() {
        world::init_world();
        // find a shoreline: a land cell whose +x neighbour is deep water
        let mut spot = None;
        'scan: for gx in -70..70 {
            for gz in -70..70 {
                let (x, z) = (gx as f32 * 2.0, gz as f32 * 2.0);
                if world::height_at(x, z) >= world::WATER_Y
                    && world::height_at(x + 4.0, z) < world::WATER_Y - 0.5 {
                    spot = Some((x, z)); break 'scan;
                }
            }
        }
        let (x, z) = spot.expect("the map should have a shoreline somewhere");
        let mut c = Critter::new(Species::Hare, x, z, 42);
        // threat just behind on the land side → it flees straight toward the water
        for _ in 0..60 {
            c.step(1.0 / 30.0, &[(x - 6.0, z)]);
            assert!(world::height_at(c.x, c.z) >= world::WATER_Y - 1e-3,
                "a hare must not wade into deep water (ended at {:.1},{:.1})", c.x, c.z);
        }
    }

    #[test]
    fn population_is_deterministic_and_placed() {
        world::init_world();
        let a = populate(400, false);
        let b = populate(400, false);
        assert_eq!(a.len(), b.len());
        assert!(!a.is_empty(), "should seed some wildlife");
        // every critter sits on land
        for c in &a { assert!(c.y() >= world::WATER_Y - 0.01); }
    }
}
