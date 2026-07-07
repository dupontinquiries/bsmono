//! creature_shots — render the Hunt predator creatures (Kraken, Widow, and the humanoid horrors) as PNGs so
//! their dedicated bodies can be eyeballed offline. Run: cargo run --example creature_shots
use early_fps::camera::{scale_for_fov, FpsCam};
use early_fps::creatures::Creature;
use early_fps::enemy::{Enemy, EnemyKind};
use early_fps::math::{Vec2, Vec3};
use early_fps::mesh::{build_material_atlas, rgb, Light};
use early_fps::render::{render, Fog};

const W: usize = 340;
const H: usize = 460;
const COLS: usize = 5;

fn render_creature(name: &str, cr: Creature, atlas: &[u8], aw: usize, ah: usize) {
    let mut e = Enemy::new(1, EnemyKind::Grunt, Vec3::new(0.0, 0.0, 0.0));
    e.hunter = true;
    e.hunter_creature = Some(cr);
    e.yaw = 0.5;
    let meshes = vec![e.build_mesh()];

    // frame the creature from a low front-three-quarter so tentacles/legs read against the floor
    let pos = Vec3::new(2.6, 1.6, 3.6);
    let target = Vec3::new(0.0, 1.1, 0.0);
    let f = (target - pos).normalized();
    let cam = FpsCam { pos, yaw: f.x.atan2(-f.z), pitch: f.y.clamp(-1.4, 1.4).asin() };
    let mut light = Light::sun(Vec3::new(-0.4, 0.85, 0.5), rgb(255, 248, 236));
    light.sky = rgb(120, 140, 170);
    light.ambient = 1.05;
    let fog = Fog::new(rgb(24, 26, 34), 24.0, 70.0);
    let scale = scale_for_fov(H as f32, 46f32.to_radians());
    let mesh = render(&meshes, &cam, W as f32 * 0.5, H as f32 * 0.5, scale, &light, &fog, W as f32, H as f32, 0.0, Vec2::new(0.0, 0.0), egui::TextureId::Managed(1), COLS, &[]);

    let mut buf = vec![0u8; W * H * 3];
    for y in 0..H {
        let t = y as f32 / H as f32;
        let c = [(18.0 + t * 16.0) as u8, (20.0 + t * 18.0) as u8, (28.0 + t * 22.0) as u8];
        for x in 0..W {
            let i = (y * W + x) * 3;
            buf[i..i + 3].copy_from_slice(&c);
        }
    }
    for tri in mesh.indices.chunks_exact(3) {
        let p = [mesh.vertices[tri[0] as usize], mesh.vertices[tri[1] as usize], mesh.vertices[tri[2] as usize]];
        raster(&mut buf, [[p[0].pos.x, p[0].pos.y], [p[1].pos.x, p[1].pos.y], [p[2].pos.x, p[2].pos.y]], [p[0].color, p[1].color, p[2].color], [[p[0].uv.x, p[0].uv.y], [p[1].uv.x, p[1].uv.y], [p[2].uv.x, p[2].uv.y]], atlas, aw, ah);
    }
    image::save_buffer(&format!("creature_{name}.png"), &buf, W as u32, H as u32, image::ColorType::Rgb8).unwrap();
    let tris = mesh.indices.len() / 3;
    println!("wrote creature_{name}.png  ({tris} tris)");
}

fn main() {
    let (atlas, aw, ah) = build_material_atlas(512, COLS);
    render_creature("kraken", Creature::Kraken, &atlas, aw, ah);
    render_creature("widow", Creature::Widow, &atlas, aw, ah);
    render_creature("revenant", Creature::Revenant, &atlas, aw, ah);
}

#[allow(clippy::too_many_arguments)]
fn raster(buf: &mut [u8], p: [[f32; 2]; 3], col: [egui::Color32; 3], uv: [[f32; 2]; 3], atlas: &[u8], aw: usize, ah: usize) {
    let minx = p.iter().map(|v| v[0]).fold(f32::MAX, f32::min).floor().max(0.0) as usize;
    let maxx = p.iter().map(|v| v[0]).fold(f32::MIN, f32::max).ceil().min(W as f32 - 1.0) as usize;
    let miny = p.iter().map(|v| v[1]).fold(f32::MAX, f32::min).floor().max(0.0) as usize;
    let maxy = p.iter().map(|v| v[1]).fold(f32::MIN, f32::max).ceil().min(H as f32 - 1.0) as usize;
    let area = edge(p[0], p[1], p[2]);
    if area.abs() < 1e-6 {
        return;
    }
    for y in miny..=maxy {
        for x in minx..=maxx {
            let pt = [x as f32 + 0.5, y as f32 + 0.5];
            let w0 = edge(p[1], p[2], pt) / area;
            let w1 = edge(p[2], p[0], pt) / area;
            let w2 = edge(p[0], p[1], pt) / area;
            if w0 < 0.0 || w1 < 0.0 || w2 < 0.0 {
                continue;
            }
            let u = w0 * uv[0][0] + w1 * uv[1][0] + w2 * uv[2][0];
            let v = w0 * uv[0][1] + w1 * uv[1][1] + w2 * uv[2][1];
            let ax = ((u * aw as f32) as usize).min(aw - 1);
            let ay = ((v * ah as f32) as usize).min(ah - 1);
            let ai = (ay * aw + ax) * 4;
            let (tr, tg, tb) = (atlas[ai] as f32 / 255.0, atlas[ai + 1] as f32 / 255.0, atlas[ai + 2] as f32 / 255.0);
            let cr = w0 * col[0].r() as f32 + w1 * col[1].r() as f32 + w2 * col[2].r() as f32;
            let cg = w0 * col[0].g() as f32 + w1 * col[1].g() as f32 + w2 * col[2].g() as f32;
            let cb = w0 * col[0].b() as f32 + w1 * col[1].b() as f32 + w2 * col[2].b() as f32;
            let i = (y * W + x) * 3;
            buf[i] = (cr * tr).clamp(0.0, 255.0) as u8;
            buf[i + 1] = (cg * tg).clamp(0.0, 255.0) as u8;
            buf[i + 2] = (cb * tb).clamp(0.0, 255.0) as u8;
        }
    }
}

fn edge(a: [f32; 2], b: [f32; 2], c: [f32; 2]) -> f32 {
    (c[0] - a[0]) * (b[1] - a[1]) - (c[1] - a[1]) * (b[0] - a[0])
}
