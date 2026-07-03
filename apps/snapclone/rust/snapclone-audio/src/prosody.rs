//! Prosody channel — the "base tones" of a parametric voice codec.
//!
//! This is the cheap, deterministic, model-free half of the speaker-tone idea:
//! per frame it extracts the three low-dimensional parameters that carry
//! intonation and rhythm — fundamental frequency (pitch / F0), energy, and a
//! voiced/unvoiced flag — and a bounded-memory predictor turns the *stream* of
//! those into small residuals, because prosody is smooth and patterned.
//!
//! ## What this is, in the bigger picture
//!
//! A voice frame splits cleanly into three channels with very different costs:
//!
//!   1. Speaker timbre — slow, ~constant per person. A compact embedding sent
//!      once (or slowly adapted). This is where your "whisper-tiny-inspired
//!      model" fits: use it as an ENCODER that emits a timbre/speaker vector,
//!      not as an ASR decoder (transcription is the high-latency trap). Because
//!      timbre barely changes, you can afford a heavier model and run it rarely.
//!
//!   2. Prosody / base tones — F0 + energy + voicing. Fast, but SMOOTH and
//!      predictable, so a predictor + residual is nearly free. THIS MODULE.
//!
//!   3. Phoneme content — the per-frame spectral envelope (which sound is being
//!      made right now). Fast, and it IS the information. Not predictable from
//!      timbre + pitch, so it must be transmitted per frame (or recognised,
//!      with latency). This is the irreducible payload — the part neither the
//!      model nor the FSM gives you for free.
//!
//! The honest catch in "timbre model + base-tone LUT": those two channels are
//! exactly the predictable ones, so prediction shines there. Channel 3 is where
//! the bits and the latency actually live, and no amount of speaker-tone hinting
//! conjures it — a model conditioned only on pitch + speaker would synthesise
//! plausible babble in the right voice with the right melody, not the words
//! that were said. A real codec needs channel 3 wired in (LPC/spectral). This
//! module is channel 2, built correctly so it's reusable whatever you pick for 1 and 3.
//!
//! Note: `analyze` is a basic autocorrelation F0 estimator — fine for clean
//! voiced frames and the place to start; production pitch tracking wants octave-
//! error handling (or let the timbre model emit pitch too).

use std::collections::VecDeque;

/// One frame's prosody parameters. `f0_hz == 0` ⇔ unvoiced.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ProsodyFrame {
    pub f0_hz: f32,
    pub energy: f32, // RMS, roughly [0, 1]
    pub voiced: bool,
}

impl ProsodyFrame {
    pub const SILENCE: ProsodyFrame = ProsodyFrame { f0_hz: 0.0, energy: 0.0, voiced: false };
}

/// Coarse prosodic state — the FSM label that a LUT/predictor can key on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProsodyState { Silence, Onset, Voiced, Unvoiced }

/// Extract pitch / energy / voicing from one frame via normalised
/// autocorrelation over the 80–400 Hz lag range.
pub fn analyze(frame: &[i16], sample_rate: u32) -> ProsodyFrame {
    let n = frame.len();
    if n < 2 || sample_rate == 0 {
        return ProsodyFrame::SILENCE;
    }
    let xs: Vec<f32> = frame.iter().map(|&s| s as f32 / 32_768.0).collect();
    let mean = xs.iter().sum::<f32>() / n as f32;
    let x: Vec<f32> = xs.iter().map(|v| v - mean).collect(); // DC-removed

    let energy = (x.iter().map(|v| v * v).sum::<f32>() / n as f32).sqrt();
    if energy < 1.0e-4 {
        return ProsodyFrame { f0_hz: 0.0, energy, voiced: false }; // silence
    }

    let sr = sample_rate as f32;
    let min_lag = (sr / 400.0).floor().max(1.0) as usize;
    let max_lag = ((sr / 80.0).ceil() as usize).min(n - 1);
    let r0: f32 = x.iter().map(|v| v * v).sum::<f32>() + 1.0e-9;

    let mut best_lag = 0usize;
    let mut best_norm = 0.0f32;
    let mut lag = min_lag;
    while lag <= max_lag {
        let mut acc = 0.0f32;
        for i in lag..n {
            acc += x[i] * x[i - lag];
        }
        let norm = acc / r0;
        if norm > best_norm {
            best_norm = norm;
            best_lag = lag;
        }
        lag += 1;
    }

    let voiced = best_norm > 0.4 && best_lag > 0;
    let f0_hz = if voiced { sr / best_lag as f32 } else { 0.0 };
    ProsodyFrame { f0_hz, energy, voiced }
}

/// Bounded-memory prosody predictor — the "30-second memory" model. Keeps a
/// window of recent frames and an EMA of pitch/energy; predicts the next frame
/// (prosody is smooth, so persistence is a strong predictor) and exposes the
/// residual you'd actually transmit.
pub struct ProsodyPredictor {
    max_frames: usize,
    history: VecDeque<ProsodyFrame>,
    f0_ema: f32,
    energy_ema: f32,
    last_voiced: bool,
    prev_voiced: bool,
    alpha: f32,
}

impl ProsodyPredictor {
    /// `memory_secs` sizes the window (e.g. 30). Frames assumed `frame_ms` apart.
    pub fn new(sample_rate: u32, frame_ms: u32, memory_secs: u32) -> Self {
        let fm = frame_ms.max(1);
        let max_frames = ((memory_secs.max(1) * 1000) / fm).max(1) as usize;
        let _ = sample_rate; // reserved (e.g. for rate-aware smoothing)
        Self {
            max_frames,
            history: VecDeque::with_capacity(max_frames),
            f0_ema: 0.0,
            energy_ema: 0.0,
            last_voiced: false,
            prev_voiced: false,
            alpha: 0.2,
        }
    }

    pub fn history_len(&self) -> usize { self.history.len() }
    pub fn capacity_frames(&self) -> usize { self.max_frames }
    pub fn mean_f0(&self) -> f32 { self.f0_ema }

    /// Predict the next frame from recent history (persistence + EMA).
    pub fn predict(&self) -> ProsodyFrame {
        ProsodyFrame { f0_hz: self.f0_ema, energy: self.energy_ema, voiced: self.last_voiced }
    }

    /// Fold an observed frame into the model (and bound the memory).
    pub fn update(&mut self, f: ProsodyFrame) {
        if f.voiced && f.f0_hz > 0.0 {
            self.f0_ema = if self.f0_ema > 0.0 {
                self.f0_ema + self.alpha * (f.f0_hz - self.f0_ema)
            } else {
                f.f0_hz // seed on first voiced frame
            };
        }
        self.energy_ema += self.alpha * (f.energy - self.energy_ema);
        self.prev_voiced = self.last_voiced;
        self.last_voiced = f.voiced;
        self.history.push_back(f);
        while self.history.len() > self.max_frames {
            self.history.pop_front();
        }
    }

    /// Residual vs the prediction: `(Δf0_hz, Δenergy)` — the cheap thing to send.
    pub fn residual(&self, actual: &ProsodyFrame) -> (f32, f32) {
        let p = self.predict();
        (actual.f0_hz - p.f0_hz, actual.energy - p.energy)
    }

    /// Current FSM state from the latest frame + the voicing transition.
    pub fn state(&self) -> ProsodyState {
        match self.history.back() {
            None => ProsodyState::Silence,
            Some(f) if f.energy < 1.0e-3 => ProsodyState::Silence,
            Some(f) if f.voiced && !self.prev_voiced => ProsodyState::Onset,
            Some(f) if f.voiced => ProsodyState::Voiced,
            Some(_) => ProsodyState::Unvoiced,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(freq: f32, sr: u32, n: usize, amp: f32) -> Vec<i16> {
        (0..n)
            .map(|i| {
                let t = i as f32 / sr as f32;
                ((amp * (2.0 * std::f32::consts::PI * freq * t).sin()) * 32_767.0) as i16
            })
            .collect()
    }

    #[test]
    fn analyze_detects_pitch_of_voiced_tone() {
        let f = analyze(&sine(150.0, 16_000, 320, 0.6), 16_000);
        assert!(f.voiced, "150 Hz tone should read voiced");
        assert!((f.f0_hz - 150.0).abs() < 8.0, "f0 ~150, got {}", f.f0_hz);
        assert!(f.energy > 0.0);
    }

    #[test]
    fn analyze_marks_noise_unvoiced() {
        // deterministic LCG noise — energetic but aperiodic in the F0 band
        let mut st: u32 = 0x12345;
        let noise: Vec<i16> = (0..320)
            .map(|_| {
                st = st.wrapping_mul(1_103_515_245).wrapping_add(12_345);
                (((st >> 16) & 0x7fff) as i32 - 16_384) as i16
            })
            .collect();
        let f = analyze(&noise, 16_000);
        assert!(!f.voiced, "noise should read unvoiced (no F0 periodicity)");
    }

    #[test]
    fn analyze_silence_is_unvoiced_zero_energy() {
        let f = analyze(&vec![0i16; 320], 16_000);
        assert!(!f.voiced);
        assert!(f.energy < 1.0e-4);
        assert_eq!(f.f0_hz, 0.0);
    }

    #[test]
    fn predictor_residual_tiny_on_steady_prosody() {
        let mut p = ProsodyPredictor::new(16_000, 20, 30);
        let v = ProsodyFrame { f0_hz: 150.0, energy: 0.3, voiced: true };
        for _ in 0..30 { p.update(v); }
        let (df0, den) = p.residual(&v);
        assert!(df0.abs() < 3.0, "steady pitch → tiny residual, got {df0}");
        assert!(den.abs() < 0.02, "steady energy → tiny residual, got {den}");
    }

    #[test]
    fn predictor_tracks_a_slow_ramp() {
        // a gentle pitch glide: the predictor should trail within a few Hz,
        // i.e. the residual stays far smaller than the total excursion (120→160).
        let mut p = ProsodyPredictor::new(16_000, 20, 30);
        let mut max_res = 0.0f32;
        for k in 0..40 {
            let f0 = 120.0 + k as f32; // +1 Hz/frame
            let actual = ProsodyFrame { f0_hz: f0, energy: 0.3, voiced: true };
            if k > 10 {
                let (df0, _) = p.residual(&actual);
                max_res = max_res.max(df0.abs());
            }
            p.update(actual);
        }
        assert!(max_res < 10.0, "ramp residual should stay small, got {max_res}");
    }

    #[test]
    fn predictor_memory_is_bounded() {
        let mut p = ProsodyPredictor::new(16_000, 20, 1); // 1 s = 50 frames
        for _ in 0..500 {
            p.update(ProsodyFrame { f0_hz: 140.0, energy: 0.2, voiced: true });
        }
        assert!(p.history_len() <= 50, "history bounded to ~1 s, got {}", p.history_len());
        assert_eq!(p.capacity_frames(), 50);
    }

    #[test]
    fn state_machine_labels_transitions() {
        let mut p = ProsodyPredictor::new(16_000, 20, 30);
        p.update(ProsodyFrame::SILENCE);
        assert_eq!(p.state(), ProsodyState::Silence);
        p.update(ProsodyFrame { f0_hz: 150.0, energy: 0.3, voiced: true }); // first voiced
        assert_eq!(p.state(), ProsodyState::Onset);
        p.update(ProsodyFrame { f0_hz: 152.0, energy: 0.3, voiced: true }); // sustained
        assert_eq!(p.state(), ProsodyState::Voiced);
        p.update(ProsodyFrame { f0_hz: 0.0, energy: 0.2, voiced: false });  // fricative
        assert_eq!(p.state(), ProsodyState::Unvoiced);
    }
}
