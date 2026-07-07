//! armor_plate — the metal system retargeted for ARMOR. Same masks, but an armor palette (gunmetal, blued
//! steel, painted enamel, brushed titanium, composite) and a SERVICE-WEAR model (edge scuffs to bare steel,
//! scratches, battle scorch) instead of the environmental rust/moss/dust that suits map surfaces. Rendered on
//! a curved, form-fitted plate so the highlight sweeps (that's half the metal read). One cell uses the map
//! palette to show why environment metal ≠ armor. Run: cargo run --example armor_plate
use early_fps::math::Vec3;

const CELL: usize = 300;
const COLS: usize = 4;
const ROWS: usize = 2;

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

// ── armor material ──────────────────────────────────────────────────────────────────────────────────────
#[derive(Clone, Copy)]
enum ArmorFinish {
    Gunmetal,
    Blued,
    Enamel([f32; 3]),
    Brushed,
    Composite,
    MapMetal, // the environment RustPlate palette, for contrast
}
#[derive(Clone, Copy)]
struct ArmorMat {
    finish: ArmorFinish,
    wear: f32,   // service wear: edge scuffing to bare steel
    battle: f32, // combat scarring: scorch + gouges
}

const POLISH: [f32; 3] = [182.0, 188.0, 198.0]; // bright bare steel revealed by wear

/// (albedo, gloss) for the plate at uv. Masks (edge, rib) are derived from uv and match the plate geometry.
fn armor_pixel(m: ArmorMat, u: f32, v: f32) -> ([f32; 3], f32) {
    let edge = 1.0 - smoothstep(0.0, 0.10, u.min(1.0 - u).min(v).min(1.0 - v)); // 1 at plate rim
    let rib = smoothstep(0.11, 0.0, (u - 0.5).abs()); // central keel
    let fine = fbm(u * 22.0, v * 22.0, 9);

    let (mut col, mut gloss, reveals_steel) = match m.finish {
        ArmorFinish::Gunmetal => ([96.0, 100.0, 110.0], 0.72, true),
        ArmorFinish::Blued => ([54.0, 58.0, 78.0], 0.86, true),
        ArmorFinish::Enamel(c) => (c, 0.36, false),
        ArmorFinish::Brushed => {
            let b = (fbm(u * 3.0, v * 80.0, 7) - 0.5) * 0.16; // strong vertical brush
            ([lerp(150.0, 158.0, b + 0.5), lerp(150.0, 158.0, b + 0.5), lerp(154.0, 150.0, b + 0.5)], 0.6, true)
        }
        ArmorFinish::Composite => {
            let weave = (((u * 34.0).sin() * (v * 34.0).sin()) * 0.5 + 0.5) * 0.10;
            ([42.0 + weave * 40.0, 44.0 + weave * 40.0, 52.0 + weave * 44.0], 0.30, false)
        }
        ArmorFinish::MapMetal => {
            // environment steel: bluish base + ambient rust + grime, matte — the map look, on armor
            let rust = clamp01(fbm(u * 5.0, v * 5.0, 4) * 0.9 + 0.15) * 0.7;
            (mix3([132.0, 120.0, 108.0], [120.0, 66.0, 42.0], rust), 0.28, false)
        }
    };

    // brushed/composite already carry their own micro; add a faint one to the rest
    if !matches!(m.finish, ArmorFinish::Brushed | ArmorFinish::Composite) {
        let mic = 1.0 + (fine - 0.5) * 0.05;
        col = [col[0] * mic, col[1] * mic, col[2] * mic];
    }

    // ── SERVICE WEAR: edges and the rib crest scuff down to bright bare steel (paint chips there too) ──
    let reveal = if reveals_steel { POLISH } else { mix3([120.0, 124.0, 134.0], POLISH, 0.6) };
    let scuff = clamp01((edge * 1.15 + rib * 0.35) * m.wear * (0.7 + fine * 0.6));
    col = mix3(col, reveal, scuff);
    gloss = lerp(gloss, 0.9, scuff);

    // fine scratches: sparse bright hairlines, mostly along the plate
    let scr_field = fbm(u * 60.0, v * 8.0, 31);
    let scratch = smoothstep(0.82, 0.9, scr_field) * m.wear;
    col = mix3(col, POLISH, scratch * 0.5);

    // ── BATTLE: a couple of scorch/gouge marks (bare metal core, sooted halo) ──
    if m.battle > 0.0 {
        for (cx, cy) in [(0.62, 0.4), (0.35, 0.66)] {
            let d = ((u - cx).powi(2) + (v - cy).powi(2)).sqrt();
            let gouge = smoothstep(0.09, 0.0, d) * m.battle; // bare torn metal
            let soot = smoothstep(0.17, 0.06, d) * m.battle; // scorch halo
            col = mix3(col, [26.0, 24.0, 26.0], soot * 0.6);
            col = mix3(col, POLISH, gouge * 0.8);
            if gouge > 0.1 {
                gloss = lerp(gloss, 0.85, gouge);
            }
        }
    }

    // MapMetal grime in the lower recess to underline the "left outside" read
    if matches!(m.finish, ArmorFinish::MapMetal) {
        col = mix3(col, [40.0, 38.0, 40.0], (1.0 - v) * 0.25);
    }
    (col, gloss)
}

// ── curved plate mesh ───────────────────────────────────────────────────────────────────────────────────
struct Surf {
    pos: Vec3,
    nrm: Vec3,
    uv: (f32, f32),
}
fn plate() -> (Vec<Surf>, Vec<[u32; 3]>) {
    let (nx, ny) = (60usize, 64usize);
    let rr = 1.25;
    let arc = 0.62;
    let disp = |u: f32, v: f32| {
        let rib = smoothstep(0.11, 0.0, (u - 0.5).abs()) * 0.05;
        let bevel = (smoothstep(0.05, 0.0, u.min(1.0 - u)) + smoothstep(0.05, 0.0, v.min(1.0 - v))) * 0.045;
        rib - bevel
    };
    let posf = |u: f32, v: f32| {
        let th = lerp(-arc, arc, u);
        let r = rr + disp(u, v);
        Vec3::new(r * th.sin(), lerp(-0.74, 0.74, v), r * th.cos() - rr * 0.55)
    };
    let nrmf = |u: f32, v: f32| {
        let e = 0.004;
        let du = posf((u + e).min(1.0), v) - posf((u - e).max(0.0), v);
        let dv = posf(u, (v + e).min(1.0)) - posf(u, (v - e).max(0.0));
        dv.cross(du).normalized()
    };
    let mut verts = vec![];
    for j in 0..=ny {
        for i in 0..=nx {
            let (u, v) = (i as f32 / nx as f32, j as f32 / ny as f32);
            verts.push(Surf { pos: posf(u, v), nrm: nrmf(u, v), uv: (u, v) });
        }
    }
    let mut tris = vec![];
    let w = nx + 1;
    for j in 0..ny {
        for i in 0..nx {
            let a = (j * w + i) as u32;
            let b = a + 1;
            let c = ((j + 1) * w + i) as u32 + 1;
            let d = ((j + 1) * w + i) as u32;
            tris.push([a, b, c]);
            tris.push([a, c, d]);
        }
    }
    (verts, tris)
}

fn shade_cell(m: ArmorMat, verts: &[Surf], tris: &[[u32; 3]], buf: &mut [u8], w: usize, ox: usize, oy: usize) {
    let cam = Vec3::new(0.0, 0.0, 2.5);
    let focal = CELL as f32 * 0.5 / (34f32.to_radians() * 0.5).tan();
    let light = Vec3::new(-0.55, 0.5, 0.66).normalized();
    let mut zb = vec![f32::INFINITY; CELL * CELL];
    let proj = |p: Vec3| {
        let z = cam.z - p.z;
        (CELL as f32 * 0.5 + p.x / z * focal, CELL as f32 * 0.5 - p.y / z * focal, z)
    };
    for tri in tris {
        let v = [&verts[tri[0] as usize], &verts[tri[1] as usize], &verts[tri[2] as usize]];
        let s = [proj(v[0].pos), proj(v[1].pos), proj(v[2].pos)];
        let area = (s[1].0 - s[0].0) * (s[2].1 - s[0].1) - (s[1].1 - s[0].1) * (s[2].0 - s[0].0);
        if area.abs() < 1e-6 {
            continue;
        }
        let minx = s.iter().map(|p| p.0).fold(f32::MAX, f32::min).floor().max(0.0) as usize;
        let maxx = s.iter().map(|p| p.0).fold(f32::MIN, f32::max).ceil().min(CELL as f32 - 1.0) as usize;
        let miny = s.iter().map(|p| p.1).fold(f32::MAX, f32::min).floor().max(0.0) as usize;
        let maxy = s.iter().map(|p| p.1).fold(f32::MIN, f32::max).ceil().min(CELL as f32 - 1.0) as usize;
        for py in miny..=maxy {
            for px in minx..=maxx {
                let (fx, fy) = (px as f32 + 0.5, py as f32 + 0.5);
                let w0 = ((s[1].0 - fx) * (s[2].1 - fy) - (s[1].1 - fy) * (s[2].0 - fx)) / area;
                let w1 = ((s[2].0 - fx) * (s[0].1 - fy) - (s[2].1 - fy) * (s[0].0 - fx)) / area;
                let w2 = 1.0 - w0 - w1;
                if w0 < 0.0 || w1 < 0.0 || w2 < 0.0 {
                    continue;
                }
                let z = w0 * s[0].2 + w1 * s[1].2 + w2 * s[2].2;
                let zi = py * CELL + px;
                if z >= zb[zi] {
                    continue;
                }
                zb[zi] = z;
                let mut n = (v[0].nrm * w0 + v[1].nrm * w1 + v[2].nrm * w2).normalized();
                if n.z < 0.0 {
                    n = n * -1.0;
                }
                let uu = w0 * v[0].uv.0 + w1 * v[1].uv.0 + w2 * v[2].uv.0;
                let vv = w0 * v[0].uv.1 + w1 * v[1].uv.1 + w2 * v[2].uv.1;
                let (alb, gloss) = armor_pixel(m, uu, vv);
                let diff = n.dot(light).max(0.0);
                let h = (light + Vec3::new(0.0, 0.0, 1.0)).normalized();
                let spec = n.dot(h).max(0.0).powf(6.0 + gloss * 90.0) * gloss;
                let shade = 0.34 + 0.72 * diff;
                let i = ((oy + py) * w + (ox + px)) * 3;
                buf[i] = (alb[0] * shade + 235.0 * spec).clamp(0.0, 255.0) as u8;
                buf[i + 1] = (alb[1] * shade + 238.0 * spec).clamp(0.0, 255.0) as u8;
                buf[i + 2] = (alb[2] * shade + 245.0 * spec).clamp(0.0, 255.0) as u8;
            }
        }
    }
}

fn catalog() -> Vec<(&'static str, ArmorMat)> {
    vec![
        ("gunmetal", ArmorMat { finish: ArmorFinish::Gunmetal, wear: 0.4, battle: 0.0 }),
        ("blued steel", ArmorMat { finish: ArmorFinish::Blued, wear: 0.28, battle: 0.0 }),
        ("olive enamel", ArmorMat { finish: ArmorFinish::Enamel([80.0, 86.0, 54.0]), wear: 0.5, battle: 0.0 }),
        ("brushed titanium", ArmorMat { finish: ArmorFinish::Brushed, wear: 0.3, battle: 0.0 }),
        ("dark composite", ArmorMat { finish: ArmorFinish::Composite, wear: 0.22, battle: 0.0 }),
        ("crimson enamel", ArmorMat { finish: ArmorFinish::Enamel([132.0, 48.0, 44.0]), wear: 0.45, battle: 0.0 }),
        ("gunmetal, battle-worn", ArmorMat { finish: ArmorFinish::Gunmetal, wear: 0.7, battle: 0.7 }),
        ("map metal (for contrast)", ArmorMat { finish: ArmorFinish::MapMetal, wear: 0.2, battle: 0.0 }),
    ]
}

fn main() {
    let (verts, tris) = plate();
    let cat = catalog();
    let w = CELL * COLS;
    let h = CELL * ROWS;
    let mut buf = vec![0u8; w * h * 3];
    for py in 0..h {
        let t = py as f32 / h as f32;
        for px in 0..w {
            let i = (py * w + px) * 3;
            buf[i] = (18.0 + t * 10.0) as u8;
            buf[i + 1] = (20.0 + t * 11.0) as u8;
            buf[i + 2] = (26.0 + t * 14.0) as u8;
        }
    }
    for (k, (_n, m)) in cat.iter().enumerate() {
        shade_cell(*m, &verts, &tris, &mut buf, w, (k % COLS) * CELL, (k / COLS) * CELL);
    }
    image::save_buffer("armor_plate.png", &buf, w as u32, h as u32, image::ColorType::Rgb8).unwrap();
    println!("wrote armor_plate.png  ({w}x{h})  {}", cat.iter().map(|(n, _)| *n).collect::<Vec<_>>().join(", "));

    // rule checks: edge wear brighter than face; armor clean vs map metal rusty
    fn stat(m: ArmorMat) -> (f32, f32, f32) {
        let (mut edge_b, mut face_b, mut rust, mut en, mut fn_, mut rn) = (0.0, 0.0, 0.0, 0u32, 0u32, 0u32);
        for j in 0..100 {
            for i in 0..100 {
                let (u, v) = (i as f32 / 100.0, j as f32 / 100.0);
                let (c, _) = armor_pixel(m, u, v);
                let b = (c[0] + c[1] + c[2]) / 3.0;
                let d = u.min(1.0 - u).min(v).min(1.0 - v);
                if d < 0.06 {
                    edge_b += b;
                    en += 1;
                } else if d > 0.25 {
                    face_b += b;
                    fn_ += 1;
                }
                if c[0] > c[2] + 18.0 {
                    rust += 1.0;
                }
                rn += 1;
            }
        }
        (edge_b / en.max(1) as f32, face_b / fn_.max(1) as f32, rust / rn.max(1) as f32 * 100.0)
    }
    println!("\n  material                    edge-bright  face-bright  rust%");
    for (n, m) in &cat {
        let (e, f, r) = stat(*m);
        println!("    {n:<26} {e:>8.0}   {f:>8.0}   {r:>5.1}");
    }
}

#[cfg(test)]
mod checks {
    use super::*;
    #[test]
    fn armor_reads_as_armor() {
        // service wear: plate edges scuff brighter than the face
        let gm = ArmorMat { finish: ArmorFinish::Gunmetal, wear: 0.5, battle: 0.0 };
        let (mut eb, mut en, mut fb, mut fnn) = (0.0, 0u32, 0.0, 0u32);
        for j in 0..80 {
            for i in 0..80 {
                let (u, v) = (i as f32 / 80.0, j as f32 / 80.0);
                let (c, _) = armor_pixel(gm, u, v);
                let b = (c[0] + c[1] + c[2]) / 3.0;
                let d = u.min(1.0 - u).min(v).min(1.0 - v);
                if d < 0.05 {
                    eb += b;
                    en += 1;
                } else if d > 0.3 {
                    fb += b;
                    fnn += 1;
                }
            }
        }
        assert!(eb / en as f32 > fb / fnn as f32 + 15.0, "armor edges should scuff bright");
        // armor gunmetal is not rusty; map metal is
        let rusty = |m: ArmorMat| {
            let mut r = 0.0;
            for j in 0..60 {
                for i in 0..60 {
                    let (c, _) = armor_pixel(m, i as f32 / 60.0, j as f32 / 60.0);
                    if c[0] > c[2] + 18.0 {
                        r += 1.0;
                    }
                }
            }
            r / 3600.0
        };
        let armor = rusty(ArmorMat { finish: ArmorFinish::Gunmetal, wear: 0.4, battle: 0.0 });
        let map = rusty(ArmorMat { finish: ArmorFinish::MapMetal, wear: 0.2, battle: 0.0 });
        assert!(map > armor + 0.2, "map palette should read rusty, armor gunmetal shouldn't: {map} vs {armor}");
    }
}
