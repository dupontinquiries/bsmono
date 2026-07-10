// src/music_masker.rs
#![allow(dead_code)]
//
// Classical-music-as-masker: spectral boost in the cancellation band.
//
// ── The insight ──
//
// A perfect anti-noise canceller would need perfect phase alignment
// at the listener's ear. The network_sim fuzz harness reveals that
// with realistic latency + jitter the alignment is usually off,
// and an off-phase anti-noise signal ADDS to the noise instead of
// cancelling it. The audible result: at jitter levels typical of
// BT audio (50-100 ms), the canceller's output can make things
// audibly worse rather than better.
//
// The user's observation: if classical music is playing through
// the same speakers, the music's spectral content in the noise
// band MASKS the residual — even an off-phase anti-noise signal
// gets covered by the music as long as the music has enough
// energy in the band. The phase-tolerance window goes from
// "must be within ±20°" (silence) to "anywhere within ±60° is
// fine" (with music).
//
// ── How this module helps ──
//
// During ANC operation, when music is playing, this module applies
// a PARAMETRIC BOOST to the music's spectrum at the canceller's
// target frequency. The boost is a biquad peaking-EQ filter
// centered on the noise fundamental with bandwidth wide enough
// to cover Doppler variation. The boost gives the music extra
// energy in the noise band, widening the phase-tolerance window
// for the anti-noise output without changing the music's overall
// perceived character (peaking EQs are localized).
//
// ── Why classical specifically ──
//
// Classical orchestral works tend to have CONTINUOUS spectral
// content (sustained string/brass/woodwind harmonics) rather than
// the sparse percussive bursts of pop. A passing-car fundamental
// at 200 Hz collides with cello (G3 = 196 Hz, D4 = 294 Hz),
// bassoon, and viola C-strings. The orchestra is already
// providing energy in that band; we're just nudging it up a few dB.
// Pop/electronic music's amplified bass drops in/out unpredictably
// and the masking effect wanders with each beat — works less well
// for sustained-tolerance.
//
// ── Implementation ──
//
// Transposed Direct Form II biquad, recomputed when the target
// frequency moves (every ~100 ms typically). The TDF II is
// preferred for time-varying coefficients because its internal
// state has direct musical meaning (delayed input/output);
// updating coefficients mid-stream doesn't introduce as much
// click/pop as DF I.
//
// Coefficients per Robert Bristow-Johnson's audio EQ cookbook:
//   A = 10^(gain_db / 40)
//   w0 = 2π · f0 / fs
//   alpha = sin(w0) / (2 · Q)
//
//   b0 = 1 + alpha · A
//   b1 = -2 cos(w0)
//   b2 = 1 - alpha · A
//   a0 = 1 + alpha / A
//   a1 = -2 cos(w0)
//   a2 = 1 - alpha / A
//
// Normalized: divide all by a0 so a0' = 1.

use std::f32::consts::PI;

/// One peaking-EQ biquad with TDF II state.
#[derive(Debug, Clone)]
pub struct PeakingEq {
    // Normalized filter coefficients.
    b0: f32, b1: f32, b2: f32,
    a1: f32, a2: f32,
    // State.
    z1: f32, z2: f32,
}

impl Default for PeakingEq {
    fn default() -> Self {
        // Identity filter (no boost).
        Self {
            b0: 1.0, b1: 0.0, b2: 0.0,
            a1: 0.0, a2: 0.0,
            z1: 0.0, z2: 0.0,
        }
    }
}

impl PeakingEq {
    pub fn new() -> Self { Self::default() }

    /// Reconfigure the filter for a new target frequency / gain / Q.
    /// State (`z1`, `z2`) is preserved to avoid clicks; in practice
    /// the user's ear tolerates a few ms of mid-band ripple while
    /// the filter recovers from the coefficient change.
    pub fn set_target(&mut self, f0_hz: f32, gain_db: f32, q: f32, fs_hz: f32) {
        if fs_hz <= 0.0 || f0_hz <= 0.0 || f0_hz >= fs_hz * 0.45 {
            // Degenerate target — set to identity. The 0.45 limit
            // is below Nyquist to avoid aliasing artifacts.
            *self = Self::default();
            // Preserve state.
            self.z1 = self.z1;
            self.z2 = self.z2;
            return;
        }
        let q = q.max(0.1);
        let a = 10.0f32.powf(gain_db / 40.0);
        let w0 = 2.0 * PI * f0_hz / fs_hz;
        let cos_w0 = w0.cos();
        let alpha = w0.sin() / (2.0 * q);

        let b0 = 1.0 + alpha * a;
        let b1 = -2.0 * cos_w0;
        let b2 = 1.0 - alpha * a;
        let a0 = 1.0 + alpha / a;
        let a1 = -2.0 * cos_w0;
        let a2 = 1.0 - alpha / a;

        // Normalize.
        self.b0 = b0 / a0;
        self.b1 = b1 / a0;
        self.b2 = b2 / a0;
        self.a1 = a1 / a0;
        self.a2 = a2 / a0;
    }

    /// Process one sample. Transposed Direct Form II.
    #[inline]
    pub fn process(&mut self, x: f32) -> f32 {
        // Defend against NaN: a corrupted state value or input
        // would explode the filter; we re-zero on detection.
        if !x.is_finite() || !self.z1.is_finite() || !self.z2.is_finite() {
            self.z1 = 0.0;
            self.z2 = 0.0;
            return 0.0;
        }
        let y = self.b0 * x + self.z1;
        self.z1 = self.b1 * x - self.a1 * y + self.z2;
        self.z2 = self.b2 * x - self.a2 * y;
        y
    }

    pub fn process_block(&mut self, samples: &mut [f32]) {
        for s in samples.iter_mut() { *s = self.process(*s); }
    }

    pub fn reset(&mut self) { self.z1 = 0.0; self.z2 = 0.0; }
}

// ─── Multi-harmonic comb masker ────────────────────────────────────────────
//
// ── The phase-coverage problem ──
//
// A single anti-noise signal at the engine fundamental has only
// ±30° of phase before cancellation crosses 0 dB and it starts
// making things worse. The single-band music masker we built
// widens that window at the FUNDAMENTAL by ~6 dB per +6 dB of
// masker boost. But it does nothing for residue at the harmonics.
//
// Residue at harmonics is where the canceller's failure mode lives:
//
//   - Anti-noise time-error Δt yields phase error Δφ_n = 2π·n·f·Δt
//     at the n-th harmonic. At 200 Hz fundamental with Δt = 0.5 ms,
//     the fundamental is off by 36° (residue: 6 dB below original),
//     the 2nd harmonic is off by 72° (residue: 3 dB ABOVE original),
//     and the 3rd is off by 108° (5 dB above).
//
//   - So even with reasonably-aligned anti-noise, harmonics ABOVE
//     the fundamental are typically AMPLIFIED, not cancelled. The
//     listener still hears engine harmonics — what's been removed
//     is just the fundamental.
//
// ── The harmonic masker insight ──
//
// We CAN'T improve the anti-noise's phase tolerance at harmonics —
// the physics is fixed. But we CAN mask the residue at the
// harmonics by boosting the music's spectrum at f, 2f, 3f, 4f
// simultaneously. The TOTAL effective "noise band" coverage of
// the system (anti-noise + masker) widens to cover the full
// harmonic series, even though no individual anti-noise component
// got better.
//
// In practice that's the difference between "fundamental gone but
// engine still buzzy" and "engine recedes into the music".
//
// ── Per-harmonic gain falloff ──
//
// Real exhaust harmonic envelopes drop ~3-5 dB per harmonic from
// the fundamental. We mirror that: the masker boost at h_n is
// `base_gain_db - falloff_db * (n-1)`. A +9 dB boost at 200 Hz
// pairs with +6 at 400, +3 at 600, ~0 at 800. Total spectral
// energy added to the music is bounded (~12 dB total across the
// series) but the masking effectiveness covers the full engine
// signature.
//
// ── Per-harmonic Q increase ──
//
// At higher frequencies the human ear's critical bands are wider
// (one ERB at 200 Hz ≈ 50 Hz wide; at 800 Hz ≈ 100 Hz wide).
// We narrow the masker Q at higher harmonics so the boost is
// PERCEPTUALLY equivalent to the fundamental's masking width.
// Without this, masking at 800 Hz would bleed into adjacent
// musical notes (E5 is at 659, G5 at 784) and color the music
// audibly. With it, the boost stays localized to "just where
// the engine's 4th harmonic lives."

pub struct CombMasker {
    /// One EQ stage per harmonic. 4 stages covers fundamental
    /// plus 3 harmonics — typically enough for engine noise
    /// where 4th harmonic of a 200 Hz fundamental is 800 Hz,
    /// already getting into "music's own range" territory.
    eqs: [PeakingEq; 4],
    fundamental_hz: f32,
    base_gain_db: f32,
    base_q: f32,
    /// Falloff in dB per harmonic from the fundamental's boost.
    /// 3.0 mirrors the typical exhaust envelope.
    falloff_db_per_harmonic: f32,
    fs: f32,
    engaged: bool,
}

impl CombMasker {
    pub fn new(fs_hz: f32) -> Self {
        Self {
            eqs: [PeakingEq::new(), PeakingEq::new(),
                  PeakingEq::new(), PeakingEq::new()],
            fundamental_hz: 0.0,
            base_gain_db: 6.0,
            base_q: 2.0,
            falloff_db_per_harmonic: 3.0,
            fs: fs_hz,
            engaged: false,
        }
    }

    /// Update the comb's fundamental + masking parameters. Subsequent
    /// process_block calls (when engaged) apply boosts at f, 2f, 3f,
    /// 4f with gain falling by falloff_db_per_harmonic per step and
    /// Q increasing proportionally to keep perceptual width constant.
    pub fn set_fundamental(&mut self, f_hz: f32, base_gain_db: f32, q: f32) {
        self.fundamental_hz = f_hz;
        self.base_gain_db = base_gain_db;
        self.base_q = q;
        self.reconfigure();
    }

    fn reconfigure(&mut self) {
        if !self.engaged || self.fundamental_hz <= 0.0 {
            for eq in &mut self.eqs { *eq = PeakingEq::default(); }
            return;
        }
        let nyquist_safe = self.fs * 0.45;
        for (i, eq) in self.eqs.iter_mut().enumerate() {
            let n = (i + 1) as f32;
            let f_h = self.fundamental_hz * n;
            if f_h >= nyquist_safe {
                // Harmonic above Nyquist-safe — disable this stage.
                *eq = PeakingEq::default();
                continue;
            }
            // Drop gain per harmonic; never go below 0 (a negative
            // gain in a peaking EQ becomes a notch, which would be
            // wrong here — we want masker boost or nothing).
            let gain = (self.base_gain_db
                - self.falloff_db_per_harmonic * (n - 1.0)).max(0.0);
            if gain <= 0.0 {
                *eq = PeakingEq::default();
                continue;
            }
            // Q scales linearly with harmonic number so perceptual
            // width is roughly constant (the ERB at n·f is roughly
            // n× wider than at f).
            let q_h = self.base_q * n;
            eq.set_target(f_h, gain, q_h, self.fs);
        }
    }

    pub fn set_engaged(&mut self, on: bool) {
        if on == self.engaged { return; }
        self.engaged = on;
        self.reconfigure();
    }

    pub fn engaged(&self) -> bool { self.engaged }
    pub fn fundamental_hz(&self) -> f32 { self.fundamental_hz }

    /// Apply the cascaded comb to a music buffer in place.
    pub fn process_block(&mut self, music: &mut [f32]) {
        // Cascade — each EQ feeds the next via the buffer.
        for eq in &mut self.eqs {
            eq.process_block(music);
        }
    }

    /// Approximate the effective phase-tolerance of the FULL system
    /// (anti-noise + masker), averaged across the engine's harmonic
    /// series weighted by perceived annoyance per harmonic.
    ///
    /// This metric is "apples-to-apples" with the corresponding
    /// MusicMasker::system_phase_tolerance_degrees — both compute
    /// the same annoyance-weighted average across f, 2f, 3f, 4f.
    /// The comb wins because the music masker only widens at the
    /// fundamental while the comb widens at every harmonic.
    pub fn effective_phase_tolerance_degrees(&self) -> f32 {
        if !self.engaged || self.fundamental_hz <= 0.0 { return 20.0; }
        let annoyance_weights = [1.0, 0.6, 0.35, 0.2];
        let mut weighted_tolerance = 0.0f32;
        let mut weight_sum = 0.0f32;
        for i in 0..4 {
            let n = (i + 1) as f32;
            let gain = (self.base_gain_db
                - self.falloff_db_per_harmonic * (n - 1.0)).max(0.0);
            // Per-harmonic tolerance: 20° baseline × amplitude ratio.
            let amp = 10.0f32.powf(gain / 20.0);
            let h_tol = (20.0 * amp).min(180.0);
            weighted_tolerance += h_tol * annoyance_weights[i];
            weight_sum += annoyance_weights[i];
        }
        if weight_sum > 0.0 { weighted_tolerance / weight_sum } else { 20.0 }
    }
}

/// The masker — wraps a peaking EQ with policy logic about when to
/// engage, how aggressive to be, and how to follow the canceller's
/// changing target frequency.
pub struct MusicMasker {
    /// Underlying filter.
    eq: PeakingEq,
    /// Current target frequency (Hz). 0 = filter is identity.
    target_hz: f32,
    /// Current boost in dB.
    gain_db: f32,
    /// Q (bandwidth) — lower Q = wider boost.
    q: f32,
    /// Sample rate.
    fs: f32,
    /// Whether boost is engaged. Set false when the canceller has
    /// no active event; set true when an event is in progress AND
    /// music is playing.
    engaged: bool,
}

impl MusicMasker {
    pub fn new(fs_hz: f32) -> Self {
        Self {
            eq: PeakingEq::new(),
            target_hz: 0.0,
            gain_db: 0.0,
            q: 2.0,
            fs: fs_hz,
            engaged: false,
        }
    }

    /// Set the masker's target frequency. Called by the canceller
    /// when its tracked fundamental changes — every ~100 ms when
    /// Doppler is active, every few seconds when steady-state.
    ///
    /// `gain_db` typical range: 3-9 dB. Above 9 dB the music
    /// starts sounding ducked/EQ'd. Below 3 dB the masking effect
    /// is too subtle to matter.
    ///
    /// `q` typical range: 1.5-3. Lower Q catches Doppler variation
    /// better but bleeds into adjacent musical notes (a Q of 1.5
    /// at 200 Hz boosts everything from 130 to 270 Hz to some
    /// degree). Higher Q is more selective but misses Doppler.
    pub fn set_target(&mut self, target_hz: f32, gain_db: f32, q: f32) {
        self.target_hz = target_hz;
        self.gain_db = gain_db;
        self.q = q;
        if self.engaged && target_hz > 0.0 {
            self.eq.set_target(target_hz, gain_db, q, self.fs);
        } else {
            // Identity — pass music through unchanged.
            self.eq = PeakingEq::default();
            // Preserve filter state through engagement transitions
            // to avoid the discontinuity click.
        }
    }

    /// Engage / disengage the masker. Engaging means subsequent
    /// `process_block` calls actually apply the boost; disengaging
    /// returns to identity (music unchanged).
    pub fn set_engaged(&mut self, on: bool) {
        if on == self.engaged { return; }
        self.engaged = on;
        if on && self.target_hz > 0.0 {
            self.eq.set_target(self.target_hz, self.gain_db, self.q, self.fs);
        } else {
            // Don't fully reset — preserve z1/z2 so disengagement
            // doesn't click.
            let z1 = self.eq.z1;
            let z2 = self.eq.z2;
            self.eq = PeakingEq::default();
            self.eq.z1 = z1;
            self.eq.z2 = z2;
        }
    }

    pub fn engaged(&self) -> bool { self.engaged }
    pub fn target_hz(&self) -> f32 { self.target_hz }
    pub fn gain_db(&self) -> f32 { self.gain_db }

    /// Apply the masker to a music buffer in place. When disengaged,
    /// this is still called but routes through the identity filter
    /// (so the buffer is essentially unchanged modulo state
    /// preservation).
    pub fn process_block(&mut self, music: &mut [f32]) {
        self.eq.process_block(music);
    }

    /// Compute the expected phase-tolerance widening factor given
    /// the current boost and the music's existing energy in the
    /// target band.
    ///
    /// Math (intuition): if anti-noise is off-phase by Δφ, the
    /// residual amplitude is 2·sin(Δφ/2)·A_anti. This residual
    /// becomes audible above the music's masking threshold when
    /// residual > masker_level. With the boost, masker_level is
    /// higher, so the tolerable Δφ is higher. We approximate the
    /// tolerable Δφ as proportional to sqrt(masker_level).
    ///
    /// Returns the tolerable phase offset in degrees. UI surfaces
    /// this as "phase forgiveness budget" for the user's intuition.
    pub fn phase_tolerance_degrees(&self) -> f32 {
        if !self.engaged || self.target_hz <= 0.0 { return 20.0; }
        // Baseline tolerance with silence: ~20°.
        // Boost in dB: each +6 dB roughly doubles linear amplitude;
        // doubling the masker doubles the residual budget.
        let amp_ratio = 10.0f32.powf(self.gain_db / 20.0);
        // Cap at 180° (anything beyond is meaningless).
        (20.0 * amp_ratio).min(180.0)
    }

    /// System-wide phase tolerance — annoyance-weighted average
    /// across f, 2f, 3f, 4f, where harmonics get NO boost from a
    /// single-band masker (they're at the 20° baseline). Comparing
    /// this against `CombMasker::effective_phase_tolerance_degrees`
    /// shows the genuine improvement the comb provides across the
    /// full engine signature, not just the fundamental.
    pub fn system_phase_tolerance_degrees(&self) -> f32 {
        let fundamental_tol = self.phase_tolerance_degrees();
        let baseline = 20.0f32;
        let annoyance_weights = [1.0, 0.6, 0.35, 0.2];
        let mut weighted_tol = fundamental_tol * annoyance_weights[0];
        let mut weight_sum = annoyance_weights[0];
        for w in &annoyance_weights[1..] {
            weighted_tol += baseline * w;
            weight_sum += w;
        }
        if weight_sum > 0.0 { weighted_tol / weight_sum } else { baseline }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fs() -> f32 { 48_000.0 }

    #[test]
    fn identity_eq_passes_through_unchanged() {
        let mut eq = PeakingEq::new();
        let mut buf = vec![1.0, -1.0, 0.5, -0.5, 0.0];
        let original = buf.clone();
        eq.process_block(&mut buf);
        // Identity filter shouldn't change anything modulo numerical
        // drift.
        for (a, b) in buf.iter().zip(&original) {
            assert!((a - b).abs() < 1e-6, "identity should pass through; {} vs {}", a, b);
        }
    }

    #[test]
    fn peaking_eq_boosts_target_frequency() {
        let mut eq = PeakingEq::new();
        eq.set_target(200.0, 12.0, 2.0, fs());
        // Push 1 second of a 200 Hz pure tone through.
        let n = 4800; // 100ms at 48k
        let mut signal: Vec<f32> = (0..n).map(|i| {
            (2.0 * PI * 200.0 * i as f32 / fs()).sin()
        }).collect();
        let original_rms: f32 = (signal.iter().map(|s| s * s).sum::<f32>() / n as f32).sqrt();
        eq.process_block(&mut signal);
        // Skip the first 480 samples (10ms) for filter settle.
        let settled = &signal[480..];
        let boosted_rms: f32 = (settled.iter().map(|s| s * s).sum::<f32>() / settled.len() as f32).sqrt();
        // +12 dB ≈ 4× amplitude. Allow generous tolerance for
        // filter ringing on a finite-length test signal.
        let ratio = boosted_rms / original_rms;
        assert!(ratio > 2.5,
            "12dB boost should raise amplitude ~4×; got {}×", ratio);
    }

    #[test]
    fn peaking_eq_leaves_out_of_band_frequencies_alone() {
        let mut eq = PeakingEq::new();
        eq.set_target(200.0, 12.0, 2.0, fs());
        // 2000 Hz tone — well outside the Q=2 boost lobe.
        let n = 4800;
        let mut signal: Vec<f32> = (0..n).map(|i| {
            (2.0 * PI * 2000.0 * i as f32 / fs()).sin()
        }).collect();
        let original_rms: f32 = (signal.iter().map(|s| s * s).sum::<f32>() / n as f32).sqrt();
        eq.process_block(&mut signal);
        let settled = &signal[480..];
        let boosted_rms: f32 = (settled.iter().map(|s| s * s).sum::<f32>() / settled.len() as f32).sqrt();
        let ratio = boosted_rms / original_rms;
        assert!(ratio < 1.3,
            "out-of-band freq should be ~unchanged, ratio={}", ratio);
    }

    #[test]
    fn nan_input_resets_state_safely() {
        let mut eq = PeakingEq::new();
        eq.set_target(200.0, 6.0, 2.0, fs());
        let mut buf = vec![1.0, 0.5, f32::NAN, 0.5, 1.0];
        eq.process_block(&mut buf);
        // After the NaN, state should be reset and subsequent
        // samples should be finite.
        for &s in &buf {
            assert!(s.is_finite() || s == 0.0,
                "post-NaN should yield finite or zero output, got {}", s);
        }
    }

    #[test]
    fn degenerate_target_yields_identity() {
        let mut eq = PeakingEq::new();
        // Frequency above Nyquist threshold → identity.
        eq.set_target(30_000.0, 12.0, 2.0, fs());
        let mut buf = vec![1.0, -1.0, 0.5];
        let orig = buf.clone();
        eq.process_block(&mut buf);
        for (a, b) in buf.iter().zip(&orig) {
            assert!((a - b).abs() < 1e-6);
        }
    }

    #[test]
    fn masker_idle_when_disengaged() {
        let mut m = MusicMasker::new(fs());
        m.set_target(200.0, 6.0, 2.0);
        // Not engaged — process_block should be identity.
        let mut buf: Vec<f32> = (0..1000).map(|i| {
            (2.0 * PI * 200.0 * i as f32 / fs()).sin()
        }).collect();
        let orig = buf.clone();
        m.process_block(&mut buf);
        for (a, b) in buf.iter().zip(&orig) {
            assert!((a - b).abs() < 1e-6,
                "disengaged masker should be identity");
        }
    }

    #[test]
    fn masker_engages_and_boosts() {
        let mut m = MusicMasker::new(fs());
        m.set_target(200.0, 9.0, 2.0);
        m.set_engaged(true);
        let mut buf: Vec<f32> = (0..4800).map(|i| {
            (2.0 * PI * 200.0 * i as f32 / fs()).sin()
        }).collect();
        let orig_rms: f32 = (buf.iter().map(|s| s * s).sum::<f32>() / buf.len() as f32).sqrt();
        m.process_block(&mut buf);
        let settled = &buf[480..];
        let boosted_rms: f32 = (settled.iter().map(|s| s * s).sum::<f32>() / settled.len() as f32).sqrt();
        assert!(boosted_rms > orig_rms * 1.5,
            "engaged masker at +9dB should boost in-band by ~3x; got {}",
            boosted_rms / orig_rms);
    }

    #[test]
    fn phase_tolerance_widens_with_boost() {
        let mut m = MusicMasker::new(fs());
        let disengaged_tolerance = m.phase_tolerance_degrees();
        m.set_target(200.0, 9.0, 2.0);
        m.set_engaged(true);
        let engaged_tolerance = m.phase_tolerance_degrees();
        assert!(engaged_tolerance > disengaged_tolerance,
            "engaged should widen tolerance; {} → {}",
            disengaged_tolerance, engaged_tolerance);
        // +9 dB → amp ratio ~2.8 → tolerance should be ~56°.
        assert!(engaged_tolerance > 40.0,
            "9dB boost should give >40° tolerance, got {}",
            engaged_tolerance);
    }

    #[test]
    fn target_change_doesnt_explode_state() {
        let mut m = MusicMasker::new(fs());
        m.set_engaged(true);
        let mut buf: Vec<f32> = (0..10_000).map(|i| {
            (2.0 * PI * 200.0 * i as f32 / fs()).sin() * 0.5
        }).collect();
        // Update target every 500 samples to simulate Doppler tracking.
        for chunk_start in (0..buf.len()).step_by(500) {
            let freq = 180.0 + (chunk_start as f32 / buf.len() as f32) * 40.0;
            m.set_target(freq, 6.0, 2.0);
            let end = (chunk_start + 500).min(buf.len());
            m.process_block(&mut buf[chunk_start..end]);
        }
        // All outputs finite.
        for &s in &buf {
            assert!(s.is_finite(), "filter should remain stable under target sweeps");
            assert!(s.abs() < 100.0, "no runaway: {}", s);
        }
    }

    // ─── CombMasker tests ──────────────────────────────────────────────────

    #[test]
    fn comb_disengaged_is_identity() {
        let mut c = CombMasker::new(fs());
        c.set_fundamental(200.0, 9.0, 2.0);
        // Engaged=false by default.
        let mut buf: Vec<f32> = (0..1000).map(|i| {
            (2.0 * PI * 200.0 * i as f32 / fs()).sin()
        }).collect();
        let orig = buf.clone();
        c.process_block(&mut buf);
        for (a, b) in buf.iter().zip(&orig) {
            assert!((a - b).abs() < 1e-6,
                "disengaged comb should be identity, {} vs {}", a, b);
        }
    }

    #[test]
    fn comb_boosts_fundamental() {
        let mut c = CombMasker::new(fs());
        c.set_fundamental(200.0, 9.0, 2.0);
        c.set_engaged(true);
        let n = 4800;
        let mut buf: Vec<f32> = (0..n).map(|i| {
            (2.0 * PI * 200.0 * i as f32 / fs()).sin()
        }).collect();
        let orig_rms: f32 = (buf.iter().map(|s| s * s).sum::<f32>() / n as f32).sqrt();
        c.process_block(&mut buf);
        let settled = &buf[480..];
        let boosted_rms: f32 = (settled.iter().map(|s| s * s).sum::<f32>()
            / settled.len() as f32).sqrt();
        assert!(boosted_rms > orig_rms * 1.5,
            "comb should boost fundamental, ratio={}", boosted_rms / orig_rms);
    }

    #[test]
    fn comb_boosts_harmonics_too() {
        let mut c = CombMasker::new(fs());
        c.set_fundamental(200.0, 9.0, 2.0);
        c.set_engaged(true);
        // Push a 400 Hz tone (2nd harmonic) — should be boosted.
        let n = 4800;
        let mut buf: Vec<f32> = (0..n).map(|i| {
            (2.0 * PI * 400.0 * i as f32 / fs()).sin()
        }).collect();
        let orig_rms: f32 = (buf.iter().map(|s| s * s).sum::<f32>() / n as f32).sqrt();
        c.process_block(&mut buf);
        let settled = &buf[480..];
        let boosted_rms: f32 = (settled.iter().map(|s| s * s).sum::<f32>()
            / settled.len() as f32).sqrt();
        // 2nd harmonic gets +6 dB (= 9 - 3); should be roughly 2× amplitude.
        // Q at 2f is 4, narrower than at fundamental → some leakage at exact f.
        // Allow generous tolerance.
        let ratio = boosted_rms / orig_rms;
        assert!(ratio > 1.3,
            "comb should boost 2nd harmonic; got {}", ratio);
    }

    #[test]
    fn comb_leaves_far_out_of_band_alone() {
        let mut c = CombMasker::new(fs());
        c.set_fundamental(200.0, 9.0, 2.0);
        c.set_engaged(true);
        // 3000 Hz — well above the highest harmonic the comb boosts (800).
        let n = 4800;
        let mut buf: Vec<f32> = (0..n).map(|i| {
            (2.0 * PI * 3000.0 * i as f32 / fs()).sin()
        }).collect();
        let orig_rms: f32 = (buf.iter().map(|s| s * s).sum::<f32>() / n as f32).sqrt();
        c.process_block(&mut buf);
        let settled = &buf[480..];
        let boosted_rms: f32 = (settled.iter().map(|s| s * s).sum::<f32>()
            / settled.len() as f32).sqrt();
        let ratio = boosted_rms / orig_rms;
        assert!(ratio < 1.3,
            "far out-of-band should be untouched; got {}", ratio);
    }

    #[test]
    fn comb_effective_tolerance_exceeds_single_band() {
        // The whole point: with a comb covering 4 harmonics, the
        // SYSTEM-WIDE phase-tolerance (annoyance-weighted across
        // the harmonic series) is strictly better than what a
        // single-band masker provides — because the single-band
        // only widens at the fundamental and leaves harmonics at
        // their baseline 20°, while the comb widens all four.
        let mut single = MusicMasker::new(fs());
        single.set_target(200.0, 9.0, 2.0);
        single.set_engaged(true);
        let single_sys_tol = single.system_phase_tolerance_degrees();

        let mut comb = CombMasker::new(fs());
        comb.set_fundamental(200.0, 9.0, 2.0);
        comb.set_engaged(true);
        let comb_tol = comb.effective_phase_tolerance_degrees();

        assert!(comb_tol > single_sys_tol,
            "comb ({:.1}°) should beat single-band system tolerance ({:.1}°)",
            comb_tol, single_sys_tol);
        // And the improvement should be meaningful — at least 15%.
        let improvement = (comb_tol - single_sys_tol) / single_sys_tol;
        assert!(improvement > 0.15,
            "improvement should be >15%; got {:.1}%", improvement * 100.0);
    }

    #[test]
    fn comb_handles_high_fundamental_gracefully() {
        // If fundamental × 4 exceeds Nyquist-safe, those stages
        // should silently disable, not blow up.
        let mut c = CombMasker::new(fs());
        c.set_fundamental(8_000.0, 9.0, 2.0);  // 4th harmonic at 32 kHz
        c.set_engaged(true);
        let mut buf: Vec<f32> = (0..1000).map(|i| {
            (2.0 * PI * 8_000.0 * i as f32 / fs()).sin()
        }).collect();
        c.process_block(&mut buf);
        for &s in &buf {
            assert!(s.is_finite(), "no blowup at high fundamental");
        }
    }

    #[test]
    fn comb_set_fundamental_zero_safely() {
        let mut c = CombMasker::new(fs());
        c.set_engaged(true);
        c.set_fundamental(0.0, 9.0, 2.0);  // degenerate
        let mut buf = vec![1.0, -1.0, 0.5, -0.5];
        let orig = buf.clone();
        c.process_block(&mut buf);
        for (a, b) in buf.iter().zip(&orig) {
            assert!((a - b).abs() < 1e-6, "zero fundamental → identity");
        }
    }
}
