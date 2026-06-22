// bo-controller/src/noise_adaptive_gain.rs
//
// Computes a soundscape master-gain multiplier from the canceller's
// noise observation, so the scene's perceived loudness tracks the
// ambient noise floor without the user manually riding gain.
//
// ── The problem ────────────────────────────────────────────────────
//
// A soundscape that masks a quiet office at gain G also masks a
// noisy street at gain G — and the street will overwhelm it. The
// user solves this by manually turning up the volume when traffic
// starts and back down when it eases. Boring; misses transients;
// distracting at 3 AM.
//
// ── The fix ────────────────────────────────────────────────────────
//
// Take the canceller's already-running estimate of ambient loudness
// (`band_energy / noise_floor`, same number the scene_recommender
// uses for its LOUD trigger) and map it through a piecewise-linear
// curve to a gain multiplier in [0.7, 1.4]. Then send that to
// bo-muxd via `set_scene_gain`.
//
//   ratio (× noise floor)    multiplier
//   ----------------------   ----------
//   ≤  3                     0.70   (quiet — pull soundscape down)
//      5                     0.85
//     10                     1.00   (nominal)
//     20                     1.15
//     30                     1.30
//   ≥ 50                     1.40   (loud — boost masker)
//
// Linear interpolation between breakpoints; clamped at the ends.
//
// ── Why not just track loudness directly? ──────────────────────────
//
// Two reasons. First, paplay/ffplay's `--volume=` is already a
// linear gain — we don't need a dB scale. Second, the perceived
// loudness curve for ambient noise is steep at low levels but
// flattens at high levels (Stevens' power law). The piecewise-
// linear approximation matches this — small changes near "quiet"
// move the gain a lot; the curve flattens as it approaches the
// 1.4 ceiling so we don't blow speakers chasing a passing truck.
//
// ── Smoothing ──────────────────────────────────────────────────────
//
// The canceller already EWMA-smooths band_energy and noise_floor,
// but `ratio` can still spike on transients (door slam, single
// vehicle). We apply additional smoothing here — a single-pole
// IIR with α=0.1 (~5-second time constant at 20Hz update rate)
// before mapping to gain. This means a sustained traffic event
// takes ~5 s to fully ramp up the scene; sudden peaks don't make
// the scene "pump."

/// Multiplier breakpoints — see file header. Each pair is
/// `(loudness_ratio, multiplier)`. Must be sorted ascending by
/// ratio. The first / last entries' multipliers are returned
/// outside the table's range (clamped).
const CURVE: &[(f32, f32)] = &[
    ( 0.0, 0.70),
    ( 3.0, 0.70),
    ( 5.0, 0.85),
    (10.0, 1.00),
    (20.0, 1.15),
    (30.0, 1.30),
    (50.0, 1.40),
];

/// EWMA smoothing factor for the loudness ratio. α=0.1 → ~5 s
/// time constant at our 0.5 s tick rate. Tuned so that sustained
/// noise (~30 s of traffic) reaches its steady-state gain by the
/// time the user might consciously notice, but short transients
/// (a single car door slam) barely move the needle.
pub const SMOOTHING_ALPHA: f32 = 0.1;

/// Stateful adapter. Owns the smoothed-ratio EWMA + the previous
/// commanded gain (so we can avoid spamming SetSceneGain commands
/// when nothing has changed).
#[derive(Debug, Clone)]
pub struct NoiseAdaptiveGain {
    smoothed_ratio: f32,
    /// Last gain we sent. Used to suppress repeated identical
    /// commands; the controller polls at 2 Hz so without this
    /// dedup we'd send ~120 SetSceneGain msgs/min even at
    /// perfectly steady noise.
    last_sent_gain: f32,
    /// Hysteresis on the SEND decision — only re-send when the
    /// new gain differs by more than this fraction from
    /// last_sent. 2% chosen so quiet drift (band-energy jitter)
    /// doesn't trigger sends, but real changes (5-15% per the
    /// curve breakpoints) always do.
    send_threshold: f32,
}

impl Default for NoiseAdaptiveGain {
    fn default() -> Self {
        Self {
            smoothed_ratio: 10.0,   // nominal (= unity gain at start)
            last_sent_gain: 1.0,
            send_threshold: 0.02,
        }
    }
}

impl NoiseAdaptiveGain {
    pub fn new() -> Self { Self::default() }

    /// Reset state. Called when ANC standby stops so the next
    /// session starts from neutral.
    pub fn reset(&mut self) {
        self.smoothed_ratio = 10.0;
        self.last_sent_gain = 1.0;
    }

    /// Feed a fresh observation. Returns `Some(gain)` if the
    /// caller should issue a `set_scene_gain` to bo-muxd (i.e.
    /// the new gain differs from the last by > send_threshold),
    /// otherwise `None`.
    ///
    /// `ratio` is `band_energy / noise_floor` from the canceller's
    /// PassbySnapshot. Values below 1.0 (band energy under the
    /// estimated floor — physically impossible but can happen
    /// during cold-start before the floor stabilizes) are clamped
    /// to 0.0 for the curve.
    pub fn observe(&mut self, ratio: f32) -> Option<f32> {
        let clean = if ratio.is_finite() && ratio >= 0.0 { ratio } else { 0.0 };
        self.smoothed_ratio =
            SMOOTHING_ALPHA * clean + (1.0 - SMOOTHING_ALPHA) * self.smoothed_ratio;
        let g = curve_lookup(self.smoothed_ratio);
        let delta = (g - self.last_sent_gain).abs() / self.last_sent_gain.max(0.01);
        if delta > self.send_threshold {
            self.last_sent_gain = g;
            Some(g)
        } else {
            None
        }
    }

    /// Current smoothed ratio (for UI / diagnostics).
    pub fn smoothed_ratio(&self) -> f32 { self.smoothed_ratio }

    /// Current gain that would be commanded (regardless of dedup).
    pub fn current_gain(&self) -> f32 { curve_lookup(self.smoothed_ratio) }
}

/// Piecewise-linear lookup on the CURVE table. Clamps to endpoints
/// for inputs outside the table range. NaN → low clamp (safe
/// default: don't boost on bad data). +∞ → high clamp (an
/// extremely loud burst is real even if numerically saturated).
pub fn curve_lookup(ratio: f32) -> f32 {
    if ratio.is_nan() { return CURVE[0].1; }
    if ratio <= CURVE[0].0 { return CURVE[0].1; }
    let last = CURVE[CURVE.len() - 1];
    if ratio >= last.0 { return last.1; }
    // Find the bracketing pair.
    for window in CURVE.windows(2) {
        let (x0, y0) = window[0];
        let (x1, y1) = window[1];
        if ratio >= x0 && ratio <= x1 {
            // Linear interp.
            let t = (ratio - x0) / (x1 - x0);
            return y0 + t * (y1 - y0);
        }
    }
    // Unreachable given the clamps above.
    last.1
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approx(a: f32, b: f32) -> bool { (a - b).abs() < 1e-4 }

    #[test]
    fn curve_clamps_at_low_end() {
        assert!(approx(curve_lookup(0.0), 0.70));
        assert!(approx(curve_lookup(-5.0), 0.70));
        assert!(approx(curve_lookup(1.5), 0.70));
    }

    #[test]
    fn curve_clamps_at_high_end() {
        assert!(approx(curve_lookup(50.0), 1.40));
        assert!(approx(curve_lookup(1000.0), 1.40));
    }

    #[test]
    fn curve_passes_through_breakpoints() {
        assert!(approx(curve_lookup(3.0), 0.70));
        assert!(approx(curve_lookup(5.0), 0.85));
        assert!(approx(curve_lookup(10.0), 1.00));
        assert!(approx(curve_lookup(20.0), 1.15));
        assert!(approx(curve_lookup(30.0), 1.30));
        assert!(approx(curve_lookup(50.0), 1.40));
    }

    #[test]
    fn curve_interpolates_between_breakpoints() {
        // Midpoint of (5, 0.85) and (10, 1.00) = (7.5, 0.925)
        assert!(approx(curve_lookup(7.5), 0.925));
        // Midpoint of (10, 1.00) and (20, 1.15) = (15, 1.075)
        assert!(approx(curve_lookup(15.0), 1.075));
    }

    #[test]
    fn nan_input_returns_low_clamp() {
        assert!(approx(curve_lookup(f32::NAN), 0.70));
        assert!(approx(curve_lookup(f32::INFINITY), 1.40));
    }

    #[test]
    fn observe_smooths_with_alpha() {
        let mut g = NoiseAdaptiveGain::new();
        // Initial smoothed_ratio = 10 (current_gain = 1.0).
        // Feed a sudden spike to 50.
        g.observe(50.0);
        // After one tick with α=0.1: new = 0.1*50 + 0.9*10 = 14
        // (not the steady-state 50 yet — confirms smoothing).
        assert!(g.smoothed_ratio() > 13.0 && g.smoothed_ratio() < 15.0,
            "expected ~14 after first tick, got {}", g.smoothed_ratio());
    }

    #[test]
    fn observe_reaches_steady_state_after_many_ticks() {
        let mut g = NoiseAdaptiveGain::new();
        // ~50 ticks should be plenty for α=0.1 to reach 95% of target.
        for _ in 0..100 { g.observe(30.0); }
        assert!((g.smoothed_ratio() - 30.0).abs() < 0.1,
            "expected ~30 after many ticks, got {}", g.smoothed_ratio());
        assert!((g.current_gain() - 1.30).abs() < 1e-3,
            "expected ~1.30 gain, got {}", g.current_gain());
    }

    #[test]
    fn observe_dedups_when_change_below_threshold() {
        let mut g = NoiseAdaptiveGain::new();
        // First call: from initial gain 1.0 → ratio 10 → gain 1.0 → no send.
        assert!(g.observe(10.0).is_none(),
            "no change from default → expected None");
        // Tiny noise floor jitter that doesn't move the gain past 2%.
        for _ in 0..3 { g.observe(10.5); }
        // Still under 2% threshold from 1.0.
        assert!(g.observe(10.6).is_none());
    }

    #[test]
    fn observe_sends_on_real_change() {
        let mut g = NoiseAdaptiveGain::new();
        // Drive to steady state at ratio 30, then check we send on
        // change. We discard the initial transient first.
        for _ in 0..100 { g.observe(30.0); }
        // Now jump to ratio 5. Eventually we should emit.
        let mut sent = false;
        for _ in 0..100 {
            if g.observe(5.0).is_some() {
                sent = true;
                break;
            }
        }
        assert!(sent, "should have sent at least once after major change");
    }

    #[test]
    fn reset_clears_state() {
        let mut g = NoiseAdaptiveGain::new();
        for _ in 0..100 { g.observe(50.0); }
        assert!((g.smoothed_ratio() - 50.0).abs() < 0.5);
        g.reset();
        assert!((g.smoothed_ratio() - 10.0).abs() < 0.01);
        assert!((g.current_gain() - 1.0).abs() < 0.01);
    }
}
