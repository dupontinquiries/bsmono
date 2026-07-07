//! downed_fade — the hardcore "fade to white" that plays when a trooper is downed, before they hand off to
//! spectator. Driven by one timer `downed` in [0,1] that the death moment starts (the `was_alive → !alive`
//! edge in track_profile) and ramps over ~1.2s. Two phases: colour drains out (the life leaving), then an
//! UNEVEN white dissolve blooms across the surface (ghosting away). Shown on the battle-worn trooper plate so
//! it fades the actual armor look. Run: cargo run --example downed_fade
use early_fps::math::Vec3;

const CELL: usize = 300;

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

struct Surf {
    pos: Vec3,
    nrm: Vec3,
    uv: (f32, f32),
}
fn plate() -> (Vec<Surf>, Vec<[u32; 3]>) {
    let (nx, ny) = (56, 60);
    let (rr, arc) = (1.25f32, 0.62f32);
    let disp = |u: f32, v: f32| smoothstep(0.11, 0.0, (u - 0.5f32).abs()) * 0.05 - (smoothstep(0.05, 0.0, u.min(1.0 - u)) + smoothstep(0.05, 0.0, v.min(1.0 - v))) * 0.045;
    let posf = |u: f32, v: f32| {
        let th = lerp(-arc, arc, u);
        let r = rr + disp(u, v);
        Vec3::new(r * th.sin(), lerp(-0.74, 0.74, v), r * th.cos() - rr * 0.55)
    };
    let nrmf = |u: f32, v: f32| {
        let e = 0.004;
        (posf(u, (v + e).min(1.0)) - posf(u, (v - e).max(0.0))).cross(posf((u + e).min(1.0), v) - posf((u - e).max(0.0), v)).normalized()
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
            tris.push([a, a + 1, ((j + 1) * w + i) as u32 + 1]);
            tris.push([a, ((j + 1) * w + i) as u32 + 1, ((j + 1) * w + i) as u32]);
        }
    }
    (verts, tris)
}

const POLISH: [f32; 3] = [182.0, 188.0, 198.0];

// battle-worn gunmetal (a downed trooper), plus the fade. Returns (albedo, gloss, emissive).
fn trooper_downed(u: f32, v: f32, downed: f32) -> ([f32; 3], f32, f32) {
    let edge = 1.0 - smoothstep(0.0, 0.10, u.min(1.0 - u).min(v).min(1.0 - v));
    let fine = fbm(u * 22.0, v * 22.0, 9);
    let mut col = [96.0, 100.0, 110.0];
    let mut gloss = 0.7;
    // service scuff at edges
    let scuff = clamp01(edge * 0.6 * (0.7 + fine * 0.6));
    col = mix3(col, POLISH, scuff);
    gloss = lerp(gloss, 0.9, scuff);
    // some battle scarring (they just went down)
    for (cx, cy) in [(0.6, 0.42), (0.36, 0.64)] {
        let d = ((u - cx).powi(2) + (v - cy).powi(2)).sqrt();
        col = mix3(col, [26.0, 24.0, 26.0], smoothstep(0.16, 0.06, d) * 0.5);
        col = mix3(col, POLISH, smoothstep(0.08, 0.0, d) * 0.7);
    }

    // ── PHASE 1: colour drains toward greyscale (0.0 – 0.55) ──
    let desat = smoothstep(0.0, 0.55, downed);
    let l = (col[0] + col[1] + col[2]) / 3.0;
    col = mix3(col, [l, l, l], desat);

    // ── PHASE 2: uneven white dissolve blooms (0.3 – 1.0), high points/edges go first ──
    let front_noise = fbm(u * 7.0, v * 7.0, 17) * 0.5 + edge * 0.25;
    let white = smoothstep(0.0, 1.0, (downed - 0.3) * 1.7 + (front_noise - 0.35) * 0.9);
    col = mix3(col, [252.0, 253.0, 255.0], white);
    // emissive bloom as it whites out (self-lit, blows to spectator hand-off)
    let emissive = white * (0.55 + 0.45 * downed) + desat * 0.05;
    gloss = lerp(gloss, 0.0, white); // dematerialising: highlight dies
    (col, gloss, emissive)
}

fn shade_cell(downed: f32, verts: &[Surf], tris: &[[u32; 3]], buf: &mut [u8], w: usize, ox: usize) {
    let cam = Vec3::new(0.0, 0.0, 2.5);
    let focal = CELL as f32 * 0.5 / (34f32.to_radians() * 0.5).tan();
    let light = Vec3::new(-0.55, 0.5, 0.66).normalized();
    let mut zb = vec![f32::INFINITY; CELL * CELL];
    let proj = |p: Vec3| {
        let z = cam.z - p.z;
        (CELL as f32 * 0.5 + p.x / z * focal, CELL as f32 * 0.5 - p.y / z * focal, z)
    };
    for tri in tris {
        let vv = [&verts[tri[0] as usize], &verts[tri[1] as usize], &verts[tri[2] as usize]];
        let s = [proj(vv[0].pos), proj(vv[1].pos), proj(vv[2].pos)];
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
                let mut n = (vv[0].nrm * w0 + vv[1].nrm * w1 + vv[2].nrm * w2).normalized();
                if n.z < 0.0 {
                    n = n * -1.0;
                }
                let uu = w0 * vv[0].uv.0 + w1 * vv[1].uv.0 + w2 * vv[2].uv.0;
                let vy = w0 * vv[0].uv.1 + w1 * vv[1].uv.1 + w2 * vv[2].uv.1;
                let (alb, gloss, emis) = trooper_downed(uu, vy, downed);
                let diff = n.dot(light).max(0.0);
                let hlf = (light + Vec3::new(0.0, 0.0, 1.0)).normalized();
                let spec = n.dot(hlf).max(0.0).powf(6.0 + gloss * 90.0) * gloss;
                let shade = 0.34 + 0.72 * diff;
                let add = emis * 235.0;
                let i = ((py) * w + (ox + px)) * 3;
                buf[i] = (alb[0] * shade + 235.0 * spec + add).clamp(0.0, 255.0) as u8;
                buf[i + 1] = (alb[1] * shade + 238.0 * spec + add).clamp(0.0, 255.0) as u8;
                buf[i + 2] = (alb[2] * shade + 245.0 * spec + add).clamp(0.0, 255.0) as u8;
            }
        }
    }
}

fn main() {
    let (verts, tris) = plate();
    let stages = [0.0f32, 0.3, 0.55, 0.8, 1.0];
    let w = CELL * stages.len();
    let mut buf = vec![10u8; w * CELL * 3];
    for (k, &t) in stages.iter().enumerate() {
        shade_cell(t, &verts, &tris, &mut buf, w, k * CELL);
    }
    image::save_buffer("downed_fade.png", &buf, w as u32, CELL as u32, image::ColorType::Rgb8).unwrap();
    println!("wrote downed_fade.png  ({}x{CELL})  downed t = {:?}", w, stages);

    // measure the fade: brightness rises, saturation falls, ends near pure white
    println!("\n  downed-t   brightness   saturation");
    for &t in &stages {
        let (mut b, mut sat, mut n) = (0.0, 0.0, 0u32);
        for j in 0..100 {
            for i in 0..100 {
                let (c, _, e) = trooper_downed(i as f32 / 100.0, j as f32 / 100.0, t);
                let lit = [(c[0] + e * 235.0).min(255.0), (c[1] + e * 235.0).min(255.0), (c[2] + e * 235.0).min(255.0)];
                let mx = lit[0].max(lit[1]).max(lit[2]);
                let mn = lit[0].min(lit[1]).min(lit[2]);
                b += (lit[0] + lit[1] + lit[2]) / 3.0;
                sat += if mx > 0.0 { (mx - mn) / mx } else { 0.0 };
                n += 1;
            }
        }
        println!("  {t:.2}        {:>8.0}     {:.3}", b / n as f32, sat / n as f32);
    }
}

#[cfg(test)]
mod checks {
    use super::*;
    fn frame(t: f32) -> (f32, f32) {
        let (mut b, mut sat, mut n) = (0.0, 0.0, 0u32);
        for j in 0..80 {
            for i in 0..80 {
                let (c, _, e) = trooper_downed(i as f32 / 80.0, j as f32 / 80.0, t);
                let lit = [(c[0] + e * 235.0).min(255.0), (c[1] + e * 235.0).min(255.0), (c[2] + e * 235.0).min(255.0)];
                let mx = lit[0].max(lit[1]).max(lit[2]);
                let mn = lit[0].min(lit[1]).min(lit[2]);
                b += (lit[0] + lit[1] + lit[2]) / 3.0;
                sat += if mx > 0.0 { (mx - mn) / mx } else { 0.0 };
                n += 1;
            }
        }
        (b / n as f32, sat / n as f32)
    }
    #[test]
    fn fade_reads_correctly() {
        let (b0, s0) = frame(0.0);
        let (bm, sm) = frame(0.55);
        let (b1, s1) = frame(1.0);
        // brightness rises monotonically toward white
        assert!(bm > b0 && b1 > bm, "should brighten: {b0} {bm} {b1}");
        // saturation drains
        assert!(sm < s0 && s1 < sm, "colour should drain: {s0} {sm} {s1}");
        // ends near pure white
        assert!(b1 > 235.0 && s1 < 0.04, "should end near white: bright {b1} sat {s1}");
    }
}
