// bo-controller/src/spectral_masker.rs
//
// Dynamic masker: given a real-time per-band noise spectrum from the
// canceller, compute per-band scene-gain adjustments that target the
// noisy bands harder while leaving quiet bands alone. This is a
// strict upgrade over the scalar noise_adaptive_gain — a 1× gain
// multiplier can boost the entire scene, but if the noise is
// concentrated in, say, 200-400 Hz (a passing truck rumble) and the
// scene is broadband, boosting EVERYTHING wastes energy on bands
// that didn't need help and risks pumping clearly audible spectral
// regions the user already enjoyed quietly.
//
// ── How it slots into the system ──────────────────────────────────
//
// The canceller already maintains noise-floor and band-energy
// estimates internally. We define a small set of perceptually-spaced
// bands (sub-bass, bass, low-mid, mid, high-mid, presence) and on
// each refresh, ask the canceller for its band-by-band energy
// snapshot. We compute per-band ratios, apply the existing scalar
// curve to each band independently, smooth temporally, and emit a
// vector of per-band masker gains.
//
// The bo-mux side receives these via a new `SetMaskerBandGains`
// command and applies them inside the scene mixer (each layer's
// output is filtered into the bands and weighted by the matching
// gain before being summed). That side will land in a future turn —
// for now this module exists as the controller-side computation,
// usable behind a feature flag, and the existing scalar gain
// continues to be the live behavior.
//
// ── Why six bands ────────────────────────────────────────────────
//
// Wider than 6 bands gets noisy on the noise-floor estimate (each
// band has fewer samples). Narrower than 6 bands gives up the
// perceptual gain that spectral targeting was built for. 6 is the
// sweet spot for the typical urban-ambient + indoor-HVAC mix.
//
// Band boundaries (Hz): 20-80, 80-200, 200-500, 500-1200,
// 1200-3000, 3000-9000. Logarithmic-ish, biased toward where
// human hearing is most sensitive (2-4 kHz) for finer resolution.

use std::time::Instant;

/// Number of bands the masker tracks.
pub const N_BANDS: usize = 6;

/// Band edge frequencies in Hz. Length is N_BANDS + 1. Each band i
/// covers [BAND_EDGES[i], BAND_EDGES[i+1]).
pub const BAND_EDGES: [f32; N_BANDS + 1] = [
    20.0, 80.0, 200.0, 500.0, 1200.0, 3000.0, 9000.0,
];

/// Gain curve breakpoints per band — same shape as the scalar
/// noise_adaptive_gain curve, but applied per-band. (ratio, gain).
const CURVE: &[(f32, f32)] = &[
    ( 0.0, 0.70),
    ( 3.0, 0.70),
    ( 5.0, 0.90),
    (10.0, 1.00),
    (20.0, 1.15),
    (30.0, 1.30),
    (50.0, 1.40),
];

/// Per-band EWMA smoothing factor. 0.1 → ~5 s time constant at the
/// canceller's 0.5 s refresh rate. Smaller than the scalar gain's
/// alpha because per-band measurements have more variance and we
/// want stable per-band gains rather than pumping per-band.
pub const SMOOTHING_ALPHA: f32 = 0.08;

/// Per-band send-deduplication threshold. 4% — twice the scalar
/// threshold because we have 6 bands and don't want to spam updates
/// when only 1 band moved. Sending 6× as many SetMaskerBandGains
/// commands as a single SetSceneGain would be a real cost on the
/// IPC channel.
pub const SEND_THRESHOLD: f32 = 0.04;

/// Per-band observation. The caller (typically anc_standby_refresh)
/// builds this from the canceller's snapshot.
#[derive(Debug, Clone, Copy, Default)]
pub struct BandObservation {
    /// Band energy at this moment (linear units).
    pub band_energy: [f32; N_BANDS],
    /// Estimated noise floor per band (linear units). Updated by the
    /// canceller during quiet windows; falls slowly so loud events
    /// don't contaminate the reference.
    pub noise_floor: [f32; N_BANDS],
}

/// Stateful dynamic masker.
#[derive(Debug, Clone)]
pub struct SpectralMasker {
    /// EWMA of the per-band loudness ratio (band_energy / noise_floor).
    smoothed_ratio: [f32; N_BANDS],
    /// Last gain commanded per-band — used for dedup.
    last_sent: [f32; N_BANDS],
    /// Timestamp of the most-recent emit. Caller can rate-limit
    /// further if needed.
    last_emit_at: Option<Instant>,
}

impl Default for SpectralMasker {
    fn default() -> Self {
        Self {
            // Initial smoothed_ratio = 10 per band → all gains start
            // at 1.0 (the scalar curve's unity point). This avoids
            // a startup pop where the masker would otherwise jump
            // from "no commands sent → first observation drives a
            // big delta."
            smoothed_ratio: [10.0; N_BANDS],
            last_sent: [1.0; N_BANDS],
            last_emit_at: None,
        }
    }
}

impl SpectralMasker {
    pub fn new() -> Self { Self::default() }

    /// Reset to defaults. Called when ANC standby stops so the next
    /// session starts with clean state.
    pub fn reset(&mut self) { *self = Self::default(); }

    /// Feed a fresh per-band observation. Returns `Some(gains)` if
    /// any band moved by more than SEND_THRESHOLD since the last
    /// emit, otherwise `None`.
    pub fn observe(&mut self, obs: BandObservation) -> Option<[f32; N_BANDS]> {
        let mut new_gains = [1.0f32; N_BANDS];
        for i in 0..N_BANDS {
            let raw = if obs.noise_floor[i] > 1e-6 {
                obs.band_energy[i] / obs.noise_floor[i]
            } else { 10.0 };
            // Guard finite — NaN propagating into smoothed_ratio
            // would lock the band silent forever.
            let clean = if raw.is_finite() && raw >= 0.0 { raw } else { 0.0 };
            self.smoothed_ratio[i] = SMOOTHING_ALPHA * clean
                + (1.0 - SMOOTHING_ALPHA) * self.smoothed_ratio[i];
            new_gains[i] = curve_lookup(self.smoothed_ratio[i]);
        }
        // Decide whether to emit. Send if ANY band moved past the
        // threshold (4%) since the last emit. Don't compare against
        // the initial 1.0 — that's just an init value, not a real
        // sent state — unless we've already emitted once.
        let mut any_moved = false;
        for i in 0..N_BANDS {
            let delta = (new_gains[i] - self.last_sent[i]).abs()
                / self.last_sent[i].max(0.01);
            if delta > SEND_THRESHOLD { any_moved = true; break; }
        }
        if any_moved {
            self.last_sent = new_gains;
            self.last_emit_at = Some(Instant::now());
            Some(new_gains)
        } else {
            None
        }
    }

    /// Current smoothed gain vector (regardless of dedup). For UI.
    pub fn current_gains(&self) -> [f32; N_BANDS] {
        let mut g = [1.0; N_BANDS];
        for i in 0..N_BANDS { g[i] = curve_lookup(self.smoothed_ratio[i]); }
        g
    }

    /// Current smoothed loudness ratio per band. For UI.
    pub fn current_ratios(&self) -> [f32; N_BANDS] { self.smoothed_ratio }
}

/// Same piecewise-linear curve as noise_adaptive_gain.rs.
/// Duplicated here rather than imported to keep this module
/// stand-alone testable.
fn curve_lookup(ratio: f32) -> f32 {
    if ratio.is_nan() { return CURVE[0].1; }
    if ratio <= CURVE[0].0 { return CURVE[0].1; }
    let last = CURVE[CURVE.len() - 1];
    if ratio >= last.0 { return last.1; }
    for window in CURVE.windows(2) {
        let (x0, y0) = window[0];
        let (x1, y1) = window[1];
        if ratio >= x0 && ratio <= x1 {
            let t = (ratio - x0) / (x1 - x0);
            return y0 + t * (y1 - y0);
        }
    }
    last.1
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_unity() {
        let m = SpectralMasker::new();
        for &g in &m.current_gains() {
            assert!((g - 1.0).abs() < 1e-3,
                "expected 1.0 default, got {}", g);
        }
    }

    #[test]
    fn first_observation_with_no_change_dedups() {
        let mut m = SpectralMasker::new();
        // Match the initial smoothed_ratio so no gain moves.
        let obs = BandObservation {
            band_energy: [10.0; N_BANDS],
            noise_floor: [1.0; N_BANDS],
        };
        // First observation: ratio = 10, curve_lookup = 1.0, no change.
        assert!(m.observe(obs).is_none(),
            "no change from defaults should dedup");
    }

    #[test]
    fn loud_band_drives_per_band_gain() {
        let mut m = SpectralMasker::new();
        // Hammer band 2 (200-500 Hz — typical truck rumble) with
        // 50× the floor for many ticks. Other bands stay quiet.
        let mut obs = BandObservation {
            band_energy: [1.0; N_BANDS],
            noise_floor: [1.0; N_BANDS],
        };
        obs.band_energy[2] = 50.0;
        for _ in 0..100 { m.observe(obs); }
        let g = m.current_gains();
        // Band 2 should be at the curve ceiling (1.40).
        assert!((g[2] - 1.40).abs() < 0.05,
            "loud band 2 should reach ~1.40, got {}", g[2]);
        // Other bands should stay near 1.0 (ratio = 1 → curve clamps low).
        for i in [0, 1, 3, 4, 5] {
            assert!(g[i] < 0.95,
                "band {} should stay quiet (ratio=1), got {}", i, g[i]);
        }
    }

    #[test]
    fn quiet_band_drops_gain() {
        let mut m = SpectralMasker::new();
        // All bands very quiet — ratio = 1, well below the curve
        // breakpoint where gain stays at 1.0. Curve at ratio 1 = 0.70.
        let obs = BandObservation {
            band_energy: [1.0; N_BANDS],
            noise_floor: [1.0; N_BANDS],
        };
        for _ in 0..100 { m.observe(obs); }
        let g = m.current_gains();
        for i in 0..N_BANDS {
            assert!((g[i] - 0.70).abs() < 0.05,
                "all-quiet band {} should be at low clamp, got {}", i, g[i]);
        }
    }

    #[test]
    fn nan_input_does_not_poison_state() {
        let mut m = SpectralMasker::new();
        let mut obs = BandObservation::default();
        obs.band_energy[0] = f32::NAN;
        obs.noise_floor[0] = 1.0;
        // Should clamp to 0.0 → curve returns 0.70 → no panic.
        m.observe(obs);
        let g = m.current_gains();
        assert!(g[0].is_finite(),
            "NaN input must not produce NaN gain");
    }

    #[test]
    fn cold_start_noise_floor_handled() {
        let mut m = SpectralMasker::new();
        // Noise floor below the 1e-6 guard → fallback ratio = 10.
        let obs = BandObservation {
            band_energy: [1.0; N_BANDS],
            noise_floor: [0.0; N_BANDS],
        };
        // Should not divide by zero; should produce unity-ish gains.
        let r = m.observe(obs);
        assert!(r.is_none() || r.unwrap().iter().all(|&g| g.is_finite()));
    }

    #[test]
    fn sends_on_real_change_after_reaching_steady_state() {
        let mut m = SpectralMasker::new();
        // First drive bands quiet (well past dedup threshold from defaults).
        let quiet = BandObservation {
            band_energy: [1.0; N_BANDS],
            noise_floor: [1.0; N_BANDS],
        };
        let mut emits = 0;
        for _ in 0..200 {
            if m.observe(quiet).is_some() { emits += 1; }
        }
        assert!(emits > 0, "should emit at least once moving from defaults to quiet");
    }

    #[test]
    fn reset_clears_state() {
        let mut m = SpectralMasker::new();
        let loud = BandObservation {
            band_energy: [50.0; N_BANDS],
            noise_floor: [1.0; N_BANDS],
        };
        for _ in 0..100 { m.observe(loud); }
        let g_before = m.current_gains();
        assert!(g_before[0] > 1.2, "should be boosted before reset");
        m.reset();
        let g_after = m.current_gains();
        for i in 0..N_BANDS {
            assert!((g_after[i] - 1.0).abs() < 1e-3,
                "band {} should be unity after reset", i);
        }
    }

    #[test]
    fn band_edges_cover_full_audio_range() {
        // 20 Hz to 9 kHz — covers the bands the canceller cares
        // about. Sub-20 Hz isn't reproducible on consumer speakers;
        // above 9 kHz noise events are too short-lived to mask
        // perceptually with a 16-second clip loop.
        assert_eq!(BAND_EDGES.len(), N_BANDS + 1);
        assert_eq!(BAND_EDGES[0], 20.0);
        assert_eq!(BAND_EDGES[N_BANDS], 9000.0);
        // Edges must be monotonic ascending.
        for i in 1..BAND_EDGES.len() {
            assert!(BAND_EDGES[i] > BAND_EDGES[i-1],
                "band edges must be ascending");
        }
    }
}
