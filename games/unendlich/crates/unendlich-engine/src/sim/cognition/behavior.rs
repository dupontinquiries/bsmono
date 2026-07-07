//! Behavior arbiter — the brain's "what do I do this tick".
//!
//! The sentinel and the forager each produce a piece of state; this folds them
//! into a single movement command with a clear priority: **danger first**, then
//! food, then nothing. The output is the compass heading the cognition layer
//! writes into [`crate::sim::mob::Mob::commanded`] — `None` means "no opinion,
//! fall back to the built-in wander reflex". Pure and world-generic, so it tests
//! against any [`WorldQuery`].
//!
//! It deliberately takes the already-decided pieces (a posture, an optional
//! threat cell, an optional food goal) rather than the whole `Sentinel`/`Forager`
//! — the brain decides *what it knows*, the arbiter decides *what to do about it*.

use crate::sim::cognition::navigate::{flee_heading, heading_toward, Locomotion, NavRegion};
use crate::sim::physics::WorldQuery;
use crate::sim::sentinel::Posture;

/// Which drive won this tick (for telemetry / choosing an animation).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Drive {
    /// Bolting from a known threat (prey).
    Flee,
    /// Closing on detected prey (predator).
    Hunt,
    /// Heading to remembered food.
    Forage,
    /// No pressing need — let the reflex wander.
    Idle,
}

/// How a creature responds to an entity its senses placed: prey runs from it, a
/// predator runs *at* it. The perception is identical — only the sign flips.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Stance {
    #[default]
    Prey,
    Predator,
}

/// The arbiter's verdict: a compass heading for `Mob::commanded` (`None` → reflex)
/// plus which drive produced it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Decision {
    pub heading: Option<u8>,
    pub drive: Drive,
}

/// Fold the brain's state into one command. `stance` decides how a detected
/// entity (`threat` — a threat to prey, quarry to a predator) is treated.
/// Priority:
/// 1. **The entity** — if reacting (`posture != Calm`) and its cell is known:
///    prey flees directly away; a predator routes toward it (`Hunt`).
/// 2. **Food** — else if a `food_goal` is set (the forager already gated this on
///    hunger), route toward it around terrain.
/// 3. **Idle** — otherwise no command; the wander reflex takes over.
pub fn decide<W: WorldQuery>(
    self_cell: (i32, i32, i32),
    stance: Stance,
    posture: Posture,
    threat: Option<(i32, i32, i32)>,
    food_goal: Option<(i32, i32, i32)>,
    region: &NavRegion,
    loco: Locomotion,
    world: &W,
) -> Decision {
    if posture != Posture::Calm {
        if let Some(t) = threat {
            match stance {
                Stance::Prey => {
                    if let Some(h) = flee_heading(self_cell, t) {
                        return Decision { heading: Some(h), drive: Drive::Flee };
                    }
                }
                Stance::Predator => {
                    if let Some(h) = heading_toward(self_cell, t, region, loco, world) {
                        return Decision { heading: Some(h), drive: Drive::Hunt };
                    }
                }
            }
        }
    }
    if let Some(goal) = food_goal {
        if let Some(h) = heading_toward(self_cell, goal, region, loco, world) {
            return Decision { heading: Some(h), drive: Drive::Forage };
        }
    }
    Decision { heading: None, drive: Drive::Idle }
}

/// Drive the maw's open amount in `[0, 1]` from the current drive. A predator
/// that has locked onto prey (`Drive::Hunt`) snaps its jaws open as a strike
/// tell; every other drive eases them shut. The rates are asymmetric — fast to
/// open (the lunge), slow to close — so the gape reads as a deliberate gape-and-
/// hold rather than chattering. `dt` is the frame time in seconds. Feed the
/// result to the jaw-hinge rotation (see `CreatureBlueprint::jaw_hinge`).
pub fn gape_for_drive(current: f32, drive: Drive, dt: f32) -> f32 {
    let target = if matches!(drive, Drive::Hunt) { 1.0 } else { 0.0 };
    let rate = if target > current { 12.0 } else { 3.0 };
    let next = current + (target - current) * (rate * dt).clamp(0.0, 1.0);
    next.clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sim::physics::SceneWorld;

    // Open world; a flat solid floor at y=0 so walkers have ground.
    fn floored() -> SceneWorld {
        use crate::sim::fixed::Fixed;
        use crate::sim::physics::BlockPhysics;
        let mut w = SceneWorld::new();
        for x in -4..16 {
            for z in -4..16 {
                w.set(x, 0, z, BlockPhysics::solid(Fixed::ZERO));
            }
        }
        w
    }

    fn region() -> NavRegion {
        NavRegion::around((4, 1, 4), 10, 4, 2)
    }

    // Compass indices (match navigate::DIR8): 0=E(+x), 2=N(+z), 4=W(-x), 6=S(-z).
    #[test]
    fn alarmed_flees_directly_away_from_the_threat() {
        let w = floored();
        // Threat to the east (+x) → flee west (index 4).
        let d = decide((4, 1, 4), Stance::Prey, Posture::Alarmed, Some((9, 1, 4)), None, &region(), Locomotion::Walk, &w);
        assert_eq!(d.drive, Drive::Flee);
        assert_eq!(d.heading, Some(4));
    }

    #[test]
    fn calm_and_hungry_heads_for_food() {
        let w = floored();
        // Food to the east → head east (index 0). Flying so pathing is simple.
        let d = decide((4, 1, 4), Stance::Prey, Posture::Calm, None, Some((10, 1, 4)), &region(), Locomotion::Fly, &w);
        assert_eq!(d.drive, Drive::Forage);
        assert_eq!(d.heading, Some(0));
    }

    #[test]
    fn danger_beats_food() {
        let w = floored();
        // Both a threat (east) and food (also east) — fear wins, so it flees west.
        let d = decide((4, 1, 4), Stance::Prey, Posture::Alarmed, Some((9, 1, 4)), Some((10, 1, 4)), &region(), Locomotion::Fly, &w);
        assert_eq!(d.drive, Drive::Flee);
        assert_eq!(d.heading, Some(4));
    }

    #[test]
    fn calm_with_nothing_to_do_falls_back_to_reflex() {
        let w = floored();
        let d = decide((4, 1, 4), Stance::Prey, Posture::Calm, None, None, &region(), Locomotion::Walk, &w);
        assert_eq!(d.drive, Drive::Idle);
        assert_eq!(d.heading, None, "no command → built-in wander takes over");
    }

    #[test]
    fn reacting_but_threat_unknown_does_not_fabricate_a_flee() {
        let w = floored();
        // Wary (heard something) but no located threat, and hungry with food east.
        let d = decide((4, 1, 4), Stance::Prey, Posture::Wary, None, Some((10, 1, 4)), &region(), Locomotion::Fly, &w);
        assert_eq!(d.drive, Drive::Forage, "no threat cell → fall through to food");
        assert_eq!(d.heading, Some(0));
    }

    #[test]
    fn a_predator_hunts_the_entity_prey_flees() {
        let w = floored();
        let entity = Some((9, 1, 4)); // detected to the east
        // Same perception, opposite response.
        let prey = decide((4, 1, 4), Stance::Prey, Posture::Alarmed, entity, None, &region(), Locomotion::Fly, &w);
        let pred = decide((4, 1, 4), Stance::Predator, Posture::Alarmed, entity, None, &region(), Locomotion::Fly, &w);
        assert_eq!(prey.drive, Drive::Flee);
        assert_eq!(prey.heading, Some(4), "prey bolts west");
        assert_eq!(pred.drive, Drive::Hunt);
        assert_eq!(pred.heading, Some(0), "predator closes east");
    }

    #[test]
    fn a_calm_predator_with_no_quarry_still_forages() {
        let w = floored();
        // Hasn't locked onto prey, but hungry → goes for remembered food.
        let d = decide((4, 1, 4), Stance::Predator, Posture::Calm, None, Some((10, 1, 4)), &region(), Locomotion::Fly, &w);
        assert_eq!(d.drive, Drive::Forage);
        assert_eq!(d.heading, Some(0));
    }

    #[test]
    fn maw_gapes_open_on_the_hunt_and_eases_shut_after() {
        let dt = 1.0 / 60.0;
        // Spotting prey snaps the jaws open...
        let mut g = 0.0;
        for _ in 0..30 {
            g = gape_for_drive(g, Drive::Hunt, dt);
        }
        assert!(g > 0.9, "the maw gapes wide while hunting (got {g})");
        // ...losing the quarry eases them shut, but slower than they opened.
        let open_step = gape_for_drive(0.0, Drive::Hunt, dt);
        let close_step = 1.0 - gape_for_drive(1.0, Drive::Idle, dt);
        assert!(open_step > close_step, "opens faster ({open_step}) than it closes ({close_step})");
        let mut c = 1.0;
        for _ in 0..200 {
            c = gape_for_drive(c, Drive::Flee, dt);
        }
        assert!(c < 0.05, "eventually closes when not hunting (got {c})");
        // Always clamped to [0,1].
        assert_eq!(gape_for_drive(1.0, Drive::Hunt, 1.0), 1.0);
        assert_eq!(gape_for_drive(0.0, Drive::Idle, 1.0), 0.0);
    }
}
