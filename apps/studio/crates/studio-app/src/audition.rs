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
//! [`PlaybackSink`] is object-safe and tiny so the shell can hold `Box<dyn
//! PlaybackSink>` and never know whether a sound card exists. [`NullSink`] keeps
//! the playhead marching off the wall clock when there is no device (or the
//! `audio` feature is off), because a silently frozen grid reads as a hang;
//! the status line is what tells the user which sink they got — an audio path
//! that cannot be seen cannot be debugged.

#[cfg(feature = "audio")]
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;

/// One buffer, played looped (the rack's bar) or once (the arrangement).
///
/// Grown from the original loop-only trait when timeline playback arrived,
/// rather than adding a second sink type: the device, the position atomics
/// and the fallback logic are identical for both jobs — only the wrap rule
/// differs, and that is one branch in the callback, not an architecture.
pub trait PlaybackSink {
    /// Replace what's playing. `channels` is the buffer's interleave count
    /// (1 = the rack's mono loop, 2 = a mixdown). For a looped buffer the
    /// position carries over modulo the new length so a mid-bar edit does
    /// not restart the bar — the edit-listen loop is the whole point, and a
    /// restart on every toggle would make swing inaudible while you dial it
    /// in. A one-shot buffer plays to its end and reports stopped.
    fn set_buffer(&mut self, samples: Arc<Vec<f32>>, channels: u16, looped: bool);
    fn set_playing(&mut self, on: bool);
    fn playing(&self) -> bool;
    /// Jump to a frame (one-shot playback starting mid-arrangement).
    fn seek(&mut self, frame: usize);
    /// Current position in FRAMES into the buffer, if playing.
    fn position(&self) -> Option<usize>;
    /// The sample rate buffers must be rendered at.
    fn rate(&self) -> u32;
    /// A short human name for the status line ("alsa @ 48000" / "silent").
    fn describe(&self) -> String;
}

/// The device-less sink: silent, but the clock still runs.
pub struct NullSink {
    rate: u32,
    frames: usize,
    looped: bool,
    playing: bool,
    /// Position frozen at the last pause, advanced from `epoch` while playing.
    base: usize,
    epoch: std::time::Instant,
}

impl NullSink {
    pub fn new(rate: u32) -> NullSink {
        NullSink { rate, frames: 0, looped: true, playing: false, base: 0,
                   epoch: std::time::Instant::now() }
    }
    fn elapsed_frames(&self) -> usize {
        (self.epoch.elapsed().as_secs_f64() * self.rate as f64) as usize
    }
    fn raw_pos(&self) -> usize {
        let p = self.base + if self.playing { self.elapsed_frames() } else { 0 };
        if self.frames == 0 { 0 }
        else if self.looped { p % self.frames }
        else { p.min(self.frames) }
    }
}

impl PlaybackSink for NullSink {
    fn set_buffer(&mut self, samples: Arc<Vec<f32>>, channels: u16, looped: bool) {
        // Freeze the carried-over position first, or the epoch keeps
        // counting against the OLD length while the new one loads.
        self.base = self.raw_pos();
        self.epoch = std::time::Instant::now();
        self.frames = samples.len() / channels.max(1) as usize;
        self.looped = looped;
        if self.frames > 0 && looped { self.base %= self.frames; }
        self.base = self.base.min(self.frames);
    }
    fn set_playing(&mut self, on: bool) {
        if on == self.playing { return; }
        if !on { self.base = self.raw_pos(); }
        self.epoch = std::time::Instant::now();
        self.playing = on;
    }
    fn playing(&self) -> bool {
        // A finished one-shot reads as stopped, the same as the device sink:
        // the shell watches this to know the arrangement reached its end.
        self.playing && (self.looped || self.raw_pos() < self.frames)
    }
    fn seek(&mut self, frame: usize) {
        self.base = frame.min(self.frames);
        self.epoch = std::time::Instant::now();
    }
    fn position(&self) -> Option<usize> {
        if !self.playing() || self.frames == 0 { return None; }
        Some(self.raw_pos())
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
    buf: arc_swap::ArcSwap<Buffer>,
    pos: AtomicUsize,
    playing: AtomicBool,
}

/// What the callback plays: one ArcSwap'd value, so interleave count and
/// wrap rule can never be observed half-updated relative to the samples.
#[cfg(feature = "audio")]
struct Buffer {
    samples: Vec<f32>,
    channels: usize,
    looped: bool,
}
#[cfg(feature = "audio")]
impl Buffer {
    fn frames(&self) -> usize { self.samples.len() / self.channels.max(1) }
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
            buf: arc_swap::ArcSwap::from_pointee(Buffer {
                samples: Vec::new(), channels: 1, looped: true,
            }),
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
                    let frames = buf.frames();
                    if !shared.playing.load(Ordering::Relaxed) || frames == 0 {
                        data.fill(silence);
                        return;
                    }
                    let mut pos = shared.pos.load(Ordering::Relaxed);
                    if buf.looped { pos %= frames; }
                    for frame in data.chunks_mut(channels) {
                        if pos >= frames {
                            // One-shot ran out: silence, and report stopped
                            // so the shell can put the transport down too.
                            for out in frame.iter_mut() { *out = silence; }
                            shared.playing.store(false, Ordering::Relaxed);
                            continue;
                        }
                        for (c, out) in frame.iter_mut().enumerate() {
                            // Device channel c takes buffer channel min(c,
                            // last): mono fans out, stereo maps 1:1, extra
                            // device channels mirror the last buffer channel.
                            let bc = c.min(buf.channels - 1);
                            *out = T::from_sample(buf.samples[pos * buf.channels + bc]);
                        }
                        pos += 1;
                        if buf.looped && pos >= frames { pos = 0; }
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
impl PlaybackSink for CpalSink {
    fn set_buffer(&mut self, samples: Arc<Vec<f32>>, channels: u16, looped: bool) {
        // Position intentionally NOT reset for loops — see the trait docs;
        // the modulo in the callback makes a shorter new loop safe. Arc is
        // unwrapped-or-cloned into the swapped value: the clone only happens
        // if the caller kept another handle, and never on the audio thread.
        let samples = Arc::try_unwrap(samples).unwrap_or_else(|a| (*a).clone());
        self.shared.buf.store(Arc::new(Buffer {
            samples, channels: channels.max(1) as usize, looped,
        }));
    }
    fn set_playing(&mut self, on: bool) {
        if on && !self.shared.playing.load(Ordering::Relaxed) && self.shared.buf.load().looped {
            self.shared.pos.store(0, Ordering::Relaxed);
        }
        self.shared.playing.store(on, Ordering::Relaxed);
    }
    fn playing(&self) -> bool { self.shared.playing.load(Ordering::Relaxed) }
    fn seek(&mut self, frame: usize) {
        self.shared.pos.store(frame, Ordering::Relaxed);
    }
    fn position(&self) -> Option<usize> {
        self.playing().then(|| self.shared.pos.load(Ordering::Relaxed))
    }
    fn rate(&self) -> u32 { self.rate }
    fn describe(&self) -> String { self.name.clone() }
}

/// The best sink this build and this machine can offer.
pub fn open_sink() -> Box<dyn PlaybackSink> {
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
        s.set_buffer(Arc::new(vec![0.0; 48_000]), 1, true);
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
        s.set_buffer(Arc::new(vec![0.0; 48_000]), 1, true);
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
        s.set_buffer(Arc::new(vec![0.0; 48_000]), 1, true);
        s.set_playing(true);
        std::thread::sleep(std::time::Duration::from_millis(15));
        s.set_playing(false);
        s.set_buffer(Arc::new(vec![0.0; 100]), 1, true);
        assert!(s.base < 100);
    }
}

#[cfg(test)]
mod one_shot_tests {
    use super::*;

    #[test]
    fn a_one_shot_reaches_its_end_and_reports_stopped() {
        // The shell watches playing() to put the transport down when the
        // arrangement runs out — a one-shot that loops forever, or that
        // claims to be playing past its end, would leave a transport that
        // never stops on its own.
        let mut s = NullSink::new(48_000);
        s.set_buffer(Arc::new(vec![0.0; 2 * 480]), 2, false); // 480 frames = 10ms stereo
        s.set_playing(true);
        assert!(s.playing());
        std::thread::sleep(std::time::Duration::from_millis(40));
        assert!(!s.playing(), "10ms of one-shot cannot still be playing after 40ms");
        assert_eq!(s.position(), None);
    }

    #[test]
    fn seek_lands_playback_mid_buffer_in_frames_not_samples() {
        let mut s = NullSink::new(48_000);
        s.set_buffer(Arc::new(vec![0.0; 2 * 48_000]), 2, false); // 1s stereo
        s.seek(24_000);
        s.set_playing(true);
        let p = s.position().expect("playing");
        assert!((24_000..27_000).contains(&p),
                "seeked to frame 24000, immediately read {p} — frames, not interleaved samples");
    }
}
