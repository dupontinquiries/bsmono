//! touch_rays — body touch/feel sensors. A creature carries a handful of feeler
//! rays (whiskers, wingtips, a dragon's blind underside); each marches out from a
//! body point along a direction until it meets something solid, reporting whether
//! it touched, how far, and the surface normal. This is the "feeling" substrate
//! the pathing learner reads, and the first step toward pain. The world is sampled
//! through a caller-supplied solidity closure, so this stays pure and self-
//! contained (the coordinator passes a real voxel sampler). Deterministic.

/// One feeler's reading.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Contact {
    pub hit: bool,
    /// Distance along the ray to the contact (or the ray's max length if clear).
    pub distance: f32,
    /// Estimated outward surface normal at the contact (zero vector if clear).
    pub normal: [f32; 3],
}

fn add(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}
fn scale(a: [f32; 3], s: f32) -> [f32; 3] {
    [a[0] * s, a[1] * s, a[2] * s]
}
fn norm3(v: [f32; 3]) -> [f32; 3] {
    let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    if l > 1e-6 {
        [v[0] / l, v[1] / l, v[2] / l]
    } else {
        [0.0, 0.0, 0.0]
    }
}

/// Estimate the outward normal at `p` from a solidity field by central
/// differences (solid neighbor pushes the normal away from itself). Falls back to
/// `back` (typically the reversed ray direction) when the field is locally
/// uniform.
fn estimate_normal(p: [f32; 3], h: f32, back: [f32; 3], solid: &impl Fn([f32; 3]) -> bool) -> [f32; 3] {
    let s = |o: [f32; 3]| if solid(add(p, o)) { 1.0 } else { 0.0 };
    let gx = s([-h, 0.0, 0.0]) - s([h, 0.0, 0.0]);
    let gy = s([0.0, -h, 0.0]) - s([0.0, h, 0.0]);
    let gz = s([0.0, 0.0, -h]) - s([0.0, 0.0, h]);
    let g = norm3([gx, gy, gz]);
    if g == [0.0, 0.0, 0.0] {
        norm3(back)
    } else {
        g
    }
}

/// March a single feeler from `origin` along `dir` up to `max_len`, stepping by
/// `step`, until `solid` reports a hit. Returns the contact (with an estimated
/// normal) or a clear reading at `max_len`. `dir` need not be normalized.
pub fn feel(origin: [f32; 3], dir: [f32; 3], max_len: f32, step: f32, solid: &impl Fn([f32; 3]) -> bool) -> Contact {
    let d = norm3(dir);
    if d == [0.0, 0.0, 0.0] || max_len <= 0.0 {
        return Contact { hit: false, distance: max_len.max(0.0), normal: [0.0; 3] };
    }
    let st = step.max(1e-3);
    let mut t = 0.0;
    while t <= max_len {
        let p = add(origin, scale(d, t));
        if solid(p) {
            let back = scale(d, -1.0);
            return Contact { hit: true, distance: t, normal: estimate_normal(p, st, back, solid) };
        }
        t += st;
    }
    Contact { hit: false, distance: max_len, normal: [0.0; 3] }
}

/// March several feelers, writing one [`Contact`] per ray into `out` (length =
/// `min(origins, dirs, out)`). No per-call allocation.
pub fn feel_into(out: &mut [Contact], origins: &[[f32; 3]], dirs: &[[f32; 3]], max_len: f32, step: f32, solid: &impl Fn([f32; 3]) -> bool) {
    let n = out.len().min(origins.len()).min(dirs.len());
    for i in 0..n {
        out[i] = feel(origins[i], dirs[i], max_len, step, solid);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lcg(seed: &mut u32) -> f32 {
        *seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
        ((*seed >> 8) as f32) / ((1u32 << 24) as f32)
    }

    #[test]
    fn clear_space_registers_no_contact() {
        let empty = |_p: [f32; 3]| false;
        let c = feel([0.0; 3], [1.0, 0.0, 0.0], 5.0, 0.1, &empty);
        assert!(!c.hit, "nothing solid → no contact");
        assert!((c.distance - 5.0).abs() < 1e-3, "reports the full ray length");
    }

    #[test]
    fn a_wall_within_range_registers() {
        // solid for x >= 2.0 — a wall ahead
        let wall = |p: [f32; 3]| p[0] >= 2.0;
        let c = feel([0.0; 3], [1.0, 0.0, 0.0], 5.0, 0.05, &wall);
        assert!(c.hit, "the wall is felt");
        assert!((c.distance - 2.0).abs() < 0.1, "contact near x=2: {}", c.distance);
        assert!(c.normal[0] < -0.5, "normal points back out of the wall (−X): {:?}", c.normal);
    }

    #[test]
    fn a_wall_beyond_range_is_not_felt() {
        let wall = |p: [f32; 3]| p[0] >= 100.0;
        let c = feel([0.0; 3], [1.0, 0.0, 0.0], 5.0, 0.1, &wall);
        assert!(!c.hit, "too far to feel");
    }

    #[test]
    fn prop_contacts_are_sane_and_finite() {
        let mut seed = 0x7011_C4A0u32;
        for _ in 0..3000 {
            let bound = lcg(&mut seed) * 10.0; // a random axis-aligned wall position
            let axis = (lcg(&mut seed) * 3.0) as usize % 3;
            let wall = move |p: [f32; 3]| p[axis] >= bound;
            let dir = {
                let mut d = [0.0; 3];
                d[axis] = 1.0;
                d
            };
            let max_len = 5.0 + lcg(&mut seed) * 10.0;
            let c = feel([0.0; 3], dir, max_len, 0.1, &wall);
            assert!(c.distance.is_finite() && c.distance >= 0.0 && c.distance <= max_len + 0.2, "distance sane: {}", c.distance);
            for k in 0..3 {
                assert!(c.normal[k].is_finite(), "normal finite");
            }
            if bound <= max_len - 0.2 {
                assert!(c.hit, "wall comfortably within range should be felt");
            }
        }
    }
}
