//! The **Lauscher** ("the Listener") — a blind, cave-dwelling horror that hunts
//! by sound.
//!
//! Lore. Pale, gaunt, and eyeless — a thing that grew up in the lightless damp
//! and never needed eyes, so it gave them up. (Mechanically it *is* near-blind:
//! a creature::Anatomy with no living `Part::Eye`, so `vision_coverage` ≈ 0; it
//! leans entirely on hearing.) Where a Wendigo is hunger and a Rake is the shape
//! at the foot of the bed, the Lauscher is the sense of being *heard*: it knows
//! the wood by its echoes. It imprints a lair — somewhere dark and damp, the
//! conditions of the cave it was born in — and remembers that signature. On
//! nights when the surface falls as dark as its lair, it comes up and walks the
//! treeline, following footfalls. The heavier you are and the louder the ground
//! you cross (dry leaves, loose stone), the farther it hears you; go still on
//! soft moss and it loses the thread, casts about where it last heard you, and —
//! if dawn finds it out — slinks back down to the dark.
//!
//! This module is the creature's deterministic decision core (pure, integer/
//! fixed-point, rolls back with the sim). It plugs into the mob layer: a caller
//! feeds it the surface light, its imprinted conditions, and the loudest sound
//! it hears this tick (computed with `sound::heard_loudness_world` from a
//! player's `sound::footstep_loudness`), and it returns where to go.

use crate::sim::fixed::Vec3;
use crate::sim::sound::Loudness;

/// Darkness (0..=255, = 255 − light) a site must reach to be lair-worthy.
pub const LAIR_MIN_DARKNESS: u8 = 200;
/// Humidity (0..=255) a site must reach to be lair-worthy — caves are damp.
pub const LAIR_MIN_HUMIDITY: u8 = 160;
/// How much brighter than its lair the Lauscher will still tolerate emerging
/// into (so it comes out at dusk, not only at the dead of night).
pub const EMERGE_TOLERANCE: u8 = 40;
/// Perceived loudness below which a sound doesn't register as worth chasing.
pub const HEAR_THRESHOLD: Loudness = 30;
/// Ticks the Lauscher remembers a sound's location after it stops (its trail
/// goes cold); it keeps closing on the last fix until this decays.
pub const TRACK_TICKS: u16 = 80;

/// The environmental signature of the Lauscher's lair: the dark, damp conditions
/// it imprints and later recognises on the surface.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct LairProfile {
    pub darkness: u8,
    pub humidity: u8,
}

/// What the Lauscher is doing right now.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Mode {
    /// Down in the dark, waiting.
    #[default]
    InLair,
    /// Out and patrolling near home, listening.
    Prowling,
    /// Closing on a heard (or last-heard) sound.
    Hunting,
    /// Dawn or daylight — heading back to the lair.
    Returning,
}

/// A site is lair-worthy if it is dark *and* damp enough.
pub fn is_lair_site(light_level: u8, humidity: u8) -> bool {
    (255 - light_level) >= LAIR_MIN_DARKNESS && humidity >= LAIR_MIN_HUMIDITY
}

/// Should the Lauscher leave the lair? Only once the surface is nearly as dark
/// as its lair conditions (i.e. night has fallen), within a dusk tolerance.
pub fn should_emerge(surface_light: u8, profile: &LairProfile) -> bool {
    let surface_darkness = 255u16 - surface_light as u16;
    surface_darkness + EMERGE_TOLERANCE as u16 >= profile.darkness as u16
}

/// What the brain decided this tick.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Decision {
    pub mode: Mode,
    /// Where to move toward (`None` = stay put, e.g. waiting in the lair with no
    /// home yet).
    pub target: Option<Vec3>,
}

/// The Lauscher's persistent state. `Copy` + integer-only so it snapshots and
/// rolls back with the rest of the sim.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Lauscher {
    pub home: Option<Vec3>,
    pub profile: LairProfile,
    pub mode: Mode,
    pub last_heard: Option<Vec3>,
    /// Ticks of trail left before the last fix goes cold.
    pub track: u16,
}

impl Lauscher {
    pub fn new() -> Lauscher {
        Lauscher { home: None, profile: LairProfile::default(), mode: Mode::InLair, last_heard: None, track: 0 }
    }

    /// While exploring, imprint the first dark+damp spot found as home and
    /// remember its conditions. Once homed, it doesn't re-home (it's bound to
    /// its lair). Returns true when a new lair was just claimed.
    pub fn consider_lair(&mut self, pos: Vec3, light_level: u8, humidity: u8) -> bool {
        if self.home.is_none() && is_lair_site(light_level, humidity) {
            self.home = Some(pos);
            self.profile = LairProfile { darkness: 255 - light_level, humidity };
            true
        } else {
            false
        }
    }

    /// Register the loudest sound heard this tick (its already-occluded perceived
    /// loudness, e.g. from `sound::heard_loudness_world`, and the source's
    /// position). Loud enough → a fresh fix, trail reset to full.
    pub fn hear(&mut self, source: Vec3, perceived: Loudness) {
        if perceived >= HEAR_THRESHOLD {
            self.last_heard = Some(source);
            self.track = TRACK_TICKS;
        }
    }

    /// Advance one decision tick *after* any `hear` calls. `surface_light` is the
    /// ambient light where it stands. Decays the trail; picks a mode + target.
    pub fn decide(&mut self, surface_light: u8) -> Decision {
        // The trail goes cold over time.
        if self.track > 0 {
            self.track -= 1;
            if self.track == 0 {
                self.last_heard = None;
            }
        }

        // Daylight (or not dark enough): go home and wait.
        if !should_emerge(surface_light, &self.profile) {
            self.mode = Mode::Returning;
            return Decision { mode: Mode::Returning, target: self.home };
        }

        // Night: hunt a live or remembered sound, else prowl near home.
        if let Some(p) = self.last_heard {
            self.mode = Mode::Hunting;
            Decision { mode: Mode::Hunting, target: Some(p) }
        } else {
            self.mode = Mode::Prowling;
            Decision { mode: Mode::Prowling, target: self.home }
        }
    }
}

impl Default for Lauscher {
    fn default() -> Lauscher {
        Lauscher::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sim::fixed::Fixed;

    fn v(x: f64, y: f64, z: f64) -> Vec3 {
        Vec3 { x: Fixed::from_f64(x), y: Fixed::from_f64(y), z: Fixed::from_f64(z) }
    }

    #[test]
    fn imprints_a_dark_damp_lair_and_only_once() {
        let mut l = Lauscher::new();
        // Bright/dry spot → not a lair.
        assert!(!l.consider_lair(v(0.0, 0.0, 0.0), 220, 40));
        assert!(l.home.is_none());
        // Dark + damp → claims it, remembers conditions.
        assert!(l.consider_lair(v(5.0, -10.0, 5.0), 20, 200));
        assert_eq!(l.home, Some(v(5.0, -10.0, 5.0)));
        assert_eq!(l.profile.darkness, 235);
        assert_eq!(l.profile.humidity, 200);
        // Won't re-home elsewhere.
        assert!(!l.consider_lair(v(99.0, -10.0, 99.0), 0, 255));
        assert_eq!(l.home, Some(v(5.0, -10.0, 5.0)));
    }

    #[test]
    fn emerges_at_night_returns_by_day() {
        let mut l = Lauscher::new();
        l.consider_lair(v(0.0, -10.0, 0.0), 20, 200); // profile darkness 235
        // Bright surface → won't emerge; heads home.
        let day = l.decide(230);
        assert_eq!(day.mode, Mode::Returning);
        assert_eq!(day.target, l.home);
        assert!(!should_emerge(230, &l.profile));
        // Dark surface → emerges (prowls, nothing heard yet).
        let night = l.decide(10);
        assert_eq!(night.mode, Mode::Prowling);
        assert!(should_emerge(10, &l.profile));
    }

    #[test]
    fn hunts_toward_a_heard_sound_then_loses_a_cold_trail() {
        let mut l = Lauscher::new();
        l.consider_lair(v(0.0, -10.0, 0.0), 20, 200);
        // A loud footstep at night → lock on and hunt toward it.
        l.hear(v(30.0, 4.0, 12.0), 180);
        let d = l.decide(10);
        assert_eq!(d.mode, Mode::Hunting);
        assert_eq!(d.target, Some(v(30.0, 4.0, 12.0)));
        // It keeps closing on the last fix from memory while the trail is warm…
        for _ in 0..TRACK_TICKS - 2 {
            let d = l.decide(10);
            assert_eq!(d.mode, Mode::Hunting, "still tracking from memory");
        }
        // …until the trail goes cold, then it falls back to prowling.
        let _ = l.decide(10);
        let cold = l.decide(10);
        assert_eq!(cold.mode, Mode::Prowling);
        assert_eq!(l.last_heard, None, "forgot the cold trail");
    }

    #[test]
    fn a_quiet_sound_is_ignored() {
        let mut l = Lauscher::new();
        l.consider_lair(v(0.0, -10.0, 0.0), 20, 200);
        l.hear(v(40.0, 4.0, 0.0), HEAR_THRESHOLD - 1); // too faint
        assert_eq!(l.last_heard, None);
        let d = l.decide(10);
        assert_eq!(d.mode, Mode::Prowling, "nothing worth chasing → prowl");
    }
}
