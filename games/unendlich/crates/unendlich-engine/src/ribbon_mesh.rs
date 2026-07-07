//! ribbon_mesh.rs — pure breath-ribbon strip geometry (§9A).
//!
//! Builds a flat triangle-strip following a path, two vertices per path point
//! (left/right of the path direction), with the width tapering toward the tail.
//! Feeds the emission consumer. Pure, deterministic, panic-free (R1/R11/R21).
//! Output is flat `xyz` floats: `6` per path point (`2` verts), so a path of `n`
//! points yields `6n` floats.

fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}
fn normalize(v: [f32; 3], fallback: [f32; 3]) -> [f32; 3] {
    let len = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    if len > 1e-6 {
        [v[0] / len, v[1] / len, v[2] / len]
    } else {
        fallback
    }
}

/// Build the ribbon strip. `width` is the full strip width at the head; `taper`
/// (clamped to `[0,1]`) shrinks it linearly toward the tail, so the tail width is
/// `width * (1 - taper)`. An empty path yields an empty `Vec`; a single point is
/// handled (a degenerate cross-strip with a default orientation).
pub fn ribbon(path: &[[f32; 3]], width: f32, taper: f32) -> Vec<f32> {
    let n = path.len();
    if n == 0 {
        return Vec::new();
    }
    let up = [0.0, 1.0, 0.0];
    let taper = taper.clamp(0.0, 1.0);
    let w0 = width.max(0.0);
    let mut out = Vec::with_capacity(n * 6);

    for i in 0..n {
        // tangent from neighbours (forward/backward/central difference)
        let tangent = if n == 1 {
            [1.0, 0.0, 0.0]
        } else if i == 0 {
            sub(path[1], path[0])
        } else if i == n - 1 {
            sub(path[n - 1], path[n - 2])
        } else {
            sub(path[i + 1], path[i - 1])
        };
        let t = normalize(tangent, [1.0, 0.0, 0.0]);
        // perpendicular in the plane; fall back to +Z if the path is vertical
        let perp = normalize(cross(t, up), [0.0, 0.0, 1.0]);

        let f = if n > 1 { i as f32 / (n - 1) as f32 } else { 0.0 };
        let w = (w0 * (1.0 - taper * f)).max(0.0);
        let half = w * 0.5;

        let p = path[i];
        let left = [p[0] + perp[0] * half, p[1] + perp[1] * half, p[2] + perp[2] * half];
        let right = [p[0] - perp[0] * half, p[1] - perp[1] * half, p[2] - perp[2] * half];
        out.extend_from_slice(&left);
        out.extend_from_slice(&right);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lcg(seed: &mut u32) -> f32 {
        *seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        (*seed >> 8) as f32 / ((1u32 << 24) as f32)
    }

    fn straight_path(n: usize) -> Vec<[f32; 3]> {
        (0..n).map(|i| [i as f32, 0.0, 0.0]).collect()
    }

    // width at point i = distance between its left and right vertices
    fn strip_width(verts: &[f32], i: usize) -> f32 {
        let l = [verts[i * 6], verts[i * 6 + 1], verts[i * 6 + 2]];
        let r = [verts[i * 6 + 3], verts[i * 6 + 4], verts[i * 6 + 5]];
        ((l[0] - r[0]).powi(2) + (l[1] - r[1]).powi(2) + (l[2] - r[2]).powi(2)).sqrt()
    }

    #[test]
    fn vert_count_scales_with_path() {
        for n in 0..40usize {
            let v = ribbon(&straight_path(n), 2.0, 0.5);
            assert_eq!(v.len(), n * 6, "expected 6 floats/point");
        }
    }

    #[test]
    fn empty_path_is_empty() {
        assert!(ribbon(&[], 2.0, 0.5).is_empty());
    }

    #[test]
    fn width_tapers_toward_tail() {
        let v = ribbon(&straight_path(10), 4.0, 0.8);
        let mut prev = f32::INFINITY;
        for i in 0..10 {
            let w = strip_width(&v, i);
            assert!(w <= prev + 1e-5, "width grew toward tail at {}", i);
            prev = w;
        }
        // head ~ full width, tail ~ width*(1-taper)
        assert!((strip_width(&v, 0) - 4.0).abs() < 1e-4);
        assert!((strip_width(&v, 9) - 4.0 * 0.2).abs() < 1e-3);
    }

    #[test]
    fn single_point_handled() {
        let v = ribbon(&[[5.0, 1.0, -2.0]], 3.0, 0.5);
        assert_eq!(v.len(), 6);
        assert!((strip_width(&v, 0) - 3.0).abs() < 1e-4, "single point uses full width");
        for c in &v {
            assert!(c.is_finite());
        }
    }

    #[test]
    fn prop_no_nan_inf() {
        let mut seed = 0x91BB_0001u32;
        for _ in 0..300 {
            let n = (lcg(&mut seed) * 12.0) as usize;
            let path: Vec<[f32; 3]> = (0..n)
                .map(|_| [lcg(&mut seed) * 20.0 - 10.0, lcg(&mut seed) * 20.0 - 10.0, lcg(&mut seed) * 20.0 - 10.0])
                .collect();
            let v = ribbon(&path, lcg(&mut seed) * 5.0, lcg(&mut seed));
            for c in &v {
                assert!(c.is_finite(), "non-finite vertex coord");
            }
        }
    }

    #[test]
    fn deterministic() {
        let p = straight_path(6);
        assert_eq!(ribbon(&p, 2.0, 0.3), ribbon(&p, 2.0, 0.3));
    }
}
