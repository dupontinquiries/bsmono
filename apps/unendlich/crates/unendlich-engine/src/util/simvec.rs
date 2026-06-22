//! simvec — **batched** `[f32;3]` vector math. WID `r336-simvectors`.
//!
//! `util::vecmath` is scalar (one vector per call); hot loops (mesher, particles,
//! flow fields) want to process *arrays* of vectors. This module is the batched
//! layer: slice-wise ops that write into a caller-provided `&mut [..]` (no alloc
//! in the hot path) as straight-line `for`/zip loops with no early exit, so LLVM
//! can autovectorize them. It complements `util::batch_kernel::map_into`.
//!
//! **Parity is the contract.** Every op here is the *same arithmetic* as its
//! `util::vecmath` scalar sibling, element for element — it only reshapes the
//! loop, it does not change results (proven element-wise in the tests against a
//! verbatim copy of the `vecmath` scalars). In particular [`normalize_into`]
//! reproduces `vecmath::norm3` exactly, **including its degenerate fallback of
//! `[1, 0, 0]`** for near-zero input (panic-free and NaN-free — see the note on
//! [`normalize_into`]).
//!
//! Pure, deterministic, panic-free: every loop is bounded by `zip` (stops at the
//! shortest slice — extra `out` capacity is left untouched, a short `out` simply
//! limits how many elements are written), no indexing, no `unwrap`.

/// 3-component vector, matching `vecmath::V3`.
pub type V3 = [f32; 3];

/// Degenerate-length threshold — identical to `vecmath::norm3`.
const EPS_LEN: f32 = 1e-6;

// ---------------------------------------------------------------------------
// AoS (array-of-structs) ops over `&[[f32; 3]]`
// ---------------------------------------------------------------------------

/// `out[i] = a[i] + b[i]`, element-wise.
#[inline]
pub fn add_into(a: &[V3], b: &[V3], out: &mut [V3]) {
    for (o, (av, bv)) in out.iter_mut().zip(a.iter().zip(b.iter())) {
        *o = [av[0] + bv[0], av[1] + bv[1], av[2] + bv[2]];
    }
}

/// `out[i] = a[i] * s`, element-wise (un-clamped — the plain scale).
#[inline]
pub fn scale_into(a: &[V3], s: f32, out: &mut [V3]) {
    for (o, av) in out.iter_mut().zip(a.iter()) {
        *o = [av[0] * s, av[1] * s, av[2] * s];
    }
}

/// `out[i] = dot(a[i], b[i])`.
#[inline]
pub fn dot_into(a: &[V3], b: &[V3], out: &mut [f32]) {
    for (o, (av, bv)) in out.iter_mut().zip(a.iter().zip(b.iter())) {
        *o = av[0] * bv[0] + av[1] * bv[1] + av[2] * bv[2];
    }
}

/// `out[i] = len(a[i])` — matches `vecmath::len3` exactly.
#[inline]
pub fn len_into(a: &[V3], out: &mut [f32]) {
    for (o, av) in out.iter_mut().zip(a.iter()) {
        *o = (av[0] * av[0] + av[1] * av[1] + av[2] * av[2]).sqrt();
    }
}

/// `out[i] = normalize(a[i])` — matches `vecmath::norm3` **exactly**, degenerate
/// fallback included: when `len(a[i]) <= 1e-6` the result is `[1, 0, 0]` (never
/// NaN, never a divide-by-zero). This preserves parity with `vecmath`; if a
/// caller genuinely wants `0 → 0` that is a `vecmath` policy change, out of scope
/// for this batched mirror (noted in the MANIFEST).
#[inline]
pub fn normalize_into(a: &[V3], out: &mut [V3]) {
    for (o, av) in out.iter_mut().zip(a.iter()) {
        let l = (av[0] * av[0] + av[1] * av[1] + av[2] * av[2]).sqrt();
        *o = if l > EPS_LEN {
            [av[0] / l, av[1] / l, av[2] / l]
        } else {
            [1.0, 0.0, 0.0]
        };
    }
}

/// `out[i] = lerp(a[i], b[i], t)` — matches `vecmath::lerp3` (t is NOT clamped).
#[inline]
pub fn lerp_into(a: &[V3], b: &[V3], t: f32, out: &mut [V3]) {
    for (o, (av, bv)) in out.iter_mut().zip(a.iter().zip(b.iter())) {
        *o = [
            av[0] + (bv[0] - av[0]) * t,
            av[1] + (bv[1] - av[1]) * t,
            av[2] + (bv[2] - av[2]) * t,
        ];
    }
}

// ---------------------------------------------------------------------------
// SoA (struct-of-arrays) — the layout LLVM vectorizes most readily
// ---------------------------------------------------------------------------

/// Struct-of-arrays storage for a batch of 3-vectors. Parallel `x`/`y`/`z` lanes.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Vec3Soa {
    pub x: Vec<f32>,
    pub y: Vec<f32>,
    pub z: Vec<f32>,
}

impl Vec3Soa {
    /// Number of vectors (the shortest lane, so it is always a valid bound).
    #[inline]
    pub fn len(&self) -> usize {
        self.x.len().min(self.y.len()).min(self.z.len())
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Split an AoS slice into SoA lanes.
    pub fn from_aos(v: &[V3]) -> Vec3Soa {
        let mut s = Vec3Soa {
            x: Vec::with_capacity(v.len()),
            y: Vec::with_capacity(v.len()),
            z: Vec::with_capacity(v.len()),
        };
        for p in v {
            s.x.push(p[0]);
            s.y.push(p[1]);
            s.z.push(p[2]);
        }
        s
    }

    /// Recombine the lanes into AoS (bounded by [`Vec3Soa::len`]).
    pub fn to_aos(&self) -> Vec<V3> {
        let n = self.len();
        let mut out = Vec::with_capacity(n);
        for i in 0..n {
            out.push([self.x[i], self.y[i], self.z[i]]);
        }
        out
    }
}

/// SoA `out[i] = len((x,y,z)[i])`.
#[inline]
pub fn len_into_soa(x: &[f32], y: &[f32], z: &[f32], out: &mut [f32]) {
    for (o, ((xi, yi), zi)) in out.iter_mut().zip(x.iter().zip(y.iter()).zip(z.iter())) {
        *o = (xi * xi + yi * yi + zi * zi).sqrt();
    }
}

/// SoA `out[i] = dot(a[i], b[i])`.
#[inline]
#[allow(clippy::too_many_arguments)]
pub fn dot_into_soa(
    ax: &[f32], ay: &[f32], az: &[f32],
    bx: &[f32], by: &[f32], bz: &[f32],
    out: &mut [f32],
) {
    for (o, (((axi, ayi), azi), ((bxi, byi), bzi))) in out.iter_mut().zip(
        ax.iter().zip(ay.iter()).zip(az.iter())
            .zip(bx.iter().zip(by.iter()).zip(bz.iter())),
    ) {
        *o = axi * bxi + ayi * byi + azi * bzi;
    }
}

/// SoA normalize, writing into three output lanes. Same `[1,0,0]` degenerate
/// fallback as [`normalize_into`] / `vecmath::norm3`.
#[inline]
#[allow(clippy::too_many_arguments)]
pub fn normalize_into_soa(
    x: &[f32], y: &[f32], z: &[f32],
    ox: &mut [f32], oy: &mut [f32], oz: &mut [f32],
) {
    for (((oxi, oyi), ozi), ((xi, yi), zi)) in ox
        .iter_mut().zip(oy.iter_mut()).zip(oz.iter_mut())
        .zip(x.iter().zip(y.iter()).zip(z.iter()))
    {
        let l = (xi * xi + yi * yi + zi * zi).sqrt();
        if l > EPS_LEN {
            *oxi = xi / l;
            *oyi = yi / l;
            *ozi = zi / l;
        } else {
            *oxi = 1.0;
            *oyi = 0.0;
            *ozi = 0.0;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- verbatim copy of the `util::vecmath` scalars used as the parity oracle.
    mod vecmath_oracle {
        pub type V3 = [f32; 3];
        pub fn len3(v: V3) -> f32 {
            (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt()
        }
        pub fn norm3(v: V3) -> V3 {
            let l = len3(v);
            if l > 1e-6 { [v[0] / l, v[1] / l, v[2] / l] } else { [1.0, 0.0, 0.0] }
        }
        pub fn lerp3(a: V3, b: V3, t: f32) -> V3 {
            [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t]
        }
        pub fn add3(a: V3, b: V3) -> V3 { [a[0] + b[0], a[1] + b[1], a[2] + b[2]] }
        pub fn scale3(a: V3, s: f32) -> V3 { [a[0] * s, a[1] * s, a[2] * s] }
        pub fn dot3(a: V3, b: V3) -> f32 { a[0] * b[0] + a[1] * b[1] + a[2] * b[2] }
    }

    fn sample() -> Vec<V3> {
        vec![
            [3.0, 0.0, 4.0],
            [1.0, 2.0, 2.0],
            [-5.0, 0.5, 12.0],
            [0.0, 0.0, 0.0],       // degenerate
            [1e-9, -1e-9, 0.0],    // sub-threshold → degenerate
            [-1.0, -2.0, -3.0],
            [100.0, 0.0, 0.0],
        ]
    }

    fn v_close(a: V3, b: V3) -> bool {
        (0..3).all(|k| (a[k] - b[k]).abs() <= 1e-6)
    }

    #[test]
    fn add_scale_dot_len_match_vecmath_elementwise() {
        let a = sample();
        let b: Vec<V3> = a.iter().rev().cloned().collect();
        let n = a.len();

        let mut sums = vec![[0.0; 3]; n];
        add_into(&a, &b, &mut sums);
        for i in 0..n {
            assert!(v_close(sums[i], vecmath_oracle::add3(a[i], b[i])), "add @{i}");
        }

        let mut scaled = vec![[0.0; 3]; n];
        scale_into(&a, -1.5, &mut scaled);
        for i in 0..n {
            assert!(v_close(scaled[i], vecmath_oracle::scale3(a[i], -1.5)), "scale @{i}");
        }

        let mut dots = vec![0.0f32; n];
        dot_into(&a, &b, &mut dots);
        for i in 0..n {
            assert!((dots[i] - vecmath_oracle::dot3(a[i], b[i])).abs() <= 1e-6, "dot @{i}");
        }

        let mut lens = vec![0.0f32; n];
        len_into(&a, &mut lens);
        for i in 0..n {
            assert!((lens[i] - vecmath_oracle::len3(a[i])).abs() <= 1e-6, "len @{i}");
        }
    }

    #[test]
    fn normalize_matches_vecmath_including_degenerate_fallback() {
        let a = sample();
        let mut out = vec![[0.0; 3]; a.len()];
        normalize_into(&a, &mut out);
        for (i, &v) in a.iter().enumerate() {
            assert!(v_close(out[i], vecmath_oracle::norm3(v)), "normalize @{i}");
            // never NaN
            assert!(out[i].iter().all(|c| c.is_finite()), "NaN/inf @{i}");
        }
        // explicit: zero normalizes to the vecmath fallback [1,0,0] (NOT zero).
        assert_eq!(out[3], [1.0, 0.0, 0.0]);
    }

    #[test]
    fn lerp_matches_vecmath_for_several_t() {
        let a = sample();
        let b: Vec<V3> = a.iter().map(|v| [v[0] + 1.0, v[1] - 2.0, v[2] * 0.5]).collect();
        for &t in &[-0.5f32, 0.0, 0.25, 1.0, 1.5] {
            let mut out = vec![[0.0; 3]; a.len()];
            lerp_into(&a, &b, t, &mut out);
            for i in 0..a.len() {
                assert!(v_close(out[i], vecmath_oracle::lerp3(a[i], b[i], t)), "lerp t={t} @{i}");
            }
        }
    }

    #[test]
    fn empty_slices_are_a_no_op() {
        let empty: Vec<V3> = Vec::new();
        let mut out_v: Vec<V3> = Vec::new();
        let mut out_f: Vec<f32> = Vec::new();
        add_into(&empty, &empty, &mut out_v);
        scale_into(&empty, 2.0, &mut out_v);
        normalize_into(&empty, &mut out_v);
        lerp_into(&empty, &empty, 0.5, &mut out_v);
        dot_into(&empty, &empty, &mut out_f);
        len_into(&empty, &mut out_f);
        assert!(out_v.is_empty() && out_f.is_empty());
        // a longer `out` than `src` only gets its first src.len() entries written
        let src = vec![[1.0, 1.0, 1.0]];
        let mut out = vec![[9.0; 3]; 3];
        scale_into(&src, 2.0, &mut out);
        assert_eq!(out[0], [2.0, 2.0, 2.0]);
        assert_eq!(out[1], [9.0, 9.0, 9.0]); // untouched
    }

    #[test]
    fn aos_soa_round_trips_and_soa_ops_match_aos() {
        let a = sample();
        let soa = Vec3Soa::from_aos(&a);
        assert_eq!(soa.len(), a.len());
        assert_eq!(soa.to_aos(), a); // exact round-trip

        let b: Vec<V3> = a.iter().rev().cloned().collect();
        let sb = Vec3Soa::from_aos(&b);
        let n = a.len();

        // SoA len == AoS len
        let mut soa_len = vec![0.0f32; n];
        len_into_soa(&soa.x, &soa.y, &soa.z, &mut soa_len);
        let mut aos_len = vec![0.0f32; n];
        len_into(&a, &mut aos_len);
        assert_eq!(soa_len, aos_len);

        // SoA dot == AoS dot
        let mut soa_dot = vec![0.0f32; n];
        dot_into_soa(&soa.x, &soa.y, &soa.z, &sb.x, &sb.y, &sb.z, &mut soa_dot);
        let mut aos_dot = vec![0.0f32; n];
        dot_into(&a, &b, &mut aos_dot);
        assert_eq!(soa_dot, aos_dot);

        // SoA normalize == AoS normalize
        let (mut ox, mut oy, mut oz) = (vec![0.0; n], vec![0.0; n], vec![0.0; n]);
        normalize_into_soa(&soa.x, &soa.y, &soa.z, &mut ox, &mut oy, &mut oz);
        let mut aos_norm = vec![[0.0; 3]; n];
        normalize_into(&a, &mut aos_norm);
        for i in 0..n {
            assert_eq!([ox[i], oy[i], oz[i]], aos_norm[i], "soa/aos normalize @{i}");
        }
    }

    #[test]
    fn deterministic() {
        let a = sample();
        let mut o1 = vec![[0.0; 3]; a.len()];
        let mut o2 = vec![[0.0; 3]; a.len()];
        normalize_into(&a, &mut o1);
        normalize_into(&a, &mut o2);
        assert_eq!(o1, o2);
    }
}
