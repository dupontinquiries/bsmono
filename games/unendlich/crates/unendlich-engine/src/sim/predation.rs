//! Predation — perception-gated hunting.
//!
//! A predator finds prey by sight (if it has eyes) or by hearing (if it has
//! ears *and* the world has sound enabled). With neither, it cannot locate prey
//! and never eats. Eating also requires a bite (jaws). This is the rule the
//! dragon brain will consult; it is kept pure and 1-D so the behaviour is easy
//! to prove.

/// What a predator can perceive and do, distilled from its capabilities plus
/// its ranges and speed.
#[derive(Clone, Copy, Debug)]
pub struct Predator {
    pub can_see: bool,
    pub can_hear: bool,
    pub can_bite: bool,
    /// How far it can spot prey by sight (units).
    pub sight_range: f32,
    /// How far it can locate prey by sound (units).
    pub hearing_range: f32,
    /// Approach speed toward detected prey (units per tick).
    pub speed: f32,
    /// Distance within which it can bite/eat.
    pub bite_range: f32,
}

impl Predator {
    /// Build a predator from a creature's capabilities. Sight/hearing/bite come
    /// straight from the body plan (eyes → see, ears → hear, jaws → eat).
    pub fn from_capabilities(
        c: &crate::creature::Capabilities,
        sight_range: f32,
        hearing_range: f32,
        speed: f32,
        bite_range: f32,
    ) -> Predator {
        Predator {
            can_see: c.see,
            can_hear: c.hear,
            can_bite: c.eat,
            sight_range,
            hearing_range,
            speed,
            bite_range,
        }
    }
}

/// Can the predator currently detect prey at `dist`, given the world sound flag?
/// Sight needs eyes + range; hearing needs ears + range + the world's sound on.
pub fn detect(p: &Predator, dist: f32, sound_enabled: bool) -> bool {
    let seen = p.can_see && dist <= p.sight_range;
    let heard = p.can_hear && sound_enabled && dist <= p.hearing_range;
    seen || heard
}

/// Outcome of a hunt.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Hunt {
    /// Did the predator reach and bite the prey?
    pub ate: bool,
    /// Ticks elapsed (== `max_ticks` if it never ate).
    pub ticks: u32,
    /// Did it ever sense the prey at all?
    pub detected: bool,
}

/// Simulate a straight-line chase: prey sits `prey_dist` away. Each tick, if the
/// predator can detect the prey it closes by `speed` and, once within
/// `bite_range`, eats (needs jaws). If it cannot detect the prey it makes no
/// progress (a blind, soundless predator never finds it).
pub fn hunt(p: &Predator, prey_dist: f32, sound_enabled: bool, max_ticks: u32) -> Hunt {
    let mut dist = prey_dist.max(0.0);
    let mut detected = false;
    for t in 0..max_ticks {
        let det = detect(p, dist, sound_enabled);
        detected |= det;
        if det && p.can_bite && dist <= p.bite_range {
            return Hunt { ate: true, ticks: t, detected: true };
        }
        if det {
            dist = (dist - p.speed.max(0.0)).max(0.0);
        }
        // No detection → no progress this tick.
    }
    Hunt { ate: false, ticks: max_ticks, detected }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pred(see: bool, hear: bool, bite: bool) -> Predator {
        Predator {
            can_see: see,
            can_hear: hear,
            can_bite: bite,
            sight_range: 60.0,
            hearing_range: 40.0,
            speed: 3.0,
            bite_range: 2.0,
        }
    }

    #[test]
    fn sighted_predator_eats_with_or_without_sound() {
        let p = pred(true, false, true);
        assert!(hunt(&p, 30.0, true, 500).ate);
        assert!(hunt(&p, 30.0, false, 500).ate, "sight does not need sound");
    }

    #[test]
    fn blind_and_silent_world_never_finds_prey() {
        // No eyes, has ears, but the world has no sound.
        let p = pred(false, true, true);
        let h = hunt(&p, 30.0, false, 500);
        assert!(!h.ate && !h.detected, "no sight + no sound → can't locate prey");
    }

    #[test]
    fn blind_predator_eats_only_when_sound_enabled() {
        let p = pred(false, true, true);
        assert!(hunt(&p, 30.0, true, 500).ate, "ears + sound → hears and eats");
        assert!(!hunt(&p, 30.0, false, 500).ate, "ears but no sound → no meal");
    }

    #[test]
    fn deaf_and_blind_cannot_hunt() {
        let p = pred(false, false, true);
        assert!(!hunt(&p, 30.0, true, 500).ate);
    }

    #[test]
    fn cannot_eat_without_jaws() {
        // Sees fine, reaches the prey, but has no bite → never eats.
        let p = pred(true, false, false);
        let h = hunt(&p, 30.0, true, 500);
        assert!(!h.ate && h.detected, "detects but cannot bite");
    }

    #[test]
    fn prey_out_of_all_ranges_is_not_detected() {
        let p = pred(true, true, true); // sight 60, hearing 40
        assert!(!hunt(&p, 100.0, true, 1).detected);
    }
}
