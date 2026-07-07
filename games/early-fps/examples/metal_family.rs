//! metal_family — turns the one-off armor plate into a parametric METAL SET. A `MetalSpec` crosses an alloy
//! tone with a finish (bare / painted+chipping / galvanized / anodized) and a structure (flat / corrugated /
//! diamond-tread / brushed), and a `condition` scalar weathers it — all on the same mask-driven surface +
//! baked-height approach as before (flat aging in the texture; the structure's relief baked into a height
//! field that lights it). This is the "handful of metal setups" before any map wiring. The per-location
//! `condition` is exactly what the seam_condition damage field would drive once these are wired.
//! Run: cargo run --example metal_family
const CELL: usize = 300;
const COLS: usize = 3;
const ROWS: usize = 2;
const REPEAT: f32 = 2.0;

// ── noise ───────────────────────────────────────────────────────────────────────────────────────────────
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
    let a = hash2(xi, yi, s);
    let b = hash2(xi + 1, yi, s);
    let c = hash2(xi, yi + 1, s);
    let d = hash2(xi + 1, yi + 1, s);
    lerp(lerp(a, b, u), lerp(c, d, u), v)
}
fn fbm(x: f32, y: f32, oct: i32, s: u32) -> f32 {
    let (mut f, mut amp, mut fr, mut n) = (0.0, 0.5, 1.0, 0.0);
    for _ in 0..oct {
        f += amp * vnoise(x * fr, y * fr, s);
        n += amp;
        amp *= 0.5;
        fr *= 2.0;
    }
    f / n
}
/// Nearest-feature Voronoi → (cell brightness id, distance to nearest). For galvanized spangle grains.
fn voronoi(x: f32, y: f32, s: u32) -> (f32, f32) {
    let (xi, yi) = (x.floor() as i32, y.floor() as i32);
    let (mut best, mut bid, mut second) = (9.0f32, 0.0f32, 9.0f32);
    for oy in -1..=1 {
        for ox in -1..=1 {
            let (cx, cy) = (xi + ox, yi + oy);
            let px = cx as f32 + hash2(cx, cy, s);
            let py = cy as f32 + hash2(cx, cy, s ^ 0x9e37);
            let d = ((px - x).powi(2) + (py - y).powi(2)).sqrt();
            if d < best {
                second = best;
                best = d;
                bid = hash2(cx, cy, s ^ 0xB5);
            } else if d < second {
                second = d;
            }
        }
    }
    (bid, second - best)
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

// ── the family ──────────────────────────────────────────────────────────────────────────────────────────
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
    Brushed,
}
#[derive(Clone, Copy)]
struct MetalSpec {
    alloy: [f32; 3],
    finish: Finish,
    structure: Struct,
    condition: f32, // 0 pristine .. 1 wrecked
}

const RUST: [f32; 3] = [120.0, 66.0, 42.0];
const RUST_DK: [f32; 3] = [86.0, 47.0, 30.0];
const GRIME: [f32; 3] = [40.0, 38.0, 40.0];

/// structure → (height, cavity mask, edge mask). Height feeds the lit normal; cavity/edge place weathering.
fn structure_field(st: Struct, u: f32, v: f32) -> (f32, f32, f32) {
    match st {
        Struct::Flat => (0.0, 0.0, 0.0),
        Struct::Corrugated => {
            let ridges = 7.0;
            let h = (u * ridges * std::f32::consts::TAU).sin();
            let cavity = clamp01(-h); // troughs collect water/grime
            let edge = 0.0;
            (h * 0.5, cavity * 0.7, edge)
        }
        Struct::Tread => {
            let n = 5.0;
            let cu = (u * n).fract() - 0.5;
            let cv = ((v * n) + if ((u * n) as i32) % 2 == 0 { 0.0 } else { 0.5 }).fract() - 0.5;
            let dia = cu.abs() + cv.abs();
            let raised = smoothstep(0.42, 0.24, dia); // diamond pads stand proud
            (raised * 0.6, clamp01(1.0 - raised) * 0.5, smoothstep(0.24, 0.42, dia) * (1.0 - raised))
        }
        Struct::Brushed => {
            let h = (fbm(u * 2.0, v * 90.0, 4, 7) - 0.5) * 0.4; // fine directional grooves
            (h, clamp01(-h) * 0.3, 0.0)
        }
    }
}

/// The material read at (u,v): returns (albedo, height, steelness-for-spec).
fn metal_pixel(spec: MetalSpec, u: f32, v: f32) -> ([f32; 3], f32, f32) {
    let (h, cavity, edge) = structure_field(spec.structure, u, v);
    let c = spec.condition;

    // base by finish
    let (mut col, mut bare_steel) = match spec.finish {
        Finish::Bare => (spec.alloy, true),
        Finish::Painted(paint) => {
            // chips reveal alloy — at structure edges/high points and via a speckled chip mask, growing with wear
            let chip_noise = fbm(u * 14.0, v * 14.0, 4, 21);
            let chip = clamp01(smoothstep(0.62 - c * 0.35, 0.72 - c * 0.35, chip_noise) + edge * (0.4 + c));
            (mix3(paint, spec.alloy, chip), chip > 0.5)
        }
        Finish::Galvanized => {
            let (grain, spangle) = voronoi(u * 9.0, v * 9.0, 5);
            let sp = mix3([150.0, 158.0, 168.0], [188.0, 194.0, 202.0], grain) ; // crystalline spangle
            let edged = mix3(sp, [120.0, 128.0, 138.0], smoothstep(0.14, 0.0, spangle)); // grain boundaries
            (edged, false)
        }
    };

    // brushed anisotropy on bare metal
    if matches!(spec.finish, Finish::Bare) && spec.structure != Struct::Brushed {
        let b = (fbm(u * 3.0, v * 60.0, 4, 7) - 0.5) * 0.08;
        col = [col[0] * (1.0 + b), col[1] * (1.0 + b), col[2] * (1.0 + b)];
    }

    // ── weathering, placed by cause and scaled by condition ──
    let bloom = fbm(u * 6.0, v * 6.0, 4, 3);
    // galvanized weathers to dull white-grey rust, not orange; others to iron-oxide
    let (rust_c, rust_dk) = if matches!(spec.finish, Finish::Galvanized) {
        ([150.0, 150.0, 148.0], [110.0, 112.0, 112.0])
    } else {
        (RUST, RUST_DK)
    };
    let rust_amt = clamp01((cavity * 0.7 + edge * 0.6 + (bloom - 0.5) * 0.7 + c * 0.6 - 0.35) * (0.6 + c)) * smoothstep(0.05, 0.5, c);
    col = mix3(col, mix3(rust_c, rust_dk, cavity), rust_amt);
    if rust_amt > 0.2 {
        bare_steel = false;
    }
    col = mix3(col, GRIME, cavity * 0.4 * smoothstep(0.1, 0.9, c)); // grime in recesses when worn

    // baked AO from cavity + micro speckle
    let ao = 1.0 - cavity * 0.34;
    let micro = 1.0 + (hash2((u * 240.0) as i32, (v * 240.0) as i32, 0xA1) - 0.5) * 0.06;
    for k in col.iter_mut() {
        *k *= ao * micro;
    }
    let steelness = if bare_steel { (1.0 - rust_amt) * (1.0 - cavity * 0.6) } else { 0.15 };
    (col, h, steelness)
}

fn shade_cell(spec: MetalSpec, buf: &mut [u8], w: usize, ox: usize, oy: usize) {
    let e = REPEAT / CELL as f32;
    for py in 0..CELL {
        for px in 0..CELL {
            let u = (px as f32 / CELL as f32 * REPEAT).fract();
            let v = (py as f32 / CELL as f32 * REPEAT).fract();
            let (alb, _, steel) = metal_pixel(spec, u, v);
            // normal from height gradient
            let hl = metal_pixel(spec, (u - e + 1.0).fract(), v).1;
            let hr = metal_pixel(spec, (u + e).fract(), v).1;
            let hd = metal_pixel(spec, u, (v - e + 1.0).fract()).1;
            let hu = metal_pixel(spec, u, (v + e).fract()).1;
            let (nx, ny, nz) = ((hl - hr) * 5.0, (hd - hu) * 5.0, 1.0);
            let inv = 1.0 / (nx * nx + ny * ny + nz * nz).sqrt();
            let (nx, ny, nz) = (nx * inv, ny * inv, nz * inv);
            let (lx, ly, lz) = {
                let n = 1.0 / (0.5f32 * 0.5 + 0.55 * 0.55 + 0.66 * 0.66).sqrt();
                (-0.5 * n, -0.55 * n, 0.66 * n)
            };
            let diff = (nx * lx + ny * ly + nz * lz).max(0.0);
            let hnz = lz + 1.0;
            let hn = 1.0 / (lx * lx + ly * ly + hnz * hnz).sqrt();
            let spec_i = (nx * lx * hn + ny * ly * hn + nz * hnz * hn).max(0.0).powf(22.0) * steel;
            let shade = 0.42 + 0.72 * diff;
            let i = ((oy + py) * w + (ox + px)) * 3;
            buf[i] = (alb[0] * shade + 220.0 * spec_i).clamp(0.0, 255.0) as u8;
            buf[i + 1] = (alb[1] * shade + 226.0 * spec_i).clamp(0.0, 255.0) as u8;
            buf[i + 2] = (alb[2] * shade + 236.0 * spec_i).clamp(0.0, 255.0) as u8;
        }
    }
}

fn catalog() -> Vec<(&'static str, MetalSpec)> {
    vec![
        ("bare steel plate", MetalSpec { alloy: [124, 131, 143].map(|x| x as f32), finish: Finish::Bare, structure: Struct::Flat, condition: 0.12 }),
        ("painted steel (chipping)", MetalSpec { alloy: [120, 126, 136].map(|x| x as f32), finish: Finish::Painted([150.0, 78.0, 60.0]), structure: Struct::Flat, condition: 0.4 }),
        ("galvanized corrugated", MetalSpec { alloy: [150, 158, 168].map(|x| x as f32), finish: Finish::Galvanized, structure: Struct::Corrugated, condition: 0.22 }),
        ("diamond tread plate", MetalSpec { alloy: [122, 128, 138].map(|x| x as f32), finish: Finish::Bare, structure: Struct::Tread, condition: 0.16 }),
        ("rusted iron", MetalSpec { alloy: [116, 96, 84].map(|x| x as f32), finish: Finish::Bare, structure: Struct::Corrugated, condition: 0.82 }),
        ("brushed aluminium", MetalSpec { alloy: [176, 180, 186].map(|x| x as f32), finish: Finish::Bare, structure: Struct::Brushed, condition: 0.05 }),
    ]
}

fn main() {
    let w = CELL * COLS;
    let h = CELL * ROWS;
    let mut buf = vec![14u8; w * h * 3];
    let cat = catalog();
    for (i, (_name, spec)) in cat.iter().enumerate() {
        let (cx, cy) = (i % COLS, i / COLS);
        shade_cell(*spec, &mut buf, w, cx * CELL, cy * CELL);
    }
    image::save_buffer("metal_family.png", &buf, w as u32, h as u32, image::ColorType::Rgb8).unwrap();
    println!("wrote metal_family.png  ({w}x{h})  6 metals: {}", cat.iter().map(|(n, _)| *n).collect::<Vec<_>>().join(", "));

    // ── rule checks: measure each material against what it should do ──
    println!("\n  metal                       mean-brightness   rust-cover%   struct-relief");
    for (name, spec) in &cat {
        let (mut bsum, mut rust, mut n) = (0.0f32, 0.0f32, 0u32);
        let (mut hmin, mut hmax) = (f32::MAX, f32::MIN);
        for j in 0..120 {
            for i in 0..120 {
                let (u, v) = (i as f32 / 120.0, j as f32 / 120.0);
                let (c, hh, _) = metal_pixel(*spec, u, v);
                bsum += (c[0] + c[1] + c[2]) / 3.0;
                // "rusty" ≈ red channel meaningfully above blue (iron oxide) — a proxy for oxide coverage
                if c[0] > c[2] + 22.0 {
                    rust += 1.0;
                }
                hmin = hmin.min(hh);
                hmax = hmax.max(hh);
                n += 1;
            }
        }
        println!("    {:<26} {:>9.0}        {:>7.1}       {:.2}", name, bsum / n as f32, rust / n as f32 * 100.0, hmax - hmin);
    }
}

#[cfg(test)]
mod checks {
    use super::*;
    fn rust_cover(spec: MetalSpec) -> f32 {
        let (mut r, mut n) = (0.0, 0);
        for j in 0..100 {
            for i in 0..100 {
                let (c, _, _) = metal_pixel(spec, i as f32 / 100.0, j as f32 / 100.0);
                if c[0] > c[2] + 22.0 {
                    r += 1.0;
                }
                n += 1;
            }
        }
        r / n as f32
    }
    #[test]
    fn family_behaves() {
        let cat = catalog();
        // structured metals have real relief; flat ones don't
        for (name, spec) in &cat {
            let (mut lo, mut hi) = (f32::MAX, f32::MIN);
            for j in 0..80 {
                for i in 0..80 {
                    let hh = metal_pixel(*spec, i as f32 / 80.0, j as f32 / 80.0).1;
                    lo = lo.min(hh);
                    hi = hi.max(hh);
                }
            }
            let relief = hi - lo;
            if matches!(spec.structure, Struct::Corrugated | Struct::Tread) {
                assert!(relief > 0.3, "{name} should have relief, got {relief}");
            }
        }
        // condition drives oxide where water can sit: a heavily-worn corrugated sheet rusts far more in its
        // troughs than a pristine one. (Flat steel barely patch-rusts — correctly — so it's the wrong probe.)
        let pristine = MetalSpec { alloy: [124.0, 131.0, 143.0], finish: Finish::Bare, structure: Struct::Corrugated, condition: 0.05 };
        let wrecked = MetalSpec { condition: 0.85, ..pristine };
        assert!(rust_cover(wrecked) > rust_cover(pristine) + 0.1, "wear should add oxide in the troughs");
    }
}
