// src/pattern_runner.rs
#![allow(dead_code)]
//
// Orchestrator that drives the predictive-cancellation pipeline:
//
//   mic_capture → ring buffer →
//     [every N seconds] estimate_period + learn_template + periodicity_score →
//       if good: push template into PredictorState
//       if bad:  push empty template (predictor goes silent)
//
// Runs in its own background thread. The audio thread (sanitizer driver)
// shares the PredictorState; it polls atomically for new templates and
// emits the inverse waveform.
//
// Lifecycle:
//   PatternRunner::start(state) spawns the thread and a MicCapture; the
//   handle's Drop signals stop and joins.
//
// Tuning knobs (all defaults are reasonable):
//   - learn_window_samples: how much recent mic audio to analyze each pass.
//     Default ~500 ms; need at least ~4 periods of the slowest noise we
//     care about (≈100 ms for 40 Hz) for good averaging.
//   - learn_interval_ms: how often to re-learn. Default 5 s — keeps up
//     with gentle drift in fan/motor speed without thrashing.
//   - min_score: minimum periodicity score to actually update the
//     predictor. Default 0.5 — below this the noise is too broadband
//     to predictably cancel.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crate::mic_capture::MicCapture;
use crate::pattern_learner;
use crate::predictor::PredictorState;

pub struct PatternRunner {
    state: Arc<PredictorState>,
    stop: Arc<AtomicBool>,
    /// Atomic counter of successful template updates, for UI freshness display.
    learned_count: Arc<std::sync::atomic::AtomicUsize>,
    /// Last detected period as f32 bits (or 0 if no recent successful detection).
    last_period_bits: Arc<std::sync::atomic::AtomicU32>,
    /// Last score as f32 bits.
    last_score_bits: Arc<std::sync::atomic::AtomicU32>,
    mic_backend: String,
    thread: Option<JoinHandle<()>>,
}

impl PatternRunner {
    /// Default tuning suitable for typical environmental noise.
    pub fn start(state: Arc<PredictorState>) -> Result<Self, String> {
        Self::start_with(state, 24_000, 5_000, 0.5)
    }

    /// Explicit-tuning constructor.
    /// `learn_window_samples` — how many recent samples to analyze each pass
    /// `learn_interval_ms`    — how often to re-learn
    /// `min_score`            — minimum periodicity score to accept a template
    pub fn start_with(
        state: Arc<PredictorState>,
        learn_window_samples: usize,
        learn_interval_ms: u64,
        min_score: f32,
    ) -> Result<Self, String> {
        let mic = MicCapture::start()
            .map_err(|e| format!("mic capture: {}", e))?;
        let backend = mic.backend_name().to_string();

        let stop = Arc::new(AtomicBool::new(false));
        let learned = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let last_period = Arc::new(std::sync::atomic::AtomicU32::new(0));
        let last_score = Arc::new(std::sync::atomic::AtomicU32::new(0));

        let thread = {
            let state_t = state.clone();
            let stop_t = stop.clone();
            let learned_t = learned.clone();
            let lp_t = last_period.clone();
            let ls_t = last_score.clone();
            thread::spawn(move || {
                runner_loop(
                    mic, state_t, stop_t, learned_t, lp_t, ls_t,
                    learn_window_samples, learn_interval_ms, min_score,
                );
            })
        };

        Ok(Self {
            state,
            stop,
            learned_count: learned,
            last_period_bits: last_period,
            last_score_bits: last_score,
            mic_backend: backend,
            thread: Some(thread),
        })
    }

    pub fn learned_count(&self) -> usize { self.learned_count.load(Ordering::Relaxed) }
    pub fn last_period(&self) -> f32 {
        f32::from_bits(self.last_period_bits.load(Ordering::Relaxed))
    }
    pub fn last_score(&self) -> f32 {
        f32::from_bits(self.last_score_bits.load(Ordering::Relaxed))
    }
    pub fn mic_backend(&self) -> &str { &self.mic_backend }

    /// Convert the last detected period to Hz. 0.0 if no detection yet.
    pub fn last_freq_hz(&self) -> f32 {
        let p = self.last_period();
        if p > 1.0 { 48_000.0 / p } else { 0.0 }
    }
}

impl Drop for PatternRunner {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        // Clear the predictor so the audio thread goes quiet immediately.
        self.state.update_template(Vec::new());
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

fn runner_loop(
    mic: MicCapture,
    state: Arc<PredictorState>,
    stop: Arc<AtomicBool>,
    learned: Arc<std::sync::atomic::AtomicUsize>,
    last_period: Arc<std::sync::atomic::AtomicU32>,
    last_score: Arc<std::sync::atomic::AtomicU32>,
    window: usize,
    interval_ms: u64,
    min_score: f32,
) {
    // Search range: 20 Hz to 200 Hz → 240..2400 sample periods at 48 kHz.
    // These are the rumble/drone bands where predictive cancellation works.
    const PERIOD_MIN: usize = 240;
    const PERIOD_MAX: usize = 2400;

    let mut last_run = Instant::now() - Duration::from_millis(interval_ms);
    while !stop.load(Ordering::Relaxed) {
        // Sleep a bit so we don't busy-loop.
        thread::sleep(Duration::from_millis(100));
        if last_run.elapsed() < Duration::from_millis(interval_ms) { continue; }
        last_run = Instant::now();

        // Need enough samples for the analysis window.
        let samples = mic.latest_samples(window);
        if samples.len() < window { continue; }

        let period = match pattern_learner::estimate_period(&samples, PERIOD_MIN, PERIOD_MAX) {
            Some(p) => p,
            None => {
                // No periodicity → tell predictor to go silent.
                state.update_template(Vec::new());
                last_score.store((0.0_f32).to_bits(), Ordering::Relaxed);
                continue;
            }
        };

        let template = match pattern_learner::learn_template(&samples, period) {
            Some(t) => t,
            None => {
                state.update_template(Vec::new());
                continue;
            }
        };

        let score = pattern_learner::periodicity_score(&samples, &template);
        last_period.store(period.to_bits(), Ordering::Relaxed);
        last_score.store(score.to_bits(), Ordering::Relaxed);

        if score >= min_score {
            state.update_template(template);
            learned.fetch_add(1, Ordering::Relaxed);
        } else {
            // Recently periodic, now noisy — clear predictor.
            state.update_template(Vec::new());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // The runner needs real mic input so we can't test the loop end-to-end
    // here; we test the data flow shape instead.

    #[test]
    fn last_freq_zero_when_no_detection() {
        // Build a PatternRunner manually without starting the mic — the
        // constructor needs real `parec`, so we test the math directly.
        let state = Arc::new(PredictorState::new(0.5));
        let lp = Arc::new(std::sync::atomic::AtomicU32::new(0));
        let runner = PatternRunner {
            state,
            stop: Arc::new(AtomicBool::new(false)),
            learned_count: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            last_period_bits: lp,
            last_score_bits: Arc::new(std::sync::atomic::AtomicU32::new(0)),
            mic_backend: "test".to_string(),
            thread: None,
        };
        assert_eq!(runner.last_freq_hz(), 0.0);
        assert_eq!(runner.last_period(), 0.0);
    }

    #[test]
    fn freq_conversion_from_period() {
        let state = Arc::new(PredictorState::new(0.5));
        let lp = Arc::new(std::sync::atomic::AtomicU32::new(800.0_f32.to_bits()));
        let runner = PatternRunner {
            state,
            stop: Arc::new(AtomicBool::new(false)),
            learned_count: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            last_period_bits: lp,
            last_score_bits: Arc::new(std::sync::atomic::AtomicU32::new(0)),
            mic_backend: "test".to_string(),
            thread: None,
        };
        // Period 800 samples @ 48k = 60 Hz.
        assert!((runner.last_freq_hz() - 60.0).abs() < 0.1);
    }
}
