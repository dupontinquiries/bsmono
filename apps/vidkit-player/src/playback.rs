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

use crate::decode::{AudioSink, AudioStream, FfmpegFrames};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;
use vidkit_core::clock::{Action, MonotonicClock, SyncPolicy};
use vidkit_core::traits::Clock;
use vidkit_core::{AudioClock, Frame};

/// Decoded frames buffered ahead of the clock. Six 1080p NV12 frames is
/// ~18 MB -- enough to ride out a slow decode, bounded however long the film.
pub const QUEUE_DEPTH: usize = 6;

/// What to do with the frame queue at this instant.
#[derive(Debug)]
pub enum Pick {
    Show { frame: Frame, dropped: u32 },
    Hold { wait: Duration, dropped: u32 },
    Starved { dropped: u32 },
}

/// Choose the frame to show at media time `now`. Pure.
///
/// Late frames past the policy's limit are dropped. A frame that is due is
/// shown -- unless the one after it is due too, in which case this one is
/// stale and skipped, or a slow repaint would show every frame late forever.
pub fn pick(queue: &mut VecDeque<Frame>, now: Duration, policy: &SyncPolicy) -> Pick {
    let mut dropped = 0;
    loop {
        let Some(head) = queue.front() else { return Pick::Starved { dropped } };
        match policy.decide(head.pts, now) {
            Action::Drop => {
                queue.pop_front();
                dropped += 1;
            }
            Action::Hold(wait) => return Pick::Hold { wait, dropped },
            Action::Show => {
                if queue.get(1).is_some_and(|next| next.pts <= now) {
                    queue.pop_front();
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
    /// Advanced by the audio thread from what the sink has consumed.
    Audio(Arc<Mutex<AudioClock>>),
    Wall(MonotonicClock),
}

impl Master {
    pub fn now(&self) -> Duration {
        match self {
            Master::Audio(c) => c.lock().map(|c| c.now()).unwrap_or_default(),
            Master::Wall(c) => c.now(),
        }
    }
    fn pause(&mut self) {
        match self {
            Master::Audio(c) => { if let Ok(mut c) = c.lock() { c.pause() } }
            Master::Wall(c) => c.pause(),
        }
    }
    fn resume(&mut self) {
        match self {
            Master::Audio(c) => { if let Ok(mut c) = c.lock() { c.resume() } }
            Master::Wall(c) => c.resume(),
        }
    }
    pub fn is_audio(&self) -> bool {
        matches!(self, Master::Audio(_))
    }
}

/// Opens the source at a position: video frames, and audio if it has any.
/// A closure so a file and a network stream take the same path, and so a
/// seek or a speed change can reopen both.
pub type Opener = Box<dyn Fn(Duration) -> Result<(FfmpegFrames, Option<AudioStream>), String> + Send>;
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
    audio_paused: Arc<AtomicBool>,
    audio_done: Arc<AtomicBool>,
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
            audio_paused: Arc::new(AtomicBool::new(false)),
            audio_done: Arc::new(AtomicBool::new(false)),
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
        self.stop = Arc::new(AtomicBool::new(false));
        self.audio_paused = Arc::new(AtomicBool::new(self.paused));
        self.audio_done = Arc::new(AtomicBool::new(false));
        self.queue.clear();
        self.shown = 0;
        self.dropped = 0;

        let (video, audio) = (self.opener)(from)?;
        self.frame_interval = video.frame_interval;
        let (tx, rx) = mpsc::sync_channel::<Frame>(QUEUE_DEPTH);
        let stop = self.stop.clone();
        std::thread::Builder::new().name("vidkit-video".into()).spawn(move || {
            let mut video = video;
            while !stop.load(Ordering::Relaxed) {
                let Some(f) = video.next_frame() else { break };
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
                Master::Audio(clock)
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
        Ok(())
    }

    fn wall_at(&self, from: Duration) -> Master {
        let mut c = MonotonicClock::new();
        c.set_position(from);
        if self.paused { c.pause(); } else { c.resume(); }
        Master::Wall(c)
    }

    fn spawn_audio(&self, mut stream: AudioStream, mut sink: Box<dyn AudioSink>, clock: Arc<Mutex<AudioClock>>) -> Result<(), String> {
        let (stop, paused, done, err) = (self.stop.clone(), self.audio_paused.clone(), self.audio_done.clone(), self.audio_error.clone());
        let bytes_per_frame = stream.channels as usize * 2;
        let rate = stream.rate as f64;
        std::thread::Builder::new().name("vidkit-audio".into()).spawn(move || {
            loop {
                if stop.load(Ordering::Relaxed) { return; }
                if paused.load(Ordering::Relaxed) {
                    std::thread::sleep(Duration::from_millis(10));
                    continue;
                }
                let Some(pcm) = stream.next_block() else { break };
                if let Err(e) = sink.play(pcm) {
                    if let Ok(mut s) = err.lock() { *s = Some(format!("audio stopped: {e}")); }
                    break;
                }
                let frames = (pcm.len() / bytes_per_frame) as u64;
                let latency = (sink.latency().as_secs_f64() * rate) as u64;
                if let Ok(mut c) = clock.lock() { c.advance(frames, latency); }
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
            while self.queue.len() < QUEUE_DEPTH {
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
        if self.paused {
            return Tick::Wait(Duration::from_millis(250));
        }
        match pick(&mut self.queue, self.master.now(), &self.policy) {
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
            self.audio_paused.store(true, Ordering::SeqCst);
            self.master.pause();
        }
    }

    pub fn resume(&mut self) {
        if self.paused {
            self.paused = false;
            self.audio_paused.store(false, Ordering::SeqCst);
            self.master.resume();
        }
    }

    pub fn is_paused(&self) -> bool {
        self.paused
    }

    /// Move video AND audio to `to`.
    pub fn seek(&mut self, to: Duration) -> Result<(), String> {
        self.restart(to)
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
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
        match pick(&mut queue, ms(100), &pol()) {
            Pick::Show { frame, dropped } => { assert_eq!(frame.pts, ms(100)); assert_eq!(dropped, 0); }
            p => panic!("{p:?}"),
        }
        match pick(&mut queue, ms(100), &pol()) {
            Pick::Hold { wait, .. } => assert!(wait > ms(20) && wait < ms(33), "{wait:?}"),
            p => panic!("{p:?}"),
        }
    }

    /// A slow repaint: three frames already due. Showing the oldest would keep
    /// the picture permanently behind; the newest due one is shown.
    #[test]
    fn stale_frames_are_skipped_when_a_newer_one_is_due() {
        let mut queue = q(&[100, 133, 166, 200]);
        match pick(&mut queue, ms(170), &pol()) {
            Pick::Show { frame, dropped } => { assert_eq!(frame.pts, ms(166)); assert_eq!(dropped, 2); }
            p => panic!("{p:?}"),
        }
        assert_eq!(queue.front().unwrap().pts, ms(200));
    }

    #[test]
    fn an_empty_queue_is_starved() {
        assert!(matches!(pick(&mut VecDeque::new(), ms(0), &pol()), Pick::Starved { dropped: 0 }));
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
        Box::new(move |from| {
            let v = if from.is_zero() { FfmpegFrames::open(&p, None)? } else { FfmpegFrames::open_at(&p, None, from)? };
            let a = AudioStream::open(&p.to_string_lossy(), 48_000, 2, from).ok();
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
}
