//! Direct least-squares ellipse fit (Fitzgibbon, Pilu and Fisher 1999, in the numerically stable form of Halir and
//! Flusser 1998): what `cv2.fitEllipse` does with a contour. It minimises the algebraic distance to the points under the
//! constraint 4AC - B^2 = 1, which only an ellipse satisfies, so the result is always an ellipse.

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Ellipse {
    pub cx: f64,
    pub cy: f64,
    /// Full axis lengths, major first.
    pub major: f64,
    pub minor: f64,
    /// The major axis' angle from +x, degrees, in (-90, 90].
    pub angle_deg: f64,
}

type M3 = [[f64; 3]; 3];

fn mul(a: &M3, b: &M3) -> M3 {
    let mut r = [[0.0; 3]; 3];
    for i in 0..3 { for j in 0..3 { for k in 0..3 { r[i][j] += a[i][k] * b[k][j]; } } }
    r
}

fn det(m: &M3) -> f64 {
    m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1]) - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0]) + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0])
}

fn inv(m: &M3) -> Option<M3> {
    let d = det(m);
    if d.abs() < 1e-300 || !d.is_finite() { return None; }
    let c = |r1: usize, c1: usize, r2: usize, c2: usize| m[r1][c1] * m[r2][c2] - m[r1][c2] * m[r2][c1];
    Some([
        [c(1, 1, 2, 2) / d, -c(0, 1, 2, 2) / d, c(0, 1, 1, 2) / d],
        [-c(1, 0, 2, 2) / d, c(0, 0, 2, 2) / d, -c(0, 0, 1, 2) / d],
        [c(1, 0, 2, 1) / d, -c(0, 0, 2, 1) / d, c(0, 0, 1, 1) / d],
    ])
}

/// Real eigenvalues of a 3x3 matrix, from its characteristic cubic.
fn eigenvalues(m: &M3) -> Vec<f64> {
    let tr = m[0][0] + m[1][1] + m[2][2];
    let minors = (m[0][0] * m[1][1] - m[0][1] * m[1][0]) + (m[0][0] * m[2][2] - m[0][2] * m[2][0]) + (m[1][1] * m[2][2] - m[1][2] * m[2][1]);
    let d = det(m);
    // l^3 - tr l^2 + minors l - d = 0, depressed with l = t + tr/3
    let (p, q) = (minors - tr * tr / 3.0, -2.0 * tr * tr * tr / 27.0 + tr * minors / 3.0 - d);
    let shift = tr / 3.0;
    let disc = (q / 2.0).powi(2) + (p / 3.0).powi(3);
    if disc > 0.0 {
        let s = disc.sqrt();
        vec![(-q / 2.0 + s).cbrt() + (-q / 2.0 - s).cbrt() + shift]
    } else {
        let r = (-(p / 3.0).powi(3)).sqrt();
        if r == 0.0 { return vec![shift]; }
        let phi = (-q / (2.0 * r)).clamp(-1.0, 1.0).acos();
        let k = 2.0 * (-p / 3.0).sqrt();
        (0..3).map(|j| k * ((phi + 2.0 * std::f64::consts::PI * j as f64) / 3.0).cos() + shift).collect()
    }
}

/// A null vector of (m - l I): the cross product of two of its rows (the best-conditioned pair).
fn null_vector(m: &M3, l: f64) -> Option<[f64; 3]> {
    let a: M3 = [[m[0][0] - l, m[0][1], m[0][2]], [m[1][0], m[1][1] - l, m[1][2]], [m[2][0], m[2][1], m[2][2] - l]];
    let cross = |u: [f64; 3], v: [f64; 3]| [u[1] * v[2] - u[2] * v[1], u[2] * v[0] - u[0] * v[2], u[0] * v[1] - u[1] * v[0]];
    let cands = [cross(a[0], a[1]), cross(a[0], a[2]), cross(a[1], a[2])];
    let best = cands.into_iter().max_by(|x, y| norm(x).total_cmp(&norm(y)))?;
    let n = norm(&best);
    (n > 1e-12).then(|| [best[0] / n, best[1] / n, best[2] / n])
}

fn norm(v: &[f64; 3]) -> f64 { (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt() }

/// The direct fit: an ellipse to at least six points, whatever they are. None when they are degenerate (collinear, all equal).
pub fn fit_direct(points: &[(f64, f64)]) -> Option<Ellipse> {
    if points.len() < 6 { return None; }
    let n = points.len() as f64;
    let (mx, my) = (points.iter().map(|p| p.0).sum::<f64>() / n, points.iter().map(|p| p.1).sum::<f64>() / n);
    let scale = (points.iter().map(|p| (p.0 - mx).powi(2) + (p.1 - my).powi(2)).sum::<f64>() / n).sqrt();
    if scale < 1e-9 { return None; }
    // normalised design sums: D1 = [x^2, xy, y^2], D2 = [x, y, 1]
    let (mut s1, mut s2, mut s3): (M3, M3, M3) = ([[0.0; 3]; 3], [[0.0; 3]; 3], [[0.0; 3]; 3]);
    for &(px, py) in points {
        let (x, y) = ((px - mx) / scale, (py - my) / scale);
        let d1 = [x * x, x * y, y * y];
        let d2 = [x, y, 1.0];
        for i in 0..3 { for j in 0..3 { s1[i][j] += d1[i] * d1[j]; s2[i][j] += d1[i] * d2[j]; s3[i][j] += d2[i] * d2[j]; } }
    }
    let t = {
        let i3 = inv(&s3)?;
        let s2t: M3 = { let mut r = [[0.0; 3]; 3]; for i in 0..3 { for j in 0..3 { r[i][j] = s2[j][i]; } } r };
        let p = mul(&i3, &s2t);
        let mut r = [[0.0; 3]; 3];
        for i in 0..3 { for j in 0..3 { r[i][j] = -p[i][j]; } }
        r
    };
    let mut m = mul(&s2, &t);
    for i in 0..3 { for j in 0..3 { m[i][j] += s1[i][j]; } }
    // premultiply by the inverse of the constraint matrix C1 = [[0,0,2],[0,-1,0],[2,0,0]]
    let m = [[m[2][0] / 2.0, m[2][1] / 2.0, m[2][2] / 2.0], [-m[1][0], -m[1][1], -m[1][2]], [m[0][0] / 2.0, m[0][1] / 2.0, m[0][2] / 2.0]];
    let a1 = eigenvalues(&m).into_iter().filter_map(|l| null_vector(&m, l)).find(|v| 4.0 * v[0] * v[2] - v[1] * v[1] > 0.0)?;
    let a2 = [
        t[0][0] * a1[0] + t[0][1] * a1[1] + t[0][2] * a1[2],
        t[1][0] * a1[0] + t[1][1] * a1[1] + t[1][2] * a1[2],
        t[2][0] * a1[0] + t[2][1] * a1[1] + t[2][2] * a1[2],
    ];
    conic_to_ellipse(a1[0], a1[1], a1[2], a2[0], a2[1], a2[2], mx, my, scale)
}

/// The conic a x^2 + b xy + c y^2 + d x + e y + f = 0, in coordinates centred on (mx, my) and divided by `scale`.
fn conic_to_ellipse(a: f64, b: f64, c: f64, d: f64, e: f64, f: f64, mx: f64, my: f64, scale: f64) -> Option<Ellipse> {
    let den = b * b - 4.0 * a * c;
    if den >= 0.0 { return None; }
    let (x0, y0) = ((2.0 * c * d - b * e) / den, (2.0 * a * e - b * d) / den);
    let fc = f + (d * x0 + e * y0) / 2.0;
    // quadratic form [[a, b/2], [b/2, c]]
    let (tr, dt) = (a + c, a * c - b * b / 4.0);
    let disc = (tr * tr / 4.0 - dt).max(0.0).sqrt();
    let (l1, l2) = (tr / 2.0 + disc, tr / 2.0 - disc);
    if l1 * fc >= 0.0 || l2 * fc >= 0.0 { return None; }
    let (r1, r2) = ((-fc / l1).sqrt(), (-fc / l2).sqrt());
    // eigenvector of l1 (the larger eigenvalue = the shorter semi-axis); the major axis is perpendicular to it
    let ang_minor = 0.5 * b.atan2(a - c);
    let (semi_major, semi_minor, ang) = if r2 >= r1 { (r2, r1, ang_minor + std::f64::consts::FRAC_PI_2) } else { (r1, r2, ang_minor) };
    let mut deg = ang.to_degrees();
    while deg > 90.0 { deg -= 180.0; }
    while deg <= -90.0 { deg += 180.0; }
    Some(Ellipse { cx: mx + x0 * scale, cy: my + y0 * scale, major: 2.0 * semi_major * scale, minor: 2.0 * semi_minor * scale, angle_deg: deg })
}

/// What `cv2.fitEllipse` does: least squares on a x^2 + b xy + c y^2 + d x + e y = 1, about the points' centroid. Not
/// constrained to an ellipse (None when the answer is a hyperbola or worse), and it weights an irregular outline
/// differently from the direct fit: on a blob that is not ellipse-shaped the two disagree by tens of pixels, so the
/// detector uses this one, to land where Orlosky's does. (Checked against OpenCV: identical centres on his test frames.)
pub fn fit_cv(points: &[(f64, f64)]) -> Option<Ellipse> {
    if points.len() < 5 { return None; }
    let n = points.len() as f64;
    let (mx, my) = (points.iter().map(|p| p.0).sum::<f64>() / n, points.iter().map(|p| p.1).sum::<f64>() / n);
    let scale = (points.iter().map(|p| (p.0 - mx).powi(2) + (p.1 - my).powi(2)).sum::<f64>() / n).sqrt();
    if scale < 1e-9 { return None; }
    // normal equations of A p = 1, A rows [x^2, xy, y^2, x, y]
    let mut m = [[0.0f64; 6]; 5];
    for &(px, py) in points {
        let (x, y) = ((px - mx) / scale, (py - my) / scale);
        let row = [x * x, x * y, y * y, x, y];
        for i in 0..5 { for j in 0..5 { m[i][j] += row[i] * row[j]; } m[i][5] += row[i]; }
    }
    // Gaussian elimination with partial pivoting
    for col in 0..5 {
        let piv = (col..5).max_by(|&a, &b| m[a][col].abs().total_cmp(&m[b][col].abs()))?;
        if m[piv][col].abs() < 1e-12 { return None; }
        m.swap(col, piv);
        for r in 0..5 {
            if r != col {
                let f = m[r][col] / m[col][col];
                for k in col..6 { m[r][k] -= f * m[col][k]; }
            }
        }
    }
    let p: Vec<f64> = (0..5).map(|i| m[i][5] / m[i][i]).collect();
    conic_to_ellipse(p[0], p[1], p[2], p[3], p[4], -1.0, mx, my, scale)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(cx: f64, cy: f64, a: f64, b: f64, deg: f64, n: usize) -> Vec<(f64, f64)> {
        let (s, c) = deg.to_radians().sin_cos();
        (0..n).map(|i| {
            let t = i as f64 / n as f64 * std::f64::consts::TAU;
            let (u, v) = (a * t.cos(), b * t.sin());
            (cx + u * c - v * s, cy + u * s + v * c)
        }).collect()
    }

    #[test]
    fn it_recovers_a_known_rotated_off_centre_ellipse_exactly() {
        for (cx, cy, a, b, deg) in [(320.0, 240.0, 60.0, 40.0, 30.0), (100.0, 400.0, 25.0, 24.0, -70.0), (500.0, 50.0, 90.0, 20.0, 0.0), (10.0, 10.0, 15.0, 9.0, 89.0)] {
            let e = fit_direct(&sample(cx, cy, a, b, deg, 60)).unwrap();
            assert!((e.cx - cx).abs() < 1e-6 && (e.cy - cy).abs() < 1e-6, "{e:?}");
            assert!((e.major - 2.0 * a).abs() < 1e-6 && (e.minor - 2.0 * b).abs() < 1e-6, "{e:?}");
            if (a - b).abs() > 2.0 {
                let d = (e.angle_deg - deg).rem_euclid(180.0);
                assert!(d.min(180.0 - d) < 1e-6, "angle {} vs {deg}", e.angle_deg);
            }
        }
    }

    #[test]
    fn it_survives_noise_and_a_partial_arc() {
        let mut pts = sample(300.0, 200.0, 50.0, 35.0, 20.0, 200);
        for (i, p) in pts.iter_mut().enumerate() { p.0 += ((i * 7919 % 13) as f64 - 6.0) * 0.1; p.1 += ((i * 104729 % 11) as f64 - 5.0) * 0.1; }
        let e = fit_direct(&pts).unwrap();
        assert!((e.cx - 300.0).abs() < 0.5 && (e.cy - 200.0).abs() < 0.5 && (e.major - 100.0).abs() < 1.5, "{e:?}");
        let arc = sample(300.0, 200.0, 50.0, 35.0, 20.0, 200)[..100].to_vec(); // half of the outline
        let e = fit_direct(&arc).unwrap();
        assert!((e.cx - 300.0).abs() < 1e-3 && (e.major - 100.0).abs() < 1e-3, "{e:?}");
    }

    #[test]
    fn the_opencv_style_fit_agrees_with_the_direct_fit_on_a_clean_ellipse_and_is_none_on_a_line() {
        let pts = sample(320.0, 240.0, 60.0, 40.0, 30.0, 80);
        let (a, b) = (fit_cv(&pts).unwrap(), fit_direct(&pts).unwrap());
        assert!((a.cx - b.cx).abs() < 1e-6 && (a.major - b.major).abs() < 1e-6 && (a.minor - 80.0).abs() < 1e-6, "{a:?} {b:?}");
        assert!(fit_cv(&(0..20).map(|i| (i as f64, 2.0 * i as f64)).collect::<Vec<_>>()).is_none());
        assert!(fit_cv(&[(0.0, 0.0), (1.0, 1.0), (2.0, 0.0), (3.0, 1.0)]).is_none(), "too few points");
    }

    #[test]
    fn degenerate_points_are_none_not_garbage() {
        assert!(fit_direct(&[(1.0, 1.0); 10]).is_none(), "all the same point");
        assert!(fit_direct(&(0..20).map(|i| (i as f64, 2.0 * i as f64)).collect::<Vec<_>>()).is_none(), "collinear");
        assert!(fit_direct(&sample(0.0, 0.0, 5.0, 3.0, 0.0, 5)).is_none(), "too few points");
    }
}
