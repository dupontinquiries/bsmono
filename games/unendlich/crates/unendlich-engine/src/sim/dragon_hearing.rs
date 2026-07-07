//! `sim::dragon_hearing` — turn the sounds a dragon can hear into a reaction:
//! how startling it is, which way to look, and whether it's loud enough to wake
//! a sleeping dragon. Composes `sound_masking` (perceived loudness vs the ambient
//! masking floor). Feeds `dragon_observe` (orient toward the noise) and
//! `dragon_activity` (wake / go alert). Pure, deterministic, panic-free.

use crate::sim::sound_masking::{perceived_loudness, Ears, Sound};

/// A sound reaching the dragon, with its direction relative to the body heading.
#[derive(Clone, Copy, Debug)]
pub struct Noise {
    pub sound: Sound,
    /// Bearing relative to body heading (rad).
    pub bearing: f32,
    pub dist2: f32,
    pub occlusion: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Reaction {
    pub heard: bool,
    /// Perceived loudness (dB-ish) of the loudest audible noise.
    pub loudness: f32,
    /// Bearing to that loudest noise (for the gaze to orient toward).
    pub bearing: f32,
    /// 0..1 how startling (loud over the ambient floor) — raises alertness.
    pub startle01: f32,
    /// Loud enough to wake a sleeping dragon.
    pub should_wake: bool,
}

impl Reaction {
    pub fn silent() -> Self {
        Reaction { heard: false, loudness: 0.0, bearing: 0.0, startle01: 0.0, should_wake: false }
    }
}

/// Sleeping dampens hearing: the masking floor rises by this much (dB).
const SLEEP_DEAFEN: f32 = 12.0;
const STARTLE_SCALE: f32 = 25.0;
const WAKE_STARTLE: f32 = 0.35;
const SENS_DB_PER_UNIT: f32 = 6.0;

/// React to the noises currently reaching the dragon. Only sounds whose perceived
/// loudness clears the ambient floor register; the loudest one sets the bearing
/// and startle.
pub fn react(ears: Ears, noises: &[Noise], ambient_floor: f32, asleep: bool) -> Reaction {
    let boost = SENS_DB_PER_UNIT * (ears.sensitivity - 1.0);
    let floor = ambient_floor + if asleep { SLEEP_DEAFEN } else { 0.0 };
    let mut best = f32::MIN;
    let mut bearing = 0.0;
    let mut heard = false;
    for n in noises {
        let p = perceived_loudness(n.sound, n.dist2, n.occlusion) + boost;
        if p > floor && p > best {
            best = p;
            bearing = n.bearing;
            heard = true;
        }
    }
    if !heard {
        return Reaction::silent();
    }
    let startle01 = ((best - floor) / STARTLE_SCALE).clamp(0.0, 1.0);
    Reaction {
        heard: true,
        loudness: best,
        bearing,
        startle01,
        should_wake: startle01 >= WAKE_STARTLE,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn ears() -> Ears { Ears { base_range: 50.0, sensitivity: 1.0 } }
    fn snd(l: f32) -> Sound { Sound { loudness: l, freq01: 0.5 } }

    #[test]
    fn loud_near_noise_is_heard_with_bearing() {
        let n = [Noise { sound: snd(90.0), bearing: 0.7, dist2: 4.0, occlusion: 0.0 }];
        let r = react(ears(), &n, 40.0, false);
        assert!(r.heard);
        assert!((r.bearing - 0.7).abs() < 1e-6, "orients toward the source");
    }

    #[test]
    fn quiet_noise_under_ambient_is_ignored() {
        let n = [Noise { sound: snd(45.0), bearing: 0.2, dist2: 900.0, occlusion: 0.5 }];
        let r = react(ears(), &n, 60.0, false);
        assert!(!r.heard);
        assert_eq!(r, Reaction::silent());
    }

    #[test]
    fn loudest_wins_and_startle_scales() {
        let n = [
            Noise { sound: snd(70.0), bearing: -0.3, dist2: 100.0, occlusion: 0.0 },
            Noise { sound: snd(100.0), bearing: 1.1, dist2: 25.0, occlusion: 0.0 }, // loudest
        ];
        let soft = react(ears(), &n[..1], 40.0, false);
        let loud = react(ears(), &n, 40.0, false);
        assert!((loud.bearing - 1.1).abs() < 1e-6, "loudest noise sets the bearing");
        assert!(loud.startle01 > soft.startle01, "louder ⇒ more startling");
    }

    #[test]
    fn only_loud_sounds_wake_a_sleeper() {
        let quietish = [Noise { sound: snd(70.0), bearing: 0.0, dist2: 100.0, occlusion: 0.0 }];
        let bang = [Noise { sound: snd(110.0), bearing: 0.0, dist2: 9.0, occlusion: 0.0 }];
        assert!(!react(ears(), &quietish, 40.0, true).should_wake, "a soft sound doesn't wake it");
        assert!(react(ears(), &bang, 40.0, true).should_wake, "a bang wakes it");
    }

    #[test]
    fn sleeping_dampens_hearing() {
        let n = [Noise { sound: snd(66.0), bearing: 0.0, dist2: 100.0, occlusion: 0.0 }];
        // a borderline sound heard while awake but missed while asleep
        assert!(react(ears(), &n, 40.0, false).heard);
        assert!(!react(ears(), &n, 40.0, true).heard, "asleep raises the floor");
    }

    #[test]
    fn empty_is_silent_and_deterministic() {
        assert_eq!(react(ears(), &[], 40.0, false), Reaction::silent());
        let n = [Noise { sound: snd(85.0), bearing: 0.4, dist2: 16.0, occlusion: 0.1 }];
        assert_eq!(react(ears(), &n, 40.0, false), react(ears(), &n, 40.0, false));
    }
}
