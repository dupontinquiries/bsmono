//! How a photo gets taken, beyond one tap: triggers that prime the
//! shutter, bracket sequences, the merge worker, the WB bracket, and the
//! cover calibration. The shell's `ui` calls `tick_capture` once a frame.

use super::Cam;
use crate::bracket::{self, Mode, Next, Sequence};
use crate::prime::Prime;
use crate::triggers::{self, KeyWatch, Keys, ShakeWatch, Trigger};
use crate::{cover, develop, stack};
use capture_kit::RawFrame;
use std::sync::mpsc::Receiver;

/// Which triggers are on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TriggerSettings { pub volume: bool, pub power: bool, pub shake: bool }

/// The capture-flow state the shell holds.
#[derive(Default)]
pub struct Flow {
    pub triggers: TriggerSettings,
    keys: Option<KeyWatch>,
    shake: Option<ShakeWatch>,
    /// What opening the triggers found, for the panel.
    pub report: Vec<String>,
    pub primed: Option<(Prime, Trigger)>,
    pub mode: Mode,
    pub sequence: Option<Sequence>,
    /// The merge running on its thread: the result, and what it was.
    merging: Option<(Receiver<Result<RawFrame, String>>, String)>,
    /// The controls a sequence was started from, for the note.
    pub last_merge: Option<String>,
}

impl Flow {
    pub fn busy(&self) -> bool { self.primed.is_some() || self.sequence.is_some() || self.merging.is_some() }
    pub fn status(&self, now_ms: f64) -> Option<String> {
        if let Some((p, t)) = &self.primed { return Some(format!("{} — holding for the sharpest frame, {:.1}s", t.label(), p.remaining(now_ms))); }
        if let Some(s) = &self.sequence { return Some(bracket::status(s)); }
        self.merging.as_ref().map(|(_, what)| format!("merging {what}…"))
    }
}

fn now_ms(ctx: &egui::Context) -> f64 { ctx.input(|i| i.time) * 1000.0 }

impl Cam {
    /// (Re)open the trigger sources to match the settings.
    pub(super) fn apply_triggers(&mut self) {
        let t = self.flow.triggers;
        self.flow.keys = None;
        self.flow.shake = None;
        self.flow.report.clear();
        if t.volume || t.power {
            let text = std::fs::read_to_string("/proc/bus/input/devices").unwrap_or_default();
            let w = KeyWatch::start(&text, std::path::Path::new("/dev/input"), Keys { volume: t.volume, power: t.power });
            self.flow.report.extend(w.report.clone());
            self.flow.keys = Some(w);
        }
        if t.shake {
            let root = std::env::var("SNAPSHOT_CAM_SYSFS_ROOT").unwrap_or_else(|_| "/".into());
            match triggers::find_accel(std::path::Path::new(&root)) {
                Some(a) => { self.flow.report.push(format!("shake: {}", a.dir.display())); self.flow.shake = Some(ShakeWatch::start(a)); }
                None => self.flow.report.push("shake: no accelerometer (IIO in_accel_*) on this device".into()),
            }
        }
    }

    /// A trigger fired, or the shutter was pressed in a multi-shot mode.
    pub fn trigger(&mut self, ctx: &egui::Context, t: Trigger) {
        if self.flow.busy() || !self.camera.is_live() { return; }
        match self.flow.mode {
            Mode::Single => { self.flow.primed = Some((Prime::new(now_ms(ctx)), t)); }
            _ => self.start_sequence(),
        }
    }

    pub fn start_sequence(&mut self) {
        if self.flow.busy() { return; }
        match Sequence::plan(self.flow.mode, self.hw.controls.as_ref()) {
            Ok(mut s) => {
                let first = s.start();
                if let Some(ctl) = &self.hw.controls { if !first.is_empty() { let d = ctl.device.clone(); self.apply_hw(&d, &first); } }
                self.flow.last_merge = self.hw.controls.as_ref().filter(|_| s.mode == Mode::Hdr).map(bracket::hdr_labels);
                self.note = bracket::status(&s);
                self.flow.sequence = Some(s);
            }
            Err(e) => self.note = e,
        }
    }

    /// Once a frame: triggers, the primed window, the sequence, the merge.
    pub(super) fn tick_capture(&mut self, ctx: &egui::Context) {
        let mut fired: Vec<Trigger> = Vec::new();
        if let Some(k) = &self.flow.keys { fired.extend(k.rx.try_iter()); }
        if let Some(s) = &self.flow.shake { fired.extend(s.rx.try_iter()); }
        if let Some(t) = fired.first() { self.trigger(ctx, *t); }
        let now = now_ms(ctx);
        if self.flow.busy() { ctx.request_repaint(); }

        // PRIMED: score every new frame; the best one when the second ends.
        if let Some((p, _)) = &mut self.flow.primed {
            if let Some(f) = self.camera.snapshot() { p.feed(&f); }
        }
        if self.flow.primed.as_ref().is_some_and(|(p, _)| p.done(now)) {
            let (p, t) = self.flow.primed.take().unwrap();
            let n = p.scored;
            match p.take() {
                Some(f) => self.save_frame(ctx, f, None, Some(format!("{}, sharpest of {n} frames", t.label()))),
                None => self.note = "primed, but no frame arrived in the second".into(),
            }
        }

        // SEQUENCE: feed frames, apply each step's controls.
        if let Some(mut s) = self.flow.sequence.take() {
            let mut keep = true;
            if let Some(f) = self.camera.snapshot() {
                match s.on_frame(&f) {
                    Next::Set(pairs) => {
                        if let Some(ctl) = &self.hw.controls { let d = ctl.device.clone(); self.apply_hw(&d, &pairs); }
                        self.note = bracket::status(&s);
                    }
                    Next::Wait => {}
                    Next::Done => {
                        keep = false;
                        if let Some(ctl) = &self.hw.controls {
                            if !s.restore().is_empty() { let d = ctl.device.clone(); let r = s.restore().to_vec(); self.apply_hw(&d, &r); }
                        }
                        self.finish_sequence(ctx, s.clone());
                    }
                }
            }
            if keep { self.flow.sequence = Some(s); }
        }

        // MERGE: the worker's answer.
        if let Some((rx, what)) = &self.flow.merging {
            if let Ok(r) = rx.try_recv() {
                let what = what.clone();
                self.flow.merging = None;
                match r {
                    Ok(f) => self.save_frame(ctx, f, None, Some(what)),
                    Err(e) => self.note = format!("merge failed: {e}"),
                }
            }
        }
    }

    fn finish_sequence(&mut self, ctx: &egui::Context, s: Sequence) {
        let frames = s.frames;
        if frames.is_empty() { self.note = "no frames were taken".into(); return; }
        if s.mode == Mode::Single { self.save_frame(ctx, frames[0].clone(), None, None); return; }
        if s.mode == Mode::WbBracket {
            let base = self.pro.gains();
            for (g, k) in bracket::wb_bracket_gains(base).into_iter().zip(bracket::WB_BRACKET_K) {
                self.save_frame(ctx, frames[0].clone(), Some(g), Some(format!("WB bracket, as for {k}K light")));
            }
            return;
        }
        let n = frames.len();
        let what = match (s.mode, &self.flow.last_merge) {
            (Mode::Hdr, Some(l)) => format!("HDR from {n} exposures ({l})"),
            (Mode::Focus, _) => format!("focus stack of {n}"),
            (Mode::Burst, _) => format!("burst merge of {n}"),
            _ => format!("{} of {n}", s.mode.label()),
        };
        let mode = s.mode;
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::Builder::new().name("merge".into()).spawn(move || {
            let (w, h) = (frames[0].width, frames[0].height);
            if frames.iter().any(|f| (f.width, f.height) != (w, h) || !f.pixels_plausible()) {
                let _ = tx.send(Err("the frames are not all the same size".into()));
                return;
            }
            let refs: Vec<&[u8]> = frames.iter().map(|f| &f.rgba[..]).collect();
            let st = stack::Frames { w, h, rgba: refs };
            let px = match mode { Mode::Hdr => stack::fuse_exposures(&st), Mode::Focus => stack::stack_focus(&st), _ => stack::average(&st) };
            let _ = tx.send(Ok(RawFrame { width: w, height: h, captured_ms: frames[0].captured_ms, seq: frames[0].seq, rgba: px.into() }));
        }).ok();
        self.note = format!("merging {what}…");
        self.flow.merging = Some((rx, what));
    }

    /// Measure the cover from the current frame (a grey card in the
    /// middle, something dark somewhere).
    pub(super) fn calibrate_cover(&mut self) {
        let Some(f) = self.camera.snapshot() else { self.note = "no frame to measure — the camera is not live".into(); return };
        let (w, h) = develop::fit_size((f.width, f.height), 320, 320);
        let small = develop::render(&f.rgba, f.width, f.height, &develop::Params::identity(w, h));
        match cover::calibrate(&small, w, h) {
            Ok(cal) => {
                self.pro.cover = cal.profile;
                self.pro.cover_on = true;
                self.settings_dirty = true;
                let saved = std::fs::create_dir_all(config_kit::config_dir(crate::store::APP))
                    .and_then(|_| config_kit::write_atomic(&cover::path(), &cal.profile.render()));
                self.note = format!("cover measured: {}{}", cal.notes.join(" · "),
                                    saved.err().map(|e| format!(" (not saved: {e})")).unwrap_or_default());
            }
            Err(e) => self.note = format!("cover not measured: {e}"),
        }
    }
}

/// Settings-file lines for the flow.
pub fn conf(f: &Flow) -> String {
    format!("mode = {}\nvolume_keys = {}\npower_key = {}\nshake = {}\n", f.mode.to_conf(), f.triggers.volume, f.triggers.power, f.triggers.shake)
}

pub fn apply_conf(f: &mut Flow, get: impl Fn(&str) -> Option<String>) {
    if let Some(m) = get("mode") { f.mode = Mode::from_conf(&m); }
    let b = |k: &str| get(k).is_some_and(|v| v == "true");
    f.triggers = TriggerSettings { volume: b("volume_keys"), power: b("power_key"), shake: b("shake") };
}
//! How a photo gets taken, beyond one tap: triggers that prime the
//! shutter, bracket sequences, the merge worker, the WB bracket, and the
//! cover calibration. The shell's `ui` calls `tick_capture` once a frame.

use super::Cam;
use crate::bracket::{self, Mode, Next, Sequence};
use crate::prime::Prime;
use crate::triggers::{self, KeyWatch, Keys, ShakeWatch, Trigger};
use crate::{cover, develop, stack};
use capture_kit::RawFrame;
use std::sync::mpsc::Receiver;

/// Which triggers are on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TriggerSettings { pub volume: bool, pub power: bool, pub shake: bool }

/// The capture-flow state the shell holds.
#[derive(Default)]
pub struct Flow {
    pub triggers: TriggerSettings,
    keys: Option<KeyWatch>,
    shake: Option<ShakeWatch>,
    /// What opening the triggers found, for the panel.
    pub report: Vec<String>,
    pub primed: Option<(Prime, Trigger)>,
    pub mode: Mode,
    pub sequence: Option<Sequence>,
    /// The merge running on its thread: the result, and what it was.
    merging: Option<(Receiver<Result<RawFrame, String>>, String)>,
    /// The controls a sequence was started from, for the note.
    pub last_merge: Option<String>,
}

impl Flow {
    pub fn busy(&self) -> bool { self.primed.is_some() || self.sequence.is_some() || self.merging.is_some() }
    pub fn status(&self, now_ms: f64) -> Option<String> {
        if let Some((p, t)) = &self.primed { return Some(format!("{} — holding for the sharpest frame, {:.1}s", t.label(), p.remaining(now_ms))); }
        if let Some(s) = &self.sequence { return Some(bracket::status(s)); }
        self.merging.as_ref().map(|(_, what)| format!("merging {what}…"))
    }
}

fn now_ms(ctx: &egui::Context) -> f64 { ctx.input(|i| i.time) * 1000.0 }

impl Cam {
    /// (Re)open the trigger sources to match the settings.
    pub(super) fn apply_triggers(&mut self) {
        let t = self.flow.triggers;
        self.flow.keys = None;
        self.flow.shake = None;
        self.flow.report.clear();
        if t.volume || t.power {
            let text = std::fs::read_to_string("/proc/bus/input/devices").unwrap_or_default();
            let w = KeyWatch::start(&text, std::path::Path::new("/dev/input"), Keys { volume: t.volume, power: t.power });
            self.flow.report.extend(w.report.clone());
            self.flow.keys = Some(w);
        }
        if t.shake {
            let root = std::env::var("SNAPSHOT_CAM_SYSFS_ROOT").unwrap_or_else(|_| "/".into());
            match triggers::find_accel(std::path::Path::new(&root)) {
                Some(a) => { self.flow.report.push(format!("shake: {}", a.dir.display())); self.flow.shake = Some(ShakeWatch::start(a)); }
                None => self.flow.report.push("shake: no accelerometer (IIO in_accel_*) on this device".into()),
            }
        }
    }

    /// A trigger fired, or the shutter was pressed in a multi-shot mode.
    pub fn trigger(&mut self, ctx: &egui::Context, t: Trigger) {
        if self.flow.busy() || !self.camera.is_live() { return; }
        match self.flow.mode {
            Mode::Single => { self.flow.primed = Some((Prime::new(now_ms(ctx)), t)); }
            _ => self.start_sequence(),
        }
    }

    pub fn start_sequence(&mut self) {
        if self.flow.busy() { return; }
        match Sequence::plan(self.flow.mode, self.hw.controls.as_ref()) {
            Ok(mut s) => {
                let first = s.start();
                if let Some(ctl) = &self.hw.controls { if !first.is_empty() { let d = ctl.device.clone(); self.apply_hw(&d, &first); } }
                self.flow.last_merge = self.hw.controls.as_ref().filter(|_| s.mode == Mode::Hdr).map(bracket::hdr_labels);
                self.note = bracket::status(&s);
                self.flow.sequence = Some(s);
            }
            Err(e) => self.note = e,
        }
    }

    /// Once a frame: triggers, the primed window, the sequence, the merge.
    pub(super) fn tick_capture(&mut self, ctx: &egui::Context) {
        let mut fired: Vec<Trigger> = Vec::new();
        if let Some(k) = &self.flow.keys { fired.extend(k.rx.try_iter()); }
        if let Some(s) = &self.flow.shake { fired.extend(s.rx.try_iter()); }
        if let Some(t) = fired.first() { self.trigger(ctx, *t); }
        let now = now_ms(ctx);
        if self.flow.busy() { ctx.request_repaint(); }

        // PRIMED: score every new frame; the best one when the second ends.
        if let Some((p, _)) = &mut self.flow.primed {
            if let Some(f) = self.camera.snapshot() { p.feed(&f); }
        }
        if self.flow.primed.as_ref().is_some_and(|(p, _)| p.done(now)) {
            let (p, t) = self.flow.primed.take().unwrap();
            let n = p.scored;
            match p.take() {
                Some(f) => self.save_frame(ctx, f, None, Some(format!("{}, sharpest of {n} frames", t.label()))),
                None => self.note = "primed, but no frame arrived in the second".into(),
            }
        }

        // SEQUENCE: feed frames, apply each step's controls.
        if let Some(mut s) = self.flow.sequence.take() {
            let mut keep = true;
            if let Some(f) = self.camera.snapshot() {
                match s.on_frame(&f) {
                    Next::Set(pairs) => {
                        if let Some(ctl) = &self.hw.controls { let d = ctl.device.clone(); self.apply_hw(&d, &pairs); }
                        self.note = bracket::status(&s);
                    }
                    Next::Wait => {}
                    Next::Done => {
                        keep = false;
                        if let Some(ctl) = &self.hw.controls {
                            if !s.restore().is_empty() { let d = ctl.device.clone(); let r = s.restore().to_vec(); self.apply_hw(&d, &r); }
                        }
                        self.finish_sequence(ctx, s.clone());
                    }
                }
            }
            if keep { self.flow.sequence = Some(s); }
        }

        // MERGE: the worker's answer.
        if let Some((rx, what)) = &self.flow.merging {
            if let Ok(r) = rx.try_recv() {
                let what = what.clone();
                self.flow.merging = None;
                match r {
                    Ok(f) => self.save_frame(ctx, f, None, Some(what)),
                    Err(e) => self.note = format!("merge failed: {e}"),
                }
            }
        }
    }

    fn finish_sequence(&mut self, ctx: &egui::Context, s: Sequence) {
        let frames = s.frames;
        if frames.is_empty() { self.note = "no frames were taken".into(); return; }
        if s.mode == Mode::Single { self.save_frame(ctx, frames[0].clone(), None, None); return; }
        if s.mode == Mode::WbBracket {
            let base = self.pro.gains();
            for (g, k) in bracket::wb_bracket_gains(base).into_iter().zip(bracket::WB_BRACKET_K) {
                self.save_frame(ctx, frames[0].clone(), Some(g), Some(format!("WB bracket, as for {k}K light")));
            }
            return;
        }
        let n = frames.len();
        let what = match (s.mode, &self.flow.last_merge) {
            (Mode::Hdr, Some(l)) => format!("HDR from {n} exposures ({l})"),
            (Mode::Focus, _) => format!("focus stack of {n}"),
            (Mode::Burst, _) => format!("burst merge of {n}"),
            _ => format!("{} of {n}", s.mode.label()),
        };
        let mode = s.mode;
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::Builder::new().name("merge".into()).spawn(move || {
            let (w, h) = (frames[0].width, frames[0].height);
            if frames.iter().any(|f| (f.width, f.height) != (w, h) || !f.pixels_plausible()) {
                let _ = tx.send(Err("the frames are not all the same size".into()));
                return;
            }
            let refs: Vec<&[u8]> = frames.iter().map(|f| &f.rgba[..]).collect();
            let st = stack::Frames { w, h, rgba: refs };
            let px = match mode { Mode::Hdr => stack::fuse_exposures(&st), Mode::Focus => stack::stack_focus(&st), _ => stack::average(&st) };
            let _ = tx.send(Ok(RawFrame { width: w, height: h, captured_ms: frames[0].captured_ms, seq: frames[0].seq, rgba: px.into() }));
        }).ok();
        self.note = format!("merging {what}…");
        self.flow.merging = Some((rx, what));
    }

    /// Measure the cover from the current frame (a grey card in the
    /// middle, something dark somewhere).
    pub(super) fn calibrate_cover(&mut self) {
        let Some(f) = self.camera.snapshot() else { self.note = "no frame to measure — the camera is not live".into(); return };
        let (w, h) = develop::fit_size((f.width, f.height), 320, 320);
        let small = develop::render(&f.rgba, f.width, f.height, &develop::Params::identity(w, h));
        match cover::calibrate(&small, w, h) {
            Ok(cal) => {
                self.pro.cover = cal.profile;
                self.pro.cover_on = true;
                self.settings_dirty = true;
                let saved = std::fs::create_dir_all(config_kit::config_dir(crate::store::APP))
                    .and_then(|_| config_kit::write_atomic(&cover::path(), &cal.profile.render()));
                self.note = format!("cover measured: {}{}", cal.notes.join(" · "),
                                    saved.err().map(|e| format!(" (not saved: {e})")).unwrap_or_default());
            }
            Err(e) => self.note = format!("cover not measured: {e}"),
        }
    }
}

/// Settings-file lines for the flow.
pub fn conf(f: &Flow) -> String {
    format!("mode = {}\nvolume_keys = {}\npower_key = {}\nshake = {}\n", f.mode.to_conf(), f.triggers.volume, f.triggers.power, f.triggers.shake)
}

pub fn apply_conf(f: &mut Flow, get: impl Fn(&str) -> Option<String>) {
    if let Some(m) = get("mode") { f.mode = Mode::from_conf(&m); }
    let b = |k: &str| get(k).is_some_and(|v| v == "true");
    f.triggers = TriggerSettings { volume: b("volume_keys"), power: b("power_key"), shake: b("shake") };
}
