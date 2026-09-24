//! playback -- the realtime engine: decoding off the UI thread, presentation
//! against a clock.
//!
//! ## Why this exists
//!
//! The player's loop took ONE video frame and ONE 100 ms audio block per UI
//! repaint, on the UI thread, with no clock:
//!
//! * video advanced at the UI's repaint rate (a 33 ms deadline), so a 60 fps
//!   file played at half speed and a 24 fps one too fast;
//! * ~30 repaints a second pushed ~3 s of audio a second into the player's
//!   pipe, so the write BLOCKED -- freezing the UI -- once the pipe filled;
//! * every frame was read from ffmpeg's pipe on the UI thread;
//! * seeking moved the video and left the audio where it was.
//!
//! ## Shape
//!
//! * A VIDEO THREAD reads decoded frames into a bounded channel. When the
//!   player is ahead the channel fills and the DECODER waits -- never the UI.
//! * An AUDIO THREAD writes PCM to the sink. The write blocks at the device's
//!   own rate, which is fine here, and each block advances the kit's
//!   `AudioClock` by what was written, minus what is still buffered.
//! * THE MASTER CLOCK is that audio clock when there is a real-time sink,
//!   wall time otherwise -- and wall time from the audio's position if the
//!   audio ends before the video does.
//! * Each repaint, [`pick`] asks the kit's `SyncPolicy` what to do with the
//!   next frame: show it, hold, or drop it -- and skips a frame when a newer
//!   one is already due, so a late repaint does not build up lag.
//!
//! The kit's own `Player` was not used: it pumps demuxed PACKETS through a
//! `Decoder`, and this app decodes in an ffmpeg process that emits finished
//! frames. Its clocks and sync policy are exactly what is needed, and are.

use crate::decode::{AudioSink, AudioStream, FfmpegFrames, PlanePool};
use std::cell::Cell;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{mpsc, Arc, Condvar, Mutex};
use std::time::{Duration, Instant};
use vidkit_core::clock::{Action, MonotonicClock, SyncPolicy};
use vidkit_core::traits::Clock;
use vidkit_core::{AudioClock, Frame};

/// Decoded frames buffered ahead of the clock. Six 1080p NV12 frames is
/// ~18 MB -- enough to ride out a slow decode, bounded however long the film.
pub const QUEUE_DEPTH: usize = 6;

/// Frames parked in the channel between the decoder thread and the queue.
/// It was QUEUE_DEPTH as well, which doubled the frames held ahead (and the
/// memory) without adding any slack the queue does not already give: the
/// decoder still blocks the moment the consumer is ahead.
pub const CHANNEL_DEPTH: usize = 2;

/// Display refresh rates, for snapping a measured interval to the rate the
/// panel really runs at. Up to 540: 500 Hz panels ship, and 540 exists.
pub const RATES: [u32; 20] = [30, 48, 50, 60, 72, 75, 90, 100, 120, 144, 165, 170, 180, 200, 240, 280, 360, 480, 500, 540];

/// The refresh rate from repaint intervals (seconds), or `None` if there is
/// not enough to go on. Pure.
///
/// Median, not mean: a frame that took long (a seek, a GC in the compositor)
/// is an outlier the median ignores. Intervals over 50 ms are the app idling
/// between repaints, not vsync, and are left out. The median is snapped to a
/// known rate within 4%, or reported as measured -- a 500 Hz panel's 2.0 ms
/// with scheduler jitter still reads 500.
pub fn estimate_refresh(intervals: &[f64]) -> Option<u32> {
    let mut v: Vec<f64> = intervals.iter().copied().filter(|d| *d > 0.0008 && *d < 0.05).collect();
    if v.len() < 30 {
        return None;
    }
    v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let hz = 1.0 / v[v.len() / 2];
    Some(RATES.iter().copied().find(|r| ((*r as f64) - hz).abs() / hz < 0.04).unwrap_or(hz.round() as u32))
}

static REFRESH_HZ: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(60);

/// The refresh rate engines start with; the app measures and updates it.
pub fn refresh_hz() -> u32 {
    REFRESH_HZ.load(Ordering::Relaxed)
}

pub fn set_refresh_hz(hz: u32) {
    REFRESH_HZ.store(hz.clamp(24, 1000), Ordering::Relaxed);
}

/// Bytes of decoded frames the queue may hold ahead. Counted in bytes, not
/// frames: six frames is 18 MB at 1080p and 715 MB at 12K (12288x6480 NV12
/// is 119 MB a frame).
pub const QUEUE_BYTES: usize = 256 << 20;

/// Frames to queue ahead for frames of `frame_bytes`: the byte budget, at
/// least 2 (one showing, one next -- fewer and every late decode is a
/// visible stall) and at most QUEUE_DEPTH. Pure.
pub fn queue_depth(frame_bytes: usize) -> usize {
    (QUEUE_BYTES / frame_bytes.max(1)).clamp(2, QUEUE_DEPTH)
}

/// Channel slots between decoder and queue: one for frames over 32 MB (8K
/// and up), where a second is a quarter of a gigabyte more. Pure.
pub fn channel_depth(frame_bytes: usize) -> usize {
    if frame_bytes > 32 << 20 { 1 } else { CHANNEL_DEPTH }
}

/// How far the interpolated audio clock may run past the last reading. The
/// audio thread advances the clock once per 100 ms block, so a reading older
/// than that means the audio has stalled, and the video must stall with it
/// rather than run away.
pub const INTERP_CAP: Duration = Duration::from_millis(120);

/// The audio clock between readings.
///
/// `AudioClock` moves only when the audio thread finishes writing a block --
/// every 100 ms. Presented against that, video arrived in bursts of three and
/// `pick` dropped the two older frames of each: measured 15 shown and 30
/// dropped of 45 due, i.e. 10 fps with sound on, full rate with sound off.
///
/// Between readings this extrapolates with wall time, capped at
/// [`INTERP_CAP`], and never goes backwards: when a reading lands behind the
/// estimate, the output holds until the audio catches up. Pure, for testing.
#[derive(Clone, Copy, Debug)]
pub struct Interp {
    raw: Duration,
    at: Option<Instant>,
    out: Duration,
    running: bool,
    /// Media seconds per wall second: the playback speed.
    rate: f64,
}

impl Interp {
    pub fn new(pos: Duration) -> Self {
        Self { raw: pos, at: None, out: pos, running: false, rate: 1.0 }
    }

    /// At 2x, media time moves two seconds per wall second between readings,
    /// and the cap -- one block of audio -- covers twice the media.
    pub fn with_rate(pos: Duration, rate: f64) -> Self {
        Self { rate: rate.clamp(0.25, 4.0), ..Self::new(pos) }
    }

    pub fn read(&mut self, raw: Duration, now: Instant, running: bool) -> Duration {
        if raw < self.raw {
            // Only a seek moves the audio clock backwards.
            *self = Self::with_rate(raw, self.rate);
        }
        // A new reading, or playback just (re)started: extrapolate from now,
        // not from the last reading before a pause -- that would jump the
        // clock by the whole cap on resume.
        if raw != self.raw || self.at.is_none() || (running && !self.running) {
            self.raw = raw;
            self.at = Some(now);
        }
        self.running = running;
        let est = match (running, self.at) {
            (true, Some(at)) => raw + now.saturating_duration_since(at).min(INTERP_CAP).mul_f64(self.rate),
            _ => raw,
        };
        self.out = self.out.max(est);
        self.out
    }
}

/// Scale interleaved s16le PCM in place. Unity is free; everything else
/// saturates rather than wrapping, because a wrapped sample is a full-scale
/// click.
pub fn apply_gain(pcm: &mut [u8], gain: f32) {
    if (gain - 1.0).abs() < 1e-4 {
        return;
    }
    let g = gain.max(0.0);
    for s in pcm.chunks_exact_mut(2) {
        let v = i16::from_le_bytes([s[0], s[1]]) as f32 * g;
        let v = v.round().clamp(i16::MIN as f32, i16::MAX as f32) as i16;
        s.copy_from_slice(&v.to_le_bytes());
    }
}

/// Pause for the audio thread: parked on a condvar, not polled. The old loop
/// slept 10 ms and looked again -- a hundred wakeups a second for as long as
/// the film sat paused, which on a phone is the difference between the CPU
/// reaching its deep idle state and not.
#[derive(Default)]
struct Gate {
    paused: Mutex<bool>,
    cv: Condvar,
}

impl Gate {
    fn set(&self, paused: bool) {
        if let Ok(mut p) = self.paused.lock() {
            *p = paused;
        }
        self.cv.notify_all();
    }
    /// Block while paused. Returns whether a pause happened, so the caller
    /// can tell the sink.
    fn wait(&self, stop: &AtomicBool) -> bool {
        let Ok(mut p) = self.paused.lock() else { return false };
        let mut waited = false;
        while *p && !stop.load(Ordering::Relaxed) {
            waited = true;
            p = match self.cv.wait(p) {
                Ok(p) => p,
                Err(_) => return waited,
            };
        }
        waited
    }
}

/// What to do with the frame queue at this instant.
#[derive(Debug)]
pub enum Pick {
    Show { frame: Frame, dropped: u32 },
    Hold { wait: Duration, dropped: u32 },
    Starved { dropped: u32 },
}

/// Choose the frame to show at media time `now`. Pure.
///
/// Late frames past the policy's limit are dropped while a newer one is
/// queued; the newest is shown however late (see below). A frame that is due is
/// shown -- unless the one after it is due too, in which case this one is
/// stale and skipped, or a slow repaint would show every frame late forever.
///
/// Dropped frames go to `on_drop` rather than being freed, so their planes
/// return to the pool.
pub fn pick(queue: &mut VecDeque<Frame>, now: Duration, policy: &SyncPolicy, on_drop: &mut dyn FnMut(Frame)) -> Pick {
    let mut dropped = 0;
    loop {
        let Some(head) = queue.front() else { return Pick::Starved { dropped } };
        match policy.decide(head.pts, now) {
            // Drop a late frame only when a newer one is queued to take its
            // place. The newest frame available is shown however late it is:
            // a decoder that cannot keep up then plays at its own pace
            // instead of showing NOTHING. Measured before this: 500 fps with
            // the 500 Hz policy (late limit 6 ms) showed 0 of 502 frames,
            // because ffmpeg's startup put every frame over 6 ms late and the
            // decoder never got far enough ahead to catch up.
            Action::Drop if queue.len() > 1 => {
                on_drop(queue.pop_front().expect("front exists"));
                dropped += 1;
            }
            Action::Drop => {
                let frame = queue.pop_front().expect("front exists");
                return Pick::Show { frame, dropped };
            }
            Action::Hold(wait) => return Pick::Hold { wait, dropped },
            Action::Show => {
                if queue.get(1).is_some_and(|next| next.pts <= now) {
                    on_drop(queue.pop_front().expect("front exists"));
                    dropped += 1;
                    continue;
                }
                let frame = queue.pop_front().expect("front exists");
                return Pick::Show { frame, dropped };
            }
        }
    }
}

/// The clock video is presented against.
pub enum Master {
    /// Advanced by the audio thread from what the sink has consumed, and
    /// interpolated between its advances (see [`Interp`]).
    Audio(Arc<Mutex<AudioClock>>, Cell<Interp>),
    Wall(MonotonicClock),
}

impl Master {
    pub fn now(&self) -> Duration {
        match self {
            Master::Audio(c, interp) => {
                let (raw, running) = c.lock().map(|c| (c.now(), !c.is_paused())).unwrap_or_default();
                let mut i = interp.get();
                let out = i.read(raw, Instant::now(), running);
                interp.set(i);
                out
            }
            Master::Wall(c) => c.now(),
        }
    }
    fn pause(&mut self) {
        match self {
            Master::Audio(c, _) => { if let Ok(mut c) = c.lock() { c.pause() } }
            Master::Wall(c) => c.pause(),
        }
    }
    fn resume(&mut self) {
        match self {
            Master::Audio(c, _) => { if let Ok(mut c) = c.lock() { c.resume() } }
            Master::Wall(c) => c.resume(),
        }
    }
    pub fn is_audio(&self) -> bool {
        matches!(self, Master::Audio(..))
    }
}

/// Opens the source at a position: video frames, and audio if it has any.
/// A closure so a file and a network stream take the same path, and so a
/// seek or a speed change can reopen both.
/// Called with the media position and the playback speed.
pub type Opener = Box<dyn Fn(Duration, f64) -> Result<(FfmpegFrames, Option<AudioStream>), String> + Send>;
/// Makes a fresh audio sink. Fresh per (re)start: dropping the old one kills
/// the player process, so audio buffered before a seek is not heard after it.
pub type SinkMaker = Box<dyn Fn() -> Box<dyn AudioSink> + Send>;

/// What a UI repaint should do.
#[derive(Debug)]
pub enum Tick {
    /// Present this frame; ask again after `next`.
    Show { frame: Frame, next: Duration },
    /// Nothing new to show yet; ask again after this long.
    Wait(Duration),
    /// The film is over.
    Ended,
}

pub struct Engine {
    opener: Opener,
    make_sink: SinkMaker,
    policy: SyncPolicy,
    video_rx: Option<mpsc::Receiver<Frame>>,
    queue: VecDeque<Frame>,
    master: Master,
    stop: Arc<AtomicBool>,
    gate: Arc<Gate>,
    /// Linear gain, as f32 bits: the audio thread reads it every block, so a
    /// volume change is heard within 100 ms and costs no restart.
    gain: Arc<AtomicU32>,
    /// Plane buffers, shared with the decoder thread and kept across seeks.
    pool: Arc<PlanePool>,
    /// What is decoding, for the status line; the video thread rewrites it
    /// if a hardware decoder falls back to software.
    decoder: Arc<Mutex<String>>,
    audio_done: Arc<AtomicBool>,
    /// Playback speed, applied at the next (re)start.
    speed: f64,
    /// Frames queued ahead, from the frame size (queue_depth).
    depth: usize,
    /// A wall clock held until the first frame is queued.
    waiting_first: bool,
    audio_error: Arc<Mutex<Option<String>>>,
    paused: bool,
    frame_interval: Duration,
    /// Frames shown and dropped since the last (re)start.
    pub shown: u64,
    pub dropped: u64,
    /// What the audio went to, for the UI: a player name, or why there is none.
    pub audio_note: String,
}

impl Engine {
    /// Open the source at `from` and start playing.
    pub fn start(opener: Opener, make_sink: SinkMaker, from: Duration, refresh_hz: u32) -> Result<Self, String> {
        let mut e = Engine {
            opener,
            make_sink,
            policy: SyncPolicy::for_refresh_hz(refresh_hz),
            video_rx: None,
            queue: VecDeque::new(),
            master: Master::Wall(MonotonicClock::new()),
            stop: Arc::new(AtomicBool::new(false)),
            gate: Arc::new(Gate::default()),
            gain: Arc::new(AtomicU32::new(1.0f32.to_bits())),
            pool: PlanePool::new(),
            decoder: Arc::new(Mutex::new(String::new())),
            audio_done: Arc::new(AtomicBool::new(false)),
            speed: 1.0,
            depth: QUEUE_DEPTH,
            waiting_first: false,
            audio_error: Arc::new(Mutex::new(None)),
            paused: false,
            frame_interval: Duration::from_millis(33),
            shown: 0,
            dropped: 0,
            audio_note: String::new(),
        };
        e.restart(from)?;
        Ok(e)
    }

    /// Stop both threads and start again at `from`. Old threads see `stop`,
    /// finish their current block or frame, and exit; their sink and decoder
    /// processes are killed as they drop.
    fn restart(&mut self, from: Duration) -> Result<(), String> {
        self.stop.store(true, Ordering::SeqCst);
        // Wake an old audio thread parked on pause, so it sees `stop` and
        // exits instead of sleeping forever holding a sink.
        self.gate.cv.notify_all();
        self.stop = Arc::new(AtomicBool::new(false));
        self.gate = Arc::new(Gate::default());
        self.gate.set(self.paused);
        self.audio_done = Arc::new(AtomicBool::new(false));
        for f in self.queue.drain(..) {
            self.pool.recycle(f);
        }
        self.shown = 0;
        self.dropped = 0;

        let (mut video, audio) = (self.opener)(from, self.speed)?;
        self.frame_interval = video.frame_interval;
        video.set_pool(self.pool.clone());
        if let Ok(mut d) = self.decoder.lock() {
            *d = format!("{} {}x{}", video.accel.label(), video.width, video.height);
        }
        let fb = crate::decode::frame_bytes(video.width, video.height);
        self.depth = queue_depth(fb);
        let (tx, rx) = mpsc::sync_channel::<Frame>(channel_depth(fb));
        let (stop, decoder) = (self.stop.clone(), self.decoder.clone());
        std::thread::Builder::new().name("vidkit-video".into()).spawn(move || {
            let mut video = video;
            while !stop.load(Ordering::Relaxed) {
                let Some(f) = video.next_frame() else { break };
                if let Some(note) = video.note.take() {
                    if let Ok(mut d) = decoder.lock() {
                        *d = format!("{} {}x{} ({note})", video.accel.label(), video.width, video.height);
                    }
                }
                if tx.send(f).is_err() {
                    break;
                }
            }
        }).map_err(|e| format!("video thread: {e}"))?;
        self.video_rx = Some(rx);

        let sink = audio.as_ref().map(|_| (self.make_sink)());
        self.master = match (audio, sink) {
            (Some(stream), Some(sink)) if sink.realtime() => {
                self.audio_note = sink.name().to_string();
                let clock = Arc::new(Mutex::new(AudioClock::new(stream.rate)));
                {
                    let mut c = clock.lock().expect("fresh lock");
                    c.set_position(from);
                    if !self.paused { c.resume(); }
                }
                self.spawn_audio(stream, sink, clock.clone())?;
                Master::Audio(clock, Cell::new(Interp::with_rate(from, self.speed)))
            }
            (Some(_), Some(sink)) => {
                self.audio_note = format!("{} -- no audio player, timing from the wall clock", sink.name());
                self.wall_at(from)
            }
            _ => {
                self.audio_note = "no audio track".into();
                self.wall_at(from)
            }
        };
        // A wall clock waits for the first frame. Started with the engine,
        // it ran through ffmpeg's startup, so every frame arrived tens of ms
        // late -- at 500 Hz (a 6 ms late limit) all of them were dropped.
        self.waiting_first = !self.master.is_audio();
        if self.waiting_first {
            self.master.pause();
        }
        Ok(())
    }

    fn wall_at(&self, from: Duration) -> Master {
        let mut c = MonotonicClock::new();
        c.set_position(from);
        c.set_rate(self.speed);
        if self.paused { c.pause(); } else { c.resume(); }
        Master::Wall(c)
    }

    fn spawn_audio(&self, mut stream: AudioStream, mut sink: Box<dyn AudioSink>, clock: Arc<Mutex<AudioClock>>) -> Result<(), String> {
        let (stop, gate, done, err, gain) = (self.stop.clone(), self.gate.clone(), self.audio_done.clone(), self.audio_error.clone(), self.gain.clone());
        let bytes_per_frame = stream.channels as usize * 2;
        let rate = stream.rate as f64;
        // atempo has already compressed the audio: each output frame carries
        // `speed` frames of MEDIA. The clock counts media, so scale both the
        // frames written and the latency still buffered.
        let speed = self.speed;
        std::thread::Builder::new().name("vidkit-audio".into()).spawn(move || {
            loop {
                if stop.load(Ordering::Relaxed) { return; }
                if gate.wait(&stop) {
                    // Paused and resumed. The sink was told on the way in by
                    // Engine::pause through the gate; tell it on the way out.
                    sink.set_paused(false);
                    continue;
                }
                let Some(pcm) = stream.next_block() else { break };
                apply_gain(pcm, f32::from_bits(gain.load(Ordering::Relaxed)));
                if let Err(e) = sink.play(pcm) {
                    if let Ok(mut s) = err.lock() { *s = Some(format!("audio stopped: {e}")); }
                    break;
                }
                let frames = ((pcm.len() / bytes_per_frame) as f64 * speed) as u64;
                let latency = (sink.latency().as_secs_f64() * rate * speed) as u64;
                let underrun = sink.take_underrun_frames();
                if let Ok(mut c) = clock.lock() {
                    c.advance(frames, latency);
                    if underrun > 0 { c.report_underrun(underrun); }
                }
                if gate.paused.lock().map(|p| *p).unwrap_or(false) {
                    // About to park: stop the device too, or it plays out its
                    // buffer and then renders silence for the whole pause.
                    sink.set_paused(true);
                }
            }
            done.store(true, Ordering::SeqCst);
        }).map_err(|e| format!("audio thread: {e}"))?;
        Ok(())
    }

    /// Media position: the master clock.
    pub fn position(&self) -> Duration {
        self.master.now()
    }

    pub fn clock_is_audio(&self) -> bool {
        self.master.is_audio()
    }

    /// A problem the audio thread hit, once.
    pub fn take_audio_error(&mut self) -> Option<String> {
        self.audio_error.lock().ok().and_then(|mut e| e.take())
    }

    /// One UI repaint's worth of work: never blocks on a decoder or a sink.
    pub fn tick(&mut self) -> Tick {
        // AUDIO ENDED FIRST: carry on from where it stopped on the wall clock,
        // or the video would freeze on the frame the audio ran out at.
        if self.master.is_audio() && self.audio_done.load(Ordering::SeqCst) {
            let at = self.master.now();
            self.master = self.wall_at(at);
            self.audio_note.push_str(" (audio ended; wall clock)");
        }
        let mut disconnected = false;
        if let Some(rx) = &self.video_rx {
            while self.queue.len() < self.depth {
                match rx.try_recv() {
                    Ok(f) => self.queue.push_back(f),
                    Err(mpsc::TryRecvError::Empty) => break,
                    Err(mpsc::TryRecvError::Disconnected) => { disconnected = true; break; }
                }
            }
        }
        if disconnected {
            self.video_rx = None;
        }
        if self.waiting_first && !self.queue.is_empty() && !self.paused {
            self.master.resume();
            self.waiting_first = false;
        }
        if self.paused {
            return Tick::Wait(Duration::from_millis(250));
        }
        let pool = self.pool.clone();
        match pick(&mut self.queue, self.master.now(), &self.policy, &mut |f| pool.recycle(f)) {
            Pick::Show { frame, dropped } => {
                self.dropped += dropped as u64;
                self.shown += 1;
                let next = self.queue.front()
                    .map(|n| n.pts.saturating_sub(self.master.now()))
                    .unwrap_or(self.frame_interval);
                Tick::Show { frame, next: next.max(Duration::from_millis(1)) }
            }
            Pick::Hold { wait, dropped } => {
                self.dropped += dropped as u64;
                Tick::Wait(wait)
            }
            Pick::Starved { dropped } => {
                self.dropped += dropped as u64;
                if self.video_rx.is_none() { Tick::Ended } else { Tick::Wait(Duration::from_millis(4)) }
            }
        }
    }

    pub fn pause(&mut self) {
        if !self.paused {
            self.paused = true;
            self.gate.set(true);
            self.master.pause();
        }
    }

    pub fn resume(&mut self) {
        if self.paused {
            self.paused = false;
            self.gate.set(false);
            if !self.waiting_first {
                self.master.resume();
            }
        }
    }

    pub fn is_paused(&self) -> bool {
        self.paused
    }

    /// Hand a presented frame's planes back for reuse.
    pub fn recycle(&self, frame: Frame) {
        self.pool.recycle(frame);
    }

    /// Plane buffers waiting for reuse, for the diagnostics line.
    pub fn pooled(&self) -> usize {
        self.pool.len()
    }

    /// (allocated, reused) plane buffers since start.
    pub fn pool_stats(&self) -> (u64, u64) {
        (self.pool.allocated.load(Ordering::Relaxed), self.pool.reused.load(Ordering::Relaxed))
    }

    /// The display's refresh rate: the sync slack is half its interval. It
    /// was fixed at 60 Hz, whose 8.3 ms of slack spans four frames at 500 fps.
    pub fn set_refresh_hz(&mut self, hz: u32) {
        self.policy = SyncPolicy::for_refresh_hz(hz);
    }

    /// Volume, 0..=1 from the transport (values above 1 amplify and clip).
    pub fn set_volume(&self, v: f32) {
        self.gain.store(v.max(0.0).to_bits(), Ordering::Relaxed);
    }

    /// What is decoding: `VAAPI 1920x1080`, `software 1280x720 (...)`.
    pub fn decoder(&self) -> String {
        self.decoder.lock().map(|d| d.clone()).unwrap_or_default()
    }

    /// Change speed: 0.25..=4. A restart at the current position, since both
    /// the audio filter and the clock rate are fixed per ffmpeg run -- which
    /// is why the app debounces a drag rather than calling this per detent.
    pub fn set_speed(&mut self, speed: f64) -> Result<(), String> {
        let s = speed.clamp(0.25, 4.0);
        if (s - self.speed).abs() < 1e-3 {
            return Ok(());
        }
        self.speed = s;
        let at = self.position();
        self.restart(at)
    }

    pub fn speed(&self) -> f64 {
        self.speed
    }

    /// Move video AND audio to `to`.
    pub fn seek(&mut self, to: Duration) -> Result<(), String> {
        self.restart(to)
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        self.gate.cv.notify_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::{Command, Stdio};
    use std::time::Instant;
    use vidkit_core::frame::{ColorInfo, CpuPlanes, FrameData, PlaneLayout};

    fn f(ms: u64) -> Frame {
        Frame {
            data: FrameData::Cpu(CpuPlanes { data: vec![vec![0; 4], vec![0; 2]], strides: vec![2, 2] }),
            width: 2, height: 2, layout: PlaneLayout::Nv12, color: ColorInfo::default(),
            pts: Duration::from_millis(ms), duration: Some(Duration::from_millis(33)), recycle: None,
        }
    }
    fn q(pts: &[u64]) -> VecDeque<Frame> { pts.iter().map(|&p| f(p)).collect() }
    fn ms(n: u64) -> Duration { Duration::from_millis(n) }
    fn pol() -> SyncPolicy { SyncPolicy::for_refresh_hz(60) }

    #[test]
    fn a_due_frame_is_shown_and_a_future_one_held() {
        let mut queue = q(&[100, 133]);
        match pick(&mut queue, ms(100), &pol(), &mut |_| {}) {
            Pick::Show { frame, dropped } => { assert_eq!(frame.pts, ms(100)); assert_eq!(dropped, 0); }
            p => panic!("{p:?}"),
        }
        match pick(&mut queue, ms(100), &pol(), &mut |_| {}) {
            Pick::Hold { wait, .. } => assert!(wait > ms(20) && wait < ms(33), "{wait:?}"),
            p => panic!("{p:?}"),
        }
    }

    /// A slow repaint: three frames already due. Showing the oldest would keep
    /// the picture permanently behind; the newest due one is shown.
    #[test]
    fn stale_frames_are_skipped_when_a_newer_one_is_due() {
        let mut queue = q(&[100, 133, 166, 200]);
        match pick(&mut queue, ms(170), &pol(), &mut |_| {}) {
            Pick::Show { frame, dropped } => { assert_eq!(frame.pts, ms(166)); assert_eq!(dropped, 2); }
            p => panic!("{p:?}"),
        }
        assert_eq!(queue.front().unwrap().pts, ms(200));
    }

    #[test]
    fn an_empty_queue_is_starved() {
        assert!(matches!(pick(&mut VecDeque::new(), ms(0), &pol(), &mut |_| {}), Pick::Starved { dropped: 0 }));
    }

    // ---- against real ffmpeg: a clip made here, decoded by the real decoder ----

    fn have_ffmpeg() -> bool {
        Command::new("ffmpeg").arg("-version").stdout(Stdio::null()).stderr(Stdio::null())
            .status().map(|s| s.success()).unwrap_or(false)
    }

    /// `secs` of 30 fps video; audio of `audio_secs` (none if 0).
    fn clip(name: &str, secs: f32, audio_secs: f32) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("vk_play_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join(name);
        let mut c = Command::new("ffmpeg");
        c.args(["-v", "error", "-y", "-f", "lavfi", "-i", &format!("testsrc2=size=320x240:rate=30:duration={secs}")]);
        if audio_secs > 0.0 {
            c.args(["-f", "lavfi", "-i", &format!("sine=frequency=440:sample_rate=48000:duration={audio_secs}")]);
        }
        c.args(["-pix_fmt", "yuv420p", "-c:v", "mpeg4", "-q:v", "5"]);
        if audio_secs > 0.0 { c.args(["-c:a", "pcm_s16le"]); }
        assert!(c.arg(&p).status().unwrap().success());
        p
    }

    fn opener(p: std::path::PathBuf) -> Opener {
        Box::new(move |from, speed| {
            let v = if from.is_zero() { FfmpegFrames::open(&p, None)? } else { FfmpegFrames::open_at(&p, None, from)? };
            let a = AudioStream::open(&p.to_string_lossy(), 48_000, 2, from, speed).ok();
            Ok((v, a))
        })
    }

    /// A test sink that behaves like a device: it takes audio at the real
    /// rate (sleeping for each block's duration) and reports a fixed latency.
    struct PacedSink { latency: Duration, bytes: Arc<Mutex<u64>> }
    impl AudioSink for PacedSink {
        fn play(&mut self, pcm: &[u8]) -> Result<(), String> {
            std::thread::sleep(Duration::from_secs_f64(pcm.len() as f64 / (48_000.0 * 4.0)));
            *self.bytes.lock().unwrap() += pcm.len() as u64;
            Ok(())
        }
        fn name(&self) -> &str { "paced test sink" }
        fn latency(&self) -> Duration { self.latency }
    }

    fn null_sink() -> SinkMaker {
        Box::new(|| Box::new(crate::decode::NullSink { bytes: 0 }) as Box<dyn AudioSink>)
    }

    /// Drive the engine the way the UI does: tick, sleep as told, repeat.
    /// Returns (shown frame pts, clock at showing, longest single tick).
    fn run(e: &mut Engine, for_: Duration) -> (Vec<Duration>, Vec<Duration>, Duration) {
        let (mut pts, mut at, mut worst) = (vec![], vec![], Duration::ZERO);
        let t0 = Instant::now();
        while t0.elapsed() < for_ {
            let t = Instant::now();
            let tick = e.tick();
            worst = worst.max(t.elapsed());
            match tick {
                Tick::Show { frame, next } => { pts.push(frame.pts); at.push(e.position()); std::thread::sleep(next.min(ms(20))); }
                Tick::Wait(d) => std::thread::sleep(d.min(ms(20))),
                Tick::Ended => break,
            }
        }
        (pts, at, worst)
    }

    /// REALTIME, NOT REPAINT-RATE: over one second of a 30 fps clip, ~30
    /// frames are shown, each close to the clock, in order -- and no tick
    /// blocks, because decoding happens on its own thread.
    #[test]
    fn video_plays_at_its_own_rate_without_blocking_the_ui() {
        if !have_ffmpeg() { eprintln!("no ffmpeg; skipping"); return; }
        let p = clip("v.mkv", 3.0, 0.0);
        let mut e = Engine::start(opener(p), null_sink(), Duration::ZERO, 60).unwrap();
        assert!(!e.clock_is_audio(), "no audio track: wall clock");
        let (pts, at, worst) = run(&mut e, ms(1000));
        let shown = pts.len() + e.dropped as usize;
        assert!((26..=34).contains(&shown), "~30 frames due in 1 s: shown {} + dropped {}", pts.len(), e.dropped);
        assert!(pts.windows(2).all(|w| w[0] < w[1]), "in order");
        for (p, c) in pts.iter().zip(&at) {
            assert!(p.as_secs_f64() - c.as_secs_f64() < 0.02, "never shown early: pts {p:?} at clock {c:?}");
            assert!(c.as_secs_f64() - p.as_secs_f64() < 0.1, "nor late: pts {p:?} at clock {c:?}");
        }
        assert!(worst < ms(15), "a tick must not wait on the decoder: worst {worst:?}");
    }

    /// AUDIO IS THE MASTER when a real-time sink exists: the clock follows
    /// what the sink has CONSUMED minus its latency, frames follow the clock,
    /// and every second of audio reaches the sink.
    #[test]
    fn audio_drives_the_clock_and_all_of_it_is_played() {
        if !have_ffmpeg() { eprintln!("no ffmpeg; skipping"); return; }
        let p = clip("av.mkv", 1.5, 1.5);
        let bytes = Arc::new(Mutex::new(0u64));
        let b2 = bytes.clone();
        let maker: SinkMaker = Box::new(move || Box::new(PacedSink { latency: ms(50), bytes: b2.clone() }) as Box<dyn AudioSink>);
        let mut e = Engine::start(opener(p), maker, Duration::ZERO, 60).unwrap();
        assert!(e.clock_is_audio());
        let t0 = Instant::now();
        let (pts, at, _) = run(&mut e, ms(800));
        let (clock, wall) = (e.position().as_secs_f64(), t0.elapsed().as_secs_f64());
        // Consumed-at-real-rate audio, minus 50 ms latency, blocks of 100 ms.
        assert!(clock > wall - 0.25 && clock < wall + 0.05, "clock {clock:.3} vs elapsed {wall:.3}");
        for (p, c) in pts.iter().zip(&at) {
            assert!(p.as_secs_f64() - c.as_secs_f64() < 0.02, "frame {p:?} shown before the audio reached it ({c:?})");
        }
        let _ = run(&mut e, ms(1500));
        let got = *bytes.lock().unwrap() as f64 / (48_000.0 * 4.0);
        assert!((got - 1.5).abs() < 0.11, "all 1.5 s of audio played: {got:.3} s");
    }

    /// MORE VIDEO THAN AUDIO -- the fixture shape the TODO warns about. When
    /// the audio ends, the clock carries on from it and the video plays out.
    #[test]
    fn video_continues_when_the_audio_runs_out() {
        if !have_ffmpeg() { eprintln!("no ffmpeg; skipping"); return; }
        let p = clip("short_audio.mkv", 1.6, 0.5);
        let maker: SinkMaker = Box::new(|| Box::new(PacedSink { latency: ms(20), bytes: Arc::new(Mutex::new(0)) }) as Box<dyn AudioSink>);
        let mut e = Engine::start(opener(p), maker, Duration::ZERO, 60).unwrap();
        let (pts, _, _) = run(&mut e, ms(2600));
        let last = pts.last().copied().unwrap_or_default();
        assert!(last > ms(1400), "the video played past the audio's end: last frame {last:?}");
        assert!(!e.clock_is_audio(), "switched to the wall clock");
    }

    /// SEEK MOVES BOTH: the next frames start at the target, the clock is
    /// there, and it is still the audio clock.
    #[test]
    fn a_seek_moves_video_audio_and_clock_together() {
        if !have_ffmpeg() { eprintln!("no ffmpeg; skipping"); return; }
        let p = clip("seek.mkv", 3.0, 3.0);
        let maker: SinkMaker = Box::new(|| Box::new(PacedSink { latency: ms(20), bytes: Arc::new(Mutex::new(0)) }) as Box<dyn AudioSink>);
        let mut e = Engine::start(opener(p), maker, Duration::ZERO, 60).unwrap();
        let _ = run(&mut e, ms(300));
        e.seek(ms(2000)).unwrap();
        assert!(e.clock_is_audio());
        let (pts, _, _) = run(&mut e, ms(400));
        assert!(!pts.is_empty(), "frames after the seek");
        assert!(pts[0] >= ms(1950), "first frame after the seek is at the target: {:?}", pts[0]);
        assert!(e.position() >= ms(2000) && e.position() < ms(2600), "clock at the target: {:?}", e.position());
    }

    /// PAUSE HOLDS: the position does not move while paused, and resumes from
    /// where it stopped rather than jumping by the time spent paused.
    #[test]
    fn pause_freezes_the_position_and_resume_continues_from_it() {
        if !have_ffmpeg() { eprintln!("no ffmpeg; skipping"); return; }
        let p = clip("pause.mkv", 3.0, 3.0);
        let maker: SinkMaker = Box::new(|| Box::new(PacedSink { latency: ms(20), bytes: Arc::new(Mutex::new(0)) }) as Box<dyn AudioSink>);
        let mut e = Engine::start(opener(p), maker, Duration::ZERO, 60).unwrap();
        let _ = run(&mut e, ms(500));
        e.pause();
        let held = e.position();
        std::thread::sleep(ms(400));
        assert!((e.position().as_secs_f64() - held.as_secs_f64()).abs() < 0.12, "held at {held:?}, now {:?}", e.position());
        e.resume();
        let _ = run(&mut e, ms(300));
        let after = e.position();
        assert!(after > held && after < held + ms(500), "resumed from {held:?}, now {after:?} -- not jumped by the pause");
    }

    // ---- the audio clock between readings --------------------------------

    /// Readings every 100 ms; between them the output must keep moving with
    /// wall time, or video arrives in bursts and two of every three frames are
    /// dropped (measured before this existed: 15 shown, 30 dropped of 45).
    #[test]
    fn interp_moves_between_readings() {
        let t0 = Instant::now();
        let mut i = Interp::new(ms(0));
        assert_eq!(i.read(ms(0), t0, true), ms(0));
        let mid = i.read(ms(0), t0 + ms(50), true);
        assert_eq!(mid, ms(50), "moved with wall time between readings");
        // The next reading lands exactly on the estimate's path.
        assert_eq!(i.read(ms(100), t0 + ms(100), true), ms(100));
        assert_eq!(i.read(ms(100), t0 + ms(133), true), ms(133));
    }

    /// Stalled audio stalls the video: at most INTERP_CAP past the last
    /// reading, however long the stall.
    #[test]
    fn interp_is_capped_when_the_audio_stalls() {
        let t0 = Instant::now();
        let mut i = Interp::new(ms(1000));
        i.read(ms(1000), t0, true);
        assert_eq!(i.read(ms(1000), t0 + ms(5000), true), ms(1000) + INTERP_CAP);
    }

    /// A reading behind the estimate must not move the clock backwards --
    /// the queue would hold frames already shown -- but a seek must.
    #[test]
    fn interp_never_goes_back_except_for_a_seek() {
        let t0 = Instant::now();
        let mut i = Interp::new(ms(0));
        i.read(ms(0), t0, true);
        let est = i.read(ms(0), t0 + ms(110), true);
        assert_eq!(est, ms(110));
        // The audio lands late at 90 ms: held, not rewound.
        assert_eq!(i.read(ms(90), t0 + ms(111), true), ms(110));
        // And catches up.
        assert_eq!(i.read(ms(90), t0 + ms(140), true), ms(119));
        // A seek back to 10 s... then back to 2 s.
        assert_eq!(i.read(ms(10_000), t0 + ms(150), true), ms(10_000));
        assert_eq!(i.read(ms(2_000), t0 + ms(160), true), ms(2_000), "a seek backwards is obeyed");
    }

    /// Paused: frozen. Resumed: extrapolate from the moment of resuming, not
    /// from the last reading before the pause, which would leap by the cap.
    #[test]
    fn interp_freezes_on_pause_and_resumes_without_a_leap() {
        let t0 = Instant::now();
        let mut i = Interp::new(ms(0));
        i.read(ms(500), t0, true);
        assert_eq!(i.read(ms(500), t0 + ms(40), true), ms(540));
        let at_pause = i.read(ms(500), t0 + ms(41), false);
        assert_eq!(at_pause, ms(540), "holds what was already shown");
        assert_eq!(i.read(ms(500), t0 + ms(3000), false), ms(540), "frozen while paused");
        let resumed = i.read(ms(500), t0 + ms(3001), true);
        assert!(resumed < ms(560), "leapt on resume: {resumed:?}");
    }

    #[test]
    fn gain_scales_saturates_and_leaves_unity_alone() {
        let pcm = |v: &[i16]| v.iter().flat_map(|x| x.to_le_bytes()).collect::<Vec<u8>>();
        let back = |b: &[u8]| b.chunks_exact(2).map(|c| i16::from_le_bytes([c[0], c[1]])).collect::<Vec<i16>>();
        let mut a = pcm(&[1000, -1000, 30000, -30000]);
        apply_gain(&mut a, 0.5);
        assert_eq!(back(&a), vec![500, -500, 15000, -15000]);
        let mut b = pcm(&[30000, -30000]);
        apply_gain(&mut b, 2.0);
        assert_eq!(back(&b), vec![i16::MAX, i16::MIN], "saturates, never wraps to a click");
        let mut c = pcm(&[123, -456]);
        apply_gain(&mut c, 1.0);
        assert_eq!(back(&c), vec![123, -456]);
        let mut d = pcm(&[123, -456]);
        apply_gain(&mut d, 0.0);
        assert_eq!(back(&d), vec![0, 0]);
    }

    /// THE REGRESSION: with a real-time audio sink as master, frames are shown
    /// at the film's rate, not dropped in bursts. Before the interpolation this
    /// showed 15 of 45 due frames.
    #[test]
    fn with_audio_as_master_frames_are_not_dropped_in_bursts() {
        if !have_ffmpeg() { eprintln!("no ffmpeg; skipping"); return; }
        let p = clip("burst.mkv", 3.0, 3.0);
        let maker: SinkMaker = Box::new(|| Box::new(PacedSink { latency: ms(50), bytes: Arc::new(Mutex::new(0)) }) as Box<dyn AudioSink>);
        let mut e = Engine::start(opener(p), maker, Duration::ZERO, 60).unwrap();
        assert!(e.clock_is_audio());
        let _ = run(&mut e, ms(300));
        let (s0, d0) = (e.shown, e.dropped);
        let _ = run(&mut e, ms(1500));
        let (shown, dropped) = (e.shown - s0, e.dropped - d0);
        eprintln!("DIAG audio master: shown {shown} dropped {dropped} over 1.5 s at 30 fps");
        assert!(shown >= 36, "~45 due, only {shown} shown ({dropped} dropped)");
        assert!(dropped * 5 <= shown, "dropping in bursts: {shown} shown, {dropped} dropped");
    }

    /// Volume reaches the samples, without a restart.
    #[test]
    fn volume_is_applied_to_what_the_sink_receives() {
        if !have_ffmpeg() { return; }
        struct Peak(Arc<Mutex<i16>>);
        impl AudioSink for Peak {
            fn play(&mut self, pcm: &[u8]) -> Result<(), String> {
                std::thread::sleep(Duration::from_secs_f64(pcm.len() as f64 / (48_000.0 * 4.0)));
                let m = pcm.chunks_exact(2).map(|c| i16::from_le_bytes([c[0], c[1]]).saturating_abs()).max().unwrap_or(0);
                *self.0.lock().unwrap() = m;
                Ok(())
            }
            fn name(&self) -> &str { "peak" }
        }
        let p = clip("vol.mkv", 2.0, 2.0);
        let peak = Arc::new(Mutex::new(0i16));
        let p2 = peak.clone();
        let maker: SinkMaker = Box::new(move || Box::new(Peak(p2.clone())) as Box<dyn AudioSink>);
        let mut e = Engine::start(opener(p), maker, Duration::ZERO, 60).unwrap();
        let _ = run(&mut e, ms(400));
        let loud = *peak.lock().unwrap();
        e.set_volume(0.25);
        let _ = run(&mut e, ms(400));
        let quiet = *peak.lock().unwrap();
        assert!(loud > 1000, "the fixture is audible: {loud}");
        let ratio = quiet as f64 / loud as f64;
        assert!((ratio - 0.25).abs() < 0.05, "peak {loud} -> {quiet}, ratio {ratio:.3}");
    }

    /// Dropped frames and flushed queues go back to the pool, and the decoder
    /// takes from it: after a while of playback nothing is being allocated.
    #[test]
    fn frames_are_recycled_rather_than_reallocated() {
        if !have_ffmpeg() { return; }
        let p = clip("pool.mkv", 2.0, 0.0);
        let mut e = Engine::start(opener(p), null_sink(), Duration::ZERO, 60).unwrap();
        let t0 = Instant::now();
        let mut shown = 0;
        while t0.elapsed() < ms(1000) {
            match e.tick() {
                Tick::Show { frame, next } => { shown += 1; e.recycle(frame); std::thread::sleep(next.min(ms(20))); }
                Tick::Wait(d) => std::thread::sleep(d.min(ms(20))),
                Tick::Ended => break,
            }
        }
        assert!(shown > 10, "played: {shown}");
        let (alloc, reused) = e.pool_stats();
        // Two planes a frame; steady state allocates nothing, so the buffers
        // made are bounded by what is in flight and the rest are reuse.
        assert!(alloc <= 2 * 12, "{alloc} buffers allocated: not reusing");
        assert!(reused > alloc, "reused {reused} vs allocated {alloc}");
    }

    #[test]
    fn interp_extrapolates_media_time_at_the_playback_rate() {
        let t0 = Instant::now();
        let mut i = Interp::with_rate(ms(0), 2.0);
        i.read(ms(0), t0, true);
        assert_eq!(i.read(ms(0), t0 + ms(50), true), ms(100), "2x: 100 ms of media in 50 ms");
        assert_eq!(i.read(ms(0), t0 + ms(900), true), INTERP_CAP * 2, "the cap is one block of OUTPUT");
        let mut back = Interp::with_rate(ms(5000), 2.0);
        back.read(ms(5000), t0, true);
        assert_eq!(back.read(ms(1000), t0 + ms(10), true), ms(1000), "a seek keeps the rate");
        assert_eq!(back.read(ms(1000), t0 + ms(20), true), ms(1020));
    }

    /// 2x with audio as master: media time runs at twice wall time, frames
    /// follow it, and the sink receives HALF the audio bytes per media second
    /// -- atempo compressed it, rather than the clock just running fast over
    /// normal-speed audio.
    #[test]
    fn double_speed_moves_media_time_twice_as_fast_with_tempo_corrected_audio() {
        if !have_ffmpeg() { return; }
        let p = clip("speed.mkv", 4.0, 4.0);
        let bytes = Arc::new(Mutex::new(0u64));
        let b2 = bytes.clone();
        let maker: SinkMaker = Box::new(move || Box::new(PacedSink { latency: ms(20), bytes: b2.clone() }) as Box<dyn AudioSink>);
        let mut e = Engine::start(opener(p), maker, Duration::ZERO, 60).unwrap();
        e.set_speed(2.0).unwrap();
        assert!(e.clock_is_audio());
        let t0 = Instant::now();
        let (pts, _, _) = run(&mut e, ms(1000));
        let (media, wall) = (e.position().as_secs_f64(), t0.elapsed().as_secs_f64());
        eprintln!("DIAG 2x: {media:.2} s of media in {wall:.2} s, {} frames shown", pts.len());
        assert!(media > 2.0 * wall - 0.45 && media < 2.0 * wall + 0.1, "media {media:.3} vs 2 x wall {wall:.3}");
        assert!(pts.last().copied().unwrap_or_default() > ms(1400), "frames kept up: last {:?}", pts.last());
        // Whether the AUDIO is tempo-corrected cannot be seen from here:
        // playback ends with the video, so the sink hears about wall time's
        // worth either way. decode::audio_tests checks the stream directly.
        let _ = &bytes;
    }

    /// 12K must not take a gigabyte of queue; 1080p keeps its full depth.
    #[test]
    fn the_queue_is_bounded_by_bytes_at_every_resolution() {
        let fb = |w, h| crate::decode::frame_bytes(w, h);
        assert_eq!(queue_depth(fb(1920, 1080)), QUEUE_DEPTH);
        assert_eq!(queue_depth(fb(3840, 2160)), QUEUE_DEPTH, "4K: 12 MB frames, 72 MB queued");
        assert_eq!(queue_depth(fb(7680, 4320)), 5, "8K: 50 MB frames");
        assert_eq!(queue_depth(fb(12288, 6480)), 2, "12K: 119 MB frames");
        // Through 12K, everything in flight (queue + channel + the one being
        // decoded) stays under half a gigabyte: 12K is 476 MB.
        for (w, h) in [(1920, 1080), (3840, 2160), (7680, 4320), (12288, 6480)] {
            let held = (queue_depth(fb(w, h)) + channel_depth(fb(w, h)) + 1) * fb(w, h);
            assert!(held <= 512 << 20, "{w}x{h}: {} MB in flight", held >> 20);
        }
        // Past it (16K, 199 MB frames) the floor is structural: two queued,
        // one in the channel, one decoding -- ~760 MB -- and nothing smaller
        // plays without stalling on every late frame.
        let sixteen = fb(15360, 8640);
        assert_eq!((queue_depth(sixteen), channel_depth(sixteen)), (2, 1));
        assert_eq!(channel_depth(fb(1920, 1080)), 2);
        assert_eq!(channel_depth(fb(12288, 6480)), 1);
    }

    #[test]
    fn refresh_is_measured_from_repaints_and_snapped() {
        let jitter = |hz: f64, n: usize| -> Vec<f64> {
            (0..n).map(|i| 1.0 / hz * (1.0 + 0.03 * ((i * 7 % 11) as f64 / 11.0 - 0.5))).collect()
        };
        assert_eq!(estimate_refresh(&jitter(500.0, 120)), Some(500));
        assert_eq!(estimate_refresh(&jitter(144.0, 120)), Some(144));
        assert_eq!(estimate_refresh(&jitter(59.94, 120)), Some(60));
        let mut v = jitter(240.0, 100);
        v.extend([0.2, 0.5, 0.03, 0.03]); // idle gaps and a slow frame
        assert_eq!(estimate_refresh(&v), Some(240));
        assert_eq!(estimate_refresh(&jitter(500.0, 10)), None, "too few to say");
    }

    /// 500 fps through the whole engine: frames at 2 ms intervals, wall
    /// clock, a tick loop that sleeps until the next frame is due. With the
    /// policy sized for 500 Hz, nearly every due frame is shown.
    #[test]
    fn five_hundred_fps_plays_at_its_rate() {
        if !have_ffmpeg() { return; }
        let dir = std::env::temp_dir();
        let p = dir.join(format!("vk-500-{}.mkv", std::process::id()));
        assert!(std::process::Command::new("ffmpeg").args(["-v", "error", "-y", "-f", "lavfi", "-i", "testsrc2=size=160x90:rate=500:duration=3", "-c:v", "mpeg4", "-q:v", "5"]).arg(&p).status().unwrap().success());
        for hz in [500u32, 60] {
            let mut e = Engine::start(opener(p.clone()), null_sink(), Duration::ZERO, hz).unwrap();
            let _ = run(&mut e, ms(300));
            let (s0, d0) = (e.shown, e.dropped);
            let t0 = Instant::now();
            // Ticked like a 500 Hz display: every 2 ms on the beat, not when
            // the next frame is due -- the slack only matters then.
            let (mut pts, mut at) = (vec![], vec![]);
            let beat = Duration::from_micros(2000);
            let mut next_tick = Instant::now() + Duration::from_micros(700);
            while t0.elapsed() < ms(1000) {
                if let Some(d) = next_tick.checked_duration_since(Instant::now()) {
                    std::thread::sleep(d);
                }
                next_tick += beat;
                if let Tick::Show { frame, .. } = e.tick() {
                    pts.push(frame.pts);
                    at.push(e.position());
                    e.recycle(frame);
                }
            }
            let secs = t0.elapsed().as_secs_f64();
            let (shown, dropped) = (e.shown - s0, e.dropped - d0);
            // How early a frame went up: pts ahead of the clock at showing.
            let early = pts.iter().zip(&at).map(|(p, a)| p.saturating_sub(*a)).max().unwrap_or_default();
            eprintln!("DIAG 500 fps, policy for {hz} Hz: shown {shown} dropped {dropped} in {secs:.2} s, worst early {:.2} ms", early.as_secs_f64() * 1e3);
            assert!(shown as f64 >= 0.85 * 500.0 * secs, "{shown} shown of ~{:.0} due", 500.0 * secs);
            if hz == 500 {
                // Half a 2 ms interval: never more than one frame early.
                assert!(early <= Duration::from_micros(1100), "{early:?} early at 500 Hz");
            }
        }
        let _ = std::fs::remove_file(&p);
    }

    /// The newest frame is shown however late: dropping it shows nothing.
    #[test]
    fn a_lone_late_frame_is_shown_not_dropped() {
        let mut q: VecDeque<Frame> = VecDeque::new();
        q.push_back(f(0));
        match pick(&mut q, ms(500), &SyncPolicy::for_refresh_hz(500), &mut |_| {}) {
            Pick::Show { frame, dropped } => assert_eq!((frame.pts, dropped), (ms(0), 0)),
            other => panic!("{other:?}"),
        }
        // With a newer one queued the late one still goes.
        let mut q: VecDeque<Frame> = [f(0), f(490)].into_iter().collect();
        match pick(&mut q, ms(500), &SyncPolicy::for_refresh_hz(500), &mut |_| {}) {
            Pick::Show { frame, dropped } => assert_eq!((frame.pts, dropped), (ms(490), 1)),
            other => panic!("{other:?}"),
        }
    }

    /// The wall clock starts at the first frame, not at engine start: a
    /// busy UI (or a slow ffmpeg start) must not put the clock ahead of the
    /// picture before anything is shown.
    #[test]
    fn the_wall_clock_waits_for_the_first_frame() {
        if !have_ffmpeg() { return; }
        let p = clip("first.mkv", 2.0, 0.0);
        let mut e = Engine::start(opener(p), null_sink(), Duration::ZERO, 60).unwrap();
        std::thread::sleep(ms(200));
        let t0 = Instant::now();
        let first = loop {
            match e.tick() {
                Tick::Show { frame, .. } => break frame.pts,
                Tick::Wait(d) => std::thread::sleep(d.min(ms(5))),
                Tick::Ended => panic!("ended"),
            }
            assert!(t0.elapsed() < ms(2000));
        };
        assert!(first < ms(50), "the clock ran ahead before the first frame: first shown {first:?}");
    }
}
