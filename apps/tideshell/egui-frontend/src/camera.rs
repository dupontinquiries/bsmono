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
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CameraChoice {
    /// A libcamera camera id, as `cam -l` prints it in parentheses.
    pub name: Option<String>,
    pub capture: Option<(usize, usize)>,
    /// Cap the rate at the SOURCE. Frames that would be dropped downstream
    /// are then never debayered at all, which is the expensive part when
    /// libcamera has no hardware ISP to hand the work to.
    pub fps: Option<u32>,
    /// Freeze the exposure. See [`Exposure`].
    pub exposure: Option<Exposure>,
}

/// A fixed exposure, so what the pane shows is how bright the room actually
/// is.
///
/// Auto-exposure is the enemy of this whole feature. Its job is to make
/// every scene look the same, which is exactly wrong for a lock screen
/// tinting itself from the room: with AE running, a dark room is brightened
/// until the wash looks like daylight, the tint stops meaning anything, and
/// the gain ramps produce slow drifts that read as the shell doing something
/// on its own. Locking exposure and gain makes a dark room look dark.
///
/// It also turns the camera into a crude light meter. With exposure and gain
/// fixed and known, the mean pixel level is comparable between readings and
/// between sessions ([`scene_ev`]) — which matters here because this phone
/// has no readable ambient light sensor at all.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Exposure {
    /// Microseconds. Long enough to see a dim room, short enough that a
    /// hand-held phone does not smear: this is a light meter, not a photo.
    pub time_us: u32,
    /// Analogue gain, 1.0 = none. Kept low; gain is noise, and noise
    /// averaged over a 4x5 grid becomes a false brightness.
    pub gain: f32,
}

impl Default for Exposure {
    fn default() -> Exposure {
        Exposure { time_us: 20_000, gain: 2.0 }
    }
}

/// A scene's brightness in stops, comparable across different fixed
/// exposures: doubling the exposure time or the gain is one stop, and a
/// doubling of measured level is one stop the other way.
///
/// Returns roughly "how much light was there", so two readings taken with
/// different settings can still be compared — and so a future auto-ranging
/// step could widen the exposure in the dark without the number jumping.
pub fn scene_ev(mean_level: f32, e: &Exposure) -> f32 {
    let level = mean_level.clamp(1.0, 255.0) / 255.0;
    let exposure_stops = (e.time_us.max(1) as f32 / 10_000.0).log2() + e.gain.max(0.01).log2();
    level.log2() - exposure_stops
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
        CameraChoice { name, capture, fps: None, exposure: None }
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
    /// The gap between windows when something is happening.
    pub period_ms: u64,
    /// The gap it backs off to when nothing is. Reached by doubling, so the
    /// first few quiet windows still sample often enough to notice a room
    /// coming back to life.
    pub idle_period_ms: u64,
    /// Quiet windows before the backoff starts. One unchanged frame is not
    /// evidence; it might be a wall.
    pub quiet_before_backoff: u32,
}

impl Default for Duty {
    fn default() -> Duty {
        Duty { warmup_ms: 1_200, period_ms: 30_000, idle_period_ms: 300_000, quiet_before_backoff: 2 }
    }
}

/// Did the scene change enough to be worth watching?
///
/// Compared on the [`glass`] grid rather than on pixels, because that is
/// what the pane shows: a change too small to alter the tint is, for this
/// purpose, not a change. `threshold` is mean absolute difference per
/// channel, 0..255.
pub fn scene_changed(a: &[[u8; 3]], b: &[[u8; 3]], threshold: f32) -> bool {
    if a.len() != b.len() || a.is_empty() {
        return true;
    }
    let total: u32 = a.iter().zip(b).map(|(p, q)| p.iter().zip(q).map(|(x, y)| x.abs_diff(*y) as u32).sum::<u32>()).sum();
    total as f32 / (a.len() * 3) as f32 > threshold
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
    /// Consecutive windows whose scene looked the same as the last.
    quiet_windows: u32,
    /// A live window: stream (not one frame) until this time. Opened when the
    /// cover appears on a lit screen, so the depth orbs have motion to track
    /// while someone is actually looking; closed on its own after `LIVE_MS`.
    live_until: Option<u64>,
    /// `go_live` interrupted a sample window: the next tick restarts at once.
    restart_live: bool,
}

/// How long the camera streams after the cover appears (see `go_live`).
pub const LIVE_MS: u64 = 8_000;
/// Frame rate while live. Enough for block matching to follow a hand's
/// motion on a 240x320 frame; the software debayer is paid per frame.
pub const LIVE_FPS: u32 = 8;
/// Frame rate for the one-frame tint samples.
pub const SAMPLE_FPS: u32 = 2;

impl DutyCycle {
    pub fn new(duty: Duty) -> DutyCycle {
        DutyCycle { duty, ..Default::default() }
    }
    pub fn running(&self) -> bool {
        self.running_since.is_some()
    }
    /// Note that a frame arrived; the window can end early. `changed` is
    /// whether the scene differs from the last one seen ([`scene_changed`]).
    ///
    /// A still scene is the only idle signal available on a phone whose
    /// accelerometer is not readable, and it is arguably the better one
    /// anyway: what matters for a tint is whether the ROOM changed, not
    /// whether the phone was picked up and put down again.
    pub fn frame(&mut self, changed: bool) {
        self.got_frame = true;
        if changed {
            self.quiet_windows = 0;
        } else {
            self.quiet_windows = self.quiet_windows.saturating_add(1);
        }
    }

    /// Something happened that is not the camera's business to detect — a
    /// touch, a wake, a notification. Sample promptly again.
    pub fn wake(&mut self) {
        self.quiet_windows = 0;
    }

    /// Stream for `ms` from `now` (the cover just appeared). A running
    /// one-frame window is extended rather than restarted.
    pub fn go_live(&mut self, now_ms: u64, ms: u64) {
        self.quiet_windows = 0;
        self.live_until = Some(now_ms + ms);
        // A sample window started at the old rate: restart it at the live rate.
        if self.running_since.is_some() {
            self.running_since = None;
            self.restart_live = true;
        }
    }

    pub fn is_live(&self, now_ms: u64) -> bool {
        self.live_until.is_some_and(|t| now_ms < t)
    }

    /// The gap in force right now: doubling from `period_ms` toward
    /// `idle_period_ms` once the scene has been still for a few windows.
    pub fn current_period_ms(&self) -> u64 {
        let over = self.quiet_windows.saturating_sub(self.duty.quiet_before_backoff);
        if over == 0 {
            return self.duty.period_ms;
        }
        self.duty.period_ms.saturating_mul(1u64 << over.min(16)).min(self.duty.idle_period_ms)
    }
    /// `wanted` is whether the pane is visible at all; when it goes false the
    /// camera stops at once rather than finishing its window.
    pub fn tick(&mut self, wanted: bool, now_ms: u64) -> DutyAction {
        if !wanted {
            self.live_until = None;
            self.restart_live = false;
            if self.running_since.take().is_some() {
                self.last_stop = Some(now_ms);
                self.got_frame = false;
                return DutyAction::Stop;
            }
            return DutyAction::Wait;
        }
        if std::mem::take(&mut self.restart_live) {
            // The caller still holds the old source; Stop it, then Start next tick.
            self.last_stop = None;
            self.got_frame = false;
            return DutyAction::Stop;
        }
        let live = self.is_live(now_ms);
        if !live && self.live_until.is_some() {
            // The live window just ended: stop streaming, back to samples.
            self.live_until = None;
            if self.running_since.take().is_some() {
                self.last_stop = Some(now_ms);
                self.got_frame = false;
                return DutyAction::Stop;
            }
        }
        match self.running_since {
            Some(_) if live => DutyAction::Wait,
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
                Some(t) if !live && now_ms.saturating_sub(t) < self.current_period_ms() => DutyAction::Wait,
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
    if let Some(e) = &choice.exposure {
        // libcamerasrc exposes libcamera's controls as element properties.
        // These must be pushed before the first "!": everything after that
        // belongs to the next element in the pipeline.
        v.push("ae-enable=false".into());
        v.push(format!("exposure-time={}", e.time_us));
        v.push(format!("analogue-gain={:.3}", e.gain));
        // Auto white balance is the same problem in colour: it neutralises
        // the tint, which is the one thing the wash is for.
        v.push("awb-enable=false".into());
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

/// The capture size to ask the sensor for, with a fallback. The default
/// stream on this phone is the sensor's full mode (4096x3072 rear), which the
/// `simple` pipeline debayers IN SOFTWARE before it is scaled to 240x320: most
/// of the camera's cost for pixels that are thrown away. So ask for a small
/// mode first; if the camera will not produce it (the pipeline ends with no
/// frame), stop asking and use the default from then on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CaptureProbe {
    pub small: (usize, usize),
    /// None = untried, Some(true) = works, Some(false) = refused.
    pub small_ok: Option<bool>,
}

impl Default for CaptureProbe {
    fn default() -> Self {
        CaptureProbe { small: (640, 480), small_ok: None }
    }
}

impl CaptureProbe {
    /// The size to request (None = the camera's default).
    pub fn request(&self) -> Option<(usize, usize)> {
        (self.small_ok != Some(false)).then_some(self.small)
    }
    /// A source using `request()` ended; `frames` is how many it produced.
    /// Returns a note when the probe learned something.
    pub fn observe(&mut self, frames: u64) -> Option<String> {
        if self.small_ok.is_some() {
            return None;
        }
        if frames > 0 {
            self.small_ok = Some(true);
            Some(format!("camera: small capture {}x{} works", self.small.0, self.small.1))
        } else {
            self.small_ok = Some(false);
            Some(format!("camera: {}x{} refused; using the camera's default mode (costlier)", self.small.0, self.small.1))
        }
    }
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
        assert_eq!(c, CameraChoice { name: Some(id.into()), capture: Some((1280, 960)), fps: None, exposure: None });
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
        let c = CameraChoice { name: Some("/base/cam@1a".into()), capture: Some((640, 480)), fps: Some(1), exposure: None };
        let line = gst_command(&c, GLASS_W, GLASS_H).join(" ");
        assert!(line.contains("camera-name=/base/cam@1a"));
        assert!(line.contains("width=640,height=480,framerate=1/1"), "on the capture caps, not only downstream: {line}");
        assert!(line.contains("videorate"));
        assert!(line.contains("width=32,height=40"));
        // No fps asked for: the pipeline is exactly as it was before.
        let plain = CameraChoice { name: None, capture: None, fps: None, exposure: None };
        let l2 = gst_command(&plain, 240, 320).join(" ");
        assert!(!l2.contains("framerate") && !l2.contains("videorate"));
    }

    #[test]
    fn a_still_scene_backs_the_camera_right_off() {
        // No accelerometer on this phone, so "has anything happened" is
        // answered by the frames themselves — and for a tint, whether the
        // ROOM changed is the better question anyway.
        let duty = Duty { warmup_ms: 100, period_ms: 10_000, idle_period_ms: 160_000, quiet_before_backoff: 2 };
        let mut d = DutyCycle::new(duty);
        assert_eq!(d.current_period_ms(), 10_000);
        // Two quiet windows are not yet evidence: it might be a wall.
        d.frame(false);
        d.frame(false);
        assert_eq!(d.current_period_ms(), 10_000, "still sampling normally");
        // Then it doubles per quiet window, up to the idle ceiling.
        d.frame(false);
        assert_eq!(d.current_period_ms(), 20_000);
        d.frame(false);
        assert_eq!(d.current_period_ms(), 40_000);
        for _ in 0..10 {
            d.frame(false);
        }
        assert_eq!(d.current_period_ms(), 160_000, "capped, not runaway");
        // The room comes back to life: back to normal at once, not by
        // halving, because the next thing to happen deserves to be seen.
        d.frame(true);
        assert_eq!(d.current_period_ms(), 10_000);
        // A touch or a wake does the same without waiting for a frame.
        for _ in 0..8 {
            d.frame(false);
        }
        assert!(d.current_period_ms() > 10_000);
        d.wake();
        assert_eq!(d.current_period_ms(), 10_000);
    }

    #[test]
    fn backoff_actually_lengthens_the_gap() {
        let mut d = DutyCycle::new(Duty { warmup_ms: 100, period_ms: 10_000, idle_period_ms: 80_000, quiet_before_backoff: 0 });
        assert_eq!(d.tick(true, 0), DutyAction::Start);
        d.frame(false);
        assert_eq!(d.tick(true, 50), DutyAction::Stop);
        // One quiet window: the gap is already 20 s, so 15 s is too soon.
        assert_eq!(d.tick(true, 15_000), DutyAction::Wait);
        assert_eq!(d.tick(true, 20_100), DutyAction::Start);
        d.frame(true);
        assert_eq!(d.tick(true, 20_200), DutyAction::Stop);
        // Changed: back to the base gap.
        assert_eq!(d.tick(true, 30_300), DutyAction::Start);
    }

    #[test]
    fn scene_change_is_judged_on_what_the_pane_shows() {
        let a = vec![[10u8, 10, 10], [200, 200, 200]];
        assert!(!scene_changed(&a, &a, 4.0), "identical is not a change");
        let nudged = vec![[12u8, 11, 10], [199, 200, 201]];
        assert!(!scene_changed(&a, &nudged, 4.0), "sensor noise is not a change");
        let lit = vec![[10u8, 10, 10], [40, 40, 40]];
        assert!(scene_changed(&a, &lit, 4.0), "someone turned a light off");
        // A first frame, or a resized grid, counts as changed rather than
        // silently starting in the backed-off state.
        assert!(scene_changed(&[], &[], 4.0));
        assert!(scene_changed(&a, &a[..1], 4.0));
    }

    #[test]
    fn the_camera_sips_rather_than_streams() {
        let mut d = DutyCycle::new(Duty { warmup_ms: 1_000, period_ms: 10_000, ..Duty::default() });
        assert_eq!(d.tick(true, 0), DutyAction::Start);
        assert_eq!(d.tick(true, 500), DutyAction::Wait, "still warming up");
        d.frame(true);
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

#[cfg(test)]
mod exposure_tests {
    use super::*;

    #[test]
    fn a_frozen_exposure_reaches_the_pipeline() {
        let c = CameraChoice { name: None, capture: None, fps: Some(2), exposure: Some(Exposure::default()) };
        let line = gst_command(&c, GLASS_W, GLASS_H).join(" ");
        assert!(line.contains("ae-enable=false"), "auto exposure must be off: {line}");
        assert!(line.contains("awb-enable=false"), "and auto white balance, or the tint is neutralised");
        assert!(line.contains("exposure-time=20000"));
        assert!(line.contains("analogue-gain=2.000"));
        // Not asked for: the pipeline is exactly as before.
        let plain = CameraChoice { name: None, capture: None, fps: None, exposure: None };
        assert!(!gst_command(&plain, 240, 320).join(" ").contains("ae-enable"));
    }

    #[test]
    fn a_dark_room_reads_as_dark() {
        // The point of freezing: brightness differences survive instead of
        // being normalised away.
        let e = Exposure::default();
        let dark = scene_ev(12.0, &e);
        let dim = scene_ev(60.0, &e);
        let bright = scene_ev(240.0, &e);
        assert!(dark < dim && dim < bright, "{dark} {dim} {bright}");
        // Each doubling of measured level is one stop.
        assert!((scene_ev(120.0, &e) - scene_ev(60.0, &e) - 1.0).abs() < 1e-4);
    }

    #[test]
    fn the_reading_is_comparable_across_settings() {
        // The same scene measured with twice the exposure must report the
        // same light, or a future auto-ranging step would make the tint jump
        // whenever it changed settings.
        let short = Exposure { time_us: 10_000, gain: 1.0 };
        let long = Exposure { time_us: 20_000, gain: 1.0 };
        let a = scene_ev(50.0, &short);
        let b = scene_ev(100.0, &long);
        assert!((a - b).abs() < 1e-4, "{a} vs {b}");
        // Gain counts the same as time: both are stops.
        let gained = Exposure { time_us: 10_000, gain: 2.0 };
        assert!((scene_ev(100.0, &gained) - b).abs() < 1e-4);
    }

    #[test]
    fn the_meter_does_not_blow_up_at_the_ends() {
        let e = Exposure::default();
        assert!(scene_ev(0.0, &e).is_finite(), "a black frame is dark, not negative infinity");
        assert!(scene_ev(255.0, &e).is_finite());
        assert!(scene_ev(1e9, &e).is_finite());
        assert!(scene_ev(100.0, &Exposure { time_us: 0, gain: 0.0 }).is_finite(), "nonsense settings must not poison the number");
    }
}

#[cfg(test)]
mod live_tests {
    use super::*;

    #[test]
    fn the_live_window_streams_then_falls_back_to_samples() {
        let mut d = DutyCycle::new(Duty::default());
        d.go_live(0, LIVE_MS);
        assert_eq!(d.tick(true, 0), DutyAction::Start);
        d.frame(true);
        assert_eq!(d.tick(true, 100), DutyAction::Wait, "live: a frame does NOT end the window");
        d.frame(true);
        assert_eq!(d.tick(true, 5_000), DutyAction::Wait);
        assert!(d.is_live(5_000));
        assert_eq!(d.tick(true, LIVE_MS), DutyAction::Stop, "window over");
        assert!(!d.is_live(LIVE_MS));
        assert_eq!(d.tick(true, LIVE_MS + 100), DutyAction::Wait, "then the normal period applies");
    }

    #[test]
    fn going_live_mid_sample_restarts_at_the_live_rate_and_hiding_ends_it() {
        let mut d = DutyCycle::new(Duty::default());
        assert_eq!(d.tick(true, 0), DutyAction::Start);
        d.go_live(100, LIVE_MS);
        assert_eq!(d.tick(true, 100), DutyAction::Stop, "drop the 2 fps source");
        assert_eq!(d.tick(true, 101), DutyAction::Start, "and start the live one at once");
        assert_eq!(d.tick(false, 200), DutyAction::Stop, "cover gone: stop now");
        assert!(!d.is_live(300), "and the live window is forgotten");
    }

    #[test]
    fn the_capture_probe_asks_small_once_then_remembers() {
        let mut p = CaptureProbe::default();
        assert_eq!(p.request(), Some((640, 480)));
        assert!(p.observe(0).unwrap().contains("refused"));
        assert_eq!(p.request(), None, "fall back to the default mode");
        assert!(p.observe(5).is_none(), "decided: no more notes");
        let mut p = CaptureProbe::default();
        assert!(p.observe(3).unwrap().contains("works"));
        assert_eq!(p.request(), Some((640, 480)));
    }
}
