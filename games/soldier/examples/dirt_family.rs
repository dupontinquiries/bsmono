//! dirt_family — the ground-plane material. Granular soil (multi-scale mottle + grain), pebbles embedded and
//! standing proud, desiccation cracks along Voronoi cell edges, moisture that darkens + pools in the low spots
//! with a faint wet sheen, and organic litter for forest floor. Same baked-height lighting as before, plus a
//! small wetness-driven specular. Run: cargo run --example dirt_family
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
/// Voronoi → (cell hash, dist to nearest, dist to 2nd, offset-to-feature). Feeds both cracks (F2−F1) and pebbles.
fn voronoi(x: f32, y: f32, s: u32) -> (f32, f32, f32, (f32, f32)) {
    let (xi, yi) = (x.floor() as i32, y.floor() as i32);
    let (mut f1, mut f2, mut id, mut feat) = (9.0f32, 9.0f32, 0.0f32, (0.0f32, 0.0f32));
    for oy in -1..=1 {
        for ox in -1..=1 {
            let (cx, cy) = (xi + ox, yi + oy);
            let px = cx as f32 + hash2(cx, cy, s);
            let py = cy as f32 + hash2(cx, cy, s ^ 0x9e37);
            let d = ((px - x).powi(2) + (py - y).powi(2)).sqrt();
            if d < f1 {
                f2 = f1;
                f1 = d;
                id = hash2(cx, cy, s ^ 0xB5);
                feat = (px - x, py - y);
            } else if d < f2 {
                f2 = d;
            }
        }
    }
    (id, f1, f2, feat)
}

#[derive(Clone, Copy)]
struct DirtSpec {
    soil: [f32; 3],
    moisture: f32,
    pebbles: f32,
    cracks: f32,
    organic: f32,
}

/// (albedo, height, wetness)
fn dirt_pixel(spec: DirtSpec, u: f32, v: f32) -> ([f32; 3], f32, f32) {
    // ── granular soil base: large patches × medium mottle × fine grain ──
    let patch = fbm(u * 3.5, v * 3.5, 4);
    let med = fbm(u * 14.0, v * 14.0, 7);
    let grain = fbm(u * 55.0, v * 55.0, 9);
    let mut col = mix3(
        [spec.soil[0] * 0.8, spec.soil[1] * 0.8, spec.soil[2] * 0.8],
        [spec.soil[0] * 1.18, spec.soil[1] * 1.18, spec.soil[2] * 1.18],
        clamp01(patch * 0.6 + med * 0.4),
    );
    col = [col[0] * (0.9 + grain * 0.2), col[1] * (0.9 + grain * 0.2), col[2] * (0.9 + grain * 0.2)];
    let mut h = (fbm(u * 22.0, v * 22.0, 11) - 0.5) * 0.3; // soil micro-relief

    // ── organic litter (forest floor): scattered leaf/twig flecks + damp moss patches ──
    if spec.organic > 0.0 {
        let f = fbm(u * 30.0, v * 30.0, 17);
        if f > 0.72 - spec.organic * 0.25 {
            let pick = hash2((u * 30.0) as i32, (v * 30.0) as i32, 0x3);
            let litter = if pick < 0.4 {
                [122.0, 72.0, 40.0] // brown leaf
            } else if pick < 0.75 {
                [150.0, 116.0, 52.0] // tan leaf
            } else {
                [96.0, 84.0, 44.0] // twig
            };
            col = mix3(col, litter, spec.organic * 0.7);
            h += 0.05;
        }
        // moss where damp
        let moss = clamp01(spec.moisture * 1.2 - 0.3) * smoothstep(0.55, 0.8, fbm(u * 8.0, v * 8.0, 23)) * spec.organic;
        col = mix3(col, [78.0, 110.0, 60.0], clamp01(moss));
    }

    // ── desiccation cracks: dark recessed network along Voronoi cell edges ──
    if spec.cracks > 0.0 {
        let (_, f1, f2, _) = voronoi(u * 7.0, v * 7.0, 4);
        let crack = smoothstep(0.05, 0.0, f2 - f1) * spec.cracks;
        col = mix3(col, [46.0, 36.0, 28.0], crack * 0.8);
        h -= crack * 0.45;
    }

    // ── embedded pebbles: raised stones with a shadow foot ──
    if spec.pebbles > 0.0 {
        let (id, f1, _, _) = voronoi(u * 12.0, v * 12.0, 71);
        if id < spec.pebbles {
            let r = 0.28 + id * 0.5; // stone radius (in cell units)
            let inside = smoothstep(r, r * 0.6, f1);
            let stone = mix3([150.0, 142.0, 130.0], [110.0, 106.0, 100.0], id * 3.0 % 1.0);
            col = mix3(col, stone, inside * 0.9);
            h += inside * 0.5;
            // shadow ring just outside the stone
            let ring = smoothstep(r, r * 1.25, f1) * (1.0 - inside);
            col = [col[0] * (1.0 - ring * 0.3), col[1] * (1.0 - ring * 0.3), col[2] * (1.0 - ring * 0.3)];
        }
    }

    // ── moisture: darkens, and pools deeper in the low spots; drives a wet sheen ──
    let low = smoothstep(0.1, -0.3, h); // 1 in hollows
    let wet = clamp01(spec.moisture * (0.55 + low * 0.6));
    col = [col[0] * (1.0 - wet * 0.42), col[1] * (1.0 - wet * 0.4), col[2] * (1.0 - wet * 0.38)];

    // baked AO from the low spots + micro
    let ao = 1.0 - low * 0.25;
    let micro = 1.0 + (hash2((u * 300.0) as i32, (v * 300.0) as i32, 0xA1) - 0.5) * 0.05;
    for k in col.iter_mut() {
        *k = (*k * ao * micro).clamp(0.0, 255.0);
    }
    (col, h, wet)
}

fn shade_cell(spec: DirtSpec, buf: &mut [u8], w: usize, ox: usize, oy: usize) {
    let e = REPEAT / CELL as f32;
    for py in 0..CELL {
        for px in 0..CELL {
            let u = (px as f32 / CELL as f32 * REPEAT).fract();
            let v = (py as f32 / CELL as f32 * REPEAT).fract();
            let (alb, _, wet) = dirt_pixel(spec, u, v);
            let hl = dirt_pixel(spec, (u - e + 1.0).fract(), v).1;
            let hr = dirt_pixel(spec, (u + e).fract(), v).1;
            let hd = dirt_pixel(spec, u, (v - e + 1.0).fract()).1;
            let hu = dirt_pixel(spec, u, (v + e).fract()).1;
            let (nx, ny, nz) = ((hl - hr) * 4.0, (hd - hu) * 4.0, 1.0);
            let inv = 1.0 / (nx * nx + ny * ny + nz * nz).sqrt();
            let (nx, ny, nz) = (nx * inv, ny * inv, nz * inv);
            let n = 1.0 / (0.5f32 * 0.5 + 0.55 * 0.55 + 0.66 * 0.66).sqrt();
            let (lx, ly, lz) = (-0.5 * n, -0.55 * n, 0.66 * n);
            let diff = (nx * lx + ny * ly + nz * lz).max(0.0);
            // wet sheen: a mild specular only where wet
            let hnz = lz + 1.0;
            let hn = 1.0 / (lx * lx + ly * ly + hnz * hnz).sqrt();
            let sp = (nx * lx * hn + ny * ly * hn + nz * hnz * hn).max(0.0).powf(18.0) * wet * 0.6;
            let shade = 0.5 + 0.6 * diff;
            let i = ((oy + py) * w + (ox + px)) * 3;
            buf[i] = (alb[0] * shade + 210.0 * sp).clamp(0.0, 255.0) as u8;
            buf[i + 1] = (alb[1] * shade + 214.0 * sp).clamp(0.0, 255.0) as u8;
            buf[i + 2] = (alb[2] * shade + 220.0 * sp).clamp(0.0, 255.0) as u8;
        }
    }
}

fn catalog() -> Vec<(&'static str, DirtSpec)> {
    vec![
        ("dry packed earth", DirtSpec { soil: [128.0, 100.0, 72.0], moisture: 0.1, pebbles: 0.28, cracks: 0.12, organic: 0.08 }),
        ("wet mud", DirtSpec { soil: [92.0, 68.0, 48.0], moisture: 0.9, pebbles: 0.18, cracks: 0.0, organic: 0.12 }),
        ("cracked clay playa", DirtSpec { soil: [178.0, 150.0, 116.0], moisture: 0.04, pebbles: 0.08, cracks: 0.92, organic: 0.0 }),
        ("gravelly dirt", DirtSpec { soil: [132.0, 114.0, 92.0], moisture: 0.2, pebbles: 0.85, cracks: 0.0, organic: 0.05 }),
        ("forest humus", DirtSpec { soil: [82.0, 64.0, 46.0], moisture: 0.55, pebbles: 0.2, cracks: 0.0, organic: 0.78 }),
        ("sandy desert", DirtSpec { soil: [198.0, 174.0, 132.0], moisture: 0.0, pebbles: 0.14, cracks: 0.22, organic: 0.0 }),
    ]
}

fn measure(spec: DirtSpec) -> (f32, f32, f32, f32) {
    // (brightness, relief = height range, low_frac = cracks+hollows, pebble_frac)
    let (mut b, mut n, mut lo, mut hi, mut low, mut peb) = (0.0, 0u32, f32::MAX, f32::MIN, 0.0, 0.0);
    for j in 0..150 {
        for i in 0..150 {
            let (u, v) = (i as f32 / 150.0, j as f32 / 150.0);
            let (c, hh, _) = dirt_pixel(spec, u, v);
            b += (c[0] + c[1] + c[2]) / 3.0;
            lo = lo.min(hh);
            hi = hi.max(hh);
            if hh < -0.15 {
                low += 1.0;
            }
            // pebble coverage: same test the pixel uses
            let (id, f1, _, _) = voronoi(u * 12.0, v * 12.0, 71);
            if spec.pebbles > 0.0 && id < spec.pebbles && f1 < (0.28 + id * 0.5) * 0.8 {
                peb += 1.0;
            }
            n += 1;
        }
    }
    let f = n as f32;
    (b / f, hi - lo, low / f * 100.0, peb / f * 100.0)
}

fn main() {
    let cat = catalog();
    let w = CELL * COLS;
    let h = CELL * ROWS_GRID;
    let mut buf = vec![14u8; w * h * 3];
    for (i, (_n, spec)) in cat.iter().enumerate() {
        shade_cell(*spec, &mut buf, w, (i % COLS) * CELL, (i / COLS) * CELL);
    }
    image::save_buffer("dirt_family.png", &buf, w as u32, h as u32, image::ColorType::Rgb8).unwrap();
    println!("wrote dirt_family.png  ({w}x{h})  {}", cat.iter().map(|(n, _)| *n).collect::<Vec<_>>().join(", "));
    println!("\n  dirt                    brightness   relief   low/crack%   pebble%");
    for (n, spec) in &cat {
        let (b, r, l, p) = measure(*spec);
        println!("    {n:<22} {b:>8.0}   {r:>5.2}   {l:>6.1}   {p:>6.1}");
    }
}

#[cfg(test)]
mod checks {
    use super::*;
    #[test]
    fn ground_reads_correctly() {
        let cat = catalog();
        let dry = cat[0].1;
        let wet = cat[1].1;
        let cracked = cat[2].1;
        let gravel = cat[3].1;
        // wet mud is darker than dry earth
        assert!(measure(wet).0 < measure(dry).0, "wet should darken: {} vs {}", measure(wet).0, measure(dry).0);
        // gravelly dirt has more relief than packed earth
        assert!(measure(gravel).3 > measure(dry).3 + 5.0, "gravel should have more stones: {} vs {}", measure(gravel).3, measure(dry).3);
        // cracked clay has a real crack network (recessed lines) that packed earth doesn't
        assert!(measure(cracked).2 > measure(dry).2 + 3.0, "cracked clay should crack: {} vs {}", measure(cracked).2, measure(dry).2);
    }
}
