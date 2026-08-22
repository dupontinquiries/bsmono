//! gait — from a raw accelerometer *sample stream* to step counts and the windowed features the
//! carry-state machine classifies on.
//!
//! This is the layer `monitor-sensor` cannot feed: tilt/orientation change events carry no cadence.
//! The source here is continuous (x, y, z) samples in m/s² — on postmarketOS that is the sysfs
//! `in_accel_*_raw × scale` reader (voiceterm's `iio_accel`, to be promoted to a shared crate), on
//! other platforms a CoreMotion/SensorManager bridge. Same rule as the rest of sensor-kit: the
//! platform source is injected, the math lives here and is tested.
//!
//! Pipeline:  samples → [`StepDetector`] (per sample)
//!            samples → [`Windower`] → [`Features`] (per ~2 s window) → `carry::CarryFsm`
//!
//! The two meet again at the top: the carry state picks the [`StepTuning`] preset, because a phone
//! in a swinging hand sees several times the dynamic amplitude of one damped through a backpack —
//! one fixed threshold undercounts one carry position or double-counts another.

/// One accelerometer sample in m/s², after scale and mount-matrix. Not `touchkit::Vec2` because
/// gravity needs the third axis; when `iio_accel` is promoted its `Vec3` should converge with this.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Sample {
    pub x: f64,
    pub y: f64,
    pub z: f64,
}

impl Sample {
    pub fn mag(self) -> f64 {
        (self.x * self.x + self.y * self.y + self.z * self.z).sqrt()
    }
    fn dot(self, o: Sample) -> f64 {
        self.x * o.x + self.y * o.y + self.z * o.z
    }
    /// Angle between two vectors in degrees (0 if either is ~zero).
    pub fn angle_deg(self, o: Sample) -> f64 {
        let (ma, mb) = (self.mag(), o.mag());
        if ma < 1e-9 || mb < 1e-9 {
            return 0.0;
        }
        (self.dot(o) / (ma * mb)).clamp(-1.0, 1.0).acos().to_degrees()
    }
}

/// Windowed features over ~1–2 s of samples. Everything the carry FSM needs, nothing per-sample.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Features {
    /// Mean acceleration over the window ≈ gravity direction in device axes. Which axis carries
    /// gravity is the main carry cue: +z-ish face-up in a held hand, ±y-dominant vertical in a
    /// trouser pocket, arbitrary-but-stable in a bag.
    pub gravity: Sample,
    /// RMS of the dynamic component (per-sample magnitude minus the window's mean magnitude).
    /// Motion energy: near zero on a table, moderate held, large swinging at the end of an arm.
    pub rms_dyn: f64,
    /// Max angle (degrees) between the mean directions of the window's quarters. A swinging arm
    /// rotates the gravity vector through a big arc each stride; a pocket or bag barely does.
    pub sway_deg: f64,
    /// Dominant cadence from dynamic-magnitude mean crossings, if it lands in a plausible
    /// human band (0.5–4 Hz). Walking is ~1.4–2.5 Hz of vertical impact.
    pub cadence_hz: Option<f64>,
}

/// Fixed-size hopping window: push samples, get [`Features`] every `hop` samples once `size` have
/// been seen. `size` ≈ 2 s and `hop` ≈ 0.5–1 s of samples at your rate.
#[derive(Debug, Clone)]
pub struct Windower {
    buf: Vec<Sample>,
    size: usize,
    hop: usize,
    since_emit: usize,
    sample_hz: f64,
}

impl Windower {
    pub fn new(sample_hz: f64, window_s: f64, hop_s: f64) -> Windower {
        let size = ((sample_hz * window_s) as usize).max(4);
        let hop = ((sample_hz * hop_s) as usize).max(1);
        Windower { buf: Vec::with_capacity(size), size, hop, since_emit: 0, sample_hz }
    }

    pub fn push(&mut self, s: Sample) -> Option<Features> {
        if self.buf.len() == self.size {
            self.buf.remove(0); // size ≤ a few hundred; a VecDeque buys nothing here
        }
        self.buf.push(s);
        self.since_emit += 1;
        if self.buf.len() < self.size || self.since_emit < self.hop {
            return None;
        }
        self.since_emit = 0;
        Some(Features::from_samples(&self.buf, self.sample_hz))
    }
}

impl Features {
    pub fn from_samples(w: &[Sample], sample_hz: f64) -> Features {
        let n = w.len().max(1) as f64;
        let mut g = Sample::default();
        for s in w {
            g.x += s.x;
            g.y += s.y;
            g.z += s.z;
        }
        g.x /= n;
        g.y /= n;
        g.z /= n;

        let mean_mag: f64 = w.iter().map(|s| s.mag()).sum::<f64>() / n;
        let rms_dyn =
            (w.iter().map(|s| (s.mag() - mean_mag).powi(2)).sum::<f64>() / n).sqrt();

        // Sway: max pairwise angle between quarter-mean directions.
        let q = (w.len() / 4).max(1);
        let mut quarter_means: Vec<Sample> = Vec::with_capacity(4);
        for c in w.chunks(q).take(4) {
            let m = c.len() as f64;
            let mut v = Sample::default();
            for s in c {
                v.x += s.x;
                v.y += s.y;
                v.z += s.z;
            }
            v.x /= m;
            v.y /= m;
            v.z /= m;
            quarter_means.push(v);
        }
        let mut sway_deg: f64 = 0.0;
        for i in 0..quarter_means.len() {
            for j in i + 1..quarter_means.len() {
                sway_deg = sway_deg.max(quarter_means[i].angle_deg(quarter_means[j]));
            }
        }

        // Cadence: crossings of the dynamic magnitude through zero, counted on the rising edge.
        // Ignore a dead band around zero so sensor noise doesn't read as sprinting.
        let dead = (0.3f64).max(rms_dyn * 0.2);
        let mut rises = 0u32;
        let mut below = true;
        for s in w {
            let d = s.mag() - mean_mag;
            if below && d > dead {
                rises += 1;
                below = false;
            } else if d < -dead {
                below = true;
            }
        }
        let secs = w.len() as f64 / sample_hz.max(1e-6);
        let hz = rises as f64 / secs;
        let cadence_hz = if rises >= 2 && (0.5..=4.0).contains(&hz) { Some(hz) } else { None };

        Features { gravity: g, rms_dyn, sway_deg, cadence_hz }
    }
}

/// Step-detector thresholds. Presets per carry position are the whole reason the carry FSM exists:
/// the same walk reads ~1 m/s² of dynamic RMS through a backpack and 4–6 m/s² in a swinging hand.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StepTuning {
    /// Dynamic magnitude that arms a step (rising through this).
    pub th_hi: f64,
    /// Falling below this after arming counts the step (hysteresis pair with `th_hi`).
    pub th_lo: f64,
    /// Refractory: two steps closer than this are one impact ringing (s).
    pub min_interval_s: f64,
}

impl Default for StepTuning {
    fn default() -> Self {
        StepTuning { th_hi: 1.2, th_lo: 0.4, min_interval_s: 0.25 }
    }
}

/// Per-sample step counter: EMA gravity magnitude removal + hysteresis peak counting + refractory.
/// Feed every sample; read `steps()`; call `retune` when the carry state switches.
#[derive(Debug, Clone)]
pub struct StepDetector {
    tuning: StepTuning,
    sample_dt: f64,
    /// EMA of |a| ≈ gravity magnitude (≈9.8 at rest); ~2 s time constant.
    grav_mag: f64,
    armed: bool,
    since_step_s: f64,
    steps: u64,
}

impl StepDetector {
    pub fn new(sample_hz: f64, tuning: StepTuning) -> StepDetector {
        StepDetector {
            tuning,
            sample_dt: 1.0 / sample_hz.max(1e-6),
            grav_mag: 9.81,
            armed: false,
            since_step_s: f64::MAX / 2.0,
            steps: 0,
        }
    }

    pub fn retune(&mut self, tuning: StepTuning) {
        self.tuning = tuning;
    }

    pub fn steps(&self) -> u64 {
        self.steps
    }

    /// Returns true when this sample completed a step.
    pub fn feed(&mut self, s: Sample) -> bool {
        let m = s.mag();
        // EMA with ~2 s time constant: alpha = dt / (tau + dt).
        let alpha = self.sample_dt / (2.0 + self.sample_dt);
        self.grav_mag += alpha * (m - self.grav_mag);
        let dyn_m = m - self.grav_mag;
        self.since_step_s += self.sample_dt;

        if !self.armed && dyn_m > self.tuning.th_hi {
            self.armed = true;
        } else if self.armed && dyn_m < self.tuning.th_lo {
            self.armed = false;
            if self.since_step_s >= self.tuning.min_interval_s {
                self.since_step_s = 0.0;
                self.steps += 1;
                return true;
            }
        }
        false
    }
}

/// Synthesize `secs` of walking at `cadence` Hz: gravity on `axis` plus a sinusoidal vertical
/// impact of `amp` m/s², sampled at `hz`. Test scaffolding shared with `carry`'s tests.
#[cfg(test)]
pub(crate) fn walk(hz: f64, secs: f64, cadence: f64, amp: f64, axis: usize) -> Vec<Sample> {
    let n = (hz * secs) as usize;
    (0..n)
        .map(|i| {
            let t = i as f64 / hz;
            let g = 9.81 + amp * (2.0 * std::f64::consts::PI * cadence * t).sin();
            match axis {
                0 => Sample { x: g, y: 0.0, z: 0.0 },
                1 => Sample { x: 0.0, y: g, z: 0.0 },
                _ => Sample { x: 0.0, y: 0.0, z: g },
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_a_two_hz_walk_within_ten_percent() {
        let hz = 50.0;
        let mut d = StepDetector::new(hz, StepTuning::default());
        for s in walk(hz, 30.0, 2.0, 3.0, 1) {
            d.feed(s);
        }
        let expect = 60.0; // 2 Hz × 30 s
        let got = d.steps() as f64;
        assert!((got - expect).abs() <= 6.0, "expected ~{expect}, got {got}");
    }

    #[test]
    fn stillness_counts_nothing() {
        let hz = 50.0;
        let mut d = StepDetector::new(hz, StepTuning::default());
        for i in 0..1500 {
            // 0.1 m/s² of sensor noise around gravity
            let n = 0.1 * ((i as f64 * 0.7).sin());
            d.feed(Sample { x: 0.0, y: 0.0, z: 9.81 + n });
        }
        assert_eq!(d.steps(), 0);
    }

    #[test]
    fn refractory_ignores_impact_ringing() {
        // A 6 Hz oscillation (heel-strike ringing) must not count as 6 steps/s.
        let hz = 50.0;
        let mut d = StepDetector::new(hz, StepTuning::default());
        for s in walk(hz, 10.0, 6.0, 3.0, 2) {
            d.feed(s);
        }
        // min_interval 0.25 s caps at 4 events/s; real cap lower because of hysteresis timing.
        assert!(d.steps() <= 40, "refractory failed: {} steps in 10 s", d.steps());
    }

    #[test]
    fn features_read_a_pocket_walk_as_y_gravity_with_cadence() {
        let hz = 50.0;
        let w = walk(hz, 2.0, 2.0, 3.0, 1);
        let f = Features::from_samples(&w, hz);
        assert!(f.gravity.y.abs() > 8.0, "gravity on y: {:?}", f.gravity);
        assert!(f.rms_dyn > 1.5, "energetic: {}", f.rms_dyn);
        let c = f.cadence_hz.expect("cadence detected");
        assert!((c - 2.0).abs() < 0.5, "≈2 Hz, got {c}");
        assert!(f.sway_deg < 10.0, "no arc in a pocket: {}", f.sway_deg);
    }

    #[test]
    fn features_read_an_arm_swing_as_sway() {
        // Gravity direction rotates through ±40° in the x/z plane at stride rate.
        let hz = 50.0;
        let n = (hz * 2.0) as usize;
        let w: Vec<Sample> = (0..n)
            .map(|i| {
                let t = i as f64 / hz;
                let th = 0.7 * (2.0 * std::f64::consts::PI * 1.0 * t).sin(); // ±40°
                let g = 9.81 + 4.0 * (2.0 * std::f64::consts::PI * 2.0 * t).sin();
                Sample { x: g * th.sin(), y: 0.0, z: g * th.cos() }
            })
            .collect();
        let f = Features::from_samples(&w, hz);
        assert!(f.sway_deg > 30.0, "big arc: {}", f.sway_deg);
        assert!(f.rms_dyn > 2.0, "energetic: {}", f.rms_dyn);
    }
}
