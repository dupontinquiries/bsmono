//! ════════════════════════════════════════════════════════════════════════════
//!  ILLUSTRATIVE SKETCH — NOT COMPILED in this environment (needs egui + eframe).
//! ════════════════════════════════════════════════════════════════════════════
//!
//! This is the whole bridge between egui's per-frame input and `touchkit`: about
//! fifty lines. Everything else in touchkit is UI-agnostic, which is the point —
//! the *same* recognizer drives snapclone's photo lightbox and logscope's strand
//! timeline (see the other two sketches), and a future Qt/winit front-end would
//! write an adapter exactly this size.
//!
//! API names are shown against egui ~0.28. egui's input surface moves slowly, but
//! these are the field/enum names to re-check against your pinned version:
//!   - `egui::Context::input(|i| …)` and the `InputState` fields used below
//!   - `egui::Event::Touch { id, phase, pos, force }`, `egui::TouchPhase`
//!   - `i.raw_scroll_delta: Vec2`, `i.zoom_delta() -> f32`
//!   - `i.pointer.{primary_down, interact_pos}`, `i.modifiers.{ctrl, command}`
//!
//! Why an adapter at all? egui delivers touches as discrete Start/Move/End
//! *events*, but `touchkit` takes the *complete live set of pointers each frame*
//! (the immediate-mode shape). So the adapter keeps a small live-touch map and
//! snapshots it per frame.

use std::collections::HashMap;
use touchkit::{Gesture, GestureRecognizer, Touch, Vec2};

/// Per-surface input bridge. Hold one next to each `GestureRecognizer`.
#[derive(Default)]
pub struct TouchSource {
    /// Live touch points, updated from egui's Start/Move/End events.
    active: HashMap<u64, egui::Pos2>,
}

impl TouchSource {
    /// Fold this frame's egui input into a batch of [`Gesture`]s.
    ///
    /// Returns `(gestures, gliding)`. `gliding` is true while fling momentum is
    /// still producing ticks — the caller should then `ctx.request_repaint()` so
    /// the coast keeps animating with no further user input.
    pub fn poll(
        &mut self,
        ctx: &egui::Context,
        rec: &mut GestureRecognizer,
    ) -> (Vec<Gesture>, bool) {
        // egui clock is seconds; touchkit thinks in milliseconds.
        let now_ms = ctx.input(|i| i.time) * 1000.0;

        // 1) Update the live touch set from raw touch events.
        ctx.input(|i| {
            for ev in &i.events {
                if let egui::Event::Touch { id, phase, pos, .. } = ev {
                    let tid = id.0; // egui::TouchId(u64)
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

        // 2) Snapshot the live set. If there are no real touches, fold the mouse in
        //    as contact id 0 while a button is held — so one gesture path serves
        //    touchscreen, trackpad drag, and mouse drag identically.
        let mut touches: Vec<Touch> = self
            .active
            .iter()
            .map(|(&id, p)| Touch::new(id, Vec2::new(p.x, p.y)))
            .collect();
        if touches.is_empty() {
            ctx.input(|i| {
                if i.pointer.primary_down() {
                    if let Some(p) = i.pointer.interact_pos() {
                        touches.push(Touch::new(0, Vec2::new(p.x, p.y)));
                    }
                }
            });
        }

        // 3) The frame-driven core call.
        let mut gestures = rec.update(&touches, now_ms);

        // 4) Trackpad / wheel. Two-finger scroll -> pan (+ flick-swipe); ctrl+wheel
        //    or a pinch zoom_delta -> Pinch. Skip while fingers are down so a
        //    touch-drag and a wheel event don't fight in the same frame.
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
                // egui hands us a multiplicative zoom factor; touchkit::wheel maps
                // ctrl+Δy back to exactly that factor (exp(-Δy*0.01)), so invert
                // here to keep a single pinch code path inside touchkit.
                let dy = -(zoom.ln()) / 0.01;
                gestures.extend(rec.wheel(Vec2::new(0.0, dy), true, at, now_ms));
            }
        }

        // 5) Advance fling momentum (no-op unless a Fling seeded it).
        gestures.extend(rec.momentum(now_ms));
        let gliding = gestures
            .iter()
            .any(|g| matches!(g, Gesture::MomentumTick { .. }));

        (gestures, gliding)
    }
}
