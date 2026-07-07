//! fsm.rs — **intent-driven finite state machines** that grow a bare primitive into a contextual map
//! feature. Each primitive is treated as an agent: it reads its [`Site`] (the spot's environment — sightline,
//! traffic, slope, high-ground, wetness…), decides whether it should exist, and materialises by choosing form
//! + material from context (via the [`crate::blockout`] dressing rules).
//!
//! The design is abstracted so the machines share machinery and the map-builder can run them uniformly:
//!   * [`Site`] — one environment query, computed once by the level and handed to every FSM.
//!   * [`Fsm`] — the common trait: `grow(site) -> Option<Dressing>` (geometry + collision, or "not here").
//!   * [`detail`] — reusable square-foot generators (debris, railings, rebar, sandbags, shards, smoke) that
//!     every machine composes, so a new POI is mostly "macro shape + a couple of detail calls".

use crate::blockout::{Block, Context, Dressing, Role};
use crate::level::{Obstacle, Palette};
use crate::math::Vec3;
use crate::mesh::{boxx, disc, hash01, lerp_c, rgb, set_alpha, tint_height, tube, uv_sphere, Mesh3, FACE_ALL, FACE_NO_BOTTOM};

/// Everything a machine needs to know about the spot it's growing in — computed once by the level and shared.
#[derive(Clone, Copy)]
pub struct Site {
    pub x: f32,
    pub z: f32,
    pub ground_y: f32,
    /// Longest clear sightline (world units) passing through here.
    pub sightline: f32,
    /// Bearing (radians) of the most dangerous open sightline — railings leave their gap facing this.
    pub danger_yaw: f32,
    /// A high-traffic crossing where cover matters most.
    pub high_traffic: bool,
    /// This is the local elevation maximum (earns a vantage).
    pub is_high_point: bool,
    /// Terrain steepness 0..1 here (slopes face kill-zones for bunkers).
    pub slope: f32,
    /// Near water/mud (forbids clean metal; favours rot/rust).
    pub wet: bool,
    /// Indoors / enclosed (favours built materials).
    pub indoor: bool,
    pub pal: Palette,
    pub seed: u32,
}

impl Site {
    /// The blockout dressing context for this site (material binding shared with the blockout system).
    pub fn context(&self) -> Context {
        Context { pal: self.pal, wet: self.wet, indoor: self.indoor, ground_y: self.ground_y }
    }
    fn rand(&self, salt: i32) -> f32 {
        hash01(self.seed as i32, salt)
    }
}

/// The common machine interface: read the site, decide, emit geometry + collision (or nothing).
pub trait Fsm {
    fn grow(&self, site: &Site) -> Option<Dressing>;
}

// ─────────────────────────────────────────────────────────────────────────────────────────────────────────
// detail — reusable "square-foot" generators every machine composes.
// ─────────────────────────────────────────────────────────────────────────────────────────────────────────
pub mod detail {
    use super::*;

    /// A localised pile of micro-cube rubble at a base. Breaches, wreck debris, crater spoil.
    pub fn debris(center: Vec3, radius: f32, count: u32, color: egui::Color32, seed: u32) -> Mesh3 {
        let mut m = Mesh3::new(color);
        for k in 0..count {
            let a = hash01(k as i32, seed as i32) * std::f32::consts::TAU;
            let d = radius * hash01(k as i32, seed as i32 ^ 0x9E) * (0.5 + hash01(k as i32, 7) * 0.5);
            let s = 0.07 + hash01(k as i32, 11) * 0.14;
            let p = Vec3::new(center.x + a.cos() * d, center.y + s, center.z + a.sin() * d);
            m.merge(&boxx(p, Vec3::new(s, s, s), lerp_c(color, rgb(0, 0, 0), hash01(k as i32, 3) * 0.3), FACE_ALL));
        }
        m
    }

    /// Posts around a rim with a deliberate gap facing `gap_yaw` (the danger angle). Vantage railings.
    pub fn railing(center: Vec3, r: f32, top: f32, gap_yaw: f32, color: egui::Color32) -> Mesh3 {
        let mut m = Mesh3::new(color);
        let posts = 12;
        for k in 0..posts {
            let a = k as f32 / posts as f32 * std::f32::consts::TAU;
            let mut da = (a - gap_yaw).rem_euclid(std::f32::consts::TAU);
            if da > std::f32::consts::PI {
                da = std::f32::consts::TAU - da;
            }
            if da < 0.95 {
                continue; // the open gap
            }
            let p = Vec3::new(center.x + a.cos() * r, top, center.z + a.sin() * r);
            m.merge(&tube(p, Vec3::new(p.x, p.y + 0.95, p.z), 0.06, 4, color));
        }
        m
    }

    /// A staggered double row of sagging sandbags along an edge (bunker parapet, barrier).
    pub fn sandbags(start: Vec3, end: Vec3, color: egui::Color32, seed: u32) -> Mesh3 {
        let mut m = Mesh3::new(color);
        let dir = end - start;
        let len = dir.len().max(0.001);
        let steps = (len / 0.45).max(1.0) as i32;
        let step = dir * (1.0 / steps as f32);
        for row in 0..2 {
            for i in 0..steps {
                let off = if row == 1 { 0.22 } else { 0.0 };
                let p = start + step * (i as f32 + off) + Vec3::new(0.0, 0.16 + row as f32 * 0.3, 0.0);
                let jig = 0.9 + hash01(i ^ (row * 97), seed as i32) * 0.2;
                m.merge(&boxx(p, Vec3::new(0.24 * jig, 0.16, 0.18 * jig), lerp_c(color, rgb(0, 0, 0), 0.06 * row as f32), FACE_ALL));
            }
        }
        m
    }

    /// Twisted rebar bristling out of a breach, aligned along `dir`.
    pub fn rebar(center: Vec3, dir: Vec3, count: u32, seed: u32) -> Mesh3 {
        let mut m = Mesh3::new(rgb(74, 62, 52));
        let perp = Vec3::new(-dir.z, 0.0, dir.x);
        for k in 0..count {
            let o = (k as f32 - count as f32 * 0.5) * 0.16;
            let base = center + perp * o;
            let tip = base + dir * (0.4 + hash01(k as i32, seed as i32) * 0.4) + Vec3::new(0.0, 0.2, 0.0);
            m.merge(&tube(base, tip, 0.025, 3, rgb(74, 62, 52)));
        }
        m
    }

    /// A dark oil/water slick decal under a wreck or leak.
    pub fn slick(center: Vec3, r: f32) -> Mesh3 {
        disc(Vec3::new(center.x, center.y + 0.03, center.z), r, 12, rgb(22, 20, 24))
    }

    /// A cone of jagged shattered-glass shards thrown forward from `origin` along `dir`.
    pub fn shards(origin: Vec3, dir: Vec3, count: u32, seed: u32) -> Mesh3 {
        let mut m = Mesh3::new(rgb(160, 180, 190));
        let perp = Vec3::new(-dir.z, 0.0, dir.x);
        for k in 0..count {
            let spread = (hash01(k as i32, seed as i32) - 0.5) * 1.2;
            let d = 0.5 + hash01(k as i32, 5) * 2.5;
            let p = origin + dir * d + perp * (spread * d);
            m.merge(&boxx(Vec3::new(p.x, origin.y + 0.05, p.z), Vec3::new(0.07, 0.07, 0.07), rgb(160, 180, 190), FACE_ALL));
        }
        set_alpha(&mut m, 150);
        m
    }

    /// Translucent fire/smoke puffs rising from a point (engine fire, burning wreck).
    pub fn smoke(base: Vec3, seed: u32) -> Mesh3 {
        let mut m = Mesh3::new(rgb(60, 56, 54));
        for k in 0..5 {
            let h = k as f32 * 0.55;
            let col = if k < 2 { rgb(220, 110, 40) } else { rgb(60, 56, 54) };
            let mut puff = uv_sphere(Vec3::new(base.x + (hash01(k, seed as i32) - 0.5) * 0.6, base.y + 0.4 + h, base.z), 0.4 + h * 0.22, 6, 4, col);
            set_alpha(&mut puff, if k < 2 { 150 } else { 80 });
            m.merge(&puff);
        }
        m
    }

    /// Apply a bottom-dark rust/grime gradient to a mesh (tanks, hulls, parapets).
    pub fn weather(m: &mut Mesh3, lo: f32, hi: f32) {
        let c = m.color;
        tint_height(m, lo, hi, lerp_c(c, rgb(0, 0, 0), 0.45), lerp_c(c, rgb(255, 255, 255), 0.06));
    }

    /// A run of ganged pipes along an axis, with periodic support brackets and occasional elbow branches.
    /// The pipe count, radii, and where branches split are all hash-driven so every run is different but
    /// deterministic. Wall-hugging industrial detail — the kind of greeble that fills dead wall space.
    pub fn pipes_run(start: Vec3, end: Vec3, count: u32, seed: u32) -> Mesh3 {
        let base = rgb(96, 92, 84);
        let mut m = Mesh3::new(base);
        let dir = end - start;
        let len = dir.len().max(0.001);
        let axis = dir * (1.0 / len);
        // perpendicular in the horizontal plane, plus vertical, to stack the pipes into a bundle
        let perp = Vec3::new(-axis.z, 0.0, axis.x);
        for k in 0..count {
            let lane = (k as f32 - (count as f32 - 1.0) * 0.5) * 0.24;
            let voff = (hash01(k as i32, seed as i32) - 0.5) * 0.2;
            let r = 0.06 + hash01(k as i32, seed as i32 ^ 0x5A) * 0.06;
            let off = perp * lane + Vec3::new(0.0, voff, 0.0);
            let a = start + off;
            let b = end + off;
            let col = lerp_c(base, rgb(120, 80, 50), hash01(k as i32, 3) * 0.5); // some pipes rustier
            m.merge(&tube(a, b, r, 5, col));
            // a hash-placed elbow branch peeling off toward the wall on some pipes
            if hash01(k as i32, seed as i32 ^ 0xB1) > 0.6 {
                let t = 0.3 + hash01(k as i32, 9) * 0.4;
                let joint = a + (b - a) * t;
                let branch = joint + perp * (0.4 * if lane >= 0.0 { 1.0 } else { -1.0 }) + Vec3::new(0.0, -0.3, 0.0);
                m.merge(&tube(joint, branch, r * 0.85, 4, col));
            }
        }
        // support brackets at intervals along the run
        let brackets = (len / 2.2).max(1.0) as i32;
        for i in 0..=brackets {
            let p = start + dir * (i as f32 / brackets as f32);
            let w = 0.24 * count as f32 * 0.5 + 0.12;
            m.merge(&boxx(Vec3::new(p.x, p.y, p.z), Vec3::new(perp.x.abs() * w + 0.05, 0.05, perp.z.abs() * w + 0.05), rgb(70, 66, 60), FACE_ALL));
        }
        m
    }

    /// A sagging catenary cable between two points, approximated by short tube segments that droop under a
    /// `sag` amount. Power lines between rooftops, zip rigging, tarp guy-lines. Segment count scales with span.
    pub fn cable_sag(a: Vec3, b: Vec3, sag: f32, color: egui::Color32) -> Mesh3 {
        let mut m = Mesh3::new(color);
        let span = (b - a).len().max(0.001);
        let segs = (span / 0.9).clamp(4.0, 24.0) as i32;
        let mut prev = a;
        for i in 1..=segs {
            let t = i as f32 / segs as f32;
            // parabolic droop: 4*sag*t*(1-t) is 0 at the ends, max at the middle
            let droop = 4.0 * sag * t * (1.0 - t);
            let p = a + (b - a) * t - Vec3::new(0.0, droop, 0.0);
            m.merge(&tube(prev, p, 0.03, 3, color));
            prev = p;
        }
        m
    }

    /// A louvered vent grille set into a face: a recessed dark frame crossed by angled slats. `dir` is the
    /// outward normal; `up`/`right` span the grille face. Cheap, reads instantly as "ventilation".
    pub fn vent_grille(center: Vec3, right: Vec3, up: Vec3, w: f32, h: f32, seed: u32) -> Mesh3 {
        let frame = rgb(60, 62, 66);
        let mut m = Mesh3::new(frame);
        // recessed backing plate
        m.merge(&boxx(center, Vec3::new(right.x.abs() * w + up.x.abs() * 0.02 + 0.04, right.y.abs() * w + up.y.abs() * h + 0.04, right.z.abs() * w + up.z.abs() * h + 0.04), rgb(28, 30, 32), FACE_ALL));
        let slats = (h / 0.12).clamp(3.0, 12.0) as i32;
        for i in 0..slats {
            let t = (i as f32 + 0.5) / slats as f32 - 0.5;
            let c = center + up * (t * h * 2.0);
            let tone = lerp_c(frame, rgb(90, 92, 96), hash01(i, seed as i32) * 0.5);
            // each slat a thin angled box
            m.merge(&boxx(c, Vec3::new(right.x.abs() * w + 0.02, 0.03, right.z.abs() * w + 0.02), tone, FACE_ALL));
        }
        m
    }

    /// Hash-scattered moss/lichen patches creeping over a surface — a cluster of small flat discs in an
    /// organic green, denser toward `center`. Ages stone and wood without touching the base material.
    pub fn moss_creep(center: Vec3, radius: f32, count: u32, seed: u32) -> Mesh3 {
        let mut m = Mesh3::new(rgb(78, 104, 58));
        for k in 0..count {
            let a = hash01(k as i32, seed as i32) * std::f32::consts::TAU;
            // bias toward the centre: square the radial hash so more patches cluster near the middle
            let rh = hash01(k as i32, seed as i32 ^ 0x2C);
            let d = radius * rh * rh;
            let s = 0.08 + hash01(k as i32, 13) * 0.16;
            let p = Vec3::new(center.x + a.cos() * d, center.y + 0.02, center.z + a.sin() * d);
            let col = lerp_c(rgb(78, 104, 58), rgb(52, 78, 40), hash01(k as i32, 17));
            m.merge(&disc(p, s, 6, col));
        }
        m
    }

    /// An FSM-driven crack network radiating from an impact/stress origin: a small number of primary
    /// fractures branch stochastically into finer ones, each segment a thin dark quad on the ground. Perfect
    /// for earthquake-struck ground, blast marks, and stressed concrete. The branching is a tiny state
    /// machine — each active crack tip either continues, forks, or terminates based on a hash of its id.
    pub fn crack_network(origin: Vec3, reach: f32, primaries: u32, seed: u32) -> Mesh3 {
        let mut m = Mesh3::new(rgb(24, 22, 22));
        // each crack tip: (position, direction, remaining length, id)
        struct Tip {
            p: Vec3,
            dir: Vec3,
            left: f32,
            id: i32,
        }
        let mut tips: Vec<Tip> = Vec::new();
        for k in 0..primaries {
            let a = (k as f32 / primaries as f32) * std::f32::consts::TAU + hash01(k as i32, seed as i32) * 0.6;
            tips.push(Tip {
                p: origin,
                dir: Vec3::new(a.cos(), 0.0, a.sin()),
                left: reach * (0.6 + hash01(k as i32, seed as i32 ^ 0x7) * 0.4),
                id: k as i32 * 131 + 1,
            });
        }
        let mut guard = 0; // cap total work so a pathological seed can't run away
        while let Some(mut t) = tips.pop() {
            guard += 1;
            if guard > 400 {
                break;
            }
            // advance this tip in short steps, wandering, until its length runs out
            let step = 0.4;
            while t.left > 0.0 {
                let h = hash01(t.id, seed as i32 ^ (t.left * 10.0) as i32 ^ 0x33);
                // wander the direction a little each step
                let turn = (h - 0.5) * 0.7;
                let (s, c) = turn.sin_cos();
                let nd = Vec3::new(t.dir.x * c - t.dir.z * s, 0.0, t.dir.x * s + t.dir.z * c);
                t.dir = nd;
                let np = t.p + t.dir * step;
                // draw the segment as a thin flat quad on the ground
                let perp = Vec3::new(-t.dir.z, 0.0, t.dir.x) * (0.015 + h * 0.02);
                let mut seg = Mesh3::new(rgb(24, 22, 22));
                let y = origin.y + 0.02;
                let a0 = Vec3::new(t.p.x + perp.x, y, t.p.z + perp.z);
                let b0 = Vec3::new(t.p.x - perp.x, y, t.p.z - perp.z);
                let c0 = Vec3::new(np.x - perp.x, y, np.z - perp.z);
                let d0 = Vec3::new(np.x + perp.x, y, np.z + perp.z);
                seg.push_quad(a0, b0, c0, d0, Vec3::new(0.0, 1.0, 0.0), rgb(24, 22, 22));
                m.merge(&seg);
                t.p = np;
                t.left -= step;
                // FSM transition: occasionally fork a child crack off the tip (only if there's length left)
                if t.left > 0.6 && hash01(t.id ^ (t.left * 7.0) as i32, seed as i32 ^ 0x9B) > 0.86 && tips.len() < 24 {
                    let fork_a: f32 = if h > 0.5 { 0.8 } else { -0.8 };
                    let (fs, fc) = fork_a.sin_cos();
                    let fd = Vec3::new(t.dir.x * fc - t.dir.z * fs, 0.0, t.dir.x * fs + t.dir.z * fc);
                    tips.push(Tip {
                        p: t.p,
                        dir: fd,
                        left: t.left * 0.6,
                        id: t.id * 17 + 3,
                    });
                }
            }
        }
        m
    }
}

// ─────────────────────────────────────────────────────────────────────────────────────────────────────────
// Feature FSMs
// ─────────────────────────────────────────────────────────────────────────────────────────────────────────

/// Sightline Breaker — cover sized and placed for the sightline it interrupts, materialised by context.
pub struct SightlineBreaker;
impl SightlineBreaker {
    pub const MIN_SIGHTLINE: f32 = 18.0;
}
impl Fsm for SightlineBreaker {
    fn grow(&self, s: &Site) -> Option<Dressing> {
        if s.sightline < Self::MIN_SIGHTLINE {
            return None; // nothing worth breaking → delete
        }
        let long = s.sightline > 34.0;
        let role = if long { Role::HighCover } else { Role::LowCover };
        let sy = if long { 1.5 } else { 0.6 };
        let bump = if s.high_traffic { 0.25 } else { 0.0 };
        Some(Block { role, x: s.x, z: s.z, sx: 1.0 + bump, sy, sz: 0.7 + bump, rot: 0.0, seed: s.seed }.dress(&s.context()))
    }
}

/// Vertical Risk/Reward — a vantage on the high ground with an open railing gap facing the danger angle.
pub struct VerticalRiskReward;
impl Fsm for VerticalRiskReward {
    fn grow(&self, s: &Site) -> Option<Dressing> {
        if !s.is_high_point {
            return None;
        }
        let mut d = Block { role: Role::Elevate, x: s.x, z: s.z, sx: 1.6, sy: 2.4, sz: 1.6, rot: 0.0, seed: s.seed }.dress(&s.context());
        d.meshes.push(detail::railing(Vec3::new(s.x, s.ground_y, s.z), 1.7, s.ground_y + 2.4, s.danger_yaw, lerp_c(s.pal.trim, rgb(0, 0, 0), 0.15)));
        Some(d)
    }
}

/// Sentinel Node — a fortified fighting position: an L of pitted concrete with a sloped tin roof and sandbags
/// on the parapet, the firing face toward the kill-zone. Wants a slope overlooking open ground.
pub struct SentinelNode;
impl Fsm for SentinelNode {
    fn grow(&self, s: &Site) -> Option<Dressing> {
        if s.slope < 0.12 || s.sightline < 22.0 {
            return None;
        }
        let conc = lerp_c(s.pal.wall, rgb(128, 128, 122), 0.5);
        let g = s.ground_y;
        let face = s.danger_yaw;
        let (fx, fz) = (face.cos(), face.sin());
        let mut walls = Mesh3::new(conc);
        walls.merge(&boxx(Vec3::new(s.x, g + 0.6, s.z), Vec3::new(1.6, 0.6, 0.4), conc, FACE_ALL));
        walls.merge(&boxx(Vec3::new(s.x - fz * 1.3, g + 0.6, s.z + fx * 1.3), Vec3::new(0.4, 0.6, 1.4), conc, FACE_ALL));
        detail::weather(&mut walls, g, g + 1.3);
        let mut roof = Mesh3::new(lerp_c(s.pal.trim, rgb(120, 120, 110), 0.4));
        roof.merge(&boxx(Vec3::new(s.x - fz * 0.6, g + 1.35, s.z + fx * 0.6), Vec3::new(1.7, 0.08, 1.5), lerp_c(s.pal.trim, rgb(120, 120, 110), 0.4), FACE_ALL));
        let bags = detail::sandbags(Vec3::new(s.x - 1.3, g + 0.95, s.z - 0.2), Vec3::new(s.x + 1.3, g + 0.95, s.z - 0.2), rgb(146, 128, 90), s.seed);
        let obs = vec![Obstacle { center: Vec3::new(s.x, g + 0.7, s.z), half: Vec3::new(1.7, 0.7, 1.5) }];
        Some(Dressing { meshes: vec![walls, roof, bags], obstacles: obs })
    }
}

/// Logistics Spine — a tower/water-tank landmark: a tall column on splayed braced legs, a flattened reservoir
/// fed by angled pipes, a rust gradient, a blinking warning light. Wants open sky and to be the high point.
pub struct LogisticsSpine;
impl Fsm for LogisticsSpine {
    fn grow(&self, s: &Site) -> Option<Dressing> {
        if s.indoor || !s.is_high_point {
            return None;
        }
        let g = s.ground_y;
        let metal = lerp_c(s.pal.wall, rgb(150, 150, 150), 0.3);
        let mut frame = Mesh3::new(metal);
        let h = 7.0;
        frame.merge(&tube(Vec3::new(s.x, g, s.z), Vec3::new(s.x, g + h, s.z), 0.28, 8, metal));
        let mut legs = [Vec3::new(0.0, 0.0, 0.0); 4];
        for (i, leg) in legs.iter_mut().enumerate() {
            let a = i as f32 * std::f32::consts::FRAC_PI_2 + 0.78;
            *leg = Vec3::new(s.x + a.cos() * 1.4, g, s.z + a.sin() * 1.4);
            frame.merge(&tube(*leg, Vec3::new(s.x, g + h * 0.55, s.z), 0.1, 4, metal));
        }
        for i in 0..4 {
            frame.merge(&tube(legs[i], legs[(i + 1) % 4], 0.07, 4, metal));
        }
        let mut tank = uv_sphere(Vec3::new(s.x, g + h + 0.5, s.z), 1.6, 10, 6, lerp_c(metal, rgb(150, 80, 50), 0.2));
        for v in &mut tank.verts {
            v.y = g + h + 0.5 + (v.y - (g + h + 0.5)) * 0.55;
        }
        detail::weather(&mut tank, g + h - 1.0, g + h + 2.0);
        let mut pipes = Mesh3::new(lerp_c(metal, rgb(0, 0, 0), 0.2));
        for i in 0..3 {
            let a = i as f32 * 2.1;
            let foot = Vec3::new(s.x + a.cos() * 2.4, g, s.z + a.sin() * 2.4);
            pipes.merge(&tube(Vec3::new(s.x, g + h, s.z), foot, 0.08, 4, lerp_c(metal, rgb(0, 0, 0), 0.2)));
        }
        pipes.merge(&detail::slick(Vec3::new(s.x + 2.4, g, s.z), 0.6));
        let mut warn = Mesh3::new(rgb(230, 200, 60)).emit(0.6);
        warn.merge(&uv_sphere(Vec3::new(s.x, g + h + 1.6, s.z), 0.18, 6, 4, rgb(255, 80, 60)));
        let obs = vec![Obstacle { center: Vec3::new(s.x, g + h * 0.5, s.z), half: Vec3::new(1.4, h * 0.5, 1.4) }];
        Some(Dressing { meshes: vec![frame, tank, pipes, warn], obstacles: obs })
    }
}

/// Kinetic Wreckage — a crashed transport sheared into three sections (front pitched, middle in a gouge, rear
/// sheared up to break the sightline), shattered glass, an oil slick, engine fire. Earns its place on a lane.
pub struct KineticWreckage;
impl Fsm for KineticWreckage {
    fn grow(&self, s: &Site) -> Option<Dressing> {
        if s.sightline < 24.0 {
            return None;
        }
        let g = s.ground_y;
        let hull = rgb(86, 96, 70);
        let chrome = rgb(170, 174, 180);
        let yaw = s.danger_yaw + std::f32::consts::FRAC_PI_2;
        let (dx, dz) = (yaw.cos(), yaw.sin());
        let sec = |t: f32| Vec3::new(s.x + dx * t, g, s.z + dz * t);
        let mut body = Mesh3::new(hull);
        let mut front = boxx(Vec3::new(sec(-2.6).x, g + 1.1, sec(-2.6).z), Vec3::new(1.3, 0.8, 1.0), chrome, FACE_ALL);
        crate::mesh::rotate_y_about(&mut front, sec(-2.6), s.rand(1) * 0.6);
        body.merge(&front);
        body.merge(&boxx(Vec3::new(sec(0.0).x, g + 0.7, sec(0.0).z), Vec3::new(1.6, 0.7, 1.1), hull, FACE_ALL));
        let mut rear = boxx(Vec3::new(sec(2.6).x, g + 1.4, sec(2.6).z), Vec3::new(1.2, 0.7, 1.0), hull, FACE_ALL);
        for v in &mut rear.verts {
            v.y += 0.6;
        }
        body.merge(&rear);
        let glass = detail::shards(Vec3::new(sec(-2.6).x, g + 1.0, sec(-2.6).z), Vec3::new(dx, 0.0, dz), 14, s.seed);
        let mut ground = detail::slick(sec(0.0), 1.6);
        ground.merge(&detail::debris(sec(0.0), 2.4, 10, rgb(60, 60, 50), s.seed ^ 0x7));
        let fire = detail::smoke(Vec3::new(sec(2.6).x, g + 1.6, sec(2.6).z), s.seed);
        let obs = vec![Obstacle {
            center: Vec3::new(sec(0.0).x, g + 0.8, sec(0.0).z),
            half: Vec3::new((dx.abs() * 3.0).max(1.1), 0.9, (dz.abs() * 3.0).max(1.1)),
        }];
        Some(Dressing { meshes: vec![body, glass, ground, fire], obstacles: obs })
    }
}

/// Ad-Hoc Shelter — a makeshift tent: a peaked canvas envelope staked with rope, a glowing stove inside, and
/// scavenged plywood on the windward wall. Wants open, not-too-steep ground.
pub struct AdHocShelter;
impl Fsm for AdHocShelter {
    fn grow(&self, s: &Site) -> Option<Dressing> {
        if s.slope > 0.5 || s.indoor {
            return None;
        }
        let g = s.ground_y;
        let canvas = rgb(150, 140, 116);
        let mut tent = Mesh3::new(canvas);
        let w = 1.5;
        let ridge_h = g + 2.0;
        let ridge_a = Vec3::new(s.x - w, ridge_h, s.z);
        let ridge_b = Vec3::new(s.x + w, ridge_h, s.z);
        let corners = [
            Vec3::new(s.x - w, g, s.z - w),
            Vec3::new(s.x + w, g, s.z - w),
            Vec3::new(s.x + w, g, s.z + w),
            Vec3::new(s.x - w, g, s.z + w),
        ];
        tent.push_tri(corners[0], corners[1], ridge_a, Vec3::new(0.0, 0.4, -1.0), canvas);
        tent.push_tri(corners[1], ridge_b, ridge_a, Vec3::new(0.0, 0.4, -1.0), canvas);
        tent.push_tri(corners[2], corners[3], ridge_b, Vec3::new(0.0, 0.4, 1.0), canvas);
        tent.push_tri(corners[3], ridge_a, ridge_b, Vec3::new(0.0, 0.4, 1.0), canvas);
        tint_height(&mut tent, g, ridge_h, lerp_c(canvas, rgb(255, 255, 255), 0.12), lerp_c(canvas, rgb(0, 0, 0), 0.4));
        let mut hard = Mesh3::new(rgb(90, 80, 64));
        for c in corners {
            hard.merge(&boxx(Vec3::new(c.x, g + 0.1, c.z), Vec3::new(0.08, 0.08, 0.08), rgb(70, 64, 52), FACE_ALL));
            hard.merge(&tube(Vec3::new(c.x, g + 0.1, c.z), Vec3::new((ridge_a.x + ridge_b.x) * 0.5, ridge_h, (ridge_a.z + ridge_b.z) * 0.5), 0.02, 3, rgb(120, 110, 92)));
        }
        hard.merge(&boxx(Vec3::new(s.x - w - 0.2, g + 0.7, s.z), Vec3::new(0.06, 0.7, 0.7), rgb(150, 120, 80), FACE_NO_BOTTOM));
        let mut glow = Mesh3::new(rgb(255, 130, 50)).emit(0.7);
        glow.merge(&boxx(Vec3::new(s.x, g + 0.18, s.z), Vec3::new(0.18, 0.18, 0.18), rgb(60, 50, 46), FACE_ALL));
        glow.merge(&disc(Vec3::new(s.x, g + 0.37, s.z), 0.12, 8, rgb(255, 140, 60)));
        let obs = vec![Obstacle { center: Vec3::new(s.x, g + 0.9, s.z), half: Vec3::new(w, 0.9, w) }];
        Some(Dressing { meshes: vec![tent, hard, glow], obstacles: obs })
    }
}

/// Decay Cascader — a *post-process*: given a finished dressing and the bearing traffic comes from, it blows a
/// directed crater on the traffic-facing side, exposes rebar, and scatters rubble — so damage points at the
/// angles players actually fight along.
pub fn cascade_decay(d: &mut Dressing, ground_y: f32, traffic_yaw: f32, center: Vec3, seed: u32) {
    let dir = Vec3::new(traffic_yaw.cos(), 0.0, traffic_yaw.sin());
    let hit = Vec3::new(center.x + dir.x * 0.9, ground_y, center.z + dir.z * 0.9);
    let mut crater = Mesh3::new(rgb(26, 24, 24));
    crater.merge(&boxx(Vec3::new(hit.x, ground_y + 0.9, hit.z), Vec3::new(0.5, 0.5, 0.12), rgb(26, 24, 24), FACE_ALL));
    d.meshes.push(crater);
    d.meshes.push(detail::rebar(Vec3::new(hit.x, ground_y + 0.9, hit.z), dir, 4, seed));
    d.meshes.push(detail::debris(hit, 1.0, 8, rgb(70, 66, 60), seed ^ 0x33));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::level::MapTheme;

    fn site(sightline: f32, slope: f32, high: bool, wet: bool, indoor: bool) -> Site {
        Site {
            x: 0.0,
            z: 0.0,
            ground_y: 0.0,
            sightline,
            danger_yaw: 0.7,
            high_traffic: sightline > 30.0,
            is_high_point: high,
            slope,
            wet,
            indoor,
            pal: MapTheme::Military.palette(),
            seed: 7,
        }
    }
    fn ok(d: &Dressing) -> bool {
        d.tri_count() > 0 && !d.obstacles.is_empty()
    }

    #[test]
    fn sightline_breaker_gates_and_dresses() {
        assert!(SightlineBreaker.grow(&site(6.0, 0.0, false, false, false)).is_none());
        assert!(ok(&SightlineBreaker.grow(&site(40.0, 0.0, false, false, false)).unwrap()));
        for s in 0..16 {
            let mut st = site(22.0, 0.0, false, true, false);
            st.seed = s;
            assert!(SightlineBreaker.grow(&st).is_some());
        }
    }
    #[test]
    fn vantage_only_on_high_ground() {
        assert!(VerticalRiskReward.grow(&site(20.0, 0.0, false, false, false)).is_none());
        assert!(ok(&VerticalRiskReward.grow(&site(20.0, 0.0, true, false, false)).unwrap()));
    }
    #[test]
    fn sentinel_needs_slope_over_open_ground() {
        assert!(SentinelNode.grow(&site(30.0, 0.0, false, false, false)).is_none());
        assert!(ok(&SentinelNode.grow(&site(30.0, 0.3, false, false, false)).unwrap()));
    }
    #[test]
    fn logistics_spine_wants_open_high_ground() {
        assert!(LogisticsSpine.grow(&site(20.0, 0.0, true, false, true)).is_none());
        assert!(ok(&LogisticsSpine.grow(&site(20.0, 0.0, true, false, false)).unwrap()));
    }
    #[test]
    fn wreckage_breaks_long_lanes() {
        assert!(KineticWreckage.grow(&site(10.0, 0.0, false, false, false)).is_none());
        assert!(ok(&KineticWreckage.grow(&site(30.0, 0.0, false, false, false)).unwrap()));
    }
    #[test]
    fn shelter_avoids_steep_and_indoor() {
        assert!(AdHocShelter.grow(&site(10.0, 0.8, false, false, false)).is_none());
        assert!(AdHocShelter.grow(&site(10.0, 0.1, false, false, true)).is_none());
        assert!(ok(&AdHocShelter.grow(&site(10.0, 0.1, false, false, false)).unwrap()));
    }
    #[test]
    fn decay_adds_directed_damage() {
        let mut d = SightlineBreaker.grow(&site(40.0, 0.0, false, false, false)).unwrap();
        let before = d.meshes.len();
        cascade_decay(&mut d, 0.0, 0.7, Vec3::new(0.0, 0.0, 0.0), 9);
        assert!(d.meshes.len() > before);
    }

    #[test]
    fn new_detail_generators_produce_clean_geometry() {
        use crate::mesh::rgb;
        // Every new detail generator should produce real, non-degenerate geometry with valid bounds — the
        // same bar the mesh-audit harness holds the rest of the game to.
        let tri_ok = |m: &Mesh3| -> bool {
            if m.tris.is_empty() {
                return false;
            }
            // no zero-area triangles
            m.tris.iter().all(|t| {
                let a = m.verts[t[0] as usize];
                let b = m.verts[t[1] as usize];
                let c = m.verts[t[2] as usize];
                (b - a).cross(c - a).len() > 1e-6
            })
        };
        let a = Vec3::new(0.0, 2.0, 0.0);
        let b = Vec3::new(10.0, 2.0, 0.0);
        assert!(tri_ok(&detail::pipes_run(a, b, 3, 0x91)), "pipes_run degenerate");
        assert!(tri_ok(&detail::cable_sag(a, b, 1.5, rgb(30, 28, 26))), "cable_sag degenerate");
        assert!(
            tri_ok(&detail::vent_grille(Vec3::new(0.0, 2.0, 0.0), Vec3::new(1.0, 0.0, 0.0), Vec3::new(0.0, 1.0, 0.0), 0.8, 0.8, 5)),
            "vent_grille degenerate"
        );
        assert!(tri_ok(&detail::moss_creep(Vec3::ZERO, 2.0, 20, 3)), "moss_creep degenerate");
        assert!(tri_ok(&detail::crack_network(Vec3::ZERO, 8.0, 5, 0xEA47)), "crack_network degenerate");
    }

    #[test]
    fn crack_network_is_deterministic_and_bounded() {
        // Same seed → identical geometry (deterministic), and the run terminates with a sane triangle count
        // (the FSM's work-guard prevents a pathological seed from running away).
        let m1 = detail::crack_network(Vec3::ZERO, 10.0, 6, 0x1234);
        let m2 = detail::crack_network(Vec3::ZERO, 10.0, 6, 0x1234);
        assert_eq!(m1.tris.len(), m2.tris.len(), "crack network must be deterministic for a seed");
        assert!(m1.tris.len() > 0 && m1.tris.len() < 5000, "crack network tri count {} out of sane range", m1.tris.len());
    }
}
