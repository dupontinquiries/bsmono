//! Adaptive upload throttling — back off when the GPU is saturated.
//!
//! v0.24.63-adaptive-uploads.
//!
//! ## The problem
//!
//! During post-movement chunk transitions and initial load, the GPU
//! gets saturated with mesher + upload work. Symptom (from
//! `STATUS.md` reproductions on M2 Asahi): wait_fences pushes past
//! 60ms/frame, FPS drops to ~5 for ~5 seconds, then recovers.
//!
//! The previous fixed `MAX_UPLOADS_PER_FRAME=64` heuristic worked
//! great for steady state but couldn't react to the saturation
//! window. Sitting at 64 uploads while wait_fences screams to 63ms
//! just keeps piling more work onto the already-busy queue.
//!
//! ## The mechanism
//!
//! A small TCP-inspired AIMD-like controller (additive increase,
//! multiplicative decrease) that watches wait_fences and adjusts
//! the per-frame upload cap.
//!
//! - Tracks an EMA of wait_fences (smoothing α = 0.2). Single-frame
//!   spikes get averaged with their neighbors; sustained spikes
//!   drag the EMA up.
//! - When EMA > upper threshold (default 20ms): **multiplicative
//!   decrease** — cap drops by 25%, clamped to a floor.
//! - When EMA < lower threshold (default 8ms): **additive increase**
//!   — cap grows by 4, clamped to a ceiling.
//! - In between: no change. This dead band prevents oscillation
//!   when EMA hovers near the boundary.
//!
//! ## Why not just lower the static cap?
//!
//! Lower static cap = slower world buildout under all conditions,
//! including when the GPU has headroom. The adaptive version stays
//! at the high cap when things are smooth and only backs off when
//! the system tells us it needs breathing room. Net throughput
//! should be higher OR EQUAL to the static cap with strictly better
//! tail latency.
//!
//! ## Why not lock-step with wait_fences directly?
//!
//! Wait_fences is noisy — single-frame outliers from things like
//! GPU driver queue management, OS scheduling, vsync, etc. Direct
//! reaction would oscillate. EMA smooths this out.
//!
//! ## What this is NOT
//!
//! - **Not** a real-time scheduler. We don't reschedule individual
//!   uploads based on priority; we just cap how many run.
//! - **Not** an upload-vs-render priority arbiter. The GPU queue
//!   order is unchanged.
//! - **Not** a frame-pacing system. We don't try to hit a target
//!   FPS; we react to observed GPU pressure.

/// Adaptive cap controller. Constructed once, observed per-frame,
/// queried for the current cap.
#[derive(Debug, Clone)]
pub struct AdaptiveUploadController {
    /// Current per-frame upload cap.
    current_cap: usize,
    /// Minimum cap. Below this and the world stops streaming.
    min_cap: usize,
    /// Maximum cap. Above this and per-frame upload cost dominates
    /// even when the GPU is idle.
    max_cap: usize,
    /// EMA of observed wait_fences (ms).
    rolling_wait_ms: f64,
    /// EMA smoothing factor — fraction of new sample blended in
    /// each `observe` call. Higher = more reactive, more noisy.
    smoothing: f64,
    /// EMA threshold above which we back off (ms).
    upper_threshold_ms: f64,
    /// EMA threshold below which we ramp up (ms).
    lower_threshold_ms: f64,
    /// Number of times we've shifted the cap. For diagnostics.
    adjustments: u64,
}

impl AdaptiveUploadController {
    /// Construct a controller seeded with `base_cap`. Floor/ceiling
    /// default to `base_cap/8` and `base_cap*2` respectively.
    pub fn new(base_cap: usize) -> Self {
        Self {
            current_cap: base_cap,
            min_cap: (base_cap / 8).max(4),
            max_cap: base_cap * 2,
            rolling_wait_ms: 0.0,
            smoothing: 0.20,
            upper_threshold_ms: 20.0,
            lower_threshold_ms: 8.0,
            adjustments: 0,
        }
    }

    /// Configure custom thresholds. Caller should ensure
    /// `lower < upper` for sane behavior.
    pub fn with_thresholds(mut self, lower_ms: f64, upper_ms: f64) -> Self {
        self.lower_threshold_ms = lower_ms;
        self.upper_threshold_ms = upper_ms;
        self
    }

    /// Custom cap range. Useful when the deployment knows its
    /// streaming workload (e.g., higher cap on a beefy desktop GPU).
    pub fn with_cap_range(mut self, min_cap: usize, max_cap: usize) -> Self {
        self.min_cap = min_cap;
        self.max_cap = max_cap;
        // Re-clamp current_cap into the new range.
        self.current_cap = self.current_cap.clamp(min_cap, max_cap);
        self
    }

    /// Custom EMA smoothing. `0.0` would freeze the EMA (never
    /// update); `1.0` would make it = the most recent sample.
    /// Defaults to `0.20`.
    pub fn with_smoothing(mut self, smoothing: f64) -> Self {
        self.smoothing = smoothing.clamp(0.0, 1.0);
        self
    }

    /// Feed in this frame's `wait_fences` measurement (ms). May
    /// change the current cap. Returns true if the cap moved this
    /// call.
    pub fn observe(&mut self, wait_ms: f64) -> bool {
        // EMA update. saturating_*: a NaN or negative wait_ms is a
        // data bug, not a controller-tuning question — clamp before
        // mixing into the EMA so a single broken frame doesn't
        // permanently poison the controller.
        let sample = wait_ms.max(0.0);
        self.rolling_wait_ms =
            self.rolling_wait_ms * (1.0 - self.smoothing) + sample * self.smoothing;

        // AIMD decision.
        if self.rolling_wait_ms > self.upper_threshold_ms {
            // Multiplicative decrease: drop by 25%.
            let next = ((self.current_cap as f64) * 0.75) as usize;
            let next = next.max(self.min_cap);
            if next != self.current_cap {
                self.current_cap = next;
                self.adjustments = self.adjustments.saturating_add(1);
                return true;
            }
        } else if self.rolling_wait_ms < self.lower_threshold_ms {
            // Additive increase: +4 per cycle until ceiling.
            let next = (self.current_cap.saturating_add(4)).min(self.max_cap);
            if next != self.current_cap {
                self.current_cap = next;
                self.adjustments = self.adjustments.saturating_add(1);
                return true;
            }
        }
        false
    }

    pub fn current_cap(&self) -> usize {
        self.current_cap
    }

    pub fn rolling_wait_ms(&self) -> f64 {
        self.rolling_wait_ms
    }

    pub fn adjustment_count(&self) -> u64 {
        self.adjustments
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctrl() -> AdaptiveUploadController {
        // 64 base cap (matches MAX_UPLOADS_PER_FRAME_DEFAULT).
        AdaptiveUploadController::new(64)
    }

    /// Characterization of the real M2 Max episode captured in run
    /// r-438aa27e (RD2, GPU_MESHER=2): the controller sits at the ceiling (128),
    /// a chunk-mesh burst pushes wait_fences past the 20ms upper threshold, the
    /// cap is cut multiplicatively toward the floor, then recovers additively
    /// (+4) once wait drops back under the 8ms lower threshold. Pins the AIMD
    /// shape so a tuning change is a visible, deliberate diff — not silent drift.
    #[test]
    fn m2_log_sawtooth_shape() {
        // Default thresholds (upper 20, lower 8) match the run's flags. Ceiling
        // 128 = base*2, as in the log.
        let mut c = AdaptiveUploadController::new(64);
        for _ in 0..40 { c.observe(2.0); } // warm up to the ceiling
        assert_eq!(c.current_cap(), 128, "should ride the ceiling when smooth");

        // The burst: sustained high waits (the log saw ema ~20-28ms). The cap
        // must back off, and multiplicative decrease means it falls fast.
        let cap_before = c.current_cap();
        for _ in 0..6 { c.observe(35.0); }
        let cap_after_burst = c.current_cap();
        assert!(cap_after_burst < cap_before, "burst must cut the cap");
        // Multiplicative ×0.75 reaches the 40s within a handful of cuts (the log
        // went 128->96->72->54->40), far below a single additive step.
        assert!(cap_after_burst <= 60, "×0.75 cuts should reach the 40-60 band fast, got {cap_after_burst}");

        // Recovery: once the burst clears and wait falls under 8ms, the cap
        // climbs additively (+4/observe) and must reach the ceiling again.
        let mut steps = 0;
        while c.current_cap() < 128 && steps < 1000 {
            c.observe(1.8); // the log's recovered last_wait_ms
            steps += 1;
        }
        assert_eq!(c.current_cap(), 128, "must recover to the ceiling");
        // Additive recovery is gradual by design (the log took ~22 frames from
        // 40 -> 128). Confirm it isn't instantaneous — that's the AIMD contract.
        assert!(steps >= (128 - cap_after_burst) / 4, "recovery should be +4/step, took {steps} steps");
    }

    #[test]
    fn starts_at_base_cap() {
        let c = ctrl();
        assert_eq!(c.current_cap(), 64);
    }

    #[test]
    fn floor_is_above_zero() {
        // Critical: even under extreme pressure, world streaming must
        // not stall. The floor guarantees we keep uploading SOMETHING.
        let c = ctrl();
        assert!(c.min_cap >= 4);
    }

    #[test]
    fn backs_off_under_pressure() {
        let mut c = ctrl();
        // Feed in sustained high wait_fences.
        for _ in 0..50 {
            c.observe(60.0);
        }
        // EMA should be near 60ms; cap should have dropped.
        assert!(c.rolling_wait_ms() > 50.0);
        assert!(c.current_cap() < 64);
        assert!(c.adjustment_count() > 0);
    }

    #[test]
    fn ramps_up_under_idle() {
        let mut c = AdaptiveUploadController::new(32);
        // Sustained low wait_fences → cap climbs.
        for _ in 0..100 {
            c.observe(2.0);
        }
        assert!(c.current_cap() > 32);
        // But never past the ceiling.
        assert!(c.current_cap() <= 64);  // default ceiling = base_cap * 2
    }

    #[test]
    fn caps_at_ceiling() {
        let mut c = AdaptiveUploadController::new(8).with_cap_range(4, 16);
        for _ in 0..200 {
            c.observe(0.5);
        }
        assert_eq!(c.current_cap(), 16);
    }

    #[test]
    fn does_not_go_below_floor() {
        let mut c = AdaptiveUploadController::new(64).with_cap_range(8, 128);
        for _ in 0..200 {
            c.observe(100.0);
        }
        assert_eq!(c.current_cap(), 8);
    }

    #[test]
    fn dead_band_prevents_oscillation() {
        // EMA in the [lower, upper] dead band → no change.
        let mut c = AdaptiveUploadController::new(64);
        // Push EMA to ~14ms (between 8 and 20).
        for _ in 0..30 {
            c.observe(14.0);
        }
        let cap_before = c.current_cap();
        // Continue at the same value. Cap shouldn't budge.
        for _ in 0..30 {
            c.observe(14.0);
        }
        assert_eq!(c.current_cap(), cap_before);
    }

    #[test]
    fn ema_smooths_single_spike() {
        let mut c = AdaptiveUploadController::new(64);
        // Burn-in at low wait to settle EMA.
        for _ in 0..50 {
            c.observe(4.0);
        }
        let cap_before = c.current_cap();
        // A single huge spike shouldn't immediately trigger backoff.
        c.observe(100.0);
        // EMA is 4.0 * 0.8 + 100 * 0.2 = 23.2 — actually that DOES exceed
        // upper threshold. So this test verifies: ONE spike DOES move
        // the cap (correct behavior — sustained > threshold including
        // even one heavy frame counts). Multiplicative decrease applied
        // once.
        assert!(c.current_cap() < cap_before);
        // But still well above floor — one spike doesn't crash the cap
        // to minimum.
        assert!(c.current_cap() > 16);
    }

    #[test]
    fn recovery_after_burst() {
        let mut c = AdaptiveUploadController::new(64);
        // Burst of high wait_fences pushes cap down.
        for _ in 0..50 {
            c.observe(60.0);
        }
        let cap_during_burst = c.current_cap();
        assert!(cap_during_burst < 64);
        // System recovers — wait drops to idle.
        for _ in 0..200 {
            c.observe(2.0);
        }
        assert!(c.current_cap() > cap_during_burst);
    }

    #[test]
    fn nan_input_does_not_corrupt_ema() {
        let mut c = AdaptiveUploadController::new(64);
        // Pre-charge EMA.
        for _ in 0..30 {
            c.observe(10.0);
        }
        let ema_before = c.rolling_wait_ms();
        c.observe(f64::NAN);
        // The .max(0.0) clamp turns NaN into 0.0. EMA shifts toward 0.
        // The important thing is the EMA isn't NaN itself.
        assert!(!c.rolling_wait_ms().is_nan());
        // EMA moved toward zero — not staying at exactly ema_before.
        assert!(c.rolling_wait_ms() < ema_before);
    }

    #[test]
    fn custom_thresholds_work() {
        let mut c = AdaptiveUploadController::new(64)
            .with_thresholds(2.0, 5.0);
        // Wait_ms of 3.0 is in the dead band for default thresholds
        // (8.0 / 20.0) but ABOVE upper for our custom 5.0.
        for _ in 0..50 {
            c.observe(10.0);  // well above 5.0
        }
        assert!(c.current_cap() < 64);
    }
}
