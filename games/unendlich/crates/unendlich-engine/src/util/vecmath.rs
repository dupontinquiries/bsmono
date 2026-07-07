//! vecmath — [f32;3] element-wise ops (guide §4.3). std-only, pure.
pub type V3 = [f32; 3];

#[inline]
pub fn len3(v: V3) -> f32 { (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt() }

/// Normalise; returns +X for near-zero input (panic-free, NaN-free).
#[inline]
pub fn norm3(v: V3) -> V3 {
    let l = len3(v);
    if l > 1e-6 { [v[0] / l, v[1] / l, v[2] / l] } else { [1.0, 0.0, 0.0] }
}

/// Element-wise lerp. t is NOT clamped — callers clamp if needed.
#[inline]
pub fn lerp3(a: V3, b: V3, t: f32) -> V3 {
    [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t]
}

/// Fast normalize (rsqrt, no divide) — SIM/VISUAL ONLY, never a value
/// that flows through the GPU worldgen shader. Delegates to
/// `fastmath::norm3_fast`; same `[1,0,0]` degenerate fallback as `norm3`.
#[inline]
pub fn norm3_fast(v: V3) -> V3 { crate::util::fastmath::norm3_fast(v) }

/// Scale every element and clamp to [0,1]. Colour shading.

#[inline]
pub fn scale_clamp01(v: V3, s: f32) -> V3 {
    [v[0] * s, v[1] * s, v[2] * s].map(|x| x.clamp(0.0, 1.0))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn norm3_unit_and_degenerate() {
        let n = norm3([3.0, 0.0, 4.0]);
        assert!((len3(n) - 1.0).abs() < 1e-5);
        assert_eq!(norm3([0.0; 3]), [1.0, 0.0, 0.0]);
    }
    #[test]
    fn lerp3_endpoints() {
        assert_eq!(lerp3([0.0; 3], [1.0, 2.0, 3.0], 0.0), [0.0; 3]);
        assert_eq!(lerp3([0.0; 3], [1.0, 2.0, 3.0], 1.0), [1.0, 2.0, 3.0]);
    }
}
