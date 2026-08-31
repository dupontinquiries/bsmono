//! # Global affine motion from track correspondences
//!
//! Phase correlation delivers one translation per frame pair — the model
//! v8's pose chain is built on, and the measured reason game-broadcast
//! GOPs triangulate poorly: an orbiting/rolling/zooming game camera
//! moves the image by an affinity, and forcing that through a
//! translation-only model turns rotation into phantom parallax (rays
//! meeting behind the camera, rejected candidates).
//!
//! This module fits the full 6-DOF affine x' = a·x + b·y + c,
//! y' = d·x + e·y + f from static-track correspondences by least squares
//! with one 3σ outlier-rejection refit. The LINEAR part (a,b,d,e) is the
//! roll/zoom/shear component no translation can express; the translation
//! part (c,f) remains a mixture of camera pan and mean parallax — their
//! rigorous separation needs depth and is the job of iterative SfM, not
//! this fitter. Rectifying observations by the linear part alone removes
//! exactly what the pose model cannot represent while preserving the
//! parallax that triangulation feeds on.

/// Row-major 2×3 affine: [a, b, c, d, e, f].
pub type Affine = [f32; 6];

pub const IDENTITY: Affine = [1.0, 0.0, 0.0, 0.0, 1.0, 0.0];

pub fn apply(m: &Affine, x: f32, y: f32) -> (f32, f32) {
    (m[0] * x + m[1] * y + m[2], m[3] * x + m[4] * y + m[5])
}

/// Compose: result maps like `outer(inner(p))`.
pub fn compose(outer: &Affine, inner: &Affine) -> Affine {
    [
        outer[0] * inner[0] + outer[1] * inner[3],
        outer[0] * inner[1] + outer[1] * inner[4],
        outer[0] * inner[2] + outer[1] * inner[5] + outer[2],
        outer[3] * inner[0] + outer[4] * inner[3],
        outer[3] * inner[1] + outer[4] * inner[4],
        outer[3] * inner[2] + outer[4] * inner[5] + outer[5],
    ]
}

pub fn invert(m: &Affine) -> Option<Affine> {
    let det = m[0] * m[4] - m[1] * m[3];
    if det.abs() < 1e-9 {
        return None;
    }
    let (ia, ib, id, ie) = (m[4] / det, -m[1] / det, -m[3] / det, m[0] / det);
    Some([ia, ib, -(ia * m[2] + ib * m[5]), id, ie, -(id * m[2] + ie * m[5])])
}

/// Decompose the linear part into (rotation radians, scale, shear-ish
/// residual magnitude) for reporting.
pub fn describe(m: &Affine) -> (f32, f32) {
    let rot = m[3].atan2(m[0]);
    let scale = (m[0] * m[0] + m[3] * m[3]).sqrt();
    (rot, scale)
}

/// Least-squares affine from (x, y) → (x', y') pairs, one 3σ refit.
/// Returns None below 6 usable pairs (the system needs 3; 6 keeps it
/// honest against degenerate configurations).
pub fn fit(pairs: &[(f32, f32, f32, f32)]) -> Option<Affine> {
    if pairs.len() < 6 {
        return None;
    }
    let solve = |use_pair: &dyn Fn(usize) -> bool| -> Option<Affine> {
        // Two independent 3-unknown systems sharing the same normal matrix
        // on [x, y, 1].
        let mut n = [[0.0f64; 3]; 3];
        let mut bx = [0.0f64; 3];
        let mut by = [0.0f64; 3];
        let mut count = 0usize;
        for (i, &(x, y, xp, yp)) in pairs.iter().enumerate() {
            if !use_pair(i) {
                continue;
            }
            count += 1;
            let r = [x as f64, y as f64, 1.0];
            for a in 0..3 {
                bx[a] += r[a] * xp as f64;
                by[a] += r[a] * yp as f64;
                for b in a..3 {
                    n[a][b] += r[a] * r[b];
                }
            }
        }
        if count < 6 {
            return None;
        }
        for a in 0..3 {
            for b in 0..a {
                n[a][b] = n[b][a];
            }
        }
        // 3×3 solve twice (Cramer is fine at this size).
        let det3 = |m: &[[f64; 3]; 3]| -> f64 {
            m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1])
                - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0])
                + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0])
        };
        let d = det3(&n);
        if d.abs() < 1e-6 {
            return None;
        }
        let solve3 = |rhs: &[f64; 3]| -> [f64; 3] {
            let mut out = [0.0f64; 3];
            for k in 0..3 {
                let mut m = n;
                for r in 0..3 {
                    m[r][k] = rhs[r];
                }
                out[k] = det3(&m) / d;
            }
            out
        };
        let sx = solve3(&bx);
        let sy = solve3(&by);
        Some([sx[0] as f32, sx[1] as f32, sx[2] as f32, sy[0] as f32, sy[1] as f32, sy[2] as f32])
    };
    let mut model = solve(&|_| true)?;
    // Iterative MEDIAN-based rejection: mean+3σ collapses under
    // high-leverage outliers (a mover's correspondences both drag the
    // fit toward themselves AND inflate σ enough to survive the cut —
    // observed on the synthetic-mover test). The median residual is
    // robust to the contaminating minority; three rounds converge on
    // every case tried.
    for _ in 0..3 {
        let mut res: Vec<f32> = pairs
            .iter()
            .map(|&(x, y, xp, yp)| {
                let (px, py) = apply(&model, x, y);
                ((px - xp).powi(2) + (py - yp).powi(2)).sqrt()
            })
            .collect();
        let mut sorted = res.clone();
        sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let med = sorted[sorted.len() / 2];
        let thr = (3.0 * med).max(2.0);
        let keep: Vec<bool> = res.drain(..).map(|r| r <= thr).collect();
        match solve(&|i| keep[i]) {
            Some(m) => model = m,
            None => break,
        }
    }
    Some(model)
}

/// RMS residual of a pair set under a model.
pub fn rms(pairs: &[(f32, f32, f32, f32)], m: &Affine) -> f32 {
    if pairs.is_empty() {
        return 0.0;
    }
    let s: f32 = pairs
        .iter()
        .map(|&(x, y, xp, yp)| {
            let (px, py) = apply(m, x, y);
            (px - xp).powi(2) + (py - yp).powi(2)
        })
        .sum();
    (s / pairs.len() as f32).sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recovers_known_rotation_zoom() {
        // Ground truth: 2° roll + 1.5% zoom about (100, 80) + (5, -3) pan.
        let (th, sc) = (0.0349f32, 1.015f32);
        let (cx, cy) = (100.0f32, 80.0f32);
        let gt = |x: f32, y: f32| -> (f32, f32) {
            let (dx, dy) = (x - cx, y - cy);
            (
                cx + sc * (th.cos() * dx - th.sin() * dy) + 5.0,
                cy + sc * (th.sin() * dx + th.cos() * dy) - 3.0,
            )
        };
        let mut pairs = Vec::new();
        for i in 0..40 {
            let x = (i * 37 % 200) as f32;
            let y = (i * 91 % 160) as f32;
            let (xp, yp) = gt(x, y);
            pairs.push((x, y, xp, yp));
        }
        // Contaminate with 4 outliers (a "mover").
        for i in 0..4 {
            pairs.push((10.0 + i as f32, 20.0, 90.0, 140.0));
        }
        let m = fit(&pairs).expect("fit");
        let (rot, scale) = describe(&m);
        assert!((rot - th).abs() < 0.004, "rot {rot}");
        assert!((scale - sc).abs() < 0.01, "scale {scale}");
        let clean_rms = rms(&pairs[..40], &m);
        assert!(clean_rms < 0.6, "rms {clean_rms}");
    }
}
