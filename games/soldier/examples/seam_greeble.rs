//! seam_greeble — proof that armor detail belongs in *geometry*, not a tiling texture. Generates a
//! welded-and-bolted seam from two primitives — a profile REVOLVED around an axis (bolt heads) and a
//! cross-section SWEPT along a path (weld bead) — with every parameter drawn from an engineering prior as
//! μ±σ clamped to a tolerance window. Renders it from several angles and two seeds (z-buffered, so features
//! genuinely self-occlude), and reports tri cost + the parameter distribution the generator actually
//! produced. Run: cargo run --example seam_greeble
use early_fps::math::Vec3;
use std::f32::consts::TAU;

const VIEW: usize = 480; // px per sub-view

// ── deterministic RNG with a normal-draw (sum-of-uniforms ≈ Gaussian) ───────────────────────────────────
struct Rng(u64);
impl Rng {
    fn new(seed: u64) -> Self {
        Rng(seed.wrapping_mul(0x9e37_79b9_7f4a_7c15) | 1)
    }
    fn u32(&mut self) -> u32 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        (x >> 32) as u32
    }
    fn f01(&mut self) -> f32 {
        self.u32() as f32 / u32::MAX as f32
    }
    fn range(&mut self, a: f32, b: f32) -> f32 {
        a + (b - a) * self.f01()
    }
    fn normal(&mut self, mu: f32, sigma: f32) -> f32 {
        // 4-uniform sum → mean 2, var 4/12; normalise to unit variance
        let s: f32 = (0..4).map(|_| self.f01()).sum::<f32>() - 2.0;
        mu + sigma * s * 1.732_05
    }
}
fn clampf(v: f32, lo: f32, hi: f32) -> f32 {
    v.clamp(lo, hi)
}

// ── geometry buffer: verts + per-tri colour ─────────────────────────────────────────────────────────────
struct Geo {
    v: Vec<Vec3>,
    t: Vec<[u32; 3]>,
    c: Vec<[f32; 3]>,
}
impl Geo {
    fn new() -> Self {
        Geo { v: vec![], t: vec![], c: vec![] }
    }
    fn vert(&mut self, p: Vec3) -> u32 {
        self.v.push(p);
        (self.v.len() - 1) as u32
    }
    fn tri(&mut self, a: u32, b: u32, cc: u32, col: [f32; 3]) {
        self.t.push([a, b, cc]);
        self.c.push(col);
    }
    fn quad(&mut self, a: u32, b: u32, cc: u32, d: u32, col: [f32; 3]) {
        self.tri(a, b, cc, col);
        self.tri(a, cc, d, col);
    }
    fn ntris(&self) -> usize {
        self.t.len()
    }
}

// Revolve a profile (r,y) about +Y at `center`, `seg` sides, `yaw` phase. Caps the top with a fan if the
// last radius is non-zero (flat head face). This one primitive makes both hex heads (seg=6) and domed pan
// heads (seg=12) — exactly the "revolve a profile around an axis" operation.
fn revolve(g: &mut Geo, center: Vec3, prof: &[[f32; 2]], seg: usize, yaw: f32, tilt: f32, col: [f32; 3], top_col: [f32; 3]) {
    let place = |r: f32, y: f32, s: usize| -> Vec3 {
        let a = yaw + s as f32 / seg as f32 * TAU;
        // tilt the whole head slightly off-axis about X so installed bolts aren't all dead-perpendicular
        let (lx, ly, lz) = (a.cos() * r, y, a.sin() * r);
        let (ty, tz) = (ly * tilt.cos() - lz * tilt.sin(), ly * tilt.sin() + lz * tilt.cos());
        center + Vec3::new(lx, ty, tz)
    };
    let mut rings: Vec<Vec<u32>> = vec![];
    for p in prof {
        let mut ring = vec![];
        for s in 0..seg {
            let idx = g.vert(place(p[0], p[1], s));
            ring.push(idx);
        }
        rings.push(ring);
    }
    for i in 0..rings.len() - 1 {
        for s in 0..seg {
            let s2 = (s + 1) % seg;
            g.quad(rings[i][s], rings[i][s2], rings[i + 1][s2], rings[i + 1][s], col);
        }
    }
    let top = *prof.last().unwrap();
    if top[0] > 1e-4 {
        let c = g.vert(place(0.0, top[1], 0));
        let last = rings.last().unwrap().clone();
        for s in 0..seg {
            let s2 = (s + 1) % seg;
            g.tri(last[s], last[s2], c, top_col);
        }
    }
}

#[derive(Clone, Copy)]
struct BoltSpec {
    dia: f32,
    head_h: f32,
    hex: bool,
    yaw: f32,
    tilt: f32,
}
// Draw one bolt from engineering priors: head height ≈ 0.64·diameter (real hex-head ratio), ~3:1 hex:dome
// mix, random clocking, a whisper of install tilt. Clamped to a tolerance window so nothing degenerate.
fn draw_bolt(rng: &mut Rng) -> BoltSpec {
    let dia = clampf(rng.normal(0.052, 0.009), 0.036, 0.074);
    let head_h = dia * clampf(rng.normal(0.64, 0.06), 0.5, 0.8);
    BoltSpec { dia, head_h, hex: rng.f01() < 0.75, yaw: rng.f01() * TAU, tilt: rng.normal(0.0, 0.03) }
}
fn emit_bolt(g: &mut Geo, center: Vec3, b: BoltSpec) -> usize {
    let before = g.ntris();
    let r = b.dia * 0.5;
    let steel = [150.0, 156.0, 166.0];
    let top = [176.0, 182.0, 192.0];
    if b.hex {
        // hex prism + chamfered flat top
        revolve(g, center, &[[r, 0.0], [r, b.head_h * 0.72], [r * 0.82, b.head_h]], 6, b.yaw, b.tilt, steel, top);
    } else {
        // domed pan head (quarter-round revolve) — kept cheap: 8 sides, 3-ring profile ≈ 32 tris
        revolve(g, center, &[[r, 0.0], [r * 0.7, b.head_h * 0.6], [0.0, b.head_h]], 8, b.yaw, b.tilt, steel, top);
    }
    g.ntris() - before
}

// Sweep a rounded half-ellipse cross-section along the seam path a→b. Width/height ripple gives the
// stacked-dimes weld read; a little lateral wander keeps the bead off a dead-straight ruler line.
fn weld_bead(g: &mut Geo, a: Vec3, b: Vec3, w0: f32, h0: f32, rng: &mut Rng) -> usize {
    let before = g.ntris();
    let col = [138.0, 134.0, 130.0]; // oxidised weld metal, slightly warm-grey vs the plate
    let col_hi = [170.0, 150.0, 138.0]; // faint heat tint on the crown
    let sect: [[f32; 2]; 5] = [[-1.0, 0.0], [-0.55, 0.72], [0.0, 1.0], [0.55, 0.72], [1.0, 0.0]];
    let n = 22;
    let wander_seed = rng.range(0.0, 10.0);
    let mut rings: Vec<Vec<u32>> = vec![];
    for i in 0..=n {
        let t = i as f32 / n as f32;
        let dime = 1.0 + 0.22 * (t * 22.0).sin().abs(); // stacked-dime pulse
        let wander = ((t * 9.0 + wander_seed).sin() + (t * 23.0).sin() * 0.4) * 0.006;
        let center = a.lerp(b, t) + Vec3::new(wander, 0.0, 0.0);
        let (w, h) = (w0 * (0.85 + 0.25 * dime.fract()), h0 * dime);
        let mut ring = vec![];
        for s in sect {
            let idx = g.vert(center + Vec3::new(s[0] * w, 0.0, s[1] * h));
            ring.push(idx);
        }
        rings.push(ring);
    }
    for i in 0..rings.len() - 1 {
        for s in 0..sect.len() - 1 {
            let crown = s == 1 || s == 2; // centre strips catch the heat tint
            g.quad(rings[i][s], rings[i][s + 1], rings[i + 1][s + 1], rings[i + 1][s], if crown { col_hi } else { col });
        }
    }
    g.ntris() - before
}

struct BuildStats {
    total: usize,
    base: usize,
    weld: usize,
    bolts: usize,
    specs: Vec<BoltSpec>,
}
fn build(seed: u64, lod_far: bool) -> (Geo, BuildStats) {
    let mut g = Geo::new();
    let mut rng = Rng::new(seed);
    let plate = [120.0, 127.0, 139.0];
    let plate_b = [116.0, 123.0, 134.0];

    // two butted plates in the XY plane facing +Z, a narrow recessed channel at the seam (x≈0)
    let (y0, y1) = (-0.75, 0.75);
    let base0 = g.ntris();
    let mk_plate = |g: &mut Geo, x0: f32, x1: f32, col: [f32; 3]| {
        let a = g.vert(Vec3::new(x0, y0, 0.0));
        let b = g.vert(Vec3::new(x1, y0, 0.0));
        let c = g.vert(Vec3::new(x1, y1, 0.0));
        let d = g.vert(Vec3::new(x0, y1, 0.0));
        g.quad(a, b, c, d, col);
    };
    mk_plate(&mut g, -0.8, -0.012, plate);
    mk_plate(&mut g, 0.012, 0.8, plate_b);
    let base = g.ntris() - base0;

    // weld bead down the seam
    let weld = weld_bead(&mut g, Vec3::new(0.0, y0 + 0.03, 0.0), Vec3::new(0.0, y1 - 0.03, 0.0), 0.028, 0.03, &mut rng);

    // two bolt rows flanking the seam, jittered pitch so they're not on a perfect grid
    let mut specs = vec![];
    let mut bolts_total = 0;
    if !lod_far {
        for &col_x in &[-0.42f32, 0.42] {
            let mut y = y0 + 0.14;
            let pitch = 0.26;
            while y < y1 - 0.1 {
                let spec = draw_bolt(&mut rng);
                let jx = rng.normal(0.0, 0.02);
                let jy = rng.normal(0.0, 0.03);
                let n = emit_bolt(&mut g, Vec3::new(col_x + jx, y + jy, 0.0), spec);
                specs.push(spec);
                        bolts_total += n;
                y += pitch * (1.0 + rng.normal(0.0, 0.12));
            }
        }
    }
    let total = g.ntris();
    (g, BuildStats { total, base, weld, bolts: bolts_total, specs })
}

// ── z-buffered flat-diffuse rasteriser (camera-oriented normals → no winding bugs) ──────────────────────
fn render_view(g: &Geo, buf: &mut [u8], w: usize, _h: usize, ox: usize, oy: usize, cam: Vec3, target: Vec3) {
    let fwd = (target - cam).normalized();
    let right = fwd.cross(Vec3::new(0.0, 1.0, 0.0)).normalized();
    let up = right.cross(fwd);
    let focal = VIEW as f32 * 0.5 / (38f32.to_radians() * 0.5).tan();
    let light = Vec3::new(-0.5, 0.62, 0.6).normalized();
    let mut zb = vec![f32::INFINITY; VIEW * VIEW];
    let proj = |p: Vec3| -> (f32, f32, f32) {
        let rel = p - cam;
        let (x, y, z) = (rel.dot(right), rel.dot(up), rel.dot(fwd));
        (VIEW as f32 * 0.5 + x / z * focal, VIEW as f32 * 0.5 - y / z * focal, z)
    };
    for (ti, tri) in g.t.iter().enumerate() {
        let p = [g.v[tri[0] as usize], g.v[tri[1] as usize], g.v[tri[2] as usize]];
        let s = [proj(p[0]), proj(p[1]), proj(p[2])];
        if s.iter().any(|v| v.2 < 0.05) {
            continue;
        }
        let mut n = (p[1] - p[0]).cross(p[2] - p[0]).normalized();
        let vd = (cam - p[0]).normalized();
        if n.dot(vd) < 0.0 {
            n = n * -1.0;
        }
        let shade = 0.32 + 0.78 * n.dot(light).max(0.0);
        let col = g.c[ti];
        let (minx, maxx) = (s.iter().map(|v| v.0).fold(f32::MAX, f32::min).floor().max(0.0) as usize, s.iter().map(|v| v.0).fold(f32::MIN, f32::max).ceil().min(VIEW as f32 - 1.0) as usize);
        let (miny, maxy) = (s.iter().map(|v| v.1).fold(f32::MAX, f32::min).floor().max(0.0) as usize, s.iter().map(|v| v.1).fold(f32::MIN, f32::max).ceil().min(VIEW as f32 - 1.0) as usize);
        let area = (s[1].0 - s[0].0) * (s[2].1 - s[0].1) - (s[1].1 - s[0].1) * (s[2].0 - s[0].0);
        if area.abs() < 1e-5 {
            continue;
        }
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
                let zi = py * VIEW + px;
                if z >= zb[zi] {
                    continue;
                }
                zb[zi] = z;
                let gx = ox + px;
                let gy = oy + py;
                let i = (gy * w + gx) * 3;
                buf[i] = (col[0] * shade).clamp(0.0, 255.0) as u8;
                buf[i + 1] = (col[1] * shade).clamp(0.0, 255.0) as u8;
                buf[i + 2] = (col[2] * shade).clamp(0.0, 255.0) as u8;
            }
        }
    }
}

fn mean_std(xs: &[f32]) -> (f32, f32, f32, f32) {
    let n = xs.len().max(1) as f32;
    let mu = xs.iter().sum::<f32>() / n;
    let var = xs.iter().map(|x| (x - mu).powi(2)).sum::<f32>() / n;
    (mu, var.sqrt(), xs.iter().cloned().fold(f32::MAX, f32::min), xs.iter().cloned().fold(f32::MIN, f32::max))
}

fn main() {
    // ── render a 2×2 grid: front(A), oblique(A), grazing(A), oblique(B) ──
    let w = VIEW * 2;
    let h = VIEW * 2;
    let mut buf = vec![0u8; w * h * 3];
    for py in 0..h {
        let t = py as f32 / h as f32;
        for px in 0..w {
            let i = (py * w + px) * 3;
            buf[i] = (16.0 + t * 12.0) as u8;
            buf[i + 1] = (18.0 + t * 13.0) as u8;
            buf[i + 2] = (24.0 + t * 16.0) as u8;
        }
    }
    let (ga, _) = build(0xA11CE, false);
    let (gb, _) = build(0xB0B, false);
    let tgt = Vec3::new(0.0, 0.0, 0.03);
    render_view(&ga, &mut buf, w, h, 0, 0, Vec3::new(0.15, 0.1, 2.3), tgt); // front
    render_view(&ga, &mut buf, w, h, VIEW, 0, Vec3::new(1.35, 0.95, 1.7), tgt); // oblique 3/4
    render_view(&ga, &mut buf, w, h, 0, VIEW, Vec3::new(1.9, 0.12, 0.55), Vec3::new(0.0, 0.0, 0.06)); // grazing
    render_view(&gb, &mut buf, w, h, VIEW, VIEW, Vec3::new(1.35, 0.95, 1.7), tgt); // oblique, seed B
    image::save_buffer("seam_greeble.png", &buf, w as u32, h as u32, image::ColorType::Rgb8).unwrap();
    println!("wrote seam_greeble.png  ({w}x{h})  [ front | oblique-A ; grazing | oblique-B ]");

    // ── tri budget ──
    let (_, sa) = build(0xA11CE, false);
    let (_, far) = build(0xA11CE, true);
    println!("\n  TRI BUDGET (close LOD)");
    println!("    base plates : {:>4}", sa.base);
    println!("    weld bead   : {:>4}", sa.weld);
    println!("    {} bolts     : {:>4}  ({} avg/bolt)", sa.specs.len(), sa.bolts, sa.bolts / sa.specs.len().max(1));
    println!("    TOTAL close : {:>4}   (budget ~600)", sa.total);
    println!("    TOTAL far   : {:>4}   (bolts LOD-baked out past ~15 m)", far.total);

    // ── statistical stability: draw 500 bolts from the same generator, confirm μ/σ land in-window ──
    let mut rng = Rng::new(0x5EED);
    let mut dias = vec![];
    let mut heads = vec![];
    let mut ratios = vec![];
    let mut hex = 0;
    let mut tris = vec![];
    for _ in 0..500 {
        let b = draw_bolt(&mut rng);
        dias.push(b.dia);
        heads.push(b.head_h);
        ratios.push(b.head_h / b.dia);
        if b.hex {
            hex += 1;
        }
        // count tris without keeping geometry
        let mut tmp = Geo::new();
        tris.push(emit_bolt(&mut tmp, Vec3::splat(0.0), b) as f32);
    }
    let (dm, ds, dlo, dhi) = mean_std(&dias);
    let (_hm, _hs, _, _) = mean_std(&heads);
    let (rm, rs, _, _) = mean_std(&ratios);
    let (tm, tsd, tlo, thi) = mean_std(&tris);
    println!("\n  PARAMETER STABILITY (500 bolts, target in parens)");
    println!("    diameter   μ {:.4}  σ {:.4}  range [{:.3},{:.3}]   (μ≈0.052, clamp[0.036,0.074])", dm, ds, dlo, dhi);
    println!("    head/dia   μ {:.3}   σ {:.3}                        (μ≈0.64, σ≈0.06)", rm, rs);
    println!("    hex share  {:.0}%                                     (target 75%)", hex as f32 / 5.0);
    println!("    tris/bolt  μ {:.1}   σ {:.1}   range [{:.0},{:.0}]      (target ~24–36)", tm, tsd, tlo, thi);
    println!("    → distinct diameters: {}/500 (non-repetition)", {
        let mut u = dias.iter().map(|d| (d * 10000.0) as i32).collect::<Vec<_>>();
        u.sort();
        u.dedup();
        u.len()
    });
}

#[cfg(test)]
mod checks {
    use super::*;
    #[test]
    fn geometry_is_sane() {
        let (g, s) = build(0xA11CE, false);
        // no NaNs / infs
        assert!(g.v.iter().all(|p| p.x.is_finite() && p.y.is_finite() && p.z.is_finite()));
        // heads + weld stand proud of the panel plane (z>0); panel is at z=0
        let maxz = g.v.iter().map(|p| p.z).fold(f32::MIN, f32::max);
        assert!(maxz > 0.02, "features should protrude in +z, got {maxz}");
        // close LOD within budget, far LOD strictly lighter
        assert!(s.total <= 600, "over budget: {}", s.total);
        // every placed bolt has a valid (in-tolerance) diameter
        assert!(s.specs.iter().all(|b| (0.036..=0.074).contains(&b.dia)));
        // far LOD is strictly lighter than close (bolts removed)
        let (_, far) = build(0xA11CE, true);
        assert!(far.total < s.total);
    }
}
