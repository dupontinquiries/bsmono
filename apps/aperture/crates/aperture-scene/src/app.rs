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
        let world = if let Some(atlas) = &viewer.atlas {
            scene::build_rooms(&viewer.stations, viewer.focus, aspect, viewer.atlas_cell_px, atlas.size[1] as f32, &viewer.revs, viewer.rev_ix)
        } else {
            scene::build(aspect)
        };
        App { viewer, world }
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
        if i.key_pressed(Key::Comma) {
            app.viewer.rev_step(1); // older
        }
        if i.key_pressed(Key::Period) {
            app.viewer.rev_step(-1); // newer, and past newest = home to the worktree
        }
        if i.key_pressed(Key::B) {
            app.viewer.branch_from_here(); // no-op with a message while live — same guard as the flat editor
        }
        if i.key_pressed(Key::M) {
            app.viewer.materialize_here(); // gitmind's materialize, same guard
        }
        if i.key_pressed(Key::V) {
            app.viewer.mark_visited(); // "I'm caught up" — refs/aperture/visited/*
        }
        if i.key_pressed(Key::H) {
            app.viewer.enter_homecoming(); // step into the gap; then , / . wind forward/back
        }
        if i.key_pressed(Key::G) {
            // logscope's own camera-fit vocabulary (centroid + bounding_radius
            // + fit_distance): fly to frame the WHOLE commit graph, not just
            // whatever the reading camera happens to be pointed at.
            let pts: Vec<scene_kit::spatial::Vec3> = app.world.graph_node_ix.iter().map(|(_, p)| *p).collect();
            app.viewer.frame_points(&pts);
        }
        if i.key_pressed(Key::CloseBracket) {
            app.viewer.jump_rel(1);
        }
        if i.key_pressed(Key::OpenBracket) {
            app.viewer.jump_rel(-1);
        }
        if i.key_pressed(Key::End) {
            app.viewer.goto_bottom();
        }
        // The way back (plaque clicks, graph-node jumps). Plain arrows: the
        // viewer's keys are unmodified, and ←/→ were free.
        if i.key_pressed(Key::ArrowLeft) {
            app.viewer.nav_go(true);
        }
        if i.key_pressed(Key::ArrowRight) {
            app.viewer.nav_go(false);
        }
    });

    // ── animate, rasterize, window ──────────────────────────────────────────
    app.viewer.poll_note();
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

    // Jump landed: rebuild the corridor with the lectern at the new focus.
    if app.viewer.world_dirty {
        app.viewer.world_dirty = false;
        if let Some(atlas) = &app.viewer.atlas {
            let aspect = (COLS as f32 * 1.0) / (ROWS as f32 * 2.0);
            let trail = app.viewer.trail_stations();
            app.world = scene::build_rooms_with_trail(
                &app.viewer.stations,
                app.viewer.focus,
                aspect,
                app.viewer.atlas_cell_px,
                atlas.size[1] as f32,
                &app.viewer.revs,
                app.viewer.rev_ix,
                &trail,
            );
        }
    }

    // ── camera + uniforms ───────────────────────────────────────────────────
    // The target FLIES between rooms (tx/ty/tz Smooths); in the classic
    // single-room world those sit at the page center anyway.
    let target = if app.viewer.stations.is_empty() {
        app.world.page_center
    } else {
        scene_kit::spatial::Vec3::new(app.viewer.tx.pos, app.viewer.ty.pos, app.viewer.tz.pos)
    };
    let cam = Camera::from_orbit(
        &orbit(&app.viewer, target),
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
    // The sun follows the CLOCK, not a fixed study-light constant: live, the
    // real wall-clock hour; previewing a revision, that commit's own real
    // timestamp — winding forward through history means walking through
    // the actual days and nights those changes happened in, literally, not
    // as a metaphor. Orthogonal to RoomEnv (kind-of-room): time-of-day and
    // room-character are independent axes in life, so they stay independent
    // here — this touches sun_dir only, never RoomEnv's tuned mood colors.
    let hour = app.viewer.current_hour();
    let sd = scene_kit::mesh3d::sun_dir_3d_at(hour);
    u.sun_dir = [sd.x, sd.y, sd.z];
    // The ROOM sets the light: each station's deterministic RoomEnv, cross-
    // faded during flight. (In the classic world this is BASE_ENV — the same
    // golden-hour study as before.)
    let env = app.viewer.current_env();
    // THE LIVING LIGHT: the focused room's kind picks a scene-kit mechanism
    // and the room breathes with it — exposure and a few degrees of hue —
    // and its dust thickens with the item's blame age. Deterministic per
    // name, so the same room always moves the same way.
    let (breath, hue, dusty) = app.viewer.stations.get(app.viewer.focus)
        .map(|s| {
            let (b, h) = crate::rooms::breathe(s.item.kind, &s.item.name, time);
            (b, h, crate::rooms::dust(s.heat))
        })
        .unwrap_or((1.0, 0.0, 1.0));
    u.sun_col = crate::rooms::hue_rotate(env.sun_col, hue);
    u.sky = env.sky;
    u.ground = [0.23, 0.19, 0.14];
    let (_, _, hour_ambient) = scene_kit::mesh3d::sun_clock_at(hour);
    u.ambient = 0.62 * (hour_ambient / 1.18).clamp(0.55, 1.0); // gentle day/night breathing, never fully dark
    u.fog = env.fog * dusty;
    u.fog_col = env.fog_col;
    u.exposure = env.exposure * breath;
    u.godray_amt = 0.22;
    u.sun_uv = [0.74, 0.16];
    u.opaque = 1.0;
    u.bg = [0.055, 0.06, 0.082];
    u.saturation = env.saturation;
    u.touch = app.viewer.touch;
    u.page = Some(page_tex);
    u.page2 = app
        .viewer
        .atlas
        .as_ref()
        .map(|p| PageTex { size: p.size, rgba: p.rgba.clone(), version: p.version });

    let verts = pack_sorted(&app.world.meshes, &cam, 0.004);
    root.painter().add(glow_scene::callback(rect, u, verts));

    // ── touch: click the page, the page answers ─────────────────────────────
    if response.clicked() {
        if let Some(click) = response.interact_pointer_pos() {
            // Graph nodes first: they're spheres, not quads, so this is a
            // screen-space DISTANCE test — project the world center, check
            // pixel distance to the click — rather than the plaques' own
            // contains-a-rect convention. A small fixed pixel radius (not
            // scaled by camera distance): the graph sits at a fairly
            // constant distance from the usual viewing angles, and a
            // generous flat radius is more forgiving to click than a
            // radius that shrinks as you zoom out.
            const NODE_CLICK_PX: f32 = 22.0;
            let mut jumped = false;
            for (rev_ix, center) in &app.world.graph_node_ix {
                if let Some(p) = project(&cam, rect, *center) {
                    if p.distance(click) < NODE_CLICK_PX {
                        app.viewer.jump_to_graph_node(*rev_ix);
                        jumped = true;
                        break;
                    }
                }
            }
            if !jumped {
                for (station_ix, mesh_ix) in &app.world.plaque_ix {
                    let pts: Vec<Pos2> = scene::plaque_corners(&app.world, *mesh_ix)
                        .iter()
                        .filter_map(|c| project(&cam, rect, *c))
                        .collect();
                    if pts.len() == 4 && Rect::from_points(&pts).contains(click) {
                        app.viewer.jump_recorded(*station_ix);
                        jumped = true;
                        break;
                    }
                }
            }
            let corners = scene::page_corners(&app.world);
            let pts: Vec<Pos2> = corners.iter().filter_map(|c| project(&cam, rect, *c)).collect();
            if !jumped && pts.len() == 4 {
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
    // Line 2: the assistant's margin note about the shown revision when one
    // has arrived; the key hints otherwise.
    let line2 = if let Some((msg, _)) = &app.viewer.branch_msg {
        format!("⎇ {msg}")
    } else if !app.viewer.rev_note.is_empty() {
        format!("✎ {}", app.viewer.rev_note)
    } else {
        "wheel scroll · drag orbit · [ ] rooms · , . revisions · b branch · m worktree · v mark · h homecoming · g graph · click plaques".to_string()
    };
    p.text(
        rect.left_bottom() + Vec2::new(12.0, -10.0),
        egui::Align2::LEFT_BOTTOM,
        line2,
        egui::FontId::proportional(11.0),
        Color32::from_rgba_unmultiplied(226, 220, 206, 140),
    );
}
