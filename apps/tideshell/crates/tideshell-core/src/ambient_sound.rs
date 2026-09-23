//! Ambient loudness from the microphone, for notification sounds that fit
//! the room: a whisper in a library, audible on a street. Pure: the caller
//! feeds RMS per window (from a capture thread) and applies the gain.
//!
//! The room's loudness is the noise FLOOR, not the level: a sentence spoken
//! next to the phone or a dropped cup must not make the next notification
//! loud. The floor follows the level down quickly (a quiet gap is real
//! quiet) and up slowly (it takes a while of sustained noise to count as a
//! noisy place). The gain is then a log map from the floor, smoothed.

use crate::Millis;

/// Floor tracking time constants (seconds): down fast, up slow.
pub const FLOOR_DOWN_S: f32 = 1.5;
pub const FLOOR_UP_S: f32 = 12.0;
/// dBFS of the floor that maps to the minimum and maximum gain.
pub const QUIET_DBFS: f32 = -60.0;
pub const LOUD_DBFS: f32 = -25.0;
pub const GAIN_MIN: f32 = 0.30;
pub const GAIN_MAX: f32 = 1.0;
/// Output smoothing (seconds).
pub const GAIN_TAU_S: f32 = 3.0;
/// Below this the microphone is considered muted or absent, not "silent".
pub const SILENCE_DBFS: f32 = -90.0;

/// RMS of f32 samples (-1..1) as dBFS; silence floors at -100.
pub fn rms_dbfs(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return -100.0;
    }
    let ms = samples.iter().map(|s| s * s).sum::<f32>() / samples.len() as f32;
    (20.0 * ms.sqrt().max(1e-5).log10()).max(-100.0)
}

/// Gain for a floor: 0.3 in a quiet room, 1.0 on a loud street, linear in dB between.
pub fn gain_for_floor(floor_dbfs: f32) -> f32 {
    let x = ((floor_dbfs - QUIET_DBFS) / (LOUD_DBFS - QUIET_DBFS)).clamp(0.0, 1.0);
    GAIN_MIN + (GAIN_MAX - GAIN_MIN) * x
}

#[derive(Debug, Clone, PartialEq)]
pub struct AmbientSound {
    floor_dbfs: Option<f32>,
    gain: f32,
    last: Option<Millis>,
    /// Windows seen since the last gain change worth reporting.
    reported: f32,
}

impl Default for AmbientSound {
    fn default() -> Self {
        AmbientSound { floor_dbfs: None, gain: 0.6, last: None, reported: 0.6 }
    }
}

impl AmbientSound {
    pub fn floor_dbfs(&self) -> Option<f32> {
        self.floor_dbfs
    }

    pub fn gain(&self) -> f32 {
        self.gain
    }

    /// One capture window's level. Returns a new gain when it moved by at
    /// least 0.05 since the last report.
    pub fn on_level(&mut self, level_dbfs: f32, now: Millis) -> Option<f32> {
        let dt = self.last.map(|t| now.saturating_sub(t) as f32 / 1000.0).unwrap_or(0.1).clamp(0.0, 5.0);
        self.last = Some(now);
        if level_dbfs <= SILENCE_DBFS {
            return None; // muted / no mic: keep the last estimate
        }
        let floor = match self.floor_dbfs {
            None => level_dbfs,
            Some(f) => {
                let tau = if level_dbfs < f { FLOOR_DOWN_S } else { FLOOR_UP_S };
                f + (level_dbfs - f) * (1.0 - (-dt / tau).exp())
            }
        };
        self.floor_dbfs = Some(floor);
        let target = gain_for_floor(floor);
        self.gain += (target - self.gain) * (1.0 - (-dt / GAIN_TAU_S).exp());
        if (self.gain - self.reported).abs() >= 0.05 {
            self.reported = self.gain;
            Some(self.gain)
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn feed(a: &mut AmbientSound, dbfs: f32, secs: f32, t0: Millis) -> (Vec<f32>, Millis) {
        let mut out = vec![];
        let n = (secs * 10.0) as u64;
        let mut t = t0;
        for i in 1..=n {
            t = t0 + i * 100;
            out.extend(a.on_level(dbfs, t));
        }
        (out, t)
    }

    #[test]
    fn rms_and_mapping() {
        assert!((rms_dbfs(&[0.5, -0.5, 0.5, -0.5]) - (-6.02)).abs() < 0.05);
        assert_eq!(rms_dbfs(&[]), -100.0);
        assert_eq!(rms_dbfs(&[0.0; 16]), -100.0);
        assert_eq!(gain_for_floor(-70.0), GAIN_MIN);
        assert_eq!(gain_for_floor(-10.0), GAIN_MAX);
        assert!((gain_for_floor((QUIET_DBFS + LOUD_DBFS) / 2.0) - 0.65).abs() < 1e-5);
    }

    #[test]
    fn a_library_is_quiet_a_street_is_loud() {
        let mut a = AmbientSound::default();
        let (_, t) = feed(&mut a, -62.0, 40.0, 0);
        assert!((a.gain() - GAIN_MIN).abs() < 0.03, "library: {}", a.gain());
        feed(&mut a, -28.0, 90.0, t);
        assert!(a.gain() > 0.9, "street after a while: {}", a.gain());
    }

    #[test]
    fn a_sentence_does_not_make_the_room_loud_but_sustained_noise_does() {
        let mut a = AmbientSound::default();
        let (_, t) = feed(&mut a, -60.0, 40.0, 0);
        let quiet = a.gain();
        let (_, t) = feed(&mut a, -25.0, 3.0, t); // someone speaks for 3 s
        assert!(a.gain() < quiet + 0.1, "3 s of speech barely moves it: {} vs {quiet}", a.gain());
        let (_, t) = feed(&mut a, -60.0, 10.0, t);
        assert!(a.gain() < quiet + 0.05, "back to quiet quickly (floor drops fast)");
        feed(&mut a, -25.0, 60.0, t);
        assert!(a.gain() > 0.8, "a minute of noise is a noisy place");
    }

    #[test]
    fn silence_from_a_muted_mic_keeps_the_estimate_and_reports_are_stepwise() {
        let mut a = AmbientSound::default();
        let (reports, t) = feed(&mut a, -30.0, 60.0, 0);
        assert!(!reports.is_empty() && reports.windows(2).all(|w| (w[1] - w[0]).abs() >= 0.049));
        let g = a.gain();
        let (r, _) = feed(&mut a, -100.0, 30.0, t);
        assert!(r.is_empty() && a.gain() == g, "muted: nothing changes");
    }
}
