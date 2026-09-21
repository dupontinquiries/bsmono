//! Finger input: from raw kernel events to egui events.
//!
//! Two stages, each testable on its own:
//!
//! 1. [`MtDecoder`] turns Linux multitouch protocol-B events (slots +
//!    tracking ids, committed on `SYN_REPORT`) into [`TouchEvent`]s in
//!    PANEL pixels. It is fed by any source of `input_event`s: a
//!    `/dev/input/eventN` file, or Blight's per-device ring buffer. qtfb
//!    does not need it: qtfb forwards touches already in framebuffer pixels.
//! 2. [`TouchToEgui`] turns touch events into egui events. egui widgets
//!    react to the POINTER, not to `Event::Touch` (which feeds multi-touch
//!    gestures like pinch). So the first finger down also drives a
//!    synthetic pointer: move + primary press on start, move while held,
//!    release + `PointerGone` on lift. `PointerGone` matters on a touch
//!    panel: without it the last touched widget stays "hovered" forever.
//!
//! ## Calibration
//!
//! The digitizer's axes are not guaranteed to line up with the panel's.
//! [`Transform`] (swap / invert) is applied after normalising by the
//! axis ranges, and comes from `EINK_TOUCH=swap,invx,invy` so it can be
//! fixed on the device without a rebuild. The demo app shows raw and mapped
//! coordinates to make finding the right flags a one-minute job.

use egui::{Event, Modifiers, PointerButton, Pos2, TouchDeviceId, TouchId, TouchPhase};

pub const EV_SYN: u16 = 0x00;
pub const EV_ABS: u16 = 0x03;
pub const SYN_REPORT: u16 = 0;
pub const SYN_DROPPED: u16 = 3;
pub const ABS_MT_SLOT: u16 = 0x2f;
pub const ABS_MT_POSITION_X: u16 = 0x35;
pub const ABS_MT_POSITION_Y: u16 = 0x36;
pub const ABS_MT_TRACKING_ID: u16 = 0x39;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Phase {
    Start,
    Move,
    End,
}

/// A finger event in panel pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TouchEvent {
    pub id: u32,
    pub phase: Phase,
    pub x: f32,
    pub y: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Transform {
    pub swap_xy: bool,
    pub invert_x: bool,
    pub invert_y: bool,
}

impl Transform {
    /// Parse `swap,invx,invy` (any subset, any order; empty = identity).
    pub fn parse(s: &str) -> Result<Transform, String> {
        let mut t = Transform::default();
        for part in s.split(',').map(str::trim).filter(|p| !p.is_empty()) {
            match part {
                "swap" => t.swap_xy = true,
                "invx" => t.invert_x = true,
                "invy" => t.invert_y = true,
                other => return Err(format!("EINK_TOUCH: unknown flag {other:?} (want swap,invx,invy)")),
            }
        }
        Ok(t)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AbsRange {
    pub min: i32,
    pub max: i32,
}

impl AbsRange {
    fn norm(&self, v: i32) -> f32 {
        let span = (self.max - self.min).max(1) as f32;
        ((v - self.min) as f32 / span).clamp(0.0, 1.0)
    }
}

#[derive(Debug, Clone, Copy, Default)]
struct Slot {
    tracking: i32,
    raw_x: i32,
    raw_y: i32,
    changed: bool,
    active: bool,
}

/// Protocol-B decoder. One per touch device.
pub struct MtDecoder {
    slots: Vec<Slot>,
    cur: usize,
    pub x_range: AbsRange,
    pub y_range: AbsRange,
    pub width: u32,
    pub height: u32,
    pub transform: Transform,
    /// Last raw (x, y) seen, for calibration displays.
    pub last_raw: Option<(i32, i32)>,
}

impl MtDecoder {
    pub fn new(x_range: AbsRange, y_range: AbsRange, width: u32, height: u32, transform: Transform) -> MtDecoder {
        MtDecoder {
            slots: vec![Slot { tracking: -1, ..Default::default() }; 16],
            cur: 0,
            x_range,
            y_range,
            width,
            height,
            transform,
            last_raw: None,
        }
    }

    pub fn map(&self, raw_x: i32, raw_y: i32) -> (f32, f32) {
        let (mut u, mut v) = (self.x_range.norm(raw_x), self.y_range.norm(raw_y));
        if self.transform.swap_xy {
            std::mem::swap(&mut u, &mut v);
        }
        if self.transform.invert_x {
            u = 1.0 - u;
        }
        if self.transform.invert_y {
            v = 1.0 - v;
        }
        (u * (self.width.saturating_sub(1)) as f32, v * (self.height.saturating_sub(1)) as f32)
    }

    /// Feed one kernel event; completed touch events are appended to `out`.
    pub fn feed(&mut self, ty: u16, code: u16, value: i32, out: &mut Vec<TouchEvent>) {
        match (ty, code) {
            (EV_ABS, ABS_MT_SLOT) => {
                self.cur = (value.max(0) as usize).min(self.slots.len() - 1);
            }
            (EV_ABS, ABS_MT_TRACKING_ID) => {
                let s = &mut self.slots[self.cur];
                s.tracking = value;
                s.changed = true;
            }
            (EV_ABS, ABS_MT_POSITION_X) => {
                let s = &mut self.slots[self.cur];
                s.raw_x = value;
                s.changed = true;
            }
            (EV_ABS, ABS_MT_POSITION_Y) => {
                let s = &mut self.slots[self.cur];
                s.raw_y = value;
                s.changed = true;
            }
            (EV_SYN, SYN_REPORT) => self.commit(out),
            (EV_SYN, SYN_DROPPED) => {
                // Events were lost: we cannot trust slot state. End every
                // active touch so no widget is left held down.
                for (i, s) in self.slots.iter_mut().enumerate() {
                    if s.active {
                        let (x, y) = (0.0, 0.0);
                        out.push(TouchEvent { id: i as u32, phase: Phase::End, x, y });
                    }
                    *s = Slot { tracking: -1, ..Default::default() };
                }
            }
            _ => {}
        }
    }

    fn commit(&mut self, out: &mut Vec<TouchEvent>) {
        for i in 0..self.slots.len() {
            let s = self.slots[i];
            if !s.changed {
                continue;
            }
            let (x, y) = self.map(s.raw_x, s.raw_y);
            let ev = match (s.active, s.tracking >= 0) {
                (false, true) => Some(Phase::Start),
                (true, true) => Some(Phase::Move),
                (true, false) => Some(Phase::End),
                (false, false) => None,
            };
            if let Some(phase) = ev {
                out.push(TouchEvent { id: i as u32, phase, x, y });
                self.last_raw = Some((s.raw_x, s.raw_y));
            }
            let s = &mut self.slots[i];
            s.active = s.tracking >= 0;
            s.changed = false;
        }
    }
}

/// Touch → egui, with pointer emulation for the first finger.
#[derive(Default)]
pub struct TouchToEgui {
    primary: Option<u32>,
    last_pos: Pos2,
    /// Fingers currently down.
    down: Vec<u32>,
}

impl TouchToEgui {
    pub fn any_down(&self) -> bool {
        !self.down.is_empty()
    }

    /// `ppp` converts panel pixels to egui points.
    pub fn translate(&mut self, t: &TouchEvent, ppp: f32, out: &mut Vec<Event>) {
        let pos = Pos2::new(t.x / ppp, t.y / ppp);
        let phase = match t.phase {
            Phase::Start => TouchPhase::Start,
            Phase::Move => TouchPhase::Move,
            Phase::End => TouchPhase::End,
        };
        out.push(Event::Touch { device_id: TouchDeviceId(0), id: TouchId(t.id as u64), phase, pos, force: None });
        match t.phase {
            Phase::Start => {
                if !self.down.contains(&t.id) {
                    self.down.push(t.id);
                }
                if self.primary.is_none() {
                    self.primary = Some(t.id);
                    self.last_pos = pos;
                    out.push(Event::PointerMoved(pos));
                    out.push(Event::PointerButton { pos, button: PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE });
                }
            }
            Phase::Move => {
                if self.primary == Some(t.id) {
                    self.last_pos = pos;
                    out.push(Event::PointerMoved(pos));
                }
            }
            Phase::End => {
                self.down.retain(|d| *d != t.id);
                if self.primary == Some(t.id) {
                    self.primary = None;
                    // Release where the finger last was: an End after
                    // SYN_DROPPED carries no trustworthy position.
                    let pos = self.last_pos;
                    out.push(Event::PointerButton { pos, button: PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE });
                    out.push(Event::PointerGone);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dec(t: Transform) -> MtDecoder {
        MtDecoder::new(AbsRange { min: 0, max: 1000 }, AbsRange { min: 0, max: 2000 }, 1001, 2001, t)
    }
    fn feed(d: &mut MtDecoder, evs: &[(u16, u16, i32)]) -> Vec<TouchEvent> {
        let mut out = vec![];
        for &(t, c, v) in evs {
            d.feed(t, c, v, &mut out);
        }
        out
    }

    #[test]
    fn protocol_b_press_move_release() {
        let mut d = dec(Transform::default());
        let a = feed(&mut d, &[(EV_ABS, ABS_MT_SLOT, 0), (EV_ABS, ABS_MT_TRACKING_ID, 7),
                               (EV_ABS, ABS_MT_POSITION_X, 500), (EV_ABS, ABS_MT_POSITION_Y, 1000), (EV_SYN, SYN_REPORT, 0)]);
        assert_eq!(a, vec![TouchEvent { id: 0, phase: Phase::Start, x: 500.0, y: 1000.0 }]);
        let b = feed(&mut d, &[(EV_ABS, ABS_MT_POSITION_X, 600), (EV_SYN, SYN_REPORT, 0)]);
        assert_eq!(b[0].phase, Phase::Move);
        assert_eq!(b[0].x, 600.0);
        let c = feed(&mut d, &[(EV_ABS, ABS_MT_TRACKING_ID, -1), (EV_SYN, SYN_REPORT, 0)]);
        assert_eq!(c[0].phase, Phase::End);
        assert!(feed(&mut d, &[(EV_SYN, SYN_REPORT, 0)]).is_empty(), "nothing changed");
    }

    #[test]
    fn two_fingers_in_separate_slots() {
        let mut d = dec(Transform::default());
        let a = feed(&mut d, &[(EV_ABS, ABS_MT_SLOT, 0), (EV_ABS, ABS_MT_TRACKING_ID, 1), (EV_ABS, ABS_MT_POSITION_X, 10),
                               (EV_ABS, ABS_MT_SLOT, 1), (EV_ABS, ABS_MT_TRACKING_ID, 2), (EV_ABS, ABS_MT_POSITION_X, 900),
                               (EV_SYN, SYN_REPORT, 0)]);
        assert_eq!(a.len(), 2);
        assert_eq!((a[0].id, a[1].id), (0, 1));
    }

    #[test]
    fn transform_swaps_then_inverts() {
        let d = dec(Transform::parse("swap,invx").unwrap());
        // raw x at max, raw y at 0 → after swap u=0,v=1 → invx u=1
        let (x, y) = d.map(1000, 0);
        assert_eq!((x, y), (1000.0, 2000.0));
        assert!(Transform::parse("rotate").is_err());
    }

    #[test]
    fn dropped_events_release_everything() {
        let mut d = dec(Transform::default());
        feed(&mut d, &[(EV_ABS, ABS_MT_TRACKING_ID, 3), (EV_SYN, SYN_REPORT, 0)]);
        let e = feed(&mut d, &[(EV_SYN, SYN_DROPPED, 0)]);
        assert_eq!(e.len(), 1);
        assert_eq!(e[0].phase, Phase::End);
    }

    #[test]
    fn first_finger_drives_the_pointer_and_lift_clears_hover() {
        let mut t = TouchToEgui::default();
        let mut out = vec![];
        t.translate(&TouchEvent { id: 0, phase: Phase::Start, x: 20.0, y: 40.0 }, 2.0, &mut out);
        assert!(out.contains(&Event::PointerMoved(Pos2::new(10.0, 20.0))));
        assert!(out.iter().any(|e| matches!(e, Event::PointerButton { pressed: true, .. })));
        out.clear();
        // a second finger does not move the pointer
        t.translate(&TouchEvent { id: 1, phase: Phase::Start, x: 200.0, y: 400.0 }, 2.0, &mut out);
        assert!(!out.iter().any(|e| matches!(e, Event::PointerMoved(_))));
        out.clear();
        t.translate(&TouchEvent { id: 0, phase: Phase::End, x: 0.0, y: 0.0 }, 2.0, &mut out);
        assert!(out.iter().any(|e| matches!(e, Event::PointerButton { pressed: false, pos, .. } if *pos == Pos2::new(10.0, 20.0))));
        assert_eq!(out.last(), Some(&Event::PointerGone));
        assert!(t.any_down(), "finger 1 is still down");
    }
}
