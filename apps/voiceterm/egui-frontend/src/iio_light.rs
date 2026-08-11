//! iio_light — reading the proximity and ambient-light sensors, the way
//! [`crate::iio_accel`] reads the accelerometer.
//!
//! Same interface, same argument: iio-sensor-proxy's own polling drivers do
//! plain file I/O against sysfs paths the kernel's IIO core already exports.
//! No module, no ioctl, no privileged call.
//!
//! ## Two sensors, one file layout, three different names
//!
//! The kernel is not consistent about what these channels are called, and a
//! reader that knows one name finds a sensor on one phone and nothing on the
//! next:
//!
//! ```text
//!   proximity   in_proximity_raw
//!               in_proximity0_raw      (indexed, when several exist)
//!   light       in_illuminance_raw     (lux, the useful one)
//!               in_intensity_both_raw  (raw counts, no unit)
//!               in_illuminance0_raw
//! ```
//!
//! So discovery takes a **list** of candidate names per sensor and reports
//! which one it found — because "no ambient light sensor" and "an ambient light
//! sensor under a name we do not read" are different problems and only one of
//! them is the user's.
//!
//! ## The EBUSY trap, inherited from the accelerometer
//!
//! `iio_accel` documents it: while a driver has the ring buffer enabled, the
//! IIO core refuses `*_raw` sysfs reads with `EBUSY`. A second reader does not
//! crash, it gets an error every time. The same applies here, so a read failure
//! is reported as **a distinct state** rather than as darkness — a light sensor
//! that reports zero lux because another process owns it would drive the
//! backlight to minimum in a bright room.
//!
//! ## Scale, and why raw counts are not lux
//!
//! IIO exports `in_illuminance_scale` when the raw value needs multiplying. A
//! reader that ignores it reports counts as if they were lux, and
//! [`crate::light_kit_bridge`]'s perceptual curve is calibrated in lux — so the
//! error is not a constant offset, it is a differently-shaped response curve.

use std::path::{Path, PathBuf};

/// Where IIO devices live.
pub const IIO_DEVICES: &str = "/sys/bus/iio/devices";

/// Candidate channel names for proximity, most specific first.
pub const PROXIMITY_CHANNELS: &[&str] = &[
    "in_proximity_raw", "in_proximity0_raw", "in_proximity1_raw",
];

/// Candidate channel names for ambient light.
///
/// `in_illuminance_raw` first because it carries a unit. The intensity
/// channels are raw counts with no defined scale, usable for **differentials**
/// but not for an absolute lux figure — which is exactly the split light-kit
/// already makes between cover detection and brightness control.
pub const LIGHT_CHANNELS: &[&str] = &[
    "in_illuminance_raw", "in_illuminance0_raw",
    "in_intensity_both_raw", "in_intensity_raw",
];

/// What a read produced.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Sample {
    /// A value, already scaled if the device exported a scale.
    Value(f64),
    /// The file exists but the read failed — almost always EBUSY from another
    /// process owning the ring buffer. Distinct from darkness on purpose.
    Busy,
    /// The file went away: a hot-unplugged USB sensor, or a driver unbound.
    Gone,
}

impl Sample {
    pub fn value(self) -> Option<f64> {
        match self { Sample::Value(v) => Some(v), _ => None }
    }
    /// Should a controller act on this? Only a real value — a Busy read must
    /// not drive the backlight to minimum in a bright room.
    pub fn is_usable(self) -> bool { matches!(self, Sample::Value(_)) }
}

/// Whether a channel's readings carry a real unit.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Unit {
    /// Lux. Absolute brightness control is meaningful.
    Illuminance,
    /// Raw counts. Differentials work; an absolute curve does not.
    Counts,
}

/// A discovered sensor.
#[derive(Clone, Debug, PartialEq)]
pub struct IioSensor {
    pub path: PathBuf,
    /// The channel file actually found — reported so a log can say WHICH name
    /// worked, which is the difference between diagnosing a missing sensor and
    /// a differently-named one.
    pub channel: String,
    /// Multiplier from `*_scale`, or 1.0.
    pub scale: f64,
    pub unit: Unit,
}

fn read_f64(p: &Path) -> Option<f64> {
    std::fs::read_to_string(p).ok()?.trim().parse().ok()
}

/// The unit a channel name implies.
pub fn unit_of(channel: &str) -> Unit {
    if channel.starts_with("in_illuminance") { Unit::Illuminance } else { Unit::Counts }
}

/// The scale file paired with a raw channel.
///
/// `in_illuminance0_raw` pairs with `in_illuminance0_scale`, and there is also
/// a shared `in_illuminance_scale`. Both are checked because devices export one
/// or the other, and missing it means reporting counts as lux.
pub fn scale_for(dir: &Path, channel: &str) -> f64 {
    let base = channel.trim_end_matches("_raw");
    for name in [format!("{base}_scale"), {
        // Strip a trailing index for the shared form.
        let shared: String = base.trim_end_matches(|c: char| c.is_ascii_digit()).into();
        format!("{shared}_scale")
    }] {
        if let Some(v) = read_f64(&dir.join(&name)) {
            if v.is_finite() && v != 0.0 { return v; }
        }
    }
    1.0
}

impl IioSensor {
    /// Find a sensor under the real sysfs root.
    pub fn discover(channels: &[&str]) -> Option<IioSensor> {
        Self::discover_in(Path::new(IIO_DEVICES), channels)
    }

    /// Find one under an arbitrary root, so discovery is testable with no
    /// device — the same seam `iio_accel::discover_in` uses.
    pub fn discover_in(root: &Path, channels: &[&str]) -> Option<IioSensor> {
        let mut found: Vec<(PathBuf, String)> = Vec::new();
        let Ok(entries) = std::fs::read_dir(root) else { return None };
        for e in entries.filter_map(|e| e.ok()) {
            let dir = e.path();
            for ch in channels {
                if dir.join(ch).exists() {
                    found.push((dir.clone(), (*ch).to_string()));
                    break;
                }
            }
        }
        // Sorted for the reason iio_accel gives: readdir order is not stable,
        // and a device that changes between boots would be maddening.
        found.sort();
        let (path, channel) = found.into_iter().next()?;
        let scale = scale_for(&path, &channel);
        let unit = unit_of(&channel);
        Some(IioSensor { path, channel, scale, unit })
    }

    /// Read once.
    pub fn read(&self) -> Sample {
        let f = self.path.join(&self.channel);
        match std::fs::read_to_string(&f) {
            Ok(s) => match s.trim().parse::<f64>() {
                Ok(v) => Sample::Value(v * self.scale),
                // A file that exists and holds something unparseable is a
                // driver quirk, not an absent sensor.
                Err(_) => Sample::Busy,
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Sample::Gone,
            // EBUSY and permission errors alike: the sensor is there and we
            // cannot have it. Reporting darkness here would drive the backlight
            // to minimum in a bright room.
            Err(_) => Sample::Busy,
        }
    }
}

/// Find the proximity sensor.
pub fn proximity() -> Option<IioSensor> { IioSensor::discover(PROXIMITY_CHANNELS) }

/// Find the ambient light sensor.
pub fn ambient_light() -> Option<IioSensor> { IioSensor::discover(LIGHT_CHANNELS) }

/// What the app can offer, given what was found.
///
/// Mirrors `hover_kit::capability` deliberately: the same question asked of the
/// hardware rather than of the policy, so a settings page can explain a missing
/// feature instead of showing a dead control.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Sensors {
    pub has_proximity: bool,
    pub has_light: bool,
    /// True when the light sensor reports counts rather than lux.
    pub light_is_relative: bool,
}

impl Sensors {
    pub fn detect_in(root: &Path) -> Sensors {
        let p = IioSensor::discover_in(root, PROXIMITY_CHANNELS);
        let l = IioSensor::discover_in(root, LIGHT_CHANNELS);
        Sensors {
            has_proximity: p.is_some(),
            light_is_relative: l.as_ref().map(|s| s.unit == Unit::Counts).unwrap_or(false),
            has_light: l.is_some(),
        }
    }

    /// Can absolute brightness control run?
    ///
    /// Needs lux, not counts: light-kit's perceptual curve is calibrated in lux
    /// and feeding it raw counts gives a differently-SHAPED response, not a
    /// constant offset — the screen would be wrong in a way that looks like bad
    /// tuning rather than bad units.
    pub fn can_auto_brightness(&self) -> bool { self.has_light && !self.light_is_relative }

    /// Can cover detection run? Differentials only need consistency, so raw
    /// counts are fine.
    pub fn can_detect_cover(&self) -> bool { self.has_light }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn fake(name: &str, files: &[(&str, &str)]) -> PathBuf {
        let root = std::env::temp_dir().join(format!("iio-light-test-{name}"));
        let _ = fs::remove_dir_all(&root);
        for (path, content) in files {
            let p = root.join(path);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(&p, content).unwrap();
        }
        fs::create_dir_all(&root).unwrap();
        root
    }

    #[test]
    fn a_proximity_sensor_is_found_under_its_usual_name() {
        let root = fake("prox", &[("iio:device0/in_proximity_raw", "42\n")]);
        let s = IioSensor::discover_in(&root, PROXIMITY_CHANNELS).expect("found");
        assert_eq!(s.channel, "in_proximity_raw");
        assert_eq!(s.read(), Sample::Value(42.0));
    }

    #[test]
    fn an_indexed_channel_name_is_found_too() {
        // Devices with several sensors index them, and a reader that knows one
        // name finds a sensor on one phone and nothing on the next.
        let root = fake("idx", &[("iio:device3/in_proximity0_raw", "7\n")]);
        let s = IioSensor::discover_in(&root, PROXIMITY_CHANNELS).expect("found");
        assert_eq!(s.channel, "in_proximity0_raw");
    }

    #[test]
    fn the_channel_that_worked_is_reported() {
        // "No ambient light sensor" and "one under a name we do not read" are
        // different problems, and only one of them is the user's.
        let root = fake("report", &[("iio:device0/in_intensity_both_raw", "800\n")]);
        let s = IioSensor::discover_in(&root, LIGHT_CHANNELS).expect("found");
        assert_eq!(s.channel, "in_intensity_both_raw");
    }

    #[test]
    fn illuminance_is_preferred_over_raw_intensity() {
        // Lux carries a unit; counts do not. Picking the intensity channel when
        // a lux one exists throws away the calibration.
        let root = fake("prefer", &[
            ("iio:device0/in_intensity_both_raw", "800\n"),
            ("iio:device0/in_illuminance_raw", "250\n"),
        ]);
        let s = IioSensor::discover_in(&root, LIGHT_CHANNELS).expect("found");
        assert_eq!(s.channel, "in_illuminance_raw");
        assert_eq!(s.unit, Unit::Illuminance);
    }

    #[test]
    fn a_scale_is_applied_so_counts_do_not_masquerade_as_lux() {
        // Ignoring the scale reports counts as lux, and light-kit's perceptual
        // curve is calibrated in lux — the error is a differently-SHAPED
        // response, not a constant offset.
        let root = fake("scale", &[
            ("iio:device0/in_illuminance_raw", "100\n"),
            ("iio:device0/in_illuminance_scale", "2.5\n"),
        ]);
        let s = IioSensor::discover_in(&root, LIGHT_CHANNELS).expect("found");
        assert_eq!(s.scale, 2.5);
        assert_eq!(s.read(), Sample::Value(250.0));
    }

    #[test]
    fn an_indexed_channel_finds_the_shared_scale() {
        // Devices export one form or the other; missing it is the same bug.
        let root = fake("sharedscale", &[
            ("iio:device0/in_illuminance0_raw", "10\n"),
            ("iio:device0/in_illuminance_scale", "3.0\n"),
        ]);
        let s = IioSensor::discover_in(&root, LIGHT_CHANNELS).expect("found");
        assert_eq!(s.scale, 3.0);
    }

    #[test]
    fn a_zero_or_missing_scale_falls_back_to_one() {
        // A zero scale would report every reading as darkness.
        let root = fake("zeroscale", &[
            ("iio:device0/in_illuminance_raw", "100\n"),
            ("iio:device0/in_illuminance_scale", "0\n"),
        ]);
        let s = IioSensor::discover_in(&root, LIGHT_CHANNELS).expect("found");
        assert_eq!(s.scale, 1.0);
        assert_eq!(s.read(), Sample::Value(100.0));
    }

    #[test]
    fn a_busy_sensor_is_not_reported_as_darkness() {
        // The EBUSY trap iio_accel documents: while another process owns the
        // ring buffer, sysfs reads fail every time. Reporting zero lux would
        // drive the backlight to minimum in a bright room.
        let root = fake("busy", &[("iio:device0/in_illuminance_raw", "not-a-number\n")]);
        let s = IioSensor::discover_in(&root, LIGHT_CHANNELS).expect("found");
        assert_eq!(s.read(), Sample::Busy);
        assert!(!s.read().is_usable());
        assert_eq!(s.read().value(), None, "a busy read must not yield a number");
    }

    #[test]
    fn a_vanished_sensor_is_distinguished_from_a_busy_one() {
        // A hot-unplugged USB sensor should stop the feature; a busy one should
        // retry. Conflating them either spins forever or gives up too early.
        let root = fake("gone", &[("iio:device0/in_proximity_raw", "1\n")]);
        let s = IioSensor::discover_in(&root, PROXIMITY_CHANNELS).expect("found");
        fs::remove_file(s.path.join(&s.channel)).unwrap();
        assert_eq!(s.read(), Sample::Gone);
    }

    #[test]
    fn discovery_is_deterministic_across_runs() {
        // iio_accel's reason, and it applies identically: readdir order is not
        // stable, and a device that changes between boots would be maddening.
        let root = fake("many", &[
            ("iio:device2/in_proximity_raw", "2\n"),
            ("iio:device0/in_proximity_raw", "0\n"),
            ("iio:device1/in_proximity_raw", "1\n"),
        ]);
        let first = IioSensor::discover_in(&root, PROXIMITY_CHANNELS).unwrap();
        for _ in 0..5 {
            assert_eq!(IioSensor::discover_in(&root, PROXIMITY_CHANNELS).unwrap().path, first.path);
        }
    }

    #[test]
    fn a_root_with_no_sensors_is_none_rather_than_an_error() {
        // A desktop. Not a failure, and it must not look like one.
        let root = fake("empty", &[("iio:device0/in_accel_x_raw", "0\n")]);
        assert!(IioSensor::discover_in(&root, PROXIMITY_CHANNELS).is_none());
        assert!(IioSensor::discover_in(&root, LIGHT_CHANNELS).is_none());
    }

    #[test]
    fn a_missing_sysfs_root_does_not_panic() {
        // Not every kernel has IIO at all.
        let missing = std::env::temp_dir().join("iio-light-test-nonexistent-xyz");
        assert!(IioSensor::discover_in(&missing, PROXIMITY_CHANNELS).is_none());
    }

    #[test]
    fn capability_distinguishes_absolute_from_relative_light() {
        // Cover detection needs only consistency; brightness control needs lux.
        // Offering auto-brightness on a counts-only sensor gives a
        // differently-shaped response that reads as bad tuning rather than bad
        // units.
        let lux = fake("caplux", &[("iio:device0/in_illuminance_raw", "100\n")]);
        let s = Sensors::detect_in(&lux);
        assert!(s.can_auto_brightness() && s.can_detect_cover());

        let counts = fake("capcounts", &[("iio:device0/in_intensity_both_raw", "100\n")]);
        let s = Sensors::detect_in(&counts);
        assert!(s.has_light);
        assert!(s.light_is_relative);
        assert!(!s.can_auto_brightness(), "offered lux control on raw counts");
        assert!(s.can_detect_cover(), "differentials work on counts");
    }

    #[test]
    fn a_desktop_offers_neither() {
        let root = fake("desktop", &[("iio:device0/in_accel_x_raw", "0\n")]);
        let s = Sensors::detect_in(&root);
        assert!(!s.has_proximity && !s.has_light);
        assert!(!s.can_auto_brightness() && !s.can_detect_cover());
    }
}
