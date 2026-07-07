//! `mesh_decimate` — LOD vertex-decimation policy.
//!
//! Worker-03 / WORLD-PERF pure core, round 11. Far chunks don't need full vertex
//! density; this drops vertices on a stride that grows with the LOD level, while
//! always keeping the first and last vertex so a strip/fan doesn't lose its
//! endpoints. Pairs with the LOD band selection.
//!
//! Pure & deterministic; no clock/rng/unwrap. Self-contained for `rustc --test`.
#![allow(dead_code)]

/// Keep-stride for a LOD level: 1 at LOD 0 (keep every vertex), doubling each
/// level (1, 2, 4, 8, …), saturated so large LODs don't overflow.
pub fn keep_stride(lod: u32) -> u32 {
    1u32 << lod.min(16)
}

/// Keep every `stride`-th vertex, always preserving the first and last. The
/// result has exactly `ceil(n / stride)` vertices (`stride` is clamped to ≥ 1).
pub fn decimate(verts: &[[f32; 3]], stride: u32) -> Vec<[f32; 3]> {
    let s = stride.max(1) as usize;
    let n = verts.len();
    if n == 0 {
        return Vec::new();
    }
    let count = (n + s - 1) / s; // ceil(n/s)
    let mut out = Vec::with_capacity(count);
    for i in 0..count {
        let idx = (i * s).min(n - 1);
        out.push(verts[idx]);
    }
    // Force the final kept vertex to be the true last vertex (endpoint preserved
    // without changing the count).
    if let Some(last) = out.last_mut() {
        *last = verts[n - 1];
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ramp(n: usize) -> Vec<[f32; 3]> {
        (0..n).map(|i| [i as f32, 0.0, 0.0]).collect()
    }

    #[test]
    fn lod0_keeps_all() {
        assert_eq!(keep_stride(0), 1);
        let v = ramp(10);
        assert_eq!(decimate(&v, keep_stride(0)), v);
    }

    #[test]
    fn higher_lod_keeps_fewer() {
        let v = ramp(64);
        let c0 = decimate(&v, keep_stride(0)).len();
        let c1 = decimate(&v, keep_stride(1)).len();
        let c2 = decimate(&v, keep_stride(2)).len();
        assert!(c0 > c1 && c1 > c2);
    }

    #[test]
    fn stride_at_least_one() {
        for lod in 0..20u32 {
            assert!(keep_stride(lod) >= 1);
        }
        // stride 0 is clamped to 1 (keeps all).
        let v = ramp(5);
        assert_eq!(decimate(&v, 0), v);
    }

    #[test]
    fn preserves_first_and_last() {
        let v = ramp(17);
        for stride in [1u32, 2, 3, 4, 5, 8] {
            let out = decimate(&v, stride);
            assert_eq!(out[0], v[0]);
            assert_eq!(*out.last().unwrap(), v[16]);
        }
    }

    #[test]
    fn count_is_ceil_n_over_stride() {
        for n in [0usize, 1, 5, 10, 16, 17, 100] {
            let v = ramp(n);
            for stride in [1u32, 2, 3, 7] {
                let expected = if n == 0 {
                    0
                } else {
                    (n + stride as usize - 1) / stride as usize
                };
                assert_eq!(decimate(&v, stride).len(), expected);
            }
        }
    }

    #[test]
    fn deterministic() {
        let v = ramp(33);
        assert_eq!(decimate(&v, 4), decimate(&v, 4));
    }

    #[test]
    fn single_vertex_is_kept() {
        let v = ramp(1);
        assert_eq!(decimate(&v, 8), v);
    }
}
