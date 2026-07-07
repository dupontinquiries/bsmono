//! A flock of sentinels — the orchestration layer the bin holds.
//!
//! Ties the whole perception loop together over many creatures so the bin only
//! has to: build a [`Flock`] from the creatures it spawned, call [`Flock::tick`]
//! once per frame with the world + the threats (typically just the player) + the
//! observer position, and react to the [`Alarm`]s that come back (a HUD ping,
//! the flock's visible scatter). Each member runs `sense_threats -> observe_tick`,
//! so distance LOD, the labelled batch queue, and the arousal cascade all apply
//! per creature for free.
//!
//! Pure and world-generic (testable against any [`WorldQuery`]); the bin supplies
//! the live world and the player's position/loudness.

use crate::sim::cognition::sentinel_agent::SentinelAgent;
use crate::sim::fixed::Vec3;
use crate::sim::physics::WorldQuery;
use crate::sim::sentinel::Alarm;
use crate::sim::sound::Loudness;

/// One sentinel placed in the world: its brain/agent, where it is, and the
/// horizontal facings of its living eyes (from `Anatomy::eye_facings`).
#[derive(Debug, Clone)]
pub struct SentinelInstance {
    pub agent: SentinelAgent,
    pub pos: Vec3,
    pub eye_facings: Vec<(f32, f32)>,
}

/// A collection of sentinels updated together each tick.
#[derive(Debug, Clone, Default)]
pub struct Flock {
    members: Vec<SentinelInstance>,
}

impl Flock {
    pub fn new() -> Flock {
        Flock { members: Vec::new() }
    }

    pub fn len(&self) -> usize {
        self.members.len()
    }

    pub fn is_empty(&self) -> bool {
        self.members.is_empty()
    }

    pub fn add(&mut self, agent: SentinelAgent, pos: Vec3, eye_facings: Vec<(f32, f32)>) {
        self.members.push(SentinelInstance { agent, pos, eye_facings });
    }

    /// Read-only view of the members (for rendering posture, etc.).
    pub fn members(&self) -> &[SentinelInstance] {
        &self.members
    }

    /// Advance every sentinel one tick against the world and the current threats
    /// (each `(position, emitted loudness)` — usually just the player).
    /// `observer` is what distance-LOD is measured from (the camera/player).
    /// Returns `(member index, Alarm)` for each sentinel revealing something this
    /// tick — the reveals the bin surfaces to the player.
    pub fn tick<W: WorldQuery>(
        &mut self,
        world: &W,
        threats: &[(Vec3, Loudness)],
        observer: Vec3,
    ) -> Vec<(usize, Alarm)> {
        let mut alarms = Vec::new();
        for (i, m) in self.members.iter_mut().enumerate() {
            let d = m.pos - observer;
            let (dx, dy, dz) = (d.x.to_f32(), d.y.to_f32(), d.z.to_f32());
            let dist2 = (dx * dx + dy * dy + dz * dz) as i64;
            let (sighted, heard) = m.agent.sense_threats(m.pos, &m.eye_facings, threats, world);
            if let Some(a) = m.agent.observe_tick(dist2, sighted, heard) {
                alarms.push((i, a));
            }
        }
        alarms
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sim::cognition::sentinel_agent::SentinelAgent;
    use crate::sim::fixed::Fixed;
    use crate::sim::physics::SceneWorld;
    use crate::sim::sentinel::{Posture, Senses};

    fn at(x: i64, z: i64) -> Vec3 {
        Vec3::new(Fixed::from_int(x), Fixed::ZERO, Fixed::from_int(z))
    }
    fn agent() -> SentinelAgent {
        SentinelAgent::new(Senses::bird(), [100, 400, 900])
    }

    #[test]
    fn a_member_facing_a_nearby_player_raises_the_alarm() {
        let world = SceneWorld::new(); // open air
        let mut flock = Flock::new();
        // One bird at the origin, eye facing +X (toward the player).
        flock.add(agent(), at(0, 0), vec![(1.0, 0.0)]);
        let player = at(6, 0);
        let loud = Senses::bird().hear_threshold * 3;
        let alarms = flock.tick(&world, &[(player, loud)], player);
        assert_eq!(alarms.len(), 1, "the bird should reveal the player");
        assert_eq!(alarms[0].0, 0);
        assert_eq!(alarms[0].1.posture, Posture::Alarmed);
    }

    #[test]
    fn a_distant_member_out_of_range_stays_quiet() {
        let world = SceneWorld::new();
        let mut flock = Flock::new();
        // A bird far from the player and facing away.
        flock.add(agent(), at(500, 0), vec![(-1.0, 0.0)]);
        let player = at(0, 0);
        // Even a loud player is out of sight range and attenuated below threshold.
        let alarms = flock.tick(&world, &[(player, Senses::bird().hear_threshold)], player);
        assert!(alarms.is_empty(), "too far to see or hear → no reveal");
    }

    #[test]
    fn only_the_members_that_sense_the_threat_raise_alarms() {
        let world = SceneWorld::new();
        let mut flock = Flock::new();
        flock.add(agent(), at(0, 0), vec![(1.0, 0.0)]); // near, facing player
        flock.add(agent(), at(800, 800), vec![(1.0, 0.0)]); // far away
        let player = at(5, 0);
        let loud = Senses::bird().hear_threshold * 3;
        let alarms = flock.tick(&world, &[(player, loud)], player);
        assert_eq!(alarms.len(), 1);
        assert_eq!(alarms[0].0, 0, "only the near bird reveals the player");
    }
}
