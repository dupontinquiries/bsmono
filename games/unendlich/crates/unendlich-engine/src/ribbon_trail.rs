//! ribbon_trail.rs — pure motion-trail ribbon vertices from a position history.
//!
//! Builds a flat ribbon strip following a trail of points, two vertices per point
//! (left/right of the trail direction). Pure, deterministic, panic-free
//! (R1/R11/R21). Output is `2 * points.len()` vertices.

fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}
fn normalize(v: [f32; 3], fallback: [f32; 3]) -> [f32; 3] {
    let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    if l > 1e-6 {
        [v[0] / l, v[1] / l, v[2] / l]
    } else {
        fallback
    }
}

/// Ribbon vertices for a trail of `points`, total width `width`. For each point
/// emits `left` then `right` offset `±width/2` along the trail's perpendicular.
/// Empty input yields empty output; `width == 0` collapses both vertices onto the
/// centerline point.
pub fn ribbon(points: &[[f32; 3]], width: f32) -> Vec<[f32; 3]> {
    let n = points.len();
    if n == 0 {
        return Vec::new();
    }
    let up = [0.0, 1.0, 0.0];
    let half = width * 0.5;
    let mut out = Vec::with_capacity(n * 2);
    for i in 0..n {
        let tangent = if n == 1 {
            [1.0, 0.0, 0.0]
        } else if i == 0 {
            sub(points[1], points[0])
        } else if i == n - 1 {
            sub(points[n - 1], points[n - 2])
        } else {
            sub(points[i + 1], points[i - 1])
        };
        let t = normalize(tangent, [1.0, 0.0, 0.0]);
        let perp = normalize(cross(t, up), [0.0, 0.0, 1.0]);
        let p = points[i];
        out.push([p[0] + perp[0] * half, p[1] + perp[1] * half, p[2] + perp[2] * half]);
        out.push([p[0] - perp[0] * half, p[1] - perp[1] * half, p[2] - perp[2] * half]);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(n: usize) -> Vec<[f32; 3]> {
        (0..n).map(|i| [i as f32, 0.0, 0.0]).collect()
    }
    fn dist(a: [f32; 3], b: [f32; 3]) -> f32 {
        ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
    }

    #[test]
    fn n_points_two_verts_each() {
        for n in 0..40usize {
            assert_eq!(ribbon(&line(n), 1.0).len(), n * 2);
        }
    }

    #[test]
    fn width_offsets() {
        let v = ribbon(&line(5), 2.0);
        for i in 0..5 {
            // distance between the pair == width
            assert!((dist(v[2 * i], v[2 * i + 1]) - 2.0).abs() < 1e-5, "width offset wrong");
        }
    }

    #[test]
    fn empty_is_empty() {
        assert!(ribbon(&[], 1.0).is_empty());
    }

    #[test]
    fn collinear_safe() {
        let v = ribbon(&line(10), 1.0);
        for c in &v {
            assert!(c[0].is_finite() && c[1].is_finite() && c[2].is_finite(), "NaN on collinear trail");
        }
    }

    #[test]
    fn width_zero_is_centerline() {
        let pts = line(6);
        let v = ribbon(&pts, 0.0);
        for i in 0..6 {
            assert_eq!(v[2 * i], pts[i], "left vert not on centerline");
            assert_eq!(v[2 * i + 1], pts[i], "right vert not on centerline");
        }
    }

    #[test]
    fn deterministic() {
        let p = line(7);
        assert_eq!(ribbon(&p, 1.5), ribbon(&p, 1.5));
    }
}
