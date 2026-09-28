//! camera — the frame source, as the shell sees it.
//!
//! Probe once, build a plan for the chosen facing, run capture-kit's rig,
//! and answer three questions per frame: is there a new frame, what is
//! the status, what does the shutter get. Facing is a plan choice (which
//! node to prefer) rather than a flip: the preview mirrors a front camera
//! on screen, and the saved photo is the true image — a selfie file that
//! reads backwards is the mistake the mirror exists to prevent on screen,
//! not to bake into the JPEG.
//!
//! `SNAPSHOT_CAM_TEST_PATTERN=1` runs the kit's pattern instead of a
//! camera, so the whole path — thread, slot, upload, shutter, JPEG, roll,
//! handoff — runs on a machine with none.

use capture_kit::probe::Probe;
use capture_kit::{RawFrame, Rig, Status};

/// Which camera. Only a phone has two; a desktop's webcam is `Front` by
/// default because it faces the person.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Facing {
    #[default]
    Front,
    Rear,
}

impl Facing {
    pub fn flipped(self) -> Facing { match self { Facing::Front => Facing::Rear, Facing::Rear => Facing::Front } }
    pub fn label(self) -> &'static str { match self { Facing::Front => "front", Facing::Rear => "rear" } }
}

/// The capture geometry the rig forces. 1280×720 is what a phone preview
/// wants and what the fdsink framing makes every frame: 3.7 MB, ~110 MB/s
/// at 30 fps through the pipe, which is within what a phone's page cache
/// does without warming.
pub const WIDTH: u32 = 1280;
pub const HEIGHT: u32 = 720;
/// The test pattern is smaller: nothing about the pipeline is being
/// exercised, only everything after it.
pub const PATTERN_WIDTH: u32 = 640;
pub const PATTERN_HEIGHT: u32 = 360;

/// Where the frames come from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source { Camera, TestPattern, None }

pub struct Camera {
    pub probe: Probe,
    pub facing: Facing,
    pub source: Source,
    rig: Option<Rig>,
    /// The last status the rig reported, kept between edges.
    pub status: Status,
    /// The last frame's sequence the shell drew, so it uploads once per
    /// new frame rather than once per paint.
    pub drawn_seq: u64,
    /// The plan the rig was started with, so the live candidate's device
    /// can be named for `hwctl`.
    started: Vec<capture_kit::SourceCandidate>,
}

impl Camera {
    /// Probe and start. The test pattern when the environment asks for
    /// it, the camera when there is one, nothing (and a status that says
    /// so) otherwise.
    pub fn open(facing: Facing) -> Camera {
        let test = std::env::var("SNAPSHOT_CAM_TEST_PATTERN").map(|v| v == "1").unwrap_or(false);
        Self::open_with(facing, test)
    }

    /// `open`, with the pattern choice explicit.
    pub fn open_with(facing: Facing, test: bool) -> Camera {
        let probe = if test { Probe::default() } else { Probe::run() };
        let mut cam = Camera { probe, facing, source: Source::None, rig: None, status: Status::Idle, drawn_seq: 0, started: vec![] };
        cam.start(test);
        cam
    }

    /// A camera over a probe, not started — for tests and for "no camera".
    pub fn unstarted(probe: Probe, facing: Facing) -> Camera {
        Camera { probe, facing, source: Source::None, rig: None,
                 status: Status::Exhausted { log: vec!["no camera found".into()] }, drawn_seq: 0, started: vec![] }
    }

    fn start(&mut self, test: bool) {
        self.stop();
        if test {
            self.rig = Some(Rig::start_test_pattern(PATTERN_WIDTH, PATTERN_HEIGHT));
            self.source = Source::TestPattern;
            self.status = Status::Trying { label: "test pattern".into() };
            return;
        }
        let plan = self.plan();
        if plan.is_empty() {
            self.source = Source::None;
            self.status = Status::Exhausted { log: vec![self.probe.summary()] };
            return;
        }
        for c in &plan { println!("snapshot-cam: capture candidate [{}]: {}", c.label, c.pipeline); }
        self.started = plan.clone();
        self.rig = Some(Rig::start(plan, WIDTH, HEIGHT));
        self.source = Source::Camera;
        self.status = Status::Trying { label: "camera".into() };
    }

    /// The plan for the current facing: the node the platform says faces
    /// that way goes first when it knows; otherwise the probe's order.
    ///
    /// `SNAPSHOT_CAM_SOURCE` overrides the head of the plan: a `/dev/video*`
    /// path, a PipeWire node name, or `gst:<source element>` — the last is
    /// how the real `gst-launch-1.0` path runs on a machine with no camera
    /// (`gst:videotestsrc pattern=ball`), which the in-process pattern
    /// cannot exercise.
    pub fn plan(&self) -> Vec<capture_kit::SourceCandidate> {
        let want_front = self.facing == Facing::Front;
        let preferred = std::env::var("SNAPSHOT_CAM_SOURCE").ok().filter(|v| !v.trim().is_empty())
            .or_else(|| self.probe.pw.iter().find(|c| c.front == Some(want_front)).map(|c| c.node.clone()));
        self.probe.plan(preferred.as_deref(), false)
    }

    /// Does this machine have a second camera to switch to?
    pub fn can_switch(&self) -> bool {
        self.probe.pw.iter().filter(|c| c.front.is_some()).map(|c| c.front).collect::<std::collections::HashSet<_>>().len() > 1
    }

    /// Switch facing and restart the rig on the other camera.
    pub fn flip(&mut self) {
        self.facing = self.facing.flipped();
        let test = self.source == Source::TestPattern;
        self.start(test);
    }

    pub fn stop(&mut self) {
        if let Some(mut r) = self.rig.take() { r.stop(); }
        self.status = Status::Idle;
    }

    /// Drain the rig's status edge into `status`.
    pub fn poll(&mut self) {
        if let Some(r) = &self.rig {
            if let Some(s) = r.shared.take_status() { self.status = s; }
        }
    }

    /// The newest frame, if there is one newer than what was drawn.
    pub fn new_frame(&mut self) -> Option<RawFrame> {
        let f = self.rig.as_ref()?.shared.peek_frame()?;
        if f.seq == self.drawn_seq { return None; }
        self.drawn_seq = f.seq;
        Some(f)
    }

    /// What the shutter takes: the frame the viewfinder shows right now.
    pub fn snapshot(&self) -> Option<RawFrame> {
        self.rig.as_ref()?.shared.peek_frame().filter(|f| f.pixels_plausible())
    }

    /// The line under the viewfinder.
    pub fn status_line(&self) -> String {
        match (&self.status, self.source) {
            (Status::Live { .. }, Source::TestPattern) => "test pattern (no camera used)".into(),
            (Status::Live { label }, _) => format!("live: {label}"),
            (Status::Trying { label }, _) => format!("opening {label}…"),
            (Status::Exhausted { log }, Source::None) => log.last().cloned().unwrap_or_else(|| "no camera found".into()),
            (Status::Exhausted { log }, _) => format!("camera failed — {}", log.last().cloned().unwrap_or_default()),
            (Status::Idle, _) => "camera off".into(),
        }
    }

    pub fn is_live(&self) -> bool { self.status.is_live() }

    /// The `/dev/video*` node of the candidate that is live, if it is a
    /// V4L2 device — what `hwctl` needs. `None` for PipeWire, the test
    /// pattern and a `gst:` override.
    pub fn live_device(&self) -> Option<String> {
        let Status::Live { label } = &self.status else { return None };
        let c = self.started.iter().find(|c| &c.label == label)?;
        device_in(&c.pipeline)
    }
}

/// The first `/dev/video<N>` in a pipeline string.
pub fn device_in(pipeline: &str) -> Option<String> {
    let i = pipeline.find("/dev/video")?;
    let rest = &pipeline[i + "/dev/video".len()..];
    let n: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
    (!n.is_empty()).then(|| format!("/dev/video{n}"))
}

impl Drop for Camera {
    fn drop(&mut self) { self.stop(); }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    #[test]
    fn the_test_pattern_path_delivers_frames_and_a_shutter_frame() {
        let mut cam = Camera::open_with(Facing::Front, true);
        assert_eq!(cam.source, Source::TestPattern);
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut got = None;
        while got.is_none() && Instant::now() < deadline {
            cam.poll();
            got = cam.new_frame();
            std::thread::sleep(Duration::from_millis(20));
        }
        let f = got.expect("a frame from the pattern");
        assert_eq!((f.width, f.height), (PATTERN_WIDTH, PATTERN_HEIGHT));
        assert!(cam.is_live());
        assert_eq!(cam.status_line(), "test pattern (no camera used)");
        assert!(cam.new_frame().map(|n| n.seq > f.seq).unwrap_or(true), "the same frame is not reported twice");
        assert!(cam.snapshot().is_some(), "the shutter has a frame to take");
        cam.flip();
        assert_eq!(cam.facing, Facing::Rear);
        assert_eq!(cam.source, Source::TestPattern, "flipping keeps the pattern");
        cam.stop();
        assert!(!cam.is_live());
    }

    #[test]
    fn the_live_device_is_read_from_the_pipeline() {
        assert_eq!(device_in("v4l2src device=/dev/video12 ! videoconvert"), Some("/dev/video12".into()));
        assert_eq!(device_in("pipewiresrc path=42 ! videoconvert"), None);
        assert_eq!(device_in("x /dev/video ! y"), None);
    }

    #[test]
    fn a_machine_with_no_camera_says_so_rather_than_spinning() {
        let cam = Camera::unstarted(Probe::default(), Facing::Front);
        assert!(cam.plan().is_empty());
        assert_eq!(cam.status_line(), "no camera found");
        assert!(!cam.can_switch());
    }

    #[test]
    fn the_facing_picks_the_platforms_matching_node_first() {
        let dump = r#"[{"info":{"props":{"media.class":"Video/Source","node.name":"libcamera_back","api.libcamera.location":"Back"}}},
                      {"info":{"props":{"media.class":"Video/Source","node.name":"libcamera_front","api.libcamera.location":"Front"}}}]"#;
        let probe = Probe { pw: capture_kit::probe::cameras_from_pw_dump(dump), v4l2: vec![], v4l2_text: String::new() };
        let cam = Camera::unstarted(probe.clone(), Facing::Rear);
        assert!(cam.can_switch());
        let plan = cam.plan();
        assert!(plan[0].pipeline.contains("libcamera_back"), "{plan:?}");
        let front = Camera::unstarted(probe, Facing::Front);
        assert!(front.plan()[0].pipeline.contains("libcamera_front"));
        assert!(!front.plan()[0].pipeline.contains("flip"), "the file is not mirrored; the preview is");
    }
}
