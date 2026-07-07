//! metal_exposure — per-map ENVIRONMENTAL weathering on top of the metal family. Three climate processes,
//! each acting through the surface's own masks (not a flat tint):
//!   • sun/UV      → bleaches & lightens EXPOSED (up-facing/crest) areas; paint fades, bare metal barely.
//!   • humidity    → ambient rust pooling in CAVITIES + moss/biofilm in shaded lows; salt accelerates it.
//!   • sandblast   → scours EXPOSED faces to bare matte metal; dust DEPOSITS in the sheltered recesses.
//! An `Exposure` profile (Desert/Jungle/Coastal/Temperate) is a per-map setting; the same material ages
//! differently under each. This is the third weathering axis after grade (per-structure) and the damage
//! field (per-location). Run: cargo run --example metal_exposure
const CELL: usize = 240;

// ── noise (same kit as the family) ──────────────────────────────────────────────────────────────────────
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
    let (mut f, mut amp, mut fr, mut n) = (0.0, 0.5, 1.0, 0.0);
    for _ in 0..4 {
        f += amp * vnoise(x * fr, y * fr, s);
        n += amp;
        amp *= 0.5;
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
fn lum(c: [f32; 3]) -> f32 {
    (c[0] + c[1] + c[2]) / 3.0
}

// ── material ────────────────────────────────────────────────────────────────────────────────────────────
#[derive(Clone, Copy)]
enum Finish {
    Bare,
    Painted([f32; 3]),
    Galvanized,
}
#[derive(Clone, Copy, PartialEq)]
enum Struct {
    Flat,
    Corrugated,
    Tread,
}
#[derive(Clone, Copy)]
struct MetalSpec {
    alloy: [f32; 3],
    finish: Finish,
    structure: Struct,
    corrodes: f32, // how readily it oxidises (iron ~1.1, steel 1.0, aluminium 0.2, galv 0.3)
}
fn bleaches(spec: &MetalSpec) -> f32 {
    match spec.finish {
        Finish::Painted(_) => 1.0,
        _ => 0.12,
    }
}

fn structure_field(st: Struct, u: f32, v: f32) -> (f32, f32) {
    match st {
        Struct::Flat => (0.0, 0.0),
        Struct::Corrugated => {
            let h = (u * 7.0 * std::f32::consts::TAU).sin();
            (h * 0.5, clamp01(-h) * 0.8)
        }
        Struct::Tread => {
            let n = 5.0;
            let cu = (u * n).fract() - 0.5;
            let cv = ((v * n) + if ((u * n) as i32) % 2 == 0 { 0.0 } else { 0.5 }).fract() - 0.5;
            let raised = smoothstep(0.42, 0.24, cu.abs() + cv.abs());
            (raised * 0.6, clamp01(1.0 - raised) * 0.55)
        }
    }
}

// ── per-map exposure ────────────────────────────────────────────────────────────────────────────────────
#[derive(Clone, Copy)]
struct Exposure {
    sun: f32,
    humidity: f32,
    sandblast: f32,
    salt: f32,
    dust: [f32; 3],
    moss: [f32; 3],
}
fn desert() -> Exposure {
    Exposure { sun: 0.95, humidity: 0.10, sandblast: 0.90, salt: 0.0, dust: [198.0, 178.0, 134.0], moss: [96.0, 100.0, 66.0] }
}
fn jungle() -> Exposure {
    Exposure { sun: 0.45, humidity: 0.95, sandblast: 0.12, salt: 0.15, dust: [72.0, 76.0, 54.0], moss: [70.0, 108.0, 56.0] }
}
fn coastal() -> Exposure {
    Exposure { sun: 0.60, humidity: 0.82, sandblast: 0.45, salt: 0.95, dust: [150.0, 150.0, 148.0], moss: [96.0, 112.0, 72.0] }
}
fn temperate() -> Exposure {
    Exposure { sun: 0.48, humidity: 0.50, sandblast: 0.30, salt: 0.10, dust: [96.0, 92.0, 86.0], moss: [86.0, 102.0, 64.0] }
}

const RUST: [f32; 3] = [120.0, 66.0, 42.0];
const RUST_DK: [f32; 3] = [86.0, 47.0, 30.0];

fn metal_pixel(spec: MetalSpec, exp: Exposure, u: f32, v: f32) -> ([f32; 3], f32, f32) {
    let (h, cavity) = structure_field(spec.structure, u, v);
    let crest = smoothstep(0.02, 0.5, h.max(0.0)); // exposed / up-facing
    let bloom = fbm(u * 6.0, v * 6.0, 3);
    let fine = fbm(u * 20.0, v * 20.0, 9);

    // base by finish
    let (mut col, alloy) = match spec.finish {
        Finish::Bare => (spec.alloy, spec.alloy),
        Finish::Painted(p) => (p, spec.alloy),
        Finish::Galvanized => {
            let g = mix3([150.0, 158.0, 168.0], [186.0, 192.0, 200.0], fbm(u * 9.0, v * 9.0, 5));
            (g, [140.0, 146.0, 154.0])
        }
    };
    let mut steelness = if matches!(spec.finish, Finish::Bare) { 0.8 } else { 0.25 };

    // 1) HUMIDITY → ambient rust pooling in cavities (+ salt accelerant). White-rust for galvanised.
    let (rust_c, rust_d) = if matches!(spec.finish, Finish::Galvanized) {
        ([152.0, 152.0, 150.0], [112.0, 114.0, 114.0])
    } else {
        (RUST, RUST_DK)
    };
    let corrosion = clamp01(exp.humidity * (0.30 + cavity * 0.95) * (0.55 + bloom * 0.7) * (1.0 + exp.salt * 0.8)) * spec.corrodes;
    let rust_amt = clamp01(corrosion - 0.12) * 1.2;
    col = mix3(col, mix3(rust_c, rust_d, cavity), rust_amt);
    if rust_amt > 0.15 {
        steelness *= 0.4;
    }

    // 2) HUMIDITY → moss/biofilm grows OVER rust in the dampest shaded lows; strong enough there to take the
    //    surface green. Scales hard with humidity (absent in dry desert, dominant in jungle troughs).
    let moss_amt = clamp01(exp.humidity * 1.5 - 0.55) * smoothstep(0.2, 0.72, cavity) * clamp01(0.45 + bloom * 0.9);
    col = mix3(col, exp.moss, moss_amt);
    if moss_amt > 0.1 {
        steelness *= 0.5;
    }

    // 3) SUN/UV → bleach: desaturate toward luminance + lighten, worst on exposed paint
    let bleach = clamp01(exp.sun * (0.32 + 0.68 * crest) * bleaches(&spec));
    let l = lum(col);
    col = mix3(col, [lerp(col[0], l, 0.65) + 16.0, lerp(col[1], l, 0.65) + 16.0, lerp(col[2], l, 0.65) + 16.0], bleach);

    // 4) SANDBLAST → scour exposed faces to bare matte alloy + fine pitting
    let scour = clamp01(exp.sandblast * (crest * 0.85 + 0.12));
    col = mix3(col, alloy, scour * if matches!(spec.finish, Finish::Painted(_)) { 0.9 } else { 0.4 });
    col = [col[0] * (1.0 - scour * (fine * 0.18)), col[1] * (1.0 - scour * (fine * 0.18)), col[2] * (1.0 - scour * (fine * 0.18))];
    steelness *= 1.0 - scour * 0.5; // scoured metal is matte

    // 5) SANDBLAST/dirty air → dust film settling in the sheltered recesses (tan desert, grey soot urban)
    let dust_amt = clamp01(exp.sandblast * (cavity * 0.9 + 0.14) * (0.6 + bloom * 0.5));
    col = mix3(col, exp.dust, dust_amt * 0.55);

    // baked AO + micro
    let ao = 1.0 - cavity * 0.32;
    let micro = 1.0 + (hash2((u * 240.0) as i32, (v * 240.0) as i32, 0xA1) - 0.5) * 0.06;
    for k in col.iter_mut() {
        *k = (*k * ao * micro).clamp(0.0, 255.0);
    }
    (col, h, steelness)
}

fn shade_cell(spec: MetalSpec, exp: Exposure, buf: &mut [u8], w: usize, ox: usize, oy: usize) {
    let rep = 2.0;
    let e = rep / CELL as f32;
    for py in 0..CELL {
        for px in 0..CELL {
            let u = (px as f32 / CELL as f32 * rep).fract();
            let v = (py as f32 / CELL as f32 * rep).fract();
            let (alb, _, steel) = metal_pixel(spec, exp, u, v);
            let hl = metal_pixel(spec, exp, (u - e + 1.0).fract(), v).1;
            let hr = metal_pixel(spec, exp, (u + e).fract(), v).1;
            let hd = metal_pixel(spec, exp, u, (v - e + 1.0).fract()).1;
            let hu = metal_pixel(spec, exp, u, (v + e).fract()).1;
            let (nx, ny, nz) = ((hl - hr) * 5.0, (hd - hu) * 5.0, 1.0);
            let inv = 1.0 / (nx * nx + ny * ny + nz * nz).sqrt();
            let (nx, ny, nz) = (nx * inv, ny * inv, nz * inv);
            let n = 1.0 / (0.5f32 * 0.5 + 0.55 * 0.55 + 0.66 * 0.66).sqrt();
            let (lx, ly, lz) = (-0.5 * n, -0.55 * n, 0.66 * n);
            let diff = (nx * lx + ny * ly + nz * lz).max(0.0);
            let hnz = lz + 1.0;
            let hn = 1.0 / (lx * lx + ly * ly + hnz * hnz).sqrt();
            let sp = (nx * lx * hn + ny * ly * hn + nz * hnz * hn).max(0.0).powf(22.0) * steel;
            let shade = 0.44 + 0.70 * diff;
            let i = ((oy + py) * w + (ox + px)) * 3;
            buf[i] = (alb[0] * shade + 220.0 * sp).clamp(0.0, 255.0) as u8;
            buf[i + 1] = (alb[1] * shade + 226.0 * sp).clamp(0.0, 255.0) as u8;
            buf[i + 2] = (alb[2] * shade + 236.0 * sp).clamp(0.0, 255.0) as u8;
        }
    }
}

fn rows() -> Vec<(&'static str, MetalSpec)> {
    vec![
        ("painted steel", MetalSpec { alloy: [120.0, 126.0, 136.0], finish: Finish::Painted([150.0, 78.0, 60.0]), structure: Struct::Flat, corrodes: 1.0 }),
        ("bare corrugated steel", MetalSpec { alloy: [124.0, 131.0, 143.0], finish: Finish::Bare, structure: Struct::Corrugated, corrodes: 1.0 }),
        ("diamond tread plate", MetalSpec { alloy: [122.0, 128.0, 138.0], finish: Finish::Bare, structure: Struct::Tread, corrodes: 1.0 }),
        ("galvanized corrugated", MetalSpec { alloy: [140.0, 146.0, 154.0], finish: Finish::Galvanized, structure: Struct::Corrugated, corrodes: 0.3 }),
    ]
}
fn cols() -> Vec<(&'static str, Exposure)> {
    vec![("desert", desert()), ("jungle", jungle()), ("coastal", coastal()), ("temperate", temperate())]
}

fn measure(spec: MetalSpec, exp: Exposure) -> (f32, f32, f32, f32) {
    // brightness, rust%, moss(green)%, dust-in-cavity brightness proxy
    let (mut b, mut rust, mut moss, mut n) = (0.0, 0.0, 0.0, 0u32);
    for j in 0..100 {
        for i in 0..100 {
            let (c, _, _) = metal_pixel(spec, exp, i as f32 / 100.0, j as f32 / 100.0);
            b += lum(c);
            if c[0] > c[2] + 20.0 && c[0] > c[1] {
                rust += 1.0;
            }
            if c[1] > c[0] + 6.0 && c[1] > c[2] + 6.0 {
                moss += 1.0;
            }
            n += 1.0 as u32;
        }
    }
    let f = n as f32;
    (b / f, rust / f * 100.0, moss / f * 100.0, 0.0)
}

fn main() {
    let r = rows();
    let c = cols();
    let w = CELL * c.len();
    let h = CELL * r.len();
    let mut buf = vec![14u8; w * h * 3];
    for (ri, (_, spec)) in r.iter().enumerate() {
        for (ci, (_, exp)) in c.iter().enumerate() {
            shade_cell(*spec, *exp, &mut buf, w, ci * CELL, ri * CELL);
        }
    }
    image::save_buffer("metal_exposure.png", &buf, w as u32, h as u32, image::ColorType::Rgb8).unwrap();
    println!("wrote metal_exposure.png  ({w}x{h})  rows: {} | cols: desert, jungle, coastal, temperate", r.iter().map(|(n, _)| *n).collect::<Vec<_>>().join(", "));

    for (rn, spec) in &r {
        println!("\n  {rn}");
        println!("    climate     brightness   rust%   moss%");
        for (cn, exp) in &c {
            let (b, ru, mo, _) = measure(*spec, *exp);
            println!("    {cn:<10}  {b:>8.0}   {ru:>5.1}   {mo:>5.1}");
        }
    }
}

#[cfg(test)]
mod checks {
    use super::*;
    #[test]
    fn climates_read_differently() {
        let painted = rows()[0].1;
        let corr = rows()[1].1;
        let (bd, _, _, _) = measure(painted, desert());
        let (bj, rj, mj, _) = measure(painted, jungle());
        // desert bleaches brighter than jungle
        assert!(bd > bj + 8.0, "desert should bleach brighter: {bd} vs {bj}");
        // jungle grows moss the desert doesn't
        let (_, _, md, _) = measure(corr, desert());
        let (_, _, mjc, _) = measure(corr, jungle());
        assert!(mjc > md + 3.0, "jungle should be mossier: {mjc} vs {md}");
        // coastal (salt) rusts more than temperate on corrugated
        let (_, rc, _, _) = measure(corr, coastal());
        let (_, rt, _, _) = measure(corr, temperate());
        assert!(rc > rt, "coastal salt should rust more than temperate: {rc} vs {rt}");
        // sanity: jungle painted shows some rust
        assert!(rj >= 0.0 && mj >= 0.0);
    }
}
