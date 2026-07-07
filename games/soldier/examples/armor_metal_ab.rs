//! armor_metal_ab — before/after for the armor plate material. Left: the current `RustPlate` (single-scalar
//! brightness on a pre-rusted brown base). Middle: a mask-driven steel plate where every weathering layer is
//! placed by a physical cause. Right: the same new albedo lit by its own baked height field, to show the
//! structure is real depth, not painted-on shading. Run: cargo run --example armor_metal_ab
use early_fps::mesh::{material_pixel, Material};

const PANEL: usize = 420; // px per panel
const REPEAT: f32 = 3.0; // tile the material 3x3 so periodicity/seams read
const GAP: usize = 16;

// ── tiny self-contained value noise (keep the prototype independent of engine internals) ────────────────
fn hash2(x: i32, y: i32, seed: u32) -> f32 {
    let mut h = (x as u32).wrapping_mul(0x27d4_eb2d) ^ (y as u32).wrapping_mul(0x1656_67b1) ^ seed.wrapping_mul(0x9e37_79b1);
    h ^= h >> 15;
    h = h.wrapping_mul(0x85eb_ca6b);
    h ^= h >> 13;
    (h & 0x00ff_ffff) as f32 / 0x00ff_ffff as f32
}
fn vnoise(x: f32, y: f32, seed: u32) -> f32 {
    let (xi, yi) = (x.floor() as i32, y.floor() as i32);
    let (xf, yf) = (x - xi as f32, y - yi as f32);
    let (u, v) = (xf * xf * (3.0 - 2.0 * xf), yf * yf * (3.0 - 2.0 * yf));
    let a = hash2(xi, yi, seed);
    let b = hash2(xi + 1, yi, seed);
    let c = hash2(xi, yi + 1, seed);
    let d = hash2(xi + 1, yi + 1, seed);
    let ab = a + (b - a) * u;
    let cd = c + (d - c) * u;
    ab + (cd - ab) * v
}
fn fbm(x: f32, y: f32, oct: i32, seed: u32) -> f32 {
    let (mut f, mut amp, mut fr, mut norm) = (0.0, 0.5, 1.0, 0.0);
    for _ in 0..oct {
        f += amp * vnoise(x * fr, y * fr, seed);
        norm += amp;
        amp *= 0.5;
        fr *= 2.0;
    }
    f / norm
}
fn clamp01(x: f32) -> f32 {
    x.clamp(0.0, 1.0)
}
fn lerp3(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t]
}
fn smoothband(d: f32, lo: f32, hi: f32) -> f32 {
    // 1 inside [lo,hi], soft shoulders
    let a = clamp01((d - (lo - 0.02)) / 0.02);
    let b = 1.0 - clamp01((d - hi) / 0.02);
    (a * b).clamp(0.0, 1.0)
}

// ── the mask-driven armor plate ─────────────────────────────────────────────────────────────────────────
// Returns (albedo, height, steelness) so the lit panel can reconstruct a normal and add a glint on bare steel.
fn armor(u: f32, v: f32) -> ([f32; 3], f32, f32, f32, f32, f32) {
    // palette: a cool STEEL substrate (not pre-rusted), worn bright steel, and two rust tones + grime.
    let steel = [120.0, 127.0, 139.0];
    let steel_worn = [178.0, 184.0, 193.0];
    let rust = [128.0, 72.0, 42.0];
    let rust_dark = [86.0, 47.0, 30.0];
    let grime = [38.0, 36.0, 40.0];

    // ── STRUCTURE: one riveted plate per tile; the border is the recessed seam shared with neighbours ──
    let d_edge = u.min(1.0 - u).min(v).min(1.0 - v); // distance to plate border
    let seam_w = 0.05;
    let seam = clamp01(1.0 - d_edge / seam_w); // 1 at the seam centre → 0 at the lip

    // rivets inset from each corner
    let rivets = [(0.14, 0.14), (0.86, 0.14), (0.14, 0.86), (0.86, 0.86)];
    let mut rd = 1.0f32;
    for (rx, ry) in rivets {
        rd = rd.min(((u - rx).powi(2) + (v - ry).powi(2)).sqrt());
    }
    let rivet_head = clamp01(1.0 - rd / 0.045); // domed head
    let rivet_ring = smoothband(rd, 0.045, 0.062); // recessed ring around it

    // ── HEIGHT FIELD (drives cavity mask + the lit panel's normal) ──
    let plate_crown = (0.5 - (u - 0.5).abs()).min(0.5 - (v - 0.5).abs()) * 0.10;
    let height = plate_crown - seam * 0.9 - rivet_ring * 0.5 + rivet_head * 0.7;

    // ── MASKS (the reasoning surface) ──
    let cavity = clamp01(seam * 0.9 + rivet_ring * 0.6); // recesses collect grime + rust + shadow
    // edge wear: the raised lip just inside each seam, and rivet crowns — where hands/gear rub bare metal
    let lip = smoothband(d_edge, seam_w, seam_w + 0.06);
    let edge_wear = clamp01(lip * 0.8 + rivet_head * 0.7) * (0.6 + 0.4 * fbm(u * 9.0, v * 9.0, 3, 5));
    let down = v; // gravity: water/rust runs downward

    // ── RUST, placed by cause: it seeds at fasteners and seams (water sits there) and streaks DOWN ──
    // vertical streak field: stretched noise so drips are tall, sampled with a downward-available term
    let streak = fbm(u * 10.0, v * 2.2, 4, 11);
    let seed_src = clamp01(seam * 0.8 + rivet_ring * 0.9 + rivet_head * 0.3); // where water lingers
    let bloom = fbm(u * 6.0, v * 6.0, 4, 3); // irregular rust edge
    let mut rust_amt = clamp01((seed_src * 1.1 + down * 0.45 * streak + (streak - 0.5) * 0.6) * bloom - 0.18) * 1.6;
    rust_amt = clamp01(rust_amt) * (1.0 - edge_wear * 0.85); // freshly-worn edges shed rust

    // ── COMPOSE ALBEDO ──
    // per-plate faint tone + fine brushed-metal anisotropy (long axis lines)
    let plate_tone = (hash2((u).floor() as i32, (v).floor() as i32, 0x5A17) - 0.5) * 0.05;
    let brushed = (fbm(u * 3.0, v * 60.0, 2, 7) - 0.5) * 0.06;
    let mut col = [
        steel[0] * (1.0 + plate_tone + brushed),
        steel[1] * (1.0 + plate_tone + brushed),
        steel[2] * (1.0 + plate_tone + brushed),
    ];
    col = lerp3(col, steel_worn, edge_wear); // worn edges reveal bright bare steel
    col = lerp3(col, lerp3(rust, rust_dark, cavity), rust_amt); // rust (darker where it pools in cavities)
    col = lerp3(col, grime, cavity * 0.45); // grime in the deep recesses
    // baked AO from the cavity + fine micro speckle
    let ao = 1.0 - cavity * 0.34;
    let micro = 1.0 + (hash2((u * 220.0) as i32, (v * 220.0) as i32, 0xA17E) - 0.5) * 0.06;
    for c in col.iter_mut() {
        *c *= ao * micro;
    }
    let steelness = (1.0 - rust_amt) * (1.0 - cavity * 0.7); // bare steel glints; rust/recess is matte
    (col, height, steelness, rust_amt, edge_wear, cavity)
}

// Measure rule-satisfaction: for named regions, report the average of each causal mask. This is the
// anti-Goodhart check — we assert the rust sits where rust should sit, not that "variance is high".
fn report_rules() {
    let regions: [(&str, fn(f32, f32) -> bool); 5] = [
        ("clean upper face", |u, v| v < 0.4 && (0.2..0.8).contains(&u) && u.min(1.0 - u).min(v).min(1.0 - v) > 0.14),
        ("lower face (below fasteners)", |u, v| v > 0.6 && (0.25..0.75).contains(&u) && u.min(1.0 - u).min(v).min(1.0 - v) > 0.14),
        ("rivet neighbourhood", |u, v| [(0.14, 0.14), (0.86, 0.14), (0.14, 0.86), (0.86, 0.86)].iter().any(|(x, y): &(f32, f32)| ((u - x).powi(2) + (v - y).powi(2)).sqrt() < 0.09)),
        ("recessed seam", |u, v| u.min(1.0 - u).min(v).min(1.0 - v) < 0.045),
        ("worn edge lip", |u, v| {
            let d = u.min(1.0 - u).min(v).min(1.0 - v);
            (0.055..0.10).contains(&d)
        }),
    ];
    let n = 300;
    println!("\n  region                          rust   wear   cavity   brightness");
    println!("  ------------------------------- ------ ------ -------- ----------");
    for (name, pred) in regions {
        let (mut rust, mut wear, mut cav, mut bright, mut cnt) = (0.0f32, 0.0f32, 0.0f32, 0.0f32, 0u32);
        for yi in 0..n {
            for xi in 0..n {
                let (u, v) = (xi as f32 / n as f32, yi as f32 / n as f32);
                if pred(u, v) {
                    let (c, _, _, r, w, cv) = armor(u, v);
                    rust += r;
                    wear += w;
                    cav += cv;
                    bright += (c[0] + c[1] + c[2]) / 3.0;
                    cnt += 1;
                }
            }
        }
        let f = cnt.max(1) as f32;
        println!("  {name:<31} {:.3}  {:.3}  {:.3}    {:.0}", rust / f, wear / f, cav / f, bright / f);
    }
}

fn main() {
    let w = PANEL * 3 + GAP * 2;
    let h = PANEL;
    let mut buf = vec![24u8; w * h * 3];
    let put = |buf: &mut [u8], x: usize, y: usize, c: [f32; 3]| {
        let i = (y * w + x) * 3;
        buf[i] = c[0].clamp(0.0, 255.0) as u8;
        buf[i + 1] = c[1].clamp(0.0, 255.0) as u8;
        buf[i + 2] = c[2].clamp(0.0, 255.0) as u8;
    };

    for py in 0..PANEL {
        for px in 0..PANEL {
            let u = (px as f32 / PANEL as f32 * REPEAT).fract();
            let v = (py as f32 / PANEL as f32 * REPEAT).fract();

            // Panel 1 — current RustPlate (engine's own generator)
            let c0 = material_pixel(Material::RustPlate, u, v);
            put(&mut buf, px, py, [c0.r() as f32, c0.g() as f32, c0.b() as f32]);

            // Panel 2 — new mask-driven albedo
            let (alb, _h, _s, _r, _w, _c) = armor(u, v);
            put(&mut buf, PANEL + GAP + px, py, alb);

            // Panel 3 — new albedo lit by its baked height field (reconstructed normal + steel glint)
            let e = 1.0 / PANEL as f32 * REPEAT;
            let (_, hl, ..) = armor((u - e + 1.0).fract(), v);
            let (_, hr, ..) = armor((u + e).fract(), v);
            let (_, hd, ..) = armor(u, (v - e + 1.0).fract());
            let (_, hu, ..) = armor(u, (v + e).fract());
            let nx = (hl - hr) * 6.0;
            let ny = (hd - hu) * 6.0;
            let nz = 1.0;
            let inv = 1.0 / (nx * nx + ny * ny + nz * nz).sqrt();
            let (nx, ny, nz) = (nx * inv, ny * inv, nz * inv);
            let l = {
                let (lx, ly, lz) = (-0.5f32, -0.55, 0.66);
                let n = 1.0 / (lx * lx + ly * ly + lz * lz).sqrt();
                (lx * n, ly * n, lz * n)
            };
            let diff = (nx * l.0 + ny * l.1 + nz * l.2).max(0.0);
            let spec = {
                let hvec = (l.0, l.1, l.2 + 1.0);
                let hn = 1.0 / (hvec.0 * hvec.0 + hvec.1 * hvec.1 + hvec.2 * hvec.2).sqrt();
                let ndh = (nx * hvec.0 * hn + ny * hvec.1 * hn + nz * hvec.2 * hn).max(0.0);
                ndh.powf(24.0) * _s
            };
            let shade = 0.45 + 0.75 * diff;
            let lit = [alb[0] * shade + 220.0 * spec, alb[1] * shade + 225.0 * spec, alb[2] * shade + 235.0 * spec];
            put(&mut buf, (PANEL + GAP) * 2 + px, py, lit);
        }
    }

    image::save_buffer("armor_metal_ab.png", &buf, w as u32, h as u32, image::ColorType::Rgb8).unwrap();
    println!("wrote armor_metal_ab.png  ({w}x{h})  [ current | new albedo | new lit ]");
    report_rules();
}
