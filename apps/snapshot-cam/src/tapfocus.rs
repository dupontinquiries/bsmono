//! tapfocus -- tap the viewfinder, the lens finds focus there.
//!
//! This phone's lens motor (a `dw9719` voice coil) is a separate V4L2 subdevice that libcamera never
//! drives, so focus is OURS: capture-kit's `focus::Search` (a coarse sweep then a refine) asks for a
//! lens position, the frame that arrives AFTER the lens has settled is scored in the tapped region,
//! and the search asks again. This module is the driver: it owns the timing rules that make the
//! scores mean something, and nothing else.
//!
//! * A probe is scored only on a frame at least `FRESH_FRAMES` past the one current when the move was
//!   issued, and at least `SETTLE_MS` after it: earlier frames were captured at the OLD position.
//! * A flat scene (a blank wall, the dark) is refused, and the lens goes back where it was.
//! * It gives up after `TIMEOUT_MS`.
//! * The lens is behind a trait so the whole thing is tested without a camera; the real one is
//!   `V4l2Lens` (a worker thread, so a slow `v4l2-ctl` never stalls the viewfinder).
//!
//! `actuator_for` answers which subdevice moves which camera, from the camera's libcamera id.

use capture_kit::focus::{self, Search, Step};
use capture_kit::RawFrame;

/// The frame must be this many frames past the move, and this long after it.
pub const FRESH_FRAMES: u64 = 2;
pub const SETTLE_MS: u64 = 150;
/// The whole search.
pub const TIMEOUT_MS: u64 = 7_000;
/// The scored region: a square of this fraction of each axis, centred on the tap.
pub const ROI_SIZE: f32 = 0.18;
/// How long the result is shown over the picture.
pub const RESULT_SHOW_MS: u64 = 1_400;

/// Something that moves the lens.
pub trait Lens {
    fn move_to(&mut self, pos: i64) -> Result<(), String>;
    /// Where the lens is now, if it can say (so a refusal can put it back).
    fn current(&mut self) -> Option<i64> { None }
}

/// A lens motor found for a camera.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Actuator {
    pub device: String,
    pub range: (i64, i64, i64),
}

#[derive(Debug, Clone, PartialEq)]
pub enum Verdict {
    /// Focused at `pos` (the lens is left there).
    Locked { pos: i64, contrast: f64 },
    /// The scene had nothing to focus on; the lens went back.
    Refused,
    Failed(String),
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct Probe {
    pos: i64,
    issued_ms: u64,
    issued_seq: u64,
}

enum State {
    Idle,
    Searching { search: Search, roi: [f32; 4], probe: Option<Probe>, started_ms: u64, previous: Option<i64> },
}

/// What the picture should show over the tap.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Ring {
    Searching,
    Locked,
    Refused,
    Failed,
}

pub struct TapFocus {
    state: State,
    /// Where the tap was on the picture (fractions of the drawn picture) and the last verdict with its time.
    pub tapped_at: Option<[f32; 2]>,
    result: Option<(Verdict, u64)>,
}

impl Default for TapFocus {
    fn default() -> Self { TapFocus { state: State::Idle, tapped_at: None, result: None } }
}

impl TapFocus {
    pub fn is_searching(&self) -> bool { matches!(self.state, State::Searching { .. }) }

    /// Start (or restart) a search on `roi` (frame fractions). `tap` is where to draw the ring.
    pub fn begin(&mut self, roi: [f32; 4], tap: [f32; 2], act: &Actuator, lens: &mut dyn Lens, now_ms: u64) {
        let (min, max, step) = act.range;
        self.state = State::Searching { search: Search::new(min, max, step), roi, probe: None, started_ms: now_ms, previous: lens.current() };
        self.tapped_at = Some(tap);
        self.result = None;
    }

    pub fn cancel(&mut self) {
        self.state = State::Idle;
        self.tapped_at = None;
        self.result = None;
    }

    /// Feed the newest frame. Returns the verdict on the call that finishes the search.
    pub fn on_frame(&mut self, f: &RawFrame, now_ms: u64, lens: &mut dyn Lens) -> Option<Verdict> {
        let State::Searching { search, roi, probe, started_ms, previous } = &mut self.state else { return None };
        if now_ms.saturating_sub(*started_ms) > TIMEOUT_MS {
            return self.finish(Verdict::Failed("focus timed out".into()), now_ms);
        }
        loop {
            match probe {
                None => match search.step() {
                    Step::Probe(p) => {
                        if let Err(e) = lens.move_to(p) { return self.finish(Verdict::Failed(e), now_ms); }
                        *probe = Some(Probe { pos: p, issued_ms: now_ms, issued_seq: f.seq });
                        return None; // the frames in flight are the old position's
                    }
                    Step::Done(o) => {
                        let verdict = match o.position {
                            Some(p) => match lens.move_to(p) {
                                Ok(()) => Verdict::Locked { pos: p, contrast: o.contrast },
                                Err(e) => Verdict::Failed(e),
                            },
                            None => {
                                // refuse: back to where the lens was, when it can say
                                if let Some(prev) = *previous { let _ = lens.move_to(prev); }
                                Verdict::Refused
                            }
                        };
                        return self.finish(verdict, now_ms);
                    }
                },
                Some(pr) => {
                    let settled = now_ms.saturating_sub(pr.issued_ms) >= SETTLE_MS && f.seq >= pr.issued_seq + FRESH_FRAMES;
                    if !settled { return None; }
                    let Some(score) = focus::sharpness(f, *roi) else { return None }; // a torn frame: wait for the next
                    search.feed(score);
                    *probe = None; // ask for the next one in this same call
                }
            }
        }
    }

    fn finish(&mut self, v: Verdict, now_ms: u64) -> Option<Verdict> {
        self.state = State::Idle;
        self.result = Some((v.clone(), now_ms));
        Some(v)
    }

    /// What to draw now: searching while it runs, then the result for `RESULT_SHOW_MS`.
    pub fn ring(&self, now_ms: u64) -> Option<(Ring, f32)> {
        if self.is_searching() { return Some((Ring::Searching, 1.0)); }
        let (v, at) = self.result.as_ref()?;
        let age = now_ms.saturating_sub(*at);
        if age >= RESULT_SHOW_MS { return None; }
        let fade = 1.0 - (age as f32 / RESULT_SHOW_MS as f32).powi(2);
        Some((match v { Verdict::Locked { .. } => Ring::Locked, Verdict::Refused => Ring::Refused, Verdict::Failed(_) => Ring::Failed }, fade))
    }

    /// The last verdict's one-line message.
    pub fn message(&self) -> Option<String> {
        match &self.result.as_ref()?.0 {
            Verdict::Locked { pos, contrast } => Some(format!("focused (lens {pos}, contrast x{contrast:.1})")),
            Verdict::Refused => Some("nothing to focus on there".into()),
            Verdict::Failed(e) => Some(format!("focus failed: {e}")),
        }
    }
}

/// The frame fractions a tap at `picture` (fractions of what is drawn, already mirrored) means, given
/// the window of the frame being shown. The region is a square of `ROI_SIZE`.
pub fn roi_for_tap(picture: [f32; 2], window: [f32; 4]) -> ([f32; 4], [f32; 2]) {
    let u = window[0] + picture[0].clamp(0.0, 1.0) * (window[2] - window[0]);
    let v = window[1] + picture[1].clamp(0.0, 1.0) * (window[3] - window[1]);
    (focus::roi_at(u, v, ROI_SIZE), [u, v])
}

/// The lens motor's device path for the camera whose libcamera id is `camera_id`
/// (`.../camera@29`), from the kernel's subdevice list; `None` when it is a fixed-focus
/// camera (no motor on its bus) or the pairing is ambiguous.
pub fn actuator_for(camera_id: &str, subdevs: &[(String, String)]) -> Option<String> {
    let sensor = focus::sensor_for_camera_id(camera_id, subdevs)?;
    focus::pair_actuators(subdevs).into_iter().find(|p| p.sensor_path == sensor.0).map(|p| p.actuator_path)
}

/// The libcamera camera id in a source string such as `gst:libcamerasrc camera-name=/base/.../camera@29`.
pub fn camera_id_in(source: &str) -> Option<String> {
    let at = source.find("camera-name=")? + "camera-name=".len();
    let rest = &source[at..];
    let id: String = rest.trim_start_matches('"').chars().take_while(|c| !c.is_whitespace() && *c != '"').collect();
    (!id.is_empty()).then_some(id)
}

/// The real lens: `v4l2-ctl` on a worker thread. `move_to` never blocks; the last error is kept.
pub struct V4l2Lens {
    tx: std::sync::mpsc::Sender<i64>,
    err: std::sync::Arc<std::sync::Mutex<Option<String>>>,
    device: String,
    prog: String,
}

impl V4l2Lens {
    pub fn new(prog: String, device: String) -> V4l2Lens {
        let (tx, rx) = std::sync::mpsc::channel::<i64>();
        let err = std::sync::Arc::new(std::sync::Mutex::new(None));
        let (e2, p2, d2) = (err.clone(), prog.clone(), device.clone());
        std::thread::Builder::new().name("lens".into()).spawn(move || {
            while let Ok(mut pos) = rx.recv() {
                // only the newest request matters
                while let Ok(p) = rx.try_recv() { pos = p; }
                *e2.lock().unwrap() = focus::set_focus(&p2, &d2, pos).err();
            }
        }).ok();
        V4l2Lens { tx, err, device, prog }
    }
}

impl Lens for V4l2Lens {
    fn move_to(&mut self, pos: i64) -> Result<(), String> {
        if let Some(e) = self.err.lock().unwrap().take() { return Err(e); }
        self.tx.send(pos).map_err(|_| "the lens worker stopped".to_string())
    }
    fn current(&mut self) -> Option<i64> {
        let out = std::process::Command::new(&self.prog).args(["-d", &self.device, "--get-ctrl", "focus_absolute"]).output().ok()?;
        String::from_utf8_lossy(&out.stdout).split(':').nth(1)?.trim().parse().ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    /// A scene seen through a lens: two regions with their own best lens position. A frame's local
    /// contrast falls off as the lens leaves a region's best position.
    #[derive(Clone, Copy)]
    struct Scene { left_peak: i64, right_peak: i64, flat: bool }

    fn frame(scene: &Scene, lens_pos: i64, seq: u64) -> RawFrame {
        let (w, h) = (160u32, 90u32);
        let mut v = Vec::with_capacity((w * h * 4) as usize);
        for y in 0..h {
            for x in 0..w {
                let peak = if x < w / 2 { scene.left_peak } else { scene.right_peak };
                let d = (lens_pos - peak) as f32 / 180.0;
                let contrast = if scene.flat { 0.0 } else { (-d * d).exp() };
                let chk = if (x + y) % 2 == 0 { 1.0 } else { -1.0 };
                v.extend_from_slice(&[(128.0 + 100.0 * contrast * chk) as u8, (128.0 + 100.0 * contrast * chk) as u8, (128.0 + 100.0 * contrast * chk) as u8, 255]);
            }
        }
        RawFrame { width: w, height: h, captured_ms: seq * 33, seq, rgba: Arc::from(v) }
    }

    /// A lens whose position takes effect immediately, but frames lag the pipeline: the frame with
    /// sequence N shows the lens as it was LAG frames earlier.
    struct FakeLens { pos: i64, history: Vec<i64>, moves: Vec<i64>, lag: usize }
    impl FakeLens {
        fn new(pos: i64, lag: usize) -> FakeLens { FakeLens { pos, history: vec![], moves: vec![], lag } }
        fn tick(&mut self) { self.history.push(self.pos); }
        fn seen(&self) -> i64 { let n = self.history.len(); self.history[n.saturating_sub(1 + self.lag)] }
    }
    impl Lens for FakeLens {
        fn move_to(&mut self, pos: i64) -> Result<(), String> { self.pos = pos; self.moves.push(pos); Ok(()) }
        fn current(&mut self) -> Option<i64> { Some(self.pos) }
    }

    const ACT: Actuator = Actuator { device: String::new(), range: (0, 1023, 1) };
    fn act() -> Actuator { Actuator { device: "/dev/v4l-subdev13".into(), range: (0, 1023, 1) } }

    /// Run a search to its verdict: one frame every 33 ms.
    fn run(scene: Scene, roi: [f32; 4], start_pos: i64, lag: usize) -> (Option<Verdict>, FakeLens, u64) {
        let mut lens = FakeLens::new(start_pos, lag);
        let mut tf = TapFocus::default();
        tf.begin(roi, [0.5, 0.5], &act(), &mut lens, 0);
        let mut verdict = None;
        let mut t = 0;
        for seq in 0..400u64 {
            t = seq * 33;
            lens.tick();
            let f = frame(&scene, lens.seen(), seq);
            if let Some(v) = tf.on_frame(&f, t, &mut lens) { verdict = Some(v); break; }
        }
        (verdict, lens, t)
    }

    #[test]
    fn it_finds_the_peak_in_the_tapped_region_despite_the_pipeline_lag() {
        let scene = Scene { left_peak: 300, right_peak: 800, flat: false };
        // tap on the left
        let (v, lens, _) = run(scene, [0.1, 0.3, 0.3, 0.6], 100, 3);
        let Some(Verdict::Locked { pos, .. }) = v else { panic!("{v:?}") };
        assert!((pos - 300).abs() <= 40, "left subject focused near 300, got {pos}");
        assert_eq!(*lens.moves.last().unwrap(), pos, "the lens is left at the answer");
        // tap on the right: the SAME scene, another answer: the region decides
        let (v, _, _) = run(scene, [0.7, 0.3, 0.9, 0.6], 100, 3);
        let Some(Verdict::Locked { pos, .. }) = v else { panic!("{v:?}") };
        assert!((pos - 800).abs() <= 40, "right subject focused near 800, got {pos}");
    }

    #[test]
    fn a_flat_scene_is_refused_and_the_lens_goes_back() {
        let scene = Scene { left_peak: 300, right_peak: 800, flat: true };
        let (v, lens, _) = run(scene, [0.4, 0.4, 0.6, 0.6], 555, 2);
        assert_eq!(v, Some(Verdict::Refused));
        assert_eq!(lens.pos, 555, "back where it started");
    }

    #[test]
    fn a_probe_is_scored_only_on_a_settled_fresh_frame() {
        // with a lag of 3 frames the search only works if the controller waits: scoring the frame
        // right after a move reads the old position. Check the wait directly.
        let scene = Scene { left_peak: 600, right_peak: 600, flat: false };
        let mut lens = FakeLens::new(100, 3);
        let mut tf = TapFocus::default();
        tf.begin([0.2, 0.2, 0.8, 0.8], [0.5, 0.5], &act(), &mut lens, 0);
        lens.tick();
        assert_eq!(tf.on_frame(&frame(&scene, 100, 0), 0, &mut lens), None);
        assert_eq!(lens.moves.len(), 1, "the first probe was issued");
        // the next frames arrive too soon (and too few): no new move
        for (seq, t) in [(1u64, 33u64), (2, 66), (3, 99)] {
            lens.tick();
            tf.on_frame(&frame(&scene, lens.seen(), seq), t, &mut lens);
            assert_eq!(lens.moves.len(), 1, "still waiting at {t} ms (seq {seq})");
        }
        // 2 frames past and 150 ms later: the next probe goes out
        lens.tick();
        tf.on_frame(&frame(&scene, lens.seen(), 5), 200, &mut lens);
        assert_eq!(lens.moves.len(), 2, "scored, then the next position asked for");
    }

    #[test]
    fn it_times_out_if_frames_never_settle_and_restarts_on_a_new_tap() {
        let scene = Scene { left_peak: 500, right_peak: 500, flat: false };
        let mut lens = FakeLens::new(0, 0);
        let mut tf = TapFocus::default();
        tf.begin([0.3, 0.3, 0.7, 0.7], [0.5, 0.5], &act(), &mut lens, 0);
        // the same frame forever (seq never advances): it must give up
        let f = frame(&scene, 0, 0);
        assert_eq!(tf.on_frame(&f, 0, &mut lens), None);
        let v = tf.on_frame(&f, TIMEOUT_MS + 100, &mut lens);
        assert!(matches!(v, Some(Verdict::Failed(ref m)) if m.contains("timed out")), "{v:?}");
        assert!(!tf.is_searching());
        // a new tap restarts cleanly
        tf.begin([0.3, 0.3, 0.7, 0.7], [0.2, 0.2], &act(), &mut lens, 10_000);
        assert!(tf.is_searching() && tf.tapped_at == Some([0.2, 0.2]));
        tf.cancel();
        assert!(!tf.is_searching() && tf.ring(10_100).is_none());
    }

    #[test]
    fn the_ring_shows_searching_then_the_result_and_fades() {
        let scene = Scene { left_peak: 500, right_peak: 500, flat: false };
        let (v, _, t) = run(scene, [0.3, 0.3, 0.7, 0.7], 0, 1);
        assert!(matches!(v, Some(Verdict::Locked { .. })));
        let mut tf = TapFocus::default();
        let mut lens = FakeLens::new(0, 0);
        tf.begin([0.3, 0.3, 0.7, 0.7], [0.5, 0.5], &act(), &mut lens, 0);
        assert_eq!(tf.ring(0), Some((Ring::Searching, 1.0)));
        tf.finish(Verdict::Locked { pos: 500, contrast: 3.0 }, t);
        let (r, a) = tf.ring(t + 100).unwrap();
        assert_eq!(r, Ring::Locked);
        assert!(a > 0.9);
        assert!(tf.ring(t + RESULT_SHOW_MS + 1).is_none(), "it fades away");
        assert!(tf.message().unwrap().contains("focused"));
        let _ = ACT;
    }

    #[test]
    fn a_tap_maps_to_the_frame_through_the_zoom_window() {
        let (roi, at) = roi_for_tap([0.5, 0.5], [0.0, 0.0, 1.0, 1.0]);
        assert_eq!(at, [0.5, 0.5]);
        assert!((roi[0] - (0.5 - ROI_SIZE / 2.0)).abs() < 1e-6);
        // zoomed 2x into the top-left: the picture's centre is the frame's (0.25, 0.25)
        let (_, at) = roi_for_tap([0.5, 0.5], [0.0, 0.0, 0.5, 0.5]);
        assert_eq!(at, [0.25, 0.25]);
        // a tap on the very edge stays inside the frame
        let (roi, _) = roi_for_tap([1.0, 1.0], [0.0, 0.0, 1.0, 1.0]);
        assert!(roi[2] <= 1.0 && roi[3] <= 1.0 && roi[0] >= 0.0);
    }

    #[test]
    fn the_lens_motor_is_found_for_the_main_camera_and_the_selfie_camera_is_fixed_focus() {
        let subdevs: Vec<(String, String)> = [
            ("/dev/v4l-subdev12", "imx858 17-0029"),
            ("/dev/v4l-subdev13", "dw9719 17-000c"),
            ("/dev/v4l-subdev14", "imx800 16-001a"),
            ("/dev/v4l-subdev15", "s5kjn1 18-0010"),
        ].iter().map(|(a, b)| (a.to_string(), b.to_string())).collect();
        assert_eq!(actuator_for("/base/soc@0/cci@ac4a000/i2c-bus@1/camera@29", &subdevs), Some("/dev/v4l-subdev13".into()));
        assert_eq!(actuator_for("/base/soc@0/cci@ac4b000/i2c-bus@1/camera@10", &subdevs), None, "no motor on the selfie camera's bus");
        assert_eq!(actuator_for("/base/soc@0/cci@ac4a000/i2c-bus@0/camera@1a", &subdevs), None, "the other sensor has none either");
        assert_eq!(camera_id_in("gst:libcamerasrc camera-name=/base/soc@0/cci@ac4a000/i2c-bus@1/camera@29"), Some("/base/soc@0/cci@ac4a000/i2c-bus@1/camera@29".into()));
        assert_eq!(camera_id_in("gst:videotestsrc"), None);
    }
}
