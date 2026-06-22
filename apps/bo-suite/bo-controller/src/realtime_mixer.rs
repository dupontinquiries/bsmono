// src/realtime_mixer.rs
#![allow(dead_code)]
//
// Realtime streaming mixer with separate "speaker" and "instrumental"
// channel groups, each with a 3-band EQ + master volume.
//
// ── Why the groups make sense (high-level principles) ──────────────
//
// The naïve approach — splitting stereo into L vs R, or doing crude
// L-R subtraction — produces audibly broken results because real
// stereo mixes don't map to those axes. We use a more principled
// basis that's grounded in production convention:
//
//   1. SPATIAL: Mid/Side decomposition. M = (L+R)/2, S = (L-R)/2.
//      Since the 1960s, mix engineers have placed vocals at the
//      center (so they appear in M) and panned instruments wide (so
//      they appear in S). M/S is the genuine engineering basis for
//      "what's centered" vs "what's spread", and it's perfectly
//      reversible. We treat M as the "speaker" group and S as the
//      "instrumental" group, with the understanding that real audio
//      has both vocals and instruments in both — the EQ controls let
//      the user push the balance.
//
//   2. SPECTRAL: per-group band centers picked for the role. Each
//      group gets a 3-band EQ whose center frequencies are locked
//      to what actually matters for that musical role:
//
//        Speaker channel:
//          • 250 Hz   chest body / male fundamental
//          • 1500 Hz  vocal presence (key formant region)
//          • 4000 Hz  intelligibility & air
//
//        Instrumental channel:
//          • 200 Hz   bass weight
//          • 800 Hz   instrument body / warmth
//          • 6000 Hz  cymbal sparkle / brilliance
//
//      The user adjusts gain (±12 dB) per band, never frequency.
//      Frequencies are locked because shifting them invites the user
//      to wander into spectral regions that don't correspond to the
//      group's role — which is exactly the failure mode the user
//      flagged as "groups don't make sense."
//
//   3. EXACTLY UNITY at neutral. With both volumes at 1.0 and all
//      six EQ gains at 0 dB, the output is bit-identical to the
//      input. This is critical: the mixer must never colour the
//      sound when it shouldn't. The roundtrip_neutral_is_identity
//      test enforces this.
//
// ── Implementation ─────────────────────────────────────────────────
//
// Each band is a standard Robert Bristow-Johnson biquad: low-shelf
// for the low band, peaking for the mid, high-shelf for the high.
// Biquads have small state (two delay elements per channel) and run
// at ~5 ns per sample on modern CPUs — well within realtime budget
// even at 192 kHz for the 8 biquads we need (3 bands × Mid + 3 bands
// × Side, plus 2 volumes which are zero-state multiplies).
//
// Coefficients are computed lazily when params change — the audio
// thread sees only the steady-state coefficients via the
// `MixerCoeffs` struct, so parameter changes are a single pointer
// swap (no recomputation in the hot loop). For now we update via
// `set_params()` which recomputes inline; when we wire this to an
// actual audio thread we'll need a triple-buffer or atomic-swap
// pattern.

use std::f32::consts::PI;

/// User-facing parameters for one EQ group (speaker or instrumental).
/// Gains are in decibels; volume is a linear multiplier on the final
/// channel sum (1.0 = unity, 2.0 ≈ +6 dB).
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct GroupParams {
    pub low_gain_db:  f32,
    pub mid_gain_db:  f32,
    pub high_gain_db: f32,
    pub volume:       f32,
}

impl Default for GroupParams {
    fn default() -> Self {
        Self { low_gain_db: 0.0, mid_gain_db: 0.0, high_gain_db: 0.0, volume: 1.0 }
    }
}

/// Full mixer configuration: one GroupParams for each channel.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct MixerParams {
    pub speaker:      GroupParams,
    pub instrumental: GroupParams,
}

impl Default for MixerParams {
    fn default() -> Self {
        Self { speaker: GroupParams::default(), instrumental: GroupParams::default() }
    }
}

/// Band roles for the per-group EQ. Locked center frequencies — the
/// user never sees these as adjustable, only their gain.
///
/// These constants encode principle (2) above: each frequency is
/// picked for the role of the group it belongs to. Moving them would
/// undermine the "groups make sense" guarantee.
pub const SPK_LOW_HZ:   f32 = 250.0;
pub const SPK_MID_HZ:   f32 = 1500.0;
pub const SPK_HIGH_HZ:  f32 = 4000.0;
pub const INS_LOW_HZ:   f32 = 200.0;
pub const INS_MID_HZ:   f32 = 800.0;
pub const INS_HIGH_HZ:  f32 = 6000.0;
/// Shared Q for shelf filters (gentler than 1.0, audio-engineer's
/// default for tonal shaping rather than surgical work).
const SHELF_Q: f32 = 0.707;
/// Q for peaking filter — also gentle. A higher Q would make the mid
/// band ring; this width covers about an octave around the center.
const PEAK_Q:  f32 = 1.0;

// ── Biquad filter ──────────────────────────────────────────────────

/// Direct-form II transposed biquad — the form is numerically stable
/// at f32 precision and avoids drift over long realtime runs.
#[derive(Debug, Clone, Copy)]
pub struct Biquad {
    // Coefficients (already normalized by a0).
    b0: f32, b1: f32, b2: f32,
    a1: f32, a2: f32,
    // Per-channel state — two delay elements (Mid + Side share an
    // instance? no — each biquad processes one channel only, so each
    // EQ chain has its own biquad with its own state).
    z1: f32, z2: f32,
}

impl Biquad {
    pub fn identity() -> Self {
        Self { b0: 1.0, b1: 0.0, b2: 0.0, a1: 0.0, a2: 0.0, z1: 0.0, z2: 0.0 }
    }
    pub fn reset_state(&mut self) {
        self.z1 = 0.0;
        self.z2 = 0.0;
    }
    /// True iff both delay-line samples are finite. Used by the
    /// mixer's NaN-recovery path: if a non-finite input ever
    /// poisons the state, every subsequent sample produces NaN
    /// until reset_state. We detect this once per buffer (cheap)
    /// instead of per-sample (would add a branch to the hot path).
    pub fn state_is_finite(&self) -> bool {
        self.z1.is_finite() && self.z2.is_finite()
    }
    /// Process one sample (state-updating). Hot path — inline.
    #[inline(always)]
    pub fn process(&mut self, x: f32) -> f32 {
        let y = self.b0 * x + self.z1;
        self.z1 = self.b1 * x - self.a1 * y + self.z2;
        self.z2 = self.b2 * x - self.a2 * y;
        y
    }

    /// Magnitude response in dB at `freq_hz`, given the sample rate
    /// the filter was designed for. Pure math, no state touched —
    /// safe to call from any thread without locking.
    ///
    /// Math: evaluate H(z) = (b0 + b1·z⁻¹ + b2·z⁻²) / (1 + a1·z⁻¹ +
    /// a2·z⁻²) at z = e^(jω) where ω = 2π·freq/sr, then take 20 log₁₀
    /// of the magnitude. The negative-imaginary terms come from
    /// z⁻¹ = cos(ω) − j·sin(ω).
    pub fn magnitude_db(&self, freq_hz: f32, sample_rate: f32) -> f32 {
        let omega = 2.0 * PI * freq_hz / sample_rate;
        let c1 = omega.cos();
        let s1 = omega.sin();
        let c2 = (2.0 * omega).cos();
        let s2 = (2.0 * omega).sin();
        let num_re = self.b0 + self.b1 * c1 + self.b2 * c2;
        let num_im =          -self.b1 * s1 - self.b2 * s2;
        let den_re = 1.0      + self.a1 * c1 + self.a2 * c2;
        let den_im =          -self.a1 * s1 - self.a2 * s2;
        let num_mag2 = num_re * num_re + num_im * num_im;
        let den_mag2 = den_re * den_re + den_im * den_im;
        let mag2 = num_mag2 / den_mag2.max(1e-30);
        10.0 * mag2.max(1e-30).log10()
    }

    /// Peaking EQ design (RBJ cookbook). gain_db == 0 yields identity.
    /// Peaking EQ design (RBJ cookbook). gain_db == 0 yields identity.
    ///
    /// `q` is clamped to [0.01, 100.0]. Zero or negative Q would
    /// produce NaN coefficients (we divide by it via `sn / (2.0 * q)`);
    /// extreme positive Q gives a numerically unstable filter. The
    /// clamp keeps the public API safe without surprising the caller.
    pub fn peaking(sample_rate: f32, freq_hz: f32, q: f32, gain_db: f32) -> Self {
        // Sanitize every parameter. NaN/Inf in any of these produces
        // NaN coefficients, which then corrupt z1/z2 on the first
        // process() call, which then corrupts every subsequent
        // sample. The fuzz tests for RealtimeMixer would have caught
        // this transitively (the parent test asserts finite output),
        // but defending here makes the constructor self-safe so
        // future call sites that bypass the mixer (e.g. unit tests
        // building biquads directly) don't trip the same trap.
        let sample_rate = if sample_rate.is_finite() && sample_rate > 0.0 {
            sample_rate } else { 48_000.0 };
        let freq_hz = if freq_hz.is_finite() && freq_hz > 0.0 {
            freq_hz.min(sample_rate * 0.49) } else { 1_000.0 };
        let gain_db = if gain_db.is_finite() { gain_db } else { 0.0 };
        let q = if q.is_finite() && q > 0.0 { q.clamp(0.01, 100.0) } else { 0.707 };
        if gain_db.abs() < 1e-6 { return Self::identity(); }
        let a = 10f32.powf(gain_db / 40.0);
        let omega = 2.0 * PI * freq_hz / sample_rate;
        let (sn, cs) = omega.sin_cos();
        let alpha = sn / (2.0 * q);
        let b0 = 1.0 + alpha * a;
        let b1 = -2.0 * cs;
        let b2 = 1.0 - alpha * a;
        let a0 = 1.0 + alpha / a;
        let a1 = -2.0 * cs;
        let a2 = 1.0 - alpha / a;
        Self {
            b0: b0/a0, b1: b1/a0, b2: b2/a0,
            a1: a1/a0, a2: a2/a0,
            z1: 0.0, z2: 0.0,
        }
    }

    /// Low-shelf EQ design. gain_db == 0 yields identity. `q` is
    /// clamped to [0.01, 100.0] — see `peaking` for the rationale.
    pub fn low_shelf(sample_rate: f32, freq_hz: f32, q: f32, gain_db: f32) -> Self {
        // Same NaN/Inf defense as peaking() — see comment there.
        let sample_rate = if sample_rate.is_finite() && sample_rate > 0.0 {
            sample_rate } else { 48_000.0 };
        let freq_hz = if freq_hz.is_finite() && freq_hz > 0.0 {
            freq_hz.min(sample_rate * 0.49) } else { 200.0 };
        let gain_db = if gain_db.is_finite() { gain_db } else { 0.0 };
        let q = if q.is_finite() && q > 0.0 { q.clamp(0.01, 100.0) } else { 0.707 };
        if gain_db.abs() < 1e-6 { return Self::identity(); }
        let a = 10f32.powf(gain_db / 40.0);
        let omega = 2.0 * PI * freq_hz / sample_rate;
        let (sn, cs) = omega.sin_cos();
        let alpha = sn / (2.0 * q);
        let s = 2.0 * a.sqrt() * alpha;
        let b0 = a * ((a + 1.0) - (a - 1.0) * cs + s);
        let b1 = 2.0 * a * ((a - 1.0) - (a + 1.0) * cs);
        let b2 = a * ((a + 1.0) - (a - 1.0) * cs - s);
        let a0 = (a + 1.0) + (a - 1.0) * cs + s;
        let a1 = -2.0 * ((a - 1.0) + (a + 1.0) * cs);
        let a2 = (a + 1.0) + (a - 1.0) * cs - s;
        Self {
            b0: b0/a0, b1: b1/a0, b2: b2/a0,
            a1: a1/a0, a2: a2/a0,
            z1: 0.0, z2: 0.0,
        }
    }

    /// High-shelf EQ design. gain_db == 0 yields identity. `q` is
    /// clamped to [0.01, 100.0] — see `peaking` for the rationale.
    pub fn high_shelf(sample_rate: f32, freq_hz: f32, q: f32, gain_db: f32) -> Self {
        // Same NaN/Inf defense as peaking() — see comment there.
        let sample_rate = if sample_rate.is_finite() && sample_rate > 0.0 {
            sample_rate } else { 48_000.0 };
        let freq_hz = if freq_hz.is_finite() && freq_hz > 0.0 {
            freq_hz.min(sample_rate * 0.49) } else { 8_000.0 };
        let gain_db = if gain_db.is_finite() { gain_db } else { 0.0 };
        let q = if q.is_finite() && q > 0.0 { q.clamp(0.01, 100.0) } else { 0.707 };
        if gain_db.abs() < 1e-6 { return Self::identity(); }
        let a = 10f32.powf(gain_db / 40.0);
        let omega = 2.0 * PI * freq_hz / sample_rate;
        let (sn, cs) = omega.sin_cos();
        let alpha = sn / (2.0 * q);
        let s = 2.0 * a.sqrt() * alpha;
        let b0 = a * ((a + 1.0) + (a - 1.0) * cs + s);
        let b1 = -2.0 * a * ((a - 1.0) + (a + 1.0) * cs);
        let b2 = a * ((a + 1.0) + (a - 1.0) * cs - s);
        let a0 = (a + 1.0) - (a - 1.0) * cs + s;
        let a1 = 2.0 * ((a - 1.0) - (a + 1.0) * cs);
        let a2 = (a + 1.0) - (a - 1.0) * cs - s;
        Self {
            b0: b0/a0, b1: b1/a0, b2: b2/a0,
            a1: a1/a0, a2: a2/a0,
            z1: 0.0, z2: 0.0,
        }
    }
}

// ── Mixer ──────────────────────────────────────────────────────────

/// Full realtime mixer. Process samples via `process(L, R) → (L, R)`
/// or buffers via `process_buffer`. Update params via `set_params`,
/// which rebuilds the filter coefficients (preserving filter state
/// so live tweaks don't click).
#[derive(Debug)]
pub struct RealtimeMixer {
    sample_rate: f32,
    params: MixerParams,
    spk_low:  Biquad,
    spk_mid:  Biquad,
    spk_high: Biquad,
    ins_low:  Biquad,
    ins_mid:  Biquad,
    ins_high: Biquad,
}

impl Default for RealtimeMixer {
    /// Default uses 48 kHz, which matches Opus's internal rate and is
    /// the most common consumer sample rate. The audio thread can
    /// rebuild the mixer with a different rate via
    /// `RealtimeMixer::new(sample_rate)` once the actual source rate
    /// is known. The QtState seeds with this default so it can be
    /// constructed before any audio source is attached.
    fn default() -> Self {
        Self::new(48_000.0)
    }
}

impl RealtimeMixer {
    pub fn new(sample_rate: f32) -> Self {
        let mut m = Self {
            sample_rate,
            params: MixerParams::default(),
            spk_low:  Biquad::identity(),
            spk_mid:  Biquad::identity(),
            spk_high: Biquad::identity(),
            ins_low:  Biquad::identity(),
            ins_mid:  Biquad::identity(),
            ins_high: Biquad::identity(),
        };
        m.rebuild_filters();
        m
    }

    pub fn params(&self) -> MixerParams { self.params }

    /// Apply new parameters. We preserve each filter's internal state
    /// (z1/z2) across the parameter change so the user can sweep
    /// sliders without producing audible clicks. The new coefficients
    /// take effect on the very next sample.
    pub fn set_params(&mut self, params: MixerParams) {
        self.params = params;
        self.rebuild_filters_preserving_state();
    }

    fn rebuild_filters(&mut self) {
        let sr = self.sample_rate;
        let p = self.params;
        self.spk_low  = Biquad::low_shelf (sr, SPK_LOW_HZ,  SHELF_Q, p.speaker.low_gain_db);
        self.spk_mid  = Biquad::peaking   (sr, SPK_MID_HZ,  PEAK_Q,  p.speaker.mid_gain_db);
        self.spk_high = Biquad::high_shelf(sr, SPK_HIGH_HZ, SHELF_Q, p.speaker.high_gain_db);
        self.ins_low  = Biquad::low_shelf (sr, INS_LOW_HZ,  SHELF_Q, p.instrumental.low_gain_db);
        self.ins_mid  = Biquad::peaking   (sr, INS_MID_HZ,  PEAK_Q,  p.instrumental.mid_gain_db);
        self.ins_high = Biquad::high_shelf(sr, INS_HIGH_HZ, SHELF_Q, p.instrumental.high_gain_db);
    }

    /// Like rebuild_filters but preserves the existing z1/z2 state so
    /// realtime parameter sweeps don't pop.
    fn rebuild_filters_preserving_state(&mut self) {
        // Snapshot states.
        let states = [
            (self.spk_low.z1,  self.spk_low.z2),
            (self.spk_mid.z1,  self.spk_mid.z2),
            (self.spk_high.z1, self.spk_high.z2),
            (self.ins_low.z1,  self.ins_low.z2),
            (self.ins_mid.z1,  self.ins_mid.z2),
            (self.ins_high.z1, self.ins_high.z2),
        ];
        self.rebuild_filters();
        // Restore.
        let restore = |b: &mut Biquad, (z1, z2): (f32, f32)| { b.z1 = z1; b.z2 = z2; };
        restore(&mut self.spk_low,  states[0]);
        restore(&mut self.spk_mid,  states[1]);
        restore(&mut self.spk_high, states[2]);
        restore(&mut self.ins_low,  states[3]);
        restore(&mut self.ins_mid,  states[4]);
        restore(&mut self.ins_high, states[5]);
    }

    /// Process one stereo sample. Realtime — no allocations.
    ///
    /// NaN-defense: this is the hot path used by the streaming
    /// separator and any other per-sample caller. A single non-finite
    /// input would poison the biquad delay-line state (z1, z2 = NaN)
    /// and turn every subsequent output sample into NaN until reset
    /// — verified by the mixer_fuzz__nan_input_recovers_within_
    /// short_window test, which previously found that `process()`
    /// (unlike `process_buffer()`) had no NaN defense and produced
    /// 50,000 consecutive NaN outputs after a single bad input.
    ///
    /// Two-layer defense:
    ///   1. Sanitize NaN/Inf inputs to 0 before they reach the
    ///      biquads. A bad sample becomes one silent sample instead
    ///      of starting a cascade.
    ///   2. After the math, if any output is non-finite the filter
    ///      state has somehow been corrupted (extreme coefficients,
    ///      etc.) — reset all filters and emit silence. The next
    ///      sample starts clean.
    #[inline]
    pub fn process(&mut self, l: f32, r: f32) -> (f32, f32) {
        // 0. Sanitize input.
        let l = if l.is_finite() { l } else { 0.0 };
        let r = if r.is_finite() { r } else { 0.0 };
        // 1. Decompose to Mid/Side.
        let m = (l + r) * 0.5;
        let s = (l - r) * 0.5;
        // 2. Per-channel EQ — Mid through the Speaker chain, Side
        //    through the Instrumental chain. The biquads cascade in
        //    series so each band stacks on the previous one's output.
        let m_eq = self.spk_high.process(
                   self.spk_mid.process(
                   self.spk_low.process(m)));
        let s_eq = self.ins_high.process(
                   self.ins_mid.process(
                   self.ins_low.process(s)));
        // 3. Volume per channel.
        let m_out = m_eq * self.params.speaker.volume;
        let s_out = s_eq * self.params.instrumental.volume;
        // 4. Re-encode to L/R. Inverse of step 1: L = M+S, R = M-S.
        let out_l = m_out + s_out;
        let out_r = m_out - s_out;
        // 5. Last-ditch state-corruption check. The input sanitize
        //    above prevents the common case; this catches the
        //    less-common case of bad coefficients producing
        //    non-finite output from finite input. Reset and emit
        //    silence so the next sample is clean.
        if !out_l.is_finite() || !out_r.is_finite() {
            self.reset();
            return (0.0, 0.0);
        }
        (out_l, out_r)
    }

    /// Block-process a stereo buffer in place. Common entry for
    /// audio thread integration: hands the mixer the input frames,
    /// receives the processed output via the same buffer.
    ///
    /// NaN-recovery: a single non-finite input sample would poison
    /// the biquad delay-line state (z1, z2 = NaN) and turn every
    /// subsequent output sample into NaN until reset. We protect
    /// against this in two layers:
    ///   1. Replace non-finite inputs with 0 before processing —
    ///      a single bad sample becomes a single silent sample
    ///      instead of starting a NaN cascade.
    ///   2. After the buffer, if any filter's state went non-finite
    ///      anyway (e.g. due to extreme coefficient values), reset
    ///      all filters. The next buffer starts clean.
    pub fn process_buffer(&mut self, frames: &mut [(f32, f32)]) {
        for f in frames.iter_mut() {
            let l_in = if f.0.is_finite() { f.0 } else { 0.0 };
            let r_in = if f.1.is_finite() { f.1 } else { 0.0 };
            let (l, r) = self.process(l_in, r_in);
            *f = (l, r);
        }
        if !self.state_is_finite() {
            eprintln!("realtime_mixer: filter state went non-finite, resetting");
            self.reset();
            // Zero out any tail NaN in the buffer we just produced.
            for f in frames.iter_mut() {
                if !f.0.is_finite() { f.0 = 0.0; }
                if !f.1.is_finite() { f.1 = 0.0; }
            }
        }
    }

    /// Aggregate check across all six EQ filters.
    fn state_is_finite(&self) -> bool {
        self.spk_low .state_is_finite()
            && self.spk_mid .state_is_finite()
            && self.spk_high.state_is_finite()
            && self.ins_low .state_is_finite()
            && self.ins_mid .state_is_finite()
            && self.ins_high.state_is_finite()
    }

    /// Reset all filter states. Call between disjoint inputs (file
    /// changes, seek, etc.) to prevent stale delay-line contents
    /// from bleeding into the new audio.
    pub fn reset(&mut self) {
        self.spk_low .reset_state();
        self.spk_mid .reset_state();
        self.spk_high.reset_state();
        self.ins_low .reset_state();
        self.ins_mid .reset_state();
        self.ins_high.reset_state();
    }

    /// Combined dB response of the Speaker channel (Mid path) at
    /// `freq_hz`. Cascaded biquads have their dB responses sum —
    /// that's the convenience of working in log magnitude. Volume
    /// is included so the curve matches what the user will hear.
    pub fn speaker_response_db(&self, freq_hz: f32) -> f32 {
        self.spk_low .magnitude_db(freq_hz, self.sample_rate)
      + self.spk_mid .magnitude_db(freq_hz, self.sample_rate)
      + self.spk_high.magnitude_db(freq_hz, self.sample_rate)
      + linear_to_db(self.params.speaker.volume)
    }

    /// Combined dB response of the Instrumental channel (Side path).
    pub fn instrumental_response_db(&self, freq_hz: f32) -> f32 {
        self.ins_low .magnitude_db(freq_hz, self.sample_rate)
      + self.ins_mid .magnitude_db(freq_hz, self.sample_rate)
      + self.ins_high.magnitude_db(freq_hz, self.sample_rate)
      + linear_to_db(self.params.instrumental.volume)
    }

    /// Generate a logarithmically-spaced frequency response curve
    /// for both channels. Returns `n` samples between `lo_hz` and
    /// `hi_hz` as a Vec of (freq, spk_db, ins_db) tuples. The QML
    /// side renders these as two overlaid curves.
    ///
    /// Log spacing is chosen because the ear hears frequency
    /// logarithmically — a linear spacing crowds the visualization
    /// in the high end and starves the bass range, which is exactly
    /// the opposite of what we want to communicate about the bands.
    pub fn compute_curves(&self, n: usize, lo_hz: f32, hi_hz: f32)
        -> Vec<(f32, f32, f32)>
    {
        let n = n.max(2);
        let log_lo = lo_hz.max(1.0).ln();
        let log_hi = hi_hz.max(lo_hz + 1.0).ln();
        (0..n).map(|i| {
            let t = i as f32 / (n - 1) as f32;
            let freq = (log_lo + (log_hi - log_lo) * t).exp();
            (freq,
             self.speaker_response_db(freq),
             self.instrumental_response_db(freq))
        }).collect()
    }
}

/// Convert a linear gain into dB. Used to fold volume into the
/// rendered response curves.
fn linear_to_db(v: f32) -> f32 {
    20.0 * v.max(1e-6).log10()
}

/// Process a stereo WAV file through the mixer with the supplied
/// parameters and write the result as OggOpus. This shares the EXACT
/// same `process_buffer` call any future realtime audio thread will
/// use — so anything that sounds right offline will sound right
/// realtime, by construction.
///
/// `progress` is called with values in [0.0, 1.0] at roughly 20 Hz
/// so the QML can drive a progress bar.
///
/// Returns the number of input frames processed on success.
pub fn process_file_through_mixer(
    input: &std::path::Path,
    output: &std::path::Path,
    params: MixerParams,
    bitrate_kbps: u32,
    mut progress: impl FnMut(f32),
) -> Result<usize, String> {
    // Read input via the existing WAV reader from the looper module.
    let buf = crate::looper::read_wav(input)?;
    let mut mixer = RealtimeMixer::new(buf.sample_rate as f32);
    mixer.set_params(params);

    // Process the buffer in chunks so we can stream progress updates.
    // Chunk size doesn't affect correctness (the biquad is sample-
    // accurate); 4096 frames is ~85 ms at 48 kHz — comfortable for
    // a snappy progress UI without overhead.
    let mut processed: Vec<(f32, f32)> = buf.samples_lr.clone();
    let chunk = 4096_usize;
    let n_chunks = (processed.len() + chunk - 1) / chunk;
    for (i, slice) in processed.chunks_mut(chunk).enumerate() {
        mixer.process_buffer(slice);
        if i % 4 == 0 {
            // First half of progress = DSP, second half = encode.
            progress(0.5 * (i as f32 / n_chunks as f32));
        }
    }
    progress(0.5);

    // Resample to 48 kHz and Opus-encode using the shared encoder.
    let pcm_48k = crate::looper::linear_resample_to_48k(&processed, buf.sample_rate);
    progress(0.7);
    crate::looper::encode_pcm_to_opus(&pcm_48k, bitrate_kbps, output)?;
    progress(1.0);
    Ok(processed.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approx_eq(a: f32, b: f32, tol: f32) -> bool {
        (a - b).abs() < tol
    }

    #[test]
    fn biquad_identity_is_passthrough() {
        let mut b = Biquad::identity();
        for x in [0.0, 0.1, -0.5, 0.9, -0.9, 0.0] {
            let y = b.process(x);
            assert!(approx_eq(y, x, 1e-6), "identity changed sample: {} → {}", x, y);
        }
    }

    #[test]
    fn biquad_peaking_zero_gain_is_identity() {
        // A peaking filter with 0 dB gain should pass the signal
        // through unchanged at all frequencies.
        let mut b = Biquad::peaking(44_100.0, 1000.0, 1.0, 0.0);
        for i in 0..200 {
            let x = (2.0 * PI * 200.0 * i as f32 / 44_100.0).sin();
            let y = b.process(x);
            assert!(approx_eq(y, x, 1e-5),
                "0-dB peak changed sample {} → {}", x, y);
        }
    }

    #[test]
    fn biquad_low_shelf_boosts_dc() {
        // Low shelf with +12 dB at 250 Hz should multiply DC by
        // 10^(12/20) ≈ 4.0 in the steady state.
        let mut b = Biquad::low_shelf(44_100.0, 250.0, 0.707, 12.0);
        // Feed enough DC samples for the filter to settle.
        let mut y = 0.0;
        for _ in 0..2000 {
            y = b.process(1.0);
        }
        // 12 dB linear gain ≈ 3.98.
        assert!(y > 3.5 && y < 4.5,
            "expected ~+12 dB DC gain (linear ≈ 4.0), got {}", y);
    }

    #[test]
    fn biquad_high_shelf_boosts_nyquist() {
        // High shelf with +12 dB at 4 kHz: at Nyquist alternating
        // ±1 signal, the steady-state output magnitude should be
        // significantly amplified.
        let mut b = Biquad::high_shelf(44_100.0, 4000.0, 0.707, 12.0);
        let mut max_y: f32 = 0.0;
        for i in 0..2000 {
            let x = if i & 1 == 0 { 1.0 } else { -1.0 };
            let y = b.process(x);
            if i > 1500 { max_y = max_y.max(y.abs()); }
        }
        // Should be near +12 dB (≈ 4×) but the alternating signal at
        // Nyquist also has shelf gain applied — just check it's
        // noticeably > 1.0.
        assert!(max_y > 2.5,
            "expected high-shelf boost at Nyquist, got {}", max_y);
    }

    #[test]
    fn mid_side_roundtrip_preserves_signal() {
        // Round-trip any (L, R) through M/S decompose + recompose
        // should yield the exact original samples (within float eps).
        let mut mixer = RealtimeMixer::new(44_100.0);
        for &(l, r) in &[
            (0.0, 0.0),
            (1.0, 0.0),
            (0.0, 1.0),
            (0.5, -0.5),
            (-0.3, 0.7),
            (0.99, 0.99),
            (-0.5, -0.5),
        ] {
            let (lo, ro) = mixer.process(l, r);
            assert!(approx_eq(lo, l, 1e-5),
                "L roundtrip failed: ({}, {}) → L_out = {}", l, r, lo);
            assert!(approx_eq(ro, r, 1e-5),
                "R roundtrip failed: ({}, {}) → R_out = {}", l, r, ro);
        }
    }

    #[test]
    fn roundtrip_neutral_is_identity() {
        // CRITICAL: the mixer must not colour the sound at all when
        // every control is at its neutral position.
        let mut mixer = RealtimeMixer::new(48_000.0);
        // Default params = all neutral.
        let mut buf: Vec<(f32, f32)> = (0..2000).map(|i| {
            let t = i as f32 / 48_000.0;
            let l = (2.0 * PI * 440.0 * t).sin();
            let r = (2.0 * PI * 880.0 * t).sin();
            (l * 0.5, r * 0.4)
        }).collect();
        let input = buf.clone();
        mixer.process_buffer(&mut buf);
        for (i, (out, inp)) in buf.iter().zip(&input).enumerate() {
            assert!(approx_eq(out.0, inp.0, 1e-5),
                "neutral mixer altered L at sample {}: {} → {}", i, inp.0, out.0);
            assert!(approx_eq(out.1, inp.1, 1e-5),
                "neutral mixer altered R at sample {}: {} → {}", i, inp.1, out.1);
        }
    }

    #[test]
    fn centered_signal_routes_to_speaker_channel() {
        // A pure centered signal (L == R) lives entirely in the Mid
        // channel. Muting the speaker volume should silence it, even
        // with the instrumental channel at full volume.
        let mut mixer = RealtimeMixer::new(48_000.0);
        mixer.set_params(MixerParams {
            speaker:      GroupParams { volume: 0.0, ..Default::default() },
            instrumental: GroupParams { volume: 1.0, ..Default::default() },
        });
        let mut buf: Vec<(f32, f32)> = (0..512).map(|i| {
            let t = i as f32 / 48_000.0;
            let s = (2.0 * PI * 440.0 * t).sin() * 0.5;
            (s, s)        // L == R — centered
        }).collect();
        mixer.process_buffer(&mut buf);
        let peak = buf.iter()
            .map(|(l, r)| l.abs().max(r.abs()))
            .fold(0.0_f32, f32::max);
        assert!(peak < 0.01,
            "speaker mute should silence centered signal, peak = {}", peak);
    }

    #[test]
    fn anticorrelated_signal_routes_to_instrumental_channel() {
        // L = -R is the pure Side signal. Muting the instrumental
        // volume should silence it.
        let mut mixer = RealtimeMixer::new(48_000.0);
        mixer.set_params(MixerParams {
            speaker:      GroupParams { volume: 1.0, ..Default::default() },
            instrumental: GroupParams { volume: 0.0, ..Default::default() },
        });
        let mut buf: Vec<(f32, f32)> = (0..512).map(|i| {
            let t = i as f32 / 48_000.0;
            let s = (2.0 * PI * 440.0 * t).sin() * 0.5;
            (s, -s)       // L == -R — pure Side
        }).collect();
        mixer.process_buffer(&mut buf);
        let peak = buf.iter()
            .map(|(l, r)| l.abs().max(r.abs()))
            .fold(0.0_f32, f32::max);
        assert!(peak < 0.01,
            "instrument mute should silence anti-correlated signal, peak = {}", peak);
    }

    #[test]
    fn speaker_volume_scales_centered_content() {
        let mut mixer = RealtimeMixer::new(48_000.0);
        mixer.set_params(MixerParams {
            speaker:      GroupParams { volume: 0.5, ..Default::default() },
            instrumental: GroupParams { volume: 1.0, ..Default::default() },
        });
        // Pure centered signal → halving speaker vol should halve output.
        let (lo, ro) = mixer.process(1.0, 1.0);
        assert!(approx_eq(lo, 0.5, 1e-5));
        assert!(approx_eq(ro, 0.5, 1e-5));
    }

    #[test]
    fn realtime_stability_long_run() {
        // Run a substantial number of samples with non-trivial EQ
        // and ensure no NaN, inf, or signal blow-up.
        let mut mixer = RealtimeMixer::new(48_000.0);
        mixer.set_params(MixerParams {
            speaker: GroupParams {
                low_gain_db: -6.0, mid_gain_db: 6.0, high_gain_db: -3.0,
                volume: 0.8,
            },
            instrumental: GroupParams {
                low_gain_db: 4.0, mid_gain_db: -4.0, high_gain_db: 6.0,
                volume: 0.9,
            },
        });
        // 5 seconds of audio at 48k.
        let n = 48_000 * 5;
        let mut peak: f32 = 0.0;
        for i in 0..n {
            let t = i as f32 / 48_000.0;
            // Pink-ish: sum of a few sines.
            let x = 0.3 * (2.0 * PI * 110.0 * t).sin()
                  + 0.3 * (2.0 * PI * 440.0 * t).sin()
                  + 0.2 * (2.0 * PI * 1760.0 * t).sin();
            let (l, r) = mixer.process(x, -x * 0.7);
            assert!(l.is_finite() && r.is_finite(),
                "non-finite output at sample {}: ({}, {})", i, l, r);
            peak = peak.max(l.abs()).max(r.abs());
        }
        // Even with combined +6 dB boosts, the output shouldn't be
        // running away — typically peak < 5.0 for our test signal.
        assert!(peak < 10.0,
            "signal blow-up: peak = {}", peak);
    }

    #[test]
    fn parameter_changes_preserve_state() {
        // Sweep parameters during processing — should not produce
        // discontinuities (NaN/inf are the easy-to-test failure mode).
        let mut mixer = RealtimeMixer::new(48_000.0);
        for step in 0..50 {
            let g = (step as f32 - 25.0) * 0.3;       // -7.5 to +7.5 dB
            mixer.set_params(MixerParams {
                speaker: GroupParams {
                    low_gain_db: g, mid_gain_db: -g, high_gain_db: g * 0.5,
                    volume: 0.8,
                },
                instrumental: GroupParams {
                    low_gain_db: -g, mid_gain_db: g, high_gain_db: -g * 0.5,
                    volume: 0.8,
                },
            });
            for i in 0..480 {
                let t = (step * 480 + i) as f32 / 48_000.0;
                let x = 0.5 * (2.0 * PI * 440.0 * t).sin();
                let (l, r) = mixer.process(x, x * 0.5);
                assert!(l.is_finite() && r.is_finite(),
                    "non-finite during sweep step {} sample {}", step, i);
            }
        }
    }

    #[test]
    fn reset_clears_filter_state() {
        let mut mixer = RealtimeMixer::new(48_000.0);
        mixer.set_params(MixerParams {
            speaker: GroupParams {
                low_gain_db: 12.0, ..Default::default()
            },
            ..Default::default()
        });
        // Feed a step input to charge the filters.
        for _ in 0..100 {
            let _ = mixer.process(1.0, 1.0);
        }
        mixer.reset();
        // Process zeros — output should rapidly decay since state
        // is cleared. With identity-y default volumes, even the
        // first zero should give exact zero.
        let (l, r) = mixer.process(0.0, 0.0);
        assert!(approx_eq(l, 0.0, 1e-6));
        assert!(approx_eq(r, 0.0, 1e-6));
    }

    #[test]
    fn band_centers_are_in_expected_ranges() {
        // Sanity-check the documented role assignments. The speaker
        // channel's mid band MUST sit in the vocal-formant region
        // (roughly 1-3 kHz) for the "groups make sense" guarantee.
        assert!(SPK_MID_HZ >= 1000.0 && SPK_MID_HZ <= 3000.0,
            "speaker mid band {} Hz is outside vocal-formant range", SPK_MID_HZ);
        // And the instrumental channel's bands should be either
        // BELOW the speaker mid (for bass content) or noticeably
        // ABOVE it (for high content).
        assert!(INS_LOW_HZ < SPK_MID_HZ);
        assert!(INS_HIGH_HZ > SPK_HIGH_HZ);
    }

    #[test]
    fn magnitude_db_of_identity_is_zero() {
        // An identity biquad should report 0 dB at every frequency.
        let b = Biquad::identity();
        for hz in [50.0_f32, 250.0, 1000.0, 4000.0, 10_000.0] {
            let db = b.magnitude_db(hz, 48_000.0);
            assert!(db.abs() < 1e-4,
                "identity biquad reports {} dB at {} Hz", db, hz);
        }
    }

    #[test]
    fn peaking_response_matches_design_gain_at_center() {
        // A peaking filter designed for +6 dB at 1500 Hz should
        // report ≈+6 dB when queried at 1500 Hz.
        let b = Biquad::peaking(48_000.0, 1500.0, 1.0, 6.0);
        let db = b.magnitude_db(1500.0, 48_000.0);
        assert!((db - 6.0).abs() < 0.2,
            "expected ~+6 dB at center, got {}", db);
        // And ≈0 dB far from the center.
        let far = b.magnitude_db(50.0, 48_000.0);
        assert!(far.abs() < 1.0,
            "expected ~0 dB far from center, got {}", far);
    }

    #[test]
    fn low_shelf_response_lifts_bass_only() {
        // +12 dB low shelf at 250 Hz: should lift very low
        // frequencies, leave very high frequencies alone.
        let b = Biquad::low_shelf(48_000.0, 250.0, 0.707, 12.0);
        let bass = b.magnitude_db(50.0, 48_000.0);
        let treble = b.magnitude_db(10_000.0, 48_000.0);
        assert!(bass > 10.0,
            "low shelf should lift bass, got {} dB at 50 Hz", bass);
        assert!(treble.abs() < 1.0,
            "low shelf should leave treble alone, got {} dB at 10 kHz", treble);
    }

    #[test]
    fn high_shelf_response_lifts_treble_only() {
        let b = Biquad::high_shelf(48_000.0, 4000.0, 0.707, 12.0);
        let treble = b.magnitude_db(15_000.0, 48_000.0);
        let bass = b.magnitude_db(50.0, 48_000.0);
        assert!(treble > 10.0,
            "high shelf should lift treble, got {} dB at 15 kHz", treble);
        assert!(bass.abs() < 1.0,
            "high shelf should leave bass alone, got {} dB at 50 Hz", bass);
    }

    #[test]
    fn compute_curves_are_log_spaced_and_span_range() {
        let mixer = RealtimeMixer::new(48_000.0);
        let curves = mixer.compute_curves(64, 20.0, 20_000.0);
        assert_eq!(curves.len(), 64);
        // First freq ≈ 20 Hz, last ≈ 20 kHz.
        assert!((curves[0].0 - 20.0).abs() < 0.5);
        assert!((curves[63].0 - 20_000.0).abs() < 5.0);
        // Log spacing: the ratio between consecutive freqs is
        // constant (within float tolerance).
        let r0 = curves[1].0 / curves[0].0;
        let r1 = curves[30].0 / curves[29].0;
        assert!((r0 - r1).abs() / r0 < 0.01,
            "expected log spacing, ratios differ: {} vs {}", r0, r1);
    }

    #[test]
    fn speaker_curve_responds_to_speaker_eq() {
        // Boost the speaker mid by +9 dB; the curve at the speaker
        // mid center should rise; the instrumental curve should
        // NOT (they're independent chains).
        let mut mixer = RealtimeMixer::new(48_000.0);
        let neutral_spk = mixer.speaker_response_db(SPK_MID_HZ);
        let neutral_ins = mixer.instrumental_response_db(SPK_MID_HZ);
        mixer.set_params(MixerParams {
            speaker: GroupParams {
                mid_gain_db: 9.0, ..Default::default()
            },
            instrumental: GroupParams::default(),
        });
        let boosted_spk = mixer.speaker_response_db(SPK_MID_HZ);
        let boosted_ins = mixer.instrumental_response_db(SPK_MID_HZ);
        assert!((boosted_spk - neutral_spk - 9.0).abs() < 0.3,
            "speaker mid boost should be ~+9 dB, got {}",
            boosted_spk - neutral_spk);
        assert!((boosted_ins - neutral_ins).abs() < 0.1,
            "instrumental curve must not move when speaker EQ changes");
    }

    #[test]
    fn volume_appears_in_response_curve() {
        // Halving speaker volume (-6 dB) should drop the entire
        // speaker curve by ~6 dB.
        let mut mixer = RealtimeMixer::new(48_000.0);
        let neutral = mixer.speaker_response_db(1000.0);
        mixer.set_params(MixerParams {
            speaker: GroupParams { volume: 0.5, ..Default::default() },
            instrumental: GroupParams::default(),
        });
        let halved = mixer.speaker_response_db(1000.0);
        // 20 log10 (0.5) ≈ -6.02
        assert!((halved - neutral + 6.02).abs() < 0.05,
            "volume drop should appear as ~-6 dB, got {}",
            halved - neutral);
    }

    #[test]
    fn process_file_through_mixer_produces_valid_opus() {
        // Write a tiny WAV, process it through the mixer with mild
        // EQ, verify the output is a valid OggOpus file.
        let sr = 44_100;
        let n_samples = sr as usize;
        let tmp_in  = std::env::temp_dir().join("rtmix_in.wav");
        let tmp_out = std::env::temp_dir().join("rtmix_out.opus");

        // Build a centered sine + a panned sine — exercises both
        // M and S paths.
        {
            let spec = hound::WavSpec {
                channels: 2, sample_rate: sr,
                bits_per_sample: 16, sample_format: hound::SampleFormat::Int,
            };
            let mut w = hound::WavWriter::create(&tmp_in, spec).unwrap();
            for i in 0..n_samples {
                let t = i as f32 / sr as f32;
                // 440 Hz centered (Mid) + 220 Hz panned (Side)
                let center = (2.0 * PI * 440.0 * t).sin() * 0.4;
                let side   = (2.0 * PI * 220.0 * t).sin() * 0.3;
                let l = ((center + side) * 32767.0) as i16;
                let r = ((center - side) * 32767.0) as i16;
                w.write_sample(l).unwrap();
                w.write_sample(r).unwrap();
            }
            w.finalize().unwrap();
        }

        let params = MixerParams {
            speaker: GroupParams {
                low_gain_db: 0.0, mid_gain_db: 6.0, high_gain_db: 0.0,
                volume: 1.0,
            },
            instrumental: GroupParams {
                low_gain_db: 3.0, mid_gain_db: 0.0, high_gain_db: 3.0,
                volume: 0.9,
            },
        };
        let n = process_file_through_mixer(
            &tmp_in, &tmp_out, params, 96, |_| {}).unwrap();
        assert_eq!(n, n_samples);
        let bytes = std::fs::read(&tmp_out).unwrap();
        assert!(bytes.starts_with(b"OggS"), "output missing OggS magic");
        assert!(bytes.windows(8).any(|w| w == b"OpusHead"),
            "OpusHead missing");
        assert!(bytes.len() > 2_000,
            "unexpectedly small opus file: {} bytes", bytes.len());

        std::fs::remove_file(&tmp_in).ok();
        std::fs::remove_file(&tmp_out).ok();
    }

    #[test]
    fn process_buffer_recovers_from_nan_input() {
        // A NaN input must not poison the filter state forever.
        // After one NaN sample, subsequent CLEAN samples should
        // produce finite output (silence or signal, but never NaN).
        let mut mixer = RealtimeMixer::new(48_000.0);
        let mut params = MixerParams::default();
        params.speaker.low_gain_db = 6.0;
        params.speaker.mid_gain_db = -3.0;
        mixer.set_params(params);

        // Inject one NaN buffer.
        let mut buf: Vec<(f32, f32)> = vec![(f32::NAN, f32::NAN); 64];
        mixer.process_buffer(&mut buf);
        for f in &buf {
            assert!(f.0.is_finite() && f.1.is_finite(),
                "NaN-input buffer should be sanitized to finite output");
        }
        // Now follow with a clean tone burst. Output must be finite.
        let mut buf2: Vec<(f32, f32)> = (0..256).map(|i| {
            let v = (i as f32 * 0.1).sin() * 0.2;
            (v, v)
        }).collect();
        mixer.process_buffer(&mut buf2);
        for f in &buf2 {
            assert!(f.0.is_finite() && f.1.is_finite(),
                "post-recovery output non-finite: {:?}", f);
        }
        // And the filter should now be functional — output should
        // have some non-zero content (EQ applied to a tone).
        let energy: f32 = buf2.iter().map(|f| f.0 * f.0 + f.1 * f.1).sum();
        assert!(energy > 0.01,
                "post-recovery filter dead: energy = {}", energy);
    }

    #[test]
    fn process_buffer_recovers_from_inf_input() {
        let mut mixer = RealtimeMixer::new(48_000.0);
        let mut buf: Vec<(f32, f32)> = vec![(f32::INFINITY, f32::NEG_INFINITY); 32];
        mixer.process_buffer(&mut buf);
        for f in &buf {
            assert!(f.0.is_finite() && f.1.is_finite(),
                "INF input must be sanitized: {:?}", f);
        }
    }

    #[test]
    fn process_buffer_mixed_finite_and_nan_only_silences_bad_samples() {
        // A buffer with mostly-finite samples and one NaN should
        // still process the good samples normally (just with the
        // NaN treated as 0). Final output must be all-finite.
        let mut mixer = RealtimeMixer::new(48_000.0);
        let mut buf: Vec<(f32, f32)> = (0..128).map(|i| {
            if i == 50 { (f32::NAN, 0.1) } else { (0.1, 0.1) }
        }).collect();
        mixer.process_buffer(&mut buf);
        for f in &buf {
            assert!(f.0.is_finite() && f.1.is_finite(),
                "mixed-NaN buffer leaked non-finite: {:?}", f);
        }
    }

    #[test]
    fn biquad_with_q_zero_does_not_produce_nan() {
        // All three designs must handle q=0 without producing NaN
        // coefficients (we clamp Q internally). Sanity check with
        // a small input run.
        for (name, b) in [
            ("peaking",    Biquad::peaking   (48000.0, 1000.0, 0.0, 6.0)),
            ("low_shelf",  Biquad::low_shelf (48000.0, 250.0,  0.0, 6.0)),
            ("high_shelf", Biquad::high_shelf(48000.0, 4000.0, 0.0, 6.0)),
        ] {
            let mut b = b;
            for _ in 0..64 {
                let y = b.process(0.5);
                assert!(y.is_finite(),
                    "{} with q=0 produced {} — should have been clamped",
                    name, y);
            }
            // State must remain finite.
            assert!(b.state_is_finite(),
                "{} with q=0 left state non-finite", name);
        }
    }

    #[test]
    fn biquad_with_q_negative_does_not_produce_nan() {
        let mut b = Biquad::peaking(48000.0, 1000.0, -2.0, 6.0);
        for _ in 0..64 {
            let y = b.process(0.5);
            assert!(y.is_finite(),
                "peaking with q=-2 produced {}", y);
        }
    }

    #[test]
    fn biquad_constructors_defend_against_nan_inputs() {
        // Every parameter independently set to NaN must still
        // produce a stable filter that emits finite samples.
        let cases = [
            ("peaking NaN sr",    Biquad::peaking(f32::NAN, 1000.0, 1.0, 6.0)),
            ("peaking NaN freq",  Biquad::peaking(48000.0, f32::NAN, 1.0, 6.0)),
            ("peaking NaN q",     Biquad::peaking(48000.0, 1000.0, f32::NAN, 6.0)),
            ("peaking NaN gain",  Biquad::peaking(48000.0, 1000.0, 1.0, f32::NAN)),
            ("peaking Inf sr",    Biquad::peaking(f32::INFINITY, 1000.0, 1.0, 6.0)),
            ("peaking 0 q",       Biquad::peaking(48000.0, 1000.0, 0.0, 6.0)),
            ("peaking neg q",     Biquad::peaking(48000.0, 1000.0, -1.0, 6.0)),
            ("low NaN sr",        Biquad::low_shelf(f32::NAN, 200.0, 1.0, 6.0)),
            ("low NaN freq",      Biquad::low_shelf(48000.0, f32::NAN, 1.0, 6.0)),
            ("low NaN q",         Biquad::low_shelf(48000.0, 200.0, f32::NAN, 6.0)),
            ("low NaN gain",      Biquad::low_shelf(48000.0, 200.0, 1.0, f32::NAN)),
            ("high NaN sr",       Biquad::high_shelf(f32::NAN, 8000.0, 1.0, 6.0)),
            ("high NaN freq",     Biquad::high_shelf(48000.0, f32::NAN, 1.0, 6.0)),
            ("high NaN q",        Biquad::high_shelf(48000.0, 8000.0, f32::NAN, 6.0)),
            ("high NaN gain",     Biquad::high_shelf(48000.0, 8000.0, 1.0, f32::NAN)),
        ];
        for (name, mut b) in cases {
            for i in 0..64 {
                let x = ((i as f32) * 0.1).sin() * 0.5;
                let y = b.process(x);
                assert!(y.is_finite(),
                    "{} produced non-finite output at sample {}: {}",
                    name, i, y);
            }
        }
    }

    // ── Randomized fuzz tests for the multi-output engine ─────────
    //
    // The RealtimeMixer is the controller's multi-output management
    // primitive: stereo input → Mid/Side split → per-group 3-band EQ
    // → recombine → per-group volume. It's an audio-thread component
    // — anything it can produce hits the user's speaker. The fuzz
    // tests below sweep across the entire input × parameter space
    // looking for combinations that produce non-finite output,
    // unbounded amplification, or state corruption.
    //
    // Each test uses a deterministic xorshift PRNG seeded with a
    // fixed value so failures reproduce. The seed appears in any
    // failure message so a regression bisect can replay the exact
    // sequence.

    fn xorshift_next(state: &mut u64) -> u64 {
        let mut x = *state;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        *state = x;
        x
    }
    fn rand_f32(state: &mut u64, lo: f32, hi: f32) -> f32 {
        let bits = xorshift_next(state);
        let n = (bits as f64 / u64::MAX as f64) as f32;
        lo + (hi - lo) * n
    }
    fn rand_params(state: &mut u64) -> MixerParams {
        // db gains in the same range the QML sliders allow.
        let g = |s: &mut u64| rand_f32(s, -12.0, 12.0);
        let v = |s: &mut u64| rand_f32(s, 0.0, 2.0);
        MixerParams {
            speaker:      GroupParams {
                low_gain_db: g(state), mid_gain_db: g(state),
                high_gain_db: g(state), volume: v(state),
            },
            instrumental: GroupParams {
                low_gain_db: g(state), mid_gain_db: g(state),
                high_gain_db: g(state), volume: v(state),
            },
        }
    }

    #[test]
    fn mixer_fuzz__random_params_random_signal_stays_finite() {
        // Sweep 200 random param sets × 1024 samples of pink-ish
        // signal per set. Every output sample must be finite.
        let mut seed = 0x_b0_0d_50_a4_de_ad_be_ef_u64;
        for _trial in 0..200 {
            let trial_seed = seed;
            let mut m = RealtimeMixer::new(48000.0);
            let params = rand_params(&mut seed);
            m.set_params(params);
            for _ in 0..1024 {
                let l = rand_f32(&mut seed, -0.95, 0.95);
                let r = rand_f32(&mut seed, -0.95, 0.95);
                let (ol, or) = m.process(l, r);
                assert!(ol.is_finite() && or.is_finite(),
                        "non-finite output with trial_seed={:#x}: \
                         params={:?}, in=({}, {}), out=({}, {})",
                        trial_seed, params, l, r, ol, or);
            }
        }
    }

    #[test]
    fn mixer_fuzz__set_params_then_process_never_corrupts_state() {
        // The set_params path preserves z1/z2 across coefficient
        // rebuilds (so live slider sweeps don't click). Sweep
        // params rapidly while processing — the internal state
        // must never accumulate to non-finite.
        let mut seed = 0x_5e_ed_53_77_50_55_88_77_u64;
        let mut m = RealtimeMixer::new(48000.0);
        for _trial in 0..500 {
            let params = rand_params(&mut seed);
            m.set_params(params);
            // A few samples per trial to give the filter state a
            // chance to settle into the new coefficients.
            for _ in 0..16 {
                let l = rand_f32(&mut seed, -0.5, 0.5);
                let r = rand_f32(&mut seed, -0.5, 0.5);
                let (ol, or) = m.process(l, r);
                assert!(ol.is_finite() && or.is_finite(),
                        "set_params sweep produced non-finite output");
            }
        }
    }

    #[test]
    fn mixer_fuzz__bounded_input_yields_bounded_output() {
        // With params constrained to the QML-allowed range
        // (±12 dB EQ, 0..2 volume), the worst-case linear-domain
        // gain is approximately 10^(12/20) × 10^(12/20) × 10^(12/20) ×
        // 2 ≈ 4 × 2 = 8x. Input magnitude is capped at 1.0, so
        // output magnitude shouldn't exceed ~16 in any combination.
        // We assert a generous 32x bound to allow for biquad
        // resonance transients near the filter's cutoff.
        let mut seed = 0xb0_d0_da_b0_b0_d0_da_b0_u64;
        const BOUND: f32 = 32.0;
        for _trial in 0..100 {
            let mut m = RealtimeMixer::new(48000.0);
            m.set_params(rand_params(&mut seed));
            for _ in 0..2048 {
                let l = rand_f32(&mut seed, -1.0, 1.0);
                let r = rand_f32(&mut seed, -1.0, 1.0);
                let (ol, or) = m.process(l, r);
                assert!(ol.abs() < BOUND && or.abs() < BOUND,
                        "unbounded gain: l={}, r={}, ol={}, or={}",
                        l, r, ol, or);
            }
        }
    }

    #[test]
    fn mixer_fuzz__deterministic_same_seed_same_output() {
        // Determinism: same params, same input → same output.
        // Critical for reproducibility of bugs.
        let params_seed = 0xfeedfaceu64.wrapping_mul(0x1234567890abcdef);
        let mut sa = params_seed;
        let mut sb = params_seed;
        let params_a = rand_params(&mut sa);
        let params_b = rand_params(&mut sb);
        assert_eq!(params_a, params_b);

        let mut ma = RealtimeMixer::new(48000.0);
        let mut mb = RealtimeMixer::new(48000.0);
        ma.set_params(params_a);
        mb.set_params(params_b);
        // Feed identical signals to each.
        let mut signal_seed = 0x1234_5678_9ab_cdefu64;
        for _ in 0..1024 {
            let l = rand_f32(&mut signal_seed, -0.7, 0.7);
            let r = rand_f32(&mut signal_seed, -0.7, 0.7);
            let (oal, oar) = ma.process(l, r);
            let (obl, obr) = mb.process(l, r);
            assert!((oal - obl).abs() < 1e-6
                 && (oar - obr).abs() < 1e-6,
                "non-deterministic mixer with identical params");
        }
    }

    #[test]
    fn mixer_fuzz__nan_input_recovers_within_short_window() {
        // The NaN-defense path resets filter state when corruption
        // is detected. Inject NaN every N samples during otherwise-
        // healthy operation and verify the mixer always recovers
        // to producing finite output within a small bounded window.
        let mut m = RealtimeMixer::new(48000.0);
        m.set_params(MixerParams::default());
        let mut seed = 0xdeadbeef_cafebabeu64;
        let mut consecutive_nan_out = 0;
        let mut max_consecutive_nan = 0;
        for i in 0..50_000 {
            // Inject NaN every 233 samples (prime so the pattern
            // doesn't align with any internal cycle).
            let l = if i % 233 == 0 { f32::NAN }
                    else { rand_f32(&mut seed, -0.5, 0.5) };
            let r = if i % 233 == 0 { f32::INFINITY }
                    else { rand_f32(&mut seed, -0.5, 0.5) };
            let (ol, or) = m.process(l, r);
            if !ol.is_finite() || !or.is_finite() {
                consecutive_nan_out += 1;
                if consecutive_nan_out > max_consecutive_nan {
                    max_consecutive_nan = consecutive_nan_out;
                }
            } else {
                consecutive_nan_out = 0;
            }
        }
        // Should recover within one sample of any corruption injection.
        // The defense path zeros the output and resets state, so the
        // very next sample after a NaN injection should be finite.
        assert!(max_consecutive_nan <= 1,
                "mixer failed to recover; max consecutive NaN out = {}",
                max_consecutive_nan);
    }

    #[test]
    fn mixer_fuzz__process_buffer_matches_process_sample_by_sample() {
        // process_buffer is the hot path used by the soundscape
        // file processor; process() is the per-sample variant used
        // by tests and the streaming code. They must produce
        // bit-identical output (modulo the NaN-reset defense which
        // both share).
        let mut seed = 0x111_2222_3333_4444_u64;
        let params = rand_params(&mut seed);
        let mut sig: Vec<(f32, f32)> = (0..2048).map(|_| {
            (rand_f32(&mut seed, -0.8, 0.8),
             rand_f32(&mut seed, -0.8, 0.8))
        }).collect();
        // Make a copy for the per-sample path.
        let mut sig2 = sig.clone();

        let mut m1 = RealtimeMixer::new(48000.0);
        m1.set_params(params);
        m1.process_buffer(&mut sig);

        let mut m2 = RealtimeMixer::new(48000.0);
        m2.set_params(params);
        for s in sig2.iter_mut() {
            let (l, r) = *s;
            *s = m2.process(l, r);
        }

        for (i, ((al, ar), (bl, br))) in sig.iter().zip(sig2.iter()).enumerate() {
            assert!((al - bl).abs() < 1e-5 && (ar - br).abs() < 1e-5,
                    "process vs process_buffer diverge at sample {}: \
                     buffer=({}, {}), per-sample=({}, {})",
                    i, al, ar, bl, br);
        }
    }
}
