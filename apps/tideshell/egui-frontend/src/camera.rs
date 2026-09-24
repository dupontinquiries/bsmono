//! Camera frames for the lock-screen pane. No GStreamer or libcamera is
//! linked: a child process (`gst-launch-1.0 ... ! fdsink`) writes raw RGBA
//! frames of a fixed size to its stdout, and a reader thread hands the
//! newest one over. The command is a parameter, so a test can point it at
//! any producer, and the phone can switch pipelines (libcamerasrc,
//! pipewiresrc, v4l2src) without a rebuild.
//!
//! The camera runs only while the shell wants it (`Shell::camera_wanted`):
//! `start` spawns, `stop` kills the child, and the pane shows an indicator
//! whenever a frame is live.

use std::io::Read;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};

pub const FRAME_W: usize = 240;
pub const FRAME_H: usize = 320;

#[derive(Debug, Clone, PartialEq)]
pub struct Frame {
    pub w: usize,
    pub h: usize,
    pub rgba: Vec<u8>,
}

/// Which libcamera camera the pane opens, and at what size it asks the
/// camera to deliver. Both optional: with no name `libcamerasrc` opens the
/// FIRST camera libcamera lists, which on the FP5 is the FRONT one; with no
/// capture size libcamera picks its default stream, which on the FP5 main
/// camera is 4096x3072 (and 8192x6144 on the front), all of it then
/// converted and scaled on the CPU for a small pane.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CameraChoice {
    /// A libcamera camera id, as `cam -l` prints it in parentheses.
    pub name: Option<String>,
    pub capture: Option<(usize, usize)>,
    /// Cap the rate at the SOURCE. Frames that would be dropped downstream
    /// are then never debayered at all, which is the expensive part when
    /// libcamera has no hardware ISP to hand the work to.
    pub fps: Option<u32>,
}

impl CameraChoice {
    /// `TIDESHELL_CAMERA` (a camera id) and `TIDESHELL_CAMERA_CAPTURE`
    /// (`WxH`), as the environment gives them. Pure: the caller passes the
    /// values in.
    pub fn from_vars(camera: Option<&str>, capture: Option<&str>) -> CameraChoice {
        let name = camera.map(str::trim).filter(|s| !s.is_empty()).map(String::from);
        let capture = capture.and_then(|c| {
            let (w, h) = c.trim().split_once(['x', 'X'])?;
            Some((w.parse().ok()?, h.parse().ok()?))
        });
        CameraChoice { name, capture, fps: None }
    }
}

/// How big a frame needs to be to TINT a lock screen: not very. The pane
/// blurs it past the point where pixels are visible, so capturing at pane
/// resolution spends CPU on detail that is thrown away — and on this phone
/// libcamera runs the `simple` pipeline handler, debayering every frame in
/// software, so the cost is close to linear in pixels.
pub const GLASS_W: usize = 32;
pub const GLASS_H: usize = 40;

/// Choose the rear camera from `cam -l` output, for when the environment
/// names none.
///
/// `CameraChoice` with no name opens whichever camera libcamera lists first,
/// which on the FP5 is the front one. A lock screen tinting itself from the
/// room wants the camera pointed AT the room, and a selfie camera aimed at a
/// face is both the wrong view and unsettling. Several rear cameras exist
/// here (main and ultra-wide); the first listed is taken, because the choice
/// must be STABLE across boots or the tint changes character for no visible
/// reason.
///
/// Lines look like:
/// `1: Internal back camera (/base/soc@0/cci@ac4a000/i2c-bus@0/camera@1a)`
pub fn pick_rear(cam_l_output: &str) -> Option<String> {
    let id_of = |l: &str| l.rsplit_once('(').and_then(|(_, r)| r.split_once(')')).map(|(id, _)| id.trim().to_string());
    cam_l_output
        .lines()
        .filter(|l| l.to_ascii_lowercase().contains("back camera"))
        .find_map(id_of)
        .or_else(|| cam_l_output.lines().find_map(id_of))
}

/// When the camera may run, so a lock-screen tint costs a sip rather than a
/// stream.
///
/// The tint tracks the room, and a room's light changes over minutes. So the
/// camera runs for a moment, yields a frame, and is shut off — which powers
/// the sensor and the ISP down rather than merely ignoring their output.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Duty {
    /// libcamera needs a moment to configure, allocate and let exposure
    /// settle; a frame grabbed before that is dark or green.
    pub warmup_ms: u64,
    pub period_ms: u64,
}

impl Default for Duty {
    fn default() -> Duty {
        Duty { warmup_ms: 1_200, period_ms: 30_000 }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DutyAction {
    Start,
    Stop,
    Wait,
}

#[derive(Debug, Clone, Default)]
pub struct DutyCycle {
    pub duty: Duty,
    running_since: Option<u64>,
    last_stop: Option<u64>,
    got_frame: bool,
}

impl DutyCycle {
    pub fn new(duty: Duty) -> DutyCycle {
        DutyCycle { duty, ..Default::default() }
    }
    pub fn running(&self) -> bool {
        self.running_since.is_some()
    }
    /// Note that a frame arrived; the window can end early.
    pub fn frame(&mut self) {
        self.got_frame = true;
    }
    /// `wanted` is whether the pane is visible at all; when it goes false the
    /// camera stops at once rather than finishing its window.
    pub fn tick(&mut self, wanted: bool, now_ms: u64) -> DutyAction {
        if !wanted {
            if self.running_since.take().is_some() {
                self.last_stop = Some(now_ms);
                self.got_frame = false;
                return DutyAction::Stop;
            }
            return DutyAction::Wait;
        }
        match self.running_since {
            Some(started) => {
                if self.got_frame || now_ms.saturating_sub(started) >= self.duty.warmup_ms {
                    self.running_since = None;
                    self.last_stop = Some(now_ms);
                    self.got_frame = false;
                    DutyAction::Stop
                } else {
                    DutyAction::Wait
                }
            }
            None => match self.last_stop {
                Some(t) if now_ms.saturating_sub(t) < self.duty.period_ms => DutyAction::Wait,
                _ => {
                    self.running_since = Some(now_ms);
                    self.got_frame = false;
                    DutyAction::Start
                }
            },
        }
    }
}

/// Reduce a frame to the average colour of a coarse grid — thick frosted
/// glass rather than a picture. Cheap on an already tiny frame, and it means
/// no recognisable image of the room ever reaches a LOCK screen.
pub fn glass(frame: &Frame, cols: usize, rows: usize) -> Vec<[u8; 3]> {
    let (cols, rows) = (cols.max(1), rows.max(1));
    let mut out = Vec::with_capacity(cols * rows);
    for gy in 0..rows {
        for gx in 0..cols {
            let (x0, x1) = (gx * frame.w / cols, ((gx + 1) * frame.w / cols).max(gx * frame.w / cols + 1));
            let (y0, y1) = (gy * frame.h / rows, ((gy + 1) * frame.h / rows).max(gy * frame.h / rows + 1));
            let (mut r, mut g, mut b, mut n) = (0u64, 0u64, 0u64, 0u64);
            for y in y0..y1.min(frame.h) {
                for x in x0..x1.min(frame.w) {
                    let i = (y * frame.w + x) * 4;
                    if i + 2 < frame.rgba.len() {
                        r += frame.rgba[i] as u64;
                        g += frame.rgba[i + 1] as u64;
                        b += frame.rgba[i + 2] as u64;
                        n += 1;
                    }
                }
            }
            let n = n.max(1);
            out.push([(r / n) as u8, (g / n) as u8, (b / n) as u8]);
        }
    }
    out
}

/// The pipeline the phone runs: libcamera through GStreamer, scaled to the
/// pane's size, RGBA on stdout. `videoflip` is not applied: the pane
/// rotates with the shell.
pub fn gst_command(choice: &CameraChoice, w: usize, h: usize) -> Vec<String> {
    let mut v: Vec<String> = vec!["gst-launch-1.0".into(), "-q".into(), "libcamerasrc".into()];
    if let Some(n) = &choice.name {
        v.push(format!("camera-name={n}"));
    }
    if let Some((cw, ch)) = choice.capture {
        v.push("!".into());
        v.push(match choice.fps {
            Some(f) => format!("video/x-raw,width={cw},height={ch},framerate={f}/1"),
            None => format!("video/x-raw,width={cw},height={ch}"),
        });
    } else if let Some(f) = choice.fps {
        v.push("!".into());
        v.push(format!("video/x-raw,framerate={f}/1"));
    }
    if choice.fps.is_some() {
        v.push("!".into());
        v.push("videorate".into());
    }
    for a in ["!", "videoconvert", "!", "videoscale", "!"] {
        v.push(a.into());
    }
    v.push(format!("video/x-raw,format=RGBA,width={w},height={h}"));
    for a in ["!", "fdsink", "fd=1", "sync=false"] {
        v.push(a.into());
    }
    v
}

pub struct ChildSource {
    child: Option<Child>,
    rx: Option<Receiver<Frame>>,
    pub status: String,
    pub frames: u64,
}

impl ChildSource {
    /// Start `argv`; frames of `w`x`h` RGBA arrive on `poll`.
    pub fn start(argv: &[String], w: usize, h: usize) -> ChildSource {
        let mut cmd = Command::new(&argv[0]);
        cmd.args(&argv[1..]).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null());
        match cmd.spawn() {
            Ok(mut child) => {
                let stdout = child.stdout.take().expect("piped stdout");
                let (tx, rx) = mpsc::channel();
                std::thread::Builder::new().name("tideshell-camera".into()).spawn(move || reader(stdout, w, h, tx)).expect("spawn camera reader");
                ChildSource { child: Some(child), rx: Some(rx), status: format!("camera: started {}", argv[0]), frames: 0 }
            }
            Err(e) => ChildSource { child: None, rx: None, status: format!("camera: cannot start {}: {e}", argv[0]), frames: 0 },
        }
    }

    pub fn is_running(&self) -> bool {
        self.child.is_some()
    }

    /// The newest frame since the last poll (older ones are dropped).
    pub fn poll(&mut self) -> Option<Frame> {
        let rx = self.rx.as_ref()?;
        let mut newest = None;
        loop {
            match rx.try_recv() {
                Ok(f) => newest = Some(f),
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    self.status = format!("camera: producer ended after {} frame(s)", self.frames);
                    self.rx = None;
                    if let Some(mut c) = self.child.take() {
                        let _ = c.wait();
                    }
                    break;
                }
            }
        }
        if newest.is_some() {
            self.frames += 1;
        }
        newest
    }

    pub fn stop(&mut self) {
        if let Some(mut c) = self.child.take() {
            let _ = c.kill();
            let _ = c.wait();
            self.status = format!("camera: stopped after {} frame(s)", self.frames);
        }
        self.rx = None;
    }
}

impl Drop for ChildSource {
    fn drop(&mut self) {
        self.stop();
    }
}

fn reader(mut stdout: std::process::ChildStdout, w: usize, h: usize, tx: Sender<Frame>) {
    let size = w * h * 4;
    let mut buf = vec![0u8; size];
    loop {
        let mut filled = 0;
        while filled < size {
            match stdout.read(&mut buf[filled..]) {
                Ok(0) | Err(_) => return,
                Ok(n) => filled += n,
            }
        }
        if tx.send(Frame { w, h, rgba: buf.clone() }).is_err() {
            return;
        }
    }
}

/// A moving pattern for the simulator: a grid that drifts with time, so
/// the tracker has something to follow.
pub fn synthetic_frame(w: usize, h: usize, t_secs: f32) -> Frame {
    let mut rgba = vec![0u8; w * h * 4];
    let (ox, oy) = ((t_secs * 18.0) as i32, (t_secs * 7.0) as i32);
    for y in 0..h {
        for x in 0..w {
            let (gx, gy) = ((x as i32 + ox).rem_euclid(48), (y as i32 + oy).rem_euclid(48));
            let line = gx < 3 || gy < 3;
            let blob = ((x as i32 + ox * 2).rem_euclid(160) - 80).pow(2) + ((y as i32 + oy).rem_euclid(200) - 100).pow(2) < 600;
            let (r, g, b) = if blob { (230, 180, 90) } else if line { (70, 110, 150) } else { (30, 40, 58) };
            let i = (y * w + x) * 4;
            rgba[i..i + 4].copy_from_slice(&[r, g, b, 255]);
        }
    }
    Frame { w, h, rgba }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_arrive_whole_and_the_newest_wins() {
        // A producer writing 3 tiny frames then exiting.
        let (w, h) = (4, 2);
        let script = format!("import sys,os\nfor k in range(3):\n    os.write(1, bytes([k]*{}))\n", w * h * 4);
        let argv = vec!["python3".to_string(), "-c".to_string(), script];
        let mut src = ChildSource::start(&argv, w, h);
        assert!(src.is_running(), "{}", src.status);
        let mut last = None;
        for _ in 0..100 {
            if let Some(f) = src.poll() {
                last = Some(f);
            }
            if !src.is_running() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        let f = last.expect("a frame");
        assert_eq!((f.w, f.h, f.rgba.len()), (w, h, w * h * 4));
        assert_eq!(f.rgba[0], 2, "the last frame written is the one kept");
        assert!(src.status.contains("producer ended"), "{}", src.status);
        src.stop();
    }

    #[test]
    fn a_missing_binary_is_reported_not_panicked() {
        let src = ChildSource::start(&["/nonexistent/gst-launch-1.0".to_string()], 4, 4);
        assert!(!src.is_running() && src.status.contains("cannot start"));
        let cmd = gst_command(&CameraChoice::default(), 240, 320);
        assert_eq!(cmd[0], "gst-launch-1.0");
        assert!(cmd.iter().any(|a| a.contains("format=RGBA,width=240,height=320")));
        assert!(!cmd.iter().any(|a| a.starts_with("camera-name=")), "no choice: libcamerasrc's own default");
    }

    #[test]
    fn the_camera_choice_reaches_the_pipeline_as_separate_arguments() {
        // The FP5's main rear camera, as `cam -l` printed it there.
        let id = "/base/soc@0/cci@ac4a000/i2c-bus@0/camera@1a";
        let c = CameraChoice::from_vars(Some(id), Some("1280x960"));
        assert_eq!(c, CameraChoice { name: Some(id.into()), capture: Some((1280, 960)), fps: None });
        let cmd = gst_command(&c, 240, 320);
        let at = |x: &str| cmd.iter().position(|a| a == x).unwrap_or_else(|| panic!("{x} missing: {cmd:?}"));
        assert_eq!(at("libcamerasrc") + 1, at(&format!("camera-name={id}")), "the property belongs to libcamerasrc");
        assert!(at("video/x-raw,width=1280,height=960") < at("videoconvert"), "the capture size is asked of the camera, before any CPU work");
        assert_eq!(CameraChoice::from_vars(Some("  "), Some("big")), CameraChoice::default(), "blank or malformed values mean no choice");
        assert_eq!(CameraChoice::from_vars(None, Some("640X480")).capture, Some((640, 480)));
    }

    #[test]
    fn synthetic_frames_are_textured_and_move() {
        let a = synthetic_frame(240, 320, 0.0);
        let b = synthetic_frame(240, 320, 1.0);
        assert_ne!(a.rgba, b.rgba);
        let fa = tideshell_core::track::Frame::from_rgba(240, 320, &a.rgba);
        assert!(tideshell_core::track::find_corners(&fa).len() > 10);
    }
}

#[cfg(test)]
mod glass_tests {
    use super::*;

    const CAM_L: &str = "\
Available cameras:
1: Internal back camera (/base/soc@0/cci@ac4a000/i2c-bus@0/camera@1a)
2: Internal back camera (/base/soc@0/cci@ac4a000/i2c-bus@1/camera@29)
3: Internal front camera (/base/soc@0/cci@ac4b000/i2c-bus@1/camera@10)";

    #[test]
    fn the_rear_camera_is_chosen_not_whichever_is_first() {
        let id = pick_rear(CAM_L).unwrap();
        assert!(id.ends_with("camera@1a"), "got {id}");
        assert!(!id.contains("cci@ac4b000"), "never the front one");
        assert_eq!(pick_rear(CAM_L), pick_rear(CAM_L), "stable across boots");
        // A front-only device still gets a pane rather than nothing.
        assert_eq!(pick_rear("1: Internal front camera (/base/x/camera@10)").unwrap(), "/base/x/camera@10");
        assert_eq!(pick_rear(""), None);
    }

    #[test]
    fn capping_the_rate_happens_at_the_source() {
        let c = CameraChoice { name: Some("/base/cam@1a".into()), capture: Some((640, 480)), fps: Some(1) };
        let line = gst_command(&c, GLASS_W, GLASS_H).join(" ");
        assert!(line.contains("camera-name=/base/cam@1a"));
        assert!(line.contains("width=640,height=480,framerate=1/1"), "on the capture caps, not only downstream: {line}");
        assert!(line.contains("videorate"));
        assert!(line.contains("width=32,height=40"));
        // No fps asked for: the pipeline is exactly as it was before.
        let plain = CameraChoice { name: None, capture: None, fps: None };
        let l2 = gst_command(&plain, 240, 320).join(" ");
        assert!(!l2.contains("framerate") && !l2.contains("videorate"));
    }

    #[test]
    fn the_camera_sips_rather_than_streams() {
        let mut d = DutyCycle::new(Duty { warmup_ms: 1_000, period_ms: 10_000 });
        assert_eq!(d.tick(true, 0), DutyAction::Start);
        assert_eq!(d.tick(true, 500), DutyAction::Wait, "still warming up");
        d.frame();
        assert_eq!(d.tick(true, 600), DutyAction::Stop, "a frame arrived, so stop early");
        assert_eq!(d.tick(true, 5_000), DutyAction::Wait, "not yet — 10 s period");
        assert_eq!(d.tick(true, 10_700), DutyAction::Start);
        // No frame ever: the warmup budget bounds it anyway.
        assert_eq!(d.tick(true, 11_200), DutyAction::Wait);
        assert_eq!(d.tick(true, 11_800), DutyAction::Stop);
    }

    #[test]
    fn hiding_the_pane_stops_the_camera_at_once() {
        let mut d = DutyCycle::new(Duty::default());
        assert_eq!(d.tick(true, 0), DutyAction::Start);
        assert_eq!(d.tick(false, 10), DutyAction::Stop, "now, not after the warmup");
        assert_eq!(d.tick(false, 20), DutyAction::Wait);
    }

    #[test]
    fn glass_keeps_the_colour_and_loses_the_picture() {
        let (w, h) = (8usize, 4usize);
        let mut rgba = vec![0u8; w * h * 4];
        for y in 0..h {
            for x in 0..w {
                let i = (y * w + x) * 4;
                if x < w / 2 { rgba[i] = 200 } else { rgba[i + 2] = 100 }
                rgba[i + 3] = 255;
            }
        }
        let f = Frame { w, h, rgba };
        assert_eq!(glass(&f, 2, 1), vec![[200, 0, 0], [0, 0, 100]]);
        assert_eq!(glass(&f, 1, 1), vec![[100, 0, 50]], "averaged, so no detail survives");
        assert_eq!(glass(&f, 0, 0).len(), 1, "degenerate asks do not panic");
        assert_eq!(glass(&Frame { w: 0, h: 0, rgba: vec![] }, 2, 2).len(), 4);
    }
}
