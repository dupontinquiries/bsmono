//! Microphone capture + lightweight onset detection.
//!
//! cpal runs its callback on a separate audio thread. We publish snapshots of a
//! rolling RMS history and a running syllable count into a shared `AudioState`
//! protected by a Mutex. The Iced subscription polls this on a timer.

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{SampleFormat, Stream, StreamConfig};
use std::sync::{Arc, Mutex};

/// Number of RMS bars kept for the visualization.
pub const HISTORY_BARS: usize = 96;

/// One detected onset, exposed to the UI for debugging.
#[derive(Debug, Clone, Copy)]
pub struct OnsetEvent {
    /// Milliseconds since recording started (approximate).
    pub elapsed_ms: u32,
    /// Smoothed RMS at the moment of trigger.
    pub level: f32,
}

/// How many recent onsets we retain for the debug panel.
pub const MAX_ONSETS: usize = 32;

/// Snapshot shared between the audio thread and the UI thread.
#[derive(Debug, Clone)]
pub struct AudioState {
    /// RMS values in [0.0, 1.0]. Index 0 is the oldest bar (left); last is newest.
    pub levels: Vec<f32>,
    /// Number of detected onsets since the last reset.
    pub syllables: u32,
    /// Whether we are currently writing to state. Setting to `false` parks the
    /// audio thread cheaply without tearing down the stream.
    pub is_recording: bool,

    // --- intermediate state exposed for debugging --------------------------
    /// Most recent smoothed RMS value (the value the threshold compares against).
    pub smoothed_level: f32,
    /// Detector hysteresis state. `true` = below LOW, ready to fire on next rise.
    pub armed: bool,
    /// Ring of recent onset events (oldest first, capped at MAX_ONSETS).
    pub onsets: Vec<OnsetEvent>,

    // --- pitch tracking ----------------------------------------------------
    /// Most recent detected fundamental frequency in Hz, or `None` during
    /// silence / unvoiced sound. Updated about every 100 ms.
    pub current_pitch_hz: Option<f32>,
    /// Sample rate of the input stream — exposed so the UI can show it in
    /// debug, but mostly used by analysis code.
    pub sample_rate: u32,
}

impl AudioState {
    pub fn new() -> Self {
        Self {
            levels: vec![0.0; HISTORY_BARS],
            syllables: 0,
            is_recording: false,
            smoothed_level: 0.0,
            armed: true,
            onsets: Vec::with_capacity(MAX_ONSETS),
            current_pitch_hz: None,
            sample_rate: 0,
        }
    }

    pub fn reset(&mut self) {
        self.levels.fill(0.0);
        self.syllables = 0;
        self.smoothed_level = 0.0;
        self.armed = true;
        self.onsets.clear();
        self.current_pitch_hz = None;
    }
}

pub type SharedAudio = Arc<Mutex<AudioState>>;

// --- sample → f32 in [-1, 1] ------------------------------------------------

#[inline]
fn norm_i16(s: i16) -> f32 {
    s as f32 / i16::MAX as f32
}
#[inline]
fn norm_u16(s: u16) -> f32 {
    (s as f32 - 32768.0) / 32768.0
}

// --- public API -------------------------------------------------------------

/// Builds and starts an input stream. The caller MUST keep the returned `Stream`
/// alive to keep capture running; dropping it stops the audio device. Returns
/// `None` if no default input device or supported config is available.
pub fn start_capture(state: SharedAudio) -> Option<Stream> {
    let host = cpal::default_host();
    let device = host.default_input_device()?;
    let supported = device.default_input_config().ok()?;
    let sample_format = supported.sample_format();
    let config: StreamConfig = supported.into();

    // Publish the sample rate so the UI can show it and pitch math is correct.
    if let Ok(mut guard) = state.lock() {
        guard.sample_rate = config.sample_rate.0;
    }

    // ~25 ms RMS window. Channels are interleaved — we treat all channels as one
    // stream for energy purposes (simpler than averaging across channels and
    // works fine for syllable detection).
    let window_samples = ((config.sample_rate.0 as f32 * 0.025) as usize).max(64)
        * config.channels.max(1) as usize;
    let channels = config.channels.max(1) as usize;

    let err_fn = |err| eprintln!("kRapper audio stream error: {err}");

    let stream = match sample_format {
        SampleFormat::F32 => {
            let mut det = OnsetDetector::new(window_samples);
            let mut pitch = PitchTracker::new(config.sample_rate.0 as f32);
            let st = Arc::clone(&state);
            device
                .build_input_stream(
                    &config,
                    move |data: &[f32], _: &cpal::InputCallbackInfo| {
                        det.feed(data.iter().copied(), &st);
                        pitch.feed_mono(data, channels, &st);
                    },
                    err_fn,
                    None,
                )
                .ok()?
        }
        SampleFormat::I16 => {
            let mut det = OnsetDetector::new(window_samples);
            let mut pitch = PitchTracker::new(config.sample_rate.0 as f32);
            let st = Arc::clone(&state);
            device
                .build_input_stream(
                    &config,
                    move |data: &[i16], _: &cpal::InputCallbackInfo| {
                        det.feed(data.iter().copied().map(norm_i16), &st);
                        // Channel 0 only for pitch.
                        for &s in data.iter().step_by(channels) {
                            pitch.feed_one(norm_i16(s), &st);
                        }
                    },
                    err_fn,
                    None,
                )
                .ok()?
        }
        SampleFormat::U16 => {
            let mut det = OnsetDetector::new(window_samples);
            let mut pitch = PitchTracker::new(config.sample_rate.0 as f32);
            let st = Arc::clone(&state);
            device
                .build_input_stream(
                    &config,
                    move |data: &[u16], _: &cpal::InputCallbackInfo| {
                        det.feed(data.iter().copied().map(norm_u16), &st);
                        for &s in data.iter().step_by(channels) {
                            pitch.feed_one(norm_u16(s), &st);
                        }
                    },
                    err_fn,
                    None,
                )
                .ok()?
        }
        other => {
            eprintln!("kRapper: unsupported sample format {other:?}");
            return None;
        }
    };

    stream.play().ok()?;
    Some(stream)
}

// --- onset detection --------------------------------------------------------

struct OnsetDetector {
    window_samples: usize,
    acc_sum: f32,
    acc_count: usize,
    smoothed: f32,
    armed: bool,
    /// Windows elapsed since recording started. Resets when we see `!is_recording`.
    /// Used to stamp onset events with an approximate `elapsed_ms`.
    windows_since_record: u32,
}

/// Hysteresis upper bound — a syllable fires when smoothed RMS crosses this.
pub const ONSET_HIGH: f32 = 0.07;
/// Hysteresis lower bound — detector re-arms after smoothed RMS dips below this.
pub const ONSET_LOW: f32 = 0.03;

impl OnsetDetector {
    /// Tuned for casual humming on a typical laptop mic. Adjust if your room
    /// is louder or quieter than average.
    const HIGH: f32 = ONSET_HIGH;
    const LOW: f32 = ONSET_LOW;
    /// Each window is exactly 25 ms by construction (see `window_samples`).
    const WINDOW_MS: u32 = 25;

    fn new(window_samples: usize) -> Self {
        Self {
            window_samples,
            acc_sum: 0.0,
            acc_count: 0,
            smoothed: 0.0,
            armed: true,
            windows_since_record: 0,
        }
    }

    fn feed<I: IntoIterator<Item = f32>>(&mut self, samples: I, state: &SharedAudio) {
        for v in samples {
            self.acc_sum += v * v;
            self.acc_count += 1;
            if self.acc_count < self.window_samples {
                continue;
            }
            let rms = (self.acc_sum / self.acc_count as f32).sqrt();
            self.acc_sum = 0.0;
            self.acc_count = 0;

            // One-pole low-pass on the level so the bars don't strobe.
            self.smoothed = 0.6 * self.smoothed + 0.4 * rms;

            let Ok(mut guard) = state.lock() else { continue };
            if !guard.is_recording {
                self.windows_since_record = 0;
                continue;
            }
            self.windows_since_record = self.windows_since_record.saturating_add(1);

            guard.levels.remove(0);
            guard.levels.push(self.smoothed.clamp(0.0, 1.0));

            let mut fired = false;
            if self.armed && self.smoothed > Self::HIGH {
                guard.syllables += 1;
                self.armed = false;
                fired = true;
            } else if !self.armed && self.smoothed < Self::LOW {
                self.armed = true;
            }

            // Publish debug-visible detector state.
            guard.smoothed_level = self.smoothed;
            guard.armed = self.armed;

            if fired {
                let event = OnsetEvent {
                    elapsed_ms: self.windows_since_record * Self::WINDOW_MS,
                    level: self.smoothed,
                };
                if guard.onsets.len() >= MAX_ONSETS {
                    guard.onsets.remove(0);
                }
                guard.onsets.push(event);
            }
        }
    }
}

// --- YIN pitch detection ----------------------------------------------------
//
// Implements the YIN algorithm (de Cheveigné & Kawahara, 2002) for monophonic
// pitch estimation. The flow:
//
//   1. Ring-buffer the latest N=1024 mono samples (~23 ms at 44.1 kHz).
//   2. Every DETECT_INTERVAL_SAMPLES (~100 ms), compute the difference function
//      d(τ) and its cumulative-mean-normalised form d'(τ).
//   3. Walk d'(τ) for the first dip below THRESHOLD, descend to its local min,
//      and parabolically interpolate around it for sub-sample accuracy.
//   4. Convert τ (lag in samples) to Hz: f = sample_rate / τ.
//
// O(N²) per detection, but N=1024 and we only detect at ~10 Hz, so it's about
// 10⁷ ops/sec — negligible.

const PITCH_BUFFER_SIZE: usize = 1024;
const YIN_THRESHOLD: f32 = 0.15;
/// Skip pitch detection when the RMS is below this — YIN on silence returns
/// garbage that will then poison the histogram.
const PITCH_RMS_FLOOR: f32 = 0.015;

struct PitchTracker {
    buffer: [f32; PITCH_BUFFER_SIZE],
    write_pos: usize,
    samples_since_detect: usize,
    detect_interval: usize,
    sample_rate: f32,
}

impl PitchTracker {
    fn new(sample_rate: f32) -> Self {
        Self {
            buffer: [0.0; PITCH_BUFFER_SIZE],
            write_pos: 0,
            samples_since_detect: 0,
            // ~100 ms between detections — fast enough to feel real-time,
            // slow enough that the O(N²) cost is invisible.
            detect_interval: (sample_rate * 0.1) as usize,
            sample_rate,
        }
    }

    /// Feed all channel-0 samples from a multi-channel interleaved buffer.
    fn feed_mono(&mut self, interleaved: &[f32], channels: usize, state: &SharedAudio) {
        for &s in interleaved.iter().step_by(channels.max(1)) {
            self.feed_one(s, state);
        }
    }

    /// Feed a single mono sample.
    fn feed_one(&mut self, sample: f32, state: &SharedAudio) {
        self.buffer[self.write_pos] = sample;
        self.write_pos = (self.write_pos + 1) % PITCH_BUFFER_SIZE;
        self.samples_since_detect += 1;
        if self.samples_since_detect < self.detect_interval {
            return;
        }
        self.samples_since_detect = 0;

        // Read out the buffer in chronological order.
        let mut buf = [0.0f32; PITCH_BUFFER_SIZE];
        for i in 0..PITCH_BUFFER_SIZE {
            buf[i] = self.buffer[(self.write_pos + i) % PITCH_BUFFER_SIZE];
        }

        // Cheap energy check before doing the expensive O(N²) work.
        let rms = (buf.iter().map(|x| x * x).sum::<f32>() / buf.len() as f32).sqrt();
        let pitch = if rms < PITCH_RMS_FLOOR {
            None
        } else {
            detect_pitch_yin(&buf, self.sample_rate)
        };

        if let Ok(mut guard) = state.lock() {
            if guard.is_recording {
                guard.current_pitch_hz = pitch;
            } else {
                guard.current_pitch_hz = None;
            }
        }
    }
}

fn detect_pitch_yin(buf: &[f32], sample_rate: f32) -> Option<f32> {
    let n = buf.len();
    let half = n / 2;

    // Step 1: difference function d(τ) = Σ_t (x[t] - x[t+τ])²
    let mut d = vec![0.0f32; half];
    for tau in 1..half {
        let mut sum = 0.0;
        for t in 0..half {
            let delta = buf[t] - buf[t + tau];
            sum += delta * delta;
        }
        d[tau] = sum;
    }

    // Step 2: cumulative mean normalised difference d'(τ)
    let mut d_prime = vec![1.0f32; half];
    let mut running_sum = 0.0;
    for tau in 1..half {
        running_sum += d[tau];
        if running_sum > 0.0 {
            d_prime[tau] = d[tau] * (tau as f32) / running_sum;
        } else {
            d_prime[tau] = 1.0;
        }
    }

    // Step 3: find the first τ where d'(τ) dips below threshold, then descend
    // to the local minimum at the bottom of that dip.
    let mut tau = 0usize;
    let mut t = 2usize;
    while t < half - 1 {
        if d_prime[t] < YIN_THRESHOLD {
            // Descend while still going down.
            let mut local_tau = t;
            while local_tau + 1 < half - 1 && d_prime[local_tau + 1] < d_prime[local_tau] {
                local_tau += 1;
            }
            tau = local_tau;
            break;
        }
        t += 1;
    }
    if tau == 0 {
        return None;
    }

    // Step 4: parabolic interpolation around the minimum for sub-sample accuracy.
    let interp_tau = if tau > 0 && tau + 1 < half {
        let x0 = d_prime[tau - 1];
        let x1 = d_prime[tau];
        let x2 = d_prime[tau + 1];
        let a = (x0 + x2 - 2.0 * x1) / 2.0;
        let b = (x2 - x0) / 2.0;
        if a.abs() > 1e-6 {
            tau as f32 - b / (2.0 * a)
        } else {
            tau as f32
        }
    } else {
        tau as f32
    };

    if interp_tau > 0.0 {
        Some(sample_rate / interp_tau)
    } else {
        None
    }
}
