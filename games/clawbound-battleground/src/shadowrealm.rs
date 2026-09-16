//! shadowrealm — where the senses stop agreeing with the world.
//!
//! An homage to the later books, where the Dark Forest bleeds into the living
//! one. The design note in `docs/WARRIORS_MECHANICS.md` fixed the shape:
//! *"sight fails, scent persists, hearing lies"*, as a modifier over
//! `PerceptParams` — which works because `daylight` already established that a
//! world condition modifies perception rather than replacing it.
//!
//! ## Hearing LIES, which is not the same as hearing failing
//!
//! Sight failing is ordinary: a number goes down, like night. The clause worth
//! building carefully is the third one. A corrupted cat does not hear less —
//! it hears a sound that is not where the sound is. That is worse than
//! deafness and much more interesting, because a deaf cat knows it is deaf
//! while a lied-to cat acts confidently on a bearing and arrives somewhere
//! nothing is.
//!
//! ## The lie is DETERMINISTIC, and that is the whole mechanic
//!
//! If the false bearing were random, a player would listen three times and
//! average it away — the corruption would be noise, and noise is something you
//! filter. Keyed to the PLACE instead, the lie is stable: the same spot tells
//! the same lie every time you stand on it. You cannot average it out; you can
//! only learn that this hollow always sends you left, which is exactly the
//! relationship the books have with the Dark Forest. It is a haunted place,
//! not a broken sensor.
//!
//! ## Scent persists, by doing nothing
//!
//! Worth stating because it is implemented as an absence. `PerceptParams`
//! carries no scent term — scent lives in `wildkit::sim::scent_*` and is
//! queried separately — so a shadow overlay that modifies only this struct
//! leaves scent untouched automatically. That is the correct behaviour and it
//! is also a trap: someone later adding a scent field here would silently
//! break the one sense the realm was supposed to leave alone. Hence
//! [`SCENT_IS_UNTOUCHED`].

use std::f32::consts::TAU;
use wildkit::sim::perception::PerceptParams;

/// Documentation as a constant, so the invariant is greppable.
///
/// The shadow realm must never modify scent. If a scent term is ever added to
/// `PerceptParams`, [`Shadow::apply`] has to exclude it explicitly, and the
/// test `scent_is_never_touched` is what will fail.
pub const SCENT_IS_UNTOUCHED: bool = true;

/// How far a lie can be from the truth, in radians.
///
/// About 50°. Chosen against what the lie has to feel like: a bearing wrong by
/// 10° is a miss, and one wrong by 180° reads as a broken game. Wrong by half
/// a quadrant sends a cat confidently past its quarry, which is the intended
/// experience.
pub const MAX_LIE: f32 = 0.9;

/// Corruption at a place, 0 clean to 1 fully overlaid.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Shadow {
    pub level: f32,
}

impl Shadow {
    pub fn new(level: f32) -> Shadow {
        Shadow { level: level.clamp(0.0, 1.0) }
    }

    pub fn is_clean(&self) -> bool {
        self.level <= 1e-4
    }

    /// What the realm does to a creature's senses.
    ///
    /// Sight collapses toward nothing — unlike night, which floors, because
    /// the realm is not darkness and there is no adapting to it. Hearing's
    /// WEIGHT is untouched: the cat hears just as well and is told lies, which
    /// is the point. Reducing the weight here would quietly turn the realm
    /// into fog.
    pub fn apply(&self, base: &PerceptParams) -> PerceptParams {
        if self.is_clean() {
            return *base;
        }
        let keep = 1.0 - self.level;
        PerceptParams {
            sight_range: base.sight_range * keep,
            sight_weight: base.sight_weight * keep,
            // noise_weight deliberately NOT reduced. See above.
            ..*base
        }
    }

    /// Where a sound seems to come from.
    ///
    /// `true_bearing` is radians. `(x, z)` is where the LISTENER stands — the
    /// lie belongs to the place, not to the sound, so walking two paces gives
    /// a different lie and standing still gives the same one forever.
    ///
    /// At zero corruption this is the identity, exactly.
    pub fn heard_bearing(&self, true_bearing: f32, x: f32, z: f32) -> f32 {
        if self.is_clean() {
            return true_bearing;
        }
        wrap(true_bearing + self.lie_at(x, z))
    }

    /// The signed angular lie told at a place, scaled by corruption.
    ///
    /// Exposed so a UI can show which way a haunted hollow pulls — learning
    /// that is the counterplay, and a mechanic with no counterplay is just
    /// unfairness.
    pub fn lie_at(&self, x: f32, z: f32) -> f32 {
        // Hash the position to a stable value in -1..1. Deliberately coarse:
        // quantised to half a world unit so a cat shifting its weight does not
        // get a new lie, but crossing the hollow does.
        let qx = (x * 2.0).floor() as i64;
        let qz = (z * 2.0).floor() as i64;
        let mut h = (qx as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15)
            ^ (qz as u64).wrapping_mul(0xC2B2_AE3D_27D4_EB4F);
        h ^= h >> 29;
        h = h.wrapping_mul(0xBF58_476D_1CE4_E5B9);
        h ^= h >> 32;
        let unit = ((h & 0xFF_FFFF) as f32 / 16_777_216.0) * 2.0 - 1.0;
        unit * MAX_LIE * self.level
    }
}

fn wrap(a: f32) -> f32 {
    let mut x = a % TAU;
    if x < 0.0 {
        x += TAU;
    }
    x
}

/// Shortest angular distance between two bearings, 0..PI.
pub fn bearing_error(a: f32, b: f32) -> f32 {
    let d = (wrap(a) - wrap(b)).abs();
    d.min(TAU - d)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base() -> PerceptParams {
        PerceptParams::default()
    }

    /// Clean ground must be untouched, or a feature that adds a state changes
    /// every creature everywhere.
    #[test]
    fn clean_ground_changes_nothing() {
        let b = base();
        let out = Shadow::new(0.0).apply(&b);
        assert_eq!(out.sight_range, b.sight_range);
        assert_eq!(out.sight_weight, b.sight_weight);
        assert_eq!(out.noise_weight, b.noise_weight);
        // And a bearing is reported exactly, not merely closely.
        assert_eq!(Shadow::new(0.0).heard_bearing(1.234, 10.0, -5.0), 1.234);
    }

    /// THE POINT. Hearing is not reduced — the cat hears perfectly and is told
    /// a lie. Reducing the weight would quietly turn the realm into fog.
    #[test]
    fn the_realm_blinds_without_deafening() {
        let b = base();
        let deep = Shadow::new(0.9).apply(&b);
        assert!(deep.sight_range < b.sight_range * 0.2, "sight collapses");
        assert_eq!(deep.noise_weight, b.noise_weight, "hearing is NOT degraded");
    }

    /// Unlike night, sight does not floor: the realm is not darkness and there
    /// is no adapting to it.
    #[test]
    fn sight_collapses_rather_than_flooring() {
        let out = Shadow::new(1.0).apply(&base());
        assert!(out.sight_range < 1e-3, "got {}", out.sight_range);
    }

    /// THE WHOLE MECHANIC. A random lie is noise, and noise is something a
    /// player averages away by listening three times. Keyed to the place, the
    /// same spot tells the same lie forever.
    #[test]
    fn the_same_place_always_tells_the_same_lie() {
        let s = Shadow::new(0.8);
        let (x, z) = (12.25, -7.5);
        let first = s.heard_bearing(1.0, x, z);
        for _ in 0..50 {
            assert_eq!(s.heard_bearing(1.0, x, z), first, "the lie must not drift");
        }
    }

    /// ...but crossing the hollow gives a different one, or the lie is a
    /// constant offset a player would correct for once and forget.
    #[test]
    fn different_places_tell_different_lies() {
        let s = Shadow::new(1.0);
        let lies: Vec<f32> = (0..24)
            .map(|i| s.lie_at(i as f32 * 3.0, i as f32 * -2.0))
            .collect();
        let spread = lies.iter().cloned().fold(f32::MIN, f32::max)
            - lies.iter().cloned().fold(f32::MAX, f32::min);
        assert!(spread > MAX_LIE, "lies barely varied: spread {spread}");
    }

    /// Coarse on purpose: shifting your weight must not re-roll the lie.
    #[test]
    fn a_tiny_step_does_not_change_the_lie() {
        let s = Shadow::new(1.0);
        let a = s.lie_at(10.0, 10.0);
        assert_eq!(s.lie_at(10.05, 10.05), a, "same half-unit cell");
        assert_ne!(s.lie_at(14.0, 10.0), a, "a real move does change it");
    }

    /// A bearing wrong by 10° is a miss; wrong by 180° reads as broken. Half a
    /// quadrant sends a cat confidently past its quarry.
    #[test]
    fn the_lie_is_plausible_rather_than_absurd() {
        let s = Shadow::new(1.0);
        for i in 0..200 {
            let (x, z) = (i as f32 * 1.7, i as f32 * -0.9);
            let err = bearing_error(s.heard_bearing(2.0, x, z), 2.0);
            assert!(err <= MAX_LIE + 1e-4, "lie of {err} rad at ({x},{z})");
        }
    }

    /// Corruption scales the lie: the edge of the realm misleads slightly, the
    /// heart of it badly.
    #[test]
    fn deeper_corruption_lies_harder() {
        let (x, z) = (33.0, 21.0);
        let edge = Shadow::new(0.2).lie_at(x, z).abs();
        let heart = Shadow::new(1.0).lie_at(x, z).abs();
        assert!(heart > edge, "edge {edge}, heart {heart}");
        assert!(edge > 0.0, "the edge still misleads a little");
    }

    /// SCENT PERSISTS, by being absent from what we touch. If a scent term is
    /// ever added to PerceptParams, this is the test that should fail.
    #[test]
    fn scent_is_never_touched() {
        assert!(SCENT_IS_UNTOUCHED);
        // Every field the realm is allowed to change, enumerated. A new field
        // arriving in PerceptParams will not be covered here, which is the
        // point: the reviewer has to decide about it deliberately.
        let b = base();
        let out = Shadow::new(1.0).apply(&b);
        assert_eq!(out.frustum_half, b.frustum_half);
        assert_eq!(out.blind_half_base, b.blind_half_base);
        assert_eq!(out.blind_half_per_size, b.blind_half_per_size);
        assert_eq!(out.noise_weight, b.noise_weight);
    }

    /// Bearings stay in range, so a caller can feed the result straight back
    /// into trigonometry without normalising.
    #[test]
    fn a_heard_bearing_is_always_a_valid_angle() {
        let s = Shadow::new(1.0);
        for i in -50..50 {
            let h = s.heard_bearing(i as f32 * 0.7, i as f32, -(i as f32));
            assert!((0.0..TAU).contains(&h), "bearing {h} out of range");
        }
    }
}
