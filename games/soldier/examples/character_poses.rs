//! character_poses — render the player mannequin in idle / walk / run / crouch poses side by side, so the
//! locomotion animation can be eyeballed. Run: cargo run --example character_poses --release

use early_fps::camera::{scale_for_fov, FpsCam};
use early_fps::character::{build_mannequin_posed, ArmorSet, Pose};
use early_fps::math::{Vec2, Vec3};
use early_fps::mesh::{build_material_atlas, Light};
use early_fps::render::Fog;

const W: usize = 320;
const H: usize = 520;
const COLS: usize = 5;

fn render_pose(name: &str, pose: Pose, atlas: &[u8], aw: usize, ah: usize) {
    let set = ArmorSet::default();
    let meshes = build_mannequin_posed(&set, pose);
    // camera: front-three-quarter, framed on the figure (~1.7m tall)
    let pos = Vec3::new(1.6, 1.15, 2.4);
    let target = Vec3::new(0.0, 0.9, 0.0);
    let f = (target - pos).normalized();
    let cam = FpsCam { pos, yaw: f.x.atan2(-f.z), pitch: f.y.clamp(-1.4, 1.4).asin() };
    let mut light = Light::sun(Vec3::new(-0.5, 0.9, 0.4), early_fps::mesh::rgb(255, 250, 240));
    light.sky = early_fps::mesh::rgb(150, 165, 185);
    light.ambient = 1.1;
    let fog = Fog::new(early_fps::mesh::rgb(40, 44, 52), 20.0, 60.0);
    let scale = scale_for_fov(H as f32, 42f32.to_radians());
    let mesh = early_fps::render::render(&meshes, &cam, W as f32 * 0.5, H as f32 * 0.5, scale, &light, &fog, W as f32, H as f32, 0.0, Vec2::new(0.0, 0.0), egui::TextureId::Managed(1), COLS, &[]);
    let mut buf = vec![0u8; W * H * 3];
    for y in 0..H {
        let t = y as f32 / H as f32;
        let c = [(30.0 + t * 20.0) as u8, (34.0 + t * 22.0) as u8, (42.0 + t * 26.0) as u8];
        for x in 0..W {
            let i = (y * W + x) * 3;
            buf[i..i + 3].copy_from_slice(&c);
        }
    }
    for tri in mesh.indices.chunks_exact(3) {
        let p = [mesh.vertices[tri[0] as usize], mesh.vertices[tri[1] as usize], mesh.vertices[tri[2] as usize]];
        raster(&mut buf, [[p[0].pos.x, p[0].pos.y], [p[1].pos.x, p[1].pos.y], [p[2].pos.x, p[2].pos.y]], [p[0].color, p[1].color, p[2].color], [[p[0].uv.x, p[0].uv.y], [p[1].uv.x, p[1].uv.y], [p[2].uv.x, p[2].uv.y]], atlas, aw, ah);
    }
    image::save_buffer(&format!("pose_{name}.png"), &buf, W as u32, H as u32, image::ColorType::Rgb8).unwrap();
    let ys: Vec<f32> = meshes.iter().flat_map(|m| m.verts.iter()).map(|v| v.y).collect();
    let top = ys.iter().cloned().fold(f32::MIN, f32::max);
    println!("wrote pose_{name}.png  (top y = {:.2})", top);
}

fn main() {
    let (atlas, aw, ah) = build_material_atlas(512, COLS);
    render_pose("idle", Pose::idle(1.0), &atlas, aw, ah);
    render_pose("walk", Pose::moving(0.6, 0.5, 0.0), &atlas, aw, ah);
    render_pose("run", Pose::moving(0.5, 1.0, 0.0), &atlas, aw, ah);
    render_pose("crouch", Pose::moving(0.0, 0.0, 1.0), &atlas, aw, ah);
}

#[allow(clippy::too_many_arguments)]
fn raster(buf: &mut [u8], p: [[f32; 2]; 3], col: [egui::Color32; 3], uv: [[f32; 2]; 3], atlas: &[u8], aw: usize, ah: usize) {
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
            if (w0 < 0.0 || w1 < 0.0 || w2 < 0.0) && (w0 > 0.0 || w1 > 0.0 || w2 > 0.0) {
                continue;
            }
            let u = w0 * uv[0][0] + w1 * uv[1][0] + w2 * uv[2][0];
            let v = w0 * uv[0][1] + w1 * uv[1][1] + w2 * uv[2][1];
            let tx = (u.clamp(0.0, 0.999) * aw as f32) as usize;
            let ty = (v.clamp(0.0, 0.999) * ah as f32) as usize;
            let ti = (ty * aw + tx) * 4;
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
fn edge(a: [f32; 2], b: [f32; 2], c: [f32; 2]) -> f32 {
    (c[0] - a[0]) * (b[1] - a[1]) - (c[1] - a[1]) * (b[0] - a[0])
}
