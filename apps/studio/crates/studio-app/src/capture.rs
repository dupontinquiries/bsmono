//! capture — vocal takes from the microphone.
//!
//! The editor could import audio, bounce audio and synthesize audio, and
//! had no way to RECORD any: a vocal take meant leaving for another tool.
//! This is the input half of `audition`'s output half, built the same way
//! — a real device behind the `audio` feature, a null implementation
//! without it, and the same refusal to let a missing sound card stop the
//! app from starting.
//!
//! ## The audio thread's rule
//!
//! The capture callback runs on the OS's audio thread and must never
//! block. It takes the buffer lock with `try_lock` and, when the UI thread
//! happens to hold it, DROPS that block and counts it rather than waiting
//! — a dropped block is a click in one take, a blocked callback is a
//! glitch in everything. The count is surfaced, not swallowed: a take that
//! lost samples says so before you build a song on it. (A lock-free ring
//! would drop nothing; it is the right next step and is named here rather
//! than pretended.)

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

/// What a finished take carries back.
pub struct Take {
    pub audio: studio_audio::SourceAudio,
    /// Blocks the callback had to drop because the UI held the lock.
    /// Zero on a healthy take; anything else is said out loud.
    pub dropped_blocks: usize,
}

#[derive(Default)]
struct Shared {
    buf: Mutex<Vec<f32>>,
    dropped: AtomicUsize,
}

/// A microphone, recording into memory until stopped.
pub trait Capture {
    fn start(&mut self) -> Result<(), String>;
    /// Stop and take the audio. `None` if nothing was ever started.
    fn stop(&mut self) -> Option<Take>;
    fn recording(&self) -> bool;
    /// Seconds captured so far — the UI's counter.
    fn elapsed(&self) -> f32;
    fn device_name(&self) -> &str;
}

/// No microphone: every call succeeds shallowly and records silence, so
/// the record button exists and explains itself on a machine without an
/// input device rather than vanishing.
pub struct NullCapture {
    on: bool,
}
impl Capture for NullCapture {
    fn start(&mut self) -> Result<(), String> {
        self.on = true;
        Err("no input device — nothing will be recorded".into())
    }
    fn stop(&mut self) -> Option<Take> { self.on = false; None }
    fn recording(&self) -> bool { self.on }
    fn elapsed(&self) -> f32 { 0.0 }
    fn device_name(&self) -> &str { "none" }
}

#[cfg(feature = "audio")]
pub struct CpalCapture {
    shared: Arc<Shared>,
    rate: u32,
    channels: usize,
    name: String,
    stream: Option<cpal::Stream>,
    device: cpal::Device,
    config: cpal::SupportedStreamConfig,
}

#[cfg(feature = "audio")]
impl CpalCapture {
    pub fn open() -> Option<CpalCapture> {
        use cpal::traits::{DeviceTrait, HostTrait};
        let host = cpal::default_host();
        let device = host.default_input_device()?;
        let config = device.default_input_config().ok()?;
        let name = device.name().unwrap_or_else(|_| "input".into());
        Some(CpalCapture {
            shared: Arc::new(Shared::default()),
            rate: config.sample_rate().0,
            channels: config.channels() as usize,
            name, stream: None, device, config,
        })
    }
}

#[cfg(feature = "audio")]
impl Capture for CpalCapture {
    fn start(&mut self) -> Result<(), String> {
        use cpal::traits::{DeviceTrait, StreamTrait};
        if self.stream.is_some() { return Ok(()); }
        self.shared.buf.lock().map_err(|_| "capture buffer poisoned")?.clear();
        self.shared.dropped.store(0, Ordering::Relaxed);

        fn build<T: cpal::SizedSample>(
            device: &cpal::Device,
            config: &cpal::StreamConfig,
            shared: Arc<Shared>,
            chans: usize,
            to_f32: fn(T) -> f32,
        ) -> Result<cpal::Stream, cpal::BuildStreamError>
        where T: Send + 'static {
            device.build_input_stream(
                config,
                move |data: &[T], _| {
                    // Mono-fold on the audio thread: it is one multiply-add
                    // per frame and it halves (or quarters) what the lock
                    // holds. A vocal take is a mono take.
                    match shared.buf.try_lock() {
                        Ok(mut b) => {
                            for frame in data.chunks(chans.max(1)) {
                                let sum: f32 = frame.iter().copied().map(to_f32).sum();
                                b.push(sum / frame.len().max(1) as f32);
                            }
                        }
                        // Never block the audio thread — count and move on.
                        Err(_) => { shared.dropped.fetch_add(1, Ordering::Relaxed); }
                    }
                },
                |e| eprintln!("capture stream error: {e}"),
                None,
            )
        }

        let cfg: cpal::StreamConfig = self.config.clone().into();
        let stream = match self.config.sample_format() {
            cpal::SampleFormat::F32 =>
                build::<f32>(&self.device, &cfg, self.shared.clone(), self.channels, |s| s),
            cpal::SampleFormat::I16 =>
                build::<i16>(&self.device, &cfg, self.shared.clone(), self.channels,
                             |s| s as f32 / 32768.0),
            cpal::SampleFormat::U16 =>
                build::<u16>(&self.device, &cfg, self.shared.clone(), self.channels,
                             |s| (s as f32 - 32768.0) / 32768.0),
            other => return Err(format!("unsupported input format {other:?}")),
        }.map_err(|e| format!("could not open the microphone: {e}"))?;
        stream.play().map_err(|e| format!("could not start capture: {e}"))?;
        self.stream = Some(stream);
        Ok(())
    }

    fn stop(&mut self) -> Option<Take> {
        drop(self.stream.take()?); // dropping the stream stops the device
        let samples = std::mem::take(&mut *self.shared.buf.lock().ok()?);
        if samples.is_empty() { return None; }
        Some(Take {
            audio: studio_audio::SourceAudio { sample_rate: self.rate,
                                               channels: vec![samples] },
            dropped_blocks: self.shared.dropped.load(Ordering::Relaxed),
        })
    }

    fn recording(&self) -> bool { self.stream.is_some() }
    fn elapsed(&self) -> f32 {
        self.shared.buf.lock().map(|b| b.len() as f32 / self.rate as f32).unwrap_or(0.0)
    }
    fn device_name(&self) -> &str { &self.name }
}

/// The microphone, or an honest stand-in for one.
pub fn open_capture() -> Box<dyn Capture> {
    #[cfg(feature = "audio")]
    if let Some(c) = CpalCapture::open() { return Box::new(c); }
    Box::new(NullCapture { on: false })
}

/// Trim leading and trailing near-silence from a take, keeping `keep_ms`
/// of room on each side. A vocal take always starts with the walk back to
/// the mic and ends with the reach for the mouse; trimming that at capture
/// time is why the clip lands where the singer actually sang.
pub fn trim_silence(a: &studio_audio::SourceAudio, threshold: f32, keep_ms: f32)
                    -> studio_audio::SourceAudio {
    let ch = match a.channels.first() { Some(c) => c, None => return a.clone() };
    let keep = ((keep_ms / 1000.0) * a.sample_rate as f32) as usize;
    let first = ch.iter().position(|s| s.abs() > threshold);
    let Some(first) = first else {
        // Entirely below the threshold: return it UNCHANGED rather than
        // returning nothing. A take that records only breath is still the
        // take that was performed, and silently deleting it would be the
        // worst possible answer to a mic-gain mistake.
        return a.clone();
    };
    let last = ch.iter().rposition(|s| s.abs() > threshold).unwrap_or(ch.len() - 1);
    let from = first.saturating_sub(keep);
    let to = (last + keep).min(ch.len());
    studio_audio::SourceAudio {
        sample_rate: a.sample_rate,
        channels: a.channels.iter().map(|c| c[from.min(c.len())..to.min(c.len())].to_vec())
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn take(rate: u32) -> studio_audio::SourceAudio {
        // Quiet, then a voice, then quiet — a real take's shape.
        let mut v = vec![0.0f32; rate as usize / 2];
        for i in 0..rate as usize {
            v.push((i as f32 / rate as f32 * 180.0 * std::f32::consts::TAU).sin() * 0.4);
        }
        v.extend(vec![0.0f32; rate as usize / 2]);
        studio_audio::SourceAudio { sample_rate: rate, channels: vec![v] }
    }

    #[test]
    fn trimming_keeps_the_performance_and_a_little_room() {
        let rate = 48_000;
        let a = take(rate);
        let t = trim_silence(&a, 0.01, 50.0);
        let kept = t.frames() as f32 / rate as f32;
        // One second of voice plus 50 ms either side, not the two seconds
        // of walking to and from the microphone.
        assert!((kept - 1.1).abs() < 0.05, "kept {kept}s");
        assert!(t.channels[0][0].abs() < 0.01, "the room tone at the head survives");
    }

    #[test]
    fn a_silent_take_is_returned_whole_not_deleted() {
        // A mic-gain mistake must not silently destroy the performance —
        // returning nothing here would be the worst possible answer.
        let a = studio_audio::SourceAudio { sample_rate: 48_000,
            channels: vec![vec![0.0; 48_000]] };
        assert_eq!(trim_silence(&a, 0.01, 50.0).frames(), 48_000);
    }

    #[test]
    fn the_null_microphone_says_so_instead_of_pretending() {
        let mut c = NullCapture { on: false };
        assert!(c.start().is_err(), "a missing device is reported, not faked");
        assert!(c.stop().is_none());
    }
}
