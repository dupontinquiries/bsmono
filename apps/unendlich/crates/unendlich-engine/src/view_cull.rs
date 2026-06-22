//! view_cull.rs — pure per-view culling for splitscreen.
//!
//! Given a view's world-space bounds (an AABB — e.g. the region a splitscreen
//! view can see, derived alongside `viewport_layout`) and a set of object
//! bounding spheres, decide which objects that view may skip. Pure math,
//! deterministic, panic-free (R1/R11/R21). Conservative: an object is culled
//! only when its sphere lies fully outside the view bounds, so nothing visible
//! is ever dropped.

/// Axis-aligned bounding box in world space.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Aabb {
    pub min: [f32; 3],
    pub max: [f32; 3],
}

/// Object bounding sphere in world space.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Sphere {
    pub center: [f32; 3],
    pub radius: f32,
}

/// Squared distance from `p` to the nearest point of `b` (0 if inside).
fn dist2_point_aabb(p: [f32; 3], b: &Aabb) -> f32 {
    let mut d2 = 0.0f32;
    for i in 0..3 {
        let lo = b.min[i].min(b.max[i]);
        let hi = b.min[i].max(b.max[i]);
        let v = p[i];
        if v < lo {
            let e = lo - v;
            d2 += e * e;
        } else if v > hi {
            let e = v - hi;
            d2 += e * e;
        }
    }
    d2
}

/// True if `s` lies fully outside `view` (i.e. it can be culled). A sphere that
/// touches or overlaps the box is kept.
pub fn is_culled(view: &Aabb, s: &Sphere) -> bool {
    let r = s.radius.max(0.0);
    dist2_point_aabb(s.center, view) > r * r
}

/// Per-object cull mask: `mask[i] == true` means object `i` can be skipped for
/// this view.
pub fn cull_mask(view: &Aabb, objects: &[Sphere]) -> Vec<bool> {
    objects.iter().map(|s| is_culled(view, s)).collect()
}

/// Indices of objects this view must still draw (the complement of the cull
/// mask), in input order.
pub fn visible_indices(view: &Aabb, objects: &[Sphere]) -> Vec<usize> {
    objects
        .iter()
        .enumerate()
        .filter(|(_, s)| !is_culled(view, s))
        .map(|(i, _)| i)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn view() -> Aabb {
        Aabb { min: [0.0, 0.0, 0.0], max: [100.0, 50.0, 100.0] }
    }

    #[test]
    fn inside_is_visible() {
        let v = view();
        let s = Sphere { center: [50.0, 25.0, 50.0], radius: 1.0 };
        assert!(!is_culled(&v, &s));
    }

    #[test]
    fn clearly_outside_is_culled() {
        let v = view();
        let s = Sphere { center: [1000.0, 25.0, 50.0], radius: 5.0 };
        assert!(is_culled(&v, &s));
    }

    #[test]
    fn overlapping_edge_is_kept() {
        let v = view();
        // 10 units outside the +X face, radius 12 -> sphere still touches the box.
        let s = Sphere { center: [110.0, 25.0, 50.0], radius: 12.0 };
        assert!(!is_culled(&v, &s), "touching sphere must not be culled");
    }

    #[test]
    fn just_outside_radius_is_culled() {
        let v = view();
        // 10 units outside, radius 9 -> fully outside.
        let s = Sphere { center: [110.0, 25.0, 50.0], radius: 9.0 };
        assert!(is_culled(&v, &s));
    }

    #[test]
    fn prop_nothing_visible_is_culled() {
        // Any sphere whose centre is inside the box is never culled, for any
        // non-negative radius. Deterministic grid of inputs (no RNG).
        let v = view();
        let mut x = 0.0;
        while x <= 100.0 {
            let mut z = 0.0;
            while z <= 100.0 {
                let s = Sphere { center: [x, 25.0, z], radius: 0.0 };
                assert!(!is_culled(&v, &s), "inside point culled at {},{}", x, z);
                z += 10.0;
            }
            x += 10.0;
        }
    }

    #[test]
    fn prop_far_objects_all_culled_and_deterministic() {
        let v = view();
        let objs: Vec<Sphere> = (0..32)
            .map(|i| Sphere {
                center: [1000.0 + i as f32, 25.0, 50.0],
                radius: 3.0,
            })
            .collect();
        let mask = cull_mask(&v, &objs);
        assert!(mask.iter().all(|&c| c), "far objects should all cull");
        assert!(visible_indices(&v, &objs).is_empty());
        assert_eq!(mask, cull_mask(&v, &objs)); // deterministic
    }

    #[test]
    fn visible_indices_are_complement_in_order() {
        let v = view();
        let objs = vec![
            Sphere { center: [50.0, 25.0, 50.0], radius: 1.0 },   // in  -> 0
            Sphere { center: [9999.0, 0.0, 0.0], radius: 1.0 },   // out
            Sphere { center: [10.0, 10.0, 10.0], radius: 1.0 },   // in  -> 2
        ];
        assert_eq!(visible_indices(&v, &objs), vec![0, 2]);
    }
}
