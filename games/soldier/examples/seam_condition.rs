//! seam_condition — the seam's look driven by three separate knobs, plus UNWIRED GPU scaffolding.
//!
//!   • grade      (from a structure's base/max HP)  → STATIC build strength: bolt pitch & size, weld throat,
//!                                                     plate thickness. Set at spawn, never animates.
//!   • integrity  (hp / max_hp)                     → a gentle GLOBAL floor of wear, everywhere.
//!   • damage field                                  → LOCAL damage deposited where hits land, then DIFFUSED
//!                                                     outward, so a hit craters/cracks *there* and the rest
//!                                                     stays clean (no sudden global re-detail pop).
//!
//! The global floor and the local spikes are seeded into one field and diffused together, so the condition
//! at any point is the blend the two produce — exactly the "diffusion of a global and a local" idea.
//!
//! GPU seam (secondary, unwired): the diffusion step — the obvious massive-parallelism win — sits behind a
//! `DiffusionBackend`. There's a real `Cpu` stencil and a `Gpu` STUB that a `GraphicsQuality::High` setting
//! would select. No GPU is linked; the stub defers to the CPU stencil so behaviour is identical until a real
//! compute backend is wired in. Run: cargo run --example seam_condition
use early_fps::math::Vec3;
use std::f32::consts::TAU;

const VIEW: usize = 480;

// ── RNG ─────────────────────────────────────────────────────────────────────────────────────────────────
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
    fn normal(&mut self, mu: f32, sigma: f32) -> f32 {
        let s: f32 = (0..4).map(|_| self.f01()).sum::<f32>() - 2.0;
        mu + sigma * s * 1.732_05
    }
}
fn clampf(v: f32, lo: f32, hi: f32) -> f32 {
    v.clamp(lo, hi)
}
fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}
fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}
fn mix3(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    [lerp(a[0], b[0], t), lerp(a[1], b[1], t), lerp(a[2], b[2], t)]
}

// ── UNWIRED GPU SEAM: diffusion behind a backend the High-graphics path would swap ─────────────────────
#[derive(Clone, Copy, Debug)]
#[allow(dead_code)]
enum GraphicsQuality {
    Low,
    Medium,
    High, // would route the diffusion (and later, greeble emission) to the GPU
}

trait DiffusionBackend {
    fn label(&self) -> &'static str;
    /// One relaxation pass of a 4-neighbour Laplacian: dst = src + rate·(avg(neighbours) − src).
    fn step(&self, src: &[f32], dst: &mut [f32], w: usize, h: usize, rate: f32);
}

/// Real, wired. This same per-cell stencil is what a compute shader would run one-thread-per-cell.
struct CpuDiffusion;
impl DiffusionBackend for CpuDiffusion {
    fn label(&self) -> &'static str {
        "cpu"
    }
    fn step(&self, src: &[f32], dst: &mut [f32], w: usize, h: usize, rate: f32) {
        for y in 0..h {
            for x in 0..w {
                let i = y * w + x;
                let l = src[y * w + x.saturating_sub(1)];
                let r = src[y * w + (x + 1).min(w - 1)];
                let u = src[y.saturating_sub(1) * w + x];
                let d = src[(y + 1).min(h - 1) * w + x];
                dst[i] = src[i] + rate * ((l + r + u + d) * 0.25 - src[i]);
            }
        }
    }
}

/// UNWIRED stub. A real backend would: upload `src` to a storage texture, dispatch the stencil across w·h
/// GPU threads (and, batched, across *every* structure's field at once — the parallelism win), and keep the
/// field resident GPU-side with NO per-frame readback (only sparse hit-events go up, sparse "did it cave?"
/// bytes come back). Until that exists, defer to the CPU stencil so results are bit-identical and High
/// quality still runs. This is the single place the GPU port slots in.
struct GpuDiffusion;
impl DiffusionBackend for GpuDiffusion {
    fn label(&self) -> &'static str {
        "gpu-stub→cpu"
    }
    fn step(&self, src: &[f32], dst: &mut [f32], w: usize, h: usize, rate: f32) {
        CpuDiffusion.step(src, dst, w, h, rate); // no compute backend linked yet
    }
}

fn backend_for(q: GraphicsQuality) -> Box<dyn DiffusionBackend> {
    match q {
        GraphicsQuality::High => Box::new(GpuDiffusion), // would be a real GPU backend once wired
        _ => Box::new(CpuDiffusion),
    }
}

// ── damage field ────────────────────────────────────────────────────────────────────────────────────────
struct DamageField {
    w: usize,
    h: usize,
    a: Vec<f32>,
}
impl DamageField {
    fn new(w: usize, h: usize) -> Self {
        DamageField { w, h, a: vec![0.0; w * h] }
    }
    fn fill(&mut self, v: f32) {
        self.a.iter_mut().for_each(|c| *c = v);
    }
    /// Deposit a smooth Gaussian bump at uv — the stamp itself is the spatial diffusion of a point hit.
    fn deposit(&mut self, u: f32, v: f32, radius: f32, strength: f32) {
        for y in 0..self.h {
            for x in 0..self.w {
                let fx = (x as f32 + 0.5) / self.w as f32;
                let fy = (y as f32 + 0.5) / self.h as f32;
                let d = ((fx - u).powi(2) + (fy - v).powi(2)).sqrt() / radius;
                let i = y * self.w + x;
                self.a[i] = (self.a[i] + strength * (-(d * d)).exp()).min(1.6);
            }
        }
    }
    fn diffuse(&mut self, backend: &dyn DiffusionBackend, iters: usize, rate: f32) {
        let mut tmp = self.a.clone();
        for _ in 0..iters {
            backend.step(&self.a, &mut tmp, self.w, self.h, rate);
            std::mem::swap(&mut self.a, &mut tmp);
        }
    }
    fn sample(&self, u: f32, v: f32) -> f32 {
        let fx = u.clamp(0.0, 1.0) * self.w as f32 - 0.5;
        let fy = v.clamp(0.0, 1.0) * self.h as f32 - 0.5;
        let x0 = fx.floor().max(0.0) as usize;
        let y0 = fy.floor().max(0.0) as usize;
        let x1 = (x0 + 1).min(self.w - 1);
        let y1 = (y0 + 1).min(self.h - 1);
        let tx = (fx - x0 as f32).clamp(0.0, 1.0);
        let ty = (fy - y0 as f32).clamp(0.0, 1.0);
        let ab = lerp(self.a[y0 * self.w + x0], self.a[y0 * self.w + x1], tx);
        let cd = lerp(self.a[y1 * self.w + x0], self.a[y1 * self.w + x1], tx);
        lerp(ab, cd, ty).clamp(0.0, 1.0)
    }
}

// panel world extents → uv[0,1]
const PX: (f32, f32) = (-0.8, 0.8);
const PY: (f32, f32) = (-0.75, 0.75);
fn to_uv(x: f32, y: f32) -> (f32, f32) {
    ((x - PX.0) / (PX.1 - PX.0), (y - PY.0) / (PY.1 - PY.0))
}

#[allow(dead_code)]
struct Cond {
    grade: f32,
    integrity: f32,
    field: DamageField,
    quality: GraphicsQuality,
}
fn make_cond(grade: f32, integrity: f32, impacts: &[(f32, f32, f32, f32)], q: GraphicsQuality) -> Cond {
    let mut field = DamageField::new(48, 48);
    field.fill((1.0 - integrity) * 0.5); // global integrity → uniform floor
    for &(u, v, r, s) in impacts {
        field.deposit(u, v, r, s); // local hits → spikes
    }
    field.diffuse(&*backend_for(q), 12, 0.9); // diffuse blends the floor + spreads the spikes
    Cond { grade, integrity, field, quality: q }
}

// ── geometry ────────────────────────────────────────────────────────────────────────────────────────────
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

fn revolve(g: &mut Geo, center: Vec3, prof: &[[f32; 2]], seg: usize, yaw: f32, tilt: f32, col: [f32; 3], top: [f32; 3]) {
    let place = |r: f32, y: f32, s: usize| -> Vec3 {
        let a = yaw + s as f32 / seg as f32 * TAU;
        let (lx, ly, lz) = (a.cos() * r, y, a.sin() * r);
        let (ty, tz) = (ly * tilt.cos() - lz * tilt.sin(), ly * tilt.sin() + lz * tilt.cos());
        center + Vec3::new(lx, ty, tz)
    };
    let mut rings: Vec<Vec<u32>> = vec![];
    for p in prof {
        let ring: Vec<u32> = (0..seg).map(|s| g.vert(place(p[0], p[1], s))).collect();
        rings.push(ring);
    }
    for i in 0..rings.len() - 1 {
        for s in 0..seg {
            let s2 = (s + 1) % seg;
            g.quad(rings[i][s], rings[i][s2], rings[i + 1][s2], rings[i + 1][s], col);
        }
    }
    let tp = *prof.last().unwrap();
    if tp[0] > 1e-4 {
        let c = g.vert(place(0.0, tp[1], 0));
        let last = rings.last().unwrap().clone();
        for s in 0..seg {
            g.tri(last[s], last[(s + 1) % seg], c, top);
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
fn draw_bolt(rng: &mut Rng, dia_mu: f32) -> BoltSpec {
    let dia = clampf(rng.normal(dia_mu, 0.009), 0.032, 0.078);
    let head_h = dia * clampf(rng.normal(0.64, 0.06), 0.5, 0.8);
    BoltSpec { dia, head_h, hex: rng.f01() < 0.75, yaw: rng.f01() * TAU, tilt: rng.normal(0.0, 0.03) }
}
/// Emit a bolt whose condition follows local damage: pristine → clean head; loosened → tilts and backs out;
/// sheared → just a rusted stub, head gone. Returns (tris, present?).
fn emit_bolt(g: &mut Geo, at: Vec3, mut b: BoltSpec, dmg: f32) -> (usize, bool) {
    let before = g.ntris();
    let steel = mix3([150.0, 156.0, 166.0], [116.0, 64.0, 40.0], smoothstep(0.15, 0.85, dmg));
    let top = mix3([176.0, 182.0, 192.0], [140.0, 78.0, 48.0], smoothstep(0.15, 0.85, dmg));
    let r = b.dia * 0.5;
    if dmg > 0.72 {
        // sheared: a short rusted stub, no head
        revolve(g, at, &[[r, 0.0], [r * 0.9, b.head_h * 0.28]], 6, b.yaw, b.tilt, mix3(steel, [90.0, 50.0, 32.0], 0.6), top);
        return (g.ntris() - before, false);
    }
    if dmg > 0.4 {
        b.tilt += dmg * 0.6 * if b.yaw > TAU * 0.5 { 1.0 } else { -1.0 };
    }
    let backout = if dmg > 0.4 { dmg * 0.03 } else { 0.0 };
    let at = at + Vec3::new(0.0, 0.0, backout);
    if b.hex {
        revolve(g, at, &[[r, 0.0], [r, b.head_h * 0.72], [r * 0.82, b.head_h]], 6, b.yaw, b.tilt, steel, top);
    } else {
        revolve(g, at, &[[r, 0.0], [r * 0.7, b.head_h * 0.6], [0.0, b.head_h]], 8, b.yaw, b.tilt, steel, top);
    }
    (g.ntris() - before, true)
}

/// Weld bead down the seam; where the local field is high the bead cracks (segments drop out).
fn weld_bead(g: &mut Geo, cond: &Cond, ya: f32, yb: f32, w0: f32, h0: f32, rng: &mut Rng) -> usize {
    let before = g.ntris();
    let base = [138.0, 134.0, 130.0];
    let hot = [170.0, 150.0, 138.0];
    let sect: [[f32; 2]; 5] = [[-1.0, 0.0], [-0.55, 0.72], [0.0, 1.0], [0.55, 0.72], [1.0, 0.0]];
    let n = 26;
    let wander_seed = rng.f01() * 10.0;
    let mut prev: Option<Vec<u32>> = None;
    for i in 0..=n {
        let t = i as f32 / n as f32;
        let y = lerp(ya, yb, t);
        let (u, v) = to_uv(0.0, y);
        let dmg = cond.field.sample(u, v);
        // crack: high damage opens gaps in the bead (skip the ring → discontinuity)
        if dmg > 0.62 && ((t * 30.0).sin() * 0.5 + 0.5) < (dmg - 0.5) * 1.8 {
            prev = None;
            continue;
        }
        let dime = 1.0 + 0.22 * (t * 24.0).sin().abs();
        let wander = ((t * 9.0 + wander_seed).sin() + (t * 23.0).sin() * 0.4) * 0.006;
        let center = Vec3::new(wander, y, 0.0);
        let (w, h) = (w0 * (0.85 + 0.25 * dime.fract()) * (1.0 - dmg * 0.3), h0 * dime * (1.0 - dmg * 0.4));
        let col = mix3(base, [110.0, 60.0, 38.0], smoothstep(0.2, 0.9, dmg));
        let ring: Vec<u32> = sect.iter().map(|s| g.vert(center + Vec3::new(s[0] * w, 0.0, s[1] * h))).collect();
        if let Some(p) = &prev {
            for s in 0..sect.len() - 1 {
                let crown = s == 1 || s == 2;
                let c = if crown { mix3(hot, col, smoothstep(0.2, 0.9, dmg)) } else { col };
                g.quad(p[s], p[s + 1], ring[s + 1], ring[s], c);
            }
        }
        prev = Some(ring);
    }
    g.ntris() - before
}

struct BuildStats {
    total: usize,
    plate: usize,
    weld: usize,
    bolts: usize,
    present: usize,
    lost: usize,
    bolt_pts: Vec<(f32, f32, f32, bool)>, // x, y, dmg, present
}
fn build(seed: u64, cond: &Cond) -> (Geo, BuildStats) {
    let mut g = Geo::new();
    let mut rng = Rng::new(seed);
    // grade → static build strength
    let pitch = lerp(0.42, 0.20, cond.grade);
    let dia_mu = lerp(0.040, 0.060, cond.grade);
    let weld_w = lerp(0.018, 0.032, cond.grade);
    let weld_h = lerp(0.020, 0.034, cond.grade);
    let thick = lerp(0.02, 0.08, cond.grade);
    let steel = [120.0, 127.0, 139.0];
    let rust = [118.0, 66.0, 42.0];

    // ── plate: two subdivided grids (left/right of the seam) that crater + rust from the local field ──
    let plate0 = g.ntris();
    for (x0, x1) in [(PX.0, -0.02), (0.02, PX.1)] {
        let (nx, ny) = (7usize, 12usize);
        let mut idx = vec![0u32; (nx + 1) * (ny + 1)];
        for j in 0..=ny {
            for i in 0..=nx {
                let x = lerp(x0, x1, i as f32 / nx as f32);
                let y = lerp(PY.0, PY.1, j as f32 / ny as f32);
                let (u, v) = to_uv(x, y);
                let dmg = cond.field.sample(u, v);
                let z = -(dmg * dmg) * 0.13; // crater inward
                idx[j * (nx + 1) + i] = g.vert(Vec3::new(x, y, z));
            }
        }
        for j in 0..ny {
            for i in 0..nx {
                let x = lerp(x0, x1, (i as f32 + 0.5) / nx as f32);
                let y = lerp(PY.0, PY.1, (j as f32 + 0.5) / ny as f32);
                let (u, v) = to_uv(x, y);
                let dmg = cond.field.sample(u, v);
                let ao = 1.0 - (dmg * dmg) * 0.4;
                let col = {
                    let c = mix3(steel, rust, smoothstep(0.12, 0.8, dmg));
                    [c[0] * ao, c[1] * ao, c[2] * ao]
                };
                let a = idx[j * (nx + 1) + i];
                let b = idx[j * (nx + 1) + i + 1];
                let c = idx[(j + 1) * (nx + 1) + i + 1];
                let d = idx[(j + 1) * (nx + 1) + i];
                g.quad(a, b, c, d, col);
            }
        }
        // thickness rim along the outer vertical edge (edge-on read of a thick vs thin plate)
        let outer = if x0 < 0.0 { x0 } else { x1 };
        let rb = g.vert(Vec3::new(outer, PY.0, 0.0));
        let rt = g.vert(Vec3::new(outer, PY.1, 0.0));
        let rb2 = g.vert(Vec3::new(outer, PY.0, -thick));
        let rt2 = g.vert(Vec3::new(outer, PY.1, -thick));
        g.quad(rb, rt, rt2, rb2, mix3(steel, [70.0, 74.0, 82.0], 0.5));
    }
    let plate = g.ntris() - plate0;

    // ── weld down the seam ──
    let weld = weld_bead(&mut g, cond, PY.0 + 0.03, PY.1 - 0.03, weld_w, weld_h, &mut rng);

    // ── bolt rows, pitch/size from grade, condition from the local field ──
    let mut present = 0;
    let mut lost = 0;
    let mut bolts = 0;
    let mut bolt_pts = vec![];
    for &col_x in &[-0.42f32, 0.42] {
        let mut y = PY.0 + 0.14;
        while y < PY.1 - 0.1 {
            let spec = draw_bolt(&mut rng, dia_mu);
            let jx = rng.normal(0.0, 0.02);
            let jy = rng.normal(0.0, 0.03);
            let (bx, by) = (col_x + jx, y + jy);
            let (u, v) = to_uv(bx, by);
            let dmg = cond.field.sample(u, v);
            let (n, ok) = emit_bolt(&mut g, Vec3::new(bx, by, 0.0), spec, dmg);
            bolts += n;
            if ok {
                present += 1;
            } else {
                lost += 1;
            }
            bolt_pts.push((bx, by, dmg, ok));
            y += pitch * (1.0 + rng.normal(0.0, 0.12));
        }
    }
    let total = g.ntris();
    (g, BuildStats { total, plate, weld, bolts, present, lost, bolt_pts })
}

// ── z-buffered flat-diffuse rasteriser ──────────────────────────────────────────────────────────────────
fn render_view(g: &Geo, buf: &mut [u8], w: usize, ox: usize, oy: usize, cam: Vec3, target: Vec3) {
    let fwd = (target - cam).normalized();
    let right = fwd.cross(Vec3::new(0.0, 1.0, 0.0)).normalized();
    let up = right.cross(fwd);
    let focal = VIEW as f32 * 0.5 / (40f32.to_radians() * 0.5).tan();
    let light = Vec3::new(-0.5, 0.62, 0.6).normalized();
    let mut zb = vec![f32::INFINITY; VIEW * VIEW];
    let proj = |p: Vec3| {
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
        if n.dot((cam - p[0]).normalized()) < 0.0 {
            n = n * -1.0;
        }
        let shade = 0.32 + 0.78 * n.dot(light).max(0.0);
        let col = g.c[ti];
        let minx = s.iter().map(|v| v.0).fold(f32::MAX, f32::min).floor().max(0.0) as usize;
        let maxx = s.iter().map(|v| v.0).fold(f32::MIN, f32::max).ceil().min(VIEW as f32 - 1.0) as usize;
        let miny = s.iter().map(|v| v.1).fold(f32::MAX, f32::min).floor().max(0.0) as usize;
        let maxy = s.iter().map(|v| v.1).fold(f32::MIN, f32::max).ceil().min(VIEW as f32 - 1.0) as usize;
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
                let i = ((oy + py) * w + (ox + px)) * 3;
                buf[i] = (col[0] * shade).clamp(0.0, 255.0) as u8;
                buf[i + 1] = (col[1] * shade).clamp(0.0, 255.0) as u8;
                buf[i + 2] = (col[2] * shade).clamp(0.0, 255.0) as u8;
            }
        }
    }
}

fn main() {
    let w = VIEW * 2;
    let mut buf = vec![0u8; w * (VIEW * 2) * 3];
    for py in 0..VIEW * 2 {
        let t = py as f32 / (VIEW * 2) as f32;
        for px in 0..w {
            let i = (py * w + px) * 3;
            buf[i] = (16.0 + t * 12.0) as u8;
            buf[i + 1] = (18.0 + t * 13.0) as u8;
            buf[i + 2] = (24.0 + t * 16.0) as u8;
        }
    }
    let q = GraphicsQuality::Low; // CPU path (High would pick the GPU stub → still CPU today)
    let hit = [(0.66, 0.66, 0.17, 1.15)]; // one strong local impact, upper-right
    let cams = |c: Vec3| c;
    let cam = cams(Vec3::new(1.25, 0.9, 1.75));
    let tgt = Vec3::new(0.0, 0.0, 0.02);

    let strong = make_cond(0.92, 1.0, &[], q);
    let weak = make_cond(0.22, 1.0, &[], q);
    let local = make_cond(0.92, 1.0, &hit, q);
    let worn = make_cond(0.92, 0.32, &hit, q);
    render_view(&build(1, &strong).0, &mut buf, w, 0, 0, cam, tgt); // TL strong pristine
    render_view(&build(1, &weak).0, &mut buf, w, VIEW, 0, cam, tgt); // TR weak build
    render_view(&build(1, &local).0, &mut buf, w, 0, VIEW, cam, tgt); // BL local hit only
    render_view(&build(1, &worn).0, &mut buf, w, VIEW, VIEW, cam, tgt); // BR global floor + local
    image::save_buffer("seam_condition.png", &buf, w as u32, (VIEW * 2) as u32, image::ColorType::Rgb8).unwrap();
    println!("wrote seam_condition.png  [ strong | weak ; local-hit | worn+hit ]  (oblique)");

    // ── report ──
    println!("\n  GPU SEAM (unwired)");
    for qq in [GraphicsQuality::Low, GraphicsQuality::High] {
        println!("    quality {:?} → diffusion backend '{}'", qq, backend_for(qq).label());
    }
    // parity: CPU vs GPU-stub produce identical fields → the future port is drop-in
    let cpu = make_cond(0.9, 0.5, &hit, GraphicsQuality::Low);
    let gpu = make_cond(0.9, 0.5, &hit, GraphicsQuality::High);
    let max_diff = cpu.field.a.iter().zip(&gpu.field.a).map(|(a, b)| (a - b).abs()).fold(0.0f32, f32::max);
    println!("    CPU vs GPU-stub field max diff: {max_diff:.6}  (0 → identical; port swaps backend only)");

    let (_, s) = build(1, &worn);
    println!("\n  TRI BUDGET (damageable wall LOD)");
    println!("    plate grid : {:>4}   weld : {:>4}   bolts : {:>4}   TOTAL : {}", s.plate, s.weld, s.bolts, s.total);
    println!("    (a pristine flat panel needs ~4; the subdivided grid is only spent where a surface is live)");

    // ── locality + diffusion: field vs distance from the impact centre ──
    println!("\n  LOCALITY / DIFFUSION  (integrity 1.0, one hit at uv(0.66,0.66))");
    println!("    dist-from-hit   mean field   bolts present");
    let f = &local.field;
    for band in [(0.0, 0.12), (0.12, 0.25), (0.25, 0.4), (0.4, 0.6), (0.6, 1.2)] {
        let (mut sum, mut cnt) = (0.0, 0);
        for j in 0..48 {
            for i in 0..48 {
                let (u, v) = ((i as f32 + 0.5) / 48.0, (j as f32 + 0.5) / 48.0);
                let d = ((u - 0.66).powi(2) + (v - 0.66).powi(2)).sqrt();
                if d >= band.0 && d < band.1 {
                    sum += f.a[j * 48 + i];
                    cnt += 1;
                }
            }
        }
        // bolts in this band
        let (mut bp, mut bt) = (0, 0);
        for &(bx, by, _, ok) in &build(1, &local).1.bolt_pts {
            let (u, v) = to_uv(bx, by);
            let d = ((u - 0.66).powi(2) + (v - 0.66).powi(2)).sqrt();
            if d >= band.0 && d < band.1 {
                bt += 1;
                if ok {
                    bp += 1;
                }
            }
        }
        println!("    {:.2}–{:.2}          {:.3}         {}/{}", band.0, band.1, sum / cnt.max(1) as f32, bp, bt);
    }

    // ── monotonic in integrity: worse global → more loss, but far-from-hit stays cleaner than near ──
    println!("\n  MONOTONIC IN INTEGRITY  (same hit)");
    println!("    integrity   bolts lost / total   mean field");
    for integ in [1.0, 0.6, 0.32] {
        let c = make_cond(0.92, integ, &hit, q);
        let (_, st) = build(1, &c);
        let mean = c.field.a.iter().sum::<f32>() / c.field.a.len() as f32;
        println!("    {integ:.2}        {} / {}                {:.3}", st.lost, st.present + st.lost, mean);
    }
}

#[cfg(test)]
mod checks {
    use super::*;
    #[test]
    fn condition_model_is_well_behaved() {
        let hit = [(0.66, 0.66, 0.17, 1.15)];
        let q = GraphicsQuality::Low;
        // locality: with integrity 1.0 + one hit, bolts far from the hit survive; near ones can shear
        let local = make_cond(0.92, 1.0, &hit, q);
        let (_, s) = build(1, &local);
        let far_present = s.bolt_pts.iter().filter(|(bx, by, _, _)| {
            let (u, v) = to_uv(*bx, *by);
            ((u - 0.66).powi(2) + (v - 0.66).powi(2)).sqrt() > 0.5
        }).all(|(_, _, _, ok)| *ok);
        assert!(far_present, "bolts far from a local hit should not shear");
        // monotonic: lower integrity → not fewer bolts lost
        let a = build(1, &make_cond(0.92, 1.0, &hit, q)).1.lost;
        let b = build(1, &make_cond(0.92, 0.5, &hit, q)).1.lost;
        let c = build(1, &make_cond(0.92, 0.2, &hit, q)).1.lost;
        assert!(a <= b && b <= c, "global damage should be monotonic: {a} {b} {c}");
        // GPU-stub parity with CPU
        let cpu = make_cond(0.9, 0.5, &hit, GraphicsQuality::Low);
        let gpu = make_cond(0.9, 0.5, &hit, GraphicsQuality::High);
        assert!(cpu.field.a.iter().zip(&gpu.field.a).all(|(x, y)| (x - y).abs() < 1e-6));
    }
}
