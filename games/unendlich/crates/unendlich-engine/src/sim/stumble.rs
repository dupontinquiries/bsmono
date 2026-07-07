//! Stumble — moving fast over broken ground should trip a body, not glide over
//! it. The model turns *speed × ground-roughness* into a trip hazard (lighter
//! for sure-footed animals), fires deterministically, and while recovering it
//! slows the creature and lurches it toward the leg that caught. This is the
//! "punish speed on rough terrain" mechanic: the price of bolting across a
//! talus slope is going down.
//!
//! Pure + deterministic. Roughness comes from [`super::posture::terrain_roughness`]
//! (the spread of the foot samples); the bin owns the per-creature [`Stumble`]
//! state and applies the speed multiplier + the orientation kick.

/// Divisor turning `speed(blk/s) × roughness(blocks)` into a unitless hazard. A
/// 2 blk/s trot across a 2-block-rough ledge → raw ≈ 0.5 hazard before
/// sure-footedness. Tuned so a calm walk on gentle ground basically never trips.
pub const STUMBLE_SCALE: f32 = 8.0;
/// Ticks a stumble takes to recover from (at the sim tick rate).
pub const STUMBLE_RECOVER_TICKS: u16 = 6;
/// Movement speed multiplier while recovering from a stumble.
pub const STUMBLE_SPEED_MULT: f32 = 0.35;
/// Max heading veer (radians) a stumble imparts toward the catching side.
pub const MAX_YAW_KICK: f32 = 0.5;
/// Max nose-dip (radians) a stumble imparts (tripping forward).
pub const MAX_PITCH_KICK: f32 = 0.4;

/// Trip likelihood in `[0,1]` from forward `speed` (blocks/s), ground
/// `roughness` (block spread under the feet), and `surefoot` in `[0,1]` (1 =
/// never trips). A standing animal or flat ground is never at risk.
pub fn stumble_risk(speed: f32, roughness: f32, surefoot: f32) -> f32 {
    if speed <= 0.0 || roughness <= 0.0 {
        return 0.0;
    }
    let hazard = (speed * roughness) / STUMBLE_SCALE;
    (hazard * (1.0 - surefoot.clamp(0.0, 1.0))).clamp(0.0, 1.0)
}

/// Does a trip fire this step? Deterministic: a hashed `[0,1)` draw from `seed`
/// is compared against `risk`, so the same creature in the same spot trips the
/// same way every run (replayable). `risk <= 0` never fires; `>= 1` always does.
pub fn trips(risk: f32, seed: u64) -> bool {
    if risk <= 0.0 {
        return false;
    }
    if risk >= 1.0 {
        return true;
    }
    // SplitMix64-ish scramble → a uniform draw in [0,1).
    let mut z = seed.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^= z >> 31;
    let draw = (z >> 11) as f32 / (1u64 << 53) as f32;
    draw < risk
}

/// The lurch a stumble imparts: a yaw veer toward the catching side and a
/// nose-dip. `catch_left` = the left leg snagged, so the body slews left. Scaled
/// by `severity` in `[0,1]` (use the risk that fired it). Returns `(yaw_kick,
/// pitch_kick)` in radians.
pub fn stumble_kick(severity: f32, catch_left: bool) -> (f32, f32) {
    let s = severity.clamp(0.0, 1.0);
    let yaw = s * MAX_YAW_KICK * if catch_left { 1.0 } else { -1.0 };
    let pitch = -s * MAX_PITCH_KICK;
    (yaw, pitch)
}

/// Per-creature stumble state: a recovery countdown. While recovering the
/// creature is slowed (and the bin can hold the orientation kick).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Stumble {
    /// Ticks of recovery remaining. 0 = sure-footed.
    pub recover: u16,
}

impl Stumble {
    pub fn new() -> Stumble {
        Stumble { recover: 0 }
    }

    pub fn is_stumbling(&self) -> bool {
        self.recover > 0
    }

    /// Start (or refresh) a stumble lasting `ticks`. Never shortens an ongoing
    /// recovery.
    pub fn trigger(&mut self, ticks: u16) {
        self.recover = self.recover.max(ticks);
    }

    /// Advance one tick. Returns the movement speed multiplier this tick:
    /// slowed while recovering, full (1.0) otherwise.
    pub fn tick(&mut self) -> f32 {
        if self.recover > 0 {
            self.recover -= 1;
            STUMBLE_SPEED_MULT
        } else {
            1.0
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flat_or_still_never_trips() {
        assert_eq!(stumble_risk(0.0, 5.0, 0.0), 0.0, "standing still: no risk");
        assert_eq!(stumble_risk(3.0, 0.0, 0.0), 0.0, "flat ground: no risk");
    }

    #[test]
    fn faster_on_rougher_ground_is_riskier() {
        let slow = stumble_risk(1.0, 1.0, 0.0);
        let fast = stumble_risk(3.0, 1.0, 0.0);
        let rough = stumble_risk(1.0, 3.0, 0.0);
        assert!(fast > slow, "speed raises risk");
        assert!(rough > slow, "roughness raises risk");
    }

    #[test]
    fn surefooted_animals_dont_trip() {
        assert_eq!(stumble_risk(5.0, 5.0, 1.0), 0.0, "perfectly sure-footed never trips");
        let half = stumble_risk(2.0, 2.0, 0.5);
        let none = stumble_risk(2.0, 2.0, 0.0);
        assert!(half < none, "sure-footedness reduces risk");
    }

    #[test]
    fn trips_brackets_are_deterministic() {
        assert!(!trips(0.0, 123), "zero risk never fires");
        assert!(trips(1.0, 123), "certain risk always fires");
        // Same seed + risk → same outcome twice.
        assert_eq!(trips(0.5, 999), trips(0.5, 999));
    }

    #[test]
    fn trips_frequency_tracks_risk() {
        // Over many seeds, a higher risk fires more often.
        let count = |risk: f32| (0..2000).filter(|&s| trips(risk, s as u64)).count();
        let lo = count(0.2);
        let hi = count(0.8);
        assert!(hi > lo, "0.8 risk fires more than 0.2 ({hi} vs {lo})");
        // And roughly in proportion (loose bounds — it's a hash, not an RNG).
        assert!((lo as f32 / 2000.0 - 0.2).abs() < 0.08);
        assert!((hi as f32 / 2000.0 - 0.8).abs() < 0.08);
    }

    #[test]
    fn kick_veers_toward_the_catching_leg() {
        let (yl, pl) = stumble_kick(1.0, true);
        let (yr, _) = stumble_kick(1.0, false);
        assert!(yl > 0.0 && yr < 0.0, "left catch veers left, right catch veers right");
        assert!(pl < 0.0, "the nose dips on a trip");
    }

    #[test]
    fn recovery_counts_down_and_slows() {
        let mut s = Stumble::new();
        assert_eq!(s.tick(), 1.0, "no stumble → full speed");
        s.trigger(2);
        assert!(s.is_stumbling());
        assert_eq!(s.tick(), STUMBLE_SPEED_MULT);
        assert_eq!(s.tick(), STUMBLE_SPEED_MULT);
        assert_eq!(s.tick(), 1.0, "recovered → full speed again");
        assert!(!s.is_stumbling());
    }
}
