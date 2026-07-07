//! brick_map — find a Crossing brick facade in a real generated map and render it CLOSE-UP (plus an
//! establishing shot for context), so the wired brick texture can actually be resolved rather than minified
//! to flat colour at distance. Run: cargo run --example brick_map
use early_fps::camera::{scale_for_fov, FpsCam};
use early_fps::game::Game;
use early_fps::level::PRESETS;
use early_fps::loadout::Loadout;
use early_fps::math::Vec3;
use early_fps::mesh::{build_material_atlas, Material};

const W: usize = 1000;
const H: usize = 620;
const COLS: usize = 5;

fn render_shot(game: &Game, cam: &FpsCam, atlas: &[u8], asz: usize, name: &str) -> u32 {
    let scale = scale_for_fov(H as f32, 74f32.to_radians());
    let meshes = game.frame_meshes(cam, false);
    let mesh = early_fps::render::render(&meshes, cam, W as f32 * 0.5, H as f32 * 0.5, scale, &game.light, &game.fog, W as f32, H as f32, game.time, game.wind(), egui::TextureId::Managed(1), COLS, &game.dynamic_lights());
    let mut buf = vec![0u8; W * H * 3];
    let sky = game.light.sky;
    let sky = [(sky.r() as f32 * 0.85) as u8, (sky.g() as f32 * 0.85) as u8, (sky.b() as f32 * 0.85) as u8];
    let fog = game.fog.color;
    for y in 0..H {
        let col = if y < H / 2 { sky } else { [fog.r(), fog.g(), fog.b()] };
        for x in 0..W {
            buf[(y * W + x) * 3..(y * W + x) * 3 + 3].copy_from_slice(&col);
        }
    }
    for tri in mesh.indices.chunks_exact(3) {
        let p = [mesh.vertices[tri[0] as usize], mesh.vertices[tri[1] as usize], mesh.vertices[tri[2] as usize]];
        raster(&mut buf, [[p[0].pos.x, p[0].pos.y], [p[1].pos.x, p[1].pos.y], [p[2].pos.x, p[2].pos.y]], [p[0].color, p[1].color, p[2].color], [[p[0].uv.x, p[0].uv.y], [p[1].uv.x, p[1].uv.y], [p[2].uv.x, p[2].uv.y]], atlas, asz);
    }
    image::save_buffer(&format!("brick_map_{name}.png"), &buf, W as u32, H as u32, image::ColorType::Rgb8).unwrap();
    mesh.indices.len() as u32 / 3
}

fn main() {
    let (atlas, asz, _) = build_material_atlas(512, COLS);
    let i = PRESETS.iter().position(|p| p.name == "Downtown").unwrap();
    // find a seed whose Reference Town frames a brick facade near spawn
    for seed in [1u64, 2, 3, 5, 6] {
        let mut game = Game::new_preset(seed, Some(i), Loadout::default());
        let input = early_fps::player::Input::default();
        for _ in 0..2 { game.step(&input, 1.0 / 60.0); }
        let lvl = &game.level;
        let (mcx, mcz) = (lvl.w as f32 * 0.5 * lvl.ts(), lvl.h as f32 * 0.5 * lvl.ts());
        let spawn = lvl.rooms[0].center;
        // overview cam to collect geometry, then hunt for brick meshes
        let over = FpsCam { pos: Vec3::new(mcx, 40.0, mcz), yaw: 0.0, pitch: -1.4 };
        let meshes = game.frame_meshes(&over, false);
        let mut bricks: Vec<(Vec3, f32)> = vec![]; // centroid, dist-to-spawn
        for m in &meshes {
            if m.material == Some(Material::Brick) && !m.verts.is_empty() {
                let mut c = Vec3::new(0.0, 0.0, 0.0);
                for v in &m.verts { c = c + *v; }
                let c = c * (1.0 / m.verts.len() as f32);
                let d = ((c.x - spawn.x).powi(2) + (c.z - spawn.z).powi(2)).sqrt();
                bricks.push((c, d));
            }
        }
        if bricks.is_empty() {
            println!("seed {seed}: no brick facade in this layout");
            continue;
        }
        bricks.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap());
        let target = bricks[0].0;
        // stand ~4m off the wall, on the side facing the map interior, at eye height
        let mut dir = Vec3::new(mcx - target.x, 0.0, mcz - target.z);
        let dl = (dir.x * dir.x + dir.z * dir.z).sqrt().max(0.001);
        dir = dir * (1.0 / dl);
        let cpos = Vec3::new(target.x + dir.x * 4.2, target.y.max(lvl.floor_h(target.x, target.z) + 1.6), target.z + dir.z * 4.2);
        let look = Vec3::new(target.x - cpos.x, (target.y - cpos.y) * 0.5, target.z - cpos.z);
        let yaw = look.x.atan2(-look.z);
        let pitch = (look.y / (look.x * look.x + look.z * look.z).sqrt()).atan().clamp(-0.5, 0.5);
        let close = FpsCam { pos: cpos, yaw, pitch };
        let t1 = render_shot(&game, &close, &atlas, asz, "closeup");
        // establishing shot for context
        let est = FpsCam { pos: Vec3::new((spawn.x - dir.x * 4.0).clamp(lvl.ts() * 2.0, mcx * 2.0), lvl.floor_h(spawn.x, spawn.z) + 3.6, (spawn.z - dir.z * 4.0).clamp(lvl.ts() * 2.0, mcz * 2.0)), yaw: (mcx - spawn.x).atan2(-(mcz - spawn.z)), pitch: -0.16 };
        let t2 = render_shot(&game, &est, &atlas, asz, "establishing");
        println!("seed {seed}: {} brick facades found; closeup {t1} tris, establishing {t2} tris; nearest wall {:.1}m from spawn", bricks.len(), bricks[0].1);
        break;
    }
}

#[allow(clippy::too_many_arguments)]
fn raster(buf: &mut [u8], p: [[f32; 2]; 3], col: [egui::Color32; 3], uv: [[f32; 2]; 3], atlas: &[u8], asz: usize) {
    let minx = p.iter().map(|v| v[0]).fold(f32::MAX, f32::min).floor().max(0.0) as usize;
    let maxx = p.iter().map(|v| v[0]).fold(f32::MIN, f32::max).ceil().min(W as f32 - 1.0) as usize;
    let miny = p.iter().map(|v| v[1]).fold(f32::MAX, f32::min).floor().max(0.0) as usize;
    let maxy = p.iter().map(|v| v[1]).fold(f32::MIN, f32::max).ceil().min(H as f32 - 1.0) as usize;
    if minx > maxx || miny > maxy { return; }
    let area = edge(p[0], p[1], p[2]);
    if area.abs() < 1e-6 { return; }
    let inv = 1.0 / area;
    for y in miny..=maxy {
        for x in minx..=maxx {
            let pt = [x as f32 + 0.5, y as f32 + 0.5];
            let (w0, w1, w2) = (edge(p[1], p[2], pt) * inv, edge(p[2], p[0], pt) * inv, edge(p[0], p[1], pt) * inv);
            if (w0 < 0.0 || w1 < 0.0 || w2 < 0.0) && (w0 > 0.0 || w1 > 0.0 || w2 > 0.0) { continue; }
            let u = w0 * uv[0][0] + w1 * uv[1][0] + w2 * uv[2][0];
            let v = w0 * uv[0][1] + w1 * uv[1][1] + w2 * uv[2][1];
            let tx = (u.clamp(0.0, 0.999) * asz as f32) as usize;
            let ty = (v.clamp(0.0, 0.999) * asz as f32) as usize;
            let ti = (ty * asz + tx) * 4;
            let cr = w0 * col[0].r() as f32 + w1 * col[1].r() as f32 + w2 * col[2].r() as f32;
            let cg = w0 * col[0].g() as f32 + w1 * col[1].g() as f32 + w2 * col[2].g() as f32;
            let cb = w0 * col[0].b() as f32 + w1 * col[1].b() as f32 + w2 * col[2].b() as f32;
            let i = (y * W + x) * 3;
            buf[i] = (atlas[ti] as f32 * cr / 255.0).clamp(0.0, 255.0) as u8;
            buf[i + 1] = (atlas[ti + 1] as f32 * cg / 255.0).clamp(0.0, 255.0) as u8;
            buf[i + 2] = (atlas[ti + 2] as f32 * cb / 255.0).clamp(0.0, 255.0) as u8;
        }
    }
}
#[inline]
fn edge(a: [f32; 2], b: [f32; 2], c: [f32; 2]) -> f32 { (c[0] - a[0]) * (b[1] - a[1]) - (c[1] - a[1]) * (b[0] - a[0]) }
