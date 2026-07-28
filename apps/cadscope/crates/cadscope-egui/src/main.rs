//! cadscope — the M0 shell. Thin by decree: the kernel owns geometry, the forge owns history, the
//! view owns GPU, motion owns keys; this file is the wiring. What M0 proves end-to-end:
//!
//!   forge Doc → kernel Model → viewport draws (skydribble pass inside egui-wgpu)
//!                          ↘ SDF pick under the pointer = the same shapes the mesh shows
//!
//! Touch story at M0: one-finger drag orbits, two-finger/right drag pans, pinch or scroll dollies,
//! tap picks. (uikit-egui gesture_layer momentum + the FP5-tuned two-tier recognizer wire in at
//! M2 — PARITY.md tracks it.) Keyboard: cad-motion Normal mode — hjkl orbit, +/- dolly, f frame,
//! n/N cycle, g Grow (stub), Esc clear.

use cad_forge::{Doc, Op, OpWarning, ShapeSpec};
use cad_kernel::{Model, Sdf};
use cad_motion::{Action, Mode};
use cad_view::camera::{ndc_of, OrbitCamera};
use cad_view::renderer::{FrameDraw, Globals, MeshKind, ObjectRaw, ViewRenderer};
use eframe::egui;
use glam::{Mat4, Quat, Vec2, Vec3};

fn main() -> eframe::Result<()> {
    env_logger::init();
    let options = eframe::NativeOptions {
        renderer: eframe::Renderer::Wgpu,
        // 32 → Depth32Float on egui's pass (egui_wgpu::depth_format_from_bits) — the format the
        // ViewRenderer pipeline declares. Skydribble parity; see cad-view/src/renderer.rs.
        depth_buffer: 32,
        viewport: egui::ViewportBuilder::default().with_inner_size([1280.0, 820.0]),
        ..Default::default()
    };
    eframe::run_native("CADscope", options, Box::new(|cc| Ok(Box::new(App::new(cc)))))
}

/// The M0 demo document: primitives only (the viewport meshes primitives; CSG surfaces arrive
/// with the M1 mesher — kernel CSG is already exercised by `cargo test -p cad-kernel`).
fn demo_doc() -> Doc {
    let add = |name: &str, shape: ShapeSpec, at: [f32; 3], rot: Option<([f32; 3], f32)>, color: [f32; 4]| Op::Add {
        name: name.into(),
        shape,
        at,
        rot_axis_deg: rot,
        color,
    };
    Doc {
        name: "m0-bench".into(),
        ops: vec![
            add("base", ShapeSpec::Cuboid { half: [7.0, 1.0, 5.0] }, [0.0, 1.0, 0.0], None, [0.62, 0.66, 0.72, 1.0]),
            add("pillar", ShapeSpec::Cylinder { r: 1.4, hh: 3.2 }, [-3.2, 5.2, 0.0], None, [0.82, 0.55, 0.28, 1.0]),
            add("knob", ShapeSpec::Sphere { r: 1.8 }, [-3.2, 10.1, 0.0], None, [0.85, 0.30, 0.30, 1.0]),
            add("ring", ShapeSpec::Torus { major: 2.6, minor: 0.7 }, [3.4, 4.6, 0.0], Some(([1.0, 0.0, 0.0], 90.0)), [0.32, 0.62, 0.85, 1.0]),
            add("handle", ShapeSpec::Capsule { r: 0.8, hh: 2.2 }, [3.4, 4.6, 0.0], Some(([0.0, 0.0, 1.0], 90.0)), [0.45, 0.78, 0.50, 1.0]),
        ],
    }
}

struct App {
    doc: Doc,
    model: Model,
    warns: Vec<OpWarning>,
    cam: OrbitCamera,
    selected: Option<u32>,
    mode: Mode,
    start: std::time::Instant,
}

impl App {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let rs = cc
            .wgpu_render_state
            .as_ref()
            .expect("CADscope needs the wgpu backend (NativeOptions.renderer = Wgpu)");
        rs.renderer.write().callback_resources.insert(ViewRenderer::new(
            &rs.device,
            rs.target_format,
            Some(wgpu::TextureFormat::Depth32Float), // pairs with depth_buffer: 32 above
            1,                                       // eframe default msaa; 4x is a PARITY debt
        ));

        let doc = demo_doc();
        let (model, warns) = doc.apply();
        let mut cam = OrbitCamera::default();
        cam.frame(Vec3::new(0.0, 4.0, 0.0), model.bound_radius().min(60.0));
        App { doc, model, warns, cam, selected: None, mode: Mode::Normal, start: std::time::Instant::now() }
    }

    fn act(&mut self, a: Action) {
        match a {
            Action::OrbitLeft => self.cam.orbit(-0.14, 0.0),
            Action::OrbitRight => self.cam.orbit(0.14, 0.0),
            Action::OrbitUp => self.cam.orbit(0.0, 0.10),
            Action::OrbitDown => self.cam.orbit(0.0, -0.10),
            Action::DollyIn => self.cam.dolly(0.85),
            Action::DollyOut => self.cam.dolly(1.18),
            Action::FrameSelection => match self.selected.and_then(|id| self.model.part(id)) {
                Some(p) => {
                    let (center, _, _) = unwrap_placement(&p.sdf);
                    self.cam.frame(center, p.sdf.bound_radius() - center.length() + 2.0);
                }
                None => self.cam.frame(Vec3::new(0.0, 4.0, 0.0), self.model.bound_radius().min(60.0)),
            },
            Action::SelectNext | Action::SelectPrev => {
                let ids: Vec<u32> = self.model.parts.iter().map(|p| p.id).collect();
                if ids.is_empty() {
                    return;
                }
                let cur = self.selected.and_then(|s| ids.iter().position(|&i| i == s));
                let next = match (cur, a) {
                    (Some(i), Action::SelectNext) => (i + 1) % ids.len(),
                    (Some(i), Action::SelectPrev) => (i + ids.len() - 1) % ids.len(),
                    _ => 0,
                };
                self.selected = Some(ids[next]);
            }
            Action::ClearSelection => self.selected = None,
            Action::EnterMode(m) => self.mode = m,
        }
    }

    fn viewport(&mut self, ui: &mut egui::Ui) {
        let (rect, response) = ui.allocate_exact_size(ui.available_size(), egui::Sense::click_and_drag());
        let aspect = rect.width() / rect.height().max(1.0);

        // ── input: orbit / pan / dolly / pick ────────────────────────────────────────────────
        let drag = response.drag_delta();
        if response.dragged_by(egui::PointerButton::Primary) {
            self.cam.orbit(drag.x * 0.008, drag.y * 0.008);
        }
        if response.dragged_by(egui::PointerButton::Secondary)
            || response.dragged_by(egui::PointerButton::Middle)
        {
            self.cam.pan(Vec2::new(drag.x / rect.width(), drag.y / rect.height()), aspect);
        }
        let (scroll, zoom) = ui.input(|i| (i.smooth_scroll_delta.y, i.zoom_delta()));
        if response.hovered() && scroll.abs() > 0.0 {
            self.cam.dolly((1.0f32 - scroll * 0.0016).clamp(0.5, 1.6));
        }
        if (zoom - 1.0).abs() > 1e-4 {
            self.cam.dolly((1.0 / zoom).clamp(0.5, 1.8)); // pinch
        }

        let (vp, inv_vp) = self.cam.view_proj(aspect);
        if response.clicked() {
            if let Some(pos) = response.interact_pointer_pos() {
                let ray = self.cam.screen_ray(ndc_of(pos, rect), &inv_vp);
                self.selected = self.model.pick(&ray, 4000.0).map(|p| p.part_id);
            }
        }

        // ── frame payload: globals + ground + one draw per part ─────────────────────────────
        let eye = self.cam.eye();
        let t = self.start.elapsed().as_secs_f32();
        let sun = Vec3::new(0.45, 0.80, 0.35).normalize();
        let globals = Globals {
            view_proj: vp.to_cols_array(),
            inv_view_proj: inv_vp.to_cols_array(),
            cam: [eye.x, eye.y, eye.z, t],
            sun: [sun.x, sun.y, sun.z, 1.10],
            fog: [0.74, 0.77, 0.82, 0.0028],
            misc: [self.selected.map_or(0.0, |id| id as f32 + 1.0), 0.0, 0.0, 0.0],
        };

        let mut draws = Vec::with_capacity(self.model.parts.len() + 1);
        draws.push(FrameDraw {
            kind: MeshKind::GroundSlab,
            object: object_raw(
                Mat4::from_scale_rotation_translation(Vec3::new(90.0, 0.12, 90.0), Quat::IDENTITY, Vec3::new(0.0, -0.12, 0.0)),
                [0.52, 0.55, 0.58, 1.0],
                [1.0, 0.0, 0.05, 0.0], // mode=ground, no pick id
            ),
        });
        for part in &self.model.parts {
            if let Some((kind, model)) = display_of(&part.sdf) {
                draws.push(FrameDraw {
                    kind,
                    object: object_raw(model, part.color, [0.0, 0.0, 0.30, part.id as f32 + 1.0]),
                });
            }
        }

        ui.painter().add(egui_wgpu::Callback::new_paint_callback(
            rect,
            cad_view::renderer::ViewportCallback { globals, draws },
        ));
        if self.selected.is_some() {
            ui.ctx().request_repaint(); // selection breathe; otherwise stay idle (phone battery)
        }
    }
}

/// mat4 + normal matrix + color + params → one 256-byte object slot.
fn object_raw(model: Mat4, color: [f32; 4], params: [f32; 4]) -> ObjectRaw {
    ObjectRaw {
        model: model.to_cols_array(),
        nrm: model.inverse().transpose().to_cols_array(),
        color,
        params,
        _pad: [0.0; 24],
    }
}

/// Walk Translate/Rotate wrappers to the primitive leaf: (offset, rotation, leaf).
fn unwrap_placement(sdf: &Sdf) -> (Vec3, Quat, &Sdf) {
    match sdf {
        Sdf::Translate { off, node } => {
            let (o, r, leaf) = unwrap_placement(node);
            (*off + o, r, leaf)
        }
        Sdf::Rotate { inv, node } => {
            let (o, r, leaf) = unwrap_placement(node);
            (inv.inverse() * o, inv.inverse() * r, leaf)
        }
        other => (Vec3::ZERO, Quat::IDENTITY, other),
    }
}

/// The M0 display map: primitive leaf → cached mesh kind + model matrix. CSG-shaped parts return
/// None (drawn once the M1 mesher lands); the kernel still picks and measures them exactly.
fn display_of(sdf: &Sdf) -> Option<(MeshKind, Mat4)> {
    let (off, rot, leaf) = unwrap_placement(sdf);
    let (kind, scale) = match *leaf {
        Sdf::Cuboid { half } => (MeshKind::Cuboid, half),
        Sdf::Sphere { r } => (MeshKind::Sphere, Vec3::splat(r)),
        Sdf::Cylinder { r, hh } => (MeshKind::Cylinder, Vec3::new(r, hh, r)),
        Sdf::Torus { major, minor } => (
            MeshKind::Torus { ratio_milli: ((minor / major) * 1000.0).round() as u32 },
            Vec3::splat(major),
        ),
        Sdf::Capsule { r, hh } => (
            MeshKind::Capsule { hh_milli: ((hh / r) * 1000.0).round() as u32 },
            Vec3::splat(r),
        ),
        _ => return None,
    };
    Some((kind, Mat4::from_scale_rotation_translation(scale, rot, off)))
}

impl eframe::App for App {
    fn ui(&mut self, root: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = root.ctx().clone();
        let ctx = &ctx;
        // keyboard → cad-motion actions (skip when a text widget wants the keys)
        if ctx.memory(|m| m.focused().is_none()) {
            let mut acts: Vec<Action> = Vec::new();
            let mut esc = false;
            ctx.input(|i| {
                for ev in &i.events {
                    match ev {
                        egui::Event::Text(t) => {
                            for ch in t.chars() {
                                if let Some(a) = cad_motion::normal_key(ch) {
                                    if self.mode == Mode::Normal {
                                        acts.push(a);
                                    }
                                }
                            }
                        }
                        egui::Event::Key { key: egui::Key::Escape, pressed: true, .. } => esc = true,
                        _ => {}
                    }
                }
            });
            for a in acts {
                self.act(a);
            }
            if esc {
                let (mode, action) = cad_motion::escape(self.mode);
                self.mode = mode;
                if let Some(a) = action {
                    self.act(a);
                }
            }
        }

        egui::Panel::left("parts")
            .resizable(true)
            .default_size(180.0)
            .show_inside(root, |ui| {
                ui.heading(&self.doc.name);
                ui.separator();
                let mut clicked = None;
                for part in &self.model.parts {
                    let sel = self.selected == Some(part.id);
                    if ui.selectable_label(sel, &part.name).clicked() {
                        clicked = Some(if sel { None } else { Some(part.id) });
                    }
                }
                if let Some(c) = clicked {
                    self.selected = c;
                }
                if !self.warns.is_empty() {
                    ui.separator();
                    ui.label(egui::RichText::new("op warnings").strong());
                    for w in &self.warns {
                        ui.label(format!("#{}: {}", w.index, w.reason));
                    }
                }
                ui.separator();
                ui.label(format!("{} ops → {} parts", self.doc.ops.len(), self.model.parts.len()));
            });

        egui::Panel::bottom("status").show_inside(root, |ui| {
            ui.horizontal(|ui| {
                let mode = match self.mode {
                    Mode::Normal => "NORMAL",
                    Mode::Grow => "GROW (M2 stub — Esc to leave)",
                    Mode::Sketch => "SKETCH (M2 stub — Esc to leave)",
                };
                ui.label(egui::RichText::new(mode).strong().monospace());
                ui.separator();
                match self.selected.and_then(|id| self.model.part(id)) {
                    Some(p) => ui.label(format!("▸ {}", p.name)),
                    None => ui.label("nothing selected"),
                };
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label("drag orbit · right-drag pan · pinch/scroll dolly · tap pick · hjkl + - f n Esc");
                });
            });
        });

        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(egui::Color32::from_rgb(26, 28, 32)))
            .show_inside(root, |ui| self.viewport(ui));
    }
}
