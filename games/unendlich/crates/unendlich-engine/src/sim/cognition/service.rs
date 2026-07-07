//! The cognition service — the server-side pass that *drives* creatures with
//! the [`Mind`] stack. It holds one mind per mob, and at a fixed cadence:
//! builds each creature's perception, runs its mind, and writes the resulting
//! [`Intent`](crate::sim::cognition::mind::Intent) heading into
//! `mob.commanded` for the deterministic `step` to act on.
//!
//! This runs *outside* the rolled-back core (minds and the flow-field pathing
//! are `f32` / server-side); the only thing crossing back in is the integer
//! commanded heading, which lives on the mob and so replays correctly.
//!
//! Wiring: call [`CognitionService::update`] once per sim tick, before stepping
//! the sim. The internal counter gates the heavy work to `cadence` ticks, so
//! between cognition ticks each creature simply keeps walking its last heading.

use crate::sim::cognition::appraisal::{SelfState, Sighting};
use crate::sim::cognition::memory::{CellPos, LandmarkTag};
use crate::sim::cognition::mind::Mind;
use crate::sim::cognition::navigate::{Locomotion, NavRegion};
use crate::sim::fixed::{Fixed, Vec3};
use crate::sim::mob::MobKind;
use crate::sim::personality::Disposition;
use crate::sim::physics::WorldQuery;
use crate::sim::state::SimState;
use crate::sim::vision::{can_see, FOV_COS2_90DEG, SIGHT_RANGE};

/// Cognition ticks every this many sim ticks (10 ⇒ ~6 Hz at 60 Hz).
const DEFAULT_CADENCE: u32 = 10;
/// Eye height above feet, matching the mob brain's vision origin.
const EYE_HEIGHT: Fixed = Fixed::from_frac(15, 10);
/// Perceived salience of a sighted prey/player (vs. `appraisal::STRENGTH_NORM`).
const PREY_STRENGTH: u16 = 48;
/// Health that counts as "full" when normalizing `health_frac`.
const REF_HEALTH: i64 = 20;
/// Local nav window: half-extent, vertical extent, and cells below the feet.
const REGION_RADIUS: i32 = 16;
const REGION_HEIGHT: usize = 4;
const REGION_DROP: i32 = 1;

/// Drives every creature's cognition over a [`SimState`].
#[derive(Clone, Debug)]
pub struct CognitionService {
    /// One mind per mob (kept in step with `state.mobs` by index).
    pub minds: Vec<Mind>,
    /// Optional authored temperaments: where index `i` is present it overrides
    /// the seed-derived disposition (hand-tune a specific creature, e.g. a named
    /// boss); otherwise [`Disposition::from_seed`] is used.
    pub dispositions: Vec<Disposition>,
    /// Sim ticks between cognition passes.
    pub cadence: u32,
    /// Hunger drive fed to every creature until a metabolism model exists.
    pub hunger: Fixed,
    counter: u32,
}

impl CognitionService {
    pub fn new() -> Self {
        Self {
            minds: Vec::new(),
            dispositions: Vec::new(),
            cadence: DEFAULT_CADENCE,
            hunger: Fixed::from_frac(8, 10),
            counter: 0,
        }
    }

    /// Service with an explicit cadence (clamped to at least 1).
    pub fn with_cadence(cadence: u32) -> Self {
        Self { cadence: cadence.max(1), ..Self::new() }
    }

    fn disposition_for(&self, i: usize, seed: u64) -> Disposition {
        self.dispositions
            .get(i)
            .copied()
            .unwrap_or_else(|| Disposition::from_seed(seed, i as u64))
    }

    /// Advance cognition for the whole state. Call once per sim tick *before*
    /// stepping; the heavy work runs only every `cadence` ticks.
    pub fn update<W: WorldQuery>(&mut self, state: &mut SimState, world: &W) {
        self.counter += 1;
        if self.counter < self.cadence {
            return;
        }
        self.counter = 0;

        if self.minds.len() != state.mobs.len() {
            self.minds.resize_with(state.mobs.len(), Mind::new);
        }
        let now = state.tick.0;
        let seed = state.world_seed;

        for i in 0..state.mobs.len() {
            // Copy decouples the read from the &mut write at the end.
            let mob = state.mobs[i];
            let observer = cell_of(mob.body.pos);
            let loco = match mob.kind {
                MobKind::Flyer => Locomotion::Fly,
                _ => Locomotion::Walk,
            };

            // Perception: each player the creature can actually see (forward
            // cone + range + line-of-sight) becomes a mobile food source, so a
            // hungry predator raids it. Structure/terrain perception arrives
            // with the GPU visibility pass.
            let eye = Vec3 { x: mob.body.pos.x, y: mob.body.pos.y + EYE_HEIGHT, z: mob.body.pos.z };
            let (fx, fz) = mob.facing();
            let mut sightings: Vec<Sighting> = Vec::new();
            for p in &state.players {
                let target = Vec3 { x: p.pos.x, y: p.pos.y + EYE_HEIGHT, z: p.pos.z };
                if can_see(eye, fx, fz, target, SIGHT_RANGE, FOV_COS2_90DEG, world) {
                    sightings.push(Sighting {
                        pos: cell_of(p.pos),
                        tag: LandmarkTag::FoodSource,
                        strength: PREY_STRENGTH,
                    });
                }
            }

            let self_state = SelfState {
                health_frac: Fixed::from_frac(mob.health.clamp(0, REF_HEALTH as i16) as i64, REF_HEALTH),
                hunger: self.hunger,
            };
            let disp = self.disposition_for(i, seed);
            let region = NavRegion::around(observer, REGION_RADIUS, REGION_HEIGHT, REGION_DROP);

            let intent = self.minds[i]
                .tick(observer, &sightings, self_state, &disp, &region, loco, world, now);
            state.mobs[i].commanded = intent.heading;
        }
    }
}

impl Default for CognitionService {
    fn default() -> Self {
        Self::new()
    }
}

fn cell_of(p: Vec3) -> CellPos {
    (p.x.floor_int() as i32, p.y.floor_int() as i32, p.z.floor_int() as i32)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sim::mob::Mob;
    use crate::sim::physics::FlatWorld;

    fn disp(a: i64, t: i64, c: i64, cu: i64, g: i64) -> Disposition {
        let f = |x: i64| Fixed::from_frac(x, 100);
        Disposition { aggression: f(a), territoriality: f(t), caution: f(c), curiosity: f(cu), greed: f(g) }
    }

    /// One seeker at the origin facing East (+x); a player ahead or behind.
    fn scene(player_x: i64) -> SimState {
        let mut s = SimState::new(1);
        s.players[0].pos = Vec3 { x: Fixed::from_int(player_x), y: Fixed::ZERO, z: Fixed::ZERO };
        s.mobs.push(Mob::new_seeker(Vec3::default(), 20));
        s
    }

    #[test]
    fn raids_visible_prey() {
        let world = FlatWorld::new();
        let mut svc = CognitionService::with_cadence(1);
        svc.dispositions = vec![disp(30, 30, 30, 40, 95)]; // greedy
        let mut state = scene(5); // player 5 east, in the seeker's cone
        svc.update(&mut state, &world);
        assert_eq!(state.mobs[0].commanded, Some(0), "should be commanded East, toward the prey");
    }

    #[test]
    fn ignores_prey_it_cannot_see() {
        let world = FlatWorld::new();
        let mut svc = CognitionService::with_cadence(1);
        svc.dispositions = vec![disp(30, 30, 30, 40, 95)];
        let mut state = scene(-5); // player behind the East-facing seeker
        svc.update(&mut state, &world);
        assert_eq!(state.mobs[0].commanded, None, "out-of-cone prey yields no command");
    }

    #[test]
    fn cadence_gates_the_work() {
        let world = FlatWorld::new();
        let mut svc = CognitionService::with_cadence(10);
        svc.dispositions = vec![disp(30, 30, 30, 40, 95)];
        let mut state = scene(5);
        for _ in 0..9 {
            svc.update(&mut state, &world);
            assert_eq!(state.mobs[0].commanded, None, "no cognition before the cadence boundary");
        }
        svc.update(&mut state, &world); // 10th call
        assert_eq!(state.mobs[0].commanded, Some(0), "cognition runs on the cadence tick");
    }
}
