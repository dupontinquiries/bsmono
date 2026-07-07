//! Mob combat — landing a hit on a living creature's body and letting the
//! damage change what it can do. Combat magnitude (see `sim::combat`) is routed
//! through the creature's `Anatomy`: the strike picks whichever part its
//! hitbox contains, wounds that part, and we compare the creature's graded
//! capabilities before and after to report what it just lost.
//!
//! This is the payoff of the part-based anatomy: a dragon's flight is gated by
//! its *weakest* wing, so tearing one wing off drops it out of the sky; blinding
//! both eyes leaves it sightless. The dragon brain (`sim::dragon`) can consult
//! [`can_fly`] each tick to decide whether it's still an aerial threat or a
//! grounded one. Pure; world placement and brain hookup are engine wiring.

use crate::creature::{Creature, Part};

/// What a single strike changed about the creature.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct StrikeReport {
    /// Which part absorbed the hit (`None` = the strike missed every hitbox).
    pub hit_part: Option<Part>,
    /// True if that part's HP reached zero this strike.
    pub part_destroyed: bool,
    /// True if the creature could fly before this strike and cannot now.
    pub grounded: bool,
    /// True if the creature had vision before this strike and is now blind.
    pub blinded: bool,
}

/// Whether the creature can currently fly, from its damage-graded capabilities.
/// Creatures without an anatomy can't fly.
pub fn can_fly(creature: &Creature) -> bool {
    creature.graded_capabilities().map(|g| g.can_fly).unwrap_or(false)
}

/// Land a hit of `magnitude` at a body-local `local_point`. Wounds the part the
/// point falls inside and reports any capability the creature lost as a result.
pub fn strike(creature: &mut Creature, local_point: [f32; 3], magnitude: u16) -> StrikeReport {
    let Some(anatomy) = creature.anatomy.as_mut() else {
        return StrikeReport::default();
    };

    let before = anatomy.graded();
    let idx = anatomy.apply_hit(local_point, magnitude);
    let (hit_part, part_destroyed) = match idx {
        Some(i) => (Some(anatomy.parts[i].part), anatomy.parts[i].is_destroyed()),
        None => (None, false),
    };
    let after = anatomy.graded();

    StrikeReport {
        hit_part,
        part_destroyed,
        grounded: before.can_fly && !after.can_fly,
        blinded: before.vision > 0.0 && after.vision <= 0.0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::item::CreatureKind;
    use crate::spawn::spawn;

    // Dragon wing center (matches spawn.rs body plan).
    const WING1: [f32; 3] = [0.0, 0.8, 2.5];
    const EYE1: [f32; 3] = [3.0, 0.6, 0.6];
    const EYE2: [f32; 3] = [3.0, 0.6, -0.6];

    #[test]
    fn tearing_one_wing_grounds_the_dragon() {
        let mut d = spawn(CreatureKind::Dragon, [0.0; 3]);
        assert!(can_fly(&d), "starts airworthy");
        let r = strike(&mut d, WING1, 100); // wing hp is 40 -> destroyed
        assert_eq!(r.hit_part, Some(Part::Wing));
        assert!(r.part_destroyed);
        assert!(r.grounded, "weakest-wing flight gating drops it");
        assert!(!can_fly(&d));
    }

    #[test]
    fn losing_both_eyes_blinds_it() {
        let mut d = spawn(CreatureKind::Dragon, [0.0; 3]);
        let r1 = strike(&mut d, EYE1, 100);
        assert_eq!(r1.hit_part, Some(Part::Eye));
        assert!(!r1.blinded, "one eye left, still sees");
        let r2 = strike(&mut d, EYE2, 100);
        assert!(r2.blinded, "second eye gone -> blind");
    }

    #[test]
    fn a_miss_changes_nothing() {
        let mut d = spawn(CreatureKind::Dragon, [0.0; 3]);
        let r = strike(&mut d, [100.0, 100.0, 100.0], 100);
        assert_eq!(r, StrikeReport::default());
        assert!(can_fly(&d), "still airworthy after a whiff");
    }

    #[test]
    fn a_glancing_wing_hit_does_not_yet_ground() {
        let mut d = spawn(CreatureKind::Dragon, [0.0; 3]);
        let r = strike(&mut d, WING1, 5); // wing hp 40, survives
        assert_eq!(r.hit_part, Some(Part::Wing));
        assert!(!r.part_destroyed);
        assert!(!r.grounded);
        assert!(can_fly(&d));
    }

    #[test]
    fn a_creature_without_anatomy_is_inert_to_strikes() {
        use crate::creature::Creature;
        use crate::physics::PhysicsBody;
        let mut c = Creature::new(PhysicsBody::new_player([0.0; 3]));
        assert_eq!(strike(&mut c, [0.0; 3], 50), StrikeReport::default());
        assert!(!can_fly(&c));
    }
}
