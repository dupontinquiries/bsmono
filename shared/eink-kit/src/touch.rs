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
use std::time::Duration;

pub const EV_SYN: u16 = 0x00;
pub const EV_ABS: u16 = 0x03;
pub const SYN_REPORT: u16 = 0;
pub const SYN_DROPPED: u16 = 3;
pub const ABS_MT_SLOT: u16 = 0x2f;
pub const ABS_MT_POSITION_X: u16 = 0x35;
pub const ABS_MT_POSITION_Y: u16 = 0x36;
pub const ABS_MT_TRACKING_ID: u16 = 0x39;
pub const EV_KEY: u16 = 0x01;
pub const ABS_X: u16 = 0x00;
pub const ABS_Y: u16 = 0x01;
pub const ABS_PRESSURE: u16 = 0x18;
pub const BTN_TOOL_PEN: u16 = 0x140;
pub const BTN_TOOL_RUBBER: u16 = 0x141;
pub const BTN_TOUCH: u16 = 0x14a;
/// Touch id used for the pen, so it never collides with a finger slot.
pub const PEN_ID: u32 = 1000;

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

/// The pen digitizer: one tool, absolute axes, `BTN_TOUCH` for contact.
///
/// Emits touch events (id [`PEN_ID`]) only while the tip is DOWN. Hover is
/// reported by the digitizer continuously; forwarding it would move egui's
/// pointer on every hover sample and each move can cost a panel refresh, so
/// hover is tracked (for calibration displays) but never emitted.
pub struct PenDecoder {
    pub x_range: AbsRange,
    pub y_range: AbsRange,
    pub width: u32,
    pub height: u32,
    pub transform: Transform,
    raw_x: i32,
    raw_y: i32,
    touching: bool,
    was_touching: bool,
    moved: bool,
    /// Eraser end in use (BTN_TOOL_RUBBER).
    pub eraser: bool,
    pub last_raw: Option<(i32, i32)>,
}

impl PenDecoder {
    pub fn new(x_range: AbsRange, y_range: AbsRange, width: u32, height: u32, transform: Transform) -> PenDecoder {
        PenDecoder { x_range, y_range, width, height, transform, raw_x: 0, raw_y: 0, touching: false, was_touching: false, moved: false, eraser: false, last_raw: None }
    }

    fn map(&self) -> (f32, f32) {
        // Same normalise → swap → invert → scale as the touch decoder.
        let m = MtDecoder::new(self.x_range, self.y_range, self.width, self.height, self.transform);
        m.map(self.raw_x, self.raw_y)
    }

    pub fn feed(&mut self, ty: u16, code: u16, value: i32, out: &mut Vec<TouchEvent>) {
        match (ty, code) {
            (EV_ABS, ABS_X) => { self.raw_x = value; self.moved = true; }
            (EV_ABS, ABS_Y) => { self.raw_y = value; self.moved = true; }
            (EV_KEY, BTN_TOUCH) => self.touching = value != 0,
            (EV_KEY, BTN_TOOL_RUBBER) => self.eraser = value != 0,
            (EV_KEY, BTN_TOOL_PEN) if value == 0 => self.touching = false, // tool left range
            (EV_SYN, SYN_REPORT) => {
                self.last_raw = Some((self.raw_x, self.raw_y));
                let (x, y) = self.map();
                let phase = match (self.was_touching, self.touching) {
                    (false, true) => Some(Phase::Start),
                    (true, true) if self.moved => Some(Phase::Move),
                    (true, false) => Some(Phase::End),
                    _ => None, // hover, or down and still
                };
                if let Some(phase) = phase {
                    out.push(TouchEvent { id: PEN_ID, phase, x, y });
                }
                self.was_touching = self.touching;
                self.moved = false;
            }
            (EV_SYN, SYN_DROPPED) => {
                if self.was_touching {
                    out.push(TouchEvent { id: PEN_ID, phase: Phase::End, x: 0.0, y: 0.0 });
                }
                self.touching = false;
                self.was_touching = false;
            }
            _ => {}
        }
    }
}

/// A contact released after this long with no event at all. A resting palm
/// goes silent; so does a touch whose release was never delivered.
pub const STUCK_TIMEOUT: Duration = Duration::from_millis(2500);
/// A pointer-holding contact still for this long yields the pointer to a new
/// contact (palm resting, finger drawing).
pub const HANDOVER_STILL: Duration = Duration::from_millis(400);
/// Where a pointer is moved before an involuntary release, so the release
/// can never be a click on whatever was under the contact.
const FAR: Pos2 = Pos2::new(-100_000.0, -100_000.0);

#[derive(Debug, Clone, Copy)]
struct Contact {
    id: u32,
    last_event: Duration,
    last_move: Duration,
}

/// Touch → egui, with pointer emulation for one contact at a time.
///
/// Hardened against what AppLoad actually forwards (rm-appload
/// FBController::touchEvent, seen on a Paper Pro with a resting hand):
/// * a STATIONARY point is forwarded as a PRESS, re-sent whenever any other
///   point moves: a PRESS for a contact already down is treated as a move;
/// * cancelled / ungrabbed touches produce no RELEASE: a contact silent for
///   [`STUCK_TIMEOUT`] is released (see [`TouchToEgui::expire`]);
/// * an UPDATE may arrive for a contact never pressed: treated as a start.
///
/// And for multi-hand use: the pen always takes the pointer; a finger takes
/// it from a contact that has been still for [`HANDOVER_STILL`] (a resting
/// palm). Every involuntary release moves the pointer far away first, so it
/// never clicks what was under the palm.
#[derive(Default)]
pub struct TouchToEgui {
    primary: Option<u32>,
    last_pos: Pos2,
    down: Vec<Contact>,
}

impl TouchToEgui {
    pub fn any_down(&self) -> bool {
        !self.down.is_empty()
    }

    /// When the next stuck contact would be released, if any are down.
    pub fn next_expiry(&self) -> Option<Duration> {
        self.down.iter().map(|c| c.last_event + STUCK_TIMEOUT).min()
    }

    /// Release contacts silent for [`STUCK_TIMEOUT`]. Returns whether any were.
    pub fn expire(&mut self, now: Duration, out: &mut Vec<Event>) -> bool {
        let stale: Vec<u32> = self.down.iter().filter(|c| now.saturating_sub(c.last_event) >= STUCK_TIMEOUT).map(|c| c.id).collect();
        for id in &stale {
            out.push(Event::Touch { device_id: TouchDeviceId(0), id: TouchId(*id as u64), phase: TouchPhase::Cancel, pos: FAR, force: None });
            self.down.retain(|c| c.id != *id);
            if self.primary == Some(*id) {
                self.cancel_pointer(out);
            }
        }
        !stale.is_empty()
    }

    /// Let go of the pointer without a click.
    fn cancel_pointer(&mut self, out: &mut Vec<Event>) {
        out.push(Event::PointerMoved(FAR));
        out.push(Event::PointerButton { pos: FAR, button: PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE });
        out.push(Event::PointerGone);
        self.primary = None;
    }

    fn take_pointer(&mut self, id: u32, pos: Pos2, out: &mut Vec<Event>) {
        self.primary = Some(id);
        self.last_pos = pos;
        out.push(Event::PointerMoved(pos));
        out.push(Event::PointerButton { pos, button: PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE });
    }

    /// `ppp` converts panel pixels to egui points; `now` is the engine clock.
    pub fn translate(&mut self, t: &TouchEvent, ppp: f32, now: Duration, out: &mut Vec<Event>) {
        let pos = Pos2::new(t.x / ppp, t.y / ppp);
        let known = self.down.iter().any(|c| c.id == t.id);
        // Normalise AppLoad's forwarding: a PRESS for a known contact is a
        // stationary re-send (a move); a MOVE for an unknown one is a start.
        let phase = match (t.phase, known) {
            (Phase::Start, true) => Phase::Move,
            (Phase::Move, false) => Phase::Start,
            (p, _) => p,
        };
        let tphase = match phase {
            Phase::Start => TouchPhase::Start,
            Phase::Move => TouchPhase::Move,
            Phase::End => TouchPhase::End,
        };
        out.push(Event::Touch { device_id: TouchDeviceId(0), id: TouchId(t.id as u64), phase: tphase, pos, force: None });
        match phase {
            Phase::Start => {
                self.down.push(Contact { id: t.id, last_event: now, last_move: now });
                let handover = match self.primary {
                    None => true,
                    Some(p) if t.id == PEN_ID && p != PEN_ID => true,
                    Some(p) => p != PEN_ID && self.down.iter().find(|c| c.id == p)
                        .is_some_and(|c| now.saturating_sub(c.last_move) >= HANDOVER_STILL),
                };
                if handover {
                    if self.primary.is_some() {
                        self.cancel_pointer(out);
                    }
                    self.take_pointer(t.id, pos, out);
                }
            }
            Phase::Move => {
                let moved = self.primary == Some(t.id) && (pos - self.last_pos).length() > 0.5
                    || self.primary != Some(t.id);
                if let Some(c) = self.down.iter_mut().find(|c| c.id == t.id) {
                    c.last_event = now;
                    // a stationary re-send (same spot) is not movement
                    if moved && t.phase == Phase::Move {
                        c.last_move = now;
                    }
                }
                if self.primary == Some(t.id) && t.phase == Phase::Move {
                    self.last_pos = pos;
                    out.push(Event::PointerMoved(pos));
                }
            }
            Phase::End => {
                self.down.retain(|c| c.id != t.id);
                if self.primary == Some(t.id) {
                    self.primary = None;
                    // Release where the contact last was: a real lift.
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

    fn pen() -> PenDecoder {
        PenDecoder::new(AbsRange { min: 0, max: 20966 }, AbsRange { min: 0, max: 15725 }, 1404, 1872, Transform::default())
    }

    #[test]
    fn pen_hover_is_silent_and_contact_is_a_touch() {
        let mut p = pen();
        let mut out = vec![];
        // tool enters range and hovers: nothing
        for (t, c, v) in [(EV_KEY, BTN_TOOL_PEN, 1), (EV_ABS, ABS_X, 1000), (EV_ABS, ABS_Y, 1000), (EV_SYN, SYN_REPORT, 0),
                          (EV_ABS, ABS_X, 1200), (EV_SYN, SYN_REPORT, 0)] {
            p.feed(t, c, v, &mut out);
        }
        assert!(out.is_empty(), "hover must not emit: {out:?}");
        assert!(p.last_raw.is_some(), "hover is still tracked for calibration");
        // tip down, move, up
        for (t, c, v) in [(EV_KEY, BTN_TOUCH, 1), (EV_ABS, ABS_PRESSURE, 900), (EV_SYN, SYN_REPORT, 0),
                          (EV_ABS, ABS_X, 2000), (EV_SYN, SYN_REPORT, 0),
                          (EV_ABS, ABS_PRESSURE, 400), (EV_SYN, SYN_REPORT, 0), // pressure only: not a move
                          (EV_KEY, BTN_TOUCH, 0), (EV_SYN, SYN_REPORT, 0)] {
            p.feed(t, c, v, &mut out);
        }
        let phases: Vec<Phase> = out.iter().map(|e| e.phase).collect();
        assert_eq!(phases, vec![Phase::Start, Phase::Move, Phase::End]);
        assert!(out.iter().all(|e| e.id == PEN_ID));
    }

    #[test]
    fn pen_leaving_range_while_down_ends_the_stroke() {
        let mut p = pen();
        let mut out = vec![];
        for (t, c, v) in [(EV_KEY, BTN_TOOL_PEN, 1), (EV_KEY, BTN_TOUCH, 1), (EV_SYN, SYN_REPORT, 0),
                          (EV_KEY, BTN_TOOL_PEN, 0), (EV_SYN, SYN_REPORT, 0)] {
            p.feed(t, c, v, &mut out);
        }
        assert_eq!(out.last().map(|e| e.phase), Some(Phase::End));
    }

    #[test]
    fn pen_and_finger_share_the_pointer_first_come_first_served() {
        let mut t = TouchToEgui::default();
        let mut out = vec![];
        t.translate(&TouchEvent { id: PEN_ID, phase: Phase::Start, x: 100.0, y: 100.0 }, 2.0, Duration::ZERO, &mut out);
        t.translate(&TouchEvent { id: 0, phase: Phase::Start, x: 900.0, y: 900.0 }, 2.0, Duration::ZERO, &mut out);
        let moves: Vec<_> = out.iter().filter_map(|e| if let Event::PointerMoved(p) = e { Some(*p) } else { None }).collect();
        assert_eq!(moves, vec![Pos2::new(50.0, 50.0)], "the palm/finger must not steal the pointer from the pen");
    }

    #[test]
    fn first_finger_drives_the_pointer_and_lift_clears_hover() {
        let mut t = TouchToEgui::default();
        let mut out = vec![];
        t.translate(&TouchEvent { id: 0, phase: Phase::Start, x: 20.0, y: 40.0 }, 2.0, Duration::ZERO, &mut out);
        assert!(out.contains(&Event::PointerMoved(Pos2::new(10.0, 20.0))));
        assert!(out.iter().any(|e| matches!(e, Event::PointerButton { pressed: true, .. })));
        out.clear();
        // a second finger does not move the pointer
        t.translate(&TouchEvent { id: 1, phase: Phase::Start, x: 200.0, y: 400.0 }, 2.0, Duration::ZERO, &mut out);
        assert!(!out.iter().any(|e| matches!(e, Event::PointerMoved(_))));
        out.clear();
        t.translate(&TouchEvent { id: 0, phase: Phase::End, x: 0.0, y: 0.0 }, 2.0, Duration::ZERO, &mut out);
        assert!(out.iter().any(|e| matches!(e, Event::PointerButton { pressed: false, pos, .. } if *pos == Pos2::new(10.0, 20.0))));
        assert_eq!(out.last(), Some(&Event::PointerGone));
        assert!(t.any_down(), "finger 1 is still down");
    }

    fn ms(v: u64) -> Duration { Duration::from_millis(v) }
    fn ev(id: u32, phase: Phase, x: f32, y: f32) -> TouchEvent { TouchEvent { id, phase, x, y } }
    fn clicked_at(out: &[Event]) -> Option<Pos2> {
        out.iter().rev().find_map(|e| match e { Event::PointerButton { pressed: false, pos, .. } => Some(*pos), _ => None })
    }

    #[test]
    fn appload_resends_a_resting_palm_as_press_and_that_is_not_a_new_touch() {
        let mut t = TouchToEgui::default();
        let mut out = vec![];
        t.translate(&ev(7, Phase::Start, 100.0, 100.0), 2.0, ms(0), &mut out);
        out.clear();
        // palm stationary: AppLoad re-sends it as PRESS each time another point moves
        for i in 1..5 { t.translate(&ev(7, Phase::Start, 100.0, 100.0), 2.0, ms(i * 50), &mut out); }
        assert!(!out.iter().any(|e| matches!(e, Event::PointerButton { .. })), "no re-press/release: {out:?}");
        assert_eq!(t.down.len(), 1);
    }

    #[test]
    fn a_lost_release_is_expired_without_a_click() {
        let mut t = TouchToEgui::default();
        let mut out = vec![];
        t.translate(&ev(3, Phase::Start, 200.0, 200.0), 2.0, ms(0), &mut out);
        assert_eq!(t.next_expiry(), Some(STUCK_TIMEOUT));
        out.clear();
        assert!(!t.expire(ms(1000), &mut out));
        assert!(t.expire(ms(2600), &mut out));
        assert!(!t.any_down());
        assert_eq!(clicked_at(&out), Some(FAR), "released far away, never on the widget under it");
        assert_eq!(out.last(), Some(&Event::PointerGone));
    }

    #[test]
    fn a_finger_takes_the_pointer_from_a_resting_palm() {
        let mut t = TouchToEgui::default();
        let mut out = vec![];
        t.translate(&ev(1, Phase::Start, 50.0, 50.0), 2.0, ms(0), &mut out); // palm lands first
        t.translate(&ev(1, Phase::Start, 50.0, 50.0), 2.0, ms(300), &mut out); // stationary re-send
        out.clear();
        t.translate(&ev(2, Phase::Start, 800.0, 800.0), 2.0, ms(600), &mut out); // drawing finger
        assert_eq!(t.primary, Some(2), "the still palm yields");
        assert_eq!(clicked_at(&out), Some(FAR), "palm released without a click");
        assert!(out.contains(&Event::PointerMoved(Pos2::new(400.0, 400.0))));
        // a moving primary is NOT taken over
        let mut t2 = TouchToEgui::default();
        t2.translate(&ev(1, Phase::Start, 50.0, 50.0), 2.0, ms(0), &mut out);
        t2.translate(&ev(1, Phase::Move, 80.0, 80.0), 2.0, ms(500), &mut out);
        t2.translate(&ev(2, Phase::Start, 800.0, 800.0), 2.0, ms(600), &mut out);
        assert_eq!(t2.primary, Some(1));
    }

    #[test]
    fn the_pen_always_takes_the_pointer() {
        let mut t = TouchToEgui::default();
        let mut out = vec![];
        t.translate(&ev(1, Phase::Start, 50.0, 50.0), 2.0, ms(0), &mut out);
        t.translate(&ev(1, Phase::Move, 60.0, 60.0), 2.0, ms(10), &mut out); // palm moving
        t.translate(&ev(PEN_ID, Phase::Start, 500.0, 500.0), 2.0, ms(20), &mut out);
        assert_eq!(t.primary, Some(PEN_ID));
    }

    #[test]
    fn an_update_for_an_unknown_contact_starts_it() {
        let mut t = TouchToEgui::default();
        let mut out = vec![];
        t.translate(&ev(9, Phase::Move, 10.0, 10.0), 2.0, ms(0), &mut out);
        assert!(t.any_down());
        assert!(out.iter().any(|e| matches!(e, Event::PointerButton { pressed: true, .. })));
    }
}
