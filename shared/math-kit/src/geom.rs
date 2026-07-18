//! Segment / polygon geometry — the primitives soldier's chunk collision,
//! unendlich's physics, and paperbound's picking each re-derive.

use crate::vec::Vec2;

/// Closest point on segment `a`–`b` to `p` (degenerate segment ⇒ `a`).
pub fn closest_point_on_segment(a: Vec2, b: Vec2, p: Vec2) -> Vec2 {
    let ab = b - a;
    let denom = ab.len_sq();
    if denom <= 1e-12 {
        return a;
    }
    let t = ((p - a).dot(ab) / denom).clamp(0.0, 1.0);
    a + ab * t
}

/// Distance from `p` to segment `a`–`b`.
pub fn dist_point_segment(a: Vec2, b: Vec2, p: Vec2) -> f32 {
    p.dist(closest_point_on_segment(a, b, p))
}

/// Proper intersection of segments `a1`–`a2` and `b1`–`b2`, endpoint-inclusive.
/// Returns the intersection point, or `None` for disjoint OR collinear-overlap
/// (collinear overlap has no single point; test it separately if you care).
pub fn segments_intersect(a1: Vec2, a2: Vec2, b1: Vec2, b2: Vec2) -> Option<Vec2> {
    let r = a2 - a1;
    let s = b2 - b1;
    let denom = r.cross(s);
    if denom.abs() <= 1e-12 {
        return None; // parallel or collinear
    }
    let q = b1 - a1;
    let t = q.cross(s) / denom;
    let u = q.cross(r) / denom;
    if (0.0..=1.0).contains(&t) && (0.0..=1.0).contains(&u) {
        Some(a1 + r * t)
    } else {
        None
    }
}

/// Point-in-polygon by even-odd ray casting (works for concave and
/// self-intersecting polygons; boundary points are implementation-edge cases,
/// as in every ray-cast test). Polygon is a closed loop given without the
/// repeated first vertex.
pub fn point_in_poly(poly: &[Vec2], p: Vec2) -> bool {
    let n = poly.len();
    if n < 3 {
        return false;
    }
    let mut inside = false;
    let mut j = n - 1;
    for i in 0..n {
        let (a, b) = (poly[i], poly[j]);
        if (a.y > p.y) != (b.y > p.y) {
            let x_at = a.x + (p.y - a.y) / (b.y - a.y) * (b.x - a.x);
            if p.x < x_at {
                inside = !inside;
            }
        }
        j = i;
    }
    inside
}

/// Signed area of a simple polygon (positive = counter-clockwise).
pub fn polygon_area(poly: &[Vec2]) -> f32 {
    let n = poly.len();
    if n < 3 {
        return 0.0;
    }
    let mut s = 0.0;
    let mut j = n - 1;
    for i in 0..n {
        s += poly[j].cross(poly[i]);
        j = i;
    }
    s * 0.5
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vec::v2;

    #[test]
    fn closest_point_and_distance() {
        let (a, b) = (v2(0.0, 0.0), v2(10.0, 0.0));
        assert_eq!(closest_point_on_segment(a, b, v2(3.0, 5.0)), v2(3.0, 0.0));
        assert_eq!(closest_point_on_segment(a, b, v2(-4.0, 2.0)), a); // clamps to endpoint
        assert_eq!(dist_point_segment(a, b, v2(15.0, 0.0)), 5.0);
        assert_eq!(closest_point_on_segment(a, a, v2(7.0, 7.0)), a); // degenerate
    }

    #[test]
    fn segment_intersection() {
        let hit = segments_intersect(v2(0.0, 0.0), v2(4.0, 4.0), v2(0.0, 4.0), v2(4.0, 0.0));
        assert_eq!(hit, Some(v2(2.0, 2.0)));
        // disjoint
        assert_eq!(segments_intersect(v2(0.0, 0.0), v2(1.0, 0.0), v2(0.0, 1.0), v2(1.0, 1.0)), None);
        // parallel
        assert_eq!(segments_intersect(v2(0.0, 0.0), v2(4.0, 0.0), v2(0.0, 1.0), v2(4.0, 1.0)), None);
        // shared endpoint counts
        assert_eq!(
            segments_intersect(v2(0.0, 0.0), v2(2.0, 2.0), v2(2.0, 2.0), v2(4.0, 0.0)),
            Some(v2(2.0, 2.0))
        );
    }

    #[test]
    fn point_in_polygon_concave() {
        // a "C" shape: concavity on the right
        let poly = [
            v2(0.0, 0.0),
            v2(4.0, 0.0),
            v2(4.0, 1.0),
            v2(1.0, 1.0),
            v2(1.0, 3.0),
            v2(4.0, 3.0),
            v2(4.0, 4.0),
            v2(0.0, 4.0),
        ];
        assert!(point_in_poly(&poly, v2(0.5, 2.0))); // in the spine
        assert!(!point_in_poly(&poly, v2(3.0, 2.0))); // in the concave notch
        assert!(point_in_poly(&poly, v2(3.0, 0.5))); // in the lower arm
        assert!(!point_in_poly(&poly, v2(5.0, 2.0))); // outside
        assert!(!point_in_poly(&poly[..2], v2(0.0, 0.0))); // degenerate poly
    }

    #[test]
    fn area_sign_tracks_winding() {
        let ccw = [v2(0.0, 0.0), v2(2.0, 0.0), v2(2.0, 2.0), v2(0.0, 2.0)];
        assert_eq!(polygon_area(&ccw), 4.0);
        let cw: Vec<_> = ccw.iter().rev().copied().collect();
        assert_eq!(polygon_area(&cw), -4.0);
    }
}
