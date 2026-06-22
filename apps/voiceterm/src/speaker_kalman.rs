//! speaker_kalman: the pure estimation core for speaker isolation
//! (F8 stage 4, see docs/design/multi_mic_pipeline.md). When a phase
//! array can localize sources, beamform-steering needs a STABLE
//! estimate of each speaker's time-delay-of-arrival (TDOA) across the
//! mics — but raw per-tick TDOA estimates are noisy and would make the
//! steering jitter. A Kalman filter smooths them: it fuses noisy
//! observations into a confident estimate that converges, tracks
//! motion, and never diverges.
//!
//! PURE + dependency-free. This is the estimator math only — the
//! actual cross-correlation that PRODUCES a TDOA observation, and the
//! beamformer that CONSUMES the smoothed estimate, are DSP/device work
//! (the design's "tuning needs real arrays"). Like the cinematic
//! camera's damped spring, the value here is a robust, tested
//! controller core that the device layer drives.
//!
//! Model: a scalar Kalman filter with a constant-position state and a
//! growing-uncertainty predict (random-walk). Good for a slowly-
//! varying quantity like a seated speaker's TDOA. (A constant-velocity
//! 2-state variant is a future extension; the design only requires the
//! predict/update core be pure + stable.)

/// A scalar Kalman filter tracking one slowly-varying quantity.
#[derive(Clone, Copy, Debug)]
pub struct ScalarKalman {
    /// Current state estimate.
    x: f32,
    /// Estimate covariance (uncertainty). Always > 0.
    p: f32,
    /// Process noise: how much the true value may drift per step.
    /// Larger → the filter trusts new observations more (tracks faster
    /// but is noisier).
    q: f32,
    /// Measurement noise: how noisy each observation is. Larger → the
    /// filter trusts observations less (smoother but laggier).
    r: f32,
    /// Whether a first observation has seeded the state.
    seeded: bool,
}

impl ScalarKalman {
    /// `q` = process noise, `r` = measurement noise. Tune per array
    /// geometry on device; sane defaults via `for_tdoa`.
    pub fn new(q: f32, r: f32) -> Self {
        Self { x: 0.0, p: 1.0, q: q.max(0.0), r: r.max(1e-6), seeded: false }
    }

    /// Reasonable starting noise for TDOA-in-samples tracking of a
    /// seated speaker: small process noise (they don't move fast),
    /// moderate measurement noise (cross-correlation is noisy).
    pub fn for_tdoa() -> Self {
        Self::new(0.01, 0.5)
    }

    /// Current best estimate.
    pub fn estimate(&self) -> f32 { self.x }
    /// Current uncertainty (covariance). Shrinks as the filter gains
    /// confidence; useful as a confidence gate (high p → don't steer).
    pub fn uncertainty(&self) -> f32 { self.p }

    /// Predict step: the state is unchanged (random-walk) but
    /// uncertainty grows by the process noise.
    fn predict(&mut self) {
        self.p += self.q;
    }

    /// Fuse one observation `z`, returning the updated estimate. The
    /// FIRST observation seeds the state directly (so the filter starts
    /// near the truth instead of crawling up from 0).
    pub fn update(&mut self, z: f32) -> f32 {
        // v0.1.527: a non-finite observation (NaN/inf) would poison the
        // state permanently — and TDOA from cross-correlation CAN be
        // non-finite on a silent/degenerate frame. Treat it as a
        // missing observation (coast) rather than corrupting the
        // estimate. (Found by adversarial probe.)
        if !z.is_finite() {
            return self.coast();
        }
        if !self.seeded {
            self.x = z;
            self.seeded = true;
            // keep p moderate so subsequent obs still move it
            return self.x;
        }
        self.predict();
        // Kalman gain in [0,1].
        let k = self.p / (self.p + self.r);
        self.x += k * (z - self.x);
        self.p = (1.0 - k) * self.p;
        self.x
    }

    /// Predict-only step when an observation is missing this tick
    /// (e.g. the speaker was silent — VAD gated it out). Uncertainty
    /// grows; estimate holds. Returns the held estimate.
    pub fn coast(&mut self) -> f32 {
        if self.seeded { self.predict(); }
        self.x
    }

    /// Kalman gain that WOULD be applied to the next observation (for
    /// tests / introspection). In [0,1].
    pub fn next_gain(&self) -> f32 {
        let p = self.p + self.q;
        p / (p + self.r)
    }
}

/// A bank of per-speaker TDOA trackers (speaker isolation tracks each
/// active source independently). Keyed by a speaker/source index.
#[derive(Default)]
pub struct SpeakerTdoaBank {
    trackers: Vec<ScalarKalman>,
}

impl SpeakerTdoaBank {
    pub fn new() -> Self { Self { trackers: Vec::new() } }

    /// Ensure a tracker exists for `speaker` (grows the bank as new
    /// speakers appear). Returns a mutable handle.
    fn ensure(&mut self, speaker: usize) -> &mut ScalarKalman {
        while self.trackers.len() <= speaker {
            self.trackers.push(ScalarKalman::for_tdoa());
        }
        &mut self.trackers[speaker]
    }

    /// Feed a TDOA observation for a speaker; returns the smoothed TDOA.
    pub fn observe(&mut self, speaker: usize, tdoa: f32) -> f32 {
        self.ensure(speaker).update(tdoa)
    }

    /// Coast a speaker (no observation this tick — silent/VAD-gated).
    pub fn coast(&mut self, speaker: usize) -> f32 {
        self.ensure(speaker).coast()
    }

    /// Current smoothed TDOA for a speaker, if tracked.
    pub fn estimate(&self, speaker: usize) -> Option<f32> {
        self.trackers.get(speaker).map(|k| k.estimate())
    }

    /// Confidence for a speaker (lower uncertainty = higher
    /// confidence). Caller can gate beamform steering on this.
    pub fn uncertainty(&self, speaker: usize) -> Option<f32> {
        self.trackers.get(speaker).map(|k| k.uncertainty())
    }

    pub fn len(&self) -> usize { self.trackers.len() }
    pub fn is_empty(&self) -> bool { self.trackers.is_empty() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn non_finite_observation_does_not_poison() {
        // v0.1.527 (adversarial probe): a NaN/inf obs must be ignored
        // (coast), not corrupt the estimate. TDOA from cross-correlation
        // can be non-finite on a degenerate frame.
        let mut k = ScalarKalman::for_tdoa();
        for _ in 0..10 { k.update(4.0); }
        let good = k.estimate();
        let after_nan = k.update(f32::NAN);
        assert!(after_nan.is_finite(), "NaN obs must not poison estimate");
        assert!((after_nan - good).abs() < 0.5, "NaN treated as missing → estimate held");
        let after_inf = k.update(f32::INFINITY);
        assert!(after_inf.is_finite(), "inf obs must not poison estimate");
        // filter recovers and keeps tracking real obs afterward
        for _ in 0..5 { k.update(4.0); }
        assert!((k.estimate() - 4.0).abs() < 0.5);
    }

    #[test]
    fn extreme_values_and_zero_noise_stay_finite() {
        // zero process+meas noise (r clamped internally) must not NaN
        let mut k = ScalarKalman::new(0.0, 0.0);
        k.update(5.0);
        assert!(k.update(7.0).is_finite());
        // huge observations
        let mut k2 = ScalarKalman::new(0.01, 0.5);
        k2.update(1e30);
        assert!(k2.update(1e30).is_finite());
        // 10k alternating extremes never diverge
        let mut k3 = ScalarKalman::new(0.1, 0.3);
        for i in 0..10000 {
            let z = if i % 2 == 0 { 1000.0 } else { -1000.0 };
            assert!(k3.update(z).is_finite() && k3.uncertainty() > 0.0);
        }
    }

    #[test]
    fn first_observation_seeds_state() {
        let mut k = ScalarKalman::for_tdoa();
        let e = k.update(12.0);
        assert_eq!(e, 12.0, "first obs seeds directly, no crawl from 0");
        assert_eq!(k.estimate(), 12.0);
    }

    #[test]
    fn converges_to_constant_truth() {
        let mut k = ScalarKalman::new(0.001, 0.1);
        let obs = [5.3, 4.7, 5.2, 4.8, 5.1, 4.9, 5.05, 4.95, 5.0, 5.0];
        let mut last = 0.0;
        for &z in &obs { last = k.update(z); }
        assert!((last - 5.0).abs() < 0.2, "converges near 5.0, got {}", last);
    }

    #[test]
    fn uncertainty_shrinks_and_stays_positive() {
        let mut k = ScalarKalman::new(0.001, 0.1);
        k.update(5.0); // seed
        let p0 = k.uncertainty();
        for _ in 0..20 { k.update(5.0); }
        let p1 = k.uncertainty();
        assert!(p1 < p0, "uncertainty shrinks as confidence grows");
        assert!(p1 > 0.0, "covariance never hits/below zero");
    }

    #[test]
    fn gain_always_in_unit_interval() {
        let mut k = ScalarKalman::new(0.05, 0.3);
        k.update(1.0);
        for i in 0..500 {
            let g = k.next_gain();
            assert!(g >= 0.0 && g <= 1.0, "gain in [0,1], got {}", g);
            k.update((i as f32 * 0.05).sin());
        }
    }

    #[test]
    fn never_diverges_on_moving_target() {
        let mut k = ScalarKalman::for_tdoa();
        for i in 0..2000 {
            let z = 10.0 + (i as f32 * 0.05).sin() * 3.0;
            let e = k.update(z);
            assert!(e.is_finite(), "estimate stays finite");
            assert!(k.uncertainty() > 0.0 && k.uncertainty().is_finite());
        }
    }

    #[test]
    fn coast_holds_estimate_grows_uncertainty() {
        let mut k = ScalarKalman::new(0.01, 0.1);
        for _ in 0..10 { k.update(7.0); }
        let est = k.estimate();
        let p_before = k.uncertainty();
        let held = k.coast();
        assert_eq!(held, est, "estimate held while coasting");
        assert!(k.uncertainty() > p_before, "uncertainty grows when coasting (no obs)");
    }

    #[test]
    fn smoothing_reduces_jitter() {
        // raw obs jitter ±1 around 8.0; smoothed output should vary far
        // less once converged.
        let mut k = ScalarKalman::new(0.001, 0.5);
        let raw = [8.0, 9.0, 7.0, 9.0, 7.0, 8.5, 7.5, 8.2, 7.8, 8.1, 7.9, 8.05];
        let mut smoothed = Vec::new();
        for &z in &raw { smoothed.push(k.update(z)); }
        // late-window spread of smoothed << spread of raw
        let tail = &smoothed[6..];
        let smax = tail.iter().cloned().fold(f32::MIN, f32::max);
        let smin = tail.iter().cloned().fold(f32::MAX, f32::min);
        assert!(smax - smin < 0.6, "smoothed jitter small, got spread {}", smax - smin);
    }

    #[test]
    fn bank_tracks_multiple_speakers_independently() {
        let mut bank = SpeakerTdoaBank::new();
        // speaker 0 around +3 samples, speaker 1 around -5 samples
        for _ in 0..15 {
            bank.observe(0, 3.0);
            bank.observe(1, -5.0);
        }
        assert!((bank.estimate(0).unwrap() - 3.0).abs() < 0.5);
        assert!((bank.estimate(1).unwrap() - (-5.0)).abs() < 0.5);
        assert_eq!(bank.len(), 2);
    }

    #[test]
    fn bank_grows_for_new_speakers() {
        let mut bank = SpeakerTdoaBank::new();
        assert!(bank.is_empty());
        bank.observe(0, 1.0);
        bank.observe(2, 9.0); // skips index 1 — bank grows to fit
        assert_eq!(bank.len(), 3);
        assert!(bank.estimate(1).is_some(), "intermediate tracker created");
    }

    #[test]
    fn bank_coast_for_silent_speaker() {
        let mut bank = SpeakerTdoaBank::new();
        for _ in 0..10 { bank.observe(0, 4.0); }
        let before = bank.uncertainty(0).unwrap();
        bank.coast(0); // speaker went silent this tick
        assert!(bank.uncertainty(0).unwrap() > before);
        assert!((bank.estimate(0).unwrap() - 4.0).abs() < 0.5, "estimate held");
    }
}
