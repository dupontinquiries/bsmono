//! SWING CAPTURE — a phone held as the prop, not pointed at the player.
//!
//! The authoring half of the mocap idea, and the half that needs no camera, no
//! model, no lighting and no network. You hold the phone and swing it; the IMU
//! reports the swing directly, because a weapon arc IS a rotation and rotation
//! is the one thing an IMU measures well.
//!
//! ## What an IMU can and cannot give you, stated up front
//!
//! **Orientation: yes.** Integrating angular rate accumulates slow drift, but
//! a swing lasts a few hundred milliseconds and drift over that window is far
//! below the noise a human arm contributes anyway.
//!
//! **Position: NO, and this is the constraint the whole design turns on.**
//! Recovering position means integrating acceleration twice, so a constant bias
//! `b` becomes a displacement error of `½bt²` — quadratic, unbounded, and
//! large within a second at any realistic bias. Anything here that returned a
//! path through space would be returning noise with a plausible shape. So a
//! captured swing is an ORIENTATION trajectory and an assumed radius, which is
//! also the honest physical model: a blade sweeps about the wrist and shoulder,
//! and the hand travels far less than the tip.
//!
//! That constraint is a feature for authoring. What a combo needs is the SHAPE
//! of the arc — its plane, its extent, how fast it whips and where in the swing
//! the speed peaks. All of those are rotational.
//!
//! ## Where the samples come from (not this module's job)
//!
//! `apps/voiceterm/egui-frontend/src/iio_accel.rs` establishes the pattern on
//! this hardware: read the same sysfs `iio-sensor-proxy` reads, and treat
//! `EBUSY` as "the daemon owns the device in buffered mode" rather than as a
//! retryable error. Gyro is the same mechanism on `in_anglvel_*_raw`.
//!
//! **UNVERIFIED, and it gates the whole feature:** whether a Fairphone under
//! postmarketOS exposes a gyroscope as an IIO device at all, and at what rate
//! sysfs polling actually sustains. `sensor-kit` reads `monitor-sensor`, which
//! is COARSE tilt events, not raw rate — so nothing in this tree has yet read a
//! gyro. This module is written against the sample shape, so it can be tested
//! and tuned entirely on recorded or synthetic data while that is settled, and
//! it will not be silently wrong if the rate turns out lower than hoped.

use glam::{Quat, Vec3};

/// One IMU reading. `accel` in m/s² INCLUDING gravity (that is what the sensor
/// reports, and gravity is useful here — it is the only absolute reference for
/// which way is down). `gyro` in rad/s, body frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ImuSample {
    pub t_s: f32,
    pub accel: Vec3,
    pub gyro: Vec3,
}

/// Angular rate that starts a swing, rad/s. A deliberate swing clears this
/// comfortably; a phone being carried, set down or gestured with does not.
pub const START_RATE: f32 = 6.0;
/// Rate the swing must fall below to end, rad/s. Hysteresis: a single
/// threshold chatters at the boundary and chops one swing into three.
pub const END_RATE: f32 = 2.5;
/// Shortest thing that counts as a swing. Below this it is a knock or a
/// fumble, and admitting those fills a combo library with noise.
pub const MIN_DURATION_S: f32 = 0.08;
/// Longest. Beyond this the phone is being waved, not swung, and the arc has
/// no crisp shape to bake.
pub const MAX_DURATION_S: f32 = 1.50;
/// Total swept angle below which there is no arc worth keeping, radians.
pub const MIN_SWEEP_RAD: f32 = 0.60;
/// A jump between sample timestamps larger than this is DROPPED SAMPLES, not
/// a slow swing, and it ENDS the swing in progress.
///
/// Two things go wrong without this and they pull in opposite directions.
/// Integrating angular rate across the hole fabricates rotation that never
/// happened — silently, since the swing merely comes out bigger. And carrying
/// the segment across the hole makes it longer than `MAX_DURATION_S`, so the
/// whole thing is discarded and real motion on both sides is lost. Closing at
/// the gap keeps what was actually measured and invents nothing.
pub const MAX_GAP_S: f32 = 0.25;

/// The shape of the arc, relative to the swinger's own vertical.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ArcKind {
    /// Rotation about the vertical: the blade sweeps level.
    Horizontal,
    /// Rotation about a horizontal axis: an overhead chop or an uppercut.
    Vertical,
    /// Rotation about the swing's own forward direction: a roll or flourish.
    Roll,
}

/// One captured swing, ready to bake.
#[derive(Clone, Debug, PartialEq)]
pub struct Swing {
    pub start_s: f32,
    pub duration_s: f32,
    /// Mean rotation axis over the swing, unit length, body frame.
    pub axis: Vec3,
    /// Total angle swept about that axis, radians. Always positive; `axis`
    /// carries the direction.
    pub sweep_rad: f32,
    /// Highest instantaneous angular rate, rad/s — what a hit's impact scales
    /// with.
    pub peak_rate: f32,
    /// Where the peak fell, 0..1 through the swing. A committed strike peaks
    /// late; a flick peaks early. This is the number that separates a windup
    /// from a snap, and it is why the whole trajectory is kept rather than
    /// just its endpoints.
    pub peak_at: f32,
    pub kind: ArcKind,
    /// Orientation at each sample, relative to the swing's start. This is the
    /// arc: apply it to a blade of any length to get a path.
    pub frames: Vec<(f32, Quat)>,
}

impl Swing {
    /// Mean angular rate over the swing.
    pub fn mean_rate(&self) -> f32 {
        if self.duration_s <= 0.0 {
            0.0
        } else {
            self.sweep_rad / self.duration_s
        }
    }
    /// Where a blade tip of `radius` metres travelled, in the frame the swing
    /// started in. NOT measured — DERIVED from the rotation and the radius you
    /// choose, because position is the thing an IMU cannot give (see the
    /// module doc). Named so nobody mistakes it for capture.
    pub fn derived_tip_path(&self, radius: f32, rest: Vec3) -> Vec<Vec3> {
        let r = rest.normalize_or(Vec3::NEG_Z) * radius;
        self.frames.iter().map(|(_, q)| *q * r).collect()
    }
    pub fn report(&self) -> String {
        format!(
            "{:?} {:.0}° in {:.0}ms, peak {:.1} rad/s at {:.0}%",
            self.kind,
            self.sweep_rad.to_degrees(),
            self.duration_s * 1000.0,
            self.peak_rate,
            self.peak_at * 100.0
        )
    }
}

/// Which way is down, from the low-motion parts of a recording.
///
/// Gravity is the only absolute reference an IMU has, and it is only visible
/// when the device is not being accelerated hard — during the swing itself the
/// accelerometer is dominated by the swing. So the reference is taken from the
/// QUIET samples, and a recording with no quiet samples honestly has no
/// vertical: the caller gets `None` rather than a confident wrong answer.
pub fn gravity_reference(samples: &[ImuSample]) -> Option<Vec3> {
    const G: f32 = 9.81;
    // within 15% of 1g and barely rotating: the phone is being held, not swung
    let quiet: Vec<Vec3> = samples
        .iter()
        .filter(|s| {
            let a = s.accel.length();
            (a - G).abs() < G * 0.15 && s.gyro.length() < 1.0
        })
        .map(|s| s.accel)
        .collect();
    if quiet.len() < 3 {
        return None;
    }
    let mean: Vec3 = quiet.iter().copied().sum::<Vec3>() / quiet.len() as f32;
    mean.try_normalize()
}

/// Find the swings in a recording.
///
/// Hysteresis on angular-rate magnitude: above `START_RATE` opens a swing,
/// below `END_RATE` closes it. Segments outside the duration or sweep bounds
/// are dropped rather than returned with a flag, because a combo library is
/// the wrong place to be sifting rejects.
pub fn detect(samples: &[ImuSample]) -> Vec<Swing> {
    let up = gravity_reference(samples).map(|g| -g);
    let mut out = Vec::new();
    let mut open: Option<usize> = None;
    for i in 0..samples.len() {
        let rate = samples[i].gyro.length();
        match open {
            None => {
                if rate >= START_RATE {
                    open = Some(i);
                }
            }
            Some(start) => {
                let last = i + 1 == samples.len();
                let gap = i > 0 && (samples[i].t_s - samples[i - 1].t_s) > MAX_GAP_S;
                if rate < END_RATE || last || gap {
                    // a gap closes the swing BEFORE the offending sample, so
                    // the hole is outside every segment rather than inside one
                    let end = if last { samples.len() } else { i };
                    if let Some(s) = build(&samples[start..end], up) {
                        out.push(s);
                    }
                    // and the sample after the gap may itself still be fast:
                    // reopen there rather than waiting for another rising edge
                    open = if gap && rate >= START_RATE { Some(i) } else { None };
                }
            }
        }
    }
    out
}

fn build(seg: &[ImuSample], up: Option<Vec3>) -> Option<Swing> {
    if seg.len() < 3 {
        return None;
    }
    let start_s = seg[0].t_s;
    let duration_s = seg[seg.len() - 1].t_s - start_s;
    if !(MIN_DURATION_S..=MAX_DURATION_S).contains(&duration_s) {
        return None;
    }

    // integrate the gyro into an orientation track, and accumulate the axis
    // weighted by how fast it was turning: the axis during the fast part of a
    // swing is the axis of the swing, and the slow entry and exit wobble
    let mut q = Quat::IDENTITY;
    let mut frames = Vec::with_capacity(seg.len());
    frames.push((0.0f32, q));
    let mut axis_acc = Vec3::ZERO;
    let mut sweep = 0.0f32;
    let mut peak_rate = 0.0f32;
    let mut peak_t = 0.0f32;
    for w in seg.windows(2) {
        let dt = (w[1].t_s - w[0].t_s).max(0.0);
        if dt <= 0.0 {
            continue;
        }
        // NOTE: there is deliberately no large-gap guard here. `detect` closes
        // a swing at `MAX_GAP_S` before the hole is ever handed to this
        // function, so a `dt > MAX_GAP_S` test in this loop is unreachable —
        // and an unreachable test reads like a check that is happening. It was
        // here, at 0.5 s, and removing it broke nothing, which is how it was
        // found. The split in `detect` is the whole defence.
        let g = w[0].gyro;
        let rate = g.length();
        if rate > peak_rate {
            peak_rate = rate;
            peak_t = w[0].t_s - start_s;
        }
        sweep += rate * dt;
        axis_acc += g * dt; // rate-weighted by construction
        if rate > 1e-6 {
            q = (q * Quat::from_axis_angle(g / rate, rate * dt)).normalize();
        }
        frames.push((w[1].t_s - start_s, q));
    }
    if sweep < MIN_SWEEP_RAD {
        return None;
    }
    let axis = axis_acc.try_normalize()?;

    // classify against the swinger's own vertical when it is known. Without a
    // gravity reference there is no vertical, and calling everything
    // Horizontal would be a confident guess — Roll is the honest default,
    // since it is the shape that makes no claim about up.
    let kind = match up {
        Some(u) => {
            let along_up = axis.dot(u).abs();
            if along_up > 0.70 {
                ArcKind::Horizontal
            } else if along_up < 0.40 {
                ArcKind::Vertical
            } else {
                ArcKind::Roll
            }
        }
        None => ArcKind::Roll,
    };

    Some(Swing {
        start_s,
        duration_s,
        axis,
        sweep_rad: sweep,
        peak_rate,
        peak_at: if duration_s > 0.0 { (peak_t / duration_s).clamp(0.0, 1.0) } else { 0.0 },
        kind,
        frames,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const HZ: f32 = 200.0;

    /// A recording: `n` quiet samples, then the swing, then quiet again.
    /// Gravity is -Y throughout, which is what a phone held upright reports.
    fn recording(swing: &[(Vec3, f32)]) -> Vec<ImuSample> {
        let g = Vec3::new(0.0, 9.81, 0.0); // accel reads +Y when gravity is -Y
        let mut out = Vec::new();
        let mut t = 0.0f32;
        for _ in 0..40 {
            out.push(ImuSample { t_s: t, accel: g, gyro: Vec3::ZERO });
            t += 1.0 / HZ;
        }
        for (gyro, secs) in swing {
            let n = (secs * HZ) as usize;
            for _ in 0..n {
                // during a swing the accelerometer is dominated by the swing
                out.push(ImuSample { t_s: t, accel: g + *gyro * 2.0, gyro: *gyro });
                t += 1.0 / HZ;
            }
        }
        for _ in 0..40 {
            out.push(ImuSample { t_s: t, accel: g, gyro: Vec3::ZERO });
            t += 1.0 / HZ;
        }
        out
    }

    #[test]
    fn a_level_sweep_is_horizontal_and_its_angle_is_recovered() {
        // rotate about +Y (the vertical) at 12 rad/s for 0.25 s = 3.0 rad
        let rec = recording(&[(Vec3::new(0.0, 12.0, 0.0), 0.25)]);
        let sw = detect(&rec);
        assert_eq!(sw.len(), 1, "expected one swing, got {}", sw.len());
        let s = &sw[0];
        assert_eq!(s.kind, ArcKind::Horizontal, "{}", s.report());
        assert!(
            (s.sweep_rad - 3.0).abs() < 0.15,
            "swept {:.2} rad, expected ~3.0: {}",
            s.sweep_rad,
            s.report()
        );
        assert!(s.axis.dot(Vec3::Y) > 0.95, "axis {:?} is not vertical", s.axis);
        assert!((s.peak_rate - 12.0).abs() < 0.1);
    }

    #[test]
    fn an_overhead_chop_is_vertical_not_horizontal() {
        // rotation about +X — a horizontal axis — is a chop
        let rec = recording(&[(Vec3::new(14.0, 0.0, 0.0), 0.20)]);
        let sw = detect(&rec);
        assert_eq!(sw.len(), 1);
        assert_eq!(sw[0].kind, ArcKind::Vertical, "{}", sw[0].report());
    }

    #[test]
    fn carrying_the_phone_is_not_a_swing() {
        // The threshold's whole job. Slow gestures, walking, setting it down —
        // none of it may reach the combo library, or the library is noise.
        // SHORT bursts, not a long wobble. A long one is rejected by
        // MAX_DURATION_S whatever the threshold is, so it tests the duration
        // bound rather than the rate bound — the first version of this test
        // did exactly that and survived START_RATE being dropped to 0.5.
        // These are gesture-length and well inside every duration bound, so
        // only the rate threshold can reject them.
        let mut rec = Vec::new();
        let mut t = 0.0f32;
        // The bursts must be long enough that MIN_SWEEP_RAD cannot reject
        // them either — otherwise the sweep bound does the work and the rate
        // bound is untested. The second version of this test still had that
        // problem: 0.2 s bursts swept ~0.25 rad, under the 0.6 floor, so
        // dropping START_RATE to 1.5 still produced nothing. At 0.35 s these
        // sweep ~0.8-1.1 rad and sit inside every duration bound, so ONLY the
        // rate threshold stands between them and the combo library.
        for burst in 0..6 {
            let peak = 3.5 + burst as f32 * 0.3; // 3.5 .. 5.0 rad/s: brisk, not a swing
            for i in 0..70 {
                let w = (i as f32 / 70.0 * std::f32::consts::PI).sin() * peak;
                rec.push(ImuSample {
                    t_s: t,
                    accel: Vec3::new(0.0, 9.81, 0.0) + Vec3::new(w * 0.2, 0.0, 0.0),
                    gyro: Vec3::new(0.0, w, 0.0),
                });
                t += 1.0 / HZ;
            }
            for _ in 0..30 {
                rec.push(ImuSample { t_s: t, accel: Vec3::new(0.0, 9.81, 0.0), gyro: Vec3::ZERO });
                t += 1.0 / HZ;
            }
        }
        assert!(
            detect(&rec).is_empty(),
            "brisk gestures were captured as swings: {:?}",
            detect(&rec).iter().map(|s| s.report()).collect::<Vec<_>>()
        );
    }

    #[test]
    fn two_swings_with_a_pause_are_two_swings_not_one() {
        // Hysteresis has to close a swing and reopen it. A single threshold
        // either merges these or chops each into several.
        let rec = recording(&[
            (Vec3::new(0.0, 12.0, 0.0), 0.20),
            (Vec3::new(0.0, 0.3, 0.0), 0.30), // pause, below END_RATE
            (Vec3::new(0.0, -12.0, 0.0), 0.20), // and back the other way
        ]);
        let sw = detect(&rec);
        assert_eq!(sw.len(), 2, "expected 2 swings, got {}: {:?}",
            sw.len(), sw.iter().map(|s| s.report()).collect::<Vec<_>>());
        // opposite directions: the axes must point opposite ways
        assert!(sw[0].axis.dot(sw[1].axis) < -0.9, "the return stroke kept the same axis");
    }

    #[test]
    fn a_rate_dip_mid_stroke_stays_one_swing() {
        // WHAT HYSTERESIS IS FOR, and the case constant-rate fixtures never
        // reach: a real arm loads, eases, then accelerates, so the rate dips
        // between END_RATE and START_RATE partway through. With two
        // thresholds that is one stroke; with one it chops into two, and the
        // combo library fills with halves. The first version of this module's
        // tests all used constant rates, so collapsing END_RATE onto
        // START_RATE broke nothing.
        let rec = recording(&[
            (Vec3::new(0.0, 11.0, 0.0), 0.10),
            (Vec3::new(0.0, 4.0, 0.0), 0.08), // dips into the hysteresis band
            (Vec3::new(0.0, 13.0, 0.0), 0.12),
        ]);
        let sw = detect(&rec);
        assert_eq!(
            sw.len(),
            1,
            "a single stroke was split into {}: {:?}",
            sw.len(),
            sw.iter().map(|s| s.report()).collect::<Vec<_>>()
        );
        assert!(sw[0].duration_s > 0.25, "the stroke was truncated: {}", sw[0].report());
    }

    #[test]
    fn peak_at_separates_a_snap_from_a_committed_strike() {
        // Assert a DIRECTION between two shapes, not a value: a metric that
        // has stopped discriminating satisfies any single-sided check.
        let early = recording(&[
            (Vec3::new(0.0, 16.0, 0.0), 0.06), // fast first
            (Vec3::new(0.0, 7.0, 0.0), 0.22),
        ]);
        let late = recording(&[
            (Vec3::new(0.0, 7.0, 0.0), 0.22),
            (Vec3::new(0.0, 16.0, 0.0), 0.06), // fast last
        ]);
        let a = &detect(&early)[0];
        let b = &detect(&late)[0];
        assert!(
            a.peak_at < 0.4 && b.peak_at > 0.6,
            "peak_at did not separate the shapes: snap {:.2}, strike {:.2}",
            a.peak_at,
            b.peak_at
        );
    }

    #[test]
    fn with_no_quiet_samples_there_is_no_vertical_and_no_confident_kind() {
        // The honesty case. Gravity is only visible when the phone is not
        // being accelerated; a recording that is all swing has no absolute
        // reference, and guessing "Horizontal" would be a confident lie.
        let mut rec = Vec::new();
        let mut t = 0.0f32;
        for _ in 0..200 {
            rec.push(ImuSample {
                t_s: t,
                accel: Vec3::new(30.0, 30.0, 0.0), // hard acceleration throughout
                gyro: Vec3::new(0.0, 12.0, 0.0),
            });
            t += 1.0 / HZ;
        }
        assert!(gravity_reference(&rec).is_none(), "invented a vertical");
        let sw = detect(&rec);
        assert_eq!(sw.len(), 1);
        assert_eq!(sw[0].kind, ArcKind::Roll, "guessed an orientation with no reference");
    }

    #[test]
    fn a_dropped_sample_run_does_not_invent_rotation() {
        // A phone will drop samples. Integrating rate across a half-second
        // gap fabricates a large rotation that never happened, and it fails
        // silently — the swing simply comes out bigger.
        let mut rec = recording(&[(Vec3::new(0.0, 12.0, 0.0), 0.25)]);
        // punch a 2 s hole into the middle of the swing by shifting later times
        let mid = rec.len() / 2;
        for s in rec[mid..].iter_mut() {
            s.t_s += 2.0;
        }
        let sw = detect(&rec);
        // the swing is SPLIT at the hole, not merged across it and not thrown
        // away: what was measured on each side is kept
        assert!(!sw.is_empty(), "the gap swallowed the swing entirely");
        let total: f32 = sw.iter().map(|s| s.sweep_rad).sum();
        assert!(
            total < 4.0,
            "a dropped-sample gap manufactured rotation: {:.1} rad total across {} swing(s)",
            total,
            sw.len()
        );
        for s in &sw {
            assert!(s.duration_s <= MAX_DURATION_S, "a segment spans the gap: {}", s.report());
        }
    }

    #[test]
    fn the_tip_path_is_derived_and_says_so() {
        // Position is not captured. This is the one place a path appears, and
        // it must come from the rotation and a CHOSEN radius, not from
        // integrating acceleration.
        let rec = recording(&[(Vec3::new(0.0, 12.0, 0.0), 0.25)]);
        let s = &detect(&rec)[0];
        let path = s.derived_tip_path(1.0, Vec3::NEG_Z);
        assert_eq!(path.len(), s.frames.len());
        // a unit blade on a unit radius stays on the unit sphere
        for p in &path {
            assert!((p.length() - 1.0).abs() < 1e-3, "tip left the sphere: {:?}", p);
        }
        // and it actually moved
        assert!(
            (path[path.len() - 1] - path[0]).length() > 0.5,
            "the derived path did not sweep"
        );
    }
}
