//! device — the one part of this crate that touches disk. Discovers a
//! vibrator motor exposed as a Linux LED-class device
//! (`/sys/class/leds/*`) by name, at runtime, rather than a hardcoded
//! per-phone path — the same reasoning `light_kit_bridge::write_backlight`
//! reads `max_brightness` fresh each call instead of trusting a cached
//! device: a hardcoded path is exactly wrong the moment this runs on
//! different hardware than whoever wrote the path had in hand, and this
//! crate has no reason to assume it only ever runs on one phone model.
//!
//! Exposing a vibrator as an LED-class device (write a brightness value,
//! 0 or up to `max_brightness`, to turn it on/off or set intensity on
//! motors that support it) is the common Linux pattern on postmarketOS/
//! Android-derived kernels — not verified against a specific device in
//! this sandbox (there is none), but not a guess either: it's the
//! documented kernel LED-class subsystem, the same subsystem
//! `light_kit_bridge` already writes to for backlight, just a different
//! device under the same `/sys/class/leds/` tree in the common case where
//! backlight itself isn't under `/sys/class/backlight/` but exposed as an
//! LED too on some devices — the two are siblings in the same subsystem.

use std::path::{Path, PathBuf};

/// Scans `/sys/class/leds/*` for a device whose own directory name
/// contains "vibrat" (case-insensitive — covers "vibrator", "vibration",
/// and the occasional non-English-first naming some kernels use).
/// Returns the first match; a device with more than one such LED (rare,
/// but not impossible) gets whichever the filesystem lists first, which
/// is not a meaningful guarantee — a caller that cares which one on a
/// multi-motor device should scan `leds_dir` itself rather than rely on
/// this picking the "right" one.
pub fn find_vibrator(leds_dir: &Path) -> Option<PathBuf> {
    let entries = std::fs::read_dir(leds_dir).ok()?;
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.to_lowercase().contains("vibrat") {
            return Some(entry.path());
        }
    }
    None
}

#[derive(Debug)]
pub enum MotorError {
    NotFound,
    Io(std::io::Error),
}

impl std::fmt::Display for MotorError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MotorError::NotFound => write!(f, "no vibrator LED device found under /sys/class/leds"),
            MotorError::Io(e) => write!(f, "vibrator I/O error: {e}"),
        }
    }
}

impl From<std::io::Error> for MotorError {
    fn from(e: std::io::Error) -> Self {
        MotorError::Io(e)
    }
}

/// A discovered motor, ready to drive. Holds only a path — re-reads
/// `max_brightness` on every `set` call rather than caching it, same
/// reasoning as `light_kit_bridge::write_backlight`.
pub struct Motor {
    device_dir: PathBuf,
}

impl Motor {
    pub fn discover() -> Result<Motor, MotorError> {
        Self::discover_under(Path::new("/sys/class/leds"))
    }

    /// Same as `discover`, with an injectable directory — what makes this
    /// testable without a real `/sys/class/leds` tree: a test can point it
    /// at a fabricated temp directory shaped the same way.
    pub fn discover_under(leds_dir: &Path) -> Result<Motor, MotorError> {
        find_vibrator(leds_dir).map(|device_dir| Motor { device_dir }).ok_or(MotorError::NotFound)
    }

    /// `intensity` in `0.0..=1.0`; clamped, same convention as
    /// `light_kit_bridge::write_backlight`. `0.0` turns the motor fully
    /// off, matching `PatternPlayer::MotorState::Off`/`Finished`.
    pub fn set(&self, intensity: f64) -> Result<(), MotorError> {
        let max: u32 = std::fs::read_to_string(self.device_dir.join("max_brightness"))?
            .trim()
            .parse()
            .map_err(|e: std::num::ParseIntError| MotorError::Io(std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string())))?;
        let value = (intensity.clamp(0.0, 1.0) * max as f64).round() as u32;
        std::fs::write(self.device_dir.join("brightness"), value.to_string())?;
        Ok(())
    }

    pub fn off(&self) -> Result<(), MotorError> {
        self.set(0.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fake_leds_dir(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("haptic-kit-test-{}-{name}", std::process::id()))
    }

    #[test]
    fn finds_a_device_whose_name_contains_vibrator_case_insensitively() {
        let dir = fake_leds_dir("find");
        std::fs::create_dir_all(dir.join("Vibrator")).unwrap();
        std::fs::create_dir_all(dir.join("charging")).unwrap(); // a decoy, must not match
        let found = find_vibrator(&dir).expect("must find the vibrator device");
        assert!(found.ends_with("Vibrator"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn no_matching_device_is_a_named_error_not_a_panic() {
        let dir = fake_leds_dir("empty");
        std::fs::create_dir_all(dir.join("charging")).unwrap();
        assert!(matches!(Motor::discover_under(&dir), Err(MotorError::NotFound)));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_missing_leds_directory_entirely_is_also_a_named_error() {
        let dir = fake_leds_dir("does-not-exist-at-all");
        assert!(matches!(Motor::discover_under(&dir), Err(MotorError::NotFound)));
    }

    #[test]
    fn set_scales_by_the_devices_own_max_not_a_hardcoded_range() {
        let dir = fake_leds_dir("scale");
        let vib_dir = dir.join("vibrator");
        std::fs::create_dir_all(&vib_dir).unwrap();
        std::fs::write(vib_dir.join("max_brightness"), "255").unwrap();
        let motor = Motor::discover_under(&dir).unwrap();
        motor.set(0.5).unwrap();
        let got: u32 = std::fs::read_to_string(vib_dir.join("brightness")).unwrap().trim().parse().unwrap();
        assert_eq!(got, 128);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn off_writes_exactly_zero_regardless_of_max_brightness() {
        let dir = fake_leds_dir("off");
        let vib_dir = dir.join("vibrator");
        std::fs::create_dir_all(&vib_dir).unwrap();
        std::fs::write(vib_dir.join("max_brightness"), "1000").unwrap();
        let motor = Motor::discover_under(&dir).unwrap();
        motor.off().unwrap();
        let got: u32 = std::fs::read_to_string(vib_dir.join("brightness")).unwrap().trim().parse().unwrap();
        assert_eq!(got, 0);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn an_out_of_range_intensity_is_clamped_not_written_raw() {
        let dir = fake_leds_dir("clamp");
        let vib_dir = dir.join("vibrator");
        std::fs::create_dir_all(&vib_dir).unwrap();
        std::fs::write(vib_dir.join("max_brightness"), "100").unwrap();
        let motor = Motor::discover_under(&dir).unwrap();
        motor.set(5.0).unwrap(); // a caller bug passing something > 1.0
        let got: u32 = std::fs::read_to_string(vib_dir.join("brightness")).unwrap().trim().parse().unwrap();
        assert_eq!(got, 100, "clamped to the device's actual max, not 500");
        std::fs::remove_dir_all(&dir).ok();
    }
}
