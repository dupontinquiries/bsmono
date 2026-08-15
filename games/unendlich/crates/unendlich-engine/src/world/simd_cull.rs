//! `world::simd_cull` — batch sphere-vs-frustum culling, SSE on x86_64, NEON on aarch64.
//!
//! A front-end CPU pre-cull: given many chunk bounding spheres and the six frustum
//! planes, decide which spheres are potentially visible so the uploader/dispatcher
//! only feeds the GPU what matters. The work is embarrassingly parallel across
//! chunks and branch-free per plane, so it vectorises cleanly — four chunks per
//! iteration with SSE. Data is Structure-of-Arrays (`xs/ys/zs/rs`) so each SIMD lane
//! loads contiguously with no gather.
//!
//! [`cull_spheres`] dispatches to the SSE path on x86_64, the NEON path on
//! aarch64, and the scalar path elsewhere; all are kept and a differential test
//! pins them to identical output, so the unsafe intrinsics can't silently diverge.

/// Plane convention: a point `p` is inside plane `[a,b,c,d]` when
/// `a*px + b*py + c*pz + d >= 0`. A sphere of radius `r` centred at `p` is culled
/// when it lies fully outside any plane, i.e. `a*px + b*py + c*pz + d < -r`.
pub type Plane = [f32; 4];

/// Scalar reference: for each sphere, visible unless some plane fully rejects it.
pub fn cull_spheres_scalar(
    xs: &[f32],
    ys: &[f32],
    zs: &[f32],
    rs: &[f32],
    planes: &[Plane],
) -> Vec<bool> {
    let n = xs.len();
    let mut out = vec![true; n];
    for i in 0..n {
        let mut visible = true;
        for p in planes {
            let dist = p[0] * xs[i] + p[1] * ys[i] + p[2] * zs[i] + p[3];
            if dist < -rs[i] {
                visible = false;
                break;
            }
        }
        out[i] = visible;
    }
    out
}

/// Batch cull. SSE (4-wide) on x86_64, scalar elsewhere. All four input slices must
/// be the same length.
pub fn cull_spheres(
    xs: &[f32],
    ys: &[f32],
    zs: &[f32],
    rs: &[f32],
    planes: &[Plane],
) -> Vec<bool> {
    assert!(
        xs.len() == ys.len() && ys.len() == zs.len() && zs.len() == rs.len(),
        "SoA slices must share a length"
    );
    #[cfg(target_arch = "x86_64")]
    {
        // SSE/SSE2 are part of the x86_64 baseline, so this is always available.
        unsafe { cull_spheres_sse(xs, ys, zs, rs, planes) }
    }
    #[cfg(target_arch = "aarch64")]
    {
        // NEON (AdvSIMD) is part of the aarch64 baseline — no feature detection.
        unsafe { cull_spheres_neon(xs, ys, zs, rs, planes) }
    }
    #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
    {
        cull_spheres_scalar(xs, ys, zs, rs, planes)
    }
}

/// NEON 4-wide mirror of the SSE path.
///
/// One deliberate difference from the SSE body: `dist` uses the *scalar*
/// association `((p0*x + p1*y) + p2*z) + p3`, not SSE's balanced-tree
/// `(p0*x + p1*y) + (p2*z + p3)`. f32 addition is not associative, so the
/// balanced form can flip a `dist < -r` comparison on spheres sitting exactly
/// at the last-ulp of a plane; matching the scalar association keeps the
/// differential test airtight rather than probabilistic. No FMA (`vfmaq_f32`)
/// for the same reason — the scalar path compiles unfused.
#[cfg(target_arch = "aarch64")]
unsafe fn cull_spheres_neon(
    xs: &[f32],
    ys: &[f32],
    zs: &[f32],
    rs: &[f32],
    planes: &[Plane],
) -> Vec<bool> {
    use core::arch::aarch64::*;

    let n = xs.len();
    let mut out = vec![true; n];
    let mut i = 0usize;

    while i + 4 <= n {
        let x = vld1q_f32(xs.as_ptr().add(i));
        let y = vld1q_f32(ys.as_ptr().add(i));
        let z = vld1q_f32(zs.as_ptr().add(i));
        let r = vld1q_f32(rs.as_ptr().add(i));
        let neg_r = vnegq_f32(r);

        // Accumulate "rejected by some plane" across all six planes.
        let mut rejected = vdupq_n_u32(0);
        for p in planes {
            let px = vdupq_n_f32(p[0]);
            let py = vdupq_n_f32(p[1]);
            let pz = vdupq_n_f32(p[2]);
            let pw = vdupq_n_f32(p[3]);
            // dist = ((p0*x + p1*y) + p2*z) + p3  — scalar association, see above.
            let dist = vaddq_f32(
                vaddq_f32(vaddq_f32(vmulq_f32(px, x), vmulq_f32(py, y)), vmulq_f32(pz, z)),
                pw,
            );
            // lane is rejected where dist < -r
            let rej = vcltq_f32(dist, neg_r);
            rejected = vorrq_u32(rejected, rej);
        }

        // Lane k all-ones → rejected → not visible.
        let mut lanes = [0u32; 4];
        vst1q_u32(lanes.as_mut_ptr(), rejected);
        out[i] = lanes[0] == 0;
        out[i + 1] = lanes[1] == 0;
        out[i + 2] = lanes[2] == 0;
        out[i + 3] = lanes[3] == 0;
        i += 4;
    }

    // Scalar tail for the remaining < 4 spheres.
    while i < n {
        let mut visible = true;
        for p in planes {
            if p[0] * xs[i] + p[1] * ys[i] + p[2] * zs[i] + p[3] < -rs[i] {
                visible = false;
                break;
            }
        }
        out[i] = visible;
        i += 1;
    }
    out
}

#[cfg(target_arch = "x86_64")]
unsafe fn cull_spheres_sse(
    xs: &[f32],
    ys: &[f32],
    zs: &[f32],
    rs: &[f32],
    planes: &[Plane],
) -> Vec<bool> {
    use std::arch::x86_64::*;

    let n = xs.len();
    let mut out = vec![true; n];
    let mut i = 0usize;

    while i + 4 <= n {
        let x = _mm_loadu_ps(xs.as_ptr().add(i));
        let y = _mm_loadu_ps(ys.as_ptr().add(i));
        let z = _mm_loadu_ps(zs.as_ptr().add(i));
        let r = _mm_loadu_ps(rs.as_ptr().add(i));
        let neg_r = _mm_sub_ps(_mm_setzero_ps(), r);

        // Accumulate "rejected by some plane" across all six planes.
        let mut rejected = _mm_setzero_ps();
        for p in planes {
            let px = _mm_set1_ps(p[0]);
            let py = _mm_set1_ps(p[1]);
            let pz = _mm_set1_ps(p[2]);
            let pw = _mm_set1_ps(p[3]);
            // dist = px*x + py*y + pz*z + pw
            let dist = _mm_add_ps(
                _mm_add_ps(_mm_mul_ps(px, x), _mm_mul_ps(py, y)),
                _mm_add_ps(_mm_mul_ps(pz, z), pw),
            );
            // lane is rejected where dist < -r
            let rej = _mm_cmplt_ps(dist, neg_r);
            rejected = _mm_or_ps(rejected, rej);
        }

        // Bit k set → lane k rejected → not visible.
        let mask = _mm_movemask_ps(rejected);
        out[i] = (mask & 0b0001) == 0;
        out[i + 1] = (mask & 0b0010) == 0;
        out[i + 2] = (mask & 0b0100) == 0;
        out[i + 3] = (mask & 0b1000) == 0;
        i += 4;
    }

    // Scalar tail for the remaining < 4 spheres.
    while i < n {
        let mut visible = true;
        for p in planes {
            if p[0] * xs[i] + p[1] * ys[i] + p[2] * zs[i] + p[3] < -rs[i] {
                visible = false;
                break;
            }
        }
        out[i] = visible;
        i += 1;
    }
    out
}

/// Count how many of a batch survive the cull — cheap metric for logging how much
/// the front-end pre-cull removed before upload/dispatch.
pub fn visible_count(mask: &[bool]) -> usize {
    mask.iter().filter(|&&v| v).count()
}

#[cfg(test)]
mod tests {
    use super::*;

    // A simple box frustum: |x|<=10, |y|<=10, |z|<=10, as six inward planes.
    fn box_planes() -> Vec<Plane> {
        vec![
            [1.0, 0.0, 0.0, 10.0],  // x >= -10
            [-1.0, 0.0, 0.0, 10.0], // x <= 10
            [0.0, 1.0, 0.0, 10.0],
            [0.0, -1.0, 0.0, 10.0],
            [0.0, 0.0, 1.0, 10.0],
            [0.0, 0.0, -1.0, 10.0],
        ]
    }

    #[test]
    fn inside_visible_outside_culled() {
        let planes = box_planes();
        // [0,0,0] inside; [100,0,0] far outside; [10.5,0,0] r=1 clips the boundary.
        let xs = [0.0, 100.0, 10.5];
        let ys = [0.0, 0.0, 0.0];
        let zs = [0.0, 0.0, 0.0];
        let rs = [1.0, 1.0, 1.0];
        let vis = cull_spheres(&xs, &ys, &zs, &rs, &planes);
        assert_eq!(vis, vec![true, false, true]);
    }

    #[test]
    fn simd_matches_scalar_on_random_inputs() {
        // Deterministic xorshift; exercise the SSE body + the scalar tail (n%4 varies).
        let mut s: u32 = 0x9E3779B9;
        let mut rng = || {
            s ^= s << 13;
            s ^= s >> 17;
            s ^= s << 5;
            (s as f32 / u32::MAX as f32) * 60.0 - 30.0 // [-30, 30]
        };
        let planes = box_planes();
        for n in 0..200usize {
            let xs: Vec<f32> = (0..n).map(|_| rng()).collect();
            let ys: Vec<f32> = (0..n).map(|_| rng()).collect();
            let zs: Vec<f32> = (0..n).map(|_| rng()).collect();
            let rs: Vec<f32> = (0..n).map(|_| rng().abs() * 0.1).collect();
            let simd = cull_spheres(&xs, &ys, &zs, &rs, &planes);
            let scalar = cull_spheres_scalar(&xs, &ys, &zs, &rs, &planes);
            assert_eq!(simd, scalar, "SIMD/scalar diverged at n={n}");
        }
    }

    #[test]
    fn tail_lengths_all_agree() {
        // Explicitly cover every tail remainder 0..=3 beyond a full SSE block.
        let planes = box_planes();
        for n in [4, 5, 6, 7, 8] {
            let xs: Vec<f32> = (0..n).map(|k| k as f32 - 2.0).collect();
            let ys = vec![0.0; n];
            let zs = vec![0.0; n];
            let rs = vec![0.5; n];
            assert_eq!(
                cull_spheres(&xs, &ys, &zs, &rs, &planes),
                cull_spheres_scalar(&xs, &ys, &zs, &rs, &planes),
                "mismatch at n={n}"
            );
        }
    }

    #[test]
    fn visible_count_metric() {
        assert_eq!(visible_count(&[true, false, true, true]), 3);
        assert_eq!(visible_count(&[]), 0);
    }
}
