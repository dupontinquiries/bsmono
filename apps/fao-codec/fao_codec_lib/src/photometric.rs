//! # photometric — cheap markers that pull the image-formation chain apart
//!
//! A pixel is `Camera(Lens(Atmosphere(Light(World))))`, and inside one
//! frame those factors are coupled: fog and exposure both brighten,
//! sun and vignette both darken. They come apart by SIGNATURE — each
//! stage is a function of a different thing:
//!
//!   exposure     global, multiplicative in intensity
//!   white balance per-channel gain (chroma planes move, luma may not)
//!   vignette     multiplicative in intensity, quadratic in radius
//!   fog          additive blend toward ONE colour, growing with depth
//!                (no depth here: chroma difference vectors converge on
//!                a single point, and the blend grows down the picture
//!                on ground-plane content)
//!   sensor       white spectrum in the difference
//!   motion       explained by a small local shift of the prediction
//!
//! Each marker is a closed-form least-squares fit of the difference
//! `target - pred` against that stage's basis, scored as explained
//! variance of the LUMA difference (chroma where the stage is about
//! chroma). Cheap enough to run per frame in an encoder. This is the
//! separator: fit the bank, and the stage that explains the most per
//! parameter is the one that happened. `fao_engine_capture --vary`
//! produces footage where exactly one stage varies, which is how the
//! bank is validated before it is trusted on film.

use crate::v11_plate::Yuv420;

#[derive(Clone, Debug, Default)]
pub struct Signature {
    /// Variance of the luma difference before any fit (levels^2/px).
    pub var_y: f64,
    /// R^2 of d_Y = a Y + b (global affine on luma). Exposure.
    pub r2_exposure: f64,
    pub exposure: (f64, f64),
    /// Fraction of samples kept by the robust exposure fit.
    pub inliers: f64,
    /// R^2 of the chroma difference under a per-plane affine, versus
    /// under the luma gain applied to chroma. White balance if the
    /// per-plane fit beats the shared gain by a margin.
    pub r2_chroma_shared: f64,
    pub r2_chroma_perplane: f64,
    /// R^2 of d_Y = (v0 + v1 r^2) Y, after removing the global affine.
    /// Vignette: v1 significant.
    pub r2_vignette: f64,
    pub vignette_v1: f64,
    /// Fog: d = t(row) (C - I) with one colour C shared by all planes
    /// and t a quadratic in row position. R^2 on luma+chroma jointly.
    pub r2_fog: f64,
    pub fog_color: (f64, f64, f64),
    /// Lag-1 spatial autocorrelation of the luma difference after the
    /// best global fit. Near 0: sensor-like. Near 1: structured.
    pub lag1: f64,
    /// Fraction of luma difference energy removed by the best local
    /// integer shift of the prediction (+-4 px, 16x16 blocks).
    pub motion_frac: f64,
}

fn lsq2(xs: &[f64], ys: &[f64]) -> (f64, f64) {
    let n = xs.len() as f64;
    let (mut sx, mut sy, mut sxx, mut sxy) = (0.0, 0.0, 0.0, 0.0);
    for (&x, &y) in xs.iter().zip(ys) {
        sx += x;
        sy += y;
        sxx += x * x;
        sxy += x * y;
    }
    let den = n * sxx - sx * sx;
    if den.abs() < 1e-9 {
        return (0.0, sy / n);
    }
    let a = (n * sxy - sx * sy) / den;
    (a, (sy - a * sx) / n)
}

fn var(v: &[f64]) -> f64 {
    let n = v.len() as f64;
    let m = v.iter().sum::<f64>() / n;
    v.iter().map(|x| (x - m) * (x - m)).sum::<f64>() / n
}

fn r2(resid: &[f64], total_var: f64) -> f64 {
    if total_var <= 1e-9 {
        return 0.0;
    }
    (1.0 - var(resid) / total_var).max(0.0)
}

/// Solve a small normal system by Gaussian elimination.
fn solve(mut a: Vec<Vec<f64>>, mut b: Vec<f64>) -> Option<Vec<f64>> {
    let n = b.len();
    for i in 0..n {
        let mut p = i;
        for r in i + 1..n {
            if a[r][i].abs() > a[p][i].abs() {
                p = r;
            }
        }
        if a[p][i].abs() < 1e-12 {
            return None;
        }
        a.swap(i, p);
        b.swap(i, p);
        for r in i + 1..n {
            let f = a[r][i] / a[i][i];
            for c in i..n {
                a[r][c] -= f * a[i][c];
            }
            b[r] -= f * b[i];
        }
    }
    let mut x = vec![0.0; n];
    for i in (0..n).rev() {
        let mut s = b[i];
        for c in i + 1..n {
            s -= a[i][c] * x[c];
        }
        x[i] = s / a[i][i];
    }
    Some(x)
}

/// Least squares y ~ sum_k beta_k * basis_k(i).
fn lsq_basis(basis: &[Vec<f64>], y: &[f64]) -> Option<Vec<f64>> {
    let k = basis.len();
    let mut a = vec![vec![0.0; k]; k];
    let mut b = vec![0.0; k];
    for i in 0..y.len() {
        for r in 0..k {
            b[r] += basis[r][i] * y[i];
            for c in 0..k {
                a[r][c] += basis[r][i] * basis[c][i];
            }
        }
    }
    solve(a, b)
}

pub fn signature(pred: &Yuv420, target: &Yuv420) -> Signature {
    let (w, h) = (pred.w, pred.h);
    let cw = w / 2;
    // Subsample on the chroma grid so luma and chroma line up: one
    // sample per 2x2 luma block, every 2nd chroma sample each way.
    let step = 2usize;
    let mut y_p = Vec::new();
    let mut d_y = Vec::new();
    let mut u_p = Vec::new();
    let mut v_p = Vec::new();
    let mut d_u = Vec::new();
    let mut d_v = Vec::new();
    let mut rr = Vec::new();
    let mut row = Vec::new();
    let (cx, cy) = (w as f64 / 2.0, h as f64 / 2.0);
    let rmax2 = cx * cx + cy * cy;
    let mut ycg = 0usize;
    while ycg < h / 2 {
        let mut xcg = 0usize;
        while xcg < cw {
            let (x, y) = (xcg * 2, ycg * 2);
            let li = y * w + x;
            let ci = ycg * cw + xcg;
            let py = pred.y[li] as f64;
            y_p.push(py);
            d_y.push(target.y[li] as f64 - py);
            u_p.push(pred.u[ci] as f64);
            v_p.push(pred.v[ci] as f64);
            d_u.push(target.u[ci] as f64 - pred.u[ci] as f64);
            d_v.push(target.v[ci] as f64 - pred.v[ci] as f64);
            let (dx, dy) = (x as f64 - cx, y as f64 - cy);
            rr.push((dx * dx + dy * dy) / rmax2);
            row.push(y as f64 / h as f64);
            xcg += step;
        }
        ycg += step;
    }
    let n = d_y.len();
    let mut sig = Signature::default();
    sig.var_y = var(&d_y);
    if n < 64 || sig.var_y < 1e-6 {
        return sig;
    }

    // ── exposure: d_Y = a Y + b, ROBUST. A plain least-squares gain
    // on a misaligned predictor is biased toward 0 (regression
    // dilution): on soldier with a forward-moving camera the naive fit
    // reported a = 0.80-0.88 for a scene whose exposure changed by
    // 1%. Two passes: fit, keep pixels within INLIER levels of the
    // fit, refit. An exact canonical keeps every pixel; a misaligned
    // one keeps the aligned ones and lands on identity.
    const INLIER: f64 = 6.0;
    let (a0, b0) = lsq2(&y_p, &d_y);
    let keep: Vec<bool> = (0..n).map(|i| (d_y[i] - (a0 * y_p[i] + b0)).abs() <= INLIER).collect();
    let (xs, ys): (Vec<f64>, Vec<f64>) =
        (0..n).filter(|&i| keep[i]).map(|i| (y_p[i], d_y[i])).unzip();
    let (a, b) = if xs.len() >= 64 { lsq2(&xs, &ys) } else { (a0, b0) };
    sig.exposure = (1.0 + a, b);
    sig.inliers = xs.len() as f64 / n as f64;
    let res_exp: Vec<f64> = (0..n).map(|i| d_y[i] - (a * y_p[i] + b)).collect();
    sig.r2_exposure = r2(&res_exp, sig.var_y);

    // ── chroma. R^2 per plane, variance-weighted (the earlier
    // concatenated form scored "nothing explained" as 0.5).
    //   shared: exposure's gain applied about 128 — what a global
    //           intensity gain does to chroma
    //   wb:     d_p = M_p . [Y, U, V] + o_p, the per-channel RGB gain
    //           conjugated into YUV (a per-plane affine in U alone
    //           cannot express it; that was the bug)
    let (var_u, var_v) = (var(&d_u), var(&d_v));
    let var_c = var_u + var_v;
    let chroma_r2 = |res_u: &[f64], res_v: &[f64]| -> f64 {
        if var_c > 1e-9 { (1.0 - (var(res_u) + var(res_v)) / var_c).max(0.0) } else { 0.0 }
    };
    let res_sh_u: Vec<f64> = (0..n).map(|i| d_u[i] - a * (u_p[i] - 128.0)).collect();
    let res_sh_v: Vec<f64> = (0..n).map(|i| d_v[i] - a * (v_p[i] - 128.0)).collect();
    sig.r2_chroma_shared = chroma_r2(&res_sh_u, &res_sh_v);
    let yuv1 = vec![y_p.clone(), u_p.clone(), v_p.clone(), vec![1.0; n]];
    let fit_plane = |d: &[f64]| -> Vec<f64> {
        match lsq_basis(&yuv1, d) {
            Some(m) => (0..n).map(|i| d[i] - (m[0] * y_p[i] + m[1] * u_p[i] + m[2] * v_p[i] + m[3])).collect(),
            None => d.to_vec(),
        }
    };
    let (res_wb_u, res_wb_v) = (fit_plane(&d_u), fit_plane(&d_v));
    sig.r2_chroma_perplane = chroma_r2(&res_wb_u, &res_wb_v);

    // ── vignette: d_Y = (v0 + v1 r^2) Y + c
    let basis = vec![y_p.clone(), (0..n).map(|i| y_p[i] * rr[i]).collect(), vec![1.0; n]];
    if let Some(beta) = lsq_basis(&basis, &d_y) {
        let res: Vec<f64> = (0..n)
            .map(|i| d_y[i] - (beta[0] * y_p[i] + beta[1] * y_p[i] * rr[i] + beta[2]))
            .collect();
        sig.r2_vignette = r2(&res, sig.var_y);
        sig.vignette_v1 = beta[1];
    }

    // ── fog: d = t(row) (C - I), C shared across planes, t quadratic
    // in row. Linear in (t_k C_p) and (t_k): per plane, d_p =
    // sum_k rowbasis_k (T_k C_p - T_k I_p). Fit luma first for t and
    // C_Y jointly, then chroma C given t.
    {
        let rb: Vec<Vec<f64>> = (0..3).map(|k| row.iter().map(|r| r.powi(k as i32)).collect()).collect();
        // unknowns: g_k = t_k*C_Y (3), t_k (3): d_Y = sum g_k rb_k - sum t_k rb_k Y
        let mut basis: Vec<Vec<f64>> = rb.clone();
        for k in 0..3 {
            basis.push((0..n).map(|i| -rb[k][i] * y_p[i]).collect());
        }
        if let Some(beta) = lsq_basis(&basis, &d_y) {
            let t: Vec<f64> = (0..n).map(|i| (0..3).map(|k| beta[3 + k] * rb[k][i]).sum()).collect();
            let g: Vec<f64> = (0..n).map(|i| (0..3).map(|k| beta[k] * rb[k][i]).sum()).collect();
            let res_y: Vec<f64> = (0..n).map(|i| d_y[i] - (g[i] - t[i] * y_p[i])).collect();
            // C_Y as the ratio of the two fitted fields where t is
            // meaningful
            let (mut sg, mut st) = (0.0, 0.0);
            for i in 0..n {
                if t[i].abs() > 1e-3 {
                    sg += g[i];
                    st += t[i];
                }
            }
            let c_y = if st.abs() > 1e-9 { sg / st } else { 0.0 };
            // chroma: d_p = t (C_p - I_p) -> C_p by least squares given t
            let cfit = |ip: &[f64], dp: &[f64]| -> (f64, Vec<f64>) {
                let (mut num, mut den) = (0.0, 0.0);
                for i in 0..n {
                    num += t[i] * (dp[i] + t[i] * ip[i]);
                    den += t[i] * t[i];
                }
                let c = if den > 1e-9 { num / den } else { 128.0 };
                let res: Vec<f64> = (0..n).map(|i| dp[i] - t[i] * (c - ip[i])).collect();
                (c, res)
            };
            let (c_u, res_u) = cfit(&u_p, &d_u);
            let (c_v, res_v) = cfit(&v_p, &d_v);
            let total = sig.var_y + var_c;
            let resid = var(&res_y) + var(&res_u) + var(&res_v);
            sig.r2_fog = if total > 1e-9 { (1.0 - resid / total).max(0.0) } else { 0.0 };
            sig.fog_color = (c_y, c_u, c_v);
        }
    }

    // ── structure of what the best global fit leaves: lag-1
    // autocorrelation along x of the exposure residual on the sample grid
    {
        let cols = (cw + step - 1) / step;
        let (mut sxy, mut sxx) = (0.0, 0.0);
        let m = res_exp.iter().sum::<f64>() / n as f64;
        for i in 0..n {
            if (i + 1) % cols != 0 && i + 1 < n {
                sxy += (res_exp[i] - m) * (res_exp[i + 1] - m);
            }
            sxx += (res_exp[i] - m) * (res_exp[i] - m);
        }
        sig.lag1 = if sxx > 1e-9 { sxy / sxx } else { 0.0 };
    }

    // ── motion: energy removed by the best integer shift per 16x16 block
    {
        const B: usize = 16;
        const R: i32 = 4;
        let (mut e0, mut e1) = (0.0f64, 0.0f64);
        let mut by = 0;
        while by + B <= h {
            let mut bx = 0;
            while bx + B <= w {
                let mut sse0 = 0.0f64;
                for y in 0..B {
                    for x in 0..B {
                        let i = (by + y) * w + bx + x;
                        let d = target.y[i] as f64 - pred.y[i] as f64;
                        sse0 += d * d;
                    }
                }
                let mut best = sse0;
                for oy in -R..=R {
                    for ox in -R..=R {
                        if ox == 0 && oy == 0 {
                            continue;
                        }
                        let (sx, sy) = (bx as i32 + ox, by as i32 + oy);
                        if sx < 0 || sy < 0 || sx as usize + B > w || sy as usize + B > h {
                            continue;
                        }
                        let mut sse = 0.0f64;
                        for y in 0..B {
                            for x in 0..B {
                                let ti = (by + y) * w + bx + x;
                                let pi = (sy as usize + y) * w + sx as usize + x;
                                let d = target.y[ti] as f64 - pred.y[pi] as f64;
                                sse += d * d;
                            }
                        }
                        if sse < best {
                            best = sse;
                        }
                    }
                }
                e0 += sse0;
                e1 += best;
                bx += B;
            }
            by += B;
        }
        sig.motion_frac = if e0 > 1e-9 { 1.0 - e1 / e0 } else { 0.0 };
    }
    sig
}

impl Signature {
    /// One line, fixed columns, for eyeballing across frames.
    pub fn row(&self) -> String {
        format!(
            "varY {:7.2} | exp {:.2} (a {:.3} b {:+.1} in {:.2}) | chroma shared {:.2} wb {:.2} | vig {:.2} (v1 {:+.3}) | fog {:.2} (C {:.0},{:.0},{:.0}) | lag1 {:.2} | motion {:.2}",
            self.var_y,
            self.r2_exposure,
            self.exposure.0,
            self.exposure.1,
            self.inliers,
            self.r2_chroma_shared,
            self.r2_chroma_perplane,
            self.r2_vignette,
            self.vignette_v1,
            self.r2_fog,
            self.fog_color.0,
            self.fog_color.1,
            self.fog_color.2,
            self.lag1,
            self.motion_frac
        )
    }
}

// ── Fog stage: the first engine-supplied stage in the chain.
//
// pixel = lit * (1 - t) + C * t,  t = clamp((depth - start) / (end - start))
//
// The world stage renders `lit` (no atmosphere) and the engine hands
// over per-pixel depth; the fog stage is then FIVE numbers per frame
// (a colour and two distances) instead of a residual across the
// picture. Fitted by grid search over (start, end) with the colour
// closed-form per plane, on a pixel subsample; refined locally. The
// blend is linear, so it commutes with the RGB->YUV transform and is
// applied in YUV directly, chroma at half resolution with depth
// box-averaged.

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FogStage {
    pub color: [u8; 3],
    pub start: f32,
    pub end: f32,
}

impl FogStage {
    pub const WIRE: usize = 3 + 2 + 2;

    fn t(&self, d: f32) -> f32 {
        if self.end <= self.start {
            return 0.0;
        }
        ((d - self.start) / (self.end - self.start)).clamp(0.0, 1.0)
    }

    pub fn apply(&self, lit: &Yuv420, depth: &[f32]) -> Yuv420 {
        let (w, h) = (lit.w, lit.h);
        let mut out = Yuv420::new(w, h);
        for i in 0..w * h {
            // no geometry: the world render already shows what is
            // there (sky / clear); atmosphere has nothing to blend
            let t = if depth[i] > 1.0e5 { 0.0 } else { self.t(depth[i]) };
            out.y[i] = (lit.y[i] as f32 * (1.0 - t) + self.color[0] as f32 * t).round().clamp(0.0, 255.0) as u8;
        }
        let cw = w / 2;
        for cy in 0..h / 2 {
            for cx in 0..cw {
                let d = (depth[(cy * 2) * w + cx * 2]
                    + depth[(cy * 2) * w + cx * 2 + 1]
                    + depth[(cy * 2 + 1) * w + cx * 2]
                    + depth[(cy * 2 + 1) * w + cx * 2 + 1])
                    / 4.0;
                let t = if d > 1.0e5 { 0.0 } else { self.t(d) };
                let ci = cy * cw + cx;
                out.u[ci] = (lit.u[ci] as f32 * (1.0 - t) + self.color[1] as f32 * t).round().clamp(0.0, 255.0) as u8;
                out.v[ci] = (lit.v[ci] as f32 * (1.0 - t) + self.color[2] as f32 * t).round().clamp(0.0, 255.0) as u8;
            }
        }
        out
    }

    /// Fit (colour, start, end) so that `apply(lit, depth)` matches
    /// `target`, on luma, over a subsample. `None` when no fog beats
    /// the identity by the gate (fraction of luma SSE removed).
    pub fn fit(lit: &Yuv420, depth: &[f32], target: &Yuv420) -> Option<FogStage> {
        // A renderer that culls past its fog end leaves clear colour
        // where the world render has geometry; those pixels obey no
        // blend and drag `end` inward. Fit, then refit excluding
        // pixels beyond the first fit's end plus a margin.
        let first = Self::fit_bounded(lit, depth, target, 1.0e5)?;
        Self::fit_bounded(lit, depth, target, first.end + 6.0).or(Some(first))
    }

    /// Cull margin past `end` at which this renderer stops drawing.
    pub const CULL_MARGIN: f32 = 6.0;

    fn fit_bounded(lit: &Yuv420, depth: &[f32], target: &Yuv420, max_depth: f32) -> Option<FogStage> {
        let (w, h) = (lit.w, lit.h);
        let step = 4usize;
        let mut ls = Vec::new();
        let mut ds = Vec::new();
        let mut ts = Vec::new();
        let mut us = Vec::new();
        let mut vs = Vec::new();
        let mut lu = Vec::new();
        let mut lv = Vec::new();
        let cw = w / 2;
        let mut y = 0;
        while y < h {
            let mut x = 0;
            while x < w {
                let i = y * w + x;
                // uncovered pixels (no geometry) carry no depth
                if !depth[i].is_finite() || depth[i] > max_depth {
                    x += step;
                    continue;
                }
                ls.push(lit.y[i] as f32);
                ds.push(depth[i]);
                ts.push(target.y[i] as f32);
                let ci = (y / 2) * cw + x / 2;
                lu.push(lit.u[ci] as f32);
                lv.push(lit.v[ci] as f32);
                us.push(target.u[ci] as f32);
                vs.push(target.v[ci] as f32);
                x += step;
            }
            y += step;
        }
        let n = ls.len();
        if n < 256 {
            return None;
        }
        let sse0: f64 = (0..n).map(|i| ((ts[i] - ls[i]) as f64).powi(2)).sum();
        // closed-form colour and SSE for a given (start, end), luma
        let eval = |start: f32, end: f32| -> (f64, f32) {
            let st = FogStage { color: [0; 3], start, end };
            let (mut num, mut den) = (0.0f64, 0.0f64);
            for i in 0..n {
                let t = st.t(ds[i]) as f64;
                num += t * (ts[i] as f64 - ls[i] as f64 * (1.0 - t));
                den += t * t;
            }
            if den < 1e-9 {
                return (sse0, 0.0);
            }
            let c = (num / den).clamp(0.0, 255.0);
            let sse: f64 = (0..n)
                .map(|i| {
                    let t = st.t(ds[i]) as f64;
                    (ts[i] as f64 - (ls[i] as f64 * (1.0 - t) + c * t)).powi(2)
                })
                .sum();
            (sse, c as f32)
        };
        let dmax = ds.iter().cloned().fold(0.0f32, f32::max).clamp(10.0, 1.0e4);
        let mut best = (sse0, 0.0f32, 0.0f32, 0.0f32);
        let mut start = 0.0f32;
        while start < dmax {
            let mut end = start + 5.0;
            while end <= dmax * 1.5 {
                let (sse, c) = eval(start, end);
                if sse < best.0 {
                    best = (sse, start, end, c);
                }
                end += dmax / 20.0;
            }
            start += dmax / 20.0;
        }
        // local refinement
        for _ in 0..2 {
            let (_, s0, e0, _) = best;
            let ds_ = dmax / 80.0;
            for si in -2..=2 {
                for ei in -2..=2 {
                    let (s, e) = (s0 + si as f32 * ds_, e0 + ei as f32 * ds_);
                    if s < 0.0 || e <= s + 1.0 {
                        continue;
                    }
                    let (sse, c) = eval(s, e);
                    if sse < best.0 {
                        best = (sse, s, e, c);
                    }
                }
            }
        }
        let (sse, start, end, c_y) = best;
        let gate: f64 = std::env::var("FAO_FOG_GATE").ok().and_then(|v| v.parse().ok()).unwrap_or(0.05);
        if sse0 <= 1e-9 || (sse0 - sse) / sse0 < gate {
            return None;
        }
        // chroma colours given t
        let st = FogStage { color: [0; 3], start, end };
        let cfit = |lp: &[f32], tp: &[f32]| -> u8 {
            let (mut num, mut den) = (0.0f64, 0.0f64);
            for i in 0..n {
                let t = st.t(ds[i]) as f64;
                num += t * (tp[i] as f64 - lp[i] as f64 * (1.0 - t));
                den += t * t;
            }
            if den < 1e-9 { 128 } else { (num / den).round().clamp(0.0, 255.0) as u8 }
        };
        Some(FogStage {
            color: [c_y.round() as u8, cfit(&lu, &us), cfit(&lv, &vs)],
            start,
            end,
        })
    }
}
