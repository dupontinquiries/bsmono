//! combat.rs — hitscan resolution. A single ray is cast from the muzzle and we return the *nearest* of:
//! a grid wall (DDA traversal), a cover crate (ray vs AABB), or an enemy (ray vs sphere). This is the
//! `HitResult` the GLM-5 `shooter_combat.rs` describes; the particle system keys its impact effect off the
//! `HitKind`, and the game applies damage when an enemy is the nearest thing the ray reached.

use crate::level::Level;
use crate::math::{Vec2, Vec3};

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum HitKind {
    Wall,
    Obstacle,
    Destructible(usize),
    Enemy(usize),
    /// A headshot on enemy `usize` — the caller can apply a damage multiplier.
    Headshot(usize),
    Miss,
}

#[derive(Clone, Copy, Debug)]
pub struct Hit {
    pub kind: HitKind,
    pub point: Vec3,
    pub normal: Vec3,
    pub dist: f32,
}

/// A hittable enemy. Modeled as a vertical **capsule** (segment `base`→`top`, radius `radius`) that follows
/// the body's torso-and-legs silhouette, plus a smaller **head** sphere for headshots. A single sphere (the
/// old model) either clipped the head/feet or padded empty space beside the torso; the capsule hugs the
/// standing figure so hits register where the body actually is.
#[derive(Clone, Copy)]
pub struct Target {
    pub base: Vec3,   // bottom of the torso/leg capsule (near the feet)
    pub top: Vec3,    // top of the capsule (neck/shoulders)
    pub radius: f32,  // body radius
    pub head: Vec3,   // head centre
    pub head_r: f32,  // head radius (for headshots)
    pub id: usize,
}

/// Cast `dir` (assumed normalized) from `origin` up to `max_dist`, against the level grid, the cover
/// obstacles, and `targets`. Returns the closest hit (or a `Miss` at max range).
pub fn raycast(level: &Level, targets: &[Target], origin: Vec3, dir: Vec3, max_dist: f32) -> Hit {
    let mut best = Hit {
        kind: HitKind::Miss,
        point: origin + dir * max_dist,
        normal: -dir,
        dist: max_dist,
    };

    // 1) Grid walls via DDA on the XZ plane (walls are full-height, so a 2D march suffices).
    if let Some((d, p, n)) = dda_wall(level, origin, dir, max_dist) {
        if d < best.dist {
            best = Hit { kind: HitKind::Wall, point: p, normal: n, dist: d };
        }
    }

    // 2) Cover crates (AABB slabs).
    for o in &level.obstacles {
        if let Some((d, n)) = ray_aabb(origin, dir, o.center, o.half) {
            if d > 0.0 && d < best.dist {
                best = Hit { kind: HitKind::Obstacle, point: origin + dir * d, normal: n, dist: d };
            }
        }
    }

    // 2b) Destructibles (intact only) — a closer hit here means the shot chips/breaks the object.
    for (i, dz) in level.destructibles.iter().enumerate() {
        if !dz.blocks() {
            continue;
        }
        if let Some((d, n)) = ray_aabb(origin, dir, dz.center, dz.half) {
            if d > 0.0 && d < best.dist {
                best = Hit { kind: HitKind::Destructible(i), point: origin + dir * d, normal: n, dist: d };
            }
        }
    }

    // 3) Enemies. Test the head sphere first (headshots take priority when both are hit at similar range),
    //    then the body capsule. This matches the visible figure far better than one torso sphere.
    for t in targets {
        // head sphere → headshot
        if let Some(d) = ray_sphere(origin, dir, t.head, t.head_r) {
            if d > 0.0 && d < best.dist {
                let p = origin + dir * d;
                best = Hit { kind: HitKind::Headshot(t.id), point: p, normal: (p - t.head).normalized(), dist: d };
            }
        }
        // body capsule (segment base→top, radius) → body hit
        if let Some((d, cp)) = ray_capsule(origin, dir, t.base, t.top, t.radius) {
            if d > 0.0 && d < best.dist {
                let p = origin + dir * d;
                best = Hit { kind: HitKind::Enemy(t.id), point: p, normal: (p - cp).normalized(), dist: d };
            }
        }
    }

    best
}

/// Ray vs. capsule (a segment `a`→`b` swept by `radius`). Returns `(distance_along_ray, nearest_segment_point)`
/// at the entry, or None. Solves the ray against the infinite cylinder around the segment axis, clamps the
/// hit to the segment span, and falls back to the two end-cap spheres — the standard capsule intersection.
fn ray_capsule(origin: Vec3, dir: Vec3, a: Vec3, b: Vec3, radius: f32) -> Option<(f32, Vec3)> {
    let ab = b - a;
    let ab_len2 = ab.dot(ab).max(1e-6);
    let ao = origin - a;
    // Decompose ray dir and ao into components along the axis and perpendicular to it.
    let ab_dot_d = ab.dot(dir);
    let ab_dot_ao = ab.dot(ao);
    let m = ab_dot_d / ab_len2;
    let n = ab_dot_ao / ab_len2;
    let q = dir - ab * m; // ray dir perpendicular to axis
    let r = ao - ab * n; // origin offset perpendicular to axis
    let aa = q.dot(q);
    let bb = 2.0 * q.dot(r);
    let cc = r.dot(r) - radius * radius;
    let mut best_t = f32::INFINITY;
    // infinite-cylinder roots, accepted only where the contact projects onto the segment span [0,1]
    if aa.abs() > 1e-6 {
        let disc = bb * bb - 4.0 * aa * cc;
        if disc >= 0.0 {
            let sq = disc.sqrt();
            for t in [(-bb - sq) / (2.0 * aa), (-bb + sq) / (2.0 * aa)] {
                if t > 0.0 && t < best_t {
                    let s = m * t + n; // projection onto the axis, normalised to [0,1]
                    if (0.0..=1.0).contains(&s) {
                        best_t = t;
                    }
                }
            }
        }
    }
    // end-cap spheres at a and b
    for c in [a, b] {
        if let Some(t) = ray_sphere(origin, dir, c, radius) {
            if t > 0.0 && t < best_t {
                best_t = t;
            }
        }
    }
    if best_t.is_finite() {
        let hit = origin + dir * best_t;
        // nearest point on the segment to the hit (for the surface normal)
        let s = (hit - a).dot(ab) / ab_len2;
        let cp = a + ab * s.clamp(0.0, 1.0);
        Some((best_t, cp))
    } else {
        None
    }
}

/// 2D DDA (Amanatides–Woo) across the tile grid; the first blocking cell stops the ray. Returns
/// `(distance, hit point at the wall face mid-height, inward face normal)`.
fn dda_wall(level: &Level, origin: Vec3, dir: Vec3, max_dist: f32) -> Option<(f32, Vec3, Vec3)> {
    let ts = level.ts();
    let d2 = Vec2::new(dir.x, dir.z);
    let dl = d2.len();
    if dl < 1e-6 {
        return None; // straight up/down never crosses a wall column
    }
    let (mut tx, mut tz) = level.world_to_tile(Vec2::new(origin.x, origin.z));
    let inv = ts / dl; // world distance per unit of the (normalized-in-XZ) march
    let dirx = d2.x / dl;
    let dirz = d2.y / dl;

    let step_x = if dirx > 0.0 { 1 } else { -1 };
    let step_z = if dirz > 0.0 { 1 } else { -1 };
    let tile_dx = if dirx.abs() < 1e-9 { f32::INFINITY } else { ts / dirx.abs() };
    let tile_dz = if dirz.abs() < 1e-9 { f32::INFINITY } else { ts / dirz.abs() };

    // distance (in XZ world units) to the first x and z tile boundary
    let next_bx = (if dirx > 0.0 { (tx + 1) as f32 } else { tx as f32 }) * ts;
    let next_bz = (if dirz > 0.0 { (tz + 1) as f32 } else { tz as f32 }) * ts;
    let mut t_max_x = if dirx.abs() < 1e-9 { f32::INFINITY } else { (next_bx - origin.x) / dirx };
    let mut t_max_z = if dirz.abs() < 1e-9 { f32::INFINITY } else { (next_bz - origin.z) / dirz };
    let _ = inv;

    let mut travelled_xz;
    // The face normal of whichever boundary we last crossed (points back toward the ray origin).
    let mut step_normal;
    for _ in 0..(level.w + level.h) * 2 {
        // advance to the nearer boundary
        if t_max_x < t_max_z {
            travelled_xz = t_max_x;
            t_max_x += tile_dx;
            tx += step_x;
            step_normal = Vec3::new(-step_x as f32, 0.0, 0.0);
        } else {
            travelled_xz = t_max_z;
            t_max_z += tile_dz;
            tz += step_z;
            step_normal = Vec3::new(0.0, 0.0, -step_z as f32);
        }
        // convert the XZ-plane distance to true 3D distance (dir may have a Y component)
        let dist3d = travelled_xz / dl;
        if dist3d > max_dist {
            return None;
        }
        if level.cell(tx, tz).blocks() {
            let p = origin + dir * dist3d;
            return Some((dist3d, p, step_normal));
        }
    }
    None
}

/// Ray vs axis-aligned box (slab method). Returns `(entry distance, face normal)` if hit ahead of origin.
fn ray_aabb(o: Vec3, d: Vec3, center: Vec3, half: Vec3) -> Option<(f32, Vec3)> {
    let lo = center - half;
    let hi = center + half;
    let mut tmin = f32::NEG_INFINITY;
    let mut tmax = f32::INFINITY;
    let mut normal = Vec3::ZERO;
    let axes = [
        (d.x, o.x, lo.x, hi.x, Vec3::new(1.0, 0.0, 0.0)),
        (d.y, o.y, lo.y, hi.y, Vec3::new(0.0, 1.0, 0.0)),
        (d.z, o.z, lo.z, hi.z, Vec3::new(0.0, 0.0, 1.0)),
    ];
    for (dd, oo, l, h, ax) in axes {
        if dd.abs() < 1e-9 {
            if oo < l || oo > h {
                return None;
            }
        } else {
            let inv = 1.0 / dd;
            let mut t1 = (l - oo) * inv;
            let mut t2 = (h - oo) * inv;
            let mut sign = -1.0;
            if t1 > t2 {
                std::mem::swap(&mut t1, &mut t2);
                sign = 1.0;
            }
            if t1 > tmin {
                tmin = t1;
                normal = ax * sign;
            }
            if t2 < tmax {
                tmax = t2;
            }
            if tmin > tmax {
                return None;
            }
        }
    }
    if tmax < 0.0 {
        return None;
    }
    let t = if tmin >= 0.0 { tmin } else { tmax };
    Some((t, normal))
}

/// Ray vs sphere; returns the near intersection distance ahead of the origin, if any.
fn ray_sphere(o: Vec3, d: Vec3, center: Vec3, r: f32) -> Option<f32> {
    let m = o - center;
    let b = m.dot(d);
    let c = m.dot(m) - r * r;
    if c > 0.0 && b > 0.0 {
        return None; // origin outside and pointing away
    }
    let disc = b * b - c;
    if disc < 0.0 {
        return None;
    }
    let t = -b - disc.sqrt();
    Some(if t < 0.0 { 0.0 } else { t })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::level::{Level, MapConfig};

    #[test]
    fn ray_hits_a_sphere_dead_ahead() {
        let d = ray_sphere(Vec3::ZERO, Vec3::new(0.0, 0.0, -1.0), Vec3::new(0.0, 0.0, -5.0), 1.0).unwrap();
        assert!((d - 4.0).abs() < 1e-4, "enters the sphere at distance 4, got {d}");
        // pointing away misses
        assert!(ray_sphere(Vec3::ZERO, Vec3::new(0.0, 0.0, 1.0), Vec3::new(0.0, 0.0, -5.0), 1.0).is_none());
    }

    #[test]
    fn ray_aabb_entry_and_normal() {
        let (t, n) = ray_aabb(Vec3::ZERO, Vec3::new(1.0, 0.0, 0.0), Vec3::new(5.0, 0.0, 0.0), Vec3::splat(1.0)).unwrap();
        assert!((t - 4.0).abs() < 1e-4);
        assert!(n.x < 0.0, "hits the -X face, normal points back toward origin");
    }

    #[test]
    fn nearer_enemy_beats_farther_wall() {
        // open arena: spawn the player, cast toward a close target; the target (not a wall) should win.
        let lvl = Level::generate(MapConfig { seed: 3, ..Default::default() });
        let room = &lvl.rooms[0];
        let origin = room.center.with_y(1.5);
        let dir = Vec3::new(0.0, 0.0, -1.0);
        let c = origin + dir * 2.0;
        let targets = [Target { base: c - Vec3::new(0.0, 0.5, 0.0), top: c + Vec3::new(0.0, 0.5, 0.0), radius: 0.6, head: c + Vec3::new(0.0, 0.9, 0.0), head_r: 0.3, id: 7 }];
        let hit = raycast(&lvl, &targets, origin, dir, 50.0);
        assert!(matches!(hit.kind, HitKind::Enemy(7) | HitKind::Headshot(7)), "the close enemy is the nearest hit");
        assert!(hit.dist < 3.0);
    }

    #[test]
    fn ray_into_a_wall_reports_wall() {
        let lvl = Level::generate(MapConfig { seed: 8, ..Default::default() });
        // march in +X from room0 centre; eventually we must cross a wall before max range
        let origin = lvl.rooms[0].center.with_y(1.5);
        let hit = raycast(&lvl, &[], origin, Vec3::new(1.0, 0.0, 0.0), 200.0);
        assert!(matches!(hit.kind, HitKind::Wall | HitKind::Obstacle), "a long indoor ray must hit something solid, got {:?}", hit.kind);
    }

    #[test]
    fn capsule_hitbox_registers_head_body_and_misses_empty_space() {
        // An enemy capsule standing at the origin: a shot at head height headshots, a shot at torso height
        // body-hits, and a shot passing well beside the body (outside the capsule radius) misses.
        let lvl = Level::generate(MapConfig { seed: 5, ..Default::default() });
        let feet = lvl.rooms[0].center.with_y(0.0);
        let t = Target {
            base: feet + Vec3::new(0.0, 0.5, 0.0),
            top: feet + Vec3::new(0.0, 1.3, 0.0),
            radius: 0.35,
            head: feet + Vec3::new(0.0, 1.6, 0.0),
            head_r: 0.22,
            id: 3,
        };
        // fire from 3m away toward the figure at various heights (short range → stays in the room)
        let shoot = |y: f32| {
            let origin = feet + Vec3::new(3.0, y, 0.0);
            let dir = Vec3::new(-1.0, 0.0, 0.0);
            raycast(&lvl, &[t], origin, dir, 8.0).kind
        };
        assert!(matches!(shoot(1.6), HitKind::Headshot(3)), "a shot at head height should headshot");
        assert!(matches!(shoot(0.9), HitKind::Enemy(3)), "a shot at torso height should body-hit");
        // a shot 1m above the head passes over → not an enemy/headshot hit
        assert!(!matches!(shoot(2.8), HitKind::Headshot(3) | HitKind::Enemy(3)), "a shot over the head should miss the figure");
    }
}
