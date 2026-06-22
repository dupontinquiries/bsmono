//! Blend two adjacent face normals to hide faceting on merged quads
//! (handoff/BODY_PART_MESHING.md). Returns a unit vector; falls back to `n_a`
//! if the blend degenerates (near-opposite normals).

pub fn blend(n_a: [f32; 3], n_b: [f32; 3], w: f32) -> [f32; 3] {
    let w = w.clamp(0.0, 1.0);
    let mut v = [
        n_a[0] * (1.0 - w) + n_b[0] * w,
        n_a[1] * (1.0 - w) + n_b[1] * w,
        n_a[2] * (1.0 - w) + n_b[2] * w,
    ];
    let len = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    if len > 1e-6 {
        v[0] /= len;
        v[1] /= len;
        v[2] /= len;
        v
    } else {
        n_a
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn ulen(v: [f32; 3]) -> f32 { (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt() }
    #[test] fn w0_is_a() { assert_eq!(blend([0.0, 0.0, 1.0], [1.0, 0.0, 0.0], 0.0), [0.0, 0.0, 1.0]); }
    #[test] fn w1_is_b() { assert_eq!(blend([0.0, 0.0, 1.0], [1.0, 0.0, 0.0], 1.0), [1.0, 0.0, 0.0]); }
    #[test] fn mid_is_unit() { assert!((ulen(blend([0.0, 0.0, 1.0], [1.0, 0.0, 0.0], 0.5)) - 1.0).abs() < 1e-5); }
    #[test] fn opposite_guard_no_nan() {
        let v = blend([1.0, 0.0, 0.0], [-1.0, 0.0, 0.0], 0.5);
        assert!(v[0].is_finite() && v[1].is_finite() && v[2].is_finite());
    }
    #[test] fn unit_length_out() {
        for &w in &[0.2f32, 0.4, 0.6, 0.8] {
            assert!((ulen(blend([0.0, 1.0, 0.0], [0.0, 0.0, 1.0], w)) - 1.0).abs() < 1e-5);
        }
    }
    #[test] fn deterministic() { assert_eq!(blend([0.0, 1.0, 0.0], [1.0, 0.0, 0.0], 0.5), blend([0.0, 1.0, 0.0], [1.0, 0.0, 0.0], 0.5)); }
}
