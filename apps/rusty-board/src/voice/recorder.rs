//! Audio capture via `cpal`, written into a `HeapRb` ring buffer for the
//! transcriber to consume.

use anyhow::{anyhow, Context, Result};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use parking_lot::Mutex;
use ringbuf::{traits::*, HeapRb};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use tracing::{debug, info, warn};

pub type AudioRing = Arc<Mutex<HeapRb<f32>>>;

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
        // Allocate a ring sized for `buffer_seconds` of 16-bit-equivalent f32
        // samples at 16 kHz.
        let capacity = 16_000 * buffer_seconds.max(1);
        let ring = HeapRb::<f32>::new(capacity);
        Self {
            stream: None,
            ring: Arc::new(Mutex::new(ring)),
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
        let len = ring.occupied_len();
        let mut out = Vec::with_capacity(len);
        for _ in 0..len {
            if let Some(s) = ring.try_pop() {
                out.push(s);
            } else {
                break;
            }
        }
        out
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
    r.iter().copied().collect()
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

    // Push to ring (mono mixdown).
    {
        let mut r = ring.lock();
        if channels == 1 {
            for &s in data {
                let _ = r.try_push(s);
            }
        } else {
            for frame in data.chunks_exact(channels) {
                let mean: f32 = frame.iter().sum::<f32>() / channels as f32;
                let _ = r.try_push(mean);
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
}
