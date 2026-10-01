//! capture-kit — the thing that finally produces a camera frame.
//!
//! Lifted from snapclone's `capture.rs`, where it was written and verified,
//! because the next camera app (snapshot-cam) needed exactly the same
//! engine and the alternative was a second copy that would drift. The
//! decisions and the reasons are unchanged; only the address is new.
//!
//! ## What it is
//!
//! * [`probe`] asks the machine what it has (`pw-dump`, `v4l2-ctl`) and
//!   hands `device-kit` the text; `device-kit` decides the order.
//! * [`run`] walks the plan: for each candidate it asks an injected opener
//!   for a [`FrameStream`], reads fixed-size RGBA frames, and offers each
//!   to a [`Shared`] slot — latest wins, because a preview is not a video
//!   file and a queue turns a stall into permanent lag.
//! * [`gst_opener`] is the real opener: `sh -c "exec gst-launch-1.0 -q
//!   <pipeline>"`, the pipeline's appsink tail retargeted to `fdsink fd=1`
//!   by `device_kit::source_plan::to_fdsink`. Raw video over a pipe has no
//!   framing, so the forced width × height IS the framing.
//! * [`TestPattern`] is a synthetic camera for machines with none: the real
//!   thread, slot, upload, shutter and encoder run; only the pixels come
//!   from arithmetic.
//!
//! ## A subprocess, not a linked GStreamer
//!
//! 1. A UI crate that opens `/dev/video0` cannot be tested; a kit that
//!    links `gstreamer-rs` puts it in every consumer's tree.
//! 2. The loop stays visible: the pipeline being tried is a string you can
//!    read in a status line and paste into a terminal, and the child's
//!    stderr says why it failed in GStreamer's own words.
//! 3. ARM cost: `gst-launch-1.0` on PATH at runtime (postmarketOS ships
//!    it) instead of matching system headers per cross target.
//!
//! ## The stop contract
//!
//! `read_exact` on a pipe blocks, so a flag alone cannot interrupt a stream
//! mid-frame. Stopping is two acts: raise the flag, then [`KillSwitch::stop`]
//! the child so the blocked read sees EOF. Do both.
//!
//! ## What is verified and what is not
//!
//! The plan walk, retry policy, status transitions, frame framing,
//! torn-frame refusal, stop behaviour and the subprocess pipe protocol are
//! tested against fakes and a stand-in launcher script. The
//! `gst-launch-1.0` CLI, caps negotiation and `fdsink` are tested against
//! a REAL GStreamer with `videotestsrc` where one is installed (the test
//! says SKIPPED where it is not). A real camera device still is not — no
//! sandbox has one — but everything after the source element is.

pub mod focus;
pub mod orient;
pub mod probe;

use std::io::Read;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub use device_kit::source_plan::SourceCandidate;

/// A frame as it arrives: RGBA8, `width * height * 4` bytes.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct RawFrame {
    pub width: u32,
    pub height: u32,
    /// Milliseconds since the run started, for staleness and lag.
    pub captured_ms: u64,
    /// Sequence from the producer, monotonic for the life of the run, so
    /// drops can be counted rather than guessed.
    pub seq: u64,
    /// An `Arc` because a frame is cloned from the slot into whoever draws
    /// or saves it; at 30 fps a deep copy per hop warms a phone.
    pub rgba: Arc<[u8]>,
}

impl RawFrame {
    /// Do the pixels describe the claimed geometry? A torn frame must be
    /// refused at the upload, not panicked on inside a renderer.
    pub fn pixels_plausible(&self) -> bool {
        self.width > 0 && self.height > 0
            && self.rgba.len() == (self.width as usize) * (self.height as usize) * 4
    }

    /// The part of the frame inside `window` (`[u0, v0, u1, v1]`, fractions
    /// of width and height), at native pixels. This is what digital zoom
    /// saves: no resampling, so a 2× photo of a 1280×720 frame is a
    /// 640×360 file of real sensor pixels rather than a blurred 1280×720.
    ///
    /// Edges are rounded to whole pixels and never smaller than one; a
    /// window that covers the whole frame returns the frame as it is (the
    /// same `Arc`, no copy). `captured_ms` and `seq` are kept: the crop is
    /// the same moment.
    pub fn crop(&self, window: [f32; 4]) -> RawFrame {
        if !self.pixels_plausible() { return self.clone(); }
        let (w, h) = (self.width as f32, self.height as f32);
        let px = |f: f32, n: f32| (f.clamp(0.0, 1.0) * n).round() as u32;
        let (mut x0, mut y0) = (px(window[0], w), px(window[1], h));
        let (x1, y1) = (px(window[2], w).max(x0 + 1).min(self.width), px(window[3], h).max(y0 + 1).min(self.height));
        x0 = x0.min(x1 - 1);
        y0 = y0.min(y1 - 1);
        if x0 == 0 && y0 == 0 && x1 == self.width && y1 == self.height { return self.clone(); }
        let (cw, ch) = (x1 - x0, y1 - y0);
        let stride = self.width as usize * 4;
        let mut out = Vec::with_capacity(cw as usize * ch as usize * 4);
        for y in y0..y1 {
            let row = y as usize * stride;
            out.extend_from_slice(&self.rgba[row + x0 as usize * 4..row + x1 as usize * 4]);
        }
        RawFrame { width: cw, height: ch, captured_ms: self.captured_ms, seq: self.seq, rgba: out.into() }
    }
}

/// Where the engine stands, for a status line. One value, replaced whole.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    /// No engine is running.
    Idle,
    /// A candidate is being opened or has not yet produced a frame.
    Trying { label: String },
    /// Frames are flowing.
    Live { label: String },
    /// Every candidate was tried and none ever produced a frame. One line
    /// per attempt, in GStreamer's words where it had any.
    Exhausted { log: Vec<String> },
}

impl Status {
    /// The one-line rendering. `None` when there is nothing worth saying
    /// over the picture (Live) or under "no camera found" (Idle).
    pub fn line(&self) -> Option<String> {
        match self {
            Status::Idle | Status::Live { .. } => None,
            Status::Trying { label } => Some(format!("opening {label}…")),
            Status::Exhausted { log } => Some(match log.last() {
                Some(last) => format!("no pipeline worked — last: {last}"),
                None => "no camera pipelines to try".to_string(),
            }),
        }
    }

    pub fn is_live(&self) -> bool { matches!(self, Status::Live { .. }) }
}

/// What the reader thread and the UI share: the latest frame, the status
/// (edge-triggered), and a one-shot "it worked".
#[derive(Default)]
pub struct Shared {
    latest: Mutex<Option<RawFrame>>,
    status: Mutex<Option<Status>>,
    worked: AtomicBool,
}

impl Shared {
    pub fn new() -> Arc<Shared> { Arc::new(Shared::default()) }

    /// The newest frame, if one arrived since the last take.
    pub fn take_frame(&self) -> Option<RawFrame> { self.latest.lock().ok()?.take() }

    /// A copy of the newest frame WITHOUT consuming it — the shutter wants
    /// "what the viewfinder shows right now" without stealing a preview.
    pub fn peek_frame(&self) -> Option<RawFrame> { self.latest.lock().ok()?.clone() }

    /// The status, if it changed since the last take.
    pub fn take_status(&self) -> Option<Status> { self.status.lock().ok()?.take() }

    /// One-shot: did a first frame arrive since the last ask?
    pub fn take_worked(&self) -> bool { self.worked.swap(false, Ordering::Relaxed) }

    fn offer(&self, f: RawFrame) { if let Ok(mut g) = self.latest.lock() { *g = Some(f); } }
    fn set_status(&self, s: Status) { if let Ok(mut g) = self.status.lock() { *g = Some(s); } }
}

/// A running pipeline, reduced to: fill this frame-sized buffer, or say
/// why you no longer can. `Err` is the stream's obituary.
pub trait FrameStream: Send {
    fn read_frame(&mut self, buf: &mut [u8]) -> Result<(), String>;
}

/// A synthetic camera: a drifting gradient with a moving bar at ~15 fps,
/// forever. For `*_TEST_PATTERN=1` runs on machines with no camera and no
/// GStreamer. Not a test fake: everything downstream is real.
pub struct TestPattern {
    width: u32,
    height: u32,
    frame: u64,
    /// Frame pacing. Zero for tests that want frames as fast as possible.
    pub pace: Duration,
}

impl TestPattern {
    pub fn new(width: u32, height: u32) -> TestPattern {
        TestPattern { width, height, frame: 0, pace: Duration::from_millis(66) }
    }

    /// Paint frame `n` into `buf`. Public so an app can draw its own
    /// overlay on top (snapclone draws a QR code on its pattern).
    pub fn paint(width: u32, height: u32, n: u64, buf: &mut [u8]) {
        let (w, h) = (width as usize, height as usize);
        let t = n as f32 * 0.05;
        let bar = ((t.sin() * 0.5 + 0.5) * w as f32) as usize;
        for y in 0..h {
            for x in 0..w {
                let i = (y * w + x) * 4;
                let fx = x as f32 / w as f32;
                let fy = y as f32 / h as f32;
                let (r, g, b) = if x.abs_diff(bar) < (w / 40).max(1) {
                    (250, 240, 200)
                } else {
                    ((40.0 + 160.0 * fx) as u8, (60.0 + 120.0 * fy) as u8,
                     (120.0 + 100.0 * (1.0 - fx) * (1.0 - fy)) as u8)
                };
                buf[i] = r; buf[i + 1] = g; buf[i + 2] = b; buf[i + 3] = 255;
            }
        }
    }
}

impl FrameStream for TestPattern {
    fn read_frame(&mut self, buf: &mut [u8]) -> Result<(), String> {
        let need = (self.width as usize) * (self.height as usize) * 4;
        if buf.len() < need { return Err("test pattern: buffer smaller than a frame".into()); }
        Self::paint(self.width, self.height, self.frame, buf);
        self.frame += 1;
        if !self.pace.is_zero() { std::thread::sleep(self.pace); }
        Ok(())
    }
}

/// How many times a working-then-dying camera is re-opened before the run
/// gives up. Three failures in a row is not a transient glitch, and the
/// cost of being wrong is one page revisit.
pub const MAX_RESTARTS: u32 = 3;

/// Walk the plan until stopped. Synchronous: the host owns the thread.
///
/// * A candidate that fails to open, or opens and never frames, is skipped.
/// * A pass where nothing ever produced a frame ends `Exhausted`.
/// * A stream that dies AFTER producing frames restarts the plan (after
///   `retry_pause`), at most [`MAX_RESTARTS`] times.
pub fn run(
    plan: &[SourceCandidate],
    width: u32,
    height: u32,
    mut open: impl FnMut(&SourceCandidate) -> Result<Box<dyn FrameStream>, String>,
    shared: &Shared,
    stop: &AtomicBool,
    retry_pause: Duration,
) {
    let frame_len = (width as usize) * (height as usize) * 4;
    let t0 = Instant::now();
    let mut seq: u64 = 0;
    let mut restarts: u32 = 0;
    loop {
        let mut log: Vec<String> = Vec::new();
        let mut any_frames_this_pass = false;
        for cand in plan {
            if stop.load(Ordering::Relaxed) { shared.set_status(Status::Idle); return; }
            shared.set_status(Status::Trying { label: cand.label.clone() });
            let mut stream = match open(cand) {
                Ok(s) => s,
                Err(e) => { log.push(format!("{}: {e}", cand.label)); continue; }
            };
            let mut buf = vec![0u8; frame_len];
            let mut frames_here: u64 = 0;
            let died = loop {
                match stream.read_frame(&mut buf) {
                    Ok(()) => {
                        seq += 1;
                        frames_here += 1;
                        if frames_here == 1 {
                            shared.worked.store(true, Ordering::Relaxed);
                            shared.set_status(Status::Live { label: cand.label.clone() });
                        }
                        shared.offer(RawFrame {
                            width, height,
                            captured_ms: t0.elapsed().as_millis() as u64,
                            seq,
                            rgba: Arc::from(&buf[..]),
                        });
                    }
                    Err(e) => break e,
                }
                if stop.load(Ordering::Relaxed) { shared.set_status(Status::Idle); return; }
            };
            log.push(format!("{}: {died} (after {frames_here} frames)", cand.label));
            if frames_here > 0 {
                any_frames_this_pass = true;
                // A working camera died: restart the whole plan rather than
                // sliding to a worse candidate — the order encodes
                // preference, and the death may be transient.
                break;
            }
        }
        if stop.load(Ordering::Relaxed) { shared.set_status(Status::Idle); return; }
        if !any_frames_this_pass { shared.set_status(Status::Exhausted { log }); return; }
        restarts += 1;
        if restarts >= MAX_RESTARTS {
            log.push(format!(
                "gave up after {restarts} restarts — the camera keeps dying after \
                 opening; leave the page and come back to try again"));
            shared.set_status(Status::Exhausted { log });
            return;
        }
        std::thread::sleep(retry_pause);
    }
}

/// Makes a blocked reader stoppable: kill the child and the `read_exact` it
/// was sitting in returns EOF. The second half of the stop contract.
#[derive(Clone, Default)]
pub struct KillSwitch(Arc<Mutex<Option<std::process::Child>>>);

impl KillSwitch {
    pub fn new() -> KillSwitch { KillSwitch::default() }

    /// Kill whatever child is running. The stream reaps it.
    pub fn stop(&self) {
        if let Ok(mut g) = self.0.lock() {
            if let Some(c) = g.as_mut() { let _ = c.kill(); }
        }
    }

    fn install(&self, c: std::process::Child) {
        if let Ok(mut g) = self.0.lock() {
            // A previous child still parked here means a new candidate was
            // opened without reaping the old one; kill it rather than leak
            // a pipeline holding the camera.
            if let Some(old) = g.as_mut() { let _ = old.kill(); let _ = old.wait(); }
            *g = Some(c);
        }
    }

    fn reap(&self) -> Option<(std::process::ExitStatus, String)> {
        let mut c = self.0.lock().ok()?.take()?;
        let status = c.wait().ok()?;
        let mut err = String::new();
        if let Some(mut e) = c.stderr.take() { let _ = e.read_to_string(&mut err); }
        Some((status, err))
    }
}

/// The real opener: `sh -c "exec gst-launch-1.0 -q <pipeline>"`.
///
/// Through `sh` because the pipeline string uses gst-launch's own syntax
/// (`!` separators, a quoted `target-object`), and `exec` so the pid the
/// [`KillSwitch`] kills is gst-launch itself.
pub fn gst_opener(width: u32, height: u32, kill: KillSwitch)
    -> impl FnMut(&SourceCandidate) -> Result<Box<dyn FrameStream>, String> {
    shell_opener("gst-launch-1.0".to_string(), width, height, kill)
}

/// [`gst_opener`] with the launcher injectable, which is what lets the
/// subprocess mechanics be verified against a stand-in that speaks the
/// same protocol: bytes on stdout, words on stderr.
pub fn shell_opener(launcher: String, width: u32, height: u32, kill: KillSwitch)
    -> impl FnMut(&SourceCandidate) -> Result<Box<dyn FrameStream>, String> {
    shell_opener_turned(launcher, width, height, 0, kill)
}

/// [`gst_opener`] that also turns the picture `turn_cw` degrees clockwise (0/90/180/270), so a
/// sideways phone sensor arrives upright (see [`orient`]). `width` x `height` is the UPRIGHT
/// size the reader gets, so a portrait phone passes 720x1280.
pub fn gst_opener_turned(width: u32, height: u32, turn_cw: u32, kill: KillSwitch)
    -> impl FnMut(&SourceCandidate) -> Result<Box<dyn FrameStream>, String> {
    shell_opener_turned("gst-launch-1.0".to_string(), width, height, turn_cw, kill)
}

/// [`shell_opener`] with an upright turn. `turn_cw == 0` is exactly [`shell_opener`].
pub fn shell_opener_turned(launcher: String, width: u32, height: u32, turn_cw: u32, kill: KillSwitch)
    -> impl FnMut(&SourceCandidate) -> Result<Box<dyn FrameStream>, String> {
    move |cand| {
        let fd = device_kit::source_plan::to_fdsink_turned(&cand.pipeline, width, height, turn_cw)
            .ok_or_else(|| "not a retargetable pipeline".to_string())?;
        let mut child = std::process::Command::new("sh")
            .arg("-c")
            .arg(format!("exec {launcher} -q {fd}"))
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .map_err(|e| format!("could not spawn sh/{launcher}: {e}"))?;
        let out = child.stdout.take().ok_or("no stdout on the child")?;
        kill.install(child);
        Ok(Box::new(ChildStream { out, kill: kill.clone() }))
    }
}

struct ChildStream {
    out: std::process::ChildStdout,
    kill: KillSwitch,
}

impl FrameStream for ChildStream {
    fn read_frame(&mut self, buf: &mut [u8]) -> Result<(), String> {
        match self.out.read_exact(buf) {
            Ok(()) => Ok(()),
            Err(e) => {
                // The pipe ended: reap the child and let its stderr be the
                // reason — "no such element pipewiresrc" tells the user
                // everything, "unexpected EOF" nothing.
                let why = match self.kill.reap() {
                    Some((status, err)) if !err.trim().is_empty() =>
                        format!("{status}: {}", err.trim().lines().last().unwrap_or("")),
                    Some((status, _)) => format!("{status}"),
                    None => format!("{e}"),
                };
                Err(why)
            }
        }
    }
}

/// A capture engine on its own thread: the plan, the opener, the slot, the
/// stop. What a host holds.
pub struct Rig {
    pub shared: Arc<Shared>,
    stop: Arc<AtomicBool>,
    kill: KillSwitch,
    thread: Option<std::thread::JoinHandle<()>>,
    pub width: u32,
    pub height: u32,
}

impl Rig {
    /// Start the real engine over `plan`.
    pub fn start(plan: Vec<SourceCandidate>, width: u32, height: u32) -> Rig {
        let kill = KillSwitch::new();
        let opener = gst_opener(width, height, kill.clone());
        Self::start_with(plan, width, height, kill, opener)
    }

    /// [`Rig::start`] with the picture turned `turn_cw` degrees clockwise. `width` x `height` is
    /// the upright size of the frames the host receives.
    pub fn start_turned(plan: Vec<SourceCandidate>, width: u32, height: u32, turn_cw: u32) -> Rig {
        let kill = KillSwitch::new();
        let opener = gst_opener_turned(width, height, turn_cw, kill.clone());
        Self::start_with(plan, width, height, kill, opener)
    }

    /// Start with the test pattern instead of a camera.
    pub fn start_test_pattern(width: u32, height: u32) -> Rig {
        let plan = vec![SourceCandidate {
            kind: device_kit::source_plan::SourceKind::V4l2,
            label: "test pattern".into(),
            pipeline: String::new(),
        }];
        Self::start_with(plan, width, height, KillSwitch::new(),
            move |_| Ok(Box::new(TestPattern::new(width, height)) as Box<dyn FrameStream>))
    }

    /// Start with any opener.
    pub fn start_with(
        plan: Vec<SourceCandidate>, width: u32, height: u32, kill: KillSwitch,
        mut open: impl FnMut(&SourceCandidate) -> Result<Box<dyn FrameStream>, String> + Send + 'static,
    ) -> Rig {
        let shared = Shared::new();
        let stop = Arc::new(AtomicBool::new(false));
        let (s2, st2) = (shared.clone(), stop.clone());
        let thread = std::thread::Builder::new().name("capture-kit".into()).spawn(move || {
            run(&plan, width, height, &mut open, &s2, &st2, Duration::from_millis(500));
        }).ok();
        Rig { shared, stop, kill, thread, width, height }
    }

    /// Both halves of the stop contract, then join.
    pub fn stop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        self.kill.stop();
        if let Some(t) = self.thread.take() { let _ = t.join(); }
    }

    pub fn is_running(&self) -> bool {
        self.thread.as_ref().map(|t| !t.is_finished()).unwrap_or(false)
    }
}

impl Drop for Rig {
    fn drop(&mut self) { self.stop(); }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    fn cand(label: &str) -> SourceCandidate {
        SourceCandidate {
            kind: device_kit::source_plan::SourceKind::V4l2,
            label: label.to_string(),
            pipeline: device_kit::source_plan::v4l2_pipeline("/dev/video0", None),
        }
    }

    struct Scripted { frames: u64, death: &'static str, fill: u8 }
    impl FrameStream for Scripted {
        fn read_frame(&mut self, buf: &mut [u8]) -> Result<(), String> {
            if self.frames == 0 { return Err(self.death.to_string()); }
            self.frames -= 1;
            buf.fill(self.fill);
            Ok(())
        }
    }

    #[test]
    fn the_first_working_candidate_goes_live_and_its_frames_reach_the_slot() {
        let shared = Shared::new();
        let stop = AtomicBool::new(false);
        let plan = vec![cand("broken"), cand("working")];
        let mut opens = VecDeque::from([
            Err("no such element".to_string()),
            Ok(Box::new(Scripted { frames: 3, death: "unplugged", fill: 7 }) as Box<dyn FrameStream>),
            Err("no such element".to_string()),
            Err("gone too".to_string()),
        ]);
        run(&plan, 4, 2, |_| opens.pop_front().expect("opened more than scripted"),
            &shared, &stop, Duration::ZERO);
        let f = shared.take_frame().expect("a frame reached the slot");
        assert_eq!((f.width, f.height), (4, 2));
        assert_eq!(f.seq, 3, "latest wins");
        assert!(f.pixels_plausible());
        assert_eq!(f.rgba[0], 7);
        assert!(shared.take_worked());
        assert!(!shared.take_worked(), "exactly once");
    }

    #[test]
    fn a_plan_that_never_produces_a_frame_exhausts_with_the_reasons() {
        let shared = Shared::new();
        let stop = AtomicBool::new(false);
        let plan = vec![cand("a"), cand("b")];
        run(&plan, 2, 2, |c| -> Result<Box<dyn FrameStream>, String> { Err(format!("cannot open {}", c.label)) },
            &shared, &stop, Duration::ZERO);
        match shared.take_status() {
            Some(Status::Exhausted { log }) => {
                assert_eq!(log.len(), 2, "{log:?}");
                let line = Status::Exhausted { log }.line().unwrap();
                assert!(line.contains("cannot open b"), "{line}");
            }
            other => panic!("expected Exhausted, got {other:?}"),
        }
        assert!(!shared.take_worked());
    }

    #[test]
    fn a_stream_that_opens_but_never_frames_is_a_failure_not_a_livelock() {
        let shared = Shared::new();
        let stop = AtomicBool::new(false);
        let plan = vec![cand("black")];
        run(&plan, 2, 2, |_| Ok(Box::new(Scripted { frames: 0, death: "EOS before any buffer", fill: 0 }) as Box<dyn FrameStream>),
            &shared, &stop, Duration::ZERO);
        match shared.take_status() {
            Some(Status::Exhausted { log }) => assert!(log[0].contains("after 0 frames"), "{log:?}"),
            other => panic!("expected Exhausted, got {other:?}"),
        }
    }

    #[test]
    fn stop_between_frames_ends_the_run_as_idle() {
        struct StopAfter { left: u64, stop: Arc<AtomicBool> }
        impl FrameStream for StopAfter {
            fn read_frame(&mut self, buf: &mut [u8]) -> Result<(), String> {
                if self.left == 0 { return Err("should have stopped already".into()); }
                self.left -= 1;
                if self.left == 0 { self.stop.store(true, Ordering::Relaxed); }
                buf.fill(1);
                Ok(())
            }
        }
        let plan = vec![cand("live")];
        let pre = Shared::new();
        run(&plan, 2, 2, |_| -> Result<Box<dyn FrameStream>, String> { panic!("the opener must not run under a raised flag") },
            &pre, &AtomicBool::new(true), Duration::ZERO);
        assert_eq!(pre.take_status(), Some(Status::Idle));
        let stop = Arc::new(AtomicBool::new(false));
        let shared = Shared::new();
        run(&plan, 2, 2, |_| Ok(Box::new(StopAfter { left: 2, stop: stop.clone() }) as Box<dyn FrameStream>),
            &shared, &stop, Duration::ZERO);
        assert_eq!(shared.take_status(), Some(Status::Idle));
        assert_eq!(shared.take_frame().unwrap().seq, 2);
    }

    #[cfg(unix)]
    fn fake_launcher(name: &str, body: &str) -> std::path::PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("capture-kit-test-{}-{}", std::process::id(), name));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("launcher");
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    #[test]
    #[cfg(unix)]
    fn a_real_subprocess_delivers_framed_bytes_and_a_stderr_obituary() {
        // 4x2 RGBA = 32 bytes/frame. The script writes 2.5 frames — the torn
        // half must not surface — then names its exit. It succeeds every
        // round, which is a flapping camera; MAX_RESTARTS terminates it.
        let launcher = fake_launcher("frames", "head -c 80 /dev/zero | tr '\\0' 'A'; echo 'lens fell off' >&2; exit 3");
        let shared = Shared::new();
        let stop = AtomicBool::new(false);
        let plan = vec![cand("stand-in")];
        run(&plan, 4, 2, shell_opener(launcher.display().to_string(), 4, 2, KillSwitch::new()),
            &shared, &stop, Duration::ZERO);
        let f = shared.take_frame().expect("two whole frames crossed the pipe");
        assert_eq!(f.seq, 2 * u64::from(MAX_RESTARTS), "the torn third frame of some round was counted");
        assert!(f.pixels_plausible());
        assert!(f.rgba.iter().all(|b| *b == b'A'));
        match shared.take_status() {
            Some(Status::Exhausted { log }) => {
                assert!(log.iter().any(|l| l.contains("lens fell off")), "{log:?}");
                assert!(log.iter().any(|l| l.contains("after 2 frames")), "{log:?}");
                assert!(log.last().unwrap().contains("gave up after"), "{log:?}");
            }
            other => panic!("expected Exhausted, got {other:?}"),
        }
    }

    /// The turn must reach the command line the subprocess is given — a stand-in launcher
    /// records its arguments, so this proves the plumbing rather than the pipeline string
    /// (device-kit tests that) and does not need gst-launch on the machine.
    #[test]
    #[cfg(unix)]
    fn the_turn_reaches_the_launchers_command_line() {
        let args = std::env::temp_dir().join(format!("capture-kit-args-{}", std::process::id()));
        let _ = std::fs::remove_file(&args);
        let launcher = fake_launcher("args", &format!("echo \"$*\" > {}; head -c 32 /dev/zero", args.display()));
        let cand = SourceCandidate {
            kind: device_kit::source_plan::SourceKind::PipewireLibcamera,
            label: "n".into(),
            pipeline: device_kit::source_plan::pipewire_pipeline("n", None),
        };
        // Upright size 4x2; a quarter turn means the camera is scaled to 2x4 first.
        let mut open = shell_opener_turned(launcher.display().to_string(), 4, 2, 270, KillSwitch::new());
        let mut stream = open(&cand).expect("opens");
        let mut buf = vec![0u8; 32];
        stream.read_frame(&mut buf).expect("one frame");
        let line = std::fs::read_to_string(&args).unwrap();
        assert!(line.contains("videoflip method=counterclockwise"), "{line}");
        assert!(line.contains("width=2,height=4"), "scaled to the transposed size first: {line}");
        // And the unturned opener is untouched.
        let _ = std::fs::remove_file(&args);
        let mut plain = shell_opener(launcher.display().to_string(), 4, 2, KillSwitch::new());
        let mut s2 = plain(&cand).expect("opens");
        s2.read_frame(&mut buf).expect("one frame");
        let line = std::fs::read_to_string(&args).unwrap();
        assert!(!line.contains("videoflip") && line.contains("width=4,height=2"), "{line}");
    }

    #[test]
    fn a_camera_that_flaps_is_given_up_on_rather_than_hammered() {
        let shared = Shared::new();
        let stop = AtomicBool::new(false);
        let plan = vec![cand("flapping")];
        let mut opens = 0u32;
        run(&plan, 2, 2, |_| { opens += 1; Ok(Box::new(Scripted { frames: 1, death: "died again", fill: 3 }) as Box<dyn FrameStream>) },
            &shared, &stop, Duration::ZERO);
        assert_eq!(opens, MAX_RESTARTS);
    }

    #[test]
    fn the_test_pattern_fills_a_frame_and_changes_over_time() {
        let mut p = TestPattern::new(16, 8);
        p.pace = Duration::ZERO;
        let mut a = vec![0u8; 16 * 8 * 4];
        let mut b = vec![0u8; 16 * 8 * 4];
        p.read_frame(&mut a).unwrap();
        for _ in 0..20 { p.read_frame(&mut b).unwrap(); }
        assert!(a.chunks(4).all(|px| px[3] == 255), "opaque");
        assert_ne!(a, b, "the bar moved");
        assert!(p.read_frame(&mut [0u8; 8]).is_err(), "a short buffer is refused");
    }

    /// The real thing, when the machine has it: `gst-launch-1.0` with a
    /// `videotestsrc`, through the same opener, plan shape and pipe framing
    /// a camera uses. Skipped (loudly) where GStreamer is not installed —
    /// the stand-in launcher above covers the mechanics there, and this
    /// covers gst's own CLI, caps negotiation and fdsink, which a stand-in
    /// cannot vouch for.
    #[test]
    fn real_gstreamer_delivers_frames_through_the_fdsink_path() {
        let has_gst = std::process::Command::new("gst-launch-1.0").arg("--version")
            .stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null())
            .status().map(|s| s.success()).unwrap_or(false);
        if !has_gst {
            eprintln!("SKIPPED: gst-launch-1.0 is not on this machine; the subprocess protocol is covered by the stand-in test");
            return;
        }
        let plan = device_kit::source_plan::plan(&[], "", Some("gst:videotestsrc pattern=smpte"), None);
        let mut rig = Rig::start(plan, 64, 48);
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut frame = None;
        while frame.is_none() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(50));
            frame = rig.shared.peek_frame();
        }
        let f = frame.expect("a frame from real gst-launch within 10s");
        assert_eq!((f.width, f.height), (64, 48));
        assert!(f.pixels_plausible());
        // SMPTE bars are not one colour: the pipe carried real pixels, not
        // zeros from a pipeline that negotiated and produced nothing.
        let first = &f.rgba[..4];
        assert!(f.rgba.chunks(4).any(|px| px != first), "every pixel identical — no picture came through");
        assert!(f.rgba.chunks(4).all(|px| px[3] == 255), "RGBA caps: alpha is opaque");
        assert!(matches!(rig.shared.take_status(), Some(Status::Live { .. })));
        rig.stop();
        assert!(!rig.is_running(), "the kill switch ended a blocked read");
    }

    #[test]
    fn a_rig_runs_the_pattern_on_its_thread_and_stops_on_both_halves() {
        let mut rig = Rig::start_test_pattern(8, 4);
        let deadline = Instant::now() + Duration::from_secs(5);
        while rig.shared.peek_frame().is_none() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        let f = rig.shared.peek_frame().expect("a frame from the rig's thread");
        assert!(f.pixels_plausible());
        assert!(rig.is_running());
        rig.stop();
        assert!(!rig.is_running());
    }
}

#[cfg(test)]
mod crop_tests {
    use super::*;

    fn frame(w: u32, h: u32) -> RawFrame {
        // Pixel (x, y) is (x, y, 7, 255): the crop can be checked by reading it back.
        let mut px = Vec::with_capacity((w * h * 4) as usize);
        for y in 0..h { for x in 0..w { px.extend_from_slice(&[x as u8, y as u8, 7, 255]); } }
        RawFrame { width: w, height: h, captured_ms: 42, seq: 9, rgba: px.into() }
    }

    #[test]
    fn a_crop_is_the_windows_pixels_at_native_size() {
        let f = frame(40, 20);
        let c = f.crop([0.25, 0.25, 0.75, 0.75]);
        assert_eq!((c.width, c.height), (20, 10));
        assert!(c.pixels_plausible());
        assert_eq!(&c.rgba[..4], &[10, 5, 7, 255], "top-left of the crop is (10, 5) of the frame");
        let last = c.rgba.len() - 4;
        assert_eq!(&c.rgba[last..], &[29, 14, 7, 255], "bottom-right is (29, 14)");
        assert_eq!((c.captured_ms, c.seq), (42, 9), "the same moment");
    }

    #[test]
    fn the_whole_frame_is_returned_as_is_and_edges_are_clamped() {
        let f = frame(40, 20);
        let same = f.crop([0.0, 0.0, 1.0, 1.0]);
        assert!(Arc::ptr_eq(&same.rgba, &f.rgba), "no copy for 1×");
        let clamped = f.crop([-0.5, -0.5, 1.5, 1.5]);
        assert_eq!((clamped.width, clamped.height), (40, 20));
        // A degenerate window still yields at least one pixel, never a panic.
        let dot = f.crop([0.5, 0.5, 0.5, 0.5]);
        assert_eq!((dot.width, dot.height), (1, 1));
        assert!(dot.pixels_plausible());
        let inverted = f.crop([0.9, 0.9, 0.1, 0.1]);
        assert!(inverted.pixels_plausible());
    }

    #[test]
    fn a_torn_frame_is_not_cropped() {
        let torn = RawFrame { width: 4, height: 4, captured_ms: 0, seq: 0, rgba: vec![0u8; 7].into() };
        let c = torn.crop([0.0, 0.0, 0.5, 0.5]);
        assert_eq!(c, torn);
    }
}
