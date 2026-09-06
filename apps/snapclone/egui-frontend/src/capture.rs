//! capture — the thing that finally produces a frame.
//!
//! Everything around this existed and was tested: `camera_source` decides
//! WHICH device and HOW to open it (an ordered list of gst pipeline strings),
//! `frame_pump` owns the latest-wins slot and the upload gating, the
//! viewfinder draws a texture when given one, and `HostEvent::CameraFrame` /
//! `CameraWorked` were waiting in the seam. What did not exist was anything
//! that RAN a pipeline — C12-20a — so no frame had ever been captured and
//! the viewfinder was a labelled rectangle on every machine.
//!
//! ## A subprocess, not a linked GStreamer
//!
//! The pipeline runs as `gst-launch-1.0` with the plan's `appsink` tail
//! retargeted to `fdsink fd=1` ([`device_kit::source_plan::to_fdsink`]), and
//! the host reads fixed-size RGBA frames off the child's stdout. Three
//! reasons, in order of weight:
//!
//! 1. **camera-bridge's rule.** Its Cargo.toml states it for this app:
//!    capture stays in the host, because a UI crate that opens `/dev/video0`
//!    cannot be tested. Linking `gstreamer-rs` here would put gstreamer in
//!    this crate's tree for every consumer, test runs included.
//! 2. **The loop stays visible.** The exact pipeline being tried is a string
//!    you can read in the status line and paste into a terminal. When it
//!    fails on a phone, the child's stderr says why in GStreamer's own
//!    words, and steering it is editing a string in settings — not
//!    recompiling.
//! 3. **ARM cost.** `gstreamer-rs` wants matching system headers per target;
//!    a subprocess wants `gst-launch-1.0` on PATH at runtime, which
//!    postmarketOS ships. Cross-building this crate stays exactly as cheap
//!    as it was.
//!
//! The price is a pipe copy per frame. At the forced 640×480 RGBA that is
//! ~1.2 MB/frame — ~36 MB/s at 30 fps — which a phone's page cache does
//! without warming. Raw video over a pipe has no in-band framing, so the
//! forced size IS the framing: every frame is exactly `w * h * 4` bytes.
//!
//! ## The seam
//!
//! [`run`] is a synchronous loop: it walks the plan, asks an injected opener
//! for a [`FrameStream`] per candidate, and pushes frames into [`Shared`].
//! The host wraps it in a thread; the tests call it directly on the test
//! thread with scripted fakes and it terminates because the fakes end. The
//! only real implementations — [`gst_opener`] and its child-stdout stream —
//! are construct-and-inject, same shape as aperture-engine's `Backend`.
//!
//! ## The stop contract
//!
//! `read_exact` on a pipe blocks, so a stop flag alone cannot interrupt a
//! stream that is mid-frame. Stopping is therefore TWO acts: set the flag,
//! then make the current stream end — which [`KillSwitch::stop`] does by
//! killing the child, turning the blocked read into an EOF the loop notices.
//! A host that only sets the flag will stop *eventually* (next stream end);
//! a host that only kills the child will restart the pipeline. Do both;
//! `CaptureRig` in the binary does.
//!
//! ## What is verified here and what is not
//!
//! The plan walk, retry policy, status transitions, frame framing, torn-frame
//! refusal and stop behaviour are unit tested against fakes. The
//! `gst-launch-1.0` path is NOT — this sandbox has no GStreamer and no
//! camera. `gst_opener` is compile-checked and its pipeline strings are the
//! ones `source_plan`'s tests pin, but no real frame has flowed through it
//! yet. Say so rather than implying a green test covered it.

use crate::frame_pump::RawFrame;
use std::io::Read;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub use device_kit::source_plan::SourceCandidate;

/// Where the capture engine currently stands, for the camera page's status
/// line. One value, replaced whole — the UI shows the latest, not a history.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    /// No engine is running (page not visible, or no camera chosen).
    Idle,
    /// A candidate pipeline is being opened or has not yet produced a frame.
    Trying { label: String },
    /// Frames are flowing.
    Live { label: String },
    /// Every candidate was tried and none ever produced a frame. The log has
    /// one line per attempt, in order, in GStreamer's own words where it had
    /// any — this is the text the user can act on.
    Exhausted { log: Vec<String> },
}

impl Status {
    /// The one-line rendering for the camera page. `None` when there is
    /// nothing worth saying over the picture (Live) or under the existing
    /// "no camera found" (Idle).
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
}

/// What the reader thread and the UI thread share. Latest-wins for the frame
/// (same argument as `frame_pump::FrameSlot`: a queue turns a stall into a
/// permanent lag), plus the status and a one-shot "it worked" edge for
/// `HostEvent::CameraWorked`.
#[derive(Default)]
pub struct Shared {
    latest: Mutex<Option<RawFrame>>,
    status: Mutex<Option<Status>>,
    worked: AtomicBool,
}

impl Shared {
    pub fn new() -> Arc<Shared> {
        Arc::new(Shared::default())
    }

    /// The newest frame, if one arrived since the last take.
    pub fn take_frame(&self) -> Option<RawFrame> {
        self.latest.lock().ok()?.take()
    }

    /// A copy of the newest frame WITHOUT consuming it — the shutter wants
    /// "what the viewfinder shows right now", and consuming it here would
    /// steal a preview frame to take a photo.
    pub fn peek_frame(&self) -> Option<RawFrame> {
        self.latest.lock().ok()?.clone()
    }

    /// The status, if it changed since the last take. Edge-triggered so the
    /// host does not push an identical event through `apply` every frame.
    pub fn take_status(&self) -> Option<Status> {
        self.status.lock().ok()?.take()
    }

    /// One-shot: did a first frame arrive since the last ask? Feeds the
    /// camera-health record, which wants the fact once, not per frame.
    pub fn take_worked(&self) -> bool {
        self.worked.swap(false, Ordering::Relaxed)
    }

    fn offer(&self, f: RawFrame) {
        if let Ok(mut g) = self.latest.lock() {
            *g = Some(f);
        }
    }

    fn set_status(&self, s: Status) {
        if let Ok(mut g) = self.status.lock() {
            *g = Some(s);
        }
    }
}

/// A running pipeline, reduced to the only thing the loop needs: fill this
/// frame-sized buffer, or say why you no longer can.
pub trait FrameStream: Send {
    /// Fill `buf` with exactly one frame. `Err` is the stream's obituary —
    /// EOF, a killed child, a device unplugged — and ends this candidate.
    fn read_frame(&mut self, buf: &mut [u8]) -> Result<(), String>;
}

/// A synthetic camera: a drifting colour gradient with a moving bar, at a
/// paced ~15 fps, forever. For `SNAPCLONE_TEST_PATTERN=1` — walking and
/// screenshotting the capture → compose → choose → send sequence on a machine
/// with no camera and no GStreamer (the sandbox that drew this app's first
/// screenshots). Not a test fake: the real capture thread, frame pump, texture
/// upload, shutter and encoder all run; only the pixels come from arithmetic.
pub struct TestPattern {
    width: u32,
    height: u32,
    frame: u64,
}

impl TestPattern {
    pub fn new(width: u32, height: u32) -> TestPattern {
        TestPattern { width, height, frame: 0 }
    }
}

impl FrameStream for TestPattern {
    fn read_frame(&mut self, buf: &mut [u8]) -> Result<(), String> {
        let (w, h) = (self.width as usize, self.height as usize);
        if buf.len() < w * h * 4 {
            return Err("test pattern: buffer smaller than a frame".into());
        }
        let t = self.frame as f32 * 0.05;
        let bar = ((t.sin() * 0.5 + 0.5) * w as f32) as usize;
        for y in 0..h {
            for x in 0..w {
                let i = (y * w + x) * 4;
                let fx = x as f32 / w as f32;
                let fy = y as f32 / h as f32;
                let (r, g, b) = if x.abs_diff(bar) < w / 40 {
                    (250, 240, 200)
                } else {
                    (
                        (40.0 + 160.0 * fx) as u8,
                        (60.0 + 120.0 * fy) as u8,
                        (120.0 + 100.0 * (1.0 - fx) * (1.0 - fy)) as u8,
                    )
                };
                buf[i] = r;
                buf[i + 1] = g;
                buf[i + 2] = b;
                buf[i + 3] = 255;
            }
        }
        self.frame += 1;
        std::thread::sleep(std::time::Duration::from_millis(66));
        Ok(())
    }
}

/// Walk the plan until stopped.
///
/// Synchronous on purpose: the tests drive it directly with fakes and it
/// terminates when they do. The host owns the thread, not this function.
///
/// Retry policy, chosen over alternatives rather than defaulted into:
/// * A candidate that fails to OPEN, or opens and never yields a frame, is
///   skipped — on to the next.
/// * A full pass where NOTHING ever produced a frame ends in `Exhausted`.
///   Hammering a set of pipelines that never worked will not make one work,
///   and the spin would warm a phone for nothing.
/// * A stream that dies AFTER producing frames restarts the whole plan
///   (after `retry_pause`) — the camera that worked may come back, and the
///   device list order still encodes preference.
/// * That restart is BOUNDED by [`MAX_RESTARTS`]. It was bounded only by the
///   stop flag, which is the host saying the page went away — and a camera
///   that delivers one frame and dies, over and over, is the shape of a
///   failing USB link or an ISP another app keeps taking. Unbounded, that is
///   a process-spawn loop for as long as the page is open, on a phone. After
///   the bound the run ends `Exhausted` with the reasons, and the person can
///   leave the page and come back to try again — a deliberate act, rather
///   than the app deciding to keep hammering the hardware on its own.
/// How many times a working-then-dying camera is re-opened before the run
/// gives up. Small on purpose: three failures in a row is not a transient
/// glitch, and the cost of being wrong is one page revisit.
pub const MAX_RESTARTS: u32 = 3;

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
            if stop.load(Ordering::Relaxed) {
                shared.set_status(Status::Idle);
                return;
            }
            shared.set_status(Status::Trying { label: cand.label.clone() });
            let mut stream = match open(cand) {
                Ok(s) => s,
                Err(e) => {
                    log.push(format!("{}: {e}", cand.label));
                    continue;
                }
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
                            width,
                            height,
                            captured_ms: t0.elapsed().as_millis() as u64,
                            seq,
                            rgba: Arc::from(&buf[..]),
                        });
                    }
                    Err(e) => break e,
                }
                if stop.load(Ordering::Relaxed) {
                    shared.set_status(Status::Idle);
                    return;
                }
            };
            log.push(format!("{}: {died} (after {frames_here} frames)", cand.label));
            if frames_here > 0 {
                any_frames_this_pass = true;
                // A working camera died mid-stream: restart the whole plan
                // rather than sliding to a worse candidate — the order
                // encodes preference, and the death may be transient.
                break;
            }
        }
        if stop.load(Ordering::Relaxed) {
            shared.set_status(Status::Idle);
            return;
        }
        if !any_frames_this_pass {
            shared.set_status(Status::Exhausted { log });
            return;
        }
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

/// The handle that makes a blocked reader stoppable: kill the child, and the
/// `read_exact` it was sitting in returns EOF. See the module doc's stop
/// contract — this is the second half, the flag is the first.
#[derive(Clone, Default)]
pub struct KillSwitch(Arc<Mutex<Option<std::process::Child>>>);

impl KillSwitch {
    pub fn new() -> KillSwitch {
        KillSwitch::default()
    }

    /// Kill whatever child is currently running. The stream reaps it — this
    /// only closes the pipe out from under the blocked read.
    pub fn stop(&self) {
        if let Ok(mut g) = self.0.lock() {
            if let Some(c) = g.as_mut() {
                let _ = c.kill();
            }
        }
    }

    fn install(&self, c: std::process::Child) {
        if let Ok(mut g) = self.0.lock() {
            // A previous child still parked here means the caller opened a
            // new candidate without reaping the old one; kill it rather than
            // leak a pipeline holding the camera.
            if let Some(old) = g.as_mut() {
                let _ = old.kill();
                let _ = old.wait();
            }
            *g = Some(c);
        }
    }

    fn reap(&self) -> Option<(std::process::ExitStatus, String)> {
        let mut c = self.0.lock().ok()?.take()?;
        let status = c.wait().ok()?;
        let mut err = String::new();
        if let Some(mut e) = c.stderr.take() {
            let _ = e.read_to_string(&mut err);
        }
        Some((status, err))
    }
}

/// The real opener: `sh -c "exec gst-launch-1.0 -q <pipeline>"`.
///
/// Through `sh` because the pipeline string uses gst-launch's own syntax —
/// `!` separators and a quoted `target-object="node"` — and shell splitting
/// is the quoting contract those strings were written against. `exec` so the
/// pid the [`KillSwitch`] kills is gst-launch itself, not a shell wrapping
/// it.
///
/// NOT RUN in any test: no GStreamer in the authoring sandbox. The strings
/// it receives are pinned by `source_plan`'s tests; the subprocess mechanics
/// are reasoned about, not proven. stderr is read only after exit — gst
/// under `-q` writes little enough that filling the 64 KB pipe buffer first
/// (which would deadlock the child against our stdout read) is not a
/// realistic failure, but it is a known edge rather than an impossible one.
pub fn gst_opener(
    width: u32,
    height: u32,
    kill: KillSwitch,
) -> impl FnMut(&SourceCandidate) -> Result<Box<dyn FrameStream>, String> {
    shell_opener("gst-launch-1.0".to_string(), width, height, kill)
}

/// [`gst_opener`] with the launcher program injectable — which is what lets
/// the subprocess mechanics (spawn, fixed-size framing off the pipe, the
/// EOF-to-stderr obituary, the kill) be VERIFIED in a sandbox that has no
/// GStreamer, against a stand-in that speaks the same protocol: bytes on
/// stdout, words on stderr. The gst-launch flag syntax itself remains
/// unverified here; the stand-in cannot vouch for another program's CLI.
fn shell_opener(
    launcher: String,
    width: u32,
    height: u32,
    kill: KillSwitch,
) -> impl FnMut(&SourceCandidate) -> Result<Box<dyn FrameStream>, String> {
    move |cand| {
        let fd = device_kit::source_plan::to_fdsink(&cand.pipeline, width, height)
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
                // reason, because "unexpected EOF" tells the user nothing and
                // "no such element pipewiresrc" tells them everything.
                let why = match self.kill.reap() {
                    Some((status, err)) if !err.trim().is_empty() => {
                        format!("{status}: {}", err.trim().lines().last().unwrap_or(""))
                    }
                    Some((status, _)) => format!("{status}"),
                    None => format!("{e}"),
                };
                Err(why)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    fn cand(label: &str) -> SourceCandidate {
        // Built by the real builder, not a literal, so these tests exercise
        // the same strings the runtime will.
        SourceCandidate {
            kind: device_kit::source_plan::SourceKind::V4l2,
            label: label.to_string(),
            pipeline: device_kit::source_plan::v4l2_pipeline("/dev/video0", None),
        }
    }

    /// A stream that yields `frames` filled frames and then dies of `death`.
    struct Scripted {
        frames: u64,
        death: &'static str,
        fill: u8,
    }
    impl FrameStream for Scripted {
        fn read_frame(&mut self, buf: &mut [u8]) -> Result<(), String> {
            if self.frames == 0 {
                return Err(self.death.to_string());
            }
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
            Ok(Box::new(Scripted { frames: 3, death: "unplugged", fill: 7 })
                as Box<dyn FrameStream>),
            // The restart pass after "unplugged": now nothing works, so the
            // run ends instead of spinning.
            Err("no such element".to_string()),
            Err("gone too".to_string()),
        ]);
        run(&plan, 4, 2, |_| opens.pop_front().expect("opened more than scripted"),
            &shared, &stop, Duration::ZERO);

        let f = shared.take_frame().expect("a frame reached the slot");
        assert_eq!((f.width, f.height), (4, 2));
        assert_eq!(f.seq, 3, "latest wins: the slot holds the last frame");
        assert!(f.pixels_plausible(), "{} bytes for 4x2", f.rgba.len());
        assert_eq!(f.rgba[0], 7);
        assert!(shared.take_worked(), "the health record hears that capture worked");
        assert!(!shared.take_worked(), "…exactly once");
    }

    #[test]
    fn a_plan_that_never_produces_a_frame_exhausts_with_the_reasons() {
        let shared = Shared::new();
        let stop = AtomicBool::new(false);
        let plan = vec![cand("a"), cand("b")];
        run(&plan, 2, 2,
            |c| -> Result<Box<dyn FrameStream>, String> {
                Err(format!("cannot open {}", c.label))
            },
            &shared, &stop, Duration::ZERO);
        match shared.take_status() {
            Some(Status::Exhausted { log }) => {
                assert_eq!(log.len(), 2, "{log:?}");
                assert!(log[0].contains("cannot open a"), "{log:?}");
                // The status line carries the LAST reason — the user reads
                // one line, and the freshest failure is the actionable one.
                let line = Status::Exhausted { log }.line().unwrap();
                assert!(line.contains("cannot open b"), "{line}");
            }
            other => panic!("expected Exhausted, got {other:?}"),
        }
        assert!(!shared.take_worked(), "nothing worked and nothing claims it did");
    }

    #[test]
    fn a_stream_that_opens_but_never_frames_is_a_failure_not_a_livelock() {
        // The pipeline that negotiates and then produces nothing is the
        // black-preview case in its rawest form; it must count as a failed
        // candidate, not as success.
        let shared = Shared::new();
        let stop = AtomicBool::new(false);
        let plan = vec![cand("black")];
        run(&plan, 2, 2,
            |_| Ok(Box::new(Scripted { frames: 0, death: "EOS before any buffer", fill: 0 })
                as Box<dyn FrameStream>),
            &shared, &stop, Duration::ZERO);
        match shared.take_status() {
            Some(Status::Exhausted { log }) => {
                assert!(log[0].contains("after 0 frames"), "{log:?}");
            }
            other => panic!("expected Exhausted, got {other:?}"),
        }
    }

    #[test]
    fn stop_between_frames_ends_the_run_as_idle() {
        // The flag half of the stop contract: a stream keeps yielding, the
        // flag goes up, the loop leaves at the next frame boundary and says
        // Idle rather than Exhausted — stopping is not a failure.
        // Owns its handle: the opener returns `Box<dyn FrameStream>`, which
        // is `'static`, so the fake cannot borrow the test's flag.
        struct StopAfter {
            left: u64,
            stop: Arc<AtomicBool>,
        }
        impl FrameStream for StopAfter {
            fn read_frame(&mut self, buf: &mut [u8]) -> Result<(), String> {
                if self.left == 0 {
                    return Err("should have stopped already".into());
                }
                self.left -= 1;
                if self.left == 0 {
                    self.stop.store(true, Ordering::Relaxed);
                }
                buf.fill(1);
                Ok(())
            }
        }
        let plan = vec![cand("live")];

        // A flag raised before the first open is an immediate Idle, with no
        // opener ever consulted.
        let pre = Shared::new();
        run(&plan, 2, 2,
            |_| -> Result<Box<dyn FrameStream>, String> {
                panic!("the opener must not run under a raised flag")
            },
            &pre, &AtomicBool::new(true), Duration::ZERO);
        assert_eq!(pre.take_status(), Some(Status::Idle));

        let stop = Arc::new(AtomicBool::new(false));
        let shared = Shared::new();
        run(&plan, 2, 2,
            |_| Ok(Box::new(StopAfter { left: 2, stop: stop.clone() }) as Box<dyn FrameStream>),
            &shared, &stop, Duration::ZERO);
        assert_eq!(shared.take_status(), Some(Status::Idle));
        assert_eq!(shared.take_frame().unwrap().seq, 2, "both frames were delivered first");
    }

    /// Write a stand-in launcher script and return its path. It ignores the
    /// `-q <pipeline>` arguments the opener passes — the point is the pipe
    /// protocol, not gst's CLI.
    #[cfg(unix)]
    fn fake_launcher(name: &str, body: &str) -> std::path::PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!(
            "snapclone-capture-test-{}-{}", std::process::id(), name));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("launcher");
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    #[test]
    #[cfg(unix)]
    fn a_real_subprocess_delivers_framed_bytes_and_a_stderr_obituary() {
        // The half `gst_opener` shares with the stand-in: spawn through sh,
        // slice the pipe into fixed-size frames, and when it ends, read the
        // reason off stderr instead of reporting "unexpected EOF".
        //
        // 4x2 RGBA = 32 bytes/frame. The script writes 2.5 frames — the
        // torn half must not surface as a frame — then names its exit.
        // The stand-in succeeds every round, which is what a flapping camera
        // looks like: opens, delivers, dies. MAX_RESTARTS is what makes this
        // terminate — before it existed this test needed a sentinel file to
        // stop the loop, and the sentinel was hiding the real defect.
        let launcher = fake_launcher("frames",
            "head -c 80 /dev/zero | tr '\\0' 'A'; echo 'lens fell off' >&2; exit 3");
        let shared = Shared::new();
        let stop = AtomicBool::new(false);
        let plan = vec![cand("stand-in")];
        run(&plan, 4, 2,
            shell_opener(launcher.display().to_string(), 4, 2, KillSwitch::new()),
            &shared, &stop, Duration::ZERO);

        let f = shared.take_frame().expect("two whole frames crossed the pipe");
        // Two whole frames per round, and `seq` counts across restarts —
        // deliberately, since gap detection needs it monotonic for the life
        // of the run rather than per pipeline. Anything but an exact
        // multiple of two means the torn 16 bytes were delivered as a frame.
        assert_eq!(f.seq, 2 * u64::from(MAX_RESTARTS),
                   "the torn third frame of some round was counted");
        assert!(f.pixels_plausible());
        assert!(f.rgba.iter().all(|b| *b == b'A'));
        match shared.take_status() {
            Some(Status::Exhausted { log }) => {
                assert!(log.iter().any(|l| l.contains("lens fell off")),
                        "stderr did not become the reason: {log:?}");
                assert!(log.iter().any(|l| l.contains("after 2 frames")),
                        "the torn third frame must not count: {log:?}");
                assert!(log.last().unwrap().contains("gave up after"),
                        "the give-up is not explained: {log:?}");
            }
            other => panic!("expected Exhausted after the restart bound, got {other:?}"),
        }
    }

    #[test]
    fn a_camera_that_flaps_is_given_up_on_rather_than_hammered() {
        // Opens, delivers one frame, dies — the shape of a failing USB link
        // or an ISP another app keeps taking. Unbounded this is a
        // process-spawn loop for as long as the page is open.
        let shared = Shared::new();
        let stop = AtomicBool::new(false);
        let plan = vec![cand("flapping")];
        let mut opens = 0u32;
        run(&plan, 2, 2,
            |_| {
                opens += 1;
                Ok(Box::new(Scripted { frames: 1, death: "died again", fill: 3 })
                    as Box<dyn FrameStream>)
            },
            &shared, &stop, Duration::ZERO);
        assert_eq!(opens, MAX_RESTARTS,
                   "the plan was re-opened {opens} times against a bound of {MAX_RESTARTS}");
        assert!(matches!(shared.take_status(), Some(Status::Exhausted { .. })));
    }

    #[test]
    #[cfg(unix)]
    fn the_kill_switch_unblocks_a_reader_parked_on_a_live_pipe() {
        // The other half of the stop contract, against a child that would
        // stream forever: `exec dd` so the pid the switch kills IS the
        // producer, and killing it turns the blocked read_exact into EOF.
        let launcher = fake_launcher("forever", "exec dd if=/dev/zero bs=64k 2>/dev/null");
        let shared = Shared::new();
        let stop = Arc::new(AtomicBool::new(false));
        let kill = KillSwitch::new();
        let plan = vec![cand("endless")];
        let (s2, st2, k2) = (shared.clone(), stop.clone(), kill.clone());
        let cmd = launcher.display().to_string();
        let t = std::thread::spawn(move || {
            run(&plan, 4, 2, shell_opener(cmd, 4, 2, k2), &s2, &st2, Duration::ZERO);
        });
        // Wait for frames to prove the reader is live before pulling the plug.
        for _ in 0..200 {
            if shared.peek_frame().is_some() { break; }
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(shared.peek_frame().is_some(), "no frame from the endless child");
        stop.store(true, Ordering::Relaxed);
        kill.stop();
        t.join().expect("the run thread never came back — the read stayed blocked");
        // Raced against the reader: either the loop saw the flag first
        // (Idle) or the EOF first — both are a stop, neither may hang.
    }

    #[test]
    fn the_shutter_peeks_without_stealing_the_preview_frame() {
        let shared = Shared::new();
        shared.offer(RawFrame { width: 1, height: 1, captured_ms: 0, seq: 1,
                                rgba: Arc::from(&[0u8; 4][..]) });
        assert!(shared.peek_frame().is_some());
        assert!(shared.take_frame().is_some(),
                "the peek left the frame for the preview to take");
        assert!(shared.take_frame().is_none());
    }

    #[test]
    fn status_is_edge_triggered_so_the_host_does_not_spam_apply() {
        let shared = Shared::new();
        shared.set_status(Status::Trying { label: "x".into() });
        assert!(shared.take_status().is_some());
        assert!(shared.take_status().is_none(), "same status is not re-delivered");
    }
}
