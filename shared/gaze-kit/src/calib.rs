//! Affine calibration: eye measurements (a pupil position in the eye camera, or anything two-dimensional) to screen
//! fractions, fitted by least squares from points the person looked at. Six numbers, so three non-collinear points
//! are the minimum and nine (a 3x3 grid) average the noise.

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Affine {
    /// screen_x = a*x + b*y + c ; screen_y = d*x + e*y + f
    pub m: [f64; 6],
}

impl Affine {
    pub fn identity() -> Affine { Affine { m: [1.0, 0.0, 0.0, 0.0, 1.0, 0.0] } }

    pub fn apply(&self, x: f64, y: f64) -> (f64, f64) {
        (self.m[0] * x + self.m[1] * y + self.m[2], self.m[3] * x + self.m[4] * y + self.m[5])
    }

    /// The map as screen fractions, clamped to the screen.
    pub fn apply_clamped(&self, x: f64, y: f64) -> (f64, f64) {
        let (sx, sy) = self.apply(x, y);
        (sx.clamp(0.0, 1.0), sy.clamp(0.0, 1.0))
    }

    /// Fit from `(eye, screen)` pairs. None with fewer than three, or when the eye points are collinear (no unique map).
    pub fn fit(pairs: &[((f64, f64), (f64, f64))]) -> Option<Affine> {
        if pairs.len() < 3 { return None; }
        // normal equations of [x y 1] * w = target, solved for both outputs at once
        let mut ata = [[0.0f64; 3]; 3];
        let (mut atx, mut aty) = ([0.0f64; 3], [0.0f64; 3]);
        for &((x, y), (sx, sy)) in pairs {
            let r = [x, y, 1.0];
            for i in 0..3 {
                for j in 0..3 { ata[i][j] += r[i] * r[j]; }
                atx[i] += r[i] * sx;
                aty[i] += r[i] * sy;
            }
        }
        let wx = solve3(ata, atx)?;
        let wy = solve3(ata, aty)?;
        Some(Affine { m: [wx[0], wx[1], wx[2], wy[0], wy[1], wy[2]] })
    }

    /// Root-mean-square screen error of this map over `pairs`, in screen fractions.
    pub fn rms_error(&self, pairs: &[((f64, f64), (f64, f64))]) -> f64 {
        let s: f64 = pairs.iter().map(|&((x, y), (sx, sy))| { let (px, py) = self.apply(x, y); (px - sx).powi(2) + (py - sy).powi(2) }).sum();
        (s / pairs.len().max(1) as f64).sqrt()
    }
}

fn solve3(mut a: [[f64; 3]; 3], mut b: [f64; 3]) -> Option<[f64; 3]> {
    for col in 0..3 {
        let piv = (col..3).max_by(|&i, &j| a[i][col].abs().total_cmp(&a[j][col].abs()))?;
        if a[piv][col].abs() < 1e-9 { return None; }
        a.swap(col, piv);
        b.swap(col, piv);
        for r in 0..3 {
            if r != col {
                let f = a[r][col] / a[col][col];
                for k in col..3 { a[r][k] -= f * a[col][k]; }
                b[r] -= f * b[col];
            }
        }
    }
    Some([b[0] / a[0][0], b[1] / a[1][1], b[2] / a[2][2]])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grid(f: impl Fn(f64, f64) -> (f64, f64)) -> Vec<((f64, f64), (f64, f64))> {
        let mut v = Vec::new();
        for i in 0..3 { for j in 0..3 {
            let (sx, sy) = (i as f64 * 0.5, j as f64 * 0.5);
            v.push((f(sx, sy), (sx, sy)));
        } }
        v
    }

    #[test]
    fn it_recovers_a_known_map_from_a_nine_point_grid() {
        // eye = a rotated, scaled, shifted view of the screen
        let eye_of = |sx: f64, sy: f64| (300.0 + 180.0 * sx + 25.0 * sy, 210.0 - 12.0 * sx + 140.0 * sy);
        let pairs = grid(eye_of);
        let cal = Affine::fit(&pairs).unwrap();
        assert!(cal.rms_error(&pairs) < 1e-9, "{}", cal.rms_error(&pairs));
        let (x, y) = eye_of(0.3, 0.7);
        let (sx, sy) = cal.apply(x, y);
        assert!((sx - 0.3).abs() < 1e-9 && (sy - 0.7).abs() < 1e-9, "({sx},{sy})");
    }

    #[test]
    fn noisy_calibration_points_are_averaged_and_the_output_is_clamped_to_the_screen() {
        let mut pairs = grid(|sx, sy| (300.0 + 180.0 * sx, 210.0 + 140.0 * sy));
        for (i, p) in pairs.iter_mut().enumerate() { p.0 .0 += ((i * 37 % 7) as f64 - 3.0) * 1.5; p.0 .1 += ((i * 53 % 5) as f64 - 2.0) * 1.5; }
        let cal = Affine::fit(&pairs).unwrap();
        assert!(cal.rms_error(&pairs) < 0.03, "{}", cal.rms_error(&pairs));
        assert_eq!(cal.apply_clamped(-1.0e6, 1.0e6), (0.0, 1.0), "looking far off the edge pins to the edge");
    }

    #[test]
    fn too_few_or_collinear_points_are_none() {
        assert!(Affine::fit(&[((0.0, 0.0), (0.0, 0.0)), ((1.0, 1.0), (1.0, 1.0))]).is_none());
        let line: Vec<_> = (0..5).map(|i| ((i as f64, 2.0 * i as f64), (i as f64 / 4.0, 0.5))).collect();
        assert!(Affine::fit(&line).is_none(), "the eye never left a line: no unique map");
    }
}
