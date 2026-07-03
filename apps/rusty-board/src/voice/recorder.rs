//! Audio capture via `cpal`, written into a growable buffer for the
//! transcriber to consume.

use anyhow::{anyhow, Context, Result};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use parking_lot::Mutex;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use tracing::{debug, info, warn};

/// Growable audio buffer for captured samples.
///
/// Replaces the old fixed-capacity ring. The ring had two bugs that
/// combined to cut off long dictation while the transcribing indicator
/// kept running:
///
///   1. It was sized `16_000 * seconds`, but samples are stored at the
///      device's NATIVE rate (often 44.1/48 kHz), so a "30 second"
///      buffer actually held ~10 s on a 48 kHz device.
///   2. On overflow it dropped the NEWEST samples (`try_push` fails when
///      full and the error was ignored), so everything after the cap was
///      silently lost while recording continued.
///
/// `GrowBuf` grows with the recording, so it only uses memory
/// proportional to what's actually captured (a few MB for typical
/// dictation). A generous safety ceiling caps runaway recording, and
/// when hit it drops the OLDEST audio — keeping the most recent speech,
/// the opposite of the old behavior.
pub struct GrowBuf {
    data: Vec<f32>,
    /// Hard ceiling in samples. Only reached by pathological non-stop
    /// recording; normal dictation never approaches it.
    max_samples: usize,
}

impl GrowBuf {
    fn new(max_samples: usize) -> Self {
        Self {
            data: Vec::new(),
            max_samples,
        }
    }

    #[inline]
    fn push(&mut self, s: f32) {
        self.data.push(s);
        if self.data.len() > self.max_samples {
            // Drop the oldest samples to get back under the ceiling,
            // plus a slack chunk so this O(n) shift amortizes (it
            // happens at most ~once per second of overflow rather than
            // every sample). Keeps the most recent audio.
            let overflow = self.data.len() - self.max_samples;
            let drop = (overflow + 16_000).min(self.data.len());
            self.data.drain(0..drop);
        }
    }

    /// Take all samples, leaving the buffer empty.
    fn drain_all(&mut self) -> Vec<f32> {
        std::mem::take(&mut self.data)
    }

    /// Copy all samples without consuming them.
    fn snapshot(&self) -> Vec<f32> {
        self.data.clone()
    }

    fn len(&self) -> usize {
        self.data.len()
    }
}

/// Safety ceiling for the capture buffer, in seconds. Computed against a
/// conservative worst-case native rate so the time bound holds even on
/// high-rate devices. This is NOT a dictation cap users will hit in
/// normal use — it only bounds runaway recording (e.g. a stuck stream).
/// 5 minutes at 48 kHz ≈ 14.4M samples ≈ 58 MB worst case; typical
/// dictation of a few seconds uses a few MB.
const MAX_BUFFER_SECONDS: usize = 300;
const ASSUMED_MAX_RATE: usize = 48_000;

pub type AudioRing = Arc<Mutex<GrowBuf>>;

/// Shared atomic peak-level meter. The cpal input callback updates
/// it on each buffer; the UI polls it from the main thread. Stored
/// as an f32's u32 bit-representation in an AtomicU32 so the audio
/// thread can update lock-free.
///
/// Values are in [0.0, 1.0] (clamped). 0 = silence, 1 = clipping.
/// The bridge applies its own decay/smoothing when displaying.
pub type LevelMeter = Arc<AtomicU32>;

pub struct AudioRecorder {
    /// The active stream handle. cpal requires we keep it alive to keep
    /// recording.
    stream: Option<cpal::Stream>,
    ring: AudioRing,
    level: LevelMeter,
    /// True iff a stream is currently open.
    running: bool,
    /// Native input sample rate of the device. Whisper wants 16 kHz, so we
    /// downsample on read if this differs.
    input_sample_rate: u32,
}

impl AudioRecorder {
    pub fn new(buffer_seconds: usize) -> Self {
        // The buffer GROWS with the recording, so buffer_seconds is no
        // longer a hard cap — it just seeds the safety ceiling. We size
        // the ceiling against a conservative worst-case native rate so
        // the time bound holds on high-rate (44.1/48 kHz) devices, and
        // honor an unusually large requested buffer_seconds if it
        // exceeds the default ceiling.
        let ceiling_seconds = buffer_seconds.max(MAX_BUFFER_SECONDS);
        let max_samples = ASSUMED_MAX_RATE * ceiling_seconds;
        let buf = GrowBuf::new(max_samples);
        Self {
            stream: None,
            ring: Arc::new(Mutex::new(buf)),
            level: Arc::new(AtomicU32::new(0)),
            running: false,
            input_sample_rate: 16_000,
        }
    }

    pub fn ring(&self) -> AudioRing {
        self.ring.clone()
    }

    /// Cloneable handle to the peak-level meter. The Arc lets the
    /// audio thread write and the UI thread read without locking.
    pub fn level_meter(&self) -> LevelMeter {
        self.level.clone()
    }

    /// Convenience: read the current peak as f32 in [0, 1].
    /// Equivalent to `f32::from_bits(meter.load(Relaxed))` for the
    /// caller, but spelled out so callers don't have to import
    /// Ordering.
    pub fn current_level(&self) -> f32 {
        f32::from_bits(self.level.load(Ordering::Relaxed))
            .clamp(0.0, 1.0)
    }

    pub fn input_sample_rate(&self) -> u32 {
        self.input_sample_rate
    }

    pub fn is_running(&self) -> bool {
        self.running
    }

    pub fn start(&mut self) -> Result<()> {
        if self.running {
            return Ok(());
        }
        let host = cpal::default_host();
        let device = host
            .default_input_device()
            .ok_or_else(|| anyhow!("no default input device"))?;
        let device_name = device.name().unwrap_or_else(|_| "?".into());

        let supported = device
            .default_input_config()
            .context("getting default input config")?;
        let config = supported.config();
        self.input_sample_rate = config.sample_rate.0;
        info!(
            "Recording from '{}' at {} Hz, {} channels, {:?}",
            device_name, config.sample_rate.0, config.channels, supported.sample_format()
        );

        let ring = self.ring.clone();
        let level = self.level.clone();
        let channels = config.channels as usize;

        let err_fn = |e| warn!("audio stream error: {}", e);

        let stream = match supported.sample_format() {
            cpal::SampleFormat::F32 => {
                let ring2 = ring.clone();
                let level2 = level.clone();
                device.build_input_stream(
                    &config,
                    move |data: &[f32], _: &cpal::InputCallbackInfo| {
                        push_samples_mono(data, channels, &ring2, &level2);
                    },
                    err_fn,
                    None,
                )
            }
            cpal::SampleFormat::I16 => {
                let ring2 = ring.clone();
                let level2 = level.clone();
                device.build_input_stream(
                    &config,
                    move |data: &[i16], _: &cpal::InputCallbackInfo| {
                        let conv: Vec<f32> = data
                            .iter()
                            .map(|s| *s as f32 / 32768.0)
                            .collect();
                        push_samples_mono(&conv, channels, &ring2, &level2);
                    },
                    err_fn,
                    None,
                )
            }
            cpal::SampleFormat::U16 => {
                let ring2 = ring.clone();
                let level2 = level.clone();
                device.build_input_stream(
                    &config,
                    move |data: &[u16], _: &cpal::InputCallbackInfo| {
                        let conv: Vec<f32> = data
                            .iter()
                            .map(|s| (*s as f32 - 32768.0) / 32768.0)
                            .collect();
                        push_samples_mono(&conv, channels, &ring2, &level2);
                    },
                    err_fn,
                    None,
                )
            }
            other => {
                return Err(anyhow!("unsupported sample format: {:?}", other));
            }
        }
        .context("building input stream")?;

        stream.play().context("starting input stream")?;
        self.stream = Some(stream);
        self.running = true;
        Ok(())
    }

    pub fn stop(&mut self) {
        if let Some(s) = self.stream.take() {
            drop(s); // dropping stops the stream
        }
        self.running = false;
        debug!("Recorder stopped");
    }

    /// Drain everything currently in the ring buffer into a Vec<f32> at the
    /// device's native sample rate.
    pub fn drain(&self) -> Vec<f32> {
        let mut ring = self.ring.lock();
        ring.drain_all()
    }

    /// Non-destructive snapshot of the current ring contents. Returns
    /// a copy of all samples currently in the buffer WITHOUT popping
    /// them — so a subsequent `drain()` still sees them.
    ///
    /// Used by the streaming-partial path: it wants to re-run
    /// inference on the recording-so-far every ~600ms while the user
    /// is still speaking, without disrupting the eventual final
    /// inference that runs when the user stops.
    ///
    /// Cost: a memcpy of up to `buffer_seconds * sample_rate` floats
    /// (e.g. 30s * 48kHz * 4 bytes = ~5.5 MB worst case). At the
    /// streaming cadence of ~600ms between partials, this is well
    /// below the inference cost itself.
    pub fn snapshot(&self) -> Vec<f32> {
        snapshot_ring(&self.ring)
    }
}

/// Free-function snapshot. Reads the current contents of a ring
/// without consuming them, returning a Vec<f32> copy.
///
/// Callable from worker threads that hold an `AudioRing` clone but
/// not the (non-Send) `AudioRecorder`. The recorder itself isn't
/// Send because it owns a `cpal::Stream`, which cpal pins to its
/// creation thread on every platform. The ring buffer behind the
/// Arc<Mutex<>> is unaffected — it's just a thread-safe ringbuf.
pub fn snapshot_ring(ring: &AudioRing) -> Vec<f32> {
    let r = ring.lock();
    r.snapshot()
}

impl Drop for AudioRecorder {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Mix multi-channel samples down to mono, push into the ring buffer,
/// AND update the peak-level meter for UI display.
///
/// The peak is the max abs value seen in this buffer. We blend with
/// the previous peak using exponential decay so the meter falls back
/// toward zero between speech bursts: `new = max(peak, old * 0.85)`.
/// That gives a meter that's responsive to onsets (peaks shoot up
/// instantly) but smooths out the gaps (peaks decay over ~10
/// callbacks rather than dropping to zero on the first silent
/// buffer). Tuned for cpal's typical buffer cadence of ~10ms.
fn push_samples_mono(
    data: &[f32],
    channels: usize,
    ring: &AudioRing,
    level: &LevelMeter,
) {
    if channels == 0 || data.is_empty() {
        return;
    }

    // Compute peak of this buffer.
    let mut peak: f32 = 0.0;
    for &s in data {
        let a = s.abs();
        if a > peak {
            peak = a;
        }
    }

    // Push to buffer (mono mixdown). GrowBuf::push appends without
    // dropping newest samples; it only drops OLDEST if the safety
    // ceiling is hit, which normal dictation never reaches.
    {
        let mut r = ring.lock();
        if channels == 1 {
            for &s in data {
                r.push(s);
            }
        } else {
            for frame in data.chunks_exact(channels) {
                let mean: f32 = frame.iter().sum::<f32>() / channels as f32;
                r.push(mean);
            }
        }
    }

    // Update level with decay. CAS loop because the audio thread is
    // the only writer but a stale read between load and store would
    // mean the previous-value blend uses the wrong baseline.
    let mut prev_bits = level.load(Ordering::Relaxed);
    loop {
        let prev = f32::from_bits(prev_bits).clamp(0.0, 1.0);
        let decayed = prev * 0.85;
        let new = if peak > decayed { peak } else { decayed };
        let new = new.clamp(0.0, 1.0);
        match level.compare_exchange_weak(
            prev_bits,
            new.to_bits(),
            Ordering::Relaxed,
            Ordering::Relaxed,
        ) {
            Ok(_) => break,
            Err(actual) => prev_bits = actual,
        }
    }
}

/// Resample audio to 16 kHz, the rate Whisper expects. Uses simple linear
/// interpolation — adequate for speech.
pub fn resample_to_16k(samples: &[f32], from_rate: u32) -> Vec<f32> {
    if from_rate == 16_000 {
        return samples.to_vec();
    }
    let ratio = 16_000.0 / from_rate as f64;
    let out_len = (samples.len() as f64 * ratio) as usize;
    let mut out = Vec::with_capacity(out_len);
    for i in 0..out_len {
        let src_idx = i as f64 / ratio;
        let lo = src_idx.floor() as usize;
        let hi = (lo + 1).min(samples.len() - 1);
        let t = (src_idx - lo as f64) as f32;
        if lo >= samples.len() {
            break;
        }
        out.push(samples[lo] * (1.0 - t) + samples[hi] * t);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resample_identity_at_16k() {
        let s = vec![0.1, 0.2, 0.3];
        let out = resample_to_16k(&s, 16_000);
        assert_eq!(out, s);
    }

    #[test]
    fn resample_downsamples_48k_to_16k() {
        let s: Vec<f32> = (0..480).map(|i| i as f32 / 480.0).collect();
        let out = resample_to_16k(&s, 48_000);
        // Roughly 1/3 the length.
        assert!(out.len() >= 158 && out.len() <= 162, "got {}", out.len());
    }

    #[test]
    fn resample_upsamples_8k_to_16k() {
        let s: Vec<f32> = (0..80).map(|i| i as f32 / 80.0).collect();
        let out = resample_to_16k(&s, 8_000);
        assert!(out.len() >= 158 && out.len() <= 162, "got {}", out.len());
    }

    #[test]
    fn ring_starts_empty() {
        let rec = AudioRecorder::new(1);
        assert_eq!(rec.drain().len(), 0);
        assert!(!rec.is_running());
    }

    #[test]
    fn growbuf_grows_past_old_fixed_cap() {
        // The old fixed ring capped at 16_000 * buffer_seconds samples
        // and dropped NEWEST on overflow. GrowBuf must accept far more
        // than that without losing samples — here 10x the old "30s"
        // sizing (480k) to prove the cut-off is gone.
        let mut buf = GrowBuf::new(ASSUMED_MAX_RATE * MAX_BUFFER_SECONDS);
        let n = 16_000 * 300; // 5 min @ 16k = 4.8M samples
        for i in 0..n {
            buf.push((i % 7) as f32 * 0.01);
        }
        assert_eq!(buf.len(), n, "all pushed samples retained");
    }

    #[test]
    fn growbuf_drops_oldest_at_ceiling_not_newest() {
        // Small ceiling to exercise overflow quickly. The MOST RECENT
        // samples must survive; the oldest get dropped.
        let ceiling = 1000;
        let mut buf = GrowBuf::new(ceiling);
        // Push a ramp so each sample is identifiable by value.
        let total = ceiling + 16_000 + 500; // forces at least one drop pass
        for i in 0..total {
            buf.push(i as f32);
        }
        let snap = buf.snapshot();
        assert!(snap.len() <= ceiling + 16_000, "stays bounded");
        // The LAST sample pushed must be present and last — newest kept.
        assert_eq!(*snap.last().unwrap(), (total - 1) as f32);
        // The very first sample (0.0) must have been dropped.
        assert!(snap[0] > 0.0, "oldest dropped, not newest");
    }

    #[test]
    fn growbuf_drain_empties_snapshot_does_not() {
        let mut buf = GrowBuf::new(100_000);
        for i in 0..100 {
            buf.push(i as f32);
        }
        let snap = buf.snapshot();
        assert_eq!(snap.len(), 100);
        assert_eq!(buf.len(), 100, "snapshot is non-destructive");
        let drained = buf.drain_all();
        assert_eq!(drained.len(), 100);
        assert_eq!(buf.len(), 0, "drain empties");
    }

    #[test]
    fn recorder_buffer_seconds_seeds_generous_ceiling() {
        // Even a tiny requested buffer gets at least the default
        // ceiling, so a small buffer_seconds can't reintroduce a cap.
        let rec = AudioRecorder::new(1);
        let ring = rec.ring();
        let mut g = ring.lock();
        // Push 2 minutes of 48k audio (5.76M samples) — would have
        // overflowed the old 16k*1 = 16k ring instantly.
        for i in 0..(48_000 * 120) {
            g.push((i & 1) as f32);
        }
        assert_eq!(g.len(), 48_000 * 120, "no cut-off at the old cap");
    }
}
