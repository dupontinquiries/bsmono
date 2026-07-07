//! foliage — the material that breaks the mold. NOT a tiling surface: alpha-cutout leaf cards (silhouette +
//! gaps) lit with SUBSURFACE TRANSLUCENCY, so thin leaves glow when backlit instead of going dark. Its
//! "condition" axis is season/health (lush → autumn → dead), not weathering. Top row: the season axis,
//! front-lit. Bottom row: the translucency proof — lush front-lit, then backlit WITHOUT vs WITH subsurface,
//! then autumn backlit. Run: cargo run --example foliage
use early_fps::math::Vec3;

const CELL: usize = 300;
const COLS: usize = 4;
const ROWS: usize = 2;

fn hash(i: u32, s: u32) -> f32 {
    let mut h = i.wrapping_mul(0x9e37_79b1) ^ s.wrapping_mul(0x85eb_ca6b);
    h ^= h >> 15;
    h = h.wrapping_mul(0x27d4_eb2d);
    h ^= h >> 13;
    (h & 0x00ff_ffff) as f32 / 0x00ff_ffff as f32
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
struct Season {
    leaf: [f32; 3],
    tip: [f32; 3], // browning at margins/tips
    transl: f32,   // subsurface strength (dries out toward dead)
    brown: f32,    // how much of the leaf has turned
}

/// Leaf-cluster on a card's uv → (albedo, alpha, local translucency). Several overlapping pointed leaves; the
/// topmost (by a per-leaf depth hash) wins. Alpha is the cutout that gives foliage its silhouette and gaps.
fn leaf_cluster(u: f32, v: f32, s: Season, seed: u32) -> ([f32; 3], f32, f32) {
    let mut alpha = 0.0;
    let mut col = [0.0; 3];
    let mut transl = 0.0;
    let mut top = -1.0;
    for k in 0..6u32 {
        let cx = 0.18 + hash(k, seed) * 0.64;
        let cy = 0.18 + hash(k, seed ^ 0x11) * 0.64;
        let ang = hash(k, seed ^ 0x22) * std::f32::consts::TAU;
        let len = 0.24 + hash(k, seed ^ 0x33) * 0.18;
        let wid = len * 0.42;
        let (sa, ca) = (ang.sin(), ang.cos());
        let (dx, dy) = (u - cx, v - cy);
        let lx = dx * ca + dy * sa; // along leaf
        let ly = -dx * sa + dy * ca; // across
        let tt = lx / len;
        if tt.abs() < 1.0 {
            let w = wid * (1.0 - tt.abs()); // taper to points at both ends
            if ly.abs() < w {
                let z = hash(k, seed ^ 0x44);
                if z > top {
                    top = z;
                    let margin = smoothstep(0.55, 1.0, ly.abs() / w.max(1e-4)) + smoothstep(0.6, 1.0, tt.abs());
                    let brownness = clamp01(margin * (0.4 + s.brown) + s.brown * 0.4);
                    let mut lc = mix3(s.leaf, s.tip, brownness);
                    // per-leaf tone + central vein highlight
                    let tone = (hash(k, seed ^ 0x55) - 0.5) * 0.22;
                    lc = [lc[0] * (1.0 + tone), lc[1] * (1.0 + tone), lc[2] * (1.0 + tone)];
                    let vein = smoothstep(w * 0.16, 0.0, ly.abs());
                    lc = [lc[0] + vein * 12.0, lc[1] + vein * 16.0, lc[2] + vein * 8.0];
                    col = lc;
                    alpha = 1.0;
                    transl = s.transl * (1.0 - brownness * 0.6);
                }
            }
        }
    }
    (col, alpha, transl)
}

struct Card {
    v: [Vec3; 4],
    n: Vec3,
    seed: u32,
}
fn build_bush() -> Vec<Card> {
    let mut cards = vec![];
    for i in 0..11u32 {
        let cx = (hash(i, 1) - 0.5) * 0.9;
        let cy = (hash(i, 2) - 0.5) * 1.1 + 0.1;
        let cz = (hash(i, 3) - 0.5) * 0.9;
        let center = Vec3::new(cx, cy, cz);
        let sz = 0.42 + hash(i, 4) * 0.28;
        // orientation: a tilted card
        let yaw = hash(i, 5) * std::f32::consts::TAU;
        let pitch = (hash(i, 6) - 0.5) * 1.2;
        let n = Vec3::new(yaw.cos() * pitch.cos(), pitch.sin(), yaw.sin() * pitch.cos()).normalized();
        let up = Vec3::new(0.0, 1.0, 0.0);
        let right = n.cross(up).normalized();
        let tup = right.cross(n).normalized();
        let (r, t) = (right * sz, tup * sz);
        cards.push(Card {
            v: [center - r - t, center + r - t, center + r + t, center - r + t],
            n,
            seed: 100 + i * 7,
        });
    }
    cards
}

/// Foliage lighting: soft two-sided diffuse + a subsurface transmission term that glows when you look toward
/// the light *through* the leaf. `sss` toggles the transmission (for the with/without proof).
fn light_leaf(albedo: [f32; 3], transl: f32, n: Vec3, lpos_dir: Vec3, vdir: Vec3, sss: bool) -> [f32; 3] {
    let nl = n.dot(lpos_dir);
    let diffuse = clamp01(nl.abs() * 0.45 + 0.55 * nl.max(0.0)); // soft, two-sided
    let ambient = 0.34;
    let mut out = [albedo[0] * (ambient + diffuse * 0.8), albedo[1] * (ambient + diffuse * 0.8), albedo[2] * (ambient + diffuse * 0.8)];
    if sss {
        // strongest when the light is behind the leaf, viewed toward it
        let back = clamp01((-lpos_dir).dot(vdir)).powf(2.5) * transl * clamp01(1.0 - nl.abs() * 0.5);
        // transmitted light is brighter and warmer (yellow-green glow)
        let glow = [albedo[0] * 1.4 + 40.0, albedo[1] * 1.5 + 55.0, albedo[2] * 0.9 + 20.0];
        out = [out[0] + glow[0] * back, out[1] + glow[1] * back, out[2] + glow[2] * back];
    }
    out
}

fn render_bush(cards: &[Card], s: Season, ldir: Vec3, sss: bool, buf: &mut [u8], w: usize, ox: usize, oy: usize) {
    let cam = Vec3::new(0.0, 0.15, 3.0);
    let focal = CELL as f32 * 0.5 / (32f32.to_radians() * 0.5).tan();
    let mut zb = vec![f32::INFINITY; CELL * CELL];
    let proj = |p: Vec3| {
        let z = cam.z - p.z;
        (CELL as f32 * 0.5 + p.x / z * focal, CELL as f32 * 0.5 - (p.y - cam.y) / z * focal, z)
    };
    for card in cards {
        let uv = [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]];
        for (a, b, c) in [(0usize, 1usize, 2usize), (0, 2, 3)] {
            let vs = [card.v[a], card.v[b], card.v[c]];
            let uvs = [uv[a], uv[b], uv[c]];
            let sp = [proj(vs[0]), proj(vs[1]), proj(vs[2])];
            let area = (sp[1].0 - sp[0].0) * (sp[2].1 - sp[0].1) - (sp[1].1 - sp[0].1) * (sp[2].0 - sp[0].0);
            if area.abs() < 1e-6 {
                continue;
            }
            let minx = sp.iter().map(|p| p.0).fold(f32::MAX, f32::min).floor().max(0.0) as usize;
            let maxx = sp.iter().map(|p| p.0).fold(f32::MIN, f32::max).ceil().min(CELL as f32 - 1.0) as usize;
            let miny = sp.iter().map(|p| p.1).fold(f32::MAX, f32::min).floor().max(0.0) as usize;
            let maxy = sp.iter().map(|p| p.1).fold(f32::MIN, f32::max).ceil().min(CELL as f32 - 1.0) as usize;
            for py in miny..=maxy {
                for px in minx..=maxx {
                    let (fx, fy) = (px as f32 + 0.5, py as f32 + 0.5);
                    let w0 = ((sp[1].0 - fx) * (sp[2].1 - fy) - (sp[1].1 - fy) * (sp[2].0 - fx)) / area;
                    let w1 = ((sp[2].0 - fx) * (sp[0].1 - fy) - (sp[2].1 - fy) * (sp[0].0 - fx)) / area;
                    let w2 = 1.0 - w0 - w1;
                    if w0 < 0.0 || w1 < 0.0 || w2 < 0.0 {
                        continue;
                    }
                    let uu = w0 * uvs[0][0] + w1 * uvs[1][0] + w2 * uvs[2][0];
                    let vv = w0 * uvs[0][1] + w1 * uvs[1][1] + w2 * uvs[2][1];
                    let (alb, alpha, tr) = leaf_cluster(uu, vv, s, card.seed);
                    if alpha < 0.5 {
                        continue; // ALPHA CUTOUT
                    }
                    let z = w0 * sp[0].2 + w1 * sp[1].2 + w2 * sp[2].2;
                    let zi = py * CELL + px;
                    if z >= zb[zi] {
                        continue;
                    }
                    zb[zi] = z;
                    let fragpos = vs[0] * w0 + vs[1] * w1 + vs[2] * w2;
                    let vdir = (cam - fragpos).normalized();
                    let out = light_leaf(alb, tr, card.n, ldir, vdir, sss);
                    let i = ((oy + py) * w + (ox + px)) * 3;
                    buf[i] = out[0].clamp(0.0, 255.0) as u8;
                    buf[i + 1] = out[1].clamp(0.0, 255.0) as u8;
                    buf[i + 2] = out[2].clamp(0.0, 255.0) as u8;
                }
            }
        }
    }
}

fn seasons() -> [(&'static str, Season); 4] {
    [
        ("lush", Season { leaf: [58.0, 118.0, 48.0], tip: [110.0, 120.0, 46.0], transl: 0.95, brown: 0.12 }),
        ("late summer", Season { leaf: [96.0, 126.0, 46.0], tip: [150.0, 120.0, 40.0], transl: 0.75, brown: 0.4 }),
        ("autumn", Season { leaf: [178.0, 120.0, 42.0], tip: [150.0, 62.0, 30.0], transl: 0.6, brown: 0.75 }),
        ("dead", Season { leaf: [120.0, 96.0, 62.0], tip: [88.0, 72.0, 50.0], transl: 0.15, brown: 0.95 }),
    ]
}

fn main() {
    let cards = build_bush();
    let w = CELL * COLS;
    let h = CELL * ROWS;
    let mut buf = vec![0u8; w * h * 3];
    for py in 0..h {
        let t = py as f32 / h as f32;
        for px in 0..w {
            let i = (py * w + px) * 3;
            buf[i] = (30.0 + t * 18.0) as u8;
            buf[i + 1] = (36.0 + t * 22.0) as u8;
            buf[i + 2] = (48.0 + t * 24.0) as u8;
        }
    }
    let front = Vec3::new(0.35, 0.55, 0.75).normalized();
    let back = Vec3::new(-0.3, 0.5, -0.82).normalized();
    let se = seasons();

    // row 1: season axis, front-lit
    for (i, (_n, s)) in se.iter().enumerate() {
        render_bush(&cards, *s, front, true, &mut buf, w, i * CELL, 0);
    }
    // row 2: translucency proof — lush front, lush backlit no-SSS, lush backlit SSS, autumn backlit SSS
    render_bush(&cards, se[0].1, front, true, &mut buf, w, 0, CELL);
    render_bush(&cards, se[0].1, back, false, &mut buf, w, CELL, CELL);
    render_bush(&cards, se[0].1, back, true, &mut buf, w, 2 * CELL, CELL);
    render_bush(&cards, se[2].1, back, true, &mut buf, w, 3 * CELL, CELL);
    image::save_buffer("foliage.png", &buf, w as u32, h as u32, image::ColorType::Rgb8).unwrap();
    println!("wrote foliage.png  ({w}x{h})  row1: lush→dead | row2: front, back(no-sss), back(sss), autumn back");

    // measures
    fn stats(cards: &[Card], s: Season, l: Vec3, sss: bool) -> (f32, f32, f32) {
        // foreground coverage, mean leaf brightness, warmth (R-G)
        let mut b = vec![0u8; CELL * CELL * 3];
        let mut tmp = vec![14u8; CELL * CELL * 3];
        render_bush(cards, s, l, sss, &mut tmp, CELL, 0, 0);
        b.copy_from_slice(&tmp);
        let (mut cov, mut sum, mut warm, mut n) = (0.0, 0.0, 0.0, 0u32);
        for i in 0..CELL * CELL {
            let (r, g, bl) = (b[i * 3] as f32, b[i * 3 + 1] as f32, b[i * 3 + 2] as f32);
            if (r - 14.0).abs() + (g - 14.0).abs() + (bl - 14.0).abs() > 12.0 {
                cov += 1.0;
                sum += (r + g + bl) / 3.0;
                warm += r - g;
                n += 1;
            }
        }
        (cov / (CELL * CELL) as f32 * 100.0, sum / n.max(1) as f32, warm / n.max(1) as f32)
    }
    let se = seasons();
    println!("\n  scene                     coverage%   mean-bright   warmth(R-G)");
    let (c0, b0, w0) = stats(&cards, se[0].1, front, true);
    println!("    lush front-lit           {c0:>7.1}    {b0:>8.0}     {w0:>6.0}");
    let (_, bn, _) = stats(&cards, se[0].1, back, false);
    println!("    lush backlit  no-SSS                {bn:>8.0}", );
    let (_, bs, _) = stats(&cards, se[0].1, back, true);
    println!("    lush backlit  with-SSS              {bs:>8.0}   ← glow: +{:.0}", bs - bn);
    let (_, _, wa) = stats(&cards, se[2].1, front, true);
    println!("    autumn front-lit                                warmth {wa:>4.0} (vs lush {w0:.0})");
}

#[cfg(test)]
mod checks {
    use super::*;
    fn stats(cards: &[Card], s: Season, l: Vec3, sss: bool) -> (f32, f32, f32) {
        let mut b = vec![14u8; CELL * CELL * 3];
        render_bush(cards, s, l, sss, &mut b, CELL, 0, 0);
        let (mut cov, mut sum, mut warm, mut n) = (0.0, 0.0, 0.0, 0u32);
        for i in 0..CELL * CELL {
            let (r, g, bl) = (b[i * 3] as f32, b[i * 3 + 1] as f32, b[i * 3 + 2] as f32);
            if (r - 14.0).abs() + (g - 14.0).abs() + (bl - 14.0).abs() > 12.0 {
                cov += 1.0;
                sum += (r + g + bl) / 3.0;
                warm += r - g;
                n += 1;
            }
        }
        (cov / (CELL * CELL) as f32 * 100.0, sum / n.max(1) as f32, warm / n.max(1) as f32)
    }
    #[test]
    fn foliage_reads_correctly() {
        let cards = build_bush();
        let se = seasons();
        let front = Vec3::new(0.35, 0.55, 0.75).normalized();
        let back = Vec3::new(-0.3, 0.5, -0.82).normalized();
        // alpha cutout: the bush has gaps — coverage is neither empty nor a solid block
        let (cov, _, warm_lush) = stats(&cards, se[0].1, front, true);
        assert!(cov > 8.0 && cov < 85.0, "bush should be a cutout clump, coverage {cov}");
        // translucency: backlit WITH subsurface glows brighter than WITHOUT
        let (_, bn, _) = stats(&cards, se[0].1, back, false);
        let (_, bs, _) = stats(&cards, se[0].1, back, true);
        assert!(bs > bn + 8.0, "backlit subsurface should glow: {bs} vs {bn}");
        // season: autumn is warmer (R−G up) than lush
        let (_, _, warm_autumn) = stats(&cards, se[2].1, front, true);
        assert!(warm_autumn > warm_lush + 30.0, "autumn should be warmer: {warm_autumn} vs {warm_lush}");
    }
}
