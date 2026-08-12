//! ════════════════════════════════════════════════════════════════════════════
//!  ILLUSTRATIVE SKETCH — NOT COMPILED in this environment (needs egui/eframe).
//! ════════════════════════════════════════════════════════════════════════════
//!
//! The voiceterm hand-drawing panel: egui input → PointerRouter → (ink → Canvas,
//! nav → touchkit) → mind-palace save + niftyplot export. This is the whole
//! adapter; everything stateful and testable lives in ink-kit / touchkit.
//!
//! ── Version / platform gotchas (the ones that cost time) ────────────────────
//!
//! * `egui::Event::Touch { force, .. }`: `force` is `Option<f32>` in current
//!   egui (0.27+). Older releases had plain `f32`. Re-check against the pinned
//!   workspace egui.
//! * **Pen input on Linux is the weak spot.** winit exposes styluses as plain
//!   mouse pointers on Wayland/X11 — no pressure, no tool type — as of winit
//!   0.29/0.30 (tablet support has been an open issue for years). So under
//!   eframe on the Fairphone/PinePhone you will usually see a pen as `Mouse`
//!   with `force: None`. Consequences:
//!     - `PointerKind::Pen` is only reliably distinguishable when egui gets
//!       real touch events with force (some Android builds, some touchscreens
//!       whose kernel driver reports pressure via touch).
//!     - For first-class stylus (pressure + palm rejection by tool type),
//!       CADscope's GTK4 shell is the right home: `GtkGestureStylus` +
//!       `gdk::DeviceTool` report pressure/tilt properly through libinput.
//!       The router doesn't care who feeds it — that's why it's headless.
//! * `TouchPhase::Cancel` exists and fires on gesture-steal (e.g. Phosh system
//!   gestures). Treat it as End for the live-map (below) — the router turns a
//!   vanished inking pointer into `InkEvent::End` on its own.
//! * If the panel lives inside a `ScrollArea`, claim the drag: check
//!   `response.dragged()` / use `ui.interact` with `Sense::drag()` and
//!   `ctx.set_dragged_id`, or egui's scroll will fight the ink.
//!
//! ── Interaction contract (mirrors the interop test) ─────────────────────────
//! one finger = draw · two fingers = pan/pinch (touchkit) · pen = always draw,
//! palm rejected · mouse drag = draw · wheel/ctrl+wheel = pan/zoom (touchkit
//! wheel path) · long-press (from touchkit, when nav) = context menu.

use std::collections::HashMap;

use ink_kit::camera::Camera;
use ink_kit::canvas::Canvas;
use ink_kit::filter::{rdp, OneEuro};
use ink_kit::recognize::recognize_graph;
use ink_kit::route::{InkEvent, PointerKind, PointerRouter, RawPointer};
use ink_kit::stroke::{InkPoint, Stroke, StrokeStyle};
use ink_kit::{io as inkio, Vec2f};
use touchkit::{Gesture, GestureRecognizer};

pub struct InkPanel {
    router: PointerRouter,
    nav: GestureRecognizer,
    camera: Camera,
    canvas: Canvas,
    /// Live egui touches (egui gives Start/Move/End events; the router wants
    /// the complete per-frame set — same shape as uikit-egui's TouchSource).
    active: HashMap<u64, (egui::Pos2, Option<f32>)>,
    /// In-progress stroke, screen-space smoothed then world-converted.
    wip: Option<(Stroke, OneEuro)>,
    style: StrokeStyle,
}

impl InkPanel {
    pub fn ui(&mut self, ui: &mut egui::Ui) {
        let rect = ui.available_rect_before_wrap();
        self.camera.viewport = Vec2f::new(rect.width(), rect.height());
        let origin = rect.min; // panel-local coords everywhere below

        // 1) Collect this frame's live pointers.
        let now_ms = ui.ctx().input(|i| i.time) * 1000.0;
        ui.ctx().input(|i| {
            for ev in &i.events {
                if let egui::Event::Touch { id, phase, pos, force } = ev {
                    match phase {
                        egui::TouchPhase::Start | egui::TouchPhase::Move => {
                            self.active.insert(id.0, (*pos, *force));
                        }
                        egui::TouchPhase::End | egui::TouchPhase::Cancel => {
                            self.active.remove(&id.0);
                        }
                    }
                }
            }
        });
        let mut live: Vec<RawPointer> = self
            .active
            .iter()
            .filter(|(_, (p, _))| rect.contains(**p))
            .map(|(&id, &(p, force))| RawPointer {
                id,
                pos: Vec2f::new(p.x - origin.x, p.y - origin.y),
                pressure: force,
                // Heuristic until a real tool-type source exists (see header):
                // pressure-reporting contact ⇒ treat as pen.
                kind: if force.is_some() { PointerKind::Pen } else { PointerKind::Touch },
            })
            .collect();
        if live.is_empty() {
            ui.ctx().input(|i| {
                if i.pointer.primary_down() {
                    if let Some(p) = i.pointer.interact_pos() {
                        if rect.contains(p) {
                            live.push(RawPointer {
                                id: 0,
                                pos: Vec2f::new(p.x - origin.x, p.y - origin.y),
                                pressure: None,
                                kind: PointerKind::Mouse,
                            });
                        }
                    }
                }
            });
        }

        // 2) Route.
        let frame = self.router.update(&live, now_ms);

        // 3) Ink side: screen-space smooth, then convert to world.
        for ev in frame.ink {
            match ev {
                InkEvent::Begin { tool, pt } => {
                    let mut style = self.style;
                    style.tool = tool;
                    // world-unit width so strokes zoom with the canvas
                    style.width = self.style.width / self.camera.zoom;
                    let id = self.canvas.alloc_id();
                    let mut s = Stroke::new(id, style);
                    let mut filt = OneEuro::new();
                    s.points.push(self.to_world_pt(&mut filt, pt));
                    self.wip = Some((s, filt));
                }
                InkEvent::Move { pt } => {
                    if let Some((s, filt)) = &mut self.wip {
                        let wp = self.to_world_pt_inner(filt, pt, &self.camera);
                        s.points.push(wp);
                    }
                }
                InkEvent::End => {
                    if let Some((mut s, _)) = self.wip.take() {
                        // simplify at ~0.75 screen px tolerance, in world units
                        s.points = rdp(&s.points, 0.75 / self.camera.zoom);
                        self.canvas.add(s);
                    }
                }
                InkEvent::Cancel => {
                    self.wip = None;
                }
            }
        }

        // 4) Nav side: feed touchkit; wheel path too (only when no contacts).
        let touches: Vec<touchkit::Touch> = frame
            .nav
            .iter()
            .map(|(id, p)| touchkit::Touch::new(*id, touchkit::Vec2::new(p.x, p.y)))
            .collect();
        for g in self.nav.update(&touches, now_ms) {
            match g {
                Gesture::Pan { delta, .. } => self.camera.pan_screen(Vec2f::new(delta.x, delta.y)),
                Gesture::Pinch { rel, center, .. } => {
                    self.camera.zoom_about(rel as f64, Vec2f::new(center.x, center.y))
                }
                Gesture::MomentumTick { delta } => {
                    self.camera.pan_screen(Vec2f::new(delta.x, delta.y))
                }
                _ => {}
            }
        }
        // (wheel + momentum request_repaint plumbing: identical to
        //  uikit-egui::touch::TouchSource::poll — reuse that shape.)

        // 5) Paint: visible strokes + WIP, world→screen through the camera.
        let painter = ui.painter_at(rect);
        let view = self.camera.visible_world();
        for s in self.canvas.visible(&view) {
            self.paint_stroke(&painter, origin, s);
        }
        if let Some((s, _)) = &self.wip {
            self.paint_stroke(&painter, origin, s);
        }
    }

    fn paint_stroke(&self, painter: &egui::Painter, origin: egui::Pos2, s: &Stroke) {
        let pts: Vec<egui::Pos2> = s
            .points
            .iter()
            .map(|p| {
                let sp = self.camera.to_screen(p.pos);
                egui::pos2(origin.x + sp.x, origin.y + sp.y)
            })
            .collect();
        let w = (s.style.width * self.camera.zoom) as f32;
        let c = s.style.color;
        painter.add(egui::Shape::line(
            pts,
            egui::Stroke::new(w.max(0.5), egui::Color32::from_rgba_unmultiplied(c[0], c[1], c[2], c[3])),
        ));
    }

    // helpers: smooth in SCREEN space (sensor noise lives there), then to world
    fn to_world_pt(&mut self, filt: &mut OneEuro, pt: InkPoint) -> InkPoint {
        Self::to_world_pt_static(filt, pt, &self.camera)
    }
    fn to_world_pt_inner(&self, filt: &mut OneEuro, pt: InkPoint, cam: &Camera) -> InkPoint {
        Self::to_world_pt_static(filt, pt, cam)
    }
    fn to_world_pt_static(filt: &mut OneEuro, pt: InkPoint, cam: &Camera) -> InkPoint {
        let smoothed = filt.filter(pt.t_ms, pt.pos); // router put screen px in pos
        let world = cam.to_world(Vec2f::new(smoothed.x as f32, smoothed.y as f32));
        InkPoint { pos: world, pressure: pt.pressure, t_ms: pt.t_ms }
    }

    // ── Mind-palace + logscope integration ─────────────────────────────────
    /// Save the drawing next to the current note: `<note>.ink` sits beside
    /// `<note>.md`, and the note gets a ZBracket link so the graph engine
    /// (voiceterm note_graph) sees the sketch as a neighbor.
    pub fn save_to_note(&self, note_dir: &std::path::Path, note_name: &str) -> std::io::Result<()> {
        let strokes: Vec<Stroke> = self.canvas.iter().cloned().collect();
        std::fs::write(note_dir.join(format!("{note_name}.ink")), inkio::emit(&strokes))
    }

    /// Recognize boxes/arrows and produce a niftyplot block for logscope —
    /// paste-ready fenced source; logscope's existing niftyplot renderer does
    /// the rest.
    pub fn export_niftyplot(&self) -> String {
        let strokes: Vec<Stroke> = self.canvas.iter().cloned().collect();
        let sketch = recognize_graph(&strokes, 0.35);
        format!("```niftyplot\n{}```\n", sketch.to_niftyplot())
    }
}
