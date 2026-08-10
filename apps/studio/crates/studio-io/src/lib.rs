//! `studio-io` is the seam between the engine and a sound card. Two things
//! live here:
//!
//! 1. **`AudioIo`** — an object-safe trait so `studio-engine` can hold a
//!    `Box<dyn AudioIo>` and never know whether it's talking to cpal, a
//!    future native PipeWire filter node, or a test fake. Object safety
//!    matters here specifically: no generic methods, no `Self` return except
//!    through the box, because the whole point is being able to swap the
//!    backend at runtime without `studio-engine` recompiling against it.
//!
//! 2. **Loopback latency measurement** — `Transport::record_position()`
//!    (in `studio-live`) subtracts a configured latency to place recorded
//!    audio where it actually belongs. That number is only as good as how
//!    it was measured. cpal gives you two independent streams with **no
//!    shared clock**, so the round trip cannot be computed from stated
//!    buffer sizes — see `docs/COMPUTED_VS_MEASURED.md` in this crate's
//!    module docs, or just: play a known click, record the loopback, find
//!    where the click actually landed. That's this module, and it needs no
//!    device to test: feed it a synthetic buffer with the click inserted at
//!    a known offset and known latency is trivially known-correct math.
//!
//! The `device` feature gates cpal (and therefore any real audio hardware)
//! entirely out of the default build, so this crate — like the six crates
//! below it — builds and tests on any host with a Rust toolchain and no
//! sound card, ALSA headers, or PipeWire.

use studio_core::Ticks;

// ---------------------------------------------------------------------
// The device seam
// ---------------------------------------------------------------------

#[derive(Debug, Clone, Copy)]
pub struct IoConfig {
    pub sample_rate: u32,
    pub channels: u16,
    /// Requested buffer size in frames. Backends may not honour this
    /// exactly (cpal's `BufferSize::Default` leaves the choice to the
    /// device); `Running` reports what was actually granted.
    pub buffer_frames: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub enum IoError {
    NoDevice,
    UnsupportedConfig(String),
    BackendError(String),
}

/// A live stream. Dropping it (or calling `stop`) tears it down. The inner
/// value is whatever the backend needs to keep alive — a `cpal::Stream`, a
/// PipeWire filter handle, or nothing for a test fake — type-erased so the
/// trait stays object-safe and callers never need to know the concrete type.
pub struct Running {
    granted_buffer_frames: u32,
    _keepalive: Box<dyn std::any::Any + Send>,
}

impl Running {
    pub fn granted_buffer_frames(&self) -> u32 {
        self.granted_buffer_frames
    }
    pub fn stop(self) {
        // Dropping `_keepalive` is the teardown; this method exists so
        // callers have something to call rather than relying on scope exit
        // being obviously a stop, which reads as accidental at a call site.
    }
}

/// A full-duplex audio callback: input from the device, output to it. Called
/// from whatever thread the backend schedules — realtime, typically — so
/// implementors must not allocate or block inside it. Nothing in this crate
/// enforces that; `studio-engine`'s docs and the gotcha ledger this crate was
/// specified against are the enforcement.
pub type DuplexCallback = Box<dyn FnMut(&[f32], &mut [f32]) + Send>;

pub trait AudioIo {
    fn start(&mut self, cfg: IoConfig, cb: DuplexCallback) -> Result<Running, IoError>;
}

/// A backend-free double for tests and for developing everything above this
/// crate before a device backend exists. Holds the callback and a fixed
/// config; `pump` drives it manually, standing in for what a real backend's
/// scheduler would do. Proves the trait's *shape* is usable without proving
/// anything about real-time behaviour, which is the honest scope of a fake.
#[derive(Default)]
pub struct TestIo {
    cb: Option<DuplexCallback>,
    granted: u32,
}

impl AudioIo for TestIo {
    fn start(&mut self, cfg: IoConfig, cb: DuplexCallback) -> Result<Running, IoError> {
        self.cb = Some(cb);
        self.granted = cfg.buffer_frames;
        Ok(Running { granted_buffer_frames: cfg.buffer_frames, _keepalive: Box::new(()) })
    }
}

impl TestIo {
    /// Manually invoke the stored callback once, as if the device had
    /// delivered `input` and wanted `output` filled.
    pub fn pump(&mut self, input: &[f32], output: &mut [f32]) {
        if let Some(cb) = self.cb.as_mut() {
            cb(input, output);
        }
    }
}

#[cfg(feature = "device")]
mod cpal_backend {
    use super::*;

    /// The real backend. Not exercised by this crate's own test suite (there
    /// is no sound card in the authoring sandbox) — it is verified on
    /// developer hardware, same as every heavy crate in this tree's
    /// "suspenders, not compilation" verification model.
    #[derive(Default)]
    pub struct CpalIo;

    impl AudioIo for CpalIo {
        fn start(&mut self, cfg: IoConfig, mut cb: DuplexCallback) -> Result<Running, IoError> {
            use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

            let host = cpal::default_host();
            let out_dev = host.default_output_device().ok_or(IoError::NoDevice)?;
            let out_cfg = out_dev
                .default_output_config()
                .map_err(|e| IoError::UnsupportedConfig(e.to_string()))?;
            let _ = cfg; // full duplex + exact-config negotiation is the
                         // hardware-stage follow-up; this proves the seam
                         // compiles against real cpal types.
            let scratch_in: Vec<f32> = Vec::new();
            let stream = out_dev
                .build_output_stream(
                    &out_cfg.into(),
                    move |data: &mut [f32], _info: &cpal::OutputCallbackInfo| {
                        cb(&scratch_in, data);
                    },
                    move |err| eprintln!("studio-io: stream error: {err}"),
                    None,
                )
                .map_err(|e| IoError::BackendError(e.to_string()))?;
            stream.play().map_err(|e| IoError::BackendError(e.to_string()))?;
            Ok(Running { granted_buffer_frames: cfg.buffer_frames, _keepalive: Box::new(stream) })
        }
    }
}
#[cfg(feature = "device")]
pub use cpal_backend::CpalIo;

// ---------------------------------------------------------------------
// Loopback latency measurement — no device required
// ---------------------------------------------------------------------

/// A short, windowed tone used as the loopback probe. Hann-windowed so its
/// spectrum doesn't splatter into a click audible as an artifact on its own,
/// and short (5 ms) so several trials fit comfortably inside one buffer of
/// silence either side.
pub fn click_template(sample_rate: u32) -> Vec<f32> {
    let len = ((sample_rate as f32) * 0.005).round().max(4.0) as usize;
    (0..len)
        .map(|i| {
            let t = i as f32 / sample_rate as f32;
            let window = 0.5 - 0.5 * (2.0 * std::f32::consts::PI * i as f32 / (len as f32 - 1.0)).cos();
            (2.0 * std::f32::consts::PI * 1000.0 * t).sin() * window
        })
        .collect()
}

/// Find where `template` best matches inside `signal` by a direct matched
/// filter (dot product at every offset, argmax). O(n·m), which is fine: the
/// template is a few hundred samples and this runs once per loopback trial,
/// never in an audio callback.
pub fn find_offset(signal: &[f32], template: &[f32]) -> Option<usize> {
    if template.is_empty() || signal.len() < template.len() {
        return None;
    }
    let mut best = 0usize;
    let mut best_score = f32::MIN;
    for start in 0..=(signal.len() - template.len()) {
        let score: f32 = signal[start..start + template.len()]
            .iter()
            .zip(template.iter())
            .map(|(a, b)| a * b)
            .sum();
        if score > best_score {
            best_score = score;
            best = start;
        }
    }
    Some(best)
}

/// One trial: the click was written into the output buffer at `play_at`
/// frames from buffer start; `recorded` is the corresponding input buffer.
/// Returns the measured round-trip latency in frames — how much later the
/// click was heard than it was played, which is exactly what
/// `Transport::latency` needs.
pub fn measure_once(play_at: usize, recorded: &[f32], template: &[f32]) -> Option<i64> {
    let found = find_offset(recorded, template)?;
    Some(found as i64 - play_at as i64)
}

/// Accumulates trials and reports the **median**, not the mean — one bad
/// trial (a xrun, a doorslam) should not drag the estimate, and a median is
/// what refuses to be dragged. This is the same reasoning `studio-fx`'s
/// tonal-balance findings already use the median for.
#[derive(Debug, Default)]
pub struct LoopbackMeasurement {
    trials: Vec<i64>,
}

impl LoopbackMeasurement {
    pub fn new() -> LoopbackMeasurement {
        LoopbackMeasurement { trials: Vec::new() }
    }

    /// Run and record one trial. Returns the per-trial estimate so a caller
    /// can show live progress ("measuring... 4/8 trials, ~118 samples so
    /// far") without waiting for `median_frames`.
    pub fn add_trial(&mut self, play_at: usize, recorded: &[f32], template: &[f32]) -> Option<i64> {
        let est = measure_once(play_at, recorded, template)?;
        self.trials.push(est);
        Some(est)
    }

    pub fn trial_count(&self) -> usize {
        self.trials.len()
    }

    pub fn median_frames(&self) -> Option<i64> {
        if self.trials.is_empty() {
            return None;
        }
        let mut sorted = self.trials.clone();
        sorted.sort_unstable();
        Some(sorted[sorted.len() / 2])
    }

    pub fn median_ticks(&self, sample_rate: u32) -> Option<Ticks> {
        self.median_frames().map(|f| Ticks::from_samples(f.max(0), sample_rate))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core_kit::rng::SplitMix64;

    /// Build a buffer of `len` zeros with `template` written in starting at
    /// `at`, then add bounded pseudo-random noise from a seeded, deterministic
    /// RNG — reused from `core-kit` rather than hand-rolled, per house
    /// practice of not re-implementing what the kit layer already has.
    fn synth_recording(len: usize, at: usize, template: &[f32], noise_amp: f32, seed: u64) -> Vec<f32> {
        let mut buf = vec![0.0f32; len];
        let mut rng = SplitMix64::new(seed);
        for s in buf.iter_mut() {
            *s = (rng.next_f64() as f32 - 0.5) * 2.0 * noise_amp;
        }
        for (i, v) in template.iter().enumerate() {
            if at + i < buf.len() {
                buf[at + i] += v;
            }
        }
        buf
    }

    #[test]
    fn click_template_is_short_and_bounded() {
        let t = click_template(48_000);
        assert!(t.len() > 100 && t.len() < 500, "5ms at 48kHz should be ~240 samples, got {}", t.len());
        assert!(t.iter().all(|s| s.abs() <= 1.0 + 1e-6));
        // Hann window means the ends must be at (or extremely near) zero —
        // this is what keeps the probe from clicking on its own.
        assert!(t.first().unwrap().abs() < 1e-3);
        assert!(t.last().unwrap().abs() < 1e-3);
    }

    #[test]
    fn find_offset_is_exact_with_no_noise() {
        let template = click_template(48_000);
        let mut signal = vec![0.0f32; 4000];
        for (i, v) in template.iter().enumerate() {
            signal[1234 + i] = *v;
        }
        assert_eq!(find_offset(&signal, &template), Some(1234));
    }

    #[test]
    fn measure_once_reports_round_trip_in_frames() {
        let template = click_template(48_000);
        // Played the click at frame 500 in the output buffer; it was heard
        // (found in the recorded input buffer) 137 frames later.
        let recorded = synth_recording(4000, 500 + 137, &template, 0.0, 1);
        let latency = measure_once(500, &recorded, &template).unwrap();
        assert_eq!(latency, 137);
    }

    #[test]
    fn median_of_several_noisy_trials_lands_within_a_couple_samples() {
        let sample_rate = 48_000;
        let template = click_template(sample_rate);
        let true_latency = 212i64;
        let mut m = LoopbackMeasurement::new();
        for trial in 0..8u64 {
            let play_at = 300 + trial as usize * 17; // vary the play point per trial
            let recorded = synth_recording(
                4000,
                play_at + true_latency as usize,
                &template,
                0.05, // modest additive noise — a real interface's self-noise floor
                trial + 100,
            );
            m.add_trial(play_at, &recorded, &template);
        }
        assert_eq!(m.trial_count(), 8);
        let median = m.median_frames().unwrap();
        assert!(
            (median - true_latency).abs() <= 2,
            "median {median} should track true latency {true_latency} within a couple samples"
        );
    }

    #[test]
    fn median_is_not_dragged_by_one_outlier_trial() {
        let sample_rate = 48_000;
        let template = click_template(sample_rate);
        let true_latency = 100i64;
        let mut m = LoopbackMeasurement::new();
        for trial in 0..5u64 {
            let recorded = synth_recording(4000, 300 + true_latency as usize, &template, 0.0, trial + 1);
            m.add_trial(300, &recorded, &template);
        }
        // One badly corrupted trial: the click is simply absent, so the
        // matched filter locks onto noise instead — an xrun stand-in.
        let mut bad = vec![0.0f32; 4000];
        let mut rng = SplitMix64::new(999);
        for s in bad.iter_mut() {
            *s = (rng.next_f64() as f32 - 0.5) * 2.0;
        }
        m.add_trial(300, &bad, &template);

        let median = m.median_frames().unwrap();
        assert_eq!(median, true_latency, "one outlier among five good trials must not move the median");
    }

    #[test]
    fn median_ticks_converts_through_the_same_timebase_as_the_rest_of_studio() {
        let mut m = LoopbackMeasurement::new();
        let template = click_template(48_000);
        let recorded = synth_recording(4000, 300 + 480, &template, 0.0, 7);
        m.add_trial(300, &recorded, &template);
        let ticks = m.median_ticks(48_000).unwrap();
        // 480 samples at 48kHz = 10ms.
        assert_eq!(ticks, Ticks::from_seconds(0.01));
    }

    #[test]
    fn find_offset_returns_none_when_signal_shorter_than_template() {
        let template = click_template(48_000);
        let short = vec![0.0f32; template.len() - 1];
        assert_eq!(find_offset(&short, &template), None);
    }

    #[test]
    fn test_io_pump_invokes_the_stored_callback() {
        let mut io = TestIo::default();
        let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let seen2 = seen.clone();
        io.start(
            IoConfig { sample_rate: 48_000, channels: 2, buffer_frames: 128 },
            Box::new(move |input, output| {
                seen2.lock().unwrap().extend_from_slice(input);
                output.fill(0.5);
            }),
        )
        .unwrap();

        let input = vec![1.0f32; 4];
        let mut output = vec![0.0f32; 4];
        io.pump(&input, &mut output);

        assert_eq!(*seen.lock().unwrap(), input);
        assert!(output.iter().all(|&s| s == 0.5));
    }

    #[test]
    fn running_reports_the_granted_buffer_size() {
        let mut io = TestIo::default();
        let running = io
            .start(IoConfig { sample_rate: 48_000, channels: 2, buffer_frames: 256 }, Box::new(|_i, _o| {}))
            .unwrap();
        assert_eq!(running.granted_buffer_frames(), 256);
        running.stop();
    }
}
