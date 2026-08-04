//! iio_accel — reading the accelerometer directly, in userspace, in Rust.
//!
//! ## Is this kernel work? No.
//!
//! iio-sensor-proxy's own polling driver (`drv-iio-poll-accel.c`) does exactly
//! three things: read `in_accel_x_raw`, `in_accel_y_raw`, `in_accel_z_raw` from
//! a sysfs directory, and multiply by a scale. That is plain file I/O against
//! an interface the kernel's IIO core already exports. No module, no ioctl, no
//! privileged call. If a device has an accelerometer that iio-sensor-proxy can
//! see, this can read it too.
//!
//! ## Can it share with iio-sensor-proxy? Sometimes, and the answer is knowable
//!
//! iio-sensor-proxy has two accelerometer drivers and it matters which one is
//! live:
//!
//! * **`drv-iio-poll-accel`** — sysfs reads on a timer. Nothing is claimed, so
//!   a second reader coexists happily. Both processes read the same files.
//! * **`drv-iio-buffer-accel`** — opens `/dev/iio:deviceN` and ENABLES THE RING
//!   BUFFER. While a buffer is enabled the IIO core refuses `*_raw` sysfs reads
//!   with `EBUSY`. A second reader does not crash; it gets an error, every
//!   time, until the daemon stops.
//!
//! So `EBUSY` is not a failure to log and retry — it is the daemon telling you
//! it owns the device in buffered mode. `Reading::Contended` names that case
//! specifically so a caller can fall back to the D-Bus orientation strings
//! rather than spinning.
//!
//! ## On passing data INTO iio-sensor-proxy
//!
//! There is no interface for that, and it is worth saying rather than leaving
//! as a maybe. iio-sensor-proxy is a *reader* of kernel devices that *exports*
//! over D-Bus; it has no ingest path, no socket, no shared buffer. Its drivers
//! are compiled-in and discovered through udev. Handing it a vector would mean
//! either writing a new driver into the daemon (C, upstream) or presenting a
//! fake IIO device to the kernel (which IS kernel-adjacent work — `iio_dummy`
//! or a UIO/HID shim).
//!
//! The direction that does work is the one this module takes: read the same
//! sysfs the daemon reads, and CLASSIFY IDENTICALLY so the two cannot disagree.
//!
//! ## Why the classifier is copied rather than invented
//!
//! `orientation.c`'s thresholds — 35° portrait, 35° landscape, 70° face-down,
//! 20° tilt, and the `SAME_AXIS_LIMIT` hysteresis — are what the whole fleet's
//! gesture behaviour is tuned against. A Rust reader with its own thresholds
//! would produce a ratchet that fires at different angles depending on which
//! backend happened to be available, which is the worst kind of bug: it works
//! on the developer's phone.

use std::path::{Path, PathBuf};

/// 1G expressed the way `orientation.c` expects it.
const ONE_G: f64 = 256.0;
const GRAVITY: f64 = 9.81;

/// `orientation.c` thresholds, in degrees. Copied deliberately — see the module
/// docs.
pub const THRESHOLD_PORTRAIT: f64 = 35.0;
pub const THRESHOLD_LANDSCAPE: f64 = 35.0;
pub const THRESHOLD_FACE_DOWN: f64 = 70.0;
pub const THRESHOLD_TILT: f64 = 20.0;
/// Hysteresis: below this, stay in the current orientation rather than flipping
/// between two portrait modes at the border.
pub const SAME_AXIS_LIMIT: f64 = 90.0;

/// Per-axis scale from `in_accel_*_scale`, or the shared `in_accel_scale`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Scale { pub x: f64, pub y: f64, pub z: f64 }

impl Default for Scale {
    /// What `reset_accel_scale` does: 1.0 on every axis. A missing scale is not
    /// a reason to refuse a reading — the orientation maths only cares about
    /// ratios between axes, and an unscaled reading still classifies correctly
    /// when all three share the same unit.
    fn default() -> Self { Scale { x: 1.0, y: 1.0, z: 1.0 } }
}

/// What the device reported.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Reading {
    Ok { x: i32, y: i32, z: i32, scale: Scale },
    /// `EBUSY`: iio-sensor-proxy (or something else) has the ring buffer
    /// enabled. Not a transient — fall back to D-Bus rather than retrying.
    Contended,
    /// No accelerometer found.
    Absent,
}

/// The orientation vocabulary, matching the daemon's strings exactly so a
/// consumer cannot tell which backend produced it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OrientationUp { Normal, BottomUp, LeftUp, RightUp, Undefined }

impl OrientationUp {
    /// The string iio-sensor-proxy would publish.
    pub fn as_str(self) -> &'static str {
        match self {
            OrientationUp::Normal => "normal",
            OrientationUp::BottomUp => "bottom-up",
            OrientationUp::LeftUp => "left-up",
            OrientationUp::RightUp => "right-up",
            OrientationUp::Undefined => "undefined",
        }
    }
}

/// The tilt vocabulary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tilt { Vertical, Up, Down, FaceUp, FaceDown, Undefined }

impl Tilt {
    pub fn as_str(self) -> &'static str {
        match self {
            Tilt::Vertical => "vertical",
            Tilt::Up => "tilted-up",
            Tilt::Down => "tilted-down",
            Tilt::FaceUp => "face-up",
            Tilt::FaceDown => "face-down",
            Tilt::Undefined => "undefined",
        }
    }
}

/// Scale a raw reading the way `SCALE()` does: to m/s², then to 1G ≈ 256.
fn scaled(raw: i32, scale: f64) -> f64 { raw as f64 * scale * ONE_G / GRAVITY }

/// `tilt_calc`.
pub fn tilt_calc(prev: Tilt, x: i32, y: i32, z: i32, s: Scale) -> Tilt {
    let (x, y, z) = (scaled(x, s.x), scaled(y, s.y), scaled(z, s.z));
    let tilt = (z.atan2((x * x + y * y).sqrt())).to_degrees().round();
    let a = tilt.abs();
    if a <= THRESHOLD_TILT { Tilt::Vertical }
    else if a > THRESHOLD_TILT && a < THRESHOLD_FACE_DOWN {
        if tilt > 0.0 { Tilt::Up } else { Tilt::Down }
    }
    else if a >= THRESHOLD_FACE_DOWN && a <= 180.0 {
        if tilt > 0.0 { Tilt::FaceUp } else { Tilt::FaceDown }
    }
    else { let _ = prev; Tilt::Undefined }
}

/// `orientation_calc`, hysteresis included.
///
/// `prev` is not decoration. Two of the branches return it unchanged, and
/// dropping them would make the reading flap between portrait modes at the
/// border — which for the glove ratchet means spurious flicks from a hand that
/// is merely holding the phone at an angle.
pub fn orientation_calc(prev: OrientationUp, x: i32, y: i32, z: i32, s: Scale)
    -> OrientationUp {
    let (x, y, z) = (scaled(x, s.x), scaled(y, s.y), scaled(z, s.z));
    let portrait = (x.atan2((y * y + z * z).sqrt())).to_degrees().round();
    let landscape = (y.atan2((x * x + z * z).sqrt())).to_degrees().round();

    // On the common border of both thresholds, hold. The daemon's comment calls
    // this out and it is the difference between a stable reading and a coin
    // flip when the phone is held corner-up.
    if portrait.abs() > THRESHOLD_PORTRAIT && landscape.abs() > THRESHOLD_LANDSCAPE {
        return prev;
    }

    if portrait.abs() > THRESHOLD_PORTRAIT {
        let mut ret = if portrait > 0.0 { OrientationUp::LeftUp } else { OrientationUp::RightUp };
        if matches!(prev, OrientationUp::LeftUp | OrientationUp::RightUp)
            && portrait.abs() < SAME_AXIS_LIMIT { ret = prev; }
        ret
    } else if landscape.abs() > THRESHOLD_LANDSCAPE {
        let mut ret = if landscape > 0.0 { OrientationUp::BottomUp } else { OrientationUp::Normal };
        if matches!(prev, OrientationUp::BottomUp | OrientationUp::Normal)
            && landscape.abs() < SAME_AXIS_LIMIT { ret = prev; }
        ret
    } else {
        prev
    }
}

/// A discovered IIO accelerometer.
#[derive(Debug, Clone, PartialEq)]
pub struct IioAccel {
    pub path: PathBuf,
    pub scale: Scale,
    prev_orientation: OrientationUp,
    prev_tilt: Tilt,
}

/// Where IIO devices live.
pub const IIO_DEVICES: &str = "/sys/bus/iio/devices";

impl IioAccel {
    /// Find the first device exposing `in_accel_x_raw`.
    ///
    /// First rather than best, matching the daemon's own behaviour: a phone
    /// with two accelerometers is rare enough that choosing between them is
    /// speculation, and picking the wrong one is visible immediately.
    pub fn discover() -> Option<IioAccel> { Self::discover_in(Path::new(IIO_DEVICES)) }

    /// Discover under an arbitrary root, so the search is testable without a
    /// device.
    pub fn discover_in(root: &Path) -> Option<IioAccel> {
        let mut entries: Vec<PathBuf> = std::fs::read_dir(root).ok()?
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.join("in_accel_x_raw").exists())
            .collect();
        // Sorted so the choice is deterministic: readdir order is not stable,
        // and a device that changes between boots would be maddening.
        entries.sort();
        let path = entries.into_iter().next()?;
        let scale = read_scale(&path);
        Some(IioAccel { path, scale, prev_orientation: OrientationUp::Undefined,
                        prev_tilt: Tilt::Undefined })
    }

    /// Read the three axes.
    pub fn read(&self) -> Reading {
        let mut vals = [0i32; 3];
        for (i, axis) in ["x", "y", "z"].iter().enumerate() {
            match read_int(&self.path.join(format!("in_accel_{axis}_raw"))) {
                Ok(v) => vals[i] = v,
                Err(e) if e.kind() == std::io::ErrorKind::ResourceBusy => {
                    return Reading::Contended;
                }
                Err(_) => return Reading::Absent,
            }
        }
        Reading::Ok { x: vals[0], y: vals[1], z: vals[2], scale: self.scale }
    }

    /// Read and classify, carrying the hysteresis forward.
    pub fn poll(&mut self) -> (Reading, OrientationUp, Tilt) {
        let r = self.read();
        if let Reading::Ok { x, y, z, scale } = r {
            self.prev_orientation = orientation_calc(self.prev_orientation, x, y, z, scale);
            self.prev_tilt = tilt_calc(self.prev_tilt, x, y, z, scale);
        }
        // On a contended or absent read the LAST classification is kept rather
        // than reset to Undefined. A momentary EBUSY should not tell the
        // ratchet the phone has no orientation — that would re-arm it.
        (r, self.prev_orientation, self.prev_tilt)
    }
}

fn read_int(p: &Path) -> std::io::Result<i32> {
    let s = std::fs::read_to_string(p)?;
    s.trim().parse::<i32>()
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
}

fn read_f64(p: &Path) -> Option<f64> {
    std::fs::read_to_string(p).ok()?.trim().parse().ok()
}

/// Per-axis scale, falling back to the shared one, then to 1.0.
pub fn read_scale(dir: &Path) -> Scale {
    let shared = read_f64(&dir.join("in_accel_scale"));
    let axis = |a: &str| read_f64(&dir.join(format!("in_accel_{a}_scale")))
        .or(shared)
        .unwrap_or(1.0);
    Scale { x: axis("x"), y: axis("y"), z: axis("z") }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// One G on an axis, in raw counts, for a scale of 1.0.
    const G: i32 = 10;   // 10 * 1.0 * 256/9.81 ≈ 261 ≈ 1G

    fn s() -> Scale { Scale::default() }

    #[test]
    fn a_phone_lying_face_up_reads_face_up() {
        // Gravity in +z.
        assert_eq!(tilt_calc(Tilt::Undefined, 0, 0, G, s()), Tilt::FaceUp);
    }

    #[test]
    fn a_phone_lying_face_down_reads_face_down() {
        assert_eq!(tilt_calc(Tilt::Undefined, 0, 0, -G, s()), Tilt::FaceDown);
    }

    #[test]
    fn a_phone_held_upright_reads_vertical() {
        // Gravity in -y, nothing in z.
        assert_eq!(tilt_calc(Tilt::Undefined, 0, -G, 0, s()), Tilt::Vertical);
    }

    #[test]
    fn upright_reads_normal() {
        assert_eq!(orientation_calc(OrientationUp::Undefined, 0, -G, 0, s()),
                   OrientationUp::Normal);
    }

    #[test]
    fn turning_the_left_edge_up_reads_left_up() {
        assert_eq!(orientation_calc(OrientationUp::Undefined, G, 0, 0, s()),
                   OrientationUp::LeftUp);
    }

    #[test]
    fn turning_the_right_edge_up_reads_right_up() {
        assert_eq!(orientation_calc(OrientationUp::Undefined, -G, 0, 0, s()),
                   OrientationUp::RightUp);
    }

    #[test]
    fn inverting_reads_bottom_up() {
        assert_eq!(orientation_calc(OrientationUp::Undefined, 0, G, 0, s()),
                   OrientationUp::BottomUp);
    }

    #[test]
    fn a_phone_held_corner_up_holds_its_previous_orientation() {
        // Both thresholds crossed at once. The daemon's own comment calls this
        // out, and it is the difference between a stable reading and a coin
        // flip.
        let held = orientation_calc(OrientationUp::Normal, G, G, 0, s());
        assert_eq!(held, OrientationUp::Normal);
    }

    #[test]
    fn a_flat_phone_holds_its_previous_orientation_rather_than_guessing() {
        // Face-up, no meaningful x or y: which way is "up" is genuinely
        // unknowable, so the last answer stands. Resetting to Undefined here
        // would re-arm the glove ratchet every time the phone was set down.
        assert_eq!(orientation_calc(OrientationUp::LeftUp, 0, 0, G, s()),
                   OrientationUp::LeftUp);
    }

    #[test]
    fn the_hysteresis_stops_portrait_modes_flapping_at_the_border() {
        // Just past the threshold, coming FROM the opposite portrait mode: hold
        // rather than flip. For the glove ratchet a flap is a spurious flick.
        let just_past = (THRESHOLD_PORTRAIT + 2.0).to_radians().tan() * 256.0;
        let x = (just_past * 9.81 / 256.0).round() as i32;
        let held = orientation_calc(OrientationUp::RightUp, x, 0, -G, s());
        assert_eq!(held, OrientationUp::RightUp, "held rather than flipping to LeftUp");
    }

    #[test]
    fn the_strings_match_what_the_daemon_publishes() {
        // A consumer must not be able to tell which backend produced a reading.
        assert_eq!(OrientationUp::Normal.as_str(), "normal");
        assert_eq!(OrientationUp::BottomUp.as_str(), "bottom-up");
        assert_eq!(OrientationUp::LeftUp.as_str(), "left-up");
        assert_eq!(OrientationUp::RightUp.as_str(), "right-up");
        assert_eq!(Tilt::FaceDown.as_str(), "face-down");
    }

    #[test]
    fn a_missing_scale_falls_back_to_one_rather_than_refusing() {
        // The orientation maths only cares about ratios between axes, so an
        // unscaled reading still classifies correctly when all three share a
        // unit. Refusing would turn a cosmetic gap into no sensor at all.
        let d = tempdir("noscale");
        assert_eq!(read_scale(&d), Scale { x: 1.0, y: 1.0, z: 1.0 });
    }

    #[test]
    fn a_shared_scale_applies_to_every_axis() {
        let d = tempdir("shared");
        fs::write(d.join("in_accel_scale"), "0.000598\n").unwrap();
        let s = read_scale(&d);
        assert!((s.x - 0.000598).abs() < 1e-9);
        assert_eq!(s.x, s.z);
    }

    #[test]
    fn a_per_axis_scale_wins_over_the_shared_one() {
        let d = tempdir("peraxis");
        fs::write(d.join("in_accel_scale"), "0.001\n").unwrap();
        fs::write(d.join("in_accel_y_scale"), "0.002\n").unwrap();
        let s = read_scale(&d);
        assert!((s.x - 0.001).abs() < 1e-9);
        assert!((s.y - 0.002).abs() < 1e-9);
    }

    #[test]
    fn discovery_finds_a_device_by_its_axis_file() {
        let root = tempdir("root");
        let dev = root.join("iio:device1");
        fs::create_dir_all(&dev).unwrap();
        fs::write(dev.join("in_accel_x_raw"), "0\n").unwrap();
        // A sibling that is not an accelerometer must be skipped.
        let other = root.join("iio:device0");
        fs::create_dir_all(&other).unwrap();
        fs::write(other.join("in_illuminance_raw"), "42\n").unwrap();
        let found = IioAccel::discover_in(&root).expect("found the accelerometer");
        assert_eq!(found.path, dev);
    }

    #[test]
    fn discovery_is_deterministic_across_runs() {
        // readdir order is not stable, and a device that changed between boots
        // would be maddening to debug.
        let root = tempdir("determ");
        for n in ["iio:device2", "iio:device0", "iio:device1"] {
            let d = root.join(n);
            fs::create_dir_all(&d).unwrap();
            fs::write(d.join("in_accel_x_raw"), "0\n").unwrap();
        }
        let a = IioAccel::discover_in(&root).unwrap().path;
        let b = IioAccel::discover_in(&root).unwrap().path;
        assert_eq!(a, b);
        assert!(a.ends_with("iio:device0"));
    }

    #[test]
    fn no_accelerometer_discovers_nothing() {
        let root = tempdir("empty");
        assert!(IioAccel::discover_in(&root).is_none());
    }

    #[test]
    fn a_partial_device_reads_as_absent_rather_than_zero() {
        // x present, y and z missing: reporting (n, 0, 0) would classify as a
        // confident sideways orientation from one axis of real data.
        let root = tempdir("partial");
        let dev = root.join("iio:device0");
        fs::create_dir_all(&dev).unwrap();
        fs::write(dev.join("in_accel_x_raw"), "100\n").unwrap();
        let a = IioAccel::discover_in(&root).unwrap();
        assert_eq!(a.read(), Reading::Absent);
    }

    #[test]
    fn a_contended_read_keeps_the_last_classification() {
        // A momentary EBUSY must not tell the ratchet the phone has no
        // orientation — that would re-arm it and allow a spurious step.
        let root = tempdir("contend");
        let dev = root.join("iio:device0");
        fs::create_dir_all(&dev).unwrap();
        fs::write(dev.join("in_accel_x_raw"), "0\n").unwrap();
        fs::write(dev.join("in_accel_y_raw"), "-10\n").unwrap();
        fs::write(dev.join("in_accel_z_raw"), "0\n").unwrap();
        let mut a = IioAccel::discover_in(&root).unwrap();
        let (_, o1, _) = a.poll();
        assert_eq!(o1, OrientationUp::Normal);
        // Now break the read the way EBUSY would.
        fs::remove_file(dev.join("in_accel_y_raw")).unwrap();
        let (r, o2, _) = a.poll();
        assert_ne!(r, Reading::Ok { x: 0, y: -10, z: 0, scale: Scale::default() });
        assert_eq!(o2, OrientationUp::Normal, "the last answer stands");
    }

    fn tempdir(tag: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("iio_accel_test_{tag}_{}", std::process::id()));
        let _ = fs::remove_dir_all(&p);
        fs::create_dir_all(&p).unwrap();
        p
    }
}
