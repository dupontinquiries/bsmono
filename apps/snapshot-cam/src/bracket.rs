//! bracket — taking the several frames a merge needs.
//!
//! A sequence is a list of steps; each step is "set these camera controls,
//! let the sensor settle, keep the next frame". It is driven by frames
//! arriving (not by a timer), so a camera that runs at 10 fps in the dark
//! still gets its settle frames, and it is pure: the shell applies the
//! control changes it hands back and feeds it frames.
//!
//! * **HDR** needs the camera's exposure-time control: −2, 0, +2 stops
//!   around what the camera was using.
//! * **focus** needs its focus control: five positions across the range.
//! * **burst** needs nothing: consecutive frames, for the average.
//!
//! When a sequence ends (or is abandoned), `restore` puts every control it
//! touched back — a bracket must not leave the camera in manual exposure.

use crate::hwctl::{self, Controls, Role};
use capture_kit::RawFrame;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Mode {
    #[default]
    Single,
    Hdr,
    Focus,
    Burst,
    /// One frame, three files: cooler, as set, warmer.
    WbBracket,
}

impl Mode {
    pub const ALL: [Mode; 5] = [Mode::Single, Mode::Hdr, Mode::Focus, Mode::Burst, Mode::WbBracket];
    pub fn label(self) -> &'static str {
        match self { Mode::Single => "single", Mode::Hdr => "HDR", Mode::Focus => "focus stack", Mode::Burst => "burst merge", Mode::WbBracket => "WB bracket" }
    }
    pub fn to_conf(self) -> &'static str {
        match self { Mode::Single => "single", Mode::Hdr => "hdr", Mode::Focus => "focus", Mode::Burst => "burst", Mode::WbBracket => "wb" }
    }
    pub fn from_conf(s: &str) -> Mode {
        match s.trim() { "hdr" => Mode::Hdr, "focus" => Mode::Focus, "burst" => Mode::Burst, "wb" => Mode::WbBracket, _ => Mode::Single }
    }
    /// What this mode needs that the camera may not have.
    pub fn available(self, ctl: Option<&Controls>) -> Result<(), String> {
        let has = |r: Role| ctl.is_some_and(|c| c.get(r).is_some());
        match self {
            Mode::Hdr if !has(Role::Exposure) => Err("HDR needs the camera's exposure control (V4L2); use burst merge".into()),
            Mode::Focus if !has(Role::Focus) => Err("focus stacking needs the camera's focus control (V4L2)".into()),
            _ => Ok(()),
        }
    }
}

/// Frames the sensor gets to settle after a control change. UVC cameras
/// commonly deliver two or three frames exposed at the old setting.
pub const SETTLE_FRAMES: u32 = 4;
/// Frames in a burst merge.
pub const BURST: usize = 6;
/// Positions in a focus stack.
pub const FOCUS_STEPS: usize = 5;
/// HDR bracket, in stops around the current exposure.
pub const HDR_STOPS: [i32; 3] = [-2, 0, 2];

#[derive(Debug, Clone, PartialEq)]
pub struct Sequence {
    pub mode: Mode,
    steps: Vec<Vec<(String, i64)>>,
    restore: Vec<(String, i64)>,
    idx: usize,
    settle: u32,
    last_seq: Option<u64>,
    pub frames: Vec<RawFrame>,
}

/// What the shell should do after feeding a frame.
#[derive(Debug, Clone, PartialEq)]
pub enum Next {
    /// Apply these controls (the next step), then keep feeding.
    Set(Vec<(String, i64)>),
    /// Keep feeding.
    Wait,
    /// All frames are in: apply `restore()`, then merge `frames`.
    Done,
}

impl Sequence {
    /// Plan a sequence. `Err` says what is missing.
    pub fn plan(mode: Mode, ctl: Option<&Controls>) -> Result<Sequence, String> {
        mode.available(ctl)?;
        let mut restore = Vec::new();
        let keep = |c: &Controls, r: Role, restore: &mut Vec<(String, i64)>| {
            if let Some(k) = c.get(r) { restore.push((k.name.clone(), k.value)); }
        };
        let steps: Vec<Vec<(String, i64)>> = match mode {
            Mode::Single | Mode::WbBracket => vec![vec![]],
            Mode::Burst => vec![vec![]; BURST],
            Mode::Hdr => {
                let c = ctl.unwrap();
                keep(c, Role::AutoExposure, &mut restore);
                keep(c, Role::Exposure, &mut restore);
                let e = c.get(Role::Exposure).unwrap();
                let base = e.value.max(e.min.max(1)) as f64;
                HDR_STOPS.iter().map(|s| c.plan_set(Role::Exposure, (base * 2f64.powi(*s)).round() as i64)).collect()
            }
            Mode::Focus => {
                let c = ctl.unwrap();
                keep(c, Role::AutoFocus, &mut restore);
                keep(c, Role::Focus, &mut restore);
                let f = c.get(Role::Focus).unwrap();
                (0..FOCUS_STEPS).map(|i| c.plan_set(Role::Focus, f.min + (f.max - f.min) * i as i64 / (FOCUS_STEPS as i64 - 1))).collect()
            }
        };
        // Restore the automatic control LAST, so a manual value restored
        // after it does not switch auto off again.
        restore.reverse();
        Ok(Sequence { mode, steps, restore, idx: 0, settle: 0, last_seq: None, frames: vec![] })
    }

    /// The first step's controls. Apply them, then feed frames.
    pub fn start(&mut self) -> Vec<(String, i64)> {
        let first = self.steps.first().cloned().unwrap_or_default();
        self.settle = if first.is_empty() { 0 } else { SETTLE_FRAMES };
        first
    }

    pub fn progress(&self) -> (usize, usize) { (self.frames.len(), self.steps.len()) }

    /// Feed a camera frame. The same frame twice is ignored.
    pub fn on_frame(&mut self, f: &RawFrame) -> Next {
        if self.idx >= self.steps.len() { return Next::Done; }
        if self.last_seq == Some(f.seq) { return Next::Wait; }
        self.last_seq = Some(f.seq);
        if self.settle > 0 { self.settle -= 1; return Next::Wait; }
        self.frames.push(f.clone());
        self.idx += 1;
        match self.steps.get(self.idx) {
            None => Next::Done,
            Some(s) if s.is_empty() => Next::Wait,
            Some(s) => { self.settle = SETTLE_FRAMES; Next::Set(s.clone()) }
        }
    }

    /// The controls to put back.
    pub fn restore(&self) -> &[(String, i64)] { &self.restore }
}

/// The WB bracket's three light temperatures, as gains relative to the
/// setting: a picture corrected for 4500K light (cooler), as set, and for
/// 6500K light (warmer).
pub const WB_BRACKET_K: [u32; 3] = [4500, 5500, 6500];
pub fn wb_bracket_gains(base: [f32; 3]) -> [[f32; 3]; 3] {
    let mid = crate::develop::kelvin_gains(5500);
    WB_BRACKET_K.map(|k| { let g = crate::develop::kelvin_gains(k); [0, 1, 2].map(|i| base[i] * g[i] / mid[i]) })
}

/// Human line for a mode's progress.
pub fn status(seq: &Sequence) -> String {
    let (n, of) = seq.progress();
    format!("{}: frame {} of {of}", seq.mode.label(), (n + 1).min(of))
}

/// The labelled shutter speeds of an HDR bracket, for the note.
pub fn hdr_labels(ctl: &Controls) -> String {
    ctl.get(Role::Exposure).map(|e| HDR_STOPS.iter()
        .map(|s| hwctl::shutter_label(((e.value.max(1) as f64) * 2f64.powi(*s)).round() as i64))
        .collect::<Vec<_>>().join(", ")).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctl() -> Controls {
        Controls { device: "/dev/video0".into(), ctrls: hwctl::parse(
            "auto_exposure 0x009a0901 (menu) : min=0 max=3 default=3 value=3\n\
             exposure_time_absolute 0x009a0902 (int) : min=3 max=2047 step=1 default=250 value=200\n\
             focus_absolute 0x009a090a (int) : min=0 max=250 step=5 default=0 value=40\n\
             focus_automatic_continuous 0x009a090c (bool) : default=1 value=1\n") }
    }

    fn frame(seq: u64) -> RawFrame { RawFrame { width: 2, height: 2, captured_ms: seq, seq, rgba: vec![seq as u8; 16].into() } }

    /// Run a sequence to the end, returning the control sets it asked for
    /// and the seqs of the frames it kept.
    fn run(mut s: Sequence) -> (Vec<Vec<(String, i64)>>, Vec<u64>, Sequence) {
        let mut sets = vec![s.start()];
        for seq in 1..200 {
            match s.on_frame(&frame(seq)) {
                Next::Set(p) => sets.push(p),
                Next::Wait => {}
                Next::Done => break,
            }
        }
        let kept = s.frames.iter().map(|f| f.seq).collect();
        (sets, kept, s)
    }

    #[test]
    fn hdr_steps_the_exposure_waits_for_the_sensor_and_restores_auto() {
        let s = Sequence::plan(Mode::Hdr, Some(&ctl())).unwrap();
        let (sets, kept, s) = run(s);
        assert_eq!(sets[0], vec![("auto_exposure".into(), 1), ("exposure_time_absolute".into(), 50)]);
        assert_eq!(sets[1].last().unwrap(), &("exposure_time_absolute".to_string(), 200));
        assert_eq!(sets[2].last().unwrap(), &("exposure_time_absolute".to_string(), 800));
        // Each kept frame comes SETTLE_FRAMES after its control change.
        let gap = SETTLE_FRAMES as u64 + 1;
        assert_eq!(kept, vec![gap, 2 * gap, 3 * gap]);
        assert_eq!(s.restore(), &[("exposure_time_absolute".to_string(), 200), ("auto_exposure".to_string(), 3)],
                   "the manual value first, auto last");
        assert_eq!(hdr_labels(&ctl()), "1/200 s, 1/50 s, 1/12 s");
    }

    #[test]
    fn focus_sweeps_the_range_and_puts_autofocus_back() {
        let (sets, kept, s) = run(Sequence::plan(Mode::Focus, Some(&ctl())).unwrap());
        let pos: Vec<i64> = sets.iter().map(|p| p.last().unwrap().1).collect();
        assert_eq!(pos, vec![0, 60, 125, 185, 250]);
        assert_eq!(kept.len(), FOCUS_STEPS);
        assert_eq!(s.restore().last().unwrap(), &("focus_automatic_continuous".to_string(), 1));
    }

    #[test]
    fn a_burst_takes_consecutive_frames_and_ignores_a_repeat() {
        let mut s = Sequence::plan(Mode::Burst, None).unwrap();
        assert!(s.start().is_empty());
        assert_eq!(s.on_frame(&frame(1)), Next::Wait);
        assert_eq!(s.on_frame(&frame(1)), Next::Wait, "the same frame twice");
        for seq in 2..=BURST as u64 { let _ = s.on_frame(&frame(seq)); }
        assert_eq!(s.frames.len(), BURST);
        assert_eq!(s.on_frame(&frame(99)), Next::Done);
        assert!(s.restore().is_empty());
    }

    #[test]
    fn modes_without_their_control_say_what_is_missing() {
        assert!(Sequence::plan(Mode::Hdr, None).unwrap_err().contains("exposure"));
        assert!(Sequence::plan(Mode::Focus, None).unwrap_err().contains("focus"));
        assert!(Sequence::plan(Mode::Burst, None).is_ok());
        for m in Mode::ALL { assert_eq!(Mode::from_conf(m.to_conf()), m); }
    }
}
//! bracket — taking the several frames a merge needs.
//!
//! A sequence is a list of steps; each step is "set these camera controls,
//! let the sensor settle, keep the next frame". It is driven by frames
//! arriving (not by a timer), so a camera that runs at 10 fps in the dark
//! still gets its settle frames, and it is pure: the shell applies the
//! control changes it hands back and feeds it frames.
//!
//! * **HDR** needs the camera's exposure-time control: −2, 0, +2 stops
//!   around what the camera was using.
//! * **focus** needs its focus control: five positions across the range.
//! * **burst** needs nothing: consecutive frames, for the average.
//!
//! When a sequence ends (or is abandoned), `restore` puts every control it
//! touched back — a bracket must not leave the camera in manual exposure.

use crate::hwctl::{self, Controls, Role};
use capture_kit::RawFrame;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Mode {
    #[default]
    Single,
    Hdr,
    Focus,
    Burst,
    /// One frame, three files: cooler, as set, warmer.
    WbBracket,
}

impl Mode {
    pub const ALL: [Mode; 5] = [Mode::Single, Mode::Hdr, Mode::Focus, Mode::Burst, Mode::WbBracket];
    pub fn label(self) -> &'static str {
        match self { Mode::Single => "single", Mode::Hdr => "HDR", Mode::Focus => "focus stack", Mode::Burst => "burst merge", Mode::WbBracket => "WB bracket" }
    }
    pub fn to_conf(self) -> &'static str {
        match self { Mode::Single => "single", Mode::Hdr => "hdr", Mode::Focus => "focus", Mode::Burst => "burst", Mode::WbBracket => "wb" }
    }
    pub fn from_conf(s: &str) -> Mode {
        match s.trim() { "hdr" => Mode::Hdr, "focus" => Mode::Focus, "burst" => Mode::Burst, "wb" => Mode::WbBracket, _ => Mode::Single }
    }
    /// What this mode needs that the camera may not have.
    pub fn available(self, ctl: Option<&Controls>) -> Result<(), String> {
        let has = |r: Role| ctl.is_some_and(|c| c.get(r).is_some());
        match self {
            Mode::Hdr if !has(Role::Exposure) => Err("HDR needs the camera's exposure control (V4L2); use burst merge".into()),
            Mode::Focus if !has(Role::Focus) => Err("focus stacking needs the camera's focus control (V4L2)".into()),
            _ => Ok(()),
        }
    }
}

/// Frames the sensor gets to settle after a control change. UVC cameras
/// commonly deliver two or three frames exposed at the old setting.
pub const SETTLE_FRAMES: u32 = 4;
/// Frames in a burst merge.
pub const BURST: usize = 6;
/// Positions in a focus stack.
pub const FOCUS_STEPS: usize = 5;
/// HDR bracket, in stops around the current exposure.
pub const HDR_STOPS: [i32; 3] = [-2, 0, 2];

#[derive(Debug, Clone, PartialEq)]
pub struct Sequence {
    pub mode: Mode,
    steps: Vec<Vec<(String, i64)>>,
    restore: Vec<(String, i64)>,
    idx: usize,
    settle: u32,
    last_seq: Option<u64>,
    pub frames: Vec<RawFrame>,
}

/// What the shell should do after feeding a frame.
#[derive(Debug, Clone, PartialEq)]
pub enum Next {
    /// Apply these controls (the next step), then keep feeding.
    Set(Vec<(String, i64)>),
    /// Keep feeding.
    Wait,
    /// All frames are in: apply `restore()`, then merge `frames`.
    Done,
}

impl Sequence {
    /// Plan a sequence. `Err` says what is missing.
    pub fn plan(mode: Mode, ctl: Option<&Controls>) -> Result<Sequence, String> {
        mode.available(ctl)?;
        let mut restore = Vec::new();
        let keep = |c: &Controls, r: Role, restore: &mut Vec<(String, i64)>| {
            if let Some(k) = c.get(r) { restore.push((k.name.clone(), k.value)); }
        };
        let steps: Vec<Vec<(String, i64)>> = match mode {
            Mode::Single | Mode::WbBracket => vec![vec![]],
            Mode::Burst => vec![vec![]; BURST],
            Mode::Hdr => {
                let c = ctl.unwrap();
                keep(c, Role::AutoExposure, &mut restore);
                keep(c, Role::Exposure, &mut restore);
                let e = c.get(Role::Exposure).unwrap();
                let base = e.value.max(e.min.max(1)) as f64;
                HDR_STOPS.iter().map(|s| c.plan_set(Role::Exposure, (base * 2f64.powi(*s)).round() as i64)).collect()
            }
            Mode::Focus => {
                let c = ctl.unwrap();
                keep(c, Role::AutoFocus, &mut restore);
                keep(c, Role::Focus, &mut restore);
                let f = c.get(Role::Focus).unwrap();
                (0..FOCUS_STEPS).map(|i| c.plan_set(Role::Focus, f.min + (f.max - f.min) * i as i64 / (FOCUS_STEPS as i64 - 1))).collect()
            }
        };
        // Restore the automatic control LAST, so a manual value restored
        // after it does not switch auto off again.
        restore.reverse();
        Ok(Sequence { mode, steps, restore, idx: 0, settle: 0, last_seq: None, frames: vec![] })
    }

    /// The first step's controls. Apply them, then feed frames.
    pub fn start(&mut self) -> Vec<(String, i64)> {
        let first = self.steps.first().cloned().unwrap_or_default();
        self.settle = if first.is_empty() { 0 } else { SETTLE_FRAMES };
        first
    }

    pub fn progress(&self) -> (usize, usize) { (self.frames.len(), self.steps.len()) }

    /// Feed a camera frame. The same frame twice is ignored.
    pub fn on_frame(&mut self, f: &RawFrame) -> Next {
        if self.idx >= self.steps.len() { return Next::Done; }
        if self.last_seq == Some(f.seq) { return Next::Wait; }
        self.last_seq = Some(f.seq);
        if self.settle > 0 { self.settle -= 1; return Next::Wait; }
        self.frames.push(f.clone());
        self.idx += 1;
        match self.steps.get(self.idx) {
            None => Next::Done,
            Some(s) if s.is_empty() => Next::Wait,
            Some(s) => { self.settle = SETTLE_FRAMES; Next::Set(s.clone()) }
        }
    }

    /// The controls to put back.
    pub fn restore(&self) -> &[(String, i64)] { &self.restore }
}

/// The WB bracket's three light temperatures, as gains relative to the
/// setting: a picture corrected for 4500K light (cooler), as set, and for
/// 6500K light (warmer).
pub const WB_BRACKET_K: [u32; 3] = [4500, 5500, 6500];
pub fn wb_bracket_gains(base: [f32; 3]) -> [[f32; 3]; 3] {
    let mid = crate::develop::kelvin_gains(5500);
    WB_BRACKET_K.map(|k| { let g = crate::develop::kelvin_gains(k); [0, 1, 2].map(|i| base[i] * g[i] / mid[i]) })
}

/// Human line for a mode's progress.
pub fn status(seq: &Sequence) -> String {
    let (n, of) = seq.progress();
    format!("{}: frame {} of {of}", seq.mode.label(), (n + 1).min(of))
}

/// The labelled shutter speeds of an HDR bracket, for the note.
pub fn hdr_labels(ctl: &Controls) -> String {
    ctl.get(Role::Exposure).map(|e| HDR_STOPS.iter()
        .map(|s| hwctl::shutter_label(((e.value.max(1) as f64) * 2f64.powi(*s)).round() as i64))
        .collect::<Vec<_>>().join(", ")).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctl() -> Controls {
        Controls { device: "/dev/video0".into(), ctrls: hwctl::parse(
            "auto_exposure 0x009a0901 (menu) : min=0 max=3 default=3 value=3\n\
             exposure_time_absolute 0x009a0902 (int) : min=3 max=2047 step=1 default=250 value=200\n\
             focus_absolute 0x009a090a (int) : min=0 max=250 step=5 default=0 value=40\n\
             focus_automatic_continuous 0x009a090c (bool) : default=1 value=1\n") }
    }

    fn frame(seq: u64) -> RawFrame { RawFrame { width: 2, height: 2, captured_ms: seq, seq, rgba: vec![seq as u8; 16].into() } }

    /// Run a sequence to the end, returning the control sets it asked for
    /// and the seqs of the frames it kept.
    fn run(mut s: Sequence) -> (Vec<Vec<(String, i64)>>, Vec<u64>, Sequence) {
        let mut sets = vec![s.start()];
        for seq in 1..200 {
            match s.on_frame(&frame(seq)) {
                Next::Set(p) => sets.push(p),
                Next::Wait => {}
                Next::Done => break,
            }
        }
        let kept = s.frames.iter().map(|f| f.seq).collect();
        (sets, kept, s)
    }

    #[test]
    fn hdr_steps_the_exposure_waits_for_the_sensor_and_restores_auto() {
        let s = Sequence::plan(Mode::Hdr, Some(&ctl())).unwrap();
        let (sets, kept, s) = run(s);
        assert_eq!(sets[0], vec![("auto_exposure".into(), 1), ("exposure_time_absolute".into(), 50)]);
        assert_eq!(sets[1].last().unwrap(), &("exposure_time_absolute".to_string(), 200));
        assert_eq!(sets[2].last().unwrap(), &("exposure_time_absolute".to_string(), 800));
        // Each kept frame comes SETTLE_FRAMES after its control change.
        let gap = SETTLE_FRAMES as u64 + 1;
        assert_eq!(kept, vec![gap, 2 * gap, 3 * gap]);
        assert_eq!(s.restore(), &[("exposure_time_absolute".to_string(), 200), ("auto_exposure".to_string(), 3)],
                   "the manual value first, auto last");
        assert_eq!(hdr_labels(&ctl()), "1/200 s, 1/50 s, 1/12 s");
    }

    #[test]
    fn focus_sweeps_the_range_and_puts_autofocus_back() {
        let (sets, kept, s) = run(Sequence::plan(Mode::Focus, Some(&ctl())).unwrap());
        let pos: Vec<i64> = sets.iter().map(|p| p.last().unwrap().1).collect();
        assert_eq!(pos, vec![0, 60, 125, 185, 250]);
        assert_eq!(kept.len(), FOCUS_STEPS);
        assert_eq!(s.restore().last().unwrap(), &("focus_automatic_continuous".to_string(), 1));
    }

    #[test]
    fn a_burst_takes_consecutive_frames_and_ignores_a_repeat() {
        let mut s = Sequence::plan(Mode::Burst, None).unwrap();
        assert!(s.start().is_empty());
        assert_eq!(s.on_frame(&frame(1)), Next::Wait);
        assert_eq!(s.on_frame(&frame(1)), Next::Wait, "the same frame twice");
        for seq in 2..=BURST as u64 { let _ = s.on_frame(&frame(seq)); }
        assert_eq!(s.frames.len(), BURST);
        assert_eq!(s.on_frame(&frame(99)), Next::Done);
        assert!(s.restore().is_empty());
    }

    #[test]
    fn modes_without_their_control_say_what_is_missing() {
        assert!(Sequence::plan(Mode::Hdr, None).unwrap_err().contains("exposure"));
        assert!(Sequence::plan(Mode::Focus, None).unwrap_err().contains("focus"));
        assert!(Sequence::plan(Mode::Burst, None).is_ok());
        for m in Mode::ALL { assert_eq!(Mode::from_conf(m.to_conf()), m); }
    }
}
