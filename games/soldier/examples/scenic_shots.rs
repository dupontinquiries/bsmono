//! scenic_shots.rs — render each map from several elevated orbit vantages and dump PNGs into
//! /tmp/scenic/<map>/<angle>.png, for the OpenCV scenic/variety scorer (tools/scenic.py) to consume.
//! These are *establishing* shots (high, looking down and across) so the scorer judges the whole layout's
//! visual richness — texture variety, colour diversity, silhouette complexity — and penalises repetition.

use early_fps::camera::{scale_for_fov, FpsCam};
use early_fps::game::Game;
use early_fps::level::PRESETS;
use early_fps::loadout::Loadout;
use early_fps::math::Vec3;
use early_fps::mesh::build_material_atlas;

const W: usize = 640;
const H: usize = 400;
const VFOV_DEG: f32 = 62.0;
const COLS: usize = 5;

fn main() {
    let (atlas, aw, ah) = build_material_atlas(512, COLS);
    let maps = [
        "Flood Zone", "Reference Town", "Desert Town", "Pyre Garage", "Concrete Slab", "Redwood Canyon",
    ];
    std::fs::create_dir_all("/tmp/scenic").ok();
    for name in maps {
        let Some(i) = PRESETS.iter().position(|p| p.name == name) else { continue };
        let game = Game::new_preset(7, Some(i), Loadout::default());
        let lvl = &game.level;
        let ts = lvl.ts();
        let mcx = lvl.w as f32 * 0.5 * ts;
        let mcz = lvl.h as f32 * 0.5 * ts;
        let center = Vec3::new(mcx, lvl.floor_y, mcz);
        let reach = lvl.w as f32 * ts;
        let dist = (reach * 0.20).clamp(24.0, 64.0);
        let height = (reach * 0.15).clamp(16.0, 46.0);
        let slug = name.to_lowercase().replace(' ', "_");
        std::fs::create_dir_all(format!("/tmp/scenic/{slug}")).ok();
        // Build a vantage set: 2 elevated orbit shots (read the whole layout) + up to 2 player eye-height
        // shots standing at spawns looking toward the centre (what players actually see — this is what makes
        // the scorer fair to enclosed maps whose interiors an overhead orbit would miss).
        let mut cams: Vec<FpsCam> = Vec::new();
        for a in 0..2 {
            let ang = 0.7 + a as f32 * std::f32::consts::PI; // two opposed orbit angles
            let target = Vec3::new(center.x, center.y + 3.0, center.z);
            let pos = target + Vec3::new(ang.cos() * dist, height, ang.sin() * dist);
            let f = (target - pos).normalized();
            cams.push(FpsCam { pos, yaw: f.x.atan2(-f.z), pitch: f.y.clamp(-1.4, 1.4).asin() });
        }
        for room in lvl.rooms.iter().take(2) {
            let eye = Vec3::new(room.center.x, room.floor_y + 1.6, room.center.z);
            // look toward the map centre but bias the aim a third of the way there and a touch downward, so
            // the frame captures the nearby ground/props a player sees stepping out — not just the far skyline.
            let mid = Vec3::new((eye.x + center.x) * 0.5, center.y + 0.8, (eye.z + center.z) * 0.5);
            let f = (mid - eye).normalized();
            cams.push(FpsCam { pos: eye, yaw: f.x.atan2(-f.z), pitch: (f.y - 0.12).clamp(-1.0, 1.0).asin() });
        }
        for (a, cam) in cams.iter().enumerate() {
            let cam = *cam;
            let scale = scale_for_fov(H as f32, VFOV_DEG.to_radians());
            let (cx, cy) = (W as f32 * 0.5, H as f32 * 0.5);
            let meshes = game.frame_meshes(&cam, false);
            let mesh = early_fps::render::render(
                &meshes, &cam, cx, cy, scale, &game.light, &game.fog, W as f32, H as f32, game.time, game.wind(),
                egui::TextureId::Managed(1), COLS, &game.dynamic_lights(),
            );
            let mut buf = vec![0u8; W * H * 3];
            let sky = game.light.sky;
            let sky = [(sky.r() as f32 * 0.85) as u8, (sky.g() as f32 * 0.85) as u8, (sky.b() as f32 * 0.85) as u8];
            let fog = game.fog.color;
            let ground = [fog.r(), fog.g(), fog.b()];
            for y in 0..H {
                let col = if y < H / 2 { sky } else { ground };
                for x in 0..W {
                    let i = (y * W + x) * 3;
                    buf[i..i + 3].copy_from_slice(&col);
                }
            }
            let verts = &mesh.vertices;
            for tri in mesh.indices.chunks_exact(3) {
                let p = [verts[tri[0] as usize], verts[tri[1] as usize], verts[tri[2] as usize]];
                let pos2 = [[p[0].pos.x, p[0].pos.y], [p[1].pos.x, p[1].pos.y], [p[2].pos.x, p[2].pos.y]];
                let col = [p[0].color, p[1].color, p[2].color];
                let uv = [[p[0].uv.x, p[0].uv.y], [p[1].uv.x, p[1].uv.y], [p[2].uv.x, p[2].uv.y]];
                raster_tri(&mut buf, pos2, col, uv, &atlas, aw, ah);
            }
            let path = format!("/tmp/scenic/{slug}/angle{a}.png");
            image::save_buffer(&path, &buf, W as u32, H as u32, image::ColorType::Rgb8).unwrap();
        }
        println!("rendered {name} → /tmp/scenic/{slug}/ ({} angles)", cams.len());
    }
}

fn edge(a: [f32; 2], b: [f32; 2], c: [f32; 2]) -> f32 {
    (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0])
}
#[allow(clippy::too_many_arguments)]
fn raster_tri(buf: &mut [u8], p: [[f32; 2]; 3], col: [egui::Color32; 3], uv: [[f32; 2]; 3], atlas: &[u8], aw: usize, ah: usize) {
    let minx = p.iter().map(|v| v[0]).fold(f32::MAX, f32::min).floor().max(0.0) as usize;
    let maxx = p.iter().map(|v| v[0]).fold(f32::MIN, f32::max).ceil().min(W as f32 - 1.0) as usize;
    let miny = p.iter().map(|v| v[1]).fold(f32::MAX, f32::min).floor().max(0.0) as usize;
    let maxy = p.iter().map(|v| v[1]).fold(f32::MIN, f32::max).ceil().min(H as f32 - 1.0) as usize;
    if minx > maxx || miny > maxy {
        return;
    }
    let area = edge(p[0], p[1], p[2]);
    if area.abs() < 1e-6 {
        return;
    }
    let inv = 1.0 / area;
    for y in miny..=maxy {
        for x in minx..=maxx {
            let pt = [x as f32 + 0.5, y as f32 + 0.5];
            let w0 = edge(p[1], p[2], pt) * inv;
            let w1 = edge(p[2], p[0], pt) * inv;
            let w2 = edge(p[0], p[1], pt) * inv;
            if w0 < 0.0 || w1 < 0.0 || w2 < 0.0 {
                continue;
            }
            let u = w0 * uv[0][0] + w1 * uv[1][0] + w2 * uv[2][0];
            let vv = w0 * uv[0][1] + w1 * uv[1][1] + w2 * uv[2][1];
            let (mut r, mut g, mut b);
            if u == 0.0 && vv == 0.0 {
                r = 1.0;
                g = 1.0;
                b = 1.0;
            } else {
                let ax = (u.clamp(0.0, 0.999) * aw as f32) as usize;
                let ay = (vv.clamp(0.0, 0.999) * ah as f32) as usize;
                let idx = (ay * aw + ax) * 4;
                r = atlas[idx] as f32 / 255.0;
                g = atlas[idx + 1] as f32 / 255.0;
                b = atlas[idx + 2] as f32 / 255.0;
            }
            r *= (w0 * col[0].r() as f32 + w1 * col[1].r() as f32 + w2 * col[2].r() as f32) / 255.0;
            g *= (w0 * col[0].g() as f32 + w1 * col[1].g() as f32 + w2 * col[2].g() as f32) / 255.0;
            b *= (w0 * col[0].b() as f32 + w1 * col[1].b() as f32 + w2 * col[2].b() as f32) / 255.0;
            let i = (y * W + x) * 3;
            buf[i] = (r * 255.0).min(255.0) as u8;
            buf[i + 1] = (g * 255.0).min(255.0) as u8;
            buf[i + 2] = (b * 255.0).min(255.0) as u8;
        }
    }
}
