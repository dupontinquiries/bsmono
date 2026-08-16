//! audition — hearing the rack while you edit it.
//!
//! Beatmaking is a loop you listen to while poking it; a rack you can only
//! hear by bouncing is a punch-card workflow. This module plays ONE mono
//! buffer on repeat — the current pattern, re-rendered by the shell whenever
//! an edit lands — and reports where in the loop it is, so the grid's
//! playhead column marches with the audio.
//!
//! ## Why a loop buffer and not the engine
//!
//! `studio-engine`'s `DeviceEngine` renders the *timeline* through the real
//! mixdown path; its own docs name pattern playback as not wired into that
//! render path yet. Audition does not need it: `render_pattern` is a pure
//! function and a bar of trap at 48k is under a megabyte, so re-rendering the
//! whole loop on every toggle and swapping the buffer is simpler, cheaper to
//! reason about, and — because the swap is a single `ArcSwap::store` — safe
//! against the callback without a lock. The engine integration (patterns
//! audible inside a full arrangement mixdown) stays real future work; this is
//! the practice-room monitor, not the console.
//!
//! ## The seam
//!
//! [`LoopSink`] is object-safe and tiny so the shell can hold `Box<dyn
//! LoopSink>` and never know whether a sound card exists. [`NullSink`] keeps
//! the playhead marching off the wall clock when there is no device (or the
//! `audio` feature is off), because a silently frozen grid reads as a hang;
//! the status line is what tells the user which sink they got — an audio path
//! that cannot be seen cannot be debugged.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;

/// Where playback should be able to go next.
pub trait LoopSink {
    /// Replace the loop. Takes effect immediately; the position is carried
    /// over modulo the new length so a mid-bar edit does not restart the bar
    /// — the edit-listen loop is the whole point, and a restart on every
    /// toggle would make swing inaudible while you dial it in.
    fn set_loop(&mut self, samples: Arc<Vec<f32>>);
    fn set_playing(&mut self, on: bool);
    fn playing(&self) -> bool;
    /// Current position in samples into the loop, if playing.
    fn position(&self) -> Option<usize>;
    /// The sample rate the loop must be rendered at.
    fn rate(&self) -> u32;
    /// A short human name for the status line ("alsa @ 48000" / "silent").
    fn describe(&self) -> String;
}

/// The device-less sink: silent, but the clock still runs.
pub struct NullSink {
    rate: u32,
    len: usize,
    playing: bool,
    /// Position frozen at the last pause, advanced from `epoch` while playing.
    base: usize,
    epoch: std::time::Instant,
}

impl NullSink {
    pub fn new(rate: u32) -> NullSink {
        NullSink { rate, len: 0, playing: false, base: 0, epoch: std::time::Instant::now() }
    }
    fn elapsed_samples(&self) -> usize {
        (self.epoch.elapsed().as_secs_f64() * self.rate as f64) as usize
    }
}

impl LoopSink for NullSink {
    fn set_loop(&mut self, samples: Arc<Vec<f32>>) {
        self.len = samples.len();
        if self.len > 0 { self.base %= self.len; }
    }
    fn set_playing(&mut self, on: bool) {
        if on == self.playing { return; }
        if on {
            self.epoch = std::time::Instant::now();
        } else if self.len > 0 {
            self.base = (self.base + self.elapsed_samples()) % self.len;
        }
        self.playing = on;
    }
    fn playing(&self) -> bool { self.playing }
    fn position(&self) -> Option<usize> {
        if !self.playing || self.len == 0 { return None; }
        Some((self.base + self.elapsed_samples()) % self.len)
    }
    fn rate(&self) -> u32 { self.rate }
    fn describe(&self) -> String { "silent (no audio device)".into() }
}

/// The state the audio callback shares with the UI thread. Lock-free on
/// purpose: the callback loads the buffer arc once per buffer and touches two
/// atomics, so a UI stall can never become an audible glitch — the same
/// argument `studio-engine`'s snapshot-swap makes, at loop-pedal scale.
#[cfg(feature = "audio")]
struct Shared {
    buf: arc_swap::ArcSwap<Vec<f32>>,
    pos: AtomicUsize,
    playing: AtomicBool,
}

#[cfg(feature = "audio")]
pub struct CpalSink {
    shared: Arc<Shared>,
    rate: u32,
    name: String,
    // Held for its Drop: the stream stops when the sink does. Dead code to
    // every reader except the OS.
    _stream: cpal::Stream,
}

#[cfg(feature = "audio")]
impl CpalSink {
    /// `None` if there is no output device or its config is unusable — the
    /// caller falls back to [`NullSink`] rather than failing to start; an
    /// editor that cannot open without a sound card is a worse editor.
    pub fn open() -> Option<CpalSink> {
        use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
        let host = cpal::default_host();
        let device = host.default_output_device()?;
        let config = device.default_output_config().ok()?;
        let rate = config.sample_rate().0;
        let channels = config.channels() as usize;
        let shared = Arc::new(Shared {
            buf: arc_swap::ArcSwap::from_pointee(Vec::new()),
            pos: AtomicUsize::new(0),
            playing: AtomicBool::new(false),
        });

        fn build<T: cpal::SizedSample + cpal::FromSample<f32>>(
            device: &cpal::Device,
            config: &cpal::StreamConfig,
            channels: usize,
            shared: Arc<Shared>,
        ) -> Result<cpal::Stream, cpal::BuildStreamError> {
            use cpal::traits::DeviceTrait;
            device.build_output_stream(
                config,
                move |data: &mut [T], _| {
                    let silence = T::from_sample(0.0f32);
                    let buf = shared.buf.load();
                    if !shared.playing.load(Ordering::Relaxed) || buf.is_empty() {
                        data.fill(silence);
                        return;
                    }
                    let mut pos = shared.pos.load(Ordering::Relaxed) % buf.len();
                    for frame in data.chunks_mut(channels) {
                        let s = T::from_sample(buf[pos]);
                        for out in frame.iter_mut() { *out = s; }
                        pos = (pos + 1) % buf.len();
                    }
                    shared.pos.store(pos, Ordering::Relaxed);
                },
                |e| eprintln!("audition stream error: {e}"),
                None,
            )
        }

        let sc: cpal::StreamConfig = config.clone().into();
        let stream = match config.sample_format() {
            cpal::SampleFormat::F32 => build::<f32>(&device, &sc, channels, Arc::clone(&shared)),
            cpal::SampleFormat::I16 => build::<i16>(&device, &sc, channels, Arc::clone(&shared)),
            cpal::SampleFormat::U16 => build::<u16>(&device, &sc, channels, Arc::clone(&shared)),
            _ => return None,
        }
        .ok()?;
        stream.play().ok()?;
        let name = device.name().unwrap_or_else(|_| "output".into());
        Some(CpalSink { shared, rate, name: format!("{name} @ {rate}"), _stream: stream })
    }
}

#[cfg(feature = "audio")]
impl LoopSink for CpalSink {
    fn set_loop(&mut self, samples: Arc<Vec<f32>>) {
        // Position intentionally NOT reset — see the trait docs. The modulo
        // in the callback makes a shorter new loop safe.
        self.shared.buf.store(samples);
    }
    fn set_playing(&mut self, on: bool) {
        if on && !self.playing() {
            self.shared.pos.store(0, Ordering::Relaxed);
        }
        self.shared.playing.store(on, Ordering::Relaxed);
    }
    fn playing(&self) -> bool { self.shared.playing.load(Ordering::Relaxed) }
    fn position(&self) -> Option<usize> {
        self.playing().then(|| self.shared.pos.load(Ordering::Relaxed))
    }
    fn rate(&self) -> u32 { self.rate }
    fn describe(&self) -> String { self.name.clone() }
}

/// The best sink this build and this machine can offer.
pub fn open_sink() -> Box<dyn LoopSink> {
    #[cfg(feature = "audio")]
    if let Some(s) = CpalSink::open() {
        return Box::new(s);
    }
    Box::new(NullSink::new(48_000))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_deviceless_sink_still_keeps_time() {
        // The grid playhead must march even in silence, or a missing sound
        // card reads as a hung editor.
        let mut s = NullSink::new(48_000);
        s.set_loop(Arc::new(vec![0.0; 48_000]));
        assert_eq!(s.position(), None, "stopped means no playhead");
        s.set_playing(true);
        std::thread::sleep(std::time::Duration::from_millis(30));
        let p = s.position().expect("playing means a position");
        assert!(p > 480, "~30ms elapsed should be > 10ms of samples, got {p}");
        assert!(p < 48_000, "and wrapped inside the loop");
    }

    #[test]
    fn pausing_freezes_the_position_rather_than_resetting_it() {
        let mut s = NullSink::new(48_000);
        s.set_loop(Arc::new(vec![0.0; 48_000]));
        s.set_playing(true);
        std::thread::sleep(std::time::Duration::from_millis(20));
        s.set_playing(false);
        let frozen = s.base;
        assert!(frozen > 0);
        std::thread::sleep(std::time::Duration::from_millis(20));
        assert_eq!(s.base, frozen, "time does not pass while paused");
    }

    #[test]
    fn a_shorter_replacement_loop_cannot_strand_the_position() {
        // The edit-listen loop swaps buffers constantly; a swap to a shorter
        // pattern must wrap the carried-over position, not index past it.
        let mut s = NullSink::new(48_000);
        s.set_loop(Arc::new(vec![0.0; 48_000]));
        s.set_playing(true);
        std::thread::sleep(std::time::Duration::from_millis(15));
        s.set_playing(false);
        s.set_loop(Arc::new(vec![0.0; 100]));
        assert!(s.base < 100);
    }
}
