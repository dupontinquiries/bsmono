//! app — the only module that touches egui/GL. Reads the viewer, builds one
//! `Uniforms` + packed vertex soup per frame, submits the scene-kit
//! `PaintCallback`, and lays two lines of chrome over the world. Everything
//! it *decides* it delegates to `viewer`/`scene`, which is where the tests are.

use crate::scene::{self, World};
use crate::viewer::{Viewer, COLS, ROWS, SCALE};
use egui::{Color32, Pos2, Rect, Sense, Ui, Vec2};
use scene_kit::glow_scene::{self, PageTex, Uniforms};
use scene_kit::gpuscene::{pack_sorted, Camera};
use scene_kit::inscribe::GLYPH;
use scene_kit::spatial::OrbitCam;

pub struct App {
    pub viewer: Viewer,
    pub world: World,
}

impl App {
    pub fn new(viewer: Viewer) -> Self {
        let cell = (GLYPH * SCALE) as f32;
        let aspect = (COLS as f32 * cell + cell * 2.0) / (ROWS as f32 * cell + cell * 2.0);
        App { viewer, world: scene::build(aspect) }
    }
}

fn orbit(v: &Viewer, target: scene_kit::spatial::Vec3) -> OrbitCam {
    OrbitCam { yaw: v.yaw.pos, pitch: v.pitch.pos, dist: v.dist.pos, target, roll: 0.0 }
}

/// Project a world point to screen px within `rect`. None when behind the eye.
fn project(cam: &Camera, rect: Rect, p: scene_kit::spatial::Vec3) -> Option<Pos2> {
    let m = cam.view_proj();
    let x = m[0] * p.x + m[4] * p.y + m[8] * p.z + m[12];
    let y = m[1] * p.x + m[5] * p.y + m[9] * p.z + m[13];
    let w = m[3] * p.x + m[7] * p.y + m[11] * p.z + m[15];
    if w <= 0.01 {
        return None;
    }
    let (nx, ny) = (x / w, y / w);
    Some(Pos2::new(
        rect.left() + (nx * 0.5 + 0.5) * rect.width(),
        rect.top() + (0.5 - ny * 0.5) * rect.height(),
    ))
}

pub fn frame(root: &mut Ui, app: &mut App) {
    let ctx = root.ctx().clone();
    let rect = root.max_rect();
    let response = root.allocate_rect(rect, Sense::click_and_drag());
    let dt = ctx.input(|i| i.stable_dt).clamp(0.0, 0.25);
    let time = ctx.input(|i| i.time) as f32;

    // ── input → viewer intent ───────────────────────────────────────────────
    if response.hovered() {
        let wheel = ctx.input(|i| i.smooth_scroll_delta.y);
        if wheel != 0.0 {
            app.viewer.scroll_lines(-wheel / 26.0);
        }
        let z = ctx.input(|i| i.zoom_delta());
        if z != 1.0 {
            app.viewer.zoom(1.0 / z);
        }
    }
    if response.dragged() {
        let d = response.drag_delta();
        app.viewer.orbit(-d.x * 0.004, -d.y * 0.003);
    }
    ctx.input(|i| {
        use egui::Key;
        if i.key_pressed(Key::PageDown) || (i.key_pressed(Key::Space) && !i.modifiers.shift) {
            app.viewer.page_step(1.0);
        }
        if i.key_pressed(Key::PageUp) || (i.key_pressed(Key::Space) && i.modifiers.shift) {
            app.viewer.page_step(-1.0);
        }
        if i.key_pressed(Key::ArrowDown) {
            app.viewer.scroll_lines(3.0);
        }
        if i.key_pressed(Key::ArrowUp) {
            app.viewer.scroll_lines(-3.0);
        }
        if i.key_pressed(Key::Home) {
            app.viewer.goto_top();
        }
        if i.key_pressed(Key::End) {
            app.viewer.goto_bottom();
        }
    });

    // ── animate, rasterize, window ──────────────────────────────────────────
    if app.viewer.step(dt) {
        ctx.request_repaint();
    } else {
        // idle: still breathe (materials animate on u_time) at a battery pace
        ctx.request_repaint_after(std::time::Duration::from_millis(66));
    }
    let (page_tex, tex_h) = {
        let p = app.viewer.ensure_page();
        (
            PageTex { size: p.size, rgba: p.rgba.clone(), version: p.version },
            p.size[1] as f32,
        )
    };
    // UV window: the visible ROWS (+ both margins) of the RASTER_ROWS raster,
    // slid by the animated fractional top.
    let cell = (GLYPH * SCALE) as f32;
    let off_px = app.viewer.uv_row_offset() * cell;
    let win_px = ROWS as f32 * cell + cell * 2.0;
    let v0 = (off_px / tex_h).clamp(0.0, 1.0);
    let v1 = ((off_px + win_px) / tex_h).clamp(0.0, 1.0);
    scene::set_page_window(&mut app.world, v0, v1);

    // ── camera + uniforms ───────────────────────────────────────────────────
    let cam = Camera::from_orbit(
        &orbit(&app.viewer, app.world.page_center),
        rect.width(),
        rect.height(),
        rect.height() * 0.78,
        0.5,
        220.0,
    );
    let mut u = Uniforms::default();
    u.viewproj = cam.view_proj();
    u.eye = cam.eye();
    u.time = time;
    u.aspect = rect.width() / rect.height().max(1.0);
    // A study at golden hour: warm key from high right, cool sky fill.
    let sd = [0.42f32, 0.78, 0.46];
    let sl = (sd[0] * sd[0] + sd[1] * sd[1] + sd[2] * sd[2]).sqrt();
    u.sun_dir = [sd[0] / sl, sd[1] / sl, sd[2] / sl];
    u.sun_col = [1.0, 0.92, 0.78];
    u.sky = [0.34, 0.40, 0.52];
    u.ground = [0.23, 0.19, 0.14];
    u.ambient = 0.62;
    u.fog = 0.0045;
    u.fog_col = [0.09, 0.095, 0.115];
    u.exposure = 1.32;
    u.godray_amt = 0.22;
    u.sun_uv = [0.74, 0.16];
    u.opaque = 1.0;
    u.bg = [0.055, 0.06, 0.082];
    u.saturation = 1.1;
    u.touch = app.viewer.touch;
    u.page = Some(page_tex);

    let verts = pack_sorted(&app.world.meshes, &cam, 0.004);
    root.painter().add(glow_scene::callback(rect, u, verts));

    // ── touch: click the page, the page answers ─────────────────────────────
    if response.clicked() {
        if let Some(click) = response.interact_pointer_pos() {
            let corners = scene::page_corners(&app.world);
            let pts: Vec<Pos2> = corners.iter().filter_map(|c| project(&cam, rect, *c)).collect();
            if pts.len() == 4 {
                let page_rect = Rect::from_points(&pts);
                if page_rect.contains(click) {
                    app.viewer.touch_at([
                        app.world.page_center.x,
                        app.world.page_center.y,
                        app.world.page_center.z,
                    ]);
                }
            }
        }
    }

    // ── chrome: two lines over the world ────────────────────────────────────
    let p = root.painter();
    p.text(
        rect.left_bottom() + Vec2::new(12.0, -26.0),
        egui::Align2::LEFT_BOTTOM,
        app.viewer.status_line(),
        egui::FontId::monospace(13.0),
        Color32::from_rgb(226, 220, 206),
    );
    p.text(
        rect.left_bottom() + Vec2::new(12.0, -10.0),
        egui::Align2::LEFT_BOTTOM,
        "wheel scroll · drag orbit · pinch/ctrl+wheel zoom · click the page",
        egui::FontId::proportional(11.0),
        Color32::from_rgba_unmultiplied(226, 220, 206, 140),
    );
}
