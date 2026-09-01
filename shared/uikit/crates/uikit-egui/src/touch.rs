//! touch — the egui ⇆ touchkit bridge: the multitouch *input half* of the UI kit, shared by every
//! egui front-end. Hold one [`TouchSource`] per gesture surface, feed it each frame's egui input,
//! and get back swipe / fling+momentum / pinch / tap / long-press gestures. One recognizer path
//! serves touchscreen, trackpad, and mouse-drag identically — which is what gives every app the
//! same feel snapclone's photo UI has.

use std::collections::HashMap;

pub use touchkit::{Config, Gesture, GestureRecognizer, Touch, Vec2};

/// Per-surface input bridge. Keep one next to each [`GestureRecognizer`]; egui delivers touches as
/// discrete Start/Move/End *events*, but touchkit wants the complete live pointer set each frame,
/// so this keeps a small live-touch map and snapshots it.
#[derive(Default)]
pub struct TouchSource {
    /// The current contact belongs to a widget — see step 2b in `poll`.
    suppressed: bool,
    active: HashMap<u64, egui::Pos2>,
}

impl TouchSource {
    pub fn new() -> TouchSource {
        TouchSource::default()
    }

    /// Fold this frame's egui input into a batch of [`Gesture`]s. Returns `(gestures, gliding)`;
    /// while `gliding` (fling momentum still ticking) the caller should `ctx.request_repaint()`
    /// so the coast keeps animating with no further input.
    pub fn poll(&mut self, ctx: &egui::Context, rec: &mut GestureRecognizer) -> (Vec<Gesture>, bool) {
        let now_ms = ctx.input(|i| i.time) * 1000.0;

        // 1) live touch set from raw touch events
        ctx.input(|i| {
            for ev in &i.events {
                if let egui::Event::Touch { id, phase, pos, .. } = ev {
                    let tid = id.0;
                    match phase {
                        egui::TouchPhase::Start | egui::TouchPhase::Move => {
                            self.active.insert(tid, *pos);
                        }
                        egui::TouchPhase::End | egui::TouchPhase::Cancel => {
                            self.active.remove(&tid);
                        }
                    }
                }
            }
        });

        // 2) snapshot; fold the mouse in as contact 0 while held, so mouse == 1-finger
        let mut touches: Vec<Touch> = self.active.iter().map(|(&id, p)| Touch::new(id, Vec2::new(p.x, p.y))).collect();
        if touches.is_empty() {
            ctx.input(|i| {
                if i.pointer.primary_down() {
                    if let Some(p) = i.pointer.interact_pos() {
                        touches.push(Touch::new(0, Vec2::new(p.x, p.y)));
                    }
                }
            });
        }

        // 2b) a contact that a WIDGET owns is not a gesture. The same hole
        // `GestureLayer` had: a finger selecting text in a field, or
        // dragging a slider, was a swipe here. While the widget holds it,
        // the recognizer sees no contact and anything it emits on that
        // transition is dropped; the suppression lifts when the contact
        // ends, so the next touch starts clean.
        // A container (a ScrollArea) is NOT a suppression here: pans are
        // what this recognizer produces, and a list scrolling is a pan.
        if !touches.is_empty() && !self.suppressed
            && crate::gesture_layer::drag_owner(ctx, ctx.content_rect()) == crate::gesture_layer::Owner::Widget
        {
            self.suppressed = true;
        }
        let widget_owned = self.suppressed;
        if touches.is_empty() {
            self.suppressed = false;
        } else if widget_owned {
            touches.clear();
        }

        // 3) frame-driven recognizer
        let mut gestures = rec.update(&touches, now_ms);
        if widget_owned {
            gestures.clear();
        }

        // 4) trackpad / wheel → pan + pinch (only when no fingers, so they don't fight)
        if touches.is_empty() {
            let (scroll, zoom, ctrl, ptr) = ctx.input(|i| {
                (
                    i.smooth_scroll_delta,
                    i.zoom_delta(),
                    i.modifiers.ctrl || i.modifiers.command,
                    i.pointer.interact_pos().unwrap_or(egui::Pos2::ZERO),
                )
            });
            let at = Vec2::new(ptr.x, ptr.y);
            if scroll != egui::Vec2::ZERO {
                gestures.extend(rec.wheel(Vec2::new(scroll.x, scroll.y), ctrl, at, now_ms));
            }
            if (zoom - 1.0).abs() > f32::EPSILON {
                let dy = -(zoom.ln()) / 0.01;
                gestures.extend(rec.wheel(Vec2::new(0.0, dy), true, at, now_ms));
            }
        }

        // 5) advance fling momentum
        gestures.extend(rec.momentum(now_ms));
        let gliding = gestures.iter().any(|g| matches!(g, Gesture::MomentumTick { .. }));
        (gestures, gliding)
    }
}

#[cfg(test)]
mod widget_ownership_tests {
    use super::*;
    use egui::{Context, Event, PointerButton, Pos2, RawInput, Rect, Ui};
    use touchkit::Gesture;

    const FIELD: Rect = Rect { min: Pos2::new(40.0, 380.0), max: Pos2::new(360.0, 420.0) };

    /// Press at `from`, drag to `to`, release — through real egui frames
    /// with a focused text field drawn each frame. Every gesture the
    /// bridge emitted, in order.
    fn drag(from: Pos2, to: Pos2) -> Vec<Gesture> {
        let ctx = Context::default();
        let mut src = TouchSource::new();
        let mut rec = GestureRecognizer::default();
        let mut text = String::from("some text to select by dragging across it");
        let mut out = Vec::new();
        let screen = Rect::from_min_size(Pos2::ZERO, egui::vec2(800.0, 800.0));
        let press = |pos: Pos2, pressed: bool| Event::PointerButton {
            pos, button: PointerButton::Primary, pressed, modifiers: Default::default() };
        let mut run = |events: Vec<Event>, t: f64, out: &mut Vec<Gesture>| {
            let _ = ctx.run(RawInput { events, screen_rect: Some(screen), time: Some(t), ..Default::default() }, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui: &mut Ui| {
                    ui.put(FIELD, egui::TextEdit::singleline(&mut text)).request_focus();
                });
                let (g, _) = src.poll(ctx, &mut rec);
                out.extend(g);
            });
        };
        let mut t = 0.0;
        run(vec![Event::PointerMoved(from)], t, &mut out);
        t += 0.016;
        run(vec![press(from, true)], t, &mut out);
        for i in 1..=8 {
            t += 0.016;
            run(vec![Event::PointerMoved(from + (to - from) * (i as f32 / 8.0))], t, &mut out);
        }
        t += 0.016;
        run(vec![press(to, false)], t, &mut out);
        t += 0.016;
        run(vec![], t, &mut out);
        out
    }

    fn moved_the_page(g: &[Gesture]) -> bool {
        g.iter().any(|g| matches!(g, Gesture::PanStart(_) | Gesture::Pan { .. } | Gesture::Fling { .. }))
    }

    #[test]
    fn a_touch_drag_across_a_focused_text_field_is_not_a_gesture() {
        let inside = drag(Pos2::new(300.0, 400.0), Pos2::new(60.0, 400.0));
        assert!(!moved_the_page(&inside), "a text-selection drag produced page gestures: {inside:?}");
        let outside = drag(Pos2::new(300.0, 600.0), Pos2::new(60.0, 600.0));
        assert!(moved_the_page(&outside), "the control drag below the field must still pan or swipe: {outside:?}");
    }
}
