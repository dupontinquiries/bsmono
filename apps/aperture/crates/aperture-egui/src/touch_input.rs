//! touch_input — a finger on the editor, as editing intent.
//!
//! On a phone the editor could not be scrolled. egui turns the first finger
//! into a simulated mouse, and the editor reads a mouse drag as SELECTION
//! (correctly, for a mouse) — so a thumb swipe dragged a selection across
//! the text and the page stayed put. Only the wheel scrolled.
//!
//! This reads egui's raw `Event::Touch` stream, runs it through the shared
//! `touchkit` recognizer, and maps the gestures the way phone editors do:
//!
//! * drag → scroll, 1:1 under the finger; a flick → a coasting glide
//! * tap → caret; double-tap → select the word
//! * long-press → start a selection there; dragging after it EXTENDS the
//!   selection instead of scrolling
//!
//! Headless: `feed` takes events and a clock and returns [`EditAct`]s; the
//! view turns positions into caret offsets. While a touch is in play the
//! view ignores the simulated pointer for this surface (see [`TouchInput::
//! owns_pointer`]), or every tap would also be a click.

use std::collections::BTreeMap;
use touchkit::{Gesture, GestureRecognizer, Touch, Vec2 as TVec2};

#[derive(Clone, Debug, PartialEq)]
pub enum EditAct {
    /// Put the caret here.
    Caret(egui::Pos2),
    /// Select the word here.
    Word(egui::Pos2),
    /// Anchor a selection here (long-press).
    SelectStart(egui::Pos2),
    /// Extend the selection to here.
    SelectTo(egui::Pos2),
    /// Scroll the content by this many points (positive = further down the
    /// file, i.e. the finger moved UP).
    Scroll(f32),
}

/// Per-frame decay of a glide, and the speed (pt/frame) it stops below.
const FRICTION: f32 = 0.93;
const GLIDE_STOP: f32 = 0.6;
/// How long after the last finger lifts the simulated pointer still belongs
/// to touch: egui reports the synthesized click on the release frame or the
/// one after.
const POINTER_GRACE_MS: f64 = 120.0;

#[derive(Default)]
pub struct TouchInput {
    rec: GestureRecognizer,
    /// Fingers that started inside the surface, by id.
    down: BTreeMap<u64, egui::Pos2>,
    /// Fingers that started OUTSIDE it: ignored until they lift.
    ignored: std::collections::BTreeSet<u64>,
    /// Long-press happened: pans now extend the selection.
    selecting: bool,
    /// Coasting speed, pt/frame, in scroll direction.
    glide: f32,
    last_touch_ms: f64,
}

impl TouchInput {
    /// Whether the simulated pointer should be ignored this frame.
    pub fn owns_pointer(&self, now_ms: f64) -> bool {
        !self.down.is_empty() || now_ms - self.last_touch_ms < POINTER_GRACE_MS
    }

    /// Whether frames are still needed (a finger is down — a long-press is
    /// decided by TIME passing — or a glide is coasting).
    pub fn wants_frames(&self) -> bool {
        !self.down.is_empty() || self.glide != 0.0
    }

    /// Consume this frame's events. `inside` is the editor surface.
    pub fn feed(&mut self, events: &[egui::Event], inside: egui::Rect, now_ms: f64) -> Vec<EditAct> {
        let mut changed = false;
        for e in events {
            let egui::Event::Touch { id, phase, pos, .. } = e else { continue };
            let id = id.0;
            changed = true;
            self.last_touch_ms = now_ms;
            match phase {
                egui::TouchPhase::Start => {
                    if inside.contains(*pos) {
                        self.down.insert(id, *pos);
                        self.glide = 0.0; // a finger stops a coasting page
                    } else {
                        self.ignored.insert(id);
                    }
                }
                egui::TouchPhase::Move => {
                    if let Some(p) = self.down.get_mut(&id) {
                        *p = *pos;
                    }
                }
                egui::TouchPhase::End | egui::TouchPhase::Cancel => {
                    self.down.remove(&id);
                    self.ignored.remove(&id);
                }
            }
        }
        let mut out = Vec::new();
        if changed || !self.down.is_empty() {
            let touches: Vec<Touch> = self.down.iter().map(|(id, p)| Touch::new(*id, TVec2::new(p.x, p.y))).collect();
            for g in self.rec.update(&touches, now_ms) {
                self.map(g, &mut out);
            }
            if self.down.is_empty() {
                self.selecting = false;
            }
        }
        if self.glide != 0.0 {
            out.push(EditAct::Scroll(self.glide));
            self.glide *= FRICTION;
            if self.glide.abs() < GLIDE_STOP {
                self.glide = 0.0;
            }
        }
        out
    }

    fn map(&mut self, g: Gesture, out: &mut Vec<EditAct>) {
        let pos = |v: TVec2| egui::pos2(v.x, v.y);
        match g {
            Gesture::Tap(p) => out.push(EditAct::Caret(pos(p))),
            Gesture::DoubleTap(p) => out.push(EditAct::Word(pos(p))),
            Gesture::LongPress(p) => {
                self.selecting = true;
                out.push(EditAct::SelectStart(pos(p)));
            }
            Gesture::Pan { delta, pos: p } => {
                if self.selecting {
                    out.push(EditAct::SelectTo(pos(p)));
                } else {
                    out.push(EditAct::Scroll(-delta.y));
                }
            }
            // A vertical flick is reported as a swipe, not a fling, so both
            // seed the glide. Velocity is px/s; the glide runs per frame.
            Gesture::SwipeUp { vel } if !self.selecting => self.glide = vel.abs() / 60.0,
            Gesture::SwipeDown { vel } if !self.selecting => self.glide = -vel.abs() / 60.0,
            Gesture::Fling { vel } if !self.selecting => self.glide = -vel.y / 60.0,
            // Pinch is egui's `zoom_delta`, already wired to the font size.
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::{pos2, Event, Rect, TouchDeviceId, TouchId, TouchPhase};

    fn t(id: u64, phase: TouchPhase, x: f32, y: f32) -> Event {
        Event::Touch { device_id: TouchDeviceId(0), id: TouchId(id), phase, pos: pos2(x, y), force: None }
    }
    fn area() -> Rect {
        Rect::from_min_size(pos2(0.0, 0.0), egui::vec2(400.0, 800.0))
    }
    /// Drag one finger from y0 to y1 over `frames` frames of 16 ms, from `t0`.
    fn drag(ti: &mut TouchInput, y0: f32, y1: f32, frames: usize, t0: f64) -> Vec<EditAct> {
        let mut acts = ti.feed(&[t(1, TouchPhase::Start, 200.0, y0)], area(), t0);
        for k in 1..=frames {
            let y = y0 + (y1 - y0) * k as f32 / frames as f32;
            acts.extend(ti.feed(&[t(1, TouchPhase::Move, 200.0, y)], area(), t0 + 16.0 * k as f64));
        }
        acts.extend(ti.feed(&[t(1, TouchPhase::End, 200.0, y1)], area(), t0 + 16.0 * (frames + 1) as f64));
        acts
    }
    fn scrolled(acts: &[EditAct]) -> f32 {
        acts.iter().map(|a| if let EditAct::Scroll(d) = a { *d } else { 0.0 }).sum()
    }

    #[test]
    fn a_slow_drag_scrolls_one_to_one_and_selects_nothing() {
        let mut ti = TouchInput::default();
        let acts = drag(&mut ti, 600.0, 400.0, 40, 0.0); // 200 pt up, slowly
        let s = scrolled(&acts);
        assert!((s - 200.0).abs() < 12.0, "finger up 200 -> content 200 further down, got {s}");
        assert!(!acts.iter().any(|a| matches!(a, EditAct::Caret(_) | EditAct::SelectTo(_))));
    }

    #[test]
    fn a_flick_keeps_coasting_after_the_finger_lifts_then_stops() {
        let mut ti = TouchInput::default();
        let during = scrolled(&drag(&mut ti, 600.0, 450.0, 4, 0.0));
        let mut after = 0.0;
        let mut frames = 0;
        while ti.wants_frames() && frames < 600 {
            frames += 1;
            after += scrolled(&ti.feed(&[], area(), 100.0 + 16.0 * frames as f64));
        }
        assert!(after > during * 0.5, "a flick glides on: during {during}, after {after}");
        assert!(frames < 600, "and comes to rest");
    }

    #[test]
    fn a_tap_places_the_caret_and_a_second_tap_selects_the_word() {
        let mut ti = TouchInput::default();
        let mut acts = ti.feed(&[t(1, TouchPhase::Start, 50.0, 60.0)], area(), 0.0);
        acts.extend(ti.feed(&[t(1, TouchPhase::End, 50.0, 60.0)], area(), 80.0));
        assert_eq!(acts, vec![EditAct::Caret(pos2(50.0, 60.0))]);
        let mut acts = ti.feed(&[t(2, TouchPhase::Start, 52.0, 61.0)], area(), 200.0);
        acts.extend(ti.feed(&[t(2, TouchPhase::End, 52.0, 61.0)], area(), 260.0));
        assert!(matches!(acts.as_slice(), [EditAct::Word(_)]), "{acts:?}");
    }

    #[test]
    fn long_press_then_drag_selects_instead_of_scrolling() {
        let mut ti = TouchInput::default();
        let mut acts = ti.feed(&[t(1, TouchPhase::Start, 100.0, 100.0)], area(), 0.0);
        // Held still: frames keep arriving with no events.
        for k in 1..40 {
            acts.extend(ti.feed(&[], area(), 16.0 * k as f64));
        }
        assert!(acts.contains(&EditAct::SelectStart(pos2(100.0, 100.0))), "{acts:?}");
        let mut more = Vec::new();
        for k in 1..=10 {
            more.extend(ti.feed(&[t(1, TouchPhase::Move, 100.0 + 10.0 * k as f32, 100.0 + 8.0 * k as f32)], area(), 640.0 + 16.0 * k as f64));
        }
        more.extend(ti.feed(&[t(1, TouchPhase::End, 200.0, 180.0)], area(), 900.0));
        assert!(more.iter().any(|a| matches!(a, EditAct::SelectTo(_))));
        assert_eq!(scrolled(&more), 0.0, "a selection drag does not scroll");
    }

    #[test]
    fn a_finger_that_started_elsewhere_is_not_ours_and_the_pointer_is_released_after() {
        let mut ti = TouchInput::default();
        let outside = pos2(10.0, 900.0);
        let acts = ti.feed(&[t(7, TouchPhase::Start, outside.x, outside.y)], area(), 0.0);
        assert!(acts.is_empty());
        let acts = ti.feed(&[t(7, TouchPhase::Move, 200.0, 300.0)], area(), 16.0);
        assert!(acts.is_empty(), "moving into the editor does not adopt it");
        ti.feed(&[t(7, TouchPhase::End, 200.0, 300.0)], area(), 32.0);
        assert!(ti.owns_pointer(40.0), "the synthesized click right after a touch is still touch");
        assert!(!ti.owns_pointer(400.0), "a mouse afterwards is a mouse");
    }
}
