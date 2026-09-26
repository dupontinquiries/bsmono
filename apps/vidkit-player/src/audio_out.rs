//! In-process audio output.
//!
//! The piped players cost a process per (re)start, a pipe copy, and a latency
//! the clock can only estimate -- and every seek killed the player and started
//! another, which on PipeWire means a new node, a renegotiation and an audible
//! gap. Here the engine's audio thread writes into a ring, the device callback
//! drains it, and the latency is MEASURED: what is in the ring, plus what the
//! device reports between its callback and playback.
//!
//! Layered so everything except the cpal glue is tested:
//!
//! - [`PcmRing`]: a bounded sample ring with a blocking push and a
//!   never-blocking pop that fills silence and counts it. Pure.
//! - [`RingSink`]: the engine's `AudioSink` over a ring and a [`Device`].
//! - [`Device`]: start/stop. [`CpalDevice`] (feature `alsa`) is the real one;
//!   tests drive the ring from a simulated device thread at real rate.
//!
//! `cpal::Stream` is `!Send` on every platform in cpal 0.15, and the engine
//! moves its sink to the audio thread. So the stream lives on a thread of its
//! own that owns it for its whole life and takes commands over a channel.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use crate::decode::AudioSink;

/// Samples (not frames) waiting for the device, bounded.
pub struct PcmRing {
    buf: VecDeque<i16>,
    cap: usize,
    channels: usize,
    /// Silence the device had to invent after audio had started, in frames.
    /// Priming silence before the first push is not an underrun.
    pub underrun_frames: u64,
    started: bool,
    closed: bool,
}

impl PcmRing {
    pub fn new(cap_frames: usize, channels: usize) -> Self {
        let channels = channels.max(1);
        Self { buf: VecDeque::with_capacity(cap_frames * channels), cap: cap_frames * channels, channels, underrun_frames: 0, started: false, closed: false }
    }

    pub fn space(&self) -> usize {
        self.cap - self.buf.len()
    }

    pub fn fill_frames(&self) -> usize {
        self.buf.len() / self.channels
    }

    /// Append what fits; returns how many samples were taken.
    pub fn push(&mut self, samples: &[i16]) -> usize {
        let n = samples.len().min(self.space());
        self.buf.extend(&samples[..n]);
        if n > 0 {
            self.started = true;
        }
        n
    }

    /// Fill `out` completely: samples if there are any, silence after.
    pub fn pop_into(&mut self, out: &mut [i16]) {
        let n = out.len().min(self.buf.len());
        for (o, s) in out[..n].iter_mut().zip(self.buf.drain(..n)) {
            *o = s;
        }
        for o in &mut out[n..] {
            *o = 0;
        }
        if self.started && n < out.len() {
            self.underrun_frames += ((out.len() - n) / self.channels) as u64;
        }
    }
}

/// The ring plus the condvar a blocked writer waits on.
pub struct Shared {
    pub ring: Mutex<PcmRing>,
    pub space: Condvar,
    /// Device latency in nanoseconds, from the callback's timestamps.
    pub device_latency_ns: AtomicU64,
}

impl Shared {
    pub fn new(cap_frames: usize, channels: usize) -> Arc<Self> {
        Arc::new(Self { ring: Mutex::new(PcmRing::new(cap_frames, channels)), space: Condvar::new(), device_latency_ns: AtomicU64::new(0) })
    }

    /// The device side: called from the audio callback. Holds the lock for
    /// one copy of a callback's worth of samples, then wakes the writer.
    pub fn render(&self, out: &mut [i16]) {
        match self.ring.lock() {
            Ok(mut r) => r.pop_into(out),
            Err(_) => out.fill(0),
        }
        self.space.notify_one();
    }
}

/// Start and stop the thing draining the ring.
pub trait Device: Send {
    fn set_paused(&mut self, paused: bool);
}

pub struct RingSink {
    shared: Arc<Shared>,
    device: Box<dyn Device>,
    rate: u32,
    channels: u16,
    name: String,
    samples: Vec<i16>,
}

impl RingSink {
    pub fn new(shared: Arc<Shared>, device: Box<dyn Device>, rate: u32, channels: u16, name: String) -> Self {
        Self { shared, device, rate, channels, name, samples: Vec::new() }
    }
}

/// How long a blocked write waits before giving up on a device that stopped
/// draining (unplugged, suspended). Long enough never to trigger in normal
/// playback, where the wait is at most a ring's worth.
const STALL: Duration = Duration::from_secs(2);

impl AudioSink for RingSink {
    fn play(&mut self, pcm: &[u8]) -> Result<(), String> {
        self.samples.clear();
        self.samples.extend(pcm.chunks_exact(2).map(|b| i16::from_le_bytes([b[0], b[1]])));
        let mut at = 0;
        let mut ring = self.shared.ring.lock().map_err(|_| "audio ring poisoned".to_string())?;
        while at < self.samples.len() {
            if ring.closed {
                return Err(format!("{}: device closed", self.name));
            }
            at += ring.push(&self.samples[at..]);
            if at < self.samples.len() {
                let (r, timeout) = self.shared.space.wait_timeout(ring, STALL).map_err(|_| "audio ring poisoned".to_string())?;
                ring = r;
                if timeout.timed_out() && ring.space() == 0 {
                    return Err(format!("{}: the device stopped taking audio", self.name));
                }
            }
        }
        Ok(())
    }

    fn name(&self) -> &str {
        &self.name
    }

    /// Measured, not estimated: the ring's fill plus the device's own delay.
    fn latency(&self) -> Duration {
        let fill = self.shared.ring.lock().map(|r| r.fill_frames()).unwrap_or(0);
        Duration::from_secs_f64(fill as f64 / self.rate.max(1) as f64)
            + Duration::from_nanos(self.shared.device_latency_ns.load(Ordering::Relaxed))
    }

    fn set_paused(&mut self, paused: bool) {
        self.device.set_paused(paused);
    }

    // take_underrun_frames stays 0 ON PURPOSE. The kit's AudioClock counts
    // frames WRITTEN TO THE DEVICE, silence included, and subtracts reported
    // underruns. Here the clock is advanced by MEDIA frames pushed, and the
    // latency is measured, so invented silence never entered the count;
    // reporting it would correct twice. The count is kept for diagnostics.
}

impl Drop for RingSink {
    fn drop(&mut self) {
        if let Ok(mut r) = self.shared.ring.lock() {
            r.closed = true;
        }
        self.shared.space.notify_all();
    }
}

/// The ring size: 150 ms, so one of the engine's 100 ms blocks always fits
/// with room to spare and the latency stays low.
pub fn ring_frames(rate: u32) -> usize {
    (rate as usize * 150) / 1000
}

// ---------------------------------------------------------------------------
// cpal
// ---------------------------------------------------------------------------

#[cfg(feature = "alsa")]
mod cpal_device {
    use super::*;
    use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
    use std::sync::mpsc;

    enum Cmd {
        Pause(bool),
    }

    /// Owns the stream thread. Dropping it drops the channel, which ends the
    /// thread, which drops the stream.
    pub struct CpalDevice {
        tx: mpsc::Sender<Cmd>,
    }

    impl Device for CpalDevice {
        fn set_paused(&mut self, paused: bool) {
            let _ = self.tx.send(Cmd::Pause(paused));
        }
    }

    fn fill<T: cpal::SizedSample + cpal::FromSample<i16>>(shared: &Shared, scratch: &mut Vec<i16>, out: &mut [T]) {
        scratch.resize(out.len(), 0);
        shared.render(scratch);
        for (o, s) in out.iter_mut().zip(scratch.iter()) {
            *o = T::from_sample(*s);
        }
    }

    /// Open the default output device at `rate`/`channels`. Errors, rather
    /// than resampling, when the device refuses the format: the caller falls
    /// back to a piped player, which will convert.
    pub fn open(rate: u32, channels: u16) -> Result<RingSink, String> {
        let shared = Shared::new(ring_frames(rate), channels as usize);
        let (ready_tx, ready_rx) = mpsc::channel::<Result<String, String>>();
        let (tx, rx) = mpsc::channel::<Cmd>();
        let s2 = shared.clone();
        std::thread::Builder::new()
            .name("vidkit-cpal".into())
            .spawn(move || {
                let built = (|| {
                    let host = cpal::default_host();
                    let dev = host.default_output_device().ok_or("no default output device")?;
                    let name = dev.name().unwrap_or_else(|_| "default".into());
                    let format = dev.default_output_config().map_err(|e| e.to_string())?.sample_format();
                    let cfg = cpal::StreamConfig { channels, sample_rate: cpal::SampleRate(rate), buffer_size: cpal::BufferSize::Default };
                    let err = |e: cpal::StreamError| eprintln!("vidkit audio: {e}");
                    let s3 = s2.clone();
                    let latency = move |info: &cpal::OutputCallbackInfo| {
                        let t = info.timestamp();
                        if let Some(d) = t.playback.duration_since(&t.callback) {
                            s3.device_latency_ns.store(d.as_nanos() as u64, Ordering::Relaxed);
                        }
                    };
                    let s4 = s2.clone();
                    let mut scratch = Vec::new();
                    let stream = match format {
                        cpal::SampleFormat::F32 => dev.build_output_stream(
                            &cfg,
                            move |out: &mut [f32], info| { latency(info); fill(&s4, &mut scratch, out) },
                            err,
                            None,
                        ),
                        _ => dev.build_output_stream(
                            &cfg,
                            move |out: &mut [i16], info| { latency(info); fill(&s4, &mut scratch, out) },
                            err,
                            None,
                        ),
                    }
                    .map_err(|e| e.to_string())?;
                    stream.play().map_err(|e| e.to_string())?;
                    Ok::<_, String>((stream, name))
                })();
                match built {
                    Ok((stream, name)) => {
                        let _ = ready_tx.send(Ok(name));
                        for cmd in rx {
                            match cmd {
                                Cmd::Pause(true) => { let _ = stream.pause(); }
                                Cmd::Pause(false) => { let _ = stream.play(); }
                            }
                        }
                        drop(stream);
                    }
                    Err(e) => {
                        let _ = ready_tx.send(Err(e));
                    }
                }
            })
            .map_err(|e| format!("audio thread: {e}"))?;
        let name = ready_rx
            .recv_timeout(Duration::from_secs(3))
            .map_err(|_| "audio device did not open in time".to_string())??;
        Ok(RingSink::new(shared, Box::new(CpalDevice { tx }), rate, channels, format!("ALSA ({name})")))
    }
}

#[cfg(feature = "alsa")]
pub use cpal_device::open as open_cpal;

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicBool;
    use std::time::Instant;

    #[test]
    fn the_ring_is_bounded_and_fills_silence_counting_only_real_underruns() {
        let mut r = PcmRing::new(4, 2); // 8 samples
        let mut out = [9i16; 4];
        r.pop_into(&mut out);
        assert_eq!(out, [0; 4]);
        assert_eq!(r.underrun_frames, 0, "priming silence is not an underrun");
        assert_eq!(r.push(&[1, 2, 3, 4, 5, 6, 7, 8, 9, 10]), 8, "bounded");
        assert_eq!(r.fill_frames(), 4);
        let mut out = [0i16; 6];
        r.pop_into(&mut out);
        assert_eq!(out, [1, 2, 3, 4, 5, 6]);
        let mut out = [7i16; 6];
        r.pop_into(&mut out);
        assert_eq!(out, [7, 8, 0, 0, 0, 0]);
        assert_eq!(r.underrun_frames, 2, "two frames of silence after audio started");
    }

    /// A device simulated at real rate: 10 ms callbacks, 48 kHz stereo.
    struct Sim {
        paused: Arc<AtomicBool>,
    }
    impl Device for Sim {
        fn set_paused(&mut self, p: bool) {
            self.paused.store(p, Ordering::SeqCst);
        }
    }
    fn sim(shared: Arc<Shared>, stop: Arc<AtomicBool>, paused: Arc<AtomicBool>, consumed: Arc<AtomicU64>) {
        std::thread::spawn(move || {
            let mut buf = vec![0i16; 480 * 2];
            let t0 = Instant::now();
            let mut n = 0u64;
            while !stop.load(Ordering::SeqCst) {
                n += 1;
                let due = t0 + Duration::from_millis(10 * n);
                if let Some(d) = due.checked_duration_since(Instant::now()) {
                    std::thread::sleep(d);
                }
                if paused.load(Ordering::SeqCst) {
                    continue;
                }
                shared.render(&mut buf);
                consumed.fetch_add(480, Ordering::SeqCst);
            }
        });
    }

    /// The sink blocks at the device's rate -- so a 100 ms write takes about
    /// 100 ms once the ring is full -- and its latency is the ring's real fill.
    #[test]
    fn writes_are_paced_by_the_device_and_latency_is_measured() {
        let shared = Shared::new(ring_frames(48_000), 2);
        let (stop, paused, consumed) = (Arc::new(AtomicBool::new(false)), Arc::new(AtomicBool::new(false)), Arc::new(AtomicU64::new(0)));
        sim(shared.clone(), stop.clone(), paused.clone(), consumed.clone());
        let mut sink = RingSink::new(shared, Box::new(Sim { paused: paused.clone() }), 48_000, 2, "sim".into());
        let block = vec![1u8; 4800 * 4]; // 100 ms
        let t0 = Instant::now();
        for _ in 0..10 {
            sink.play(&block).unwrap();
        }
        let took = t0.elapsed();
        // 1 s of audio, of which up to 150 ms sits in the ring at the end.
        assert!(took > Duration::from_millis(750) && took < Duration::from_millis(1300), "{took:?}");
        let lat = sink.latency();
        assert!(lat > Duration::from_millis(40) && lat <= Duration::from_millis(160), "latency {lat:?}");

        // Pause reaches the device: it stops consuming, and a write after
        // resuming goes through.
        sink.set_paused(true);
        std::thread::sleep(Duration::from_millis(50));
        let before = consumed.load(Ordering::SeqCst);
        std::thread::sleep(Duration::from_millis(200));
        assert_eq!(consumed.load(Ordering::SeqCst), before, "a paused device consumes nothing");
        sink.set_paused(false);
        sink.play(&block).unwrap();
        stop.store(true, Ordering::SeqCst);
    }

    /// The whole in-process path in the engine: a ring sink as master clock
    /// against real ffmpeg audio. The clock tracks what the device consumed.
    #[test]
    fn the_engine_runs_its_clock_from_a_ring_sink() {
        if std::process::Command::new("ffmpeg").arg("-version").output().is_err() {
            return;
        }
        let p = std::env::temp_dir().join(format!("vk_ring_{}.mkv", std::process::id()));
        assert!(std::process::Command::new("ffmpeg")
            .args(["-v", "error", "-y", "-f", "lavfi", "-i", "testsrc2=size=160x120:rate=30:duration=3", "-f", "lavfi", "-i", "sine=sample_rate=48000:duration=3", "-c:v", "mpeg4", "-c:a", "pcm_s16le"])
            .arg(&p)
            .status()
            .unwrap()
            .success());
        let stop = Arc::new(AtomicBool::new(false));
        let stop2 = stop.clone();
        let maker: crate::playback::SinkMaker = Box::new(move || {
            let shared = Shared::new(ring_frames(48_000), 2);
            let paused = Arc::new(AtomicBool::new(false));
            sim(shared.clone(), stop2.clone(), paused.clone(), Arc::new(AtomicU64::new(0)));
            Box::new(RingSink::new(shared, Box::new(Sim { paused }), 48_000, 2, "sim".into())) as Box<dyn AudioSink>
        });
        let p2 = p.clone();
        let opener: crate::playback::Opener = Box::new(move |from, speed| {
            let v = crate::decode::FfmpegFrames::open_at(&p2, None, from)?;
            let a = crate::decode::AudioStream::open(&p2.to_string_lossy(), 48_000, 2, from, speed).ok();
            Ok((Box::new(v) as Box<dyn crate::decode::VideoSource>, a))
        });
        let mut e = crate::playback::Engine::start(opener, maker, Duration::ZERO, 60).unwrap();
        assert!(e.clock_is_audio());
        let t0 = Instant::now();
        let mut shown = 0;
        while t0.elapsed() < Duration::from_millis(1500) {
            match e.tick() {
                crate::playback::Tick::Show { frame, next } => { shown += 1; e.recycle(frame); std::thread::sleep(next.min(Duration::from_millis(20))); }
                crate::playback::Tick::Wait(d) => std::thread::sleep(d.min(Duration::from_millis(20))),
                crate::playback::Tick::Ended => break,
            }
        }
        let (clock, wall) = (e.position().as_secs_f64(), t0.elapsed().as_secs_f64());
        stop.store(true, Ordering::SeqCst);
        // Consumed audio minus ~150 ms of ring and the first block's priming.
        assert!(clock > wall - 0.35 && clock < wall + 0.05, "clock {clock:.3} vs wall {wall:.3}");
        assert!(shown >= 35, "frames at the film's rate: {shown} in 1.5 s");
        let _ = std::fs::remove_file(&p);
    }

    /// The real cpal path, against whatever the machine's default ALSA
    /// device is. Ignored by default: a machine with no device fails it.
    /// Run with an ALSA `null` default to check the stream builds, drains
    /// and pauses; on a phone, run it as is.
    #[cfg(feature = "alsa")]
    #[test]
    #[ignore]
    fn cpal_opens_and_drains_the_default_device() {
        let mut sink = open_cpal(48_000, 2).expect("open");
        eprintln!("DIAG opened {}", sink.name());
        let block = vec![0u8; 4800 * 4];
        let t0 = Instant::now();
        for _ in 0..5 {
            sink.play(&block).expect("play");
        }
        eprintln!("DIAG 500 ms written in {:?}, latency {:?}", t0.elapsed(), sink.latency());
        sink.set_paused(true);
        sink.set_paused(false);
        sink.play(&block).expect("play after resume");
    }
}
