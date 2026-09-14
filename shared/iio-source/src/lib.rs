//! iio-source — the raw sysfs accelerometer reader, shared.
//!
//! Lifted VERBATIM from `apps/voiceterm/egui-frontend/src/iio_accel.rs`,
//! which `maintenance/C18_TODO.md` section 2 identified as the most
//! complete of eight independent, hand-rolled IIO readers in this fleet
//! (voiceterm, `uikit-egui`'s `glove.rs`, `blackstar-rs`/`skydribble`'s
//! byte-identical `gyro.rs` pair, `snapclone`'s `glove_input.rs`,
//! `ambient-learner`'s `environment.rs`, plus `systems/
//! environment_system.rs`). Three of those independently glob
//! `/sys/bus/iio/devices` and parse `*_raw`/`*_scale`; this is the only one
//! that also applies `in_accel_mount_matrix`, without which a reading is
//! silently wrong on any panel mounted rotated relative to the SoC — which
//! is most phones.
//!
//! `shared/sensor-kit` is NOT this layer and does not replace it — checked
//! before assuming otherwise, since sensor-kit exists, mentions IIO, and
//! has zero consumers, which looks like exactly the "why didn't anyone use
//! the kit" story this module would otherwise repeat. Reading it says
//! differently: sensor-kit parses `iio-sensor-proxy`'s `monitor-sensor`
//! TEXT output into coarse `Tilt`/`Orient` events, and its own doc scopes
//! it deliberately to that: "the platform reader … feeds lines/events in;
//! the control math lives here." The eight readers this crate replaces
//! want continuous RAW values read directly from sysfs — a different
//! layer, one level lower, which is why an app needing continuous tilt
//! could never have started from sensor-kit no matter how well-known it
//! was. This crate is that lower layer; sensor-kit (or a future
//! `iio-sensor-proxy`-text-based reader) can consume it as one possible
//! source, and apps needing raw values can depend on it directly.
//!
//! Zero adaptation was needed for the move — the original had no
//! `crate::` dependencies at all (confirmed before copying, not assumed),
//! so this is the file verbatim plus this note.


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

/// A row of the mount matrix.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Vec3 { pub x: f64, pub y: f64, pub z: f64 }

/// The sensor's mounting orientation, as the kernel exports it.
///
/// Ported rather than skipped because WITHOUT IT THE AXES ARE WRONG on a large
/// share of real hardware. A phone's accelerometer is soldered at whatever
/// rotation suited the board layout, and the kernel publishes a 3x3 matrix
/// saying how to get from chip axes to screen axes. Skip it and every gesture
/// is mirrored or rotated on those devices — which is precisely the "works on
/// the developer's phone" failure this port exists to avoid.
///
/// The lookup order is iio-sensor-proxy's: the udev property `ACCEL_MOUNT_MATRIX`
/// first (distributions ship per-device quirks there), then sysfs
/// `mount_matrix`, then `in_accel_mount_matrix` — the last because some IIO
/// drivers expose several sensors from one path and each needs its own matrix.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MountMatrix(pub [Vec3; 3]);

impl Default for MountMatrix {
    /// The identity. An empty string means identity upstream too, and a missing
    /// matrix must not mean "no readings" — most devices are mounted straight.
    fn default() -> Self {
        MountMatrix([Vec3 { x: 1.0, y: 0.0, z: 0.0 },
                     Vec3 { x: 0.0, y: 1.0, z: 0.0 },
                     Vec3 { x: 0.0, y: 0.0, z: 1.0 }])
    }
}

impl MountMatrix {
    /// Parse the kernel's format: three rows of three, `;` between rows and `,`
    /// between elements.
    ///
    /// An EMPTY string is the identity, not an error — that is upstream's rule
    /// and drivers do emit it.
    pub fn parse(s: &str) -> Option<MountMatrix> {
        if s.trim().is_empty() { return Some(MountMatrix::default()); }
        let rows: Vec<&str> = s.split(';').collect();
        if rows.len() != 3 { return None; }
        let mut out = [Vec3 { x: 0.0, y: 0.0, z: 0.0 }; 3];
        for (i, row) in rows.iter().enumerate() {
            let e: Vec<f64> = row.split(',')
                .map(|t| t.trim().parse::<f64>().ok())
                .collect::<Option<Vec<f64>>>()?;
            if e.len() != 3 { return None; }
            out[i] = Vec3 { x: e[0], y: e[1], z: e[2] };
        }
        let m = MountMatrix(out);
        // Upstream rejects a matrix with an all-zero row: it would collapse an
        // axis and silently return a constant, which reads as a sensor that
        // works but never moves in one direction.
        if m.0.iter().any(|r| r.x == 0.0 && r.y == 0.0 && r.z == 0.0) { return None; }
        Some(m)
    }

    /// Apply to a raw reading, exactly as `apply_mount_matrix` does.
    pub fn apply(&self, x: f64, y: f64, z: f64) -> (f64, f64, f64) {
        let m = &self.0;
        (x * m[0].x + y * m[0].y + z * m[0].z,
         x * m[1].x + y * m[1].y + z * m[1].z,
         x * m[2].x + y * m[2].y + z * m[2].z)
    }
}

/// Read the mount matrix for a device, in iio-sensor-proxy's lookup order.
///
/// `udev_property` is the caller's job to supply — this crate does not link
/// libudev, and the property is the FIRST place upstream looks, so pretending
/// it does not exist would silently ignore every distribution quirk file.
pub fn read_mount_matrix(dir: &Path, udev_property: Option<&str>) -> MountMatrix {
    if let Some(s) = udev_property {
        if let Some(m) = MountMatrix::parse(s) { return m; }
    }
    for name in ["mount_matrix", "in_accel_mount_matrix"] {
        if let Ok(s) = std::fs::read_to_string(dir.join(name)) {
            if let Some(m) = MountMatrix::parse(&s) { return m; }
        }
    }
    MountMatrix::default()
}

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

// ── relative flicks ─────────────────────────────────────────────────────────

/// A flick measured against WHERE THE PHONE ALREADY IS.
///
/// The orientation ratchet needs the phone turned through an absolute pose —
/// "left-up", "bottom-up" — which means physically rotating it a long way for
/// every step. That is fine for a maze game and tiring for navigating a form,
/// and it is impossible if the phone is docked, propped, or held flat on a
/// table.
///
/// This is the alternative: hold the phone however you like, and jerk it. The
/// motion is measured in the phone's CURRENT frame, so the same wrist movement
/// means the same thing whether you are holding it upright, flat, or at forty
/// degrees on a car mount.
///
/// ## Gravity is the reference, and it is also the problem
///
/// The only thing that says which way is "down" is gravity — the same signal a
/// flick perturbs. So the reference is a SLOW average and the flick is what
/// deviates from it: a low-pass for orientation, the residual for motion. A
/// single sample cannot distinguish "tilted" from "accelerating", which is why
/// this needs a buffer rather than a reading.
///
/// ## Why timing still gates it
///
/// Same reason the orientation ratchet gates: on a bus the whole phone
/// accelerates, and it does so for a long time. A flick is a spike that RETURNS
/// — the residual rises and falls within the window. Sustained acceleration
/// never fires, however strong.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RelFlick { Up, Down, Left, Right }

/// One sample in the phone's own frame, after the mount matrix.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Sample { pub x: f64, pub y: f64, pub z: f64, pub t_ms: f64 }

/// How long the gravity reference takes to follow a real reorientation.
///
/// Slow on purpose: fast enough that turning the phone updates "down" within a
/// second, slow enough that a flick does not drag the reference with it. Too
/// fast and a flick becomes its own baseline and never registers.
pub const GRAVITY_TAU_MS: f64 = 900.0;

/// Residual magnitude, in m/s², that counts as a flick.
///
/// Above a comfortable wrist jerk and well above walking, which sits under
/// about 2 m/s² at the hand.
pub const FLICK_ACCEL: f64 = 4.5;

/// The residual must fall back under this fraction of the peak within
/// `FLICK_RETURN_MS`, or it was a sustained push rather than a flick.
pub const RETURN_FRACTION: f64 = 0.4;
pub const FLICK_RETURN_MS: f64 = 450.0;

/// Minimum gap between flicks, so one jerk is one event.
pub const FLICK_REFRACTORY_MS: f64 = 250.0;

/// Detects flicks relative to the phone's current resting attitude.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RelativeFlicks {
    /// Low-passed gravity, the "which way is down" reference.
    gravity: Option<(f64, f64, f64)>,
    /// Peak residual of the flick in progress, and when it happened.
    peak: Option<(f64, f64, f64, f64)>,
    last_fire_ms: Option<f64>,
}

impl RelativeFlicks {
    /// Feed a sample. Returns a flick on the frame one completes.
    pub fn feed(&mut self, s: Sample) -> Option<RelFlick> {
        let g = match self.gravity {
            None => {
                // The first sample IS the reference. Starting from zero would
                // make the first reading look like a 9.8 m/s² flick.
                self.gravity = Some((s.x, s.y, s.z));
                return None;
            }
            Some(g) => g,
        };
        // Exponential low-pass, framerate-independent so a slow poll does not
        // change what "down" means.
        let a = 1.0 - (-16.0 / GRAVITY_TAU_MS).exp();
        let g = (g.0 + (s.x - g.0) * a, g.1 + (s.y - g.1) * a, g.2 + (s.z - g.2) * a);
        self.gravity = Some(g);

        // The residual: what is left after gravity. This is the motion.
        let (rx, ry, rz) = (s.x - g.0, s.y - g.1, s.z - g.2);
        let mag = (rx * rx + ry * ry + rz * rz).sqrt();

        match self.peak {
            None if mag >= FLICK_ACCEL => {
                if let Some(last) = self.last_fire_ms {
                    // Refractory: one jerk is one event, not a burst.
                    if s.t_ms - last < FLICK_REFRACTORY_MS { return None; }
                }
                self.peak = Some((rx, ry, rz, s.t_ms));
                None
            }
            None => None,
            Some((px, py, pz, pt)) => {
                let peak_mag = (px * px + py * py + pz * pz).sqrt();
                if mag > peak_mag {
                    // Still rising: keep the strongest sample as the direction.
                    self.peak = Some((rx, ry, rz, pt));
                    return None;
                }
                if mag <= peak_mag * RETURN_FRACTION {
                    // Came back. That is a flick.
                    self.peak = None;
                    self.last_fire_ms = Some(s.t_ms);
                    return Self::classify(px, py, pz, g);
                }
                if s.t_ms - pt > FLICK_RETURN_MS {
                    // Never returned: a sustained push, a car accelerating, a
                    // walk. Discarded rather than fired.
                    self.peak = None;
                }
                None
            }
        }
    }

    /// Name a direction from the residual, in the SCREEN's plane.
    ///
    /// The screen plane is phone-frame x and y, always — z points out of the
    /// glass. So a flick is classified on x and y and the z component is
    /// dropped: moving the phone toward or away from the user's face is not a
    /// direction on the screen, and treating it as one would fire every time
    /// the phone was lifted off a table.
    ///
    /// An earlier version removed the component along GRAVITY instead, on the
    /// theory that "up" should mean up in the world. That is wrong and a test
    /// caught it: held upright, the screen's up axis IS the gravity axis, so
    /// every up-and-down flick was cancelled to nothing. Gravity's job here is
    /// to be subtracted as a baseline, not to define the axes.
    ///
    /// The residual has already had gravity removed by the low-pass, so what
    /// arrives is motion in the phone's own frame — which is exactly "relative
    /// to however you are holding it".
    fn classify(rx: f64, ry: f64, _rz: f64, _g: (f64, f64, f64)) -> Option<RelFlick> {
        if rx.abs() < 1e-6 && ry.abs() < 1e-6 { return None; }
        if rx.abs() >= ry.abs() {
            Some(if rx > 0.0 { RelFlick::Right } else { RelFlick::Left })
        } else {
            Some(if ry > 0.0 { RelFlick::Up } else { RelFlick::Down })
        }
    }

    /// The current gravity reference, for a UI that wants to show which way it
    /// thinks is down.
    pub fn gravity(&self) -> Option<(f64, f64, f64)> { self.gravity }

    /// True while a flick is in progress — useful for suppressing the
    /// orientation ratchet so one motion cannot fire both.
    pub fn in_flick(&self) -> bool { self.peak.is_some() }
}

#[cfg(test)]
mod mount_matrix_tests {
    use super::*;

    // The literals are upstream's, from test-mount-matrix.c, so a divergence
    // shows up as a failing test rather than as a phone that feels wrong.
    const SWAP_Y_Z: &str = "1, 0, 0; 0, 0, 1; 0, 1, 0";
    const INVALID: &str = "0, 1, 0; 1, 0, 0; 0, 0, 0";
    const DEFAULT: &str = "1, 0, 0; 0, 1, 0; 0, 0, 1";

    #[test]
    fn swapping_y_and_z_moves_the_reading_the_way_upstream_says() {
        // Upstream's exact case: (0, -256, 0) through the swap matrix becomes
        // x=0, z=-256, y=0.
        let m = MountMatrix::parse(SWAP_Y_Z).expect("parses");
        let (x, y, z) = m.apply(0.0, -256.0, 0.0);
        assert_eq!(x, 0.0);
        assert_eq!(z, -256.0);
        assert_eq!(y, 0.0);
    }

    #[test]
    fn an_empty_matrix_is_the_identity_not_an_error() {
        // Upstream's rule, and drivers do emit an empty string.
        let m = MountMatrix::parse("").expect("empty is identity");
        let (x, y, z) = m.apply(0.0, -256.0, 0.0);
        assert_eq!((x, y, z), (0.0, -256.0, 0.0));
    }

    #[test]
    fn a_matrix_with_a_zero_row_is_rejected() {
        // It would collapse an axis and return a constant — a sensor that
        // appears to work but never moves one way.
        assert!(MountMatrix::parse(INVALID).is_none());
    }

    #[test]
    fn the_default_matrix_parses() {
        assert_eq!(MountMatrix::parse(DEFAULT), Some(MountMatrix::default()));
    }

    #[test]
    fn a_malformed_matrix_is_refused_rather_than_half_read() {
        for bad in ["1, 0, 0", "1,0,0; 0,1,0", "a, b, c; d, e, f; g, h, i",
                    "1,0,0; 0,1,0; 0,0,1; 2,2,2"] {
            assert!(MountMatrix::parse(bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn whitespace_around_elements_is_tolerated() {
        // The kernel's own format has spaces after the commas.
        assert!(MountMatrix::parse("  1 , 0 ,0 ;0, 1 ,0; 0,0,1  ").is_some());
    }

    #[test]
    fn the_udev_property_wins_over_sysfs() {
        // Distributions ship per-device quirks in ACCEL_MOUNT_MATRIX, and they
        // exist precisely because the sysfs value is wrong on that hardware.
        let d = std::env::temp_dir().join(format!("mm_test_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("mount_matrix"), DEFAULT).unwrap();
        let m = read_mount_matrix(&d, Some(SWAP_Y_Z));
        assert_eq!(m, MountMatrix::parse(SWAP_Y_Z).unwrap());
    }

    #[test]
    fn a_bad_udev_property_falls_through_to_sysfs() {
        // Rather than failing: a broken quirk file should not disable the
        // sensor.
        let d = std::env::temp_dir().join(format!("mm_test2_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("mount_matrix"), SWAP_Y_Z).unwrap();
        assert_eq!(read_mount_matrix(&d, Some(INVALID)),
                   MountMatrix::parse(SWAP_Y_Z).unwrap());
    }

    #[test]
    fn no_matrix_anywhere_is_the_identity() {
        // Most devices are mounted straight, and a missing matrix must not mean
        // no readings.
        let d = std::env::temp_dir().join(format!("mm_test3_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        assert_eq!(read_mount_matrix(&d, None), MountMatrix::default());
    }
}

#[cfg(test)]
mod relative_flick_tests {
    use super::*;

    /// Hold still at a given attitude until the reference settles.
    fn settle(r: &mut RelativeFlicks, g: (f64, f64, f64), t: &mut f64) {
        for _ in 0..400 {
            *t += 16.0;
            r.feed(Sample { x: g.0, y: g.1, z: g.2, t_ms: *t });
        }
    }

    /// A jerk: `n` samples of residual on top of gravity, then a return.
    fn jerk(r: &mut RelativeFlicks, g: (f64, f64, f64), d: (f64, f64, f64),
            n: usize, t: &mut f64) -> Option<RelFlick> {
        let mut out = None;
        for _ in 0..n {
            *t += 16.0;
            if let Some(f) = r.feed(Sample { x: g.0 + d.0, y: g.1 + d.1,
                                             z: g.2 + d.2, t_ms: *t }) { out = Some(f); }
        }
        for _ in 0..6 {
            *t += 16.0;
            if let Some(f) = r.feed(Sample { x: g.0, y: g.1, z: g.2, t_ms: *t }) {
                out = Some(f);
            }
        }
        out
    }

    /// Upright: gravity down the screen's -y.
    const UPRIGHT: (f64, f64, f64) = (0.0, -9.81, 0.0);
    /// Flat on a table: gravity out of the screen.
    const FLAT: (f64, f64, f64) = (0.0, 0.0, 9.81);

    #[test]
    fn a_jerk_right_reads_right_when_held_upright() {
        let (mut r, mut t) = (RelativeFlicks::default(), 0.0);
        settle(&mut r, UPRIGHT, &mut t);
        assert_eq!(jerk(&mut r, UPRIGHT, (6.0, 0.0, 0.0), 3, &mut t),
                   Some(RelFlick::Right));
    }

    #[test]
    fn the_same_wrist_movement_works_with_the_phone_flat() {
        // THE point of relative flicks: no reorientation needed. The operator
        // matches the motion at whatever attitude they are already holding.
        let (mut r, mut t) = (RelativeFlicks::default(), 0.0);
        settle(&mut r, FLAT, &mut t);
        assert_eq!(jerk(&mut r, FLAT, (6.0, 0.0, 0.0), 3, &mut t),
                   Some(RelFlick::Right));
        t += FLICK_REFRACTORY_MS;
        assert_eq!(jerk(&mut r, FLAT, (0.0, 6.0, 0.0), 3, &mut t),
                   Some(RelFlick::Up));
    }

    #[test]
    fn all_four_directions_are_distinguished() {
        for (d, want) in [((6.0, 0.0, 0.0), RelFlick::Right),
                          ((-6.0, 0.0, 0.0), RelFlick::Left),
                          ((0.0, 6.0, 0.0), RelFlick::Up),
                          ((0.0, -6.0, 0.0), RelFlick::Down)] {
            let (mut r, mut t) = (RelativeFlicks::default(), 0.0);
            settle(&mut r, UPRIGHT, &mut t);
            assert_eq!(jerk(&mut r, UPRIGHT, d, 3, &mut t), Some(want), "{d:?}");
        }
    }

    #[test]
    fn a_gentle_movement_is_not_a_flick() {
        // Below the threshold: shifting the phone in your hand, or walking.
        let (mut r, mut t) = (RelativeFlicks::default(), 0.0);
        settle(&mut r, UPRIGHT, &mut t);
        assert_eq!(jerk(&mut r, UPRIGHT, (1.5, 0.0, 0.0), 4, &mut t), None);
    }

    #[test]
    fn sustained_acceleration_never_fires() {
        // A bus pulling away, a car, a lift. Strong, and it does not return
        // inside the window — which is exactly what separates it from a flick.
        let (mut r, mut t) = (RelativeFlicks::default(), 0.0);
        settle(&mut r, UPRIGHT, &mut t);
        let mut fired = None;
        for _ in 0..60 {
            t += 16.0;
            if let Some(f) = r.feed(Sample { x: 6.0, y: -9.81, z: 0.0, t_ms: t }) {
                fired = Some(f);
            }
        }
        assert_eq!(fired, None);
    }

    #[test]
    fn one_jerk_is_one_event() {
        // The refractory window. Without it a single vigorous flick fires
        // several times as the residual crosses the threshold on the rebound.
        let (mut r, mut t) = (RelativeFlicks::default(), 0.0);
        settle(&mut r, UPRIGHT, &mut t);
        assert!(jerk(&mut r, UPRIGHT, (6.0, 0.0, 0.0), 3, &mut t).is_some());
        // Immediately again, inside the refractory period.
        assert_eq!(jerk(&mut r, UPRIGHT, (6.0, 0.0, 0.0), 3, &mut t), None);
    }

    #[test]
    fn the_first_sample_is_the_reference_not_a_flick() {
        // Starting the reference at zero would make the very first reading look
        // like a 9.8 m/s² jerk downward.
        let mut r = RelativeFlicks::default();
        assert_eq!(r.feed(Sample { x: 0.0, y: -9.81, z: 0.0, t_ms: 0.0 }), None);
    }

    #[test]
    fn turning_the_phone_slowly_does_not_fire() {
        // A real reorientation moves the gravity reference with it rather than
        // registering as motion — which is what the low-pass is for.
        let (mut r, mut t) = (RelativeFlicks::default(), 0.0);
        settle(&mut r, UPRIGHT, &mut t);
        let mut fired = None;
        for i in 0..200 {
            let a = (i as f64 / 200.0) * std::f64::consts::FRAC_PI_2;
            t += 16.0;
            if let Some(f) = r.feed(Sample { x: 9.81 * a.sin(), y: -9.81 * a.cos(),
                                             z: 0.0, t_ms: t }) { fired = Some(f); }
        }
        assert_eq!(fired, None, "a slow turn is not a flick");
    }

    #[test]
    fn movement_out_of_the_screen_is_not_a_screen_direction() {
        // Lifting a flat phone off a table, or pushing it away from your face,
        // is not a direction ON the glass — and firing there would trigger
        // every time the phone was picked up.
        let (mut r, mut t) = (RelativeFlicks::default(), 0.0);
        settle(&mut r, FLAT, &mut t);
        assert_eq!(jerk(&mut r, FLAT, (0.0, 0.0, 6.0), 3, &mut t), None);
    }

    #[test]
    fn up_works_when_the_screen_axis_and_gravity_coincide() {
        // The case that caught a wrong model. Held upright, the screen's up
        // axis IS the gravity axis; an implementation that cancelled the
        // along-gravity component swallowed every up and down flick.
        let (mut r, mut t) = (RelativeFlicks::default(), 0.0);
        settle(&mut r, UPRIGHT, &mut t);
        assert_eq!(jerk(&mut r, UPRIGHT, (0.0, 6.0, 0.0), 3, &mut t),
                   Some(RelFlick::Up));
    }

    #[test]
    fn a_flick_in_progress_is_visible_so_the_ratchet_can_stand_down() {
        // One motion must not fire both a relative flick and an orientation
        // step.
        let (mut r, mut t) = (RelativeFlicks::default(), 0.0);
        settle(&mut r, UPRIGHT, &mut t);
        t += 16.0;
        r.feed(Sample { x: 6.0, y: -9.81, z: 0.0, t_ms: t });
        assert!(r.in_flick());
    }

    #[test]
    fn the_gravity_reference_is_exposed_for_a_ui_to_show() {
        let (mut r, mut t) = (RelativeFlicks::default(), 0.0);
        settle(&mut r, UPRIGHT, &mut t);
        let g = r.gravity().expect("settled");
        assert!((g.1 + 9.81).abs() < 0.5, "gravity found -y, got {g:?}");
    }
}
