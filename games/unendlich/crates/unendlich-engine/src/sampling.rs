//! Cheap approximate medium interactions via short blue-noise sampling rays.
//!
//! Exact collision against voxels or boxes is overkill for "how much snow lands
//! here", "is this droplet touching water", "how sheltered is this nook from
//! settling dust". Those want a *fraction*, not a contact point. So we scatter a
//! small fixed set of short rays — distributed with Poisson-disk (blue-noise)
//! spacing so a dozen rays cover a hemisphere evenly instead of clumping — and
//! read back a coverage ratio. A handful of `collision::Aabb::ray` or
//! `raycast::raycast_voxels` calls per query; precompute the directions once.
//!
//! - **Snow / dust deposition** ← [`exposure`]: upward rays that escape to the
//!   sky. Open ground catches snow; an overhang shelters it.
//! - **Water contact / wetness** ← [`immersion`]: sample points across a body
//!   that fall inside fluid.
//! - **Settling orientation** ← [`settle_normal`]: mean normal of the surface a
//!   particle would land on.
//!
//! Pure (the queries take a hit-test closure, so they're world-agnostic and
//! testable), deterministic (seeded), `f32`.

use std::f32::consts::TAU;

// ── Deterministic RNG (xorshift64*) ───────────────────────────────────────────

struct Rng(u64);

impl Rng {
    fn new(seed: u32) -> Rng {
        // Avoid the all-zero state xorshift can't escape.
        Rng((seed as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15).wrapping_add(0xD1B5_4A32_D192_ED03))
    }
    fn next_u32(&mut self) -> u32 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        (x.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 32) as u32
    }
    fn next_f32(&mut self) -> f32 {
        self.next_u32() as f32 / u32::MAX as f32
    }
}

// ── Blue-noise sample sets ────────────────────────────────────────────────────

fn dist2(a: [f32; 2], b: [f32; 2]) -> f32 {
    let dx = a[0] - b[0];
    let dy = a[1] - b[1];
    dx * dx + dy * dy
}

/// `n` points in the unit disk with Poisson-disk (blue-noise) spacing, via
/// Mitchell's best-candidate: each new point is the farthest-from-existing of a
/// growing pool of random candidates. Deterministic in `seed`.
pub fn poisson_disk(n: usize, seed: u32) -> Vec<[f32; 2]> {
    let mut rng = Rng::new(seed);
    let mut pts: Vec<[f32; 2]> = Vec::with_capacity(n);
    for _ in 0..n {
        let k = (pts.len() + 1).min(20); // more candidates as the set fills
        let mut best = [0.0f32; 2];
        let mut best_d = -1.0f32;
        for _ in 0..k {
            // Uniform point in the unit disk (sqrt for equal-area radius).
            let r = rng.next_f32().sqrt();
            let th = TAU * rng.next_f32();
            let c = [r * th.cos(), r * th.sin()];
            let d = if pts.is_empty() {
                f32::INFINITY
            } else {
                pts.iter().map(|p| dist2(*p, c)).fold(f32::INFINITY, f32::min)
            };
            if d > best_d {
                best_d = d;
                best = c;
            }
        }
        pts.push(best);
    }
    pts
}

fn normalize3(v: [f32; 3]) -> [f32; 3] {
    let len = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    if len <= 1e-12 {
        [0.0, 1.0, 0.0]
    } else {
        [v[0] / len, v[1] / len, v[2] / len]
    }
}

fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

/// `n` blue-noise unit directions in the hemisphere around `up`. Built by
/// lifting a Poisson disk onto the hemisphere (`z = √(1−r²)`), so directions are
/// denser toward the pole — a good cosine-ish weighting for "how open is the
/// sky above me". Every direction has `dot(dir, up) ≥ 0`.
pub fn hemisphere_dirs(n: usize, up: [f32; 3], seed: u32) -> Vec<[f32; 3]> {
    let up = normalize3(up);
    // Orthonormal basis around `up`.
    let helper = if up[0].abs() < 0.9 { [1.0, 0.0, 0.0] } else { [0.0, 1.0, 0.0] };
    let t = normalize3(cross(helper, up));
    let b = cross(up, t);
    poisson_disk(n, seed)
        .into_iter()
        .map(|[dx, dy]| {
            let r2 = (dx * dx + dy * dy).min(1.0);
            let z = (1.0 - r2).sqrt();
            normalize3([
                dx * t[0] + dy * b[0] + z * up[0],
                dx * t[1] + dy * b[1] + z * up[1],
                dx * t[2] + dy * b[2] + z * up[2],
            ])
        })
        .collect()
}

/// Convenience: blue-noise rays pointing up into the sky — the deposition set
/// for snow and dust. Precompute once and reuse.
pub fn upward_hemisphere(n: usize, seed: u32) -> Vec<[f32; 3]> {
    hemisphere_dirs(n, [0.0, 1.0, 0.0], seed)
}

// ── Approximate interaction queries ───────────────────────────────────────────

/// Fraction of sample rays from `origin` along `dirs` that travel `max_dist`
/// without being blocked — `1.0` fully open, `0.0` fully sheltered. Snow and
/// dust deposit in proportion to this (cast `upward_hemisphere` rays); it's also
/// a cheap ambient-occlusion term. `blocked(origin, dir, max_dist) -> bool`,
/// e.g. `|o, d, m| world_aabb.ray(o, d).map_or(false, |t| t <= m)` or a
/// `raycast_voxels(...).is_some()`.
pub fn exposure(
    origin: [f32; 3],
    dirs: &[[f32; 3]],
    max_dist: f32,
    mut blocked: impl FnMut([f32; 3], [f32; 3], f32) -> bool,
) -> f32 {
    if dirs.is_empty() {
        return 1.0;
    }
    let open = dirs.iter().filter(|d| !blocked(origin, **d, max_dist)).count();
    open as f32 / dirs.len() as f32
}

/// Shelter / approximate ambient occlusion = `1 − exposure`.
pub fn occlusion(
    origin: [f32; 3],
    dirs: &[[f32; 3]],
    max_dist: f32,
    blocked: impl FnMut([f32; 3], [f32; 3], f32) -> bool,
) -> f32 {
    1.0 - exposure(origin, dirs, max_dist, blocked)
}

/// Fraction of sample `points` that lie inside fluid — an approximate
/// submersion / wetness for a body, finer than a few axis samples. Scatter
/// `points` across the body's footprint (e.g. a `poisson_disk` scaled and
/// placed). `in_fluid(point) -> bool`.
pub fn immersion(points: &[[f32; 3]], mut in_fluid: impl FnMut([f32; 3]) -> bool) -> f32 {
    if points.is_empty() {
        return 0.0;
    }
    let wet = points.iter().filter(|p| in_fluid(**p)).count();
    wet as f32 / points.len() as f32
}

/// Mean (unit) surface normal seen by sample rays — the orientation a settling
/// particle would rest at (snow on a slope, dust on a ledge, a droplet on a
/// face). `cast(origin, dir, max_dist) -> Option<normal>`; returns `None` if no
/// ray hits anything. Cast downward-biased rays for deposition surfaces.
pub fn settle_normal(
    origin: [f32; 3],
    dirs: &[[f32; 3]],
    max_dist: f32,
    mut cast: impl FnMut([f32; 3], [f32; 3], f32) -> Option<[f32; 3]>,
) -> Option<[f32; 3]> {
    let mut acc = [0.0f32; 3];
    let mut hits = 0u32;
    for d in dirs {
        if let Some(n) = cast(origin, *d, max_dist) {
            acc[0] += n[0];
            acc[1] += n[1];
            acc[2] += n[2];
            hits += 1;
        }
    }
    if hits == 0 {
        None
    } else {
        Some(normalize3(acc))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::collision::Aabb;

    #[test]
    fn poisson_disk_is_inside_and_deterministic_and_spread() {
        let a = poisson_disk(16, 7);
        let b = poisson_disk(16, 7);
        assert_eq!(a.len(), 16);
        assert_eq!(a, b, "same seed → identical set");
        for p in &a {
            assert!(p[0] * p[0] + p[1] * p[1] <= 1.0 + 1e-5, "inside the unit disk");
        }
        // Blue-noise: the closest pair is far better separated than random
        // (random 16-in-disk min spacing is tiny; best-candidate keeps it open).
        let mut min_d2 = f32::INFINITY;
        for i in 0..a.len() {
            for j in i + 1..a.len() {
                min_d2 = min_d2.min(dist2(a[i], a[j]));
            }
        }
        assert!(min_d2.sqrt() > 0.18, "well-spaced, min dist = {}", min_d2.sqrt());
        // A different seed gives a different set.
        assert_ne!(a, poisson_disk(16, 8));
    }

    #[test]
    fn hemisphere_dirs_are_unit_and_above_the_plane() {
        let up = normalize3([0.3, 1.0, -0.2]);
        let dirs = hemisphere_dirs(24, up, 3);
        assert_eq!(dirs.len(), 24);
        for d in &dirs {
            let len = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
            assert!((len - 1.0).abs() < 1e-4, "unit length");
            let dot = d[0] * up[0] + d[1] * up[1] + d[2] * up[2];
            assert!(dot >= -1e-4, "in the hemisphere around up, dot={dot}");
        }
    }

    #[test]
    fn exposure_open_sheltered_and_half() {
        let dirs = upward_hemisphere(20, 1);
        let o = [0.0, 0.0, 0.0];
        assert_eq!(exposure(o, &dirs, 4.0, |_, _, _| false), 1.0, "nothing blocks → open");
        assert_eq!(exposure(o, &dirs, 4.0, |_, _, _| true), 0.0, "all blocked → sheltered");
        // Block rays leaning toward +x only → partial exposure in (0,1).
        let e = exposure(o, &dirs, 4.0, |_, d, _| d[0] > 0.0);
        assert!(e > 0.0 && e < 1.0, "partial overhang gives partial exposure: {e}");
        assert_eq!(occlusion(o, &dirs, 4.0, |_, _, _| true), 1.0);
    }

    #[test]
    fn exposure_uses_the_collision_aabb_api() {
        // A slab hanging directly overhead shelters the upward rays beneath it.
        let roof = Aabb::new([-2.0, 2.0, -2.0], [2.0, 2.5, 2.0]);
        let dirs = upward_hemisphere(32, 5);
        let under = exposure([0.0, 0.0, 0.0], &dirs, 8.0, |o, d, m| {
            roof.ray(o, d).map_or(false, |t| t <= m)
        });
        let beside = exposure([10.0, 0.0, 0.0], &dirs, 8.0, |o, d, m| {
            roof.ray(o, d).map_or(false, |t| t <= m)
        });
        assert!(under < beside, "sheltered under the slab ({under}) < open beside it ({beside})");
        assert_eq!(beside, 1.0, "clear sky away from the slab");
    }

    #[test]
    fn immersion_counts_points_in_fluid() {
        // Water surface at y = 0; points below are wet.
        let pts = [
            [0.0, -1.0, 0.0],
            [0.0, -0.5, 0.0],
            [0.0, 0.5, 0.0],
            [0.0, 1.0, 0.0],
        ];
        let frac = immersion(&pts, |p| p[1] < 0.0);
        assert!((frac - 0.5).abs() < 1e-6, "half submerged, got {frac}");
        assert_eq!(immersion(&[], |_| true), 0.0, "no samples → dry");
    }

    #[test]
    fn settle_normal_averages_to_the_floor() {
        // Downward rays all hit a flat floor with an up normal.
        let dirs = hemisphere_dirs(12, [0.0, -1.0, 0.0], 2);
        let n = settle_normal([0.0, 3.0, 0.0], &dirs, 8.0, |_, _, _| Some([0.0, 1.0, 0.0]))
            .expect("floor hit");
        assert!(n[1] > 0.999, "settles flat on the floor: {n:?}");
        // Nothing hit → None.
        assert!(settle_normal([0.0, 3.0, 0.0], &dirs, 8.0, |_, _, _| None).is_none());
    }
}
