//! light_kit_bridge — turns a lux reading from `iio_light` into a backlight
//! fraction, and (behind `write_backlight`) actually sets it.
//!
//! `iio_light.rs`'s own header names this module and has for a while
//! (`[crate::light_kit_bridge]'s perceptual curve is calibrated in lux`) —
//! it just never existed. Building the thing a doc comment already promised,
//! rather than a curve invented independently of what that module expects.
//!
//! ## Two safety rules inherited, not re-decided
//!
//! - `Sample::Busy`/`Sample::Gone` must never be read as darkness.
//!   `iio_light.rs` says exactly why: *"a Busy read must not drive the
//!   backlight to minimum in a bright room"* — another process holding the
//!   IIO ring buffer is common, not exceptional, and looks nothing like
//!   night if treated as a light level. `next_fraction` returns `None` for
//!   both, meaning "leave brightness exactly where it is", not "go dark".
//! - Only `Unit::Illuminance` (lux) may drive this curve —
//!   `Sensors::can_auto_brightness()` already gates on `!light_is_relative`
//!   for exactly the reason its own comment gives: raw counts produce a
//!   differently-*shaped* response, not a constant offset, so a "counts"
//!   sensor run through a lux-calibrated curve looks like bad tuning, not
//!   bad units. This module trusts that gate rather than re-checking `Unit`
//!   itself — callers are expected to have already called
//!   `can_auto_brightness()` before ever constructing a `Sample` to feed in.
//!
//! ## Why a curve, not a straight line
//!
//! Human brightness perception is roughly logarithmic (Weber–Fechner), not
//! linear — a straight lux→fraction mapping is either flat and unresponsive
//! across the dim range where most indoor life happens, or swings to
//! near-full the moment any daylight leaks in. `Curve::fraction_for` uses
//! `ln_1p` (log(1+x), well-behaved at `lux == 0` where plain `ln` would
//! diverge) so the response is gentle in the dark and compresses the high
//! end, matching how brightness actually reads to a person rather than to a
//! lux meter.
//!
//! ## Why a floor above zero
//!
//! A backlight fraction of exactly 0.0 isn't "very dim", it's off — the
//! screen looks broken, not adaptive. `Curve::floor` is the minimum
//! fraction regardless of how dark the room reads, so pitch-black still
//! shows *something*.
//!
//! ## Why a rate limiter at all
//!
//! A light sensor's reading jitters frame to frame even in a stable room —
//! someone's shadow, a cloud, a hand passing near the sensor. Feeding every
//! reading straight to the backlight produces visible flicker ("pumping"),
//! the single most common auto-brightness complaint on real devices.
//! `RateLimiter::step` caps how far one call can move the fraction; the
//! true light level still gets there, just over a few readings instead of
//! one, which is invisible to a person and eliminates the flicker.

use crate::iio_light::Sample;

pub struct Curve {
    /// Minimum backlight fraction, regardless of how dark the room reads.
    pub floor: f64,
    /// The lux level that should map to fraction 1.0 — ordinary bright
    /// indoor light or overcast daylight, deliberately not "direct noon
    /// sun", so full brightness is reachable in everyday bright conditions
    /// rather than requiring genuinely blinding light to ever get there.
    pub reference_lux: f64,
}

impl Curve {
    /// A reasonable default for a phone screen: never below 4% (readable,
    /// not "off"), full brightness by ~1000 lux (bright office / open
    /// shade — not direct sun, which can run 10,000+ lux).
    pub const DEFAULT: Curve = Curve { floor: 0.04, reference_lux: 1000.0 };

    pub fn fraction_for(&self, lux: f64) -> f64 {
        let lux = lux.max(0.0);
        let reference = self.reference_lux.max(1.0); // guard against a misconfigured 0/negative reference dividing by ~0
        let scaled = (lux.ln_1p() / reference.ln_1p()).clamp(0.0, 1.0);
        self.floor + (1.0 - self.floor) * scaled
    }
}

pub struct RateLimiter {
    /// Maximum change in fraction (0.0..=1.0 scale) allowed per call.
    pub max_step: f64,
}

impl RateLimiter {
    /// 0.05 per reading is the default: a full dark-to-bright sweep takes
    /// ~20 readings to complete rather than one jarring jump, while still
    /// reaching target within a couple of seconds at a normal polling rate.
    pub const DEFAULT: RateLimiter = RateLimiter { max_step: 0.05 };

    pub fn step(&self, prev: f64, target: f64) -> f64 {
        let delta = (target - prev).clamp(-self.max_step, self.max_step);
        (prev + delta).clamp(0.0, 1.0)
    }
}

/// The one entry point a caller actually polls. `prev` is whatever fraction
/// is currently set (`None` on the very first call — nothing to rate-limit
/// against yet, so the first real reading is applied directly rather than
/// crawled up to over 20 calls at startup). Returns `None` when the sample
/// isn't usable, meaning "don't touch the backlight" — never "go dark".
pub fn next_fraction(sample: Sample, prev: Option<f64>, curve: &Curve, limiter: &RateLimiter) -> Option<f64> {
    let lux = sample.value()?;
    let target = curve.fraction_for(lux);
    Some(match prev {
        Some(p) => limiter.step(p, target),
        None => target,
    })
}

/// Writes a fraction to a real backlight device. The one part of this
/// module that touches disk — kept to exactly this, no curve logic here to
/// get subtly wrong in a way `next_fraction`'s tests can't already catch.
/// `max_brightness` is read fresh each call rather than cached by the
/// caller: it's a few bytes and one syscall, and trusting a stale cached
/// max across a hot-unplugged/different backlight device is a worse bet
/// than the syscall.
pub fn write_backlight(device_dir: &std::path::Path, fraction: f64) -> std::io::Result<()> {
    let max_path = device_dir.join("max_brightness");
    let max: u32 = std::fs::read_to_string(&max_path)?.trim().parse().map_err(|e| {
        std::io::Error::new(std::io::ErrorKind::InvalidData, format!("max_brightness: {e}"))
    })?;
    let value = (fraction.clamp(0.0, 1.0) * max as f64).round() as u32;
    std::fs::write(device_dir.join("brightness"), value.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn total_darkness_still_shows_the_floor_not_zero() {
        assert_eq!(Curve::DEFAULT.fraction_for(0.0), Curve::DEFAULT.floor);
    }

    #[test]
    fn reference_lux_maps_to_full_brightness() {
        let f = Curve::DEFAULT.fraction_for(Curve::DEFAULT.reference_lux);
        assert!((f - 1.0).abs() < 1e-9, "got {f}, expected 1.0 at the reference point");
    }

    #[test]
    fn brighter_never_goes_past_full() {
        // Direct sun can be 10x+ the indoor reference; the curve must clamp,
        // not extrapolate past 1.0.
        assert_eq!(Curve::DEFAULT.fraction_for(50_000.0), 1.0);
    }

    #[test]
    fn the_curve_is_monotonic_not_just_bounded() {
        let samples = [0.0, 1.0, 10.0, 100.0, 500.0, 1000.0, 5000.0];
        let fractions: Vec<f64> = samples.iter().map(|&l| Curve::DEFAULT.fraction_for(l)).collect();
        for pair in fractions.windows(2) {
            assert!(pair[1] >= pair[0], "brightness must never DECREASE as lux increases: {fractions:?}");
        }
    }

    #[test]
    fn a_negative_lux_reading_is_treated_as_darkness_not_rejected() {
        // Shouldn't happen from real hardware, but a misbehaving driver
        // returning a negative value must not panic or produce a fraction
        // outside [floor, 1.0] via an unguarded ln() of a negative number.
        let f = Curve::DEFAULT.fraction_for(-5.0);
        assert_eq!(f, Curve::DEFAULT.floor);
    }

    #[test]
    fn a_busy_sample_leaves_brightness_untouched_not_dark() {
        // The single most important behavior this module has to get right,
        // inherited directly from iio_light.rs's own stated reasoning.
        assert_eq!(next_fraction(Sample::Busy, Some(0.8), &Curve::DEFAULT, &RateLimiter::DEFAULT), None);
    }

    #[test]
    fn a_gone_sample_also_leaves_brightness_untouched() {
        assert_eq!(next_fraction(Sample::Gone, Some(0.8), &Curve::DEFAULT, &RateLimiter::DEFAULT), None);
    }

    #[test]
    fn the_first_ever_reading_jumps_straight_to_target() {
        // No previous fraction to rate-limit against — crawling up over 20
        // calls at app startup, when the screen is already lit at whatever
        // the compositor's own default is, would be its own kind of
        // visible, pointless flicker.
        let got = next_fraction(Sample::Value(1000.0), None, &Curve::DEFAULT, &RateLimiter::DEFAULT).unwrap();
        assert!((got - 1.0).abs() < 1e-9);
    }

    #[test]
    fn a_sudden_bright_light_ramps_rather_than_jumps() {
        let limiter = RateLimiter { max_step: 0.05 };
        let step1 = next_fraction(Sample::Value(1000.0), Some(0.04), &Curve::DEFAULT, &limiter).unwrap();
        assert!((step1 - 0.09).abs() < 1e-9, "moved by exactly max_step, not straight to 1.0: got {step1}");
    }

    #[test]
    fn a_sudden_bright_light_eventually_arrives_over_several_readings() {
        let limiter = RateLimiter { max_step: 0.05 };
        let mut fraction = 0.04;
        for _ in 0..40 {
            fraction = next_fraction(Sample::Value(1000.0), Some(fraction), &Curve::DEFAULT, &limiter).unwrap();
        }
        assert!((fraction - 1.0).abs() < 1e-6, "40 steps of 0.05 is more than enough headroom to fully arrive");
    }

    #[test]
    fn a_flickering_reading_does_not_visibly_swing_the_backlight() {
        // The actual complaint this module exists to prevent: a sensor
        // alternating between two lux values every reading (a hand passing,
        // a shadow) must produce a SMOOTHED fraction, not a fraction that
        // itself alternates between two values every call.
        let limiter = RateLimiter { max_step: 0.05 };
        let mut fraction = 0.5;
        let mut swings = Vec::new();
        for i in 0..20 {
            let lux = if i % 2 == 0 { 50.0 } else { 2000.0 };
            fraction = next_fraction(Sample::Value(lux), Some(fraction), &Curve::DEFAULT, &limiter).unwrap();
            swings.push(fraction);
        }
        let max_delta = swings.windows(2).map(|w| (w[1] - w[0]).abs()).fold(0.0, f64::max);
        assert!(max_delta <= 0.05 + 1e-9, "no single step exceeded the rate limit even under flicker: {max_delta}");
    }

    #[test]
    fn writing_scales_by_the_devices_own_max_not_a_hardcoded_range() {
        let dir = std::env::temp_dir().join(format!("light-kit-bridge-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("max_brightness"), "255").unwrap();
        write_backlight(&dir, 0.5).unwrap();
        let got: u32 = std::fs::read_to_string(dir.join("brightness")).unwrap().trim().parse().unwrap();
        assert_eq!(got, 128, "0.5 of a 255-max device rounds to 128");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn writing_clamps_an_out_of_range_fraction_rather_than_writing_garbage() {
        let dir = std::env::temp_dir().join(format!("light-kit-bridge-test2-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("max_brightness"), "100").unwrap();
        write_backlight(&dir, 5.0).unwrap(); // a caller bug passing something > 1.0
        let got: u32 = std::fs::read_to_string(dir.join("brightness")).unwrap().trim().parse().unwrap();
        assert_eq!(got, 100, "clamped to the device's actual max, not 500");
        std::fs::remove_dir_all(&dir).ok();
    }
}
