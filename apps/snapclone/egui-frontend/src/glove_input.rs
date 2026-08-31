//! glove_input — what was missing between the sensor and glove mode.
//!
//! Snapclone has had the whole of cold-weather glove mode for some time:
//! a `GloveCursor` on the shell, `ShakeCommands` configured with a
//! camera-swap binding, and `apply_glove`/`apply_shake` written and
//! tested. All three sat on the dead-API list, because nothing produced
//! the input they consume — there was no accelerometer anywhere in this
//! app. The gesture layer was complete and had no way to be reached.
//!
//! ## Reading the sensor
//!
//! `/sys/bus/iio/devices/*/in_accel_{x,y,z}_raw`, with the scale applied.
//! Not iio-sensor-proxy's D-Bus interface, which the kit already parses:
//! that daemon is absent on plenty of phone distributions, and requiring
//! it means the accessibility mode is off precisely on the minimal builds
//! this fleet targets.
//!
//! ## Behind a trait, so the gesture is testable
//!
//! [`Accel`] is the seam. A test drives a scripted sequence of readings
//! and asserts the flicks that come out; the real implementation reads
//! files. Without it the only possible test is "does it compile", which
//! is what the three dead methods already had.
//!
//! ## Polled, not per frame
//!
//! [`POLL_MS`]. An accelerometer read is three file opens, and doing that
//! at 60Hz is a syscall storm for a value that changes at the speed of a
//! wrist. The ratchet is fed the ELAPSED time so its thresholds stay in
//! milliseconds regardless of the poll rate.

use std::path::{Path, PathBuf};

use uikit_egui::glove::{from_accel, Orient, Tilt};

/// How often to read the sensor.
///
/// 50ms is twenty times a second: fast enough that a flick out and back
/// cannot slip between two samples, slow enough to cost nothing.
pub const POLL_MS: f32 = 50.0;

/// A source of accelerometer readings, in g.
pub trait Accel {
    /// `None` when the sensor is unreadable this poll. Distinct from a
    /// zero reading, which is a real measurement — free fall, or a
    /// horizontal device with a miscalibrated axis.
    fn read(&mut self) -> Option<(f32, f32, f32)>;
}

/// The Linux IIO accelerometer, read from sysfs.
#[derive(Debug, Clone)]
pub struct SysfsAccel {
    dir: PathBuf,
    scale: f32,
}

impl SysfsAccel {
    /// Find the first IIO device that looks like an accelerometer.
    ///
    /// Located ONCE. Walking `/sys/bus/iio/devices` on every poll is a
    /// directory scan twenty times a second for a device that does not
    /// move between sockets.
    pub fn discover() -> Option<SysfsAccel> {
        let root = Path::new("/sys/bus/iio/devices");
        let rd = std::fs::read_dir(root).ok()?;
        for e in rd.flatten() {
            let dir = e.path();
            if !dir.join("in_accel_x_raw").is_file() {
                continue;
            }
            // The scale converts raw counts to m/s². Absent on some
            // drivers, where the raw value is already usable — 1.0 is the
            // right fallback rather than refusing the device.
            let scale = std::fs::read_to_string(dir.join("in_accel_scale"))
                .ok()
                .and_then(|s| s.trim().parse::<f32>().ok())
                .unwrap_or(1.0);
            return Some(SysfsAccel { dir, scale });
        }
        None
    }

    fn axis(&self, name: &str) -> Option<f32> {
        let raw: f32 = std::fs::read_to_string(self.dir.join(name))
            .ok()?.trim().parse().ok()?;
        // To g, because that is what `from_accel` documents. Dividing by
        // 9.80665 here rather than in the kit keeps the kit's units stated
        // in one place.
        Some(raw * self.scale / 9.806_65)
    }
}

impl Accel for SysfsAccel {
    fn read(&mut self) -> Option<(f32, f32, f32)> {
        Some((self.axis("in_accel_x_raw")?,
              self.axis("in_accel_y_raw")?,
              self.axis("in_accel_z_raw")?))
    }
}

/// The sensor half of glove mode: readings, at a sane rate.
///
/// It owns NO ratchet. My first version had one, and it was the mistake
/// this module's own comment warns about: `GloveCursor::feed` already
/// contains a ratchet, so a second one here would be a second opinion
/// about what a flick is — the cursor would step on its ratchet's
/// verdict and the command layer on mine, and the two would disagree the
/// moment their timing drifted. This produces readings; the cursor
/// decides what they mean.
pub struct GloveInput {
    since_poll: f32,
    /// The last reading, so a caller can show what the sensor says without
    /// reading it again.
    pub last: Option<(Orient, Tilt)>,
    /// Off until a sensor is found. A device with no accelerometer must
    /// not pay for any of this, and must not show glove mode as available.
    pub enabled: bool,
}

impl Default for GloveInput {
    fn default() -> Self {
        GloveInput { since_poll: 0.0, last: None, enabled: false }
    }
}

impl GloveInput {
    pub fn new() -> GloveInput { GloveInput::default() }

    /// Advance by a frame. Returns a reading on the frames it polls.
    ///
    /// The ELAPSED time comes back with it, because the consumer's
    /// thresholds are in milliseconds: handing it the frame's dt when the
    /// sensor is read every fourth frame makes a gesture mean different
    /// things at different frame rates, which is how one tuned on a
    /// desktop becomes unusable on a phone.
    pub fn tick(&mut self, dt_ms: f32, src: &mut dyn Accel)
        -> Option<(Orient, Tilt, f32)> {
        if !self.enabled {
            return None;
        }
        self.since_poll += dt_ms.max(0.0);
        if self.since_poll < POLL_MS {
            return None;
        }
        let elapsed = self.since_poll;
        self.since_poll = 0.0;
        let Some((x, y, z)) = src.read() else {
            // An unreadable sensor is not a reading of "unknown": feeding
            // one would re-arm the ratchet on a device that has simply
            // been unplugged. The gesture holds its state instead.
            return None;
        };
        let (o, t) = from_accel(x, y, z);
        self.last = Some((o, t));
        Some((o, t, elapsed))
    }

    /// Turn the driver on if a sensor exists.
    pub fn enable_if_present(&mut self, present: bool) {
        self.enabled = present;
        if !present {
            self.last = None;
        }
    }

}

#[cfg(test)]
mod tests {
    use super::*;

    /// A scripted sensor: one reading per poll, repeating the last.
    struct Script {
        frames: Vec<Option<(f32, f32, f32)>>,
        i: usize,
    }

    impl Script {
        fn of(v: Vec<Option<(f32, f32, f32)>>) -> Script { Script { frames: v, i: 0 } }
    }

    impl Accel for Script {
        fn read(&mut self) -> Option<(f32, f32, f32)> {
            let v = self.frames.get(self.i).copied()
                .unwrap_or_else(|| self.frames.last().copied().flatten().map(Some)
                                .unwrap_or(None));
            self.i += 1;
            v
        }
    }

    const FLAT: (f32, f32, f32) = (0.0, 0.0, 1.0);
    const LEFT_UP: (f32, f32, f32) = (1.0, 0.0, 0.1);

    /// Readings out of the driver, fed to a ratchet the way the shell
    /// feeds them to the cursor's — so the test exercises the same path
    /// rather than a private copy of the rules.
    fn run(input: &mut GloveInput, src: &mut Script, polls: usize) -> Vec<Orient> {
        let mut r = uikit_egui::glove::Ratchet::default();
        let mut out = Vec::new();
        for _ in 0..polls {
            if let Some((o, t, elapsed)) = input.tick(POLL_MS, src) {
                if let Some(f) = r.feed(o, t, elapsed) { out.push(f); }
            }
        }
        out
    }

    #[test]
    fn a_flick_out_and_back_produces_exactly_one_step() {
        // The ratchet's whole contract, and until now nothing in this app
        // could exercise it: there was no accelerometer anywhere.
        let mut g = GloveInput::new();
        g.enable_if_present(true);
        // Four polls of rest first: the ratchet arms only after
        // REARM_MIN_MS (120ms) of it, and at a 50ms poll two samples is
        // not enough. My first version rested for two and got no flick —
        // the ratchet being right, not the driver being wrong.
        let mut s = Script::of(vec![
            Some(FLAT), Some(FLAT), Some(FLAT), Some(FLAT),  // at rest, arms
            Some(LEFT_UP), Some(LEFT_UP),                    // flicked out
            Some(FLAT), Some(FLAT),                          // and back
        ]);
        let flicks = run(&mut g, &mut s, 8);
        assert_eq!(flicks, vec![Orient::LeftUp], "{flicks:?}");
    }

    #[test]
    fn holding_the_phone_over_does_not_repeat_the_step() {
        // A held tilt is one step, not one per poll — the difference
        // between moving the cursor and watching it run away.
        let mut g = GloveInput::new();
        g.enable_if_present(true);
        let mut s = Script::of(vec![Some(FLAT), Some(FLAT), Some(FLAT), Some(FLAT),
                                    Some(LEFT_UP)]);
        let flicks = run(&mut g, &mut s, 20);
        assert!(flicks.len() <= 1, "a held tilt stepped {} times", flicks.len());
    }

    #[test]
    fn the_sensor_is_polled_at_its_own_rate_not_every_frame() {
        // Three file opens per read; at 60Hz that is a syscall storm for a
        // value that changes at the speed of a wrist.
        let mut g = GloveInput::new();
        g.enable_if_present(true);
        let mut s = Script::of(vec![Some(FLAT)]);
        for _ in 0..10 { let _ = g.tick(1.0, &mut s); }
        assert!(s.i <= 1, "polled {} times in 10ms", s.i);
    }

    #[test]
    fn an_unreadable_sensor_holds_the_gesture_rather_than_re_arming_it() {
        // Feeding "unknown" for a missing read would re-arm the ratchet on
        // a device that has simply been unplugged.
        let mut g = GloveInput::new();
        g.enable_if_present(true);
        let mut s = Script::of(vec![Some(FLAT), Some(FLAT), Some(FLAT), Some(FLAT),
                                    Some(LEFT_UP), None, None]);
        let flicks = run(&mut g, &mut s, 7);
        assert!(flicks.is_empty(), "an absent reading produced {flicks:?}");
    }

    #[test]
    fn a_device_with_no_sensor_pays_nothing_and_reads_nothing() {
        let mut g = GloveInput::new();
        g.enable_if_present(false);
        let mut s = Script::of(vec![Some(FLAT), Some(LEFT_UP), Some(FLAT)]);
        assert!(run(&mut g, &mut s, 10).is_empty());
        assert_eq!(s.i, 0, "the sensor was read with glove mode off");
        assert_eq!(g.last, None);
    }
}
