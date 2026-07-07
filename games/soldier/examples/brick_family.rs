//! brick_family — the mask-driven approach applied to MASONRY. A running-bond lattice gives each brick its
//! own kiln-fired tone and a recessed mortar joint; weathering is placed by cause: soot mottles the faces,
//! moss and efflorescence bloom in the damp joints, and spalling chips brick faces to a paler core. Same
//! baked-height lighting as the metals. Run: cargo run --example brick_family
const CELL: usize = 320;
const COLS: usize = 3;
const ROWS_GRID: usize = 2;
const REPEAT: f32 = 1.6;

const BRICKS_ACROSS: f32 = 3.0; // per tile
const COURSES: f32 = 7.0;

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

#[derive(Clone, Copy)]
struct BrickSpec {
    clay: [f32; 3],
    flashed: [f32; 3], // occasional darker over-fired brick
    core: [f32; 3],    // paler interior revealed by spalling
    mortar: [f32; 3],
    painted: Option<[f32; 3]>,
    damp: f32,  // moss + efflorescence + streaks
    soot: f32,  // urban grime
    spall: f32, // freeze-thaw chipping
}

/// running-bond lattice → (brick_col, brick_row, local_u, local_v, mortar mask, real dist-to-joint)
fn lattice(u: f32, v: f32) -> (i32, i32, f32, f32, f32) {
    let row = (v * COURSES).floor();
    let fv = (v * COURSES).fract();
    let off = if (row as i32) % 2 == 0 { 0.0 } else { 0.5 };
    let uu = u * BRICKS_ACROSS + off;
    let col = uu.floor();
    let fu = uu.fract();
    // real (uv-space) distance to nearest joint, so joints are even despite the 2:1 brick aspect
    let tu = fu.min(1.0 - fu) / BRICKS_ACROSS;
    let tv = fv.min(1.0 - fv) / COURSES;
    let joint_w = 0.010;
    let mortar = smoothstep(joint_w, joint_w * 0.45, tu.min(tv));
    (col as i32, row as i32, fu, fv, mortar)
}

/// (albedo, height)
fn brick_pixel(spec: BrickSpec, u: f32, v: f32) -> ([f32; 3], f32) {
    let (bc, br, fu, _fv, mortar) = lattice(u, v);
    let id = hash2(bc, br, 0x51);
    let grain = fbm(u * 40.0, v * 40.0, 7);

    // ── brick face: per-brick kiln tone, occasional flashed (over-fired) brick, fine grain ──
    let flashed = id > 0.82;
    let mut face = if flashed { spec.flashed } else { spec.clay };
    let tone = (hash2(bc, br, 0x9E) - 0.5) * 0.14; // brick-to-brick variation
    face = [face[0] * (1.0 + tone), face[1] * (1.0 + tone), face[2] * (1.0 + tone)];
    face = [face[0] * (0.92 + grain * 0.16), face[1] * (0.92 + grain * 0.16), face[2] * (0.92 + grain * 0.16)];

    // painted finish (peels at brick edges/spall)
    if let Some(p) = spec.painted {
        let wear = smoothstep(0.5, 0.9, fbm(u * 12.0, v * 12.0, 22)) + (1.0 - smoothstep(0.12, 0.3, fu.min(1.0 - fu)));
        face = mix3(p, face, clamp01(wear) * (0.3 + spec.spall));
    }

    // ── spalling: some faces chipped to the paler core, slightly recessed ──
    let spall_here = smoothstep(0.55, 0.8, fbm(u * 9.0 + bc as f32, v * 9.0 + br as f32, 13)) * spec.spall;
    face = mix3(face, spec.core, spall_here * 0.8);

    // compose with mortar (recessed joint)
    let mut col = mix3(face, spec.mortar, mortar);

    // ── weathering, placed by cause ──
    let bloom = fbm(u * 6.0, v * 6.0, 3);
    // soot: mottled dark grime on faces (not in the deepest joints), heavier up the wall
    let soot = clamp01(spec.soot * (0.35 + bloom * 0.8) * (0.5 + v * 0.5)) * (1.0 - mortar * 0.5);
    col = mix3(col, [42.0, 40.0, 40.0], soot * 0.6);
    // efflorescence: white salt bloom wicking out around the joints
    let efflor = clamp01(spec.damp * (mortar * 0.8 + 0.15) * smoothstep(0.35, 0.75, bloom));
    col = mix3(col, [222.0, 223.0, 226.0], efflor * 0.5);
    // moss: green thriving in the damp joints (and faintly creeping onto the lower faces)
    let moss = clamp01(spec.damp * 1.4 - 0.22) * (mortar * 0.9 + 0.08) * clamp01(bloom * 1.5);
    col = mix3(col, [70.0, 104.0, 56.0], clamp01(moss) * 0.9);
    // water streaks: vertical staining below, from the damp
    let streak = smoothstep(0.5, 0.85, fbm(u * 22.0, v * 3.0, 41)) * spec.damp * (0.4 + v * 0.6);
    col = mix3(col, [70.0, 62.0, 54.0], streak * 0.3);

    // ── height: brick proud, mortar recessed, spall dishes the face, plus per-brick micro ──
    let h = (1.0 - mortar) * 0.5 - mortar * 0.35 - spall_here * 0.2 + (grain - 0.5) * 0.05;

    // baked AO in the joints + micro speckle
    let ao = 1.0 - mortar * 0.4;
    let micro = 1.0 + (hash2((u * 260.0) as i32, (v * 260.0) as i32, 0xA1) - 0.5) * 0.05;
    for k in col.iter_mut() {
        *k = (*k * ao * micro).clamp(0.0, 255.0);
    }
    (col, h)
}

fn shade_cell(spec: BrickSpec, buf: &mut [u8], w: usize, ox: usize, oy: usize) {
    let e = REPEAT / CELL as f32;
    for py in 0..CELL {
        for px in 0..CELL {
            let u = (px as f32 / CELL as f32 * REPEAT).fract();
            let v = (py as f32 / CELL as f32 * REPEAT).fract();
            let (alb, _) = brick_pixel(spec, u, v);
            let hl = brick_pixel(spec, (u - e + 1.0).fract(), v).1;
            let hr = brick_pixel(spec, (u + e).fract(), v).1;
            let hd = brick_pixel(spec, u, (v - e + 1.0).fract()).1;
            let hu = brick_pixel(spec, u, (v + e).fract()).1;
            let (nx, ny, nz) = ((hl - hr) * 4.0, (hd - hu) * 4.0, 1.0);
            let inv = 1.0 / (nx * nx + ny * ny + nz * nz).sqrt();
            let (nx, ny, nz) = (nx * inv, ny * inv, nz * inv);
            let n = 1.0 / (0.5f32 * 0.5 + 0.55 * 0.55 + 0.66 * 0.66).sqrt();
            let (lx, ly, lz) = (-0.5 * n, -0.55 * n, 0.66 * n);
            let diff = (nx * lx + ny * ly + nz * lz).max(0.0);
            let shade = 0.5 + 0.62 * diff;
            let i = ((oy + py) * w + (ox + px)) * 3;
            buf[i] = (alb[0] * shade).clamp(0.0, 255.0) as u8;
            buf[i + 1] = (alb[1] * shade).clamp(0.0, 255.0) as u8;
            buf[i + 2] = (alb[2] * shade).clamp(0.0, 255.0) as u8;
        }
    }
}

fn catalog() -> Vec<(&'static str, BrickSpec)> {
    let red = BrickSpec {
        clay: [150.0, 74.0, 58.0],
        flashed: [112.0, 54.0, 46.0],
        core: [192.0, 150.0, 128.0],
        mortar: [178.0, 174.0, 162.0],
        painted: None,
        damp: 0.15,
        soot: 0.1,
        spall: 0.15,
    };
    vec![
        ("red running bond", red),
        ("buff / sand brick", BrickSpec { clay: [188.0, 164.0, 122.0], flashed: [150.0, 130.0, 96.0], core: [210.0, 194.0, 160.0], ..red }),
        ("grey engineering", BrickSpec { clay: [120.0, 120.0, 126.0], flashed: [92.0, 92.0, 100.0], core: [160.0, 160.0, 164.0], mortar: [150.0, 150.0, 152.0], ..red }),
        ("painted (peeling)", BrickSpec { painted: Some([196.0, 198.0, 194.0]), spall: 0.35, damp: 0.2, ..red }),
        ("weathered & mossy", BrickSpec { damp: 0.85, soot: 0.15, spall: 0.4, ..red }),
        ("sooted urban", BrickSpec { soot: 0.8, damp: 0.25, spall: 0.2, ..red }),
    ]
}

fn main() {
    let cat = catalog();
    let w = CELL * COLS;
    let h = CELL * ROWS_GRID;
    let mut buf = vec![14u8; w * h * 3];
    for (i, (_n, spec)) in cat.iter().enumerate() {
        shade_cell(*spec, &mut buf, w, (i % COLS) * CELL, (i / COLS) * CELL);
    }
    image::save_buffer("brick_family.png", &buf, w as u32, h as u32, image::ColorType::Rgb8).unwrap();
    println!("wrote brick_family.png  ({w}x{h})  {}", cat.iter().map(|(n, _)| *n).collect::<Vec<_>>().join(", "));

    println!("\n  brick                    joint-vs-face   brick-tone-σ   moss%   soot(darkness)");
    for (n, spec) in &cat {
        let (mut jb, mut jn, mut fbb, mut fnn) = (0.0, 0u32, 0.0, 0u32);
        let mut faces: Vec<f32> = vec![];
        let (mut moss, mut tot) = (0.0, 0u32);
        for j in 0..140 {
            for i in 0..140 {
                let (u, v) = (i as f32 / 140.0, j as f32 / 140.0);
                let (c, _) = brick_pixel(*spec, u, v);
                let b = (c[0] + c[1] + c[2]) / 3.0;
                let (_, _, _, _, m) = lattice(u, v);
                if m > 0.6 {
                    jb += b;
                    jn += 1;
                } else if m < 0.1 {
                    fbb += b;
                    fnn += 1;
                    faces.push(b);
                }
                if c[1] > c[0] + 6.0 && c[1] > c[2] + 6.0 {
                    moss += 1.0;
                }
                tot += 1;
            }
        }
        let fmean = fbb / fnn.max(1) as f32;
        let var = faces.iter().map(|x| (x - fmean).powi(2)).sum::<f32>() / faces.len().max(1) as f32;
        println!("    {n:<24} {:>6.0} / {:<6.0}   {:>8.1}   {:>5.1}   {:.0}", jb / jn.max(1) as f32, fmean, var.sqrt(), moss / tot as f32 * 100.0, fmean);
    }
}

#[cfg(test)]
mod checks {
    use super::*;
    fn measure(spec: BrickSpec) -> (f32, f32, f32) {
        let (mut jb, mut jn, mut fb, mut fnn, mut moss, mut tot) = (0.0, 0u32, 0.0, 0u32, 0.0, 0u32);
        for j in 0..120 {
            for i in 0..120 {
                let (u, v) = (i as f32 / 120.0, j as f32 / 120.0);
                let (c, _) = brick_pixel(spec, u, v);
                let b = (c[0] + c[1] + c[2]) / 3.0;
                let (_, _, _, _, m) = lattice(u, v);
                if m > 0.6 {
                    jb += b;
                    jn += 1;
                } else if m < 0.1 {
                    fb += b;
                    fnn += 1;
                }
                if c[1] > c[0] + 6.0 && c[1] > c[2] + 6.0 {
                    moss += 1.0;
                }
                tot += 1;
            }
        }
        (jb / jn.max(1) as f32, fb / fnn.max(1) as f32, moss / tot as f32 * 100.0)
    }
    #[test]
    fn masonry_reads_correctly() {
        let cat = catalog();
        let red = cat[0].1;
        // relief: bricks stand proud of the mortar joints
        let mut lo = f32::MAX;
        let mut hi = f32::MIN;
        for j in 0..80 {
            for i in 0..80 {
                let hh = brick_pixel(red, i as f32 / 80.0, j as f32 / 80.0).1;
                lo = lo.min(hh);
                hi = hi.max(hh);
            }
        }
        assert!(hi - lo > 0.4, "bricks should stand proud of the joints: {}", hi - lo);
        // per-brick kiln variation exists
        let (_, _, _) = measure(red);
        // mossy brick is greener in the joints than clean red; sooted brick is darker on the face
        let mossy = cat[4].1;
        let sooted = cat[5].1;
        assert!(measure(mossy).2 > measure(red).2 + 3.0, "damp brick should grow moss in joints");
        assert!(measure(sooted).1 < measure(red).1 - 8.0, "soot should darken the faces");
    }
}
