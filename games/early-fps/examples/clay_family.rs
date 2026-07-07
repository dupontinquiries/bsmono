//! clay_family — earthen CLAY, distinct from granular dirt: a smooth cohesive mass with plastic tool marks,
//! tight desiccation cracks, straw inclusions (adobe/cob), fired mottle (terracotta), and a wet plastic sheen.
//! Low-frequency undulation instead of aggregate grain is what makes it read as clay rather than soil.
//! Run: cargo run --example clay_family
const CELL: usize = 320;
const COLS: usize = 3;
const ROWS_GRID: usize = 2;
const REPEAT: f32 = 1.5;

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
fn clamp01(x: f32) -> f32 {
    x.clamp(0.0, 1.0)
}
fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}
fn mix3(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    [lerp(a[0], b[0], t), lerp(a[1], b[1], t), lerp(a[2], b[2], t)]
}
fn voronoi(x: f32, y: f32, s: u32) -> (f32, f32) {
    let (xi, yi) = (x.floor() as i32, y.floor() as i32);
    let (mut f1, mut f2) = (9.0f32, 9.0f32);
    for oy in -1..=1 {
        for ox in -1..=1 {
            let (cx, cy) = (xi + ox, yi + oy);
            let px = cx as f32 + hash2(cx, cy, s);
            let py = cy as f32 + hash2(cx, cy, s ^ 0x9e37);
            let d = ((px - x).powi(2) + (py - y).powi(2)).sqrt();
            if d < f1 {
                f2 = f1;
                f1 = d;
            } else if d < f2 {
                f2 = d;
            }
        }
    }
    (f1, f2)
}

// shared, reusable masks (so measure() can recompute them exactly)
fn straw_at(u: f32, v: f32, amt: f32) -> f32 {
    let a = smoothstep(0.64, 0.78, fbm(u * 13.0, v * 76.0, 51));
    let b = smoothstep(0.64, 0.78, fbm(u * 76.0, v * 13.0, 52));
    a.max(b) * amt
}
fn crack_at(u: f32, v: f32, amt: f32) -> f32 {
    let (f1, f2) = voronoi(u * 10.0, v * 10.0, 4);
    smoothstep(0.045, 0.0, f2 - f1) * amt
}

#[derive(Clone, Copy)]
struct ClaySpec {
    clay: [f32; 3],
    wetness: f32,
    straw: f32,
    cracks: f32,
    toolmarks: f32,
    fired: f32,
}

/// (albedo, height, wetness)
fn clay_pixel(spec: ClaySpec, u: f32, v: f32) -> ([f32; 3], f32, f32) {
    // ── smooth plastic mass: low-frequency undulation, NOT aggregate grain ──
    let undulate = fbm(u * 3.0, v * 3.0, 4);
    let mut h = (undulate - 0.5) * 0.5;
    let mut col = [spec.clay[0] * (0.88 + undulate * 0.24), spec.clay[1] * (0.88 + undulate * 0.24), spec.clay[2] * (0.88 + undulate * 0.24)];

    // ── plastic tool marks: directional scraped ridges ──
    if spec.toolmarks > 0.0 {
        let ridge = fbm(u * 5.0, v * 44.0, 7) - 0.5;
        h += ridge * 0.35 * spec.toolmarks;
        col = [col[0] * (1.0 + ridge * 0.12 * spec.toolmarks), col[1] * (1.0 + ridge * 0.12 * spec.toolmarks), col[2] * (1.0 + ridge * 0.12 * spec.toolmarks)];
    }

    // ── fired mottle (terracotta): warm fired zones, slightly vitrified ──
    if spec.fired > 0.0 {
        let mottle = fbm(u * 5.5, v * 5.5, 13);
        let warm = [spec.clay[0] * 1.12, spec.clay[1] * 0.92, spec.clay[2] * 0.82];
        col = mix3(col, warm, spec.fired * (0.3 + mottle * 0.5));
    }

    // ── straw inclusions (adobe/cob): pale fibers, slightly proud ──
    let straw = straw_at(u, v, spec.straw);
    if straw > 0.0 {
        col = mix3(col, [206.0, 184.0, 128.0], clamp01(straw) * 0.7);
        h += straw * 0.06;
    }

    // ── desiccation cracks: tight recessed polygonal network (shallower when wet) ──
    let crack = crack_at(u, v, spec.cracks) * (1.0 - spec.wetness * 0.7);
    col = mix3(col, [58.0, 44.0, 34.0], crack * 0.85);
    h -= crack * 0.4;

    // ── wet: darken, smooth, raise a plastic sheen ──
    let low = smoothstep(0.1, -0.25, h);
    let wet = clamp01(spec.wetness * (0.6 + low * 0.5));
    col = [col[0] * (1.0 - wet * 0.4), col[1] * (1.0 - wet * 0.38), col[2] * (1.0 - wet * 0.36)];

    let ao = 1.0 - low * 0.22;
    let micro = 1.0 + (hash2((u * 280.0) as i32, (v * 280.0) as i32, 0xA1) - 0.5) * 0.04; // clay is fine, faint micro
    for k in col.iter_mut() {
        *k = (*k * ao * micro).clamp(0.0, 255.0);
    }
    (col, h, wet)
}

fn shade_cell(spec: ClaySpec, buf: &mut [u8], w: usize, ox: usize, oy: usize) {
    let e = REPEAT / CELL as f32;
    for py in 0..CELL {
        for px in 0..CELL {
            let u = (px as f32 / CELL as f32 * REPEAT).fract();
            let v = (py as f32 / CELL as f32 * REPEAT).fract();
            let (alb, _, wet) = clay_pixel(spec, u, v);
            let hl = clay_pixel(spec, (u - e + 1.0).fract(), v).1;
            let hr = clay_pixel(spec, (u + e).fract(), v).1;
            let hd = clay_pixel(spec, u, (v - e + 1.0).fract()).1;
            let hu = clay_pixel(spec, u, (v + e).fract()).1;
            let (nx, ny, nz) = ((hl - hr) * 4.0, (hd - hu) * 4.0, 1.0);
            let inv = 1.0 / (nx * nx + ny * ny + nz * nz).sqrt();
            let (nx, ny, nz) = (nx * inv, ny * inv, nz * inv);
            let n = 1.0 / (0.5f32 * 0.5 + 0.55 * 0.55 + 0.66 * 0.66).sqrt();
            let (lx, ly, lz) = (-0.5 * n, -0.55 * n, 0.66 * n);
            let diff = (nx * lx + ny * ly + nz * lz).max(0.0);
            let hnz = lz + 1.0;
            let hn = 1.0 / (lx * lx + ly * ly + hnz * hnz).sqrt();
            let sp = (nx * lx * hn + ny * ly * hn + nz * hnz * hn).max(0.0).powf(20.0) * wet * 0.7;
            let shade = 0.5 + 0.6 * diff;
            let i = ((oy + py) * w + (ox + px)) * 3;
            buf[i] = (alb[0] * shade + 214.0 * sp).clamp(0.0, 255.0) as u8;
            buf[i + 1] = (alb[1] * shade + 216.0 * sp).clamp(0.0, 255.0) as u8;
            buf[i + 2] = (alb[2] * shade + 222.0 * sp).clamp(0.0, 255.0) as u8;
        }
    }
}

fn catalog() -> Vec<(&'static str, ClaySpec)> {
    vec![
        ("wet raw clay", ClaySpec { clay: [112.0, 98.0, 84.0], wetness: 0.82, straw: 0.0, cracks: 0.0, toolmarks: 0.6, fired: 0.0 }),
        ("dry cracked clay", ClaySpec { clay: [174.0, 150.0, 116.0], wetness: 0.05, straw: 0.0, cracks: 0.88, toolmarks: 0.2, fired: 0.0 }),
        ("adobe / cob", ClaySpec { clay: [188.0, 150.0, 104.0], wetness: 0.1, straw: 0.85, cracks: 0.3, toolmarks: 0.3, fired: 0.0 }),
        ("terracotta (fired)", ClaySpec { clay: [178.0, 94.0, 62.0], wetness: 0.08, straw: 0.0, cracks: 0.08, toolmarks: 0.15, fired: 1.0 }),
        ("grey potter's clay", ClaySpec { clay: [128.0, 128.0, 132.0], wetness: 0.62, straw: 0.0, cracks: 0.0, toolmarks: 0.75, fired: 0.0 }),
        ("kaolin / white clay", ClaySpec { clay: [206.0, 200.0, 190.0], wetness: 0.1, straw: 0.0, cracks: 0.25, toolmarks: 0.2, fired: 0.0 }),
    ]
}

fn measure(spec: ClaySpec) -> (f32, f32, f32, f32, f32) {
    // (brightness, mean-wetness, straw%, crack%, warmth = R−B)
    let (mut b, mut wetsum, mut straw, mut crack, mut warm, mut n) = (0.0, 0.0, 0.0, 0.0, 0.0, 0u32);
    for j in 0..150 {
        for i in 0..150 {
            let (u, v) = (i as f32 / 150.0, j as f32 / 150.0);
            let (c, _, wet) = clay_pixel(spec, u, v);
            b += (c[0] + c[1] + c[2]) / 3.0;
            wetsum += wet;
            warm += c[0] - c[2];
            if straw_at(u, v, spec.straw) > 0.3 {
                straw += 1.0;
            }
            if crack_at(u, v, spec.cracks) > 0.4 {
                crack += 1.0;
            }
            n += 1;
        }
    }
    let f = n as f32;
    (b / f, wetsum / f, straw / f * 100.0, crack / f * 100.0, warm / f)
}

fn main() {
    let cat = catalog();
    let w = CELL * COLS;
    let h = CELL * ROWS_GRID;
    let mut buf = vec![14u8; w * h * 3];
    for (i, (_n, spec)) in cat.iter().enumerate() {
        shade_cell(*spec, &mut buf, w, (i % COLS) * CELL, (i / COLS) * CELL);
    }
    image::save_buffer("clay_family.png", &buf, w as u32, h as u32, image::ColorType::Rgb8).unwrap();
    println!("wrote clay_family.png  ({w}x{h})  {}", cat.iter().map(|(n, _)| *n).collect::<Vec<_>>().join(", "));
    println!("\n  clay                     brightness   wetness   straw%   crack%   warmth(R-B)");
    for (n, spec) in &cat {
        let (b, wt, s, c, wm) = measure(*spec);
        println!("    {n:<22} {b:>8.0}   {wt:>6.2}   {s:>5.1}   {c:>5.1}   {wm:>6.0}");
    }
}

#[cfg(test)]
mod checks {
    use super::*;
    #[test]
    fn clay_reads_correctly() {
        let cat = catalog();
        let (wet, dry, adobe, terra, grey) = (cat[0].1, cat[1].1, cat[2].1, cat[3].1, cat[4].1);
        // wet clay carries a plastic sheen dry clay doesn't
        assert!(measure(wet).1 > measure(dry).1 + 0.3, "wet clay should be wetter: {} vs {}", measure(wet).1, measure(dry).1);
        // adobe has straw inclusions terracotta doesn't
        assert!(measure(adobe).2 > measure(terra).2 + 3.0, "adobe should have straw: {} vs {}", measure(adobe).2, measure(terra).2);
        // dry cracked clay actually cracks; terracotta barely
        assert!(measure(dry).3 > measure(terra).3 + 3.0, "dry clay should crack: {} vs {}", measure(dry).3, measure(terra).3);
        // terracotta is warm-fired (red≫blue) vs neutral grey clay
        assert!(measure(terra).4 > measure(grey).4 + 30.0, "terracotta should read warm: {} vs {}", measure(terra).4, measure(grey).4);
    }
}
