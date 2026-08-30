//! lu.rs — dense LU with partial pivoting.
//!
//! Deliberately the *bootstrap* solver, not the destination. Every layer here ultimately wants a
//! sparse factorisation — power flow Jacobians have ~4 nonzeros per row, FEA stiffness matrices
//! are worse — and a dense solve is O(n³) time and O(n²) memory, which caps us around a thousand
//! unknowns.
//!
//! It exists first because it is *obviously correct*, and correctness has to be established before
//! performance. When the sparse solver lands, this becomes its differential oracle: same matrix,
//! same right-hand side, answers must agree to within the conditioning. That is a much stronger
//! test than checking a sparse solve against itself.
//!
//! Partial pivoting is not optional. Without it, a zero on the diagonal — which happens routinely
//! in a power flow Jacobian when a bus has no shunt — divides by zero and produces NaN rather than
//! an error.

/// A factorised matrix, owning its LU in packed form plus the row permutation.
#[derive(Clone, Debug)]
pub struct Lu {
    n: usize,
    /// L (unit diagonal, below) and U (on and above) packed into one n×n array, row-major.
    a: Vec<f64>,
    perm: Vec<usize>,
    /// Sign of the permutation, for the determinant.
    sign: f64,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum LuError {
    /// A pivot was zero (or below the tolerance) — the matrix is singular to working precision.
    Singular { column: usize },
    NotSquare,
    NonFinite,
}

impl Lu {
    /// Factor a row-major n×n matrix. Consumes a copy; the caller's matrix is untouched.
    pub fn factor(a_in: &[f64], n: usize) -> Result<Lu, LuError> {
        if a_in.len() != n * n {
            return Err(LuError::NotSquare);
        }
        if a_in.iter().any(|v| !v.is_finite()) {
            return Err(LuError::NonFinite);
        }
        let mut a = a_in.to_vec();
        let mut perm: Vec<usize> = (0..n).collect();
        let mut sign = 1.0;

        for k in 0..n {
            // Partial pivot: largest magnitude in the column at or below the diagonal.
            let mut piv = k;
            let mut best = a[k * n + k].abs();
            for i in k + 1..n {
                let v = a[i * n + k].abs();
                if v > best {
                    best = v;
                    piv = i;
                }
            }
            if best == 0.0 {
                return Err(LuError::Singular { column: k });
            }
            if piv != k {
                for j in 0..n {
                    a.swap(k * n + j, piv * n + j);
                }
                perm.swap(k, piv);
                sign = -sign;
            }
            let pivot = a[k * n + k];
            for i in k + 1..n {
                let m = a[i * n + k] / pivot;
                a[i * n + k] = m;
                if m != 0.0 {
                    for j in k + 1..n {
                        a[i * n + j] = f64::mul_add(-m, a[k * n + j], a[i * n + j]);
                    }
                }
            }
        }
        Ok(Lu { n, a, perm, sign })
    }

    /// Solve A x = b for x. The factorisation is reusable across right-hand sides, which is what
    /// makes it worth separating from `factor`.
    pub fn solve(&self, b: &[f64]) -> Vec<f64> {
        let n = self.n;
        let mut x = vec![0.0; n];
        // Apply the row permutation.
        for i in 0..n {
            x[i] = b[self.perm[i]];
        }
        // Forward substitution through L (unit diagonal).
        for i in 1..n {
            let mut s = x[i];
            for j in 0..i {
                s = f64::mul_add(-self.a[i * n + j], x[j], s);
            }
            x[i] = s;
        }
        // Back substitution through U.
        for i in (0..n).rev() {
            let mut s = x[i];
            for j in i + 1..n {
                s = f64::mul_add(-self.a[i * n + j], x[j], s);
            }
            x[i] = s / self.a[i * n + i];
        }
        x
    }

    /// ln|det A|, and the sign separately — the determinant itself overflows for any real problem.
    pub fn ln_det(&self) -> (f64, f64) {
        let mut ln = 0.0;
        let mut sign = self.sign;
        for i in 0..self.n {
            let d = self.a[i * self.n + i];
            ln += d.abs().ln();
            if d < 0.0 {
                sign = -sign;
            }
        }
        (ln, sign)
    }

    pub fn dim(&self) -> usize {
        self.n
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kernel::norm_inf;

    fn matvec(a: &[f64], x: &[f64], n: usize) -> Vec<f64> {
        (0..n)
            .map(|i| crate::sum::sum_pairwise(&(0..n).map(|j| a[i * n + j] * x[j]).collect::<Vec<_>>()))
            .collect()
    }

    #[test]
    fn solves_a_system_whose_answer_is_known() {
        // 3x3 with an exact integer solution.
        let a = vec![2.0, 1.0, -1.0, -3.0, -1.0, 2.0, -2.0, 1.0, 2.0];
        let b = vec![8.0, -11.0, -3.0];
        let x = Lu::factor(&a, 3).unwrap().solve(&b);
        for (got, want) in x.iter().zip([2.0, 3.0, -1.0]) {
            assert!((got - want).abs() < 1e-12, "{x:?}");
        }
    }

    #[test]
    fn pivoting_handles_a_zero_on_the_diagonal() {
        // Without partial pivoting this divides by zero on the first step.
        let a = vec![0.0, 1.0, 1.0, 1.0];
        let b = vec![1.0, 3.0];
        let x = Lu::factor(&a, 2).unwrap().solve(&b);
        assert!((x[0] - 2.0).abs() < 1e-12 && (x[1] - 1.0).abs() < 1e-12, "{x:?}");
    }

    #[test]
    fn residual_is_small_on_an_ill_conditioned_matrix() {
        // A Hilbert matrix: condition number ~1e13 at n=8, which is close to what f64 can carry.
        // We do not assert the *solution* is accurate — it cannot be, that is what conditioning
        // means — we assert the residual is small, which is what LU actually promises.
        let n = 8;
        let mut a = vec![0.0; n * n];
        for i in 0..n {
            for j in 0..n {
                a[i * n + j] = 1.0 / ((i + j + 1) as f64);
            }
        }
        let x_true: Vec<f64> = (0..n).map(|i| 1.0 + i as f64).collect();
        let b = matvec(&a, &x_true, n);
        let x = Lu::factor(&a, n).unwrap().solve(&b);
        let r = matvec(&a, &x, n);
        let resid: Vec<f64> = r.iter().zip(&b).map(|(p, q)| p - q).collect();
        assert!(norm_inf(&resid) < 1e-12, "residual {:e}", norm_inf(&resid));
    }

    #[test]
    fn a_singular_matrix_is_reported_rather_than_returning_nonsense() {
        let a = vec![1.0, 2.0, 2.0, 4.0]; // second row is twice the first
        assert_eq!(Lu::factor(&a, 2).unwrap_err(), LuError::Singular { column: 1 });
        assert_eq!(Lu::factor(&[1.0, 2.0], 2).unwrap_err(), LuError::NotSquare);
        assert_eq!(Lu::factor(&[f64::NAN, 0.0, 0.0, 1.0], 2).unwrap_err(), LuError::NonFinite);
    }

    #[test]
    fn one_factorisation_serves_many_right_hand_sides() {
        let a = vec![4.0, 1.0, 1.0, 3.0];
        let lu = Lu::factor(&a, 2).unwrap();
        for b in [[1.0, 2.0], [0.0, 1.0], [5.0, -3.0]] {
            let x = lu.solve(&b);
            let r = matvec(&a, &x, 2);
            assert!((r[0] - b[0]).abs() < 1e-13 && (r[1] - b[1]).abs() < 1e-13);
        }
    }

    #[test]
    fn ln_det_matches_a_hand_computed_determinant() {
        let a = vec![4.0, 1.0, 1.0, 3.0]; // det = 11
        let (ln, sign) = Lu::factor(&a, 2).unwrap().ln_det();
        assert!((ln.exp() - 11.0).abs() < 1e-10);
        assert_eq!(sign, 1.0);
        let b = vec![0.0, 1.0, 1.0, 0.0]; // det = -1
        let (ln, sign) = Lu::factor(&b, 2).unwrap().ln_det();
        assert!(ln.abs() < 1e-12);
        assert_eq!(sign, -1.0);
    }
}
