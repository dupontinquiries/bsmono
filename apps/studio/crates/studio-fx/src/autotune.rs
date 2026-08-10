//! autotune — wires `retune` (the brain) to `psola` (the hands).
//!
//! Neither module alone moves a recording toward a key. `retune` decides,
//! hop by hop, what the pitch should become; `psola` can resynthesize a
//! shift once it has epochs and a ratio. [`AutotuneChannel::process_buffer`]
//! is the first place those two paths actually run together.
//!
//! ## Scope, stated plainly (again — see `psola`'s own docs for the same
//! caveat in more depth)
//!
//! This is an **offline, whole-buffer** pass: it needs the entire take in
//! memory, because both epoch continuity (the epoch train has to run
//! unbroken across hop boundaries to avoid a click at every hop edge) and
//! the pitch/ratio track (computed hop-by-hop, ahead of resynthesis) depend
//! on more context than a single fixed-size realtime callback has available.
//! That makes this the right shape for **bouncing a take through print-side
//! autotune** — the mastering-style use — and not yet the right shape for
//! **live monitoring**, where a singer needs to hear the corrected pitch in
//! their headphones with only the audio already captured, not audio from
//! the future. A live-monitor version needs the same two pieces
//! (`Retune`, `psola_shift_varying`) driven by a streaming wrapper that
//! carries epoch and ratio state across calls instead of seeing the whole
//! buffer up front — real, separate work, not implemented here.

use crate::psola::psola_shift_varying;
use crate::retune::{yin_pitch, Pitch, Retune, Scale};

/// A print-side (offline) autotune channel: one [`Retune`] brain plus the
/// detection/resynthesis plumbing around it.
pub struct AutotuneChannel {
    pub retune: Retune,
    pub sample_rate: f32,
    pub hop_ms: f32,
    pub lo_hz: f32,
    pub hi_hz: f32,
    pub yin_threshold: f32,
}

impl AutotuneChannel {
    pub fn new(scale: Scale, sample_rate: f32, seed: u64) -> AutotuneChannel {
        AutotuneChannel {
            retune: Retune::new(scale, seed),
            sample_rate,
            hop_ms: 10.0,
            lo_hz: crate::retune::DEFAULT_MIN_HZ,
            hi_hz: crate::retune::DEFAULT_MAX_HZ,
            yin_threshold: 0.15,
        }
    }

    fn hop_len(&self) -> usize {
        ((self.hop_ms / 1000.0) * self.sample_rate).round().max(1.0) as usize
    }

    /// Run the whole pass: detect pitch hop by hop, correct via `self.retune`
    /// (its smoothing/humanize state carries naturally across hops, since
    /// it's the same instance called repeatedly — no separate plumbing
    /// needed for that continuity), mark epochs across the entire buffer in
    /// one unbroken pass, and resynthesize with the per-hop ratio track.
    /// Returns a buffer the same length as `samples`.
    pub fn process_buffer(&mut self, samples: &[f32]) -> Vec<f32> {
        let n = samples.len();
        if n == 0 {
            return Vec::new();
        }
        let hop_len = self.hop_len();

        // 1) Pitch-detect and correct, one hop at a time. Each analysis
        // window extends past its own hop by up to one period at the lowest
        // supported frequency, since YIN needs that much signal to even
        // consider the slowest pitch in range — the window supplies context,
        // the HOP is still what advances.
        let max_period = (self.sample_rate / self.lo_hz.max(1.0)).ceil() as usize;
        let mut ratio_at_hop: Vec<f32> = Vec::new();
        let mut period_at_hop: Vec<f32> = Vec::new();
        let mut last_period = self.sample_rate / 200.0; // a reasonable fallback before any voiced hop
        let mut hop_start = 0usize;
        while hop_start < n {
            let win_end = (hop_start + hop_len + max_period).min(n);
            let detected: Option<Pitch> = yin_pitch(&samples[hop_start..win_end], self.sample_rate, self.lo_hz, self.hi_hz, self.yin_threshold);
            let ratio = self.retune.process_with_hop(detected, self.hop_ms);
            ratio_at_hop.push(ratio);
            if let Some(p) = detected {
                last_period = self.sample_rate / p.hz;
            }
            period_at_hop.push(last_period);
            hop_start += hop_len;
        }

        // 2) Mark epochs across the WHOLE buffer in one continuous walk,
        // switching to whichever hop's tracked period applies as the walk
        // crosses hop boundaries — this is what keeps the epoch train
        // unbroken (and therefore click-free) through a pitch change,
        // something calling `mark_epochs` once per hop separately could not
        // do without stitching seams back together after the fact.
        let mut epochs = Vec::new();
        let mut pos = 0usize;
        while pos < n {
            let hop_idx = (pos / hop_len).min(period_at_hop.len().saturating_sub(1));
            let period = period_at_hop.get(hop_idx).copied().unwrap_or(last_period).max(2.0);
            let radius = (period * 0.25).max(1.0) as usize;
            let lo = pos.saturating_sub(radius);
            let hi = (pos + radius).min(n - 1);
            let mut best = pos.min(n - 1);
            let mut best_val = f32::MIN;
            for i in lo..=hi {
                let v = samples[i].abs();
                if v > best_val {
                    best_val = v;
                    best = i;
                }
            }
            epochs.push(best);
            let advance = period.round().max(1.0) as usize;
            pos = best + advance;
        }
        // A single fixed period (what `mark_epochs` in `psola` takes) can't
        // track pitch changes across a whole buffer, which is why epoch
        // marking is inlined here with a per-hop-updated period instead of
        // reused directly — `mark_epochs` remains the right tool for a
        // known-constant-pitch region, and is exercised on its own in
        // `psola`'s tests.

        // 3) Resynthesize with the ratio in effect at each output position.
        psola_shift_varying(samples, &epochs, |sample_idx| {
            let hop_idx = (sample_idx / hop_len).min(ratio_at_hop.len().saturating_sub(1));
            ratio_at_hop.get(hop_idx).copied().unwrap_or(1.0)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::retune::{Mode, DEFAULT_MAX_HZ, DEFAULT_MIN_HZ};

    fn sine(hz: f32, sr: f32, n: usize) -> Vec<f32> {
        (0..n).map(|i| (2.0 * std::f32::consts::PI * hz * i as f32 / sr).sin()).collect()
    }

    #[test]
    fn a_sustained_detuned_tone_lands_near_its_scale_degree() {
        let sr = 48_000.0;
        // A minor: A(69) B C D E F G. Sing ~40 cents sharp of A.
        let hz = 440.0 * 2f32.powf(0.40 / 12.0);
        let samples = sine(hz, sr, 24_000); // 0.5s, comfortably several hops
        let mut ch = AutotuneChannel::new(Scale { root: 9, mode: Mode::Minor }, sr, 1);
        ch.retune.retune_speed_ms = 0.0; // hard snap for a clean, checkable target

        let out = ch.process_buffer(&samples);
        assert_eq!(out.len(), samples.len());
        assert!(out.iter().all(|s| s.is_finite()));

        // Measure well past the start (letting any initial-hop edge effects
        // settle) and well before the end.
        let p = yin_pitch(&out[4000..20_000], sr, DEFAULT_MIN_HZ, DEFAULT_MAX_HZ, 0.15).expect("corrected tone should still read as pitched");
        assert!((p.hz - 440.0).abs() < 440.0 * 0.02, "should land within 2% of A4 (440Hz), got {}Hz", p.hz);
    }

    #[test]
    fn silence_in_gives_silence_out() {
        let sr = 48_000.0;
        let samples = vec![0.0f32; 9600];
        let mut ch = AutotuneChannel::new(Scale { root: 0, mode: Mode::Minor }, sr, 1);
        let out = ch.process_buffer(&samples);
        assert_eq!(out.len(), samples.len());
        assert!(out.iter().all(|s| s.abs() < 1e-4), "silence must not be conjured into audible output");
    }

    #[test]
    fn an_empty_buffer_returns_empty_rather_than_panicking() {
        let mut ch = AutotuneChannel::new(Scale { root: 0, mode: Mode::Minor }, 48_000.0, 1);
        assert_eq!(ch.process_buffer(&[]), Vec::<f32>::new());
    }

    #[test]
    fn already_on_pitch_material_is_left_close_to_unchanged() {
        let sr = 48_000.0;
        let hz = 440.0; // exactly A, already on the A-minor scale
        let samples = sine(hz, sr, 24_000);
        let mut ch = AutotuneChannel::new(Scale { root: 9, mode: Mode::Minor }, sr, 1);
        ch.retune.retune_speed_ms = 0.0;

        let out = ch.process_buffer(&samples);
        let p = yin_pitch(&out[4000..20_000], sr, DEFAULT_MIN_HZ, DEFAULT_MAX_HZ, 0.15).unwrap();
        assert!((p.hz - hz).abs() < hz * 0.02, "material already on-degree should stay near {hz}Hz, got {}", p.hz);
    }

    #[test]
    fn retune_state_carries_across_the_whole_buffer_not_just_within_one_hop() {
        // A slow retune speed should still be visibly converging by the END
        // of a half-second buffer even though any single hop's correction is
        // tiny — this only happens if `self.retune`'s smoothing state
        // persists across every hop of process_buffer, not reset per hop.
        let sr = 48_000.0;
        let hz = 440.0 * 2f32.powf(0.5 / 12.0); // 50 cents sharp
        let samples = sine(hz, sr, 24_000);
        let mut ch = AutotuneChannel::new(Scale { root: 9, mode: Mode::Minor }, sr, 1);
        ch.retune.retune_speed_ms = 80.0; // slow enough that one hop alone can't fix it

        let out = ch.process_buffer(&samples);
        let early = yin_pitch(&out[500..4000], sr, DEFAULT_MIN_HZ, DEFAULT_MAX_HZ, 0.15).unwrap();
        let late = yin_pitch(&out[18_000..22_000], sr, DEFAULT_MIN_HZ, DEFAULT_MAX_HZ, 0.15).unwrap();
        let early_cents_off = 1200.0 * (early.hz / 440.0).log2();
        let late_cents_off = 1200.0 * (late.hz / 440.0).log2();
        assert!(late_cents_off.abs() < early_cents_off.abs(),
            "later in the buffer should be closer to on-pitch than earlier (early {early_cents_off} cents, late {late_cents_off} cents) — proves retune state persists across hops");
    }
}
