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
        CameraChoice { name, capture }
    }
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
        v.push(format!("video/x-raw,width={cw},height={ch}"));
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
        assert_eq!(c, CameraChoice { name: Some(id.into()), capture: Some((1280, 960)) });
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
