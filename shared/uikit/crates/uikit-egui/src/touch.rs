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

        // 3) frame-driven recognizer
        let mut gestures = rec.update(&touches, now_ms);

        // 4) trackpad / wheel → pan + pinch (only when no fingers, so they don't fight)
        if touches.is_empty() {
            let (scroll, zoom, ctrl, ptr) = ctx.input(|i| {
                (
                    i.raw_scroll_delta,
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
