//! preview.rs — headless render of the world into PNG snapshots, so the textured software renderer can be
//! eyeballed without a display. Runs the *same* `render()` the game uses (now atlas-textured), then
//! rasterizes the resulting `egui` mesh — sampling the material atlas per pixel and multiplying by the
//! gouraud-shaded vertex colour, exactly like egui's GPU path. Run: cargo run --example preview --release

use early_fps::camera::{scale_for_fov, FpsCam};
use early_fps::game::Game;
use early_fps::level::PRESETS;
use early_fps::loadout::Loadout;
use early_fps::math::Vec3;
use early_fps::mesh::build_material_atlas;

const W: usize = 1000;
const H: usize = 620;
const VFOV_DEG: f32 = 74.0;
const COLS: usize = 5;

fn darker(c: [u8; 3], f: f32) -> [u8; 3] {
    [(c[0] as f32 * f) as u8, (c[1] as f32 * f) as u8, (c[2] as f32 * f) as u8]
}

fn main() {
    let (atlas, aw, ah) = build_material_atlas(512, COLS);
    let preset = |name: &str| PRESETS.iter().position(|p| p.name == name);
    // (game, label, presteps) — presteps advances the sim so we can catch timed events like the sandstorm.
    let mut shots: Vec<(Game, &str, u32)> = Vec::new();
    if let Some(i) = preset("Desert Town") {
        shots.push((Game::new_preset(7, Some(i), Loadout::default()), "desert_town", 0));
        shots.push((Game::new_preset(7, Some(i), Loadout::default()), "desert_storm", 1680)); // ~28s → storm peak
    }
    if let Some(i) = preset("Pyre Garage") {
        // step a bit so fire particles populate the plume
        shots.push((Game::new_preset(7, Some(i), Loadout::default()), "pyre_garage", 90));
    }
    if let Some(i) = preset("Concrete Slab") {
        shots.push((Game::new_preset(7, Some(i), Loadout::default()), "concrete_slab", 0));
    }
    if let Some(i) = preset("Redwood Canyon") {
        shots.push((Game::new_preset(7, Some(i), Loadout::default()), "redwood_canyon", 0));
    }
    if let Some(i) = preset("Malecon Breakwater") {
        shots.push((Game::new_preset(7, Some(i), Loadout::default()), "malecon", 0));
    }
    if let Some(i) = preset("Flood Zone") {
        shots.push((Game::new_preset(7, Some(i), Loadout::default()), "flood_zone", 0));
    }
    if let Some(i) = preset("Reference Town") {
        shots.push((Game::new_preset(4, Some(i), Loadout::default()), "reference_town", 0));
        let mut ctf = Game::new_preset(4, Some(i), Loadout::default());
        ctf.set_capture_the_flag();
        shots.push((ctf, "reference_ctf", 6));
    }
    shots.push((Game::new(3), "urban", 0));
    shots.push((Game::new(0), "industrial", 0));

    for (game, name, presteps) in &mut shots {
        let input = early_fps::player::Input::default();
        for _ in 0..*presteps {
            game.step(&input, 1.0 / 60.0);
        }
        let game = &*game;
        let lvl = &game.level;
        let mcx = lvl.w as f32 * 0.5 * lvl.ts();
        let mcz = lvl.h as f32 * 0.5 * lvl.ts();
        // Establishing shot: stand back near the spawn corner, raised to clear the near cover, and look
        // diagonally across the whole map toward its centre so the textured streets/buildings fill the frame.
        let spawn = lvl.rooms[0].center;
        let (mut dx, mut dz) = (mcx - spawn.x, mcz - spawn.z);
        let dl = (dx * dx + dz * dz).sqrt().max(0.001);
        dx /= dl;
        dz /= dl;
        // step a little further back from centre than the spawn, if we're not against the border
        let back = 4.0;
        let px = (spawn.x - dx * back).clamp(lvl.ts() * 2.0, mcx * 2.0 - lvl.ts() * 2.0);
        let pz = (spawn.z - dz * back).clamp(lvl.ts() * 2.0, mcz * 2.0 - lvl.ts() * 2.0);
        let pos = Vec3::new(px, lvl.floor_h(px, pz) + 3.6, pz);
        let yaw = (mcx - px).atan2(-(mcz - pz));
        let cam = FpsCam { pos, yaw, pitch: -0.16 };

        let scale = scale_for_fov(H as f32, VFOV_DEG.to_radians());
        let (cx, cy) = (W as f32 * 0.5, H as f32 * 0.5);
        let meshes = game.frame_meshes(&cam, false);
        let mesh = early_fps::render::render(
            &meshes, &cam, cx, cy, scale, &game.light, &game.fog, W as f32, H as f32, game.time, game.wind(),
            egui::TextureId::Managed(1), COLS, &game.dynamic_lights(),
        );

        let mut buf = vec![0u8; W * H * 3];
        let sky = game.light.sky;
        let sky = darker([sky.r(), sky.g(), sky.b()], 0.85);
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

        let path = format!("preview_{name}.png");
        image::save_buffer(&path, &buf, W as u32, H as u32, image::ColorType::Rgb8).unwrap();
        println!("wrote {path}  ({} tris)", mesh.indices.len() / 3);
    }
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
    let samp = |u: f32, v: f32| -> [f32; 3] {
        let x = (u.clamp(0.0, 0.999) * aw as f32) as usize;
        let y = (v.clamp(0.0, 0.999) * ah as f32) as usize;
        let i = (y * aw + x) * 4;
        [atlas[i] as f32, atlas[i + 1] as f32, atlas[i + 2] as f32]
    };
    for y in miny..=maxy {
        for x in minx..=maxx {
            let pt = [x as f32 + 0.5, y as f32 + 0.5];
            let w0 = edge(p[1], p[2], pt) * inv;
            let w1 = edge(p[2], p[0], pt) * inv;
            let w2 = edge(p[0], p[1], pt) * inv;
            if (w0 < 0.0 || w1 < 0.0 || w2 < 0.0) && (w0 > 0.0 || w1 > 0.0 || w2 > 0.0) {
                continue;
            }
            let u = w0 * uv[0][0] + w1 * uv[1][0] + w2 * uv[2][0];
            let v = w0 * uv[0][1] + w1 * uv[1][1] + w2 * uv[2][1];
            let t = samp(u, v);
            let cr = w0 * col[0].r() as f32 + w1 * col[1].r() as f32 + w2 * col[2].r() as f32;
            let cg = w0 * col[0].g() as f32 + w1 * col[1].g() as f32 + w2 * col[2].g() as f32;
            let cb = w0 * col[0].b() as f32 + w1 * col[1].b() as f32 + w2 * col[2].b() as f32;
            let i = (y * W + x) * 3;
            buf[i] = (t[0] * cr / 255.0).clamp(0.0, 255.0) as u8;
            buf[i + 1] = (t[1] * cg / 255.0).clamp(0.0, 255.0) as u8;
            buf[i + 2] = (t[2] * cb / 255.0).clamp(0.0, 255.0) as u8;
        }
    }
}

#[inline]
fn edge(a: [f32; 2], b: [f32; 2], c: [f32; 2]) -> f32 {
    (c[0] - a[0]) * (b[1] - a[1]) - (c[1] - a[1]) * (b[0] - a[0])
}
