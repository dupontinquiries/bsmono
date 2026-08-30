//! # Two-view rotation from static tracks
//!
//! The affine layer measured its own limit: on the game-broadcast GOP it
//! removed roll/zoom (reprojection 1.67 → 1.11 px) but could not touch
//! yaw/pitch, whose image effect is a position-dependent shift the
//! affine's linear part cannot separate from pan. The correct model for
//! a rotating camera is the rotation-only homography H ≈ K·R·K⁻¹. This
//! module fits H robustly from static-track correspondences (normalized
//! DLT least squares, iterative median rejection — same discipline the
//! affine fitter needed) and extracts the nearest rotation to K⁻¹HK by
//! polar decomposition (Jacobi eigensolver on MᵀM; no external linalg).
//!
//! With per-pair rotations accumulated, observations can be rectified
//! into frame 0's orientation: unproject through K, rotate by R⁻¹,
//! reproject. What remains in the rectified tracks is translation
//! parallax only — the quantity triangulation is actually defined over.

/// Row-major 3×3.
pub type Mat3 = [f32; 9];

pub const M3_IDENTITY: Mat3 = [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0];

pub fn m3_mul(a: &Mat3, b: &Mat3) -> Mat3 {
    let mut o = [0.0f32; 9];
    for r in 0..3 {
        for c in 0..3 {
            o[r * 3 + c] = a[r * 3] * b[c] + a[r * 3 + 1] * b[3 + c] + a[r * 3 + 2] * b[6 + c];
        }
    }
    o
}

pub fn m3_transpose(a: &Mat3) -> Mat3 {
    [a[0], a[3], a[6], a[1], a[4], a[7], a[2], a[5], a[8]]
}

pub fn m3_apply(a: &Mat3, v: [f32; 3]) -> [f32; 3] {
    [
        a[0] * v[0] + a[1] * v[1] + a[2] * v[2],
        a[3] * v[0] + a[4] * v[1] + a[5] * v[2],
        a[6] * v[0] + a[7] * v[1] + a[8] * v[2],
    ]
}

fn m3_det(a: &Mat3) -> f32 {
    a[0] * (a[4] * a[8] - a[5] * a[7]) - a[1] * (a[3] * a[8] - a[5] * a[6])
        + a[2] * (a[3] * a[7] - a[4] * a[6])
}

/// Jacobi eigendecomposition of a SYMMETRIC 3×3: returns (eigenvalues,
/// eigenvectors as columns of V). Plenty for polar decomposition; ~10
/// sweeps reach f32 machine precision.
pub fn sym_eigen(a: &Mat3) -> ([f32; 3], Mat3) {
    let mut m = *a;
    let mut v = M3_IDENTITY;
    for _ in 0..30 {
        // Largest off-diagonal.
        let pairs = [(0usize, 1usize), (0, 2), (1, 2)];
        let (mut p, mut q, mut mx) = (0usize, 1usize, 0.0f32);
        for &(i, j) in &pairs {
            if m[i * 3 + j].abs() > mx {
                mx = m[i * 3 + j].abs();
                p = i;
                q = j;
            }
        }
        if mx < 1e-9 {
            break;
        }
        let app = m[p * 3 + p];
        let aqq = m[q * 3 + q];
        let apq = m[p * 3 + q];
        // Jacobi rotation angle: θ = ½·atan2(2·a_pq, a_pp − a_qq).
        let t = 0.5 * (2.0 * apq).atan2(app - aqq);
        let (s, c) = t.sin_cos();
        // Build rotation G in the (p,q) plane and apply m = GᵀmG, v = vG.
        let mut g = M3_IDENTITY;
        g[p * 3 + p] = c;
        g[q * 3 + q] = c;
        g[p * 3 + q] = s;
        g[q * 3 + p] = -s;
        m = m3_mul(&m3_transpose(&g), &m3_mul(&m, &g));
        v = m3_mul(&v, &g);
    }
    ([m[0], m[4], m[8]], v)
}

/// Nearest rotation to M (polar decomposition): R = M·(MᵀM)^(−1/2).
/// Assumes det(M) > 0 (caller normalizes scale/sign).
pub fn nearest_rotation(m: &Mat3) -> Mat3 {
    let mtm = m3_mul(&m3_transpose(m), m);
    let (vals, vecs) = sym_eigen(&mtm);
    // (MᵀM)^(-1/2) = V · diag(1/√λ) · Vᵀ
    let mut d = [0.0f32; 9];
    for i in 0..3 {
        d[i * 3 + i] = 1.0 / vals[i].max(1e-12).sqrt();
    }
    let inv_sqrt = m3_mul(&vecs, &m3_mul(&d, &m3_transpose(&vecs)));
    m3_mul(m, &inv_sqrt)
}

/// Rotation angle (radians) of a rotation matrix.
pub fn rotation_angle(r: &Mat3) -> f32 {
    let tr = (r[0] + r[4] + r[8]).clamp(-1.0, 3.0);
    ((tr - 1.0) * 0.5).clamp(-1.0, 1.0).acos()
}

/// Least-squares homography (h33 = 1) with Hartley normalization and the
/// same iterative median rejection the affine fitter needed. Pairs are
/// (x, y, x', y') in pixels. Returns None under 8 usable pairs.
pub fn fit_homography(pairs: &[(f32, f32, f32, f32)]) -> Option<Mat3> {
    if pairs.len() < 8 {
        return None;
    }
    // Hartley normalization: center + scale each side to mean distance √2.
    let norm = |get: &dyn Fn(&(f32, f32, f32, f32)) -> (f32, f32)| -> (f32, f32, f32) {
        let n = pairs.len() as f32;
        let (mut cx, mut cy) = (0.0f32, 0.0f32);
        for p in pairs {
            let (x, y) = get(p);
            cx += x;
            cy += y;
        }
        cx /= n;
        cy /= n;
        let mut md = 0.0f32;
        for p in pairs {
            let (x, y) = get(p);
            md += ((x - cx).powi(2) + (y - cy).powi(2)).sqrt();
        }
        md /= n;
        let s = if md > 1e-6 { std::f32::consts::SQRT_2 / md } else { 1.0 };
        (cx, cy, s)
    };
    let (cx0, cy0, s0) = norm(&|p| (p.0, p.1));
    let (cx1, cy1, s1) = norm(&|p| (p.2, p.3));

    let solve = |keep: &dyn Fn(usize) -> bool| -> Option<Mat3> {
        // 8 unknowns h11..h32 (h33 = 1), two equations per pair:
        //  x'(h31 x + h32 y + 1) = h11 x + h12 y + h13
        //  y'(h31 x + h32 y + 1) = h21 x + h22 y + h23
        let mut ata = [[0.0f64; 8]; 8];
        let mut atb = [0.0f64; 8];
        let mut count = 0usize;
        for (i, &(x, y, xp, yp)) in pairs.iter().enumerate() {
            if !keep(i) {
                continue;
            }
            count += 1;
            let (x, y) = ((x - cx0) * s0, (y - cy0) * s0);
            let (xp, yp) = ((xp - cx1) * s1, (yp - cy1) * s1);
            let rows: [([f64; 8], f64); 2] = [
                (
                    [x as f64, y as f64, 1.0, 0.0, 0.0, 0.0, -(xp * x) as f64, -(xp * y) as f64],
                    xp as f64,
                ),
                (
                    [0.0, 0.0, 0.0, x as f64, y as f64, 1.0, -(yp * x) as f64, -(yp * y) as f64],
                    yp as f64,
                ),
            ];
            for (row, b) in &rows {
                for a in 0..8 {
                    atb[a] += row[a] * b;
                    for bcol in a..8 {
                        ata[a][bcol] += row[a] * row[bcol];
                    }
                }
            }
        }
        if count < 8 {
            return None;
        }
        for a in 0..8 {
            for b in 0..a {
                ata[a][b] = ata[b][a];
            }
            ata[a][a] += 1e-9;
        }
        // Gaussian elimination, 8×8.
        let mut aa = ata;
        let mut bb = atb;
        for col in 0..8 {
            let mut piv = col;
            for r in col + 1..8 {
                if aa[r][col].abs() > aa[piv][col].abs() {
                    piv = r;
                }
            }
            if aa[piv][col].abs() < 1e-12 {
                return None;
            }
            aa.swap(col, piv);
            bb.swap(col, piv);
            for r in col + 1..8 {
                let f = aa[r][col] / aa[col][col];
                if f == 0.0 {
                    continue;
                }
                for c in col..8 {
                    aa[r][c] -= f * aa[col][c];
                }
                bb[r] -= f * bb[col];
            }
        }
        for col in (0..8).rev() {
            let mut s = bb[col];
            for c in col + 1..8 {
                s -= aa[col][c] * bb[c];
            }
            bb[col] = s / aa[col][col];
        }
        // Denormalize: H = T1⁻¹ · Hn · T0
        let hn: Mat3 = [
            bb[0] as f32, bb[1] as f32, bb[2] as f32,
            bb[3] as f32, bb[4] as f32, bb[5] as f32,
            bb[6] as f32, bb[7] as f32, 1.0,
        ];
        let t0: Mat3 = [s0, 0.0, -s0 * cx0, 0.0, s0, -s0 * cy0, 0.0, 0.0, 1.0];
        let t1_inv: Mat3 = [1.0 / s1, 0.0, cx1, 0.0, 1.0 / s1, cy1, 0.0, 0.0, 1.0];
        Some(m3_mul(&t1_inv, &m3_mul(&hn, &t0)))
    };

    let project = |h: &Mat3, x: f32, y: f32| -> (f32, f32) {
        let p = m3_apply(h, [x, y, 1.0]);
        let w = if p[2].abs() > 1e-9 { p[2] } else { 1e-9 };
        (p[0] / w, p[1] / w)
    };
    let mut model = solve(&|_| true)?;
    for _ in 0..3 {
        let res: Vec<f32> = pairs
            .iter()
            .map(|&(x, y, xp, yp)| {
                let (px, py) = project(&model, x, y);
                ((px - xp).powi(2) + (py - yp).powi(2)).sqrt()
            })
            .collect();
        let mut sorted = res.clone();
        sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let thr = (3.0 * sorted[sorted.len() / 2]).max(2.0);
        match solve(&|i| res[i] <= thr) {
            Some(m) => model = m,
            None => break,
        }
    }
    Some(model)
}

/// Extract the nearest camera rotation from a (near-)rotation-only
/// homography: M = K⁻¹·H·K, scale-normalized by det, polar-projected.
pub fn rotation_from_homography(h: &Mat3, fx: f32, fy: f32, cx: f32, cy: f32) -> Mat3 {
    let k: Mat3 = [fx, 0.0, cx, 0.0, fy, cy, 0.0, 0.0, 1.0];
    let k_inv: Mat3 = [1.0 / fx, 0.0, -cx / fx, 0.0, 1.0 / fy, -cy / fy, 0.0, 0.0, 1.0];
    let mut m = m3_mul(&k_inv, &m3_mul(h, &k));
    let d = m3_det(&m);
    // det(R) = 1: normalize scale (cube root) and sign.
    let s = d.abs().max(1e-12).powf(1.0 / 3.0) * d.signum();
    for v in m.iter_mut() {
        *v /= s;
    }
    nearest_rotation(&m)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn axis_angle(axis: [f32; 3], ang: f32) -> Mat3 {
        let n = (axis[0].powi(2) + axis[1].powi(2) + axis[2].powi(2)).sqrt();
        let (x, y, z) = (axis[0] / n, axis[1] / n, axis[2] / n);
        let (s, c) = ang.sin_cos();
        let t = 1.0 - c;
        [
            t * x * x + c, t * x * y - s * z, t * x * z + s * y,
            t * x * y + s * z, t * y * y + c, t * y * z - s * x,
            t * x * z - s * y, t * y * z + s * x, t * z * z + c,
        ]
    }

    /// Pure camera rotation: project rays through K before/after a known
    /// R; the fitted homography's extracted rotation must match R.
    #[test]
    fn recovers_pure_camera_rotation() {
        let (fx, fy, cx, cy) = (1000.0f32, 1000.0f32, 640.0f32, 360.0f32);
        let r_true = axis_angle([0.2, 1.0, 0.1], 0.03); // ~1.7°, mostly yaw
        let mut pairs = Vec::new();
        for i in 0..60 {
            // Rays across the field of view.
            let dx = ((i * 37 % 100) as f32 - 50.0) / 120.0;
            let dy = ((i * 91 % 80) as f32 - 40.0) / 120.0;
            let ray = [dx, dy, 1.0f32];
            let p0 = (fx * ray[0] / ray[2] + cx, fy * ray[1] / ray[2] + cy);
            // Camera rotates by R ⇒ scene ray in new camera = Rᵀ·ray.
            let r2 = m3_apply(&m3_transpose(&r_true), ray);
            let p1 = (fx * r2[0] / r2[2] + cx, fy * r2[1] / r2[2] + cy);
            pairs.push((p0.0, p0.1, p1.0, p1.1));
        }
        // 10% mover contamination.
        for i in 0..6 {
            pairs.push((400.0 + i as f32 * 3.0, 300.0, 900.0, 100.0));
        }
        let h = fit_homography(&pairs).expect("homography");
        let r = rotation_from_homography(&h, fx, fy, cx, cy);
        // Compare R to Rᵀ_true (the map recovered is the scene-ray map):
        let diff = m3_mul(&m3_transpose(&r), &m3_transpose(&r_true));
        let err = rotation_angle(&diff);
        assert!(err < 0.003, "rotation error {err} rad");
    }
}
