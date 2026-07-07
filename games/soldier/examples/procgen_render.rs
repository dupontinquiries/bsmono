//! procgen_render — headless PNG snapshots of recursively-generated maps, so the FSM-dressed output can be
//! eyeballed. Renders the level's mesh set with the same `render()` the game uses. Run:
//!   cargo run --example procgen_render --release

use early_fps::camera::{scale_for_fov, FpsCam};
use early_fps::mesh::{build_material_atlas, Light};
use early_fps::math::{Vec2, Vec3};
use early_fps::procgen::{generate, ActivationNet, ProcParams};
use early_fps::render::Fog;

const W: usize = 1000;
const H: usize = 620;
const COLS: usize = 5;

fn main() {
    let (atlas, aw, ah) = build_material_atlas(512, COLS);
    let params = ProcParams::default();

    // Build a few taste nets so the previews span the pipeline's range: the default, and a "dense" net that
    // packs more cover (so at least one preview shows structures at readable density rather than the sparse,
    // open layouts the composition-optimised default favours).
    let default_net = ActivationNet::default();
    let mut dense = ActivationNet::default();
    dense.w2[0][8] += 1.4; // more sightline-breakers
    dense.w2[2][8] += 1.0; // more sentinels
    dense.w2[5][8] += 1.4; // more shelters
    dense.w2[6][8] -= 2.2; // far fewer voids

    let shots: [(&str, &ActivationNet, u64); 3] = [("default_7", &default_net, 7), ("dense_7", &dense, 7), ("dense_99", &dense, 99)];

    for (name, net, seed) in shots {
        let (level, rep) = generate(seed, net, &params);
        let ts = level.ts();
        let mcx = level.w as f32 * 0.5 * ts;
        let mcz = level.h as f32 * 0.5 * ts;

        // A whole-map shot of a sparse map is mostly empty ground. Instead, find the densest LOCAL cluster of
        // obstacles and frame that at near-eye level, the way you'd actually see structures in play — a handful
        // of POIs at readable scale with real silhouette, not scattered dots on a distant plain.
        let (mut best_c, mut best_n) = ((mcx, mcz), 0usize);
        for o in &level.obstacles {
            let n = level
                .obstacles
                .iter()
                .filter(|q| (q.center.x - o.center.x).abs() < 16.0 && (q.center.z - o.center.z).abs() < 16.0)
                .count();
            if n > best_n {
                best_n = n;
                best_c = (o.center.x, o.center.z);
            }
        }
        let (focus_x, focus_z) = best_c;
        // stand ~14 m back at head height, looking slightly down — a player's-eye establishing angle
        let dist = 14.0;
        let ang = 2.4f32;
        let pos = Vec3::new(focus_x + ang.cos() * dist, level.floor_y + 4.5, focus_z + ang.sin() * dist);
        let target = Vec3::new(focus_x, level.floor_y + 1.2, focus_z);
        let f = (target - pos).normalized();
        let cam = FpsCam { pos, yaw: f.x.atan2(-f.z), pitch: f.y.clamp(-1.4, 1.4).asin() };

        let mut light = Light::sun(Vec3::new(-0.4, 0.92, 0.42), early_fps::mesh::rgb(255, 248, 230));
        light.sky = level.pal.sky;
        light.ambient = 1.05;
        let fog = Fog::new(level.pal.fog, 40.0, 160.0);
        let scale = scale_for_fov(H as f32, 68f32.to_radians());
        let _ = (mcx, mcz);

        // collect meshes (LOD field if present)
        let meshes: Vec<_> = if let Some(fld) = &level.lod_field {
            fld.render_meshes().into_iter().cloned().collect()
        } else {
            level.meshes.clone()
        };
        let mesh = early_fps::render::render(
            &meshes, &cam, W as f32 * 0.5, H as f32 * 0.5, scale, &light, &fog, W as f32, H as f32, 0.0, Vec2::new(0.0, 0.0),
            egui::TextureId::Managed(1), COLS, &[],
        );

        // sky/ground fill — darken the ground band so it contrasts against the sky and the geometry reads
        let mut buf = vec![0u8; W * H * 3];
        let sky = [light.sky.r(), light.sky.g(), light.sky.b()];
        let gc = fog.color;
        let grd = [(gc.r() as f32 * 0.6) as u8, (gc.g() as f32 * 0.62) as u8, (gc.b() as f32 * 0.58) as u8];
        for y in 0..H {
            let col = if y < H / 2 { sky } else { grd };
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

        let path = format!("procgen_{name}.png");
        image::save_buffer(&path, &buf, W as u32, H as u32, image::ColorType::Rgb8).unwrap();
        let tris: usize = meshes.iter().map(|m| m.tris.len()).sum();
        println!("wrote {path}  ({} render-tris, {} scene-tris, {} obstacles, depth {})", mesh.indices.len() / 3, tris, level.obstacles.len(), rep.max_depth);
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
