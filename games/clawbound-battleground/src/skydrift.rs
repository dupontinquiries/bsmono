//! skydrift -- what floats in the sky round Starwell Glacier.
//!
//! Nothing here is walkable or collides: it is the air between the islands
//! made to look lived in. Three kinds:
//!
//! * **Anchor stones** -- broken tiles of old flagstone drifting in the
//!   chasms and out past the rim, each resting on a small cloud. Most hang
//!   below the islands; a few, far out, ride higher than the floor.
//! * **Fragments** -- chunks of an island's edge, broken off and hanging
//!   just past its lip.
//! * **Cloud puffs** -- billows drifting below the islands that change
//!   shape as they go.
//!
//! All of it rises and falls on slow air: each piece bobs on its own
//! period, and pieces near each other share a slow swell ([`swell`]), the
//! way a bank of cloud lifts and settles together. Placement is data
//! ([`placements`], deterministic); motion is a pure function of time
//! ([`Drift::at`]); `scene` draws it.

use glam::Vec3;

use crate::sim::Rng;
use crate::sky;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DriftKind {
    Anchor,
    Fragment,
    Puff,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Drift {
    pub kind: DriftKind,
    /// Where it rests, and how big it is (x its mesh).
    pub home: Vec3,
    pub size: f32,
    pub yaw: f32,
    pub variant: i32,
    /// Bob: height and period, seconds; and where in its cycle it starts.
    pub bob: f32,
    pub period: f32,
    pub phase: f32,
    /// Turning, rad/s; and (puffs) the radius of its slow wander.
    pub spin: f32,
    pub wander: f32,
}

/// Where a drifting piece is at time `t`: position, yaw, and scale per
/// axis (puffs breathe out of step on x and z, so they change shape).
pub struct DriftAt {
    pub pos: Vec3,
    pub yaw: f32,
    pub scale: Vec3,
}

/// The slow swell of the air at (x, z): -1..1, shared by neighbours (a
/// wave ~150 u long rolling across the map in about three minutes).
pub fn swell(x: f32, z: f32, t: f32) -> f32 {
    let a = (x * 0.021 + z * 0.013 + t * 0.035).sin();
    let b = (x * -0.011 + z * 0.024 - t * 0.022 + 1.7).sin();
    (a * 0.6 + b * 0.4).clamp(-1.0, 1.0)
}

impl Drift {
    pub fn at(&self, t: f32) -> DriftAt {
        let w = std::f32::consts::TAU / self.period.max(1.0);
        let lift = self.bob * (t * w + self.phase).sin() + self.bob * 1.5 * swell(self.home.x, self.home.z, t);
        let mut pos = self.home + Vec3::Y * lift;
        let mut scale = Vec3::splat(self.size);
        if self.kind == DriftKind::Puff {
            // wander a slow circle, and breathe: wider one way, then the other
            let a = t * w * 0.25 + self.phase;
            pos += Vec3::new(a.cos(), 0.0, a.sin()) * self.wander;
            let b = t * w * 0.5 + self.phase * 1.3;
            scale *= Vec3::new(1.0 + 0.24 * b.sin(), 1.0 + 0.12 * (b * 0.7).cos(), 1.0 - 0.2 * b.sin());
        }
        DriftAt { pos, yaw: self.yaw + self.spin * t, scale }
    }
}

/// Everything drifting on the sky map. Computed AS the sky map, whatever
/// is active.
pub fn placements() -> Vec<Drift> {
    crate::map::with(crate::map::MapId::Sky, placements_here)
}

fn placements_here() -> Vec<Drift> {
    let f = sky::floor();
    let mut rng = Rng::new(0xD21F_7ED);
    let mut out = Vec::new();
    let lim = sky::ARENA_RADIUS + 60.0;
    let near_ferry = |x: f32, z: f32| {
        sky::ferry_routes().iter().any(|(a, b)| {
            let p = Vec3::new(x, 0.0, z);
            let ab = *b - *a;
            let u = ((p - *a).dot(ab) / ab.length_squared().max(1e-6)).clamp(0.0, 1.0);
            (*a + ab * u - p).length() < 12.0
        })
    };
    let mut z = -lim;
    while z <= lim {
        let mut x = -lim;
        while x <= lim {
            let (jx, jz) = (x + rng.range(-4.0, 4.0), z + rng.range(-4.0, 4.0));
            let d = f.sdf(jx, jz);
            let roll = rng.f32();
            let v = (rng.f32() * 12.0) as i32;
            let (phase, yaw) = (rng.f32() * std::f32::consts::TAU, rng.f32() * std::f32::consts::TAU);
            // fragments just past the lip, a little below the floor (or,
            // now and then, a little above it); never where a ferry docks
            if d > sky::RIM_W + 1.5 && d < sky::RIM_W + 15.0 && roll < 0.16 && !near_ferry(jx, jz) {
                out.push(Drift {
                    kind: DriftKind::Fragment,
                    home: Vec3::new(jx, rng.range(-12.0, 1.5), jz),
                    size: rng.range(1.0, 2.0),
                    yaw,
                    variant: v,
                    bob: rng.range(0.3, 1.0),
                    period: rng.range(9.0, 16.0),
                    phase,
                    spin: rng.range(-0.02, 0.02),
                    wander: 0.0,
                });
            } else if d > 18.0 && roll < 0.035 {
                // anchor stones: mostly below; far out, some ride high
                // (the floor's distance field is clamped at 24 u out: past
                // that, everything reads as "far")
                let far = d > 22.0;
                let y = if far && rng.f32() < 0.35 { rng.range(1.5, 6.0) } else { rng.range(-55.0, -5.0) };
                out.push(Drift {
                    kind: DriftKind::Anchor,
                    home: Vec3::new(jx, y, jz),
                    size: rng.range(1.0, 2.4),
                    yaw,
                    variant: v,
                    bob: rng.range(0.6, 2.2),
                    period: rng.range(10.0, 22.0),
                    phase,
                    spin: rng.range(-0.05, 0.05),
                    wander: 0.0,
                });
            } else if d > 14.0 && roll > 0.955 {
                out.push(Drift {
                    kind: DriftKind::Puff,
                    home: Vec3::new(jx, rng.range(-85.0, -20.0), jz),
                    size: rng.range(2.0, 5.0),
                    yaw,
                    variant: v,
                    bob: rng.range(1.5, 4.0),
                    period: rng.range(40.0, 70.0),
                    phase,
                    spin: rng.range(-0.01, 0.01),
                    wander: rng.range(4.0, 14.0),
                });
            }
            x += 12.0;
        }
        z += 12.0;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_sky_has_all_three_and_none_of_it_is_on_the_floor() {
        let v = placements();
        let n = |k: DriftKind| v.iter().filter(|d| d.kind == k).count();
        let (a, f, p) = (n(DriftKind::Anchor), n(DriftKind::Fragment), n(DriftKind::Puff));
        assert!((40..400).contains(&a) && (40..600).contains(&f) && (25..300).contains(&p), "anchors {a}, fragments {f}, puffs {p}");
        let floor = sky::floor();
        for d in &v {
            assert!(floor.sdf(d.home.x, d.home.z) > sky::RIM_W, "{d:?} floats in the sky, not on an island");
        }
        // some ride higher than the floor, most lower
        assert!(v.iter().any(|d| d.kind == DriftKind::Anchor && d.home.y > 0.0));
        assert!(v.iter().filter(|d| d.home.y < 0.0).count() * 4 > v.len() * 3);
    }

    #[test]
    fn it_rises_and_falls_and_the_clouds_change_shape() {
        let v = placements();
        let a = v.iter().find(|d| d.kind == DriftKind::Anchor).unwrap();
        let ys: Vec<f32> = (0..120).map(|k| a.at(k as f32).pos.y).collect();
        let (lo, hi) = ys.iter().fold((f32::MAX, f32::MIN), |(l, h), &y| (l.min(y), h.max(y)));
        assert!(hi - lo > a.bob, "it bobs: {lo}..{hi}");
        assert!(ys.windows(2).all(|w| (w[1] - w[0]).abs() < 2.0), "slowly");
        let p = v.iter().find(|d| d.kind == DriftKind::Puff).unwrap();
        let (s0, s1) = (p.at(0.0).scale, p.at(p.period * 0.5).scale);
        assert!(((s0.x / s0.z) - (s1.x / s1.z)).abs() > 0.1, "a puff changes shape: {s0} vs {s1}");
    }
}
