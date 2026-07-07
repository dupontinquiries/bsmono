//! armor_wear — the PERSISTENT armor-weathering accumulator + a dwell-over-time render. The field lives on
//! the player's loadout (would serialise into db.rs) and integrates environmental exposure over time. Two
//! channels, and which one a process writes to is the whole realism:
//!   • PERMANENT  (ratchets up only): sun bleach, sandblast abrasion, slow oxide — the travelogue.
//!   • TRANSIENT  (rises in a biome, decays out): surface wet, fresh dust.
//!   • BATTLE     (separate, repairable): combat scarring — the only thing that costs protection.
//! repair() clears battle + transient but KEEPS the permanent patina; reforge() is a full reset. Change is
//! rate-limited so it crawls (minutes of dwell), never snaps. Run: cargo run --example armor_wear
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

// ── the biome the wearer is currently standing in ──
#[derive(Clone, Copy)]
struct Biome {
    sun: f32,
    humidity: f32,
    sandblast: f32,
    salt: f32,
}
const DESERT: Biome = Biome { sun: 0.95, humidity: 0.08, sandblast: 0.9, salt: 0.0 };
const JUNGLE: Biome = Biome { sun: 0.4, humidity: 0.95, sandblast: 0.1, salt: 0.15 };

// ── the persistent field (serialises into the loadout record) ──
#[derive(Clone, Copy, Default)]
struct ArmorWear {
    // permanent — ratchet up only
    bleach: f32,
    abrasion: f32,
    oxide: f32,
    // transient — rise in biome, decay out
    wet: f32,
    dust: f32,
    // repairable
    battle: f32,
}
const MAX_STEP: f32 = 0.04; // rate limit: no single tick can jump a permanent channel more than this

impl ArmorWear {
    /// Integrate a stint of `dt` seconds standing in `b`. Permanent channels ratchet slowly and clamped;
    /// transient channels relax toward the biome's ambient.
    fn integrate(&mut self, b: Biome, dt: f32) {
        let ratchet = |cur: f32, drive: f32, rate: f32| -> f32 {
            (cur + (drive * rate * dt).min(MAX_STEP)).min(1.0)
        };
        self.bleach = ratchet(self.bleach, b.sun, 0.020);
        self.abrasion = ratchet(self.abrasion, b.sandblast, 0.016);
        self.oxide = ratchet(self.oxide, b.humidity * (1.0 + b.salt), 0.008); // slowest
        // transient relax toward ambient (fast enough to feel responsive, still not instant)
        let relax = |cur: f32, target: f32, k: f32| cur + (target - cur) * (k * dt).min(1.0);
        self.wet = relax(self.wet, b.humidity, 0.5);
        self.dust = relax(self.dust, b.sandblast, 0.35);
    }
    fn take_fire(&mut self, amt: f32) {
        self.battle = (self.battle + amt).min(1.0);
    }
    /// Field repair between matches: fixes battle damage and wipes transient grime — but the permanent
    /// travelogue (bleach/abrasion/oxide) stays. Cheap, not performance-gated.
    fn repair(&mut self) {
        self.battle = 0.0;
        self.wet = 0.0;
        self.dust = 0.0;
    }
    /// Reforge: a full reset to issued-clean, including the permanent patina.
    fn reforge(&mut self) {
        *self = ArmorWear::default();
    }
    // ── recommended gameplay couplings (capped, non-snowballing) ──
    /// Only BATTLE damage costs protection, hard-capped so it can't spiral.
    fn protection_loss(&self) -> f32 {
        (self.battle * 0.12).min(0.12)
    }
    /// "More exposed" = more detectable: bright scuffs + wet sheen + faded contrast make you louder to spot.
    fn detectability(&self) -> f32 {
        clamp01(self.bleach * 0.45 + self.abrasion * 0.4 + self.wet * 0.35 + self.battle * 0.3)
    }
}

// ── curved plate ────────────────────────────────────────────────────────────────────────────────────────
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

/// Polished gunmetal driven by the accumulator. Masks come from uv so they line up with the plate.
fn worn_pixel(wr: ArmorWear, u: f32, v: f32) -> ([f32; 3], f32) {
    let edge = 1.0 - smoothstep(0.0, 0.10, u.min(1.0 - u).min(v).min(1.0 - v));
    let rib = smoothstep(0.11, 0.0, (u - 0.5).abs());
    let crest = clamp01(rib + smoothstep(0.5, 1.0, v) * 0.5); // up-facing / sun-exposed
    let recess = clamp01(edge + (1.0 - v) * 0.3);
    let fine = fbm(u * 22.0, v * 22.0, 9);

    let mut col = [96.0, 100.0, 110.0]; // gunmetal
    let mut gloss = 0.72;

    // baseline service scuff at edges (use), independent of environment
    let scuff = clamp01(edge * 0.5 * (0.7 + fine * 0.6));
    col = mix3(col, POLISH, scuff);
    gloss = lerp(gloss, 0.9, scuff);

    // PERMANENT: sun bleach — fade/lighten exposed areas, chalk the gloss
    let bl = wr.bleach * (0.4 + 0.6 * crest);
    let l = (col[0] + col[1] + col[2]) / 3.0;
    col = mix3(col, [lerp(col[0], l, 0.6) + 22.0, lerp(col[1], l, 0.6) + 22.0, lerp(col[2], l, 0.6) + 20.0], bl);
    gloss = lerp(gloss, 0.28, bl * 0.7);

    // PERMANENT: abrasion — scoured matte bare metal on exposed faces
    let ab = wr.abrasion * (0.35 + 0.65 * crest);
    col = mix3(col, [150.0, 154.0, 160.0], ab * 0.5);
    gloss = lerp(gloss, 0.2, ab);

    // PERMANENT: oxide — slow rust creeping in the recesses/seams
    let ox = wr.oxide * (0.3 + recess * 0.9) * (0.5 + fine * 0.8);
    col = mix3(col, [116.0, 66.0, 44.0], clamp01(ox));
    if ox > 0.15 {
        gloss = lerp(gloss, 0.12, clamp01(ox));
    }

    // TRANSIENT: wet — darkens + raises gloss (sheen)
    let wet = wr.wet * (0.5 + recess * 0.5);
    col = [col[0] * (1.0 - wet * 0.3), col[1] * (1.0 - wet * 0.3), col[2] * (1.0 - wet * 0.28)];
    gloss = lerp(gloss, 0.95, wet * 0.6);

    // TRANSIENT: dust — tan film settling in recesses, mattes
    let dust = wr.dust * (recess * 0.8 + 0.12);
    col = mix3(col, [190.0, 172.0, 132.0], dust * 0.5);
    gloss = lerp(gloss, 0.25, dust * 0.6);

    // BATTLE: scorch + torn-metal gouges
    if wr.battle > 0.0 {
        for (cx, cy) in [(0.6, 0.42), (0.36, 0.64)] {
            let d = ((u - cx).powi(2) + (v - cy).powi(2)).sqrt();
            col = mix3(col, [26.0, 24.0, 26.0], smoothstep(0.16, 0.06, d) * wr.battle * 0.6);
            col = mix3(col, POLISH, smoothstep(0.08, 0.0, d) * wr.battle * 0.85);
        }
    }
    (col, gloss)
}

fn shade_cell(wr: ArmorWear, verts: &[Surf], tris: &[[u32; 3]], buf: &mut [u8], w: usize, ox: usize) {
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
                let (alb, gloss) = worn_pixel(wr, uu, vy);
                let diff = n.dot(light).max(0.0);
                let hlf = (light + Vec3::new(0.0, 0.0, 1.0)).normalized();
                let spec = n.dot(hlf).max(0.0).powf(6.0 + gloss * 90.0) * gloss;
                let shade = 0.34 + 0.72 * diff;
                let i = ((py) * w + (ox + px)) * 3;
                buf[i] = (alb[0] * shade + 235.0 * spec).clamp(0.0, 255.0) as u8;
                buf[i + 1] = (alb[1] * shade + 238.0 * spec).clamp(0.0, 255.0) as u8;
                buf[i + 2] = (alb[2] * shade + 245.0 * spec).clamp(0.0, 255.0) as u8;
            }
        }
    }
}

fn main() {
    let (verts, tris) = plate();
    // a route: issued → a desert leg → a jungle leg → a firefight → field-repaired → reforged
    let mut wr = ArmorWear::default();
    let mut frames = vec![("issued", wr)];
    for _ in 0..90 {
        wr.integrate(DESERT, 1.0);
    }
    frames.push(("desert leg", wr));
    for _ in 0..90 {
        wr.integrate(JUNGLE, 1.0);
    }
    frames.push(("jungle leg", wr));
    wr.take_fire(0.8);
    frames.push(("firefight", wr));
    wr.repair();
    frames.push(("repaired", wr));
    wr.reforge();
    frames.push(("reforged", wr));

    let w = CELL * frames.len();
    let mut buf = vec![18u8; w * CELL * 3];
    for (k, (_n, f)) in frames.iter().enumerate() {
        shade_cell(*f, &verts, &tris, &mut buf, w, k * CELL);
    }
    image::save_buffer("armor_wear.png", &buf, w as u32, CELL as u32, image::ColorType::Rgb8).unwrap();
    println!("wrote armor_wear.png  ({}x{CELL})  {}", w, frames.iter().map(|(n, _)| *n).collect::<Vec<_>>().join(" → "));

    println!("\n  stage       bleach abras oxide  wet  dust battle | prot-loss detect");
    for (n, f) in &frames {
        println!(
            "  {n:<11} {:.2}  {:.2}  {:.2}  {:.2} {:.2}  {:.2}  |   {:.0}%     {:.2}",
            f.bleach, f.abrasion, f.oxide, f.wet, f.dust, f.battle, f.protection_loss() * 100.0, f.detectability()
        );
    }
}

#[cfg(test)]
mod checks {
    use super::*;
    #[test]
    fn accumulator_is_well_behaved() {
        let mut w = ArmorWear::default();
        // permanent ratchets up in the desert
        for _ in 0..60 {
            w.integrate(DESERT, 1.0);
        }
        let (b1, a1) = (w.bleach, w.abrasion);
        assert!(b1 > 0.0 && a1 > 0.0, "desert should bleach + abrade");
        let dust_desert = w.dust;
        // move to jungle: bleach must NOT decrease (permanent), wet rises, desert dust decays
        for _ in 0..60 {
            w.integrate(JUNGLE, 1.0);
        }
        assert!(w.bleach >= b1 - 1e-6, "bleach is permanent, must not fall");
        assert!(w.wet > 0.5, "jungle should wet the armor");
        assert!(w.dust < dust_desert, "dust should decay leaving the desert");
        // rate limit: one huge-dt tick can't leap a permanent channel
        let mut r = ArmorWear::default();
        r.integrate(DESERT, 1000.0);
        assert!(r.bleach <= MAX_STEP + 1e-6, "rate-limited: {}", r.bleach);
        // repair keeps the travelogue, clears battle + transient
        w.take_fire(0.9);
        let keep = w.bleach;
        w.repair();
        assert!(w.battle == 0.0 && w.wet == 0.0 && (w.bleach - keep).abs() < 1e-6, "repair keeps patina, clears damage");
        // reforge wipes everything
        w.reforge();
        assert!(w.bleach == 0.0 && w.oxide == 0.0 && w.abrasion == 0.0);
        // protection loss only from battle, capped
        let mut p = ArmorWear::default();
        p.bleach = 1.0;
        p.abrasion = 1.0;
        assert_eq!(p.protection_loss(), 0.0, "weathering must not cost protection");
        p.take_fire(1.0);
        assert!(p.protection_loss() <= 0.12 + 1e-6, "protection loss capped");
    }
}
