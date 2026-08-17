//! LOAD-BEARING ANALYSIS — does a body's own weight sit over its feet?
//!
//! The classic static-stability check, not a simulation: project every
//! ground-contact part (`partdetect::Kind::Leg`/`Wheel`) onto the
//! ground plane, take the convex hull of those points — the SUPPORT
//! POLYGON — and check whether the body's own centre of mass, also
//! projected onto the ground plane, falls inside it. Outside means the
//! body would tip over standing still, before anything even moves;
//! this is the same check a robotics or biomechanics text uses for
//! "can this thing stand up," not something invented for this project.
//!
//! ## What is measured, what is assumed
//!
//! The geometry (convex hull, point-in-polygon, centroid) is exact —
//! tested against shapes with a known right answer, same discipline as
//! `partdetect.rs`. The INPUT to it is not exact: centre of mass here
//! is a per-prim bounding-box-volume-weighted centroid, which assumes
//! uniform density and treats a hollow cockpit shell the same as a
//! solid strut of the same box size. Real mass distribution (armour
//! plating, a power core, empty greebles) is not something this
//! codebase has data for. Stated once here rather than re-qualified in
//! every function: `stability_margin`'s number is a real answer to a
//! simplified question, not a real answer to the real one.
//!
//! ## What this deliberately does not attempt
//!
//! Per-leg LOAD (how much of the body's weight each contact point
//! actually carries) needs the support polygon's internal force
//! distribution, which for more than 3 contact points is statically
//! INDETERMINATE without knowing the structure's own stiffness — a
//! real engineering unknown, not a fact this module is withholding.
//! `check`'s margin answers "would it stand," not "how hard is each
//! leg working."

use glam::Vec3;

use crate::orient::perp_pair;

/// A point in the ground plane — `orient::perp_pair`'s two axes, not
/// world X/Z, so this works under any `up`.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct P2 {
    pub a: f32,
    pub b: f32,
}

fn project(p: Vec3, up: Vec3) -> P2 {
    let (pa, pb) = perp_pair(up);
    P2 { a: p.dot(pa), b: p.dot(pb) }
}

fn cross(o: P2, a: P2, b: P2) -> f32 {
    (a.a - o.a) * (b.b - o.b) - (a.b - o.b) * (b.a - o.a)
}

/// Convex hull of a 2D point set, Andrew's monotone chain — O(n log n),
/// returned counter-clockwise, no duplicate closing point. Collinear
/// points on an edge are dropped (`cross <= 0.0` in the sweep), which
/// matters here: three leg tips that happen to line up should not
/// produce a degenerate hull edge a point-in-polygon test then has to
/// special-case.
pub fn convex_hull(points: &[P2]) -> Vec<P2> {
    let mut pts: Vec<P2> = points.to_vec();
    pts.sort_by(|p, q| p.a.partial_cmp(&q.a).unwrap().then(p.b.partial_cmp(&q.b).unwrap()));
    pts.dedup_by(|a, b| (a.a - b.a).abs() < 1e-9 && (a.b - b.b).abs() < 1e-9);
    if pts.len() < 3 {
        return pts;
    }
    let build = |pts: &[P2]| -> Vec<P2> {
        let mut hull: Vec<P2> = Vec::new();
        for &p in pts {
            while hull.len() >= 2 && cross(hull[hull.len() - 2], hull[hull.len() - 1], p) <= 0.0 {
                hull.pop();
            }
            hull.push(p);
        }
        hull
    };
    let mut lower = build(&pts);
    let rev: Vec<P2> = pts.iter().rev().copied().collect();
    let mut upper = build(&rev);
    lower.pop();
    upper.pop();
    lower.extend(upper);
    lower
}

/// Is `p` inside (or on the boundary of) a convex polygon, given
/// counter-clockwise as `convex_hull` returns? Winding test: inside
/// iff every edge turns the same way (or straight) toward `p`.
pub fn point_in_convex(p: P2, hull: &[P2]) -> bool {
    if hull.len() < 3 {
        return false;
    }
    for i in 0..hull.len() {
        let a = hull[i];
        let b = hull[(i + 1) % hull.len()];
        if cross(a, b, p) < -1e-6 {
            return false;
        }
    }
    true
}

/// Signed distance from `p` to the NEAREST edge of a convex hull —
/// positive inside, negative outside. What `check` reports: a small
/// positive number is standing but barely, a negative one is standing
/// on nothing.
pub fn signed_margin(p: P2, hull: &[P2]) -> f32 {
    if hull.len() < 3 {
        return f32::MIN;
    }
    let inside = point_in_convex(p, hull);
    let mut best = f32::MAX;
    for i in 0..hull.len() {
        let a = hull[i];
        let b = hull[(i + 1) % hull.len()];
        let edge = P2 { a: b.a - a.a, b: b.b - a.b };
        let len = (edge.a * edge.a + edge.b * edge.b).sqrt().max(1e-9);
        let d = ((p.a - a.a) * edge.b - (p.b - a.b) * edge.a).abs() / len;
        best = best.min(d);
    }
    if inside { best } else { -best }
}

/// One prim's contribution to the load-bearing check: its geometry and
/// what `partdetect::classify` thinks it is.
pub struct PrimInput<'a> {
    pub verts: &'a [Vec3],
    pub kind: crate::partdetect::Kind,
}

/// The result of one body's static-stability check.
pub struct Stability {
    /// Positive: the centre of mass falls inside the support polygon
    /// by this much (same units as the input geometry). Negative: it
    /// falls outside by this much -- the body cannot stand as posed.
    /// `f32::MIN` when there are not enough contact points for a real
    /// polygon to exist.
    pub margin: f32,
    /// How many Leg/Wheel prims contributed a contact point.
    pub contact_points: usize,
}

/// Run the check over a whole body's prims.
pub fn check(prims: &[PrimInput], up: Vec3) -> Stability {
    // Centre of mass: bounding-box-volume-weighted centroid across
    // EVERY prim, not just the legs -- the torso and cockpit are most
    // of a real body's mass and have to be in this sum for the answer
    // to mean anything.
    let mut com = Vec3::ZERO;
    let mut total_mass = 0.0f32;
    for p in prims {
        if p.verts.is_empty() {
            continue;
        }
        let (mut lo, mut hi) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
        let mut centroid = Vec3::ZERO;
        for &v in p.verts {
            lo = lo.min(v);
            hi = hi.max(v);
            centroid += v;
        }
        centroid /= p.verts.len() as f32;
        let ext = (hi - lo).max(Vec3::splat(1e-4));
        let mass = (ext.x * ext.y * ext.z).max(1e-6);
        com += centroid * mass;
        total_mass += mass;
    }
    if total_mass <= 0.0 {
        return Stability { margin: f32::MIN, contact_points: 0 };
    }
    com /= total_mass;

    let contacts: Vec<P2> = prims
        .iter()
        .filter(|p| matches!(p.kind, crate::partdetect::Kind::Leg | crate::partdetect::Kind::Wheel))
        .filter_map(|p| {
            p.verts
                .iter()
                .copied()
                .min_by(|a, b| a.dot(up).partial_cmp(&b.dot(up)).unwrap())
                .map(|lowest| project(lowest, up))
        })
        .collect();

    if contacts.len() < 3 {
        return Stability { margin: f32::MIN, contact_points: contacts.len() };
    }
    let hull = convex_hull(&contacts);
    let com_2d = project(com, up);
    Stability { margin: signed_margin(com_2d, &hull), contact_points: contacts.len() }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(a: f32, b: f32) -> P2 {
        P2 { a, b }
    }

    #[test]
    fn convex_hull_of_a_square_with_an_interior_point_drops_the_interior_point() {
        let pts = [p(0.0, 0.0), p(2.0, 0.0), p(2.0, 2.0), p(0.0, 2.0), p(1.0, 1.0)];
        let hull = convex_hull(&pts);
        assert_eq!(hull.len(), 4, "the interior point should not appear in the hull");
        assert!(!hull.contains(&p(1.0, 1.0)));
    }

    #[test]
    fn convex_hull_of_collinear_points_does_not_produce_a_degenerate_polygon() {
        let pts = [p(0.0, 0.0), p(1.0, 0.0), p(2.0, 0.0), p(1.0, 1.0)];
        let hull = convex_hull(&pts);
        // a valid triangle, not four points with a flat edge that would
        // confuse point-in-polygon
        assert_eq!(hull.len(), 3, "collinear points on an edge should be dropped: {hull:?}");
    }

    #[test]
    fn a_centred_point_is_inside_a_square_and_an_off_centre_one_is_not() {
        let hull = convex_hull(&[p(0.0, 0.0), p(2.0, 0.0), p(2.0, 2.0), p(0.0, 2.0)]);
        assert!(point_in_convex(p(1.0, 1.0), &hull));
        assert!(!point_in_convex(p(5.0, 5.0), &hull));
        // on the boundary counts as inside -- a body balanced exactly
        // on the edge of its feet is not yet falling
        assert!(point_in_convex(p(0.0, 1.0), &hull));
    }

    #[test]
    fn margin_is_positive_inside_and_negative_outside_the_same_hull() {
        let hull = convex_hull(&[p(0.0, 0.0), p(2.0, 0.0), p(2.0, 2.0), p(0.0, 2.0)]);
        let inside = signed_margin(p(1.0, 1.0), &hull); // dead centre, 1.0 from every edge
        assert!((inside - 1.0).abs() < 1e-4, "centre of a 2x2 square should be 1.0 from every edge: {inside}");
        let outside = signed_margin(p(3.0, 1.0), &hull);
        assert!(outside < 0.0, "a point outside the hull should score negative: {outside}");
    }

    fn box_prim(lo: Vec3, hi: Vec3) -> Vec<Vec3> {
        vec![
            Vec3::new(lo.x, lo.y, lo.z), Vec3::new(hi.x, lo.y, lo.z),
            Vec3::new(hi.x, hi.y, lo.z), Vec3::new(lo.x, hi.y, lo.z),
            Vec3::new(lo.x, lo.y, hi.z), Vec3::new(hi.x, lo.y, hi.z),
            Vec3::new(hi.x, hi.y, hi.z), Vec3::new(lo.x, hi.y, hi.z),
        ]
    }

    #[test]
    fn a_symmetric_quadruped_with_centred_mass_stands() {
        // torso centred at the origin, four legs at the corners, each
        // leg's lowest point directly under a corner of the torso
        let torso = box_prim(Vec3::new(-0.5, 0.0, -0.5), Vec3::new(0.5, 1.0, 0.5));
        let leg = |x: f32, z: f32| box_prim(Vec3::new(x - 0.05, -1.0, z - 0.05), Vec3::new(x + 0.05, 0.0, z + 0.05));
        let legs = [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)];
        let leg_vs: Vec<Vec<Vec3>> = legs.iter().map(|&(x, z)| leg(x, z)).collect();
        let mut inputs: Vec<PrimInput> = vec![PrimInput { verts: &torso, kind: crate::partdetect::Kind::Body }];
        for lv in &leg_vs {
            inputs.push(PrimInput { verts: lv, kind: crate::partdetect::Kind::Leg });
        }
        let st = check(&inputs, Vec3::Y);
        assert_eq!(st.contact_points, 4);
        assert!(st.margin > 0.0, "a symmetric body over its own four feet should stand: margin {}", st.margin);
    }

    #[test]
    fn mass_hanging_far_off_one_side_tips_it_over() {
        let torso = box_prim(Vec3::new(-0.5, 0.0, -0.5), Vec3::new(0.5, 1.0, 0.5));
        // a big, heavy-reading mass (large box = large volume proxy)
        // cantilevered far to one side, well outside the leg footprint
        let arm = box_prim(Vec3::new(3.0, 0.0, -0.5), Vec3::new(5.0, 1.0, 0.5));
        let leg = |x: f32, z: f32| box_prim(Vec3::new(x - 0.05, -1.0, z - 0.05), Vec3::new(x + 0.05, 0.0, z + 0.05));
        let legs = [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)];
        let leg_vs: Vec<Vec<Vec3>> = legs.iter().map(|&(x, z)| leg(x, z)).collect();
        let mut inputs: Vec<PrimInput> = vec![
            PrimInput { verts: &torso, kind: crate::partdetect::Kind::Body },
            PrimInput { verts: &arm, kind: crate::partdetect::Kind::Body },
        ];
        for lv in &leg_vs {
            inputs.push(PrimInput { verts: lv, kind: crate::partdetect::Kind::Leg });
        }
        let st = check(&inputs, Vec3::Y);
        assert!(st.margin < 0.0, "mass cantilevered outside the footprint should tip: margin {}", st.margin);
    }

    #[test]
    fn fewer_than_three_contact_points_reports_no_polygon_rather_than_a_number() {
        let torso = box_prim(Vec3::new(-0.5, 0.0, -0.5), Vec3::new(0.5, 1.0, 0.5));
        let leg = box_prim(Vec3::new(-0.05, -1.0, -0.05), Vec3::new(0.05, 0.0, 0.05));
        let inputs = vec![
            PrimInput { verts: &torso, kind: crate::partdetect::Kind::Body },
            PrimInput { verts: &leg, kind: crate::partdetect::Kind::Leg },
        ];
        let st = check(&inputs, Vec3::Y);
        assert_eq!(st.contact_points, 1);
        assert_eq!(st.margin, f32::MIN);
    }
}
