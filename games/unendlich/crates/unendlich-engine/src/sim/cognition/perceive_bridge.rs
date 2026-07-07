//! Bridge: the reflex perception layer feeds the deliberative `Mind`.
//!
//! The engine already has a goal-arbitrating [`Mind`](super::mind::Mind) that
//! turns `Sighting`s + a `SelfState` into an `Intent`. The sentinel and forager
//! are the front-end for it, not a rival brain: this module converts a
//! sentinel's per-tick detections into threat [`Sighting`]s and a forager's
//! hunger into the Mind's [`SelfState`]. With it, the same eyes/ears that raise
//! a reflex alarm also feed the slower, personality-driven planner — one
//! perception source, two consumers.

use crate::sim::cognition::appraisal::{SelfState, Sighting};
use crate::sim::cognition::forager::Forager;
use crate::sim::cognition::memory::{CellPos, LandmarkTag};
use crate::sim::cognition::mind::{Intent, Mind};
use crate::sim::cognition::navigate::{Locomotion, NavRegion};
use crate::sim::fixed::{Fixed, Vec3};
use crate::sim::personality::Disposition;
use crate::sim::physics::WorldQuery;
use crate::sim::sound::Loudness;

/// Strength assigned to a *seen* threat (sight is high-confidence; outranks any
/// heard cue when the Mind picks the most prominent sighting).
const SIGHT_STRENGTH: u16 = 200;

fn cell_of(p: Vec3) -> CellPos {
    (p.x.to_f32().floor() as i32, p.y.to_f32().floor() as i32, p.z.to_f32().floor() as i32)
}

/// Convert a sentinel's per-tick detections (from `SentinelAgent::sense_threats`)
/// into threat sightings the `Mind` can appraise. A seen entity is a strong
/// sighting; a heard one is scaled by its perceived loudness.
pub fn threat_sightings(sighted: Option<Vec3>, heard: Option<(Vec3, Loudness)>) -> Vec<Sighting> {
    let mut out = Vec::new();
    if let Some(p) = sighted {
        out.push(Sighting { pos: cell_of(p), tag: LandmarkTag::ThreatStructure, strength: SIGHT_STRENGTH });
    }
    if let Some((p, loud)) = heard {
        let strength = loud.clamp(0, u16::MAX as i32) as u16;
        out.push(Sighting { pos: cell_of(p), tag: LandmarkTag::ThreatStructure, strength });
    }
    out
}

/// Map a forager's hunger (and a health fraction the caller supplies) into the
/// Mind's [`SelfState`]. Hunger is normalised to `[0,1]` against the forager's
/// own cap.
pub fn self_state(forager: &Forager, health_frac: Fixed) -> SelfState {
    let hunger = if forager.max == 0 {
        Fixed::ZERO
    } else {
        Fixed::from_frac(forager.hunger as i64, forager.max as i64)
    };
    SelfState { health_frac, hunger }
}

/// One deliberative tick driven straight from sentinel perception: convert this
/// tick's detections + the forager's state into the `Mind`'s inputs and run it.
/// This is the seam the bin uses for goal-driven creatures — the sentinel is the
/// eyes/ears, the `Mind` does the planning.
#[allow(clippy::too_many_arguments)]
pub fn mind_tick_from_perception<W: WorldQuery>(
    mind: &mut Mind,
    observer_cell: CellPos,
    sighted: Option<Vec3>,
    heard: Option<(Vec3, Loudness)>,
    forager: &Forager,
    health_frac: Fixed,
    disposition: &Disposition,
    region: &NavRegion,
    loco: Locomotion,
    world: &W,
    now: u64,
) -> Intent {
    let sightings = threat_sightings(sighted, heard);
    let ss = self_state(forager, health_frac);
    mind.tick(observer_cell, &sightings, ss, disposition, region, loco, world, now)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(x: i64, z: i64) -> Vec3 {
        Vec3::new(Fixed::from_int(x), Fixed::from_int(2), Fixed::from_int(z))
    }

    #[test]
    fn a_seen_threat_becomes_a_strong_sighting_at_its_cell() {
        let s = threat_sightings(Some(v(5, -3)), None);
        assert_eq!(s.len(), 1);
        assert_eq!(s[0].tag, LandmarkTag::ThreatStructure);
        assert_eq!(s[0].pos, (5, 2, -3));
        assert_eq!(s[0].strength, SIGHT_STRENGTH);
    }

    #[test]
    fn a_heard_threat_strength_tracks_loudness() {
        let quiet = threat_sightings(None, Some((v(1, 1), 40)));
        let loud = threat_sightings(None, Some((v(1, 1), 120)));
        assert_eq!(quiet[0].strength, 40);
        assert_eq!(loud[0].strength, 120);
        assert!(loud[0].strength > quiet[0].strength);
    }

    #[test]
    fn seeing_and_hearing_yields_both_with_sight_strongest() {
        let s = threat_sightings(Some(v(2, 0)), Some((v(9, 9), 50)));
        assert_eq!(s.len(), 2);
        let seen = s.iter().find(|x| x.pos == (2, 2, 0)).unwrap();
        let heard = s.iter().find(|x| x.pos == (9, 2, 9)).unwrap();
        assert!(seen.strength > heard.strength, "sight outranks a faint sound");
    }

    #[test]
    fn hunger_maps_into_self_state_as_a_fraction() {
        let mut f = Forager::new(10); // max = 20
        for _ in 0..20 {
            f.tick();
        }
        let st = self_state(&f, Fixed::from_frac(80, 100));
        assert_eq!(st.hunger, Fixed::from_frac(20, 20), "starving → hunger 1.0");
        assert_eq!(st.health_frac, Fixed::from_frac(80, 100));

        let fresh = Forager::new(10);
        assert_eq!(self_state(&fresh, Fixed::from_frac(100, 100)).hunger, Fixed::ZERO);
    }

    #[test]
    fn seeing_the_player_drives_the_mind_to_a_threat_response() {
        use crate::sim::cognition::navigate::{Locomotion, NavRegion};
        use crate::sim::goals::Goal;
        use crate::sim::physics::{BlockPhysics, SceneWorld};

        // A timid creature: very cautious, barely aggressive — it should avoid,
        // not confront.
        let timid = Disposition {
            aggression: Fixed::from_frac(5, 100),
            territoriality: Fixed::from_frac(10, 100),
            caution: Fixed::from_frac(90, 100),
            curiosity: Fixed::from_frac(20, 100),
            greed: Fixed::from_frac(10, 100),
        };
        let mut world = SceneWorld::new();
        for x in -20..20 {
            for z in -20..20 {
                world.set(x, 0, z, BlockPhysics::solid(Fixed::ZERO));
            }
        }
        let region = NavRegion::around((0, 1, 0), 16, 4, 2);
        let forager = Forager::new(50);
        let mut mind = Mind::new();

        // Run a couple of ticks with the player clearly seen nearby; the mind
        // appraises the threat and commits to a defensive goal.
        let mut intent = Intent { goal: Goal::Wander, target: None, heading: None };
        for now in 0..3 {
            intent = mind_tick_from_perception(
                &mut mind,
                (0, 1, 0),
                Some(v(8, 0)),
                None,
                &forager,
                Fixed::from_frac(90, 100),
                &timid,
                &region,
                Locomotion::Fly,
                &world,
                now,
            );
        }
        assert_ne!(intent.goal, Goal::Wander, "a seen player should provoke a reaction");
        assert!(
            matches!(intent.goal, Goal::Evade | Goal::Flee | Goal::Defend | Goal::Investigate),
            "reaction should be a threat-response goal, got {:?}",
            intent.goal
        );
        // And the mind now knows where the threat is.
        assert!(mind.memory.iter().any(|l| l.pos == (8, 2, 0)), "the sighting was remembered");
    }
}
