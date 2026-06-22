// src/predictor.rs
#![allow(dead_code)]
//
// Phase-locked inverse-template generator. The predictor holds a learned
// per-cycle waveform (from `pattern_learner::learn_template`) and emits
// samples one at a time, walking through the cycle and wrapping at the
// period boundary. The output IS the template's inverse — that's how
// destructive interference works when this signal is mixed with the
// continuing periodic noise.
//
// The predictor has no I/O; it's the same shape as the AntiToneGen but
// uses an arbitrary learned template instead of a hand-coded harmonic
// series. Plug it into the generator thread of the sanitizer driver and
// mix its output with the masker.
//
// Phase tracking: the predictor maintains an internal sample counter
// that advances by 1 on each call to `next_sample()`. The counter
// modulo the template length gives the current phase within the cycle.
// When a new template arrives, the phase is reset to 0 (we have no
// information about which phase the new template corresponds to — that's
// captured by *when* the learning ran).
//
// The amplitude of the output is scaled down by a configurable factor
// (default 0.5) to prevent over-cancellation (which sounds worse than
// under-cancellation — it adds tonal artifacts of its own). The user
// can dial this up if they want more aggressive cancellation.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

/// Shared template + period state. The pattern-learning thread updates
/// this; the predictor's `next_sample()` reads it. Updates are atomic
/// (full template swap) — no torn reads of half-old half-new templates.
pub struct PredictorState {
    /// Current template (single cycle of mean-subtracted waveform).
    /// Empty when no good template has been learned yet.
    template: Mutex<Vec<f32>>,
    /// Cancellation gain in [0.0, 1.0]; 0 = no output, 1.0 = full
    /// inverse playback (often too much). Stored as raw bits so we can
    /// load it without locking.
    gain_bits: std::sync::atomic::AtomicU32,
    /// How many template updates have happened — useful for logging
    /// and for the UI to surface a "freshness" indicator.
    generation: AtomicUsize,
}

impl PredictorState {
    pub fn new(initial_gain: f32) -> Self {
        Self {
            template: Mutex::new(Vec::new()),
            gain_bits: std::sync::atomic::AtomicU32::new(initial_gain.clamp(0.0, 1.0).to_bits()),
            generation: AtomicUsize::new(0),
        }
    }

    /// Atomically swap in a new template. Pass an empty Vec to silence
    /// the predictor (e.g. if pattern learning lost confidence).
    pub fn update_template(&self, template: Vec<f32>) {
        if let Ok(mut t) = self.template.lock() {
            *t = template;
        }
        self.generation.fetch_add(1, Ordering::Relaxed);
    }

    /// Adjust cancellation gain on the fly.
    pub fn set_gain(&self, gain: f32) {
        self.gain_bits.store(gain.clamp(0.0, 1.0).to_bits(), Ordering::Relaxed);
    }

    pub fn gain(&self) -> f32 {
        f32::from_bits(self.gain_bits.load(Ordering::Relaxed))
    }

    pub fn generation(&self) -> usize {
        self.generation.load(Ordering::Relaxed)
    }

    /// True when a usable template is loaded.
    pub fn is_active(&self) -> bool {
        self.template.lock().map(|t| !t.is_empty()).unwrap_or(false)
    }
}

/// The actual sample generator. Owns a phase counter and a clone of the
/// shared state. Run inside the audio generator thread.
pub struct Predictor {
    state: Arc<PredictorState>,
    phase: usize,
    last_generation: usize,
}

impl Predictor {
    pub fn new(state: Arc<PredictorState>) -> Self {
        Self { state, phase: 0, last_generation: 0 }
    }

    /// Yield the next inverse-template sample. Returns 0.0 when no
    /// template is loaded (predictor is dormant; sanitizer plays just the
    /// masker noise).
    pub fn next_sample(&mut self) -> f32 {
        // Cheap atomic check: did the learning thread swap in a new template?
        let gen = self.state.generation.load(Ordering::Relaxed);
        if gen != self.last_generation {
            self.last_generation = gen;
            // Reset phase whenever the template changes — old phase is
            // meaningless against a new template (different period).
            self.phase = 0;
        }
        let gain = self.state.gain();
        let s = {
            let t = match self.state.template.lock() {
                Ok(g) => g,
                Err(_) => return 0.0,
            };
            if t.is_empty() { return 0.0; }
            let i = self.phase % t.len();
            let val = -t[i] * gain;   // inverse polarity, attenuated
            self.phase += 1;
            // Don't let phase overflow on extremely long runs.
            if self.phase >= t.len() * 1024 { self.phase %= t.len(); }
            val
        };
        s.clamp(-1.0, 1.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn predictor_is_silent_with_no_template() {
        let state = Arc::new(PredictorState::new(0.5));
        let mut p = Predictor::new(state);
        for _ in 0..100 {
            assert_eq!(p.next_sample(), 0.0);
        }
    }

    #[test]
    fn predictor_emits_inverse_of_template() {
        let state = Arc::new(PredictorState::new(1.0));
        state.update_template(vec![0.5, -0.5, 0.3, -0.3]);
        let mut p = Predictor::new(state);
        // With gain=1.0, output should be the exact negation.
        assert!((p.next_sample() - (-0.5)).abs() < 1e-6);
        assert!((p.next_sample() - 0.5).abs() < 1e-6);
        assert!((p.next_sample() - (-0.3)).abs() < 1e-6);
        assert!((p.next_sample() - 0.3).abs() < 1e-6);
        // Wrap.
        assert!((p.next_sample() - (-0.5)).abs() < 1e-6);
    }

    #[test]
    fn predictor_respects_gain() {
        let state = Arc::new(PredictorState::new(0.5));
        state.update_template(vec![1.0, -1.0]);
        let mut p = Predictor::new(state);
        assert!((p.next_sample() - (-0.5)).abs() < 1e-6);  // 1.0 * -0.5
        assert!((p.next_sample() - 0.5).abs() < 1e-6);     // -1.0 * -0.5
    }

    #[test]
    fn template_update_resets_phase() {
        let state = Arc::new(PredictorState::new(1.0));
        state.update_template(vec![0.1, 0.2, 0.3, 0.4]);
        let mut p = Predictor::new(state.clone());
        p.next_sample();
        p.next_sample();
        // Now phase=2. Update template.
        state.update_template(vec![1.0, 2.0]);
        // Next sample should come from phase 0 of the new template.
        let v = p.next_sample();
        assert!((v - (-1.0)).abs() < 1e-6, "expected -1.0 (inverse of new[0]), got {}", v);
    }

    #[test]
    fn empty_template_makes_silent() {
        let state = Arc::new(PredictorState::new(1.0));
        state.update_template(vec![1.0, 2.0]);
        let mut p = Predictor::new(state.clone());
        p.next_sample();  // make sure we've started
        state.update_template(Vec::new());
        for _ in 0..10 {
            assert_eq!(p.next_sample(), 0.0);
        }
    }

    #[test]
    fn is_active_reflects_template_state() {
        let state = Arc::new(PredictorState::new(0.5));
        assert!(!state.is_active());
        state.update_template(vec![0.1; 100]);
        assert!(state.is_active());
        state.update_template(Vec::new());
        assert!(!state.is_active());
    }

    #[test]
    fn output_clamped_in_range() {
        // If someone uploads a runaway template, we shouldn't blow out.
        let state = Arc::new(PredictorState::new(1.0));
        state.update_template(vec![10.0, -10.0, 100.0]);
        let mut p = Predictor::new(state);
        for _ in 0..30 {
            let s = p.next_sample();
            assert!(s.abs() <= 1.0, "output {} out of range", s);
        }
    }
}
