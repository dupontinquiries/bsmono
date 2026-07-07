//! trooper_gunmetal — apply the gunmetal armor treatment to the ACTUAL character mannequin. Renders the real
//! `build_mannequin_posed` figure twice: left with the stock atlas (RustPlate/HexPlate armor), right with the
//! two armor cells overwritten by a tileable gunmetal texture. Only the armor material's atlas cells change —
//! same mesh, same everything else. Run: cargo run --example trooper_gunmetal
use early_fps::camera::{scale_for_fov, FpsCam};
use early_fps::character::{build_mannequin_posed, ArmorSet, Pose};
use early_fps::math::{Vec2, Vec3};
use early_fps::mesh::{build_material_atlas, rgb, Light};
use early_fps::render::Fog;
use egui::Color32;

const W: usize = 340;
const H: usize = 540;
const COLS: usize = 5;
const TILE: usize = 512;

// atlas slots: material idx+1 → (slot%COLS, slot/COLS). RustPlate idx 9 → slot 10 → (0,2); HexPlate idx 19 → slot 20 → (0,4).
const RUST_CELL: (usize, usize) = (0, 2);
const HEX_CELL: (usize, usize) = (0, 4);

fn hash2(x: i32, y: i32, s: u32) -> f32 {
    let mut h = (x as u32).wrapping_mul(0x27d4_eb2d) ^ (y as u32).wrapping_mul(0x1656_67b1) ^ s.wrapping_mul(0x9e37_79b1);
    h ^= h >> 15;
    h = h.wrapping_mul(0x85eb_ca6b);
    h ^= h >> 13;
    (h & 0x00ff_ffff) as f32 / 0x00ff_ffff as f32
}
fn vnoise(x: f32, y: f32, s: u32) -> f32 {
    let (xi, yi) = (x.floor() as i32, y.floor() as i32);
    let (xf, yf) = (x - xi as f32, y - yi as f32);
    let (u, v) = (xf * xf * (3.0 - 2.0 * xf), yf * yf * (3.0 - 2.0 * yf));
    lerp(lerp(hash2(xi, yi, s), hash2(xi + 1, yi, s), u), lerp(hash2(xi, yi + 1, s), hash2(xi + 1, yi + 1, s), u), v)
}
fn fbm(x: f32, y: f32, s: u32) -> f32 {
    let (mut f, mut a, mut fr, mut n) = (0.0, 0.5, 1.0, 0.0);
    for _ in 0..4 {
        f += a * vnoise(x * fr, y * fr, s);
        n += a;
        a *= 0.5;
        fr *= 2.0;
    }
    f / n
}
fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}
fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Tileable gunmetal: cool steel base + vertical brush + sparse bright scratches + micro; `tread` adds a faint
/// gunmetal diamond pattern for the HexPlate cell so the two armor materials stay distinct.
fn gunmetal(u: f32, v: f32, tread: bool) -> Color32 {
    let base = [96.0, 100.0, 110.0];
    let polish = [182.0, 188.0, 198.0];
    let brush = (fbm(u * 3.0, v * 60.0, 7) - 0.5) * 0.12;
    let micro = (hash2((u * 240.0) as i32, (v * 240.0) as i32, 0xA1) - 0.5) * 0.06;
    let mut col = [base[0] * (1.0 + brush + micro), base[1] * (1.0 + brush + micro), base[2] * (1.0 + brush + micro)];
    let scratch = smoothstep(0.86, 0.93, fbm(u * 52.0, v * 9.0, 31)) * 0.45;
    for k in 0..3 {
        col[k] = lerp(col[k], polish[k], scratch);
    }
    if tread {
        let n = 6.0;
        let cu = (u * n).fract() - 0.5;
        let cv = ((v * n) + if ((u * n) as i32) % 2 == 0 { 0.0 } else { 0.5 }).fract() - 0.5;
        let raised = smoothstep(0.42, 0.24, cu.abs() + cv.abs());
        let f = 0.82 + raised * 0.42; // grooves dark, pads bright
        for c in col.iter_mut() {
            *c *= f;
        }
    }
    Color32::from_rgb(col[0].clamp(0.0, 255.0) as u8, col[1].clamp(0.0, 255.0) as u8, col[2].clamp(0.0, 255.0) as u8)
}

fn overwrite_cell(atlas: &mut [u8], size: usize, cell: (usize, usize), tread: bool) {
    let (ox, oy) = (cell.0 * TILE, cell.1 * TILE);
    for py in 0..TILE {
        for px in 0..TILE {
            let c = gunmetal(px as f32 / TILE as f32, py as f32 / TILE as f32, tread);
            let i = ((oy + py) * size + (ox + px)) * 4;
            atlas[i] = c.r();
            atlas[i + 1] = c.g();
            atlas[i + 2] = c.b();
            atlas[i + 3] = 255;
        }
    }
}

fn cell_mean(atlas: &[u8], size: usize, cell: (usize, usize)) -> [f32; 3] {
    let (ox, oy) = (cell.0 * TILE, cell.1 * TILE);
    let (mut s, mut n) = ([0.0; 3], 0.0);
    for py in (0..TILE).step_by(4) {
        for px in (0..TILE).step_by(4) {
            let i = ((oy + py) * size + (ox + px)) * 4;
            s[0] += atlas[i] as f32;
            s[1] += atlas[i + 1] as f32;
            s[2] += atlas[i + 2] as f32;
            n += 1.0;
        }
    }
    [s[0] / n, s[1] / n, s[2] / n]
}

fn render_into(buf: &mut [u8], stride: usize, ox: usize, atlas: &[u8], aw: usize, ah: usize) {
    let set = ArmorSet::default();
    let meshes = build_mannequin_posed(&set, Pose::idle(1.0));
    let pos = Vec3::new(1.7, 1.15, 2.5);
    let target = Vec3::new(0.0, 0.92, 0.0);
    let f = (target - pos).normalized();
    let cam = FpsCam { pos, yaw: f.x.atan2(-f.z), pitch: f.y.clamp(-1.4, 1.4).asin() };
    let mut light = Light::sun(Vec3::new(-0.5, 0.9, 0.4), rgb(255, 250, 240));
    light.sky = rgb(150, 165, 185);
    light.ambient = 1.1;
    let fog = Fog::new(rgb(34, 38, 46), 20.0, 60.0);
    let scale = scale_for_fov(H as f32, 42f32.to_radians());
    let mesh = early_fps::render::render(&meshes, &cam, W as f32 * 0.5, H as f32 * 0.5, scale, &light, &fog, W as f32, H as f32, 0.0, Vec2::new(0.0, 0.0), egui::TextureId::Managed(1), COLS, &[]);
    for tri in mesh.indices.chunks_exact(3) {
        let p = [mesh.vertices[tri[0] as usize], mesh.vertices[tri[1] as usize], mesh.vertices[tri[2] as usize]];
        raster(buf, stride, ox, [[p[0].pos.x, p[0].pos.y], [p[1].pos.x, p[1].pos.y], [p[2].pos.x, p[2].pos.y]], [p[0].color, p[1].color, p[2].color], [[p[0].uv.x, p[0].uv.y], [p[1].uv.x, p[1].uv.y], [p[2].uv.x, p[2].uv.y]], atlas, aw, ah);
    }
}

fn main() {
    let (atlas, size, _) = build_material_atlas(TILE, COLS);
    let mut gun = atlas.clone();
    overwrite_cell(&mut gun, size, RUST_CELL, false);
    overwrite_cell(&mut gun, size, HEX_CELL, true);

    let stride = W * 2;
    let mut buf = vec![0u8; stride * H * 3];
    for y in 0..H {
        let t = y as f32 / H as f32;
        let c = [(30.0 + t * 18.0) as u8, (34.0 + t * 20.0) as u8, (42.0 + t * 24.0) as u8];
        for x in 0..stride {
            let i = (y * stride + x) * 3;
            buf[i..i + 3].copy_from_slice(&c);
        }
    }
    render_into(&mut buf, stride, 0, &atlas, size, size); // left: stock
    render_into(&mut buf, stride, W, &gun, size, size); // right: gunmetal
    image::save_buffer("trooper_gunmetal.png", &buf, stride as u32, H as u32, image::ColorType::Rgb8).unwrap();
    println!("wrote trooper_gunmetal.png  ({stride}x{H})  [ stock armor | gunmetal ]");

    for (name, cell) in [("RustPlate", RUST_CELL), ("HexPlate", HEX_CELL)] {
        let a = cell_mean(&atlas, size, cell);
        let b = cell_mean(&gun, size, cell);
        println!("  {name:<10} stock rgb({:.0},{:.0},{:.0}) [warm/rusty]  →  gunmetal rgb({:.0},{:.0},{:.0}) [cool steel]", a[0], a[1], a[2], b[0], b[1], b[2]);
    }
}

#[allow(clippy::too_many_arguments)]
fn raster(buf: &mut [u8], stride: usize, ox: usize, p: [[f32; 2]; 3], col: [Color32; 3], uv: [[f32; 2]; 3], atlas: &[u8], aw: usize, ah: usize) {
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
            let i = (y * stride + (ox + x)) * 3;
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
