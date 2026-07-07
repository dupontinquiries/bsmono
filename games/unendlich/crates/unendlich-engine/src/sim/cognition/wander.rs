//! Ambient wander — the idle locomotion that makes scattered creatures drift
//! around their patch of world instead of standing frozen. It's intentionally
//! tiny and self-contained: a heading, a speed, and a retarget timer. No world
//! query, no pathing — the caller advances the returned horizontal delta and
//! snaps the creature back onto the surface each tick. Reactive behaviour (flee
//! the player, hunt prey) lives in [`super::behavior`] and layers on top.
//!
//! Deterministic: a `Wander` seeded the same way produces the same walk, so a
//! reloaded creature resumes a matching path and tests are exact.

use std::f32::consts::TAU;

/// SplitMix64 step — a cheap deterministic hash for rolling headings/timers.
fn mix(mut h: u64) -> u64 {
    h = h.wrapping_add(0x9E37_79B9_7F4A_7C15);
    h = (h ^ (h >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    h = (h ^ (h >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    h ^ (h >> 31)
}

/// Per-creature idle wander state.
#[derive(Clone, Copy, Debug)]
pub struct Wander {
    /// Current heading in radians, `0` = facing `+X`.
    pub heading: f32,
    /// Ground speed in blocks per second.
    pub speed: f32,
    seed: u64,
    timer: f32,
    step: u64,
}

impl Wander {
    /// A new wanderer. `seed` makes the walk deterministic (use the spawn cell);
    /// `speed` is blocks/sec (≈1–2 reads as an ambling animal).
    pub fn new(seed: u64, speed: f32) -> Self {
        let mut w = Wander { heading: 0.0, speed, seed, timer: 0.0, step: 0 };
        w.retarget();
        w
    }

    /// Pick a fresh heading and a 1.5–4.0 s interval before the next turn.
    fn retarget(&mut self) {
        let a = mix(self.seed ^ self.step.wrapping_mul(0x1000_0001));
        let b = mix(a ^ 0xD1B5_4A32_D192_ED03);
        self.heading = (a % 36000) as f32 / 36000.0 * TAU;
        self.timer = 1.5 + (b % 2500) as f32 / 1000.0;
        self.step = self.step.wrapping_add(1);
    }

    /// Advance `dt` seconds. Returns the horizontal world delta `(dx, dz)`; the
    /// caller adds it to the creature's position and re-grounds Y. Turns to a new
    /// heading when the interval elapses.
    pub fn step(&mut self, dt: f32) -> (f32, f32) {
        self.timer -= dt;
        if self.timer <= 0.0 {
            self.retarget();
        }
        let d = self.speed * dt;
        (self.heading.cos() * d, self.heading.sin() * d)
    }

    /// Wander, but bolt away from a threat (e.g. the player) within `flee_radius`.
    /// `self_xz` and `threat_xz` are world `(x, z)`. While fleeing, the heading
    /// snaps to point directly away from the threat and the creature moves at
    /// `flee_speed` (a startled sprint). Outside the radius — or with no threat —
    /// it just [`step`](Self::step)s its normal wander.
    pub fn step_aware(
        &mut self,
        dt: f32,
        self_xz: (f32, f32),
        threat_xz: Option<(f32, f32)>,
        flee_radius: f32,
        flee_speed: f32,
    ) -> (f32, f32) {
        if let Some((tx, tz)) = threat_xz {
            let dx = self_xz.0 - tx;
            let dz = self_xz.1 - tz;
            let d2 = dx * dx + dz * dz;
            if d2 > 1e-6 && d2 < flee_radius * flee_radius {
                self.heading = dz.atan2(dx); // face directly away
                let inv = flee_speed * dt / d2.sqrt();
                return (dx * inv, dz * inv);
            }
        }
        self.step(dt)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn steps_advance_along_the_heading() {
        let mut w = Wander::new(42, 2.0);
        let h = w.heading;
        let (dx, dz) = w.step(0.5); // 2.0 * 0.5 = 1.0 block this tick
        let dist = (dx * dx + dz * dz).sqrt();
        assert!((dist - 1.0).abs() < 1e-4, "moved one block (got {dist})");
        // direction matches the heading
        assert!((dx - h.cos()).abs() < 1e-4 && (dz - h.sin()).abs() < 1e-4);
    }

    #[test]
    fn a_still_creature_does_not_drift() {
        let mut w = Wander::new(7, 0.0);
        let (dx, dz) = w.step(1.0);
        assert_eq!((dx, dz), (0.0, 0.0));
    }

    #[test]
    fn it_turns_after_the_interval() {
        let mut w = Wander::new(1, 1.0);
        let first = w.heading;
        // Step well past the max 4.0 s interval; the heading must have changed.
        for _ in 0..50 {
            w.step(0.2);
        }
        assert!(w.heading != first, "the wanderer eventually turns");
    }

    #[test]
    fn the_same_seed_walks_the_same_path() {
        let mut a = Wander::new(99, 1.5);
        let mut b = Wander::new(99, 1.5);
        for _ in 0..100 {
            assert_eq!(a.step(0.1), b.step(0.1));
        }
    }

    #[test]
    fn it_bolts_away_from_a_close_threat() {
        let mut w = Wander::new(3, 1.0);
        let self_xz = (3.0, 0.0); // 3 east of the threat at the origin
        let (dx, dz) = w.step_aware(0.5, self_xz, Some((0.0, 0.0)), 10.0, 4.0);
        assert!(dx > 0.0 && dz.abs() < 1e-4, "flees due east, away from the threat");
        let mag = (dx * dx + dz * dz).sqrt();
        assert!((mag - 4.0 * 0.5).abs() < 1e-4, "moves at flee speed (got {mag})");
        let new_dist = ((self_xz.0 + dx).powi(2) + (self_xz.1 + dz).powi(2)).sqrt();
        assert!(new_dist > 3.0, "the gap to the threat grows");
    }

    #[test]
    fn a_distant_threat_does_not_spook_it() {
        // A threat well outside the flee radius -> identical to a plain wander.
        let mut a = Wander::new(5, 1.5);
        let mut b = Wander::new(5, 1.5);
        let far = Some((1000.0, 1000.0));
        assert_eq!(a.step_aware(0.1, (0.0, 0.0), far, 8.0, 4.0), b.step(0.1));
    }
}
