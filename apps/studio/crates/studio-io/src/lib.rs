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

impl std::fmt::Display for IoError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            IoError::NoDevice => write!(f, "no audio device available"),
            IoError::UnsupportedConfig(msg) => write!(f, "unsupported audio config: {msg}"),
            IoError::BackendError(msg) => write!(f, "audio backend error: {msg}"),
        }
    }
}

impl std::error::Error for IoError {}

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
    use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
    use cpal::{BufferSize, SampleFormat, SampleRate, StreamConfig, SupportedBufferSize};
    use dasp_sample::{FromSample, Sample as DaspSample};
    use std::sync::mpsc;
    use std::time::Duration;

    /// Frames of slack in the input→output bridge. Comfortably above one
    /// callback's worth (typical buffers are a few hundred to a couple
    /// thousand frames) so ordinary scheduler jitter doesn't underrun, but
    /// small enough that it doesn't add a second, invisible tier of latency
    /// on top of the device's own — `Running::granted_buffer_frames` and the
    /// loopback measurement above only account for the device, not this.
    const BRIDGE_FRAMES: u32 = 4096;

    /// The real backend.
    ///
    /// `cpal::Stream` is `!Send` on every backend (its platform handle is a
    /// `PhantomData<*mut ()>`-tagged type) so it cannot be boxed into
    /// `Running`'s `Box<dyn Any + Send>` the way the seam's first draft did —
    /// confirmed by `cargo build --features device` failing with E0277
    /// (`*mut () cannot be sent between threads safely`) before this patch.
    /// There was no sound card in the authoring sandbox to have caught that
    /// any other way; it needed the feature actually compiled, not just
    /// read.
    ///
    /// The fix is the standard shape for a `!Send` resource: a dedicated OS
    /// thread owns the streams for the life of the session, and only a stop
    /// signal and a join handle — both plain `Send` types — cross back to
    /// the caller inside `Running`.
    ///
    /// That thread also does the full-duplex bridging cpal itself doesn't
    /// provide: `default_host()` hands out independent input and output
    /// streams with no shared clock, so an input callback writes into an
    /// [`rtrb`] ring buffer and the output callback drains it, padding with
    /// silence on underrun (no input device, or the input side hasn't
    /// caught up yet) rather than blocking — blocking an audio callback to
    /// wait for data is how a buffer underrun becomes an audible glitch on
    /// *every* stream instead of a dropped frame on one.
    #[derive(Default)]
    pub struct CpalIo;

    impl AudioIo for CpalIo {
        fn start(&mut self, cfg: IoConfig, cb: DuplexCallback) -> Result<Running, IoError> {
            let (ready_tx, ready_rx) = mpsc::channel::<Result<u32, IoError>>();
            let (stop_tx, stop_rx) = mpsc::channel::<()>();

            let handle = std::thread::Builder::new()
                .name("studio-io-cpal".into())
                .spawn(move || run_audio_thread(cfg, cb, ready_tx, stop_rx))
                .map_err(|e| IoError::BackendError(e.to_string()))?;

            // The thread reports back once streams are actually playing (or
            // have failed to start) so `start` returns a config that is
            // really running rather than one merely requested — same
            // reasoning as `Running::granted_buffer_frames` existing at all.
            match ready_rx.recv_timeout(Duration::from_secs(3)) {
                Ok(Ok(granted_buffer_frames)) => Ok(Running {
                    granted_buffer_frames,
                    _keepalive: Box::new(CpalHandle { stop_tx: Some(stop_tx), join: Some(handle) }),
                }),
                Ok(Err(e)) => {
                    let _ = handle.join();
                    Err(e)
                }
                Err(_) => {
                    let _ = stop_tx.send(());
                    let _ = handle.join();
                    Err(IoError::BackendError("audio thread did not confirm startup in time".into()))
                }
            }
        }
    }

    /// Owns the stop signal and the audio thread's join handle — both
    /// `Send`, unlike the streams themselves, which is exactly why they
    /// (not the streams) are what crosses back into `Running`. Dropping this
    /// (via `Running` being dropped, or `Running::stop`) signals the thread
    /// and waits for it, so the streams are guaranteed torn down before this
    /// returns rather than left to an arbitrary future point.
    struct CpalHandle {
        stop_tx: Option<mpsc::Sender<()>>,
        join: Option<std::thread::JoinHandle<()>>,
    }

    impl Drop for CpalHandle {
        fn drop(&mut self) {
            if let Some(tx) = self.stop_tx.take() {
                let _ = tx.send(());
            }
            if let Some(j) = self.join.take() {
                let _ = j.join();
            }
        }
    }

    /// Runs entirely on the dedicated audio thread: negotiates configs,
    /// builds the streams, bridges them, and parks until told to stop.
    /// Everything cpal hands back — `Device`, `Stream` — lives and dies on
    /// this one thread; nothing of them crosses back to the caller.
    fn run_audio_thread(
        cfg: IoConfig,
        mut cb: DuplexCallback,
        ready_tx: mpsc::Sender<Result<u32, IoError>>,
        stop_rx: mpsc::Receiver<()>,
    ) {
        let host = cpal::default_host();
        let out_dev = match host.default_output_device() {
            Some(d) => d,
            None => {
                let _ = ready_tx.send(Err(IoError::NoDevice));
                return;
            }
        };

        let (out_cfg, out_format) = match negotiate(&out_dev, &cfg, true) {
            Ok(v) => v,
            Err(e) => {
                let _ = ready_tx.send(Err(e));
                return;
            }
        };

        // Input is best-effort: a machine with no microphone, or one whose
        // input permission was declined, still gets output — the same
        // "must still open" bargain the rest of Studio makes for a project
        // that references a missing plugin or an unloaded model.
        let in_dev = host.default_input_device();
        let in_negotiated = in_dev.as_ref().and_then(|d| negotiate(d, &cfg, false).ok());

        let bridge_capacity = (BRIDGE_FRAMES as usize) * (cfg.channels.max(1) as usize);
        let (prod, mut cons) = rtrb::RingBuffer::<f32>::new(bridge_capacity.max(1));

        let in_stream = match (&in_dev, &in_negotiated) {
            (Some(dev), Some((in_stream_cfg, in_format))) => {
                let built = match *in_format {
                    SampleFormat::F32 => build_input_stream::<f32>(dev, in_stream_cfg, prod),
                    SampleFormat::I16 => build_input_stream::<i16>(dev, in_stream_cfg, prod),
                    other => Err(IoError::UnsupportedConfig(format!(
                        "input sample format {other} not supported yet (only f32/i16)"
                    ))),
                };
                match built {
                    Ok(s) => match s.play() {
                        Ok(()) => Some(s),
                        Err(e) => {
                            eprintln!("studio-io: input stream failed to start, continuing with silence: {e}");
                            None
                        }
                    },
                    Err(e) => {
                        eprintln!("studio-io: input stream unavailable, continuing with silence: {e}");
                        None
                    }
                }
            }
            _ => None,
        };

        let granted_buffer_frames = match out_cfg.buffer_size {
            BufferSize::Fixed(n) => n,
            BufferSize::Default => cfg.buffer_frames,
        };

        let mut in_scratch: Vec<f32> = Vec::new();
        let fill = move |out: &mut [f32]| {
            in_scratch.clear();
            in_scratch.resize(out.len(), 0.0);
            for slot in in_scratch.iter_mut() {
                match cons.pop() {
                    Ok(s) => *slot = s,
                    // Underrun (no input device, or the producer hasn't
                    // caught up yet): leave the rest of this callback's
                    // input as silence rather than blocking for it.
                    Err(_) => break,
                }
            }
            cb(&in_scratch, out);
        };

        let out_stream = match out_format {
            SampleFormat::F32 => build_output_stream::<f32>(&out_dev, &out_cfg, fill),
            SampleFormat::I16 => build_output_stream::<i16>(&out_dev, &out_cfg, fill),
            other => Err(IoError::UnsupportedConfig(format!(
                "output sample format {other} not supported yet (only f32/i16)"
            ))),
        };
        let out_stream = match out_stream {
            Ok(s) => s,
            Err(e) => {
                let _ = ready_tx.send(Err(e));
                return;
            }
        };
        if let Err(e) = out_stream.play() {
            let _ = ready_tx.send(Err(IoError::BackendError(e.to_string())));
            return;
        }

        let _ = ready_tx.send(Ok(granted_buffer_frames));
        let _ = stop_rx.recv(); // park until CpalHandle::drop signals stop
        drop(out_stream);
        drop(in_stream);
    }

    /// Picks a config close to `requested` from what the device actually
    /// supports, preferring an exact channel-count and F32 match, then
    /// falling back progressively (matching channels in any format, then
    /// any config at all) rather than failing outright — a project should
    /// open on whatever interface is plugged in, not only the one it was
    /// authored against. `buffer_frames` is only honoured as `Fixed` when
    /// the device's own reported range covers it; forcing a size outside
    /// that range is a `BuildStreamError` waiting to happen, not a request
    /// a device can simply be told to grant.
    fn negotiate(
        dev: &cpal::Device,
        requested: &IoConfig,
        output: bool,
    ) -> Result<(StreamConfig, SampleFormat), IoError> {
        let ranges: Vec<cpal::SupportedStreamConfigRange> = if output {
            dev.supported_output_configs()
                .map_err(|e| IoError::UnsupportedConfig(format!("{e:?}")))?
                .collect()
        } else {
            dev.supported_input_configs()
                .map_err(|e| IoError::UnsupportedConfig(format!("{e:?}")))?
                .collect()
        };
        pick_config(&ranges, requested)
    }

    /// The selection logic proper, pulled out of [`negotiate`] so it can be
    /// tested against hand-built [`cpal::SupportedStreamConfigRange`]s —
    /// `SupportedStreamConfigRange::new` is a public constructor, so this
    /// needs no device either, same reasoning `studio-egui`'s time↔pixel
    /// arithmetic is kept `Ui`-free: the part most likely to be got subtly
    /// wrong (fallback order, an off-by-one at a range boundary) is exactly
    /// the part worth being able to test without hardware.
    fn pick_config(
        ranges: &[cpal::SupportedStreamConfigRange],
        requested: &IoConfig,
    ) -> Result<(StreamConfig, SampleFormat), IoError> {
        if ranges.is_empty() {
            return Err(IoError::UnsupportedConfig("device reports no supported configs".into()));
        }

        let rate_ok = |r: &cpal::SupportedStreamConfigRange| {
            r.min_sample_rate().0 <= requested.sample_rate && requested.sample_rate <= r.max_sample_rate().0
        };
        let picked = ranges
            .iter()
            .find(|r| r.channels() == requested.channels && r.sample_format() == SampleFormat::F32 && rate_ok(r))
            .or_else(|| ranges.iter().find(|r| r.channels() == requested.channels && rate_ok(r)))
            .or_else(|| ranges.iter().find(|r| r.sample_format() == SampleFormat::F32))
            .or_else(|| ranges.first())
            .copied()
            .expect("checked non-empty above");

        let clamped_rate = requested.sample_rate.clamp(picked.min_sample_rate().0, picked.max_sample_rate().0);
        let supported = picked.with_sample_rate(SampleRate(clamped_rate));
        let format = supported.sample_format();
        let mut stream_cfg = supported.config();
        if let SupportedBufferSize::Range { min, max } = supported.buffer_size() {
            if requested.buffer_frames >= *min && requested.buffer_frames <= *max {
                stream_cfg.buffer_size = BufferSize::Fixed(requested.buffer_frames);
            }
        }
        Ok((stream_cfg, format))
    }

    /// Builds an input stream of concrete sample type `T`, converting every
    /// sample to `f32` (the ring buffer's — and [`DuplexCallback`]'s — only
    /// currency) via `dasp_sample`'s conversion, and pushing it into the
    /// bridge. A full ring buffer means the reader side hasn't kept up;
    /// `push` failing is that overrun, and the sample is dropped rather than
    /// the callback blocking to wait for room.
    fn build_input_stream<T>(
        dev: &cpal::Device,
        cfg: &StreamConfig,
        mut prod: rtrb::Producer<f32>,
    ) -> Result<cpal::Stream, IoError>
    where
        T: cpal::SizedSample + DaspSample,
        f32: FromSample<T>,
    {
        dev.build_input_stream(
            cfg,
            move |data: &[T], _: &cpal::InputCallbackInfo| {
                for &s in data {
                    let f: f32 = f32::from_sample(s);
                    let _ = prod.push(f);
                }
            },
            |err| eprintln!("studio-io: input stream error: {err}"),
            None,
        )
        .map_err(|e| IoError::BackendError(e.to_string()))
    }

    /// Builds an output stream of concrete sample type `T`. `fill` produces
    /// one callback's worth of `f32` (it is `run_audio_thread`'s closure
    /// over the ring buffer and the user's [`DuplexCallback`]); this
    /// function's only job is converting that into whatever format the
    /// device actually wants.
    fn build_output_stream<T>(
        dev: &cpal::Device,
        cfg: &StreamConfig,
        mut fill: impl FnMut(&mut [f32]) + Send + 'static,
    ) -> Result<cpal::Stream, IoError>
    where
        T: cpal::SizedSample + DaspSample,
        T: FromSample<f32>,
    {
        let mut scratch: Vec<f32> = Vec::new();
        dev.build_output_stream(
            cfg,
            move |data: &mut [T], _: &cpal::OutputCallbackInfo| {
                scratch.clear();
                scratch.resize(data.len(), 0.0);
                fill(&mut scratch);
                for (o, &f) in data.iter_mut().zip(scratch.iter()) {
                    *o = T::from_sample(f);
                }
            },
            |err| eprintln!("studio-io: output stream error: {err}"),
            None,
        )
        .map_err(|e| IoError::BackendError(e.to_string()))
    }

    #[cfg(test)]
    mod pick_config_tests {
        use super::*;

        fn range(
            channels: u16,
            min_hz: u32,
            max_hz: u32,
            buf: Option<(u32, u32)>,
            format: SampleFormat,
        ) -> cpal::SupportedStreamConfigRange {
            let buffer_size = match buf {
                Some((min, max)) => SupportedBufferSize::Range { min, max },
                None => SupportedBufferSize::Unknown,
            };
            cpal::SupportedStreamConfigRange::new(
                channels,
                SampleRate(min_hz),
                SampleRate(max_hz),
                buffer_size,
                format,
            )
        }

        fn wanted(channels: u16, sample_rate: u32, buffer_frames: u32) -> IoConfig {
            IoConfig { sample_rate, channels, buffer_frames }
        }

        #[test]
        fn no_ranges_is_a_clean_error_not_a_panic() {
            let err = pick_config(&[], &wanted(2, 48_000, 512)).unwrap_err();
            assert!(matches!(err, IoError::UnsupportedConfig(_)));
        }

        #[test]
        fn prefers_exact_channels_and_f32_when_available() {
            let ranges = vec![
                range(2, 44_100, 48_000, None, SampleFormat::I16),
                range(2, 44_100, 48_000, None, SampleFormat::F32),
                range(1, 44_100, 48_000, None, SampleFormat::F32),
            ];
            let (cfg, format) = pick_config(&ranges, &wanted(2, 48_000, 512)).unwrap();
            assert_eq!(format, SampleFormat::F32);
            assert_eq!(cfg.channels, 2);
            assert_eq!(cfg.sample_rate.0, 48_000);
        }

        #[test]
        fn falls_back_to_matching_channels_in_any_format_over_wrong_channel_count() {
            // Only I16 supports the requested channel count; a wrong-channel
            // F32 range exists too. Getting the channel count right matters
            // more than getting the sample format right — a stream opened
            // on the wrong channel count is unusable, a converted format
            // is merely extra work already handled by build_output_stream.
            let ranges = vec![
                range(1, 44_100, 48_000, None, SampleFormat::F32),
                range(2, 44_100, 48_000, None, SampleFormat::I16),
            ];
            let (cfg, format) = pick_config(&ranges, &wanted(2, 48_000, 512)).unwrap();
            assert_eq!(cfg.channels, 2);
            assert_eq!(format, SampleFormat::I16);
        }

        #[test]
        fn falls_back_to_any_config_when_nothing_matches_channels_or_rate() {
            let ranges = vec![range(6, 8_000, 16_000, None, SampleFormat::I16)];
            let (cfg, _format) = pick_config(&ranges, &wanted(2, 48_000, 512)).unwrap();
            // Nothing requested is honoured, but a config comes back rather
            // than an error — a project should open on whatever interface
            // is plugged in.
            assert_eq!(cfg.channels, 6);
        }

        #[test]
        fn sample_rate_is_clamped_into_the_supported_range_not_rejected() {
            let ranges = vec![range(2, 44_100, 96_000, None, SampleFormat::F32)];
            let (cfg, _) = pick_config(&ranges, &wanted(2, 192_000, 512)).unwrap();
            assert_eq!(cfg.sample_rate.0, 96_000);

            let (cfg, _) = pick_config(&ranges, &wanted(2, 8_000, 512)).unwrap();
            assert_eq!(cfg.sample_rate.0, 44_100);
        }

        #[test]
        fn buffer_frames_fixed_when_device_range_covers_the_request() {
            let ranges = vec![range(2, 44_100, 48_000, Some((64, 2048)), SampleFormat::F32)];
            let (cfg, _) = pick_config(&ranges, &wanted(2, 48_000, 256)).unwrap();
            assert_eq!(cfg.buffer_size, BufferSize::Fixed(256));
        }

        #[test]
        fn buffer_frames_left_default_when_request_is_outside_the_device_range() {
            // Forcing a size outside what the device reports it can grant is
            // a `BuildStreamError` waiting to happen, not a request a device
            // can simply be told to satisfy — falling back to `Default`
            // here is what keeps `negotiate` honest about that.
            let ranges = vec![range(2, 44_100, 48_000, Some((256, 512)), SampleFormat::F32)];
            let (cfg, _) = pick_config(&ranges, &wanted(2, 48_000, 4096)).unwrap();
            assert_eq!(cfg.buffer_size, BufferSize::Default);
        }

        #[test]
        fn unknown_device_buffer_range_also_leaves_default() {
            let ranges = vec![range(2, 44_100, 48_000, None, SampleFormat::F32)];
            let (cfg, _) = pick_config(&ranges, &wanted(2, 48_000, 256)).unwrap();
            assert_eq!(cfg.buffer_size, BufferSize::Default);
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
