//! Server-authoritative cognitive simulation — the loop that actually makes
//! creatures think.
//!
//! Each tick this runs the [`CognitionService`] (which writes every mob's
//! `commanded` heading into the state) and then the deterministic
//! [`step`](crate::sim::state::step). Mobs are server-authoritative: they don't
//! roll back. Cognition runs once per tick at the leading edge, and because
//! `commanded` lives in the mob state, the state this produces stays
//! snapshot/replay-consistent when it feeds a client-side rollback driver (which
//! predicts players and consumes the authoritative mob state).
//!
//! This is deliberately the *safe* integration seam — it composes the public
//! `step` and never mutates the rollback core. Threading AI into the rollback
//! prediction path itself (so predicted ticks also think) is a separate
//! decision: it needs an AI-input channel in [`Rollback`](crate::sim::rollback)
//! analogous to player input so it replays deterministically, which touches the
//! deterministic core and should be designed and tested on purpose, not wired
//! in blind.

use crate::sim::cognition::service::CognitionService;
use crate::sim::physics::WorldQuery;
use crate::sim::state::{step, Input, SimState};

/// Owns the authoritative sim state and the cognition that drives its mobs.
pub struct CognitiveSim {
    pub state: SimState,
    pub cognition: CognitionService,
}

impl CognitiveSim {
    pub fn new(state: SimState, cognition: CognitionService) -> Self {
        Self { state, cognition }
    }

    /// Advance one server tick: cognition writes each mob's command, then the
    /// deterministic sim steps with the given per-player `inputs`.
    pub fn tick<W: WorldQuery>(&mut self, inputs: &[Input], world: &W) {
        self.cognition.update(&mut self.state, world);
        self.state = step(&self.state, inputs, world);
    }

    pub fn state(&self) -> &SimState {
        &self.state
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sim::fixed::{Fixed, Vec3};
    use crate::sim::mob::Mob;
    use crate::sim::personality::Disposition;
    use crate::sim::physics::FlatWorld;

    fn greedy() -> Disposition {
        let f = |x: i64| Fixed::from_frac(x, 100);
        Disposition { aggression: f(30), territoriality: f(30), caution: f(30), curiosity: f(40), greed: f(95) }
    }

    fn scene(player_x: i64) -> SimState {
        let mut s = SimState::new(1);
        s.players[0].pos = Vec3 { x: Fixed::from_int(player_x), y: Fixed::ZERO, z: Fixed::ZERO };
        s.mobs.push(Mob::new_seeker(Vec3::default(), 20)); // at origin, facing East
        s
    }

    #[test]
    fn cognition_drives_a_chase() {
        let world = FlatWorld::new();
        let mut svc = CognitionService::with_cadence(1);
        svc.dispositions = vec![greedy()];
        let mut sim = CognitiveSim::new(scene(12), svc);

        let start_x = sim.state.mobs[0].body.pos.x.0;
        sim.tick(&[], &world);
        assert_eq!(sim.state.mobs[0].commanded, Some(0), "first tick commands East toward the prey");
        for _ in 0..59 {
            sim.tick(&[], &world);
        }
        let end_x = sim.state.mobs[0].body.pos.x.0;
        assert!(end_x > start_x, "the mob closes on the prey at +x ({start_x} -> {end_x})");
        assert_eq!(sim.state.tick.0, 60, "sixty ticks advanced");
    }

    #[test]
    fn no_command_without_visible_prey() {
        let world = FlatWorld::new();
        let mut svc = CognitionService::with_cadence(1);
        svc.dispositions = vec![greedy()];
        let mut sim = CognitiveSim::new(scene(100), svc); // prey beyond sight range
        sim.tick(&[], &world);
        assert_eq!(sim.state.mobs[0].commanded, None, "unseen prey yields no command");
    }
}
