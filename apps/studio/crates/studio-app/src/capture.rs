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

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

/// One thing to try when opening a device: a rate, a channel count and a
/// buffer choice. Kept as data so the ORDER of attempts is pure and
/// testable — the part that actually decides whether a Fairphone's ALSA
/// stack cooperates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Attempt {
    pub rate: u32,
    pub channels: u16,
    /// `None` lets the backend choose; `Some(n)` pins the period. ALSA
    /// plug devices frequently reject one and accept the other, and which
    /// one varies by machine — so both are tried rather than guessed.
    pub buffer: Option<u32>,
}

/// What a device says it supports: one range from cpal's enumeration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Range {
    pub min_rate: u32,
    pub max_rate: u32,
    pub channels: u16,
}

/// Build the attempt list for a device, best first.
///
/// This exists because `default_input_config()` lies on a real Linux
/// audio stack: it reports a configuration that `snd_pcm_hw_params` then
/// refuses with ENOENT ("No such file or directory"), which is ALSA's way
/// of saying "not that combination, and I will not tell you which". The
/// cure is to negotiate rather than assume — walk what the device
/// enumerates, prefer the rates this app actually wants, and try both
/// buffer strategies before giving up.
///
/// Order: 48 kHz first (the project rate — no resample), then 44.1, then
/// whatever the range's own bounds are; mono before stereo, since a vocal
/// take is mono and a mono open often succeeds where stereo does not.
pub fn attempts(ranges: &[Range]) -> Vec<Attempt> {
    let mut out: Vec<Attempt> = Vec::new();
    let push = |a: Attempt, out: &mut Vec<Attempt>| {
        if !out.contains(&a) { out.push(a); }
    };
    // Mono first across every range, then stereo — a channel count the
    // device cannot do is skipped rather than attempted.
    for want_ch in [1u16, 2] {
        for r in ranges {
            if r.channels < want_ch { continue; }
            let ch = want_ch;
            let mut rates = vec![48_000u32, 44_100];
            rates.push(r.min_rate);
            rates.push(r.max_rate);
            for rate in rates {
                if rate < r.min_rate || rate > r.max_rate { continue; }
                // Backend's choice first: it is right more often, and a
                // pinned period is the fallback for the stacks where it
                // is not.
                push(Attempt { rate, channels: ch, buffer: None }, &mut out);
                push(Attempt { rate, channels: ch, buffer: Some(1024) }, &mut out);
            }
        }
    }
    out
}

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
    /// Whether the callback appends to `buf` — a take in progress. The
    /// stream can be open without one (the call keeps it open), so this
    /// flag, not the stream's existence, is what "recording" means.
    taking: AtomicBool,
    /// The live tap: the same mono fold, into a second buffer the owner
    /// drains every frame instead of at the end. On while a call is on.
    tap: Mutex<Vec<f32>>,
    tap_on: AtomicBool,
    tap_dropped: AtomicUsize,
}

/// A microphone, recording into memory until stopped.
pub trait Capture {
    fn start(&mut self) -> Result<(), String>;
    /// Stop and take the audio. `None` if nothing was ever started.
    fn stop(&mut self) -> Option<Take>;
    /// Take the audio so far and keep recording — the loop-record cut.
    /// `None` when not recording or nothing has arrived since the last
    /// cut; the dropped-block count is since the take started, not
    /// since the cut, so a hole anywhere in the session is still said.
    fn split_take(&mut self) -> Option<Take>;
    fn recording(&self) -> bool;
    /// Seconds captured so far — the UI's counter.
    fn elapsed(&self) -> f32;
    fn device_name(&self) -> &str;
    /// The rate actually negotiated — reported because "recording" at a
    /// rate nobody chose is how a take ends up the wrong length.
    fn rate(&self) -> u32;
    /// Open the microphone for the LIVE tap: samples flow to
    /// [`Capture::tap_drain`] continuously, independent of any take. A
    /// take started while the tap is on shares the stream; stopping
    /// either leaves the other running. The call is the tap's owner.
    fn tap_start(&mut self) -> Result<(), String>;
    fn tap_stop(&mut self);
    fn tapping(&self) -> bool;
    /// Everything the tap captured since the last drain, mono at
    /// [`Capture::rate`]. Empty when the tap is off or nothing arrived.
    fn tap_drain(&mut self) -> Vec<f32>;
    /// Blocks the tap lost because the owner held its lock — the call's
    /// dropout counter, said out loud like the take's.
    fn tap_dropped(&self) -> usize;
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
    fn split_take(&mut self) -> Option<Take> { None }
    fn recording(&self) -> bool { self.on }
    fn elapsed(&self) -> f32 { 0.0 }
    fn device_name(&self) -> &str { "none" }
    fn rate(&self) -> u32 { 0 }
    // No microphone: the tap refuses like the take does, so a call on
    // this machine is listen-only and says so, rather than sending a
    // silence nobody chose.
    fn tap_start(&mut self) -> Result<(), String> {
        Err("no input device — the call is listen-only here".into())
    }
    fn tap_stop(&mut self) {}
    fn tapping(&self) -> bool { false }
    fn tap_drain(&mut self) -> Vec<f32> { Vec::new() }
    fn tap_dropped(&self) -> usize { 0 }
}

#[cfg(feature = "audio")]
pub struct CpalCapture {
    shared: Arc<Shared>,
    /// The rate and channel count actually NEGOTIATED, set when a stream
    /// opens — not what a default config claimed before anything was
    /// tried.
    rate: u32,
    channels: usize,
    name: String,
    stream: Option<cpal::Stream>,
    device: cpal::Device,
    /// Every device on the host, so a failing default falls through to a
    /// working USB interface instead of ending the attempt.
    others: Vec<cpal::Device>,
}

#[cfg(feature = "audio")]
impl CpalCapture {
    pub fn open() -> Option<CpalCapture> {
        use cpal::traits::{DeviceTrait, HostTrait};
        let host = cpal::default_host();
        let device = host.default_input_device()?;
        let name = device.name().unwrap_or_else(|_| "input".into());
        // Enumerated now, used only if the default device refuses every
        // attempt: on a phone the default is often a plug device that
        // cannot actually be opened while a real one sits behind it.
        let others: Vec<cpal::Device> = host.input_devices()
            .map(|d| d.collect()).unwrap_or_default();
        Some(CpalCapture {
            shared: Arc::new(Shared::default()),
            // Placeholders until a stream is actually negotiated — never
            // reported as the take's rate, because nothing has opened yet.
            rate: 48_000, channels: 1,
            name, stream: None, device, others,
        })
    }

    fn ranges_of(device: &cpal::Device) -> Vec<Range> {
        use cpal::traits::DeviceTrait;
        device.supported_input_configs().map(|it| it.map(|c| Range {
            min_rate: c.min_sample_rate().0,
            max_rate: c.max_sample_rate().0,
            channels: c.channels(),
        }).collect()).unwrap_or_default()
    }
}

#[cfg(feature = "audio")]
impl CpalCapture {
    /// Open the device if it is not open. The negotiation lives here so
    /// the take and the tap share one stream: whichever asks first opens
    /// it, and it closes when the last of them stops.
    fn ensure_stream(&mut self) -> Result<(), String> {
        use cpal::traits::{DeviceTrait, StreamTrait};
        if self.stream.is_some() { return Ok(()); }

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
                    // holds. A vocal take is a mono take, and so is a voice
                    // on a call. Each destination is gated by its own flag
                    // and guarded by its own try_lock, so the take and the
                    // tap cannot block each other any more than the UI.
                    let fold = |b: &mut Vec<f32>| {
                        for frame in data.chunks(chans.max(1)) {
                            let sum: f32 = frame.iter().copied().map(to_f32).sum();
                            b.push(sum / frame.len().max(1) as f32);
                        }
                    };
                    if shared.taking.load(Ordering::Relaxed) {
                        match shared.buf.try_lock() {
                            Ok(mut b) => fold(&mut b),
                            // Never block the audio thread — count and move on.
                            Err(_) => { shared.dropped.fetch_add(1, Ordering::Relaxed); }
                        }
                    }
                    if shared.tap_on.load(Ordering::Relaxed) {
                        match shared.tap.try_lock() {
                            Ok(mut b) => fold(&mut b),
                            Err(_) => { shared.tap_dropped.fetch_add(1, Ordering::Relaxed); }
                        }
                    }
                },
                |e| eprintln!("capture stream error: {e}"),
                None,
            )
        }

        // Negotiate. Each (device, attempt, format) pair is tried until
        // one opens AND plays — `build_input_stream` succeeding is not
        // enough on ALSA, where hw_params can still refuse at play().
        // Failures are collected so a total failure can say what was
        // tried instead of repeating one backend string.
        let mut devices: Vec<cpal::Device> = vec![self.device.clone()];
        for d in &self.others {
            let same = d.name().ok() == Some(self.name.clone());
            if !same { devices.push(d.clone()); }
        }
        let mut tried = 0usize;
        let mut last_err = String::new();
        for device in devices {
            let dname = device.name().unwrap_or_else(|_| "input".into());
            let ranges = Self::ranges_of(&device);
            // A device that enumerates nothing still gets one honest try
            // at the project rate rather than being skipped.
            let plan = if ranges.is_empty() {
                vec![Attempt { rate: 48_000, channels: 1, buffer: None }]
            } else { attempts(&ranges) };
            let formats = device.supported_input_configs()
                .map(|it| it.map(|c| c.sample_format()).collect::<Vec<_>>())
                .unwrap_or_else(|_| vec![cpal::SampleFormat::F32]);
            for a in plan {
                for fmt in formats.iter().copied() {
                    tried += 1;
                    let cfg = cpal::StreamConfig {
                        channels: a.channels,
                        sample_rate: cpal::SampleRate(a.rate),
                        buffer_size: match a.buffer {
                            Some(n) => cpal::BufferSize::Fixed(n),
                            None => cpal::BufferSize::Default,
                        },
                    };
                    let chans = a.channels as usize;
                    let built = match fmt {
                        cpal::SampleFormat::F32 =>
                            build::<f32>(&device, &cfg, self.shared.clone(), chans, |s| s),
                        cpal::SampleFormat::I16 =>
                            build::<i16>(&device, &cfg, self.shared.clone(), chans,
                                         |s| s as f32 / 32768.0),
                        cpal::SampleFormat::U16 =>
                            build::<u16>(&device, &cfg, self.shared.clone(), chans,
                                         |s| (s as f32 - 32768.0) / 32768.0),
                        other => { last_err = format!("format {other:?} unsupported"); continue; }
                    };
                    // build succeeding is not enough on ALSA: hw_params
                    // can still refuse at play(), which is exactly the
                    // ENOENT this whole negotiation exists for. Both
                    // stages must pass before the attempt counts.
                    let opened = match built {
                        Ok(s) => match s.play() {
                            Ok(()) => Ok(s),
                            Err(e) => { last_err = e.to_string(); continue; }
                        },
                        Err(e) => Err(e),
                    };
                    match opened {
                        Ok(stream) => {
                            self.rate = a.rate;
                            self.channels = chans;
                            self.name = dname.clone();
                            self.stream = Some(stream);
                            return Ok(());
                        }
                        Err(e) => last_err = e.to_string(),
                    }
                }
            }
        }
        Err(format!(
            "no microphone configuration worked ({tried} tried across every input \
             device). Last error: {last_err}. On Linux this is usually ALSA \
             refusing the default config — check `arecord -l`, and if the device \
             is busy, whatever else is holding it."))
    }

    /// Close the device if nobody needs it — dropping the stream stops it.
    fn release_if_idle(&mut self) {
        let taking = self.shared.taking.load(Ordering::Relaxed);
        let tapping = self.shared.tap_on.load(Ordering::Relaxed);
        if !taking && !tapping { drop(self.stream.take()); }
    }
}

#[cfg(feature = "audio")]
impl Capture for CpalCapture {
    fn start(&mut self) -> Result<(), String> {
        if self.shared.taking.load(Ordering::Relaxed) { return Ok(()); }
        self.shared.buf.lock().map_err(|_| "capture buffer poisoned")?.clear();
        self.shared.dropped.store(0, Ordering::Relaxed);
        self.ensure_stream()?;
        self.shared.taking.store(true, Ordering::Relaxed);
        Ok(())
    }

    fn stop(&mut self) -> Option<Take> {
        if !self.shared.taking.load(Ordering::Relaxed) { return None; }
        self.shared.taking.store(false, Ordering::Relaxed);
        // The callback may be mid-fold; the lock below waits for it, and
        // the flag it just read means it will not start another.
        self.release_if_idle();
        let samples = std::mem::take(&mut *self.shared.buf.lock().ok()?);
        if samples.is_empty() { return None; }
        Some(Take {
            audio: studio_audio::SourceAudio { sample_rate: self.rate,
                                               channels: vec![samples] },
            dropped_blocks: self.shared.dropped.load(Ordering::Relaxed),
        })
    }

    fn split_take(&mut self) -> Option<Take> {
        if !self.shared.taking.load(Ordering::Relaxed) { return None; }
        // A real lock, briefly: the callback's try_lock loses this one
        // block at worst, and counts it.
        let samples = std::mem::take(&mut *self.shared.buf.lock().ok()?);
        if samples.is_empty() { return None; }
        Some(Take {
            audio: studio_audio::SourceAudio { sample_rate: self.rate, channels: vec![samples] },
            dropped_blocks: self.shared.dropped.load(Ordering::Relaxed),
        })
    }

    fn recording(&self) -> bool { self.shared.taking.load(Ordering::Relaxed) }
    fn elapsed(&self) -> f32 {
        self.shared.buf.lock().map(|b| b.len() as f32 / self.rate as f32).unwrap_or(0.0)
    }
    fn device_name(&self) -> &str { &self.name }
    fn rate(&self) -> u32 { self.rate }

    fn tap_start(&mut self) -> Result<(), String> {
        if self.shared.tap_on.load(Ordering::Relaxed) { return Ok(()); }
        self.shared.tap.lock().map_err(|_| "tap buffer poisoned")?.clear();
        self.shared.tap_dropped.store(0, Ordering::Relaxed);
        self.ensure_stream()?;
        self.shared.tap_on.store(true, Ordering::Relaxed);
        Ok(())
    }
    fn tap_stop(&mut self) {
        self.shared.tap_on.store(false, Ordering::Relaxed);
        self.release_if_idle();
        if let Ok(mut t) = self.shared.tap.lock() { t.clear(); }
    }
    fn tapping(&self) -> bool { self.shared.tap_on.load(Ordering::Relaxed) }
    fn tap_drain(&mut self) -> Vec<f32> {
        // A real lock, not try_lock: this side is the UI thread, and it
        // is the callback's job (try_lock, count) to never wait on us.
        match self.shared.tap.lock() {
            Ok(mut t) => std::mem::take(&mut *t),
            Err(_) => Vec::new(),
        }
    }
    fn tap_dropped(&self) -> usize { self.shared.tap_dropped.load(Ordering::Relaxed) }
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
    fn the_attempt_order_negotiates_instead_of_trusting_the_default() {
        // The bug this exists for: cpal's default_input_config() reported
        // a configuration ALSA then refused with ENOENT from
        // snd_pcm_hw_params. Rather than assume, walk what the device
        // actually enumerates.
        let ranges = vec![Range { min_rate: 44_100, max_rate: 48_000, channels: 2 }];
        let a = attempts(&ranges);
        // 48k first (the project rate, no resample), mono before stereo (a
        // vocal take is mono, and mono opens where stereo sometimes does
        // not), backend buffer before a pinned one.
        assert_eq!(a[0], Attempt { rate: 48_000, channels: 1, buffer: None });
        assert_eq!(a[1], Attempt { rate: 48_000, channels: 1, buffer: Some(1024) });
        assert_eq!(a[2].rate, 44_100);
        assert!(a.iter().any(|x| x.channels == 2), "stereo is still tried, later");
        // A rate outside the device's range is never proposed — proposing
        // one is how you get ENOENT in the first place.
        assert!(a.iter().all(|x| x.rate >= 44_100 && x.rate <= 48_000));
        // A mono-only device is never asked for stereo.
        let mono = attempts(&[Range { min_rate: 16_000, max_rate: 16_000, channels: 1 }]);
        assert!(mono.iter().all(|x| x.channels == 1));
        assert_eq!(mono[0].rate, 16_000, "a device that only does 16k is asked for 16k");
        // No duplicates: every attempt costs a device round trip.
        let mut seen = std::collections::BTreeSet::new();
        for x in &a { assert!(seen.insert((x.rate, x.channels, x.buffer)), "duplicate {x:?}"); }
        assert!(attempts(&[]).is_empty(), "nothing enumerated, nothing invented");
    }

    #[test]
    fn the_null_microphone_says_so_instead_of_pretending() {
        let mut c = NullCapture { on: false };
        assert!(c.start().is_err(), "a missing device is reported, not faked");
        assert!(c.stop().is_none());
    }
}
