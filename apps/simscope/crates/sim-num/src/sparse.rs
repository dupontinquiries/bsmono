//! sparse.rs — CSR matrices and a conjugate-gradient solve.
//!
//! This is where the workspace stops being able to use dense LU. A 2D Laplace stiffness matrix has
//! ~7 nonzeros per row regardless of size; storing it dense is O(n²) memory and factorising it is
//! O(n³), which caps a mesh at a few thousand nodes. CG is O(nnz) per iteration and converges in
//! O(√κ) of them.
//!
//! **CG requires symmetric positive definite.** A Laplace operator with at least one Dirichlet
//! condition is SPD; a power flow Jacobian is not, which is why L0 uses LU and L1 uses this. That
//! is a property of the physics, not a preference, and `cg` checks what it can rather than
//! assuming.
//!
//! The inner products use `dot_compensated`. This is the precision policy from PLAN.md landing
//! somewhere concrete: CG's α and β are ratios of dot products, and near convergence the residual
//! is small and its inner product is mostly cancellation. Plain accumulation there stalls the
//! iteration at a residual floor well above what the problem can reach.

use crate::kernel::{dot_compensated, norm2};

/// Compressed sparse row. Built once from triplets, then read-only.
#[derive(Clone, Debug)]
pub struct Csr {
    pub n: usize,
    /// Row start offsets, length n+1.
    pub row: Vec<usize>,
    pub col: Vec<usize>,
    pub val: Vec<f64>,
}

impl Csr {
    /// Assemble from (row, col, value) triplets, summing duplicates — which is exactly what finite
    /// element assembly produces, since every interior node is touched by several elements.
    pub fn from_triplets(n: usize, triplets: &[(usize, usize, f64)]) -> Csr {
        let mut counts = vec![0usize; n + 1];
        for &(r, _, _) in triplets {
            counts[r + 1] += 1;
        }
        for i in 0..n {
            counts[i + 1] += counts[i];
        }
        let mut col = vec![0usize; triplets.len()];
        let mut val = vec![0.0; triplets.len()];
        let mut cursor = counts.clone();
        for &(r, c, v) in triplets {
            let p = cursor[r];
            col[p] = c;
            val[p] = v;
            cursor[r] += 1;
        }
        // Sort each row by column and merge duplicates, so the layout is canonical and SpMV is
        // deterministic in its accumulation order.
        let mut out_col = Vec::with_capacity(col.len());
        let mut out_val = Vec::with_capacity(val.len());
        let mut out_row = vec![0usize; n + 1];
        for r in 0..n {
            let (s, e) = (counts[r], counts[r + 1]);
            let mut entries: Vec<(usize, f64)> =
                (s..e).map(|p| (col[p], val[p])).collect();
            entries.sort_by_key(|(c, _)| *c);
            let mut k = 0;
            while k < entries.len() {
                let c = entries[k].0;
                let mut acc = 0.0;
                while k < entries.len() && entries[k].0 == c {
                    acc += entries[k].1;
                    k += 1;
                }
                if acc != 0.0 {
                    out_col.push(c);
                    out_val.push(acc);
                }
            }
            out_row[r + 1] = out_col.len();
        }
        Csr { n, row: out_row, col: out_col, val: out_val }
    }

    pub fn nnz(&self) -> usize {
        self.val.len()
    }

    /// y = A·x. Accumulation order within a row is the stored column order, which
    /// `from_triplets` made canonical — so this is reproducible regardless of how the caller
    /// assembled the triplets.
    pub fn spmv(&self, x: &[f64], y: &mut [f64]) {
        for r in 0..self.n {
            let mut acc = 0.0;
            for p in self.row[r]..self.row[r + 1] {
                acc = f64::mul_add(self.val[p], x[self.col[p]], acc);
            }
            y[r] = acc;
        }
    }

    pub fn diagonal(&self) -> Vec<f64> {
        let mut d = vec![0.0; self.n];
        for r in 0..self.n {
            for p in self.row[r]..self.row[r + 1] {
                if self.col[p] == r {
                    d[r] = self.val[p];
                }
            }
        }
        d
    }

    /// Largest asymmetry |A_ij − A_ji|. Zero for a proper stiffness matrix; a cheap guard against
    /// handing CG something it cannot solve.
    pub fn asymmetry(&self) -> f64 {
        let mut worst = 0.0f64;
        for r in 0..self.n {
            for p in self.row[r]..self.row[r + 1] {
                let c = self.col[p];
                let mut mirror = 0.0;
                for q in self.row[c]..self.row[c + 1] {
                    if self.col[q] == r {
                        mirror = self.val[q];
                    }
                }
                worst = worst.max((self.val[p] - mirror).abs());
            }
        }
        worst
    }

    pub fn to_dense(&self) -> Vec<f64> {
        let mut d = vec![0.0; self.n * self.n];
        for r in 0..self.n {
            for p in self.row[r]..self.row[r + 1] {
                d[r * self.n + self.col[p]] = self.val[p];
            }
        }
        d
    }
}

#[derive(Clone, Debug)]
pub struct CgReport {
    pub iterations: usize,
    pub residual: f64,
    pub converged: bool,
}

/// Jacobi-preconditioned conjugate gradient. `x` carries the initial guess in and the solution out.
///
/// The tolerance is relative to ‖b‖, which is the only scale-free thing to compare against.
pub fn cg(a: &Csr, b: &[f64], x: &mut [f64], tol: f64, max_iter: usize) -> CgReport {
    let n = a.n;
    let mut r = vec![0.0; n];
    let mut z = vec![0.0; n];
    let mut p = vec![0.0; n];
    let mut ap = vec![0.0; n];

    // Jacobi preconditioner: cheap, and for a Laplace operator on a graded mesh it is worth
    // several times its cost because it normalises the row scaling that element size introduces.
    let diag = a.diagonal();
    let minv: Vec<f64> = diag.iter().map(|d| if *d != 0.0 { 1.0 / d } else { 1.0 }).collect();

    a.spmv(x, &mut r);
    for i in 0..n {
        r[i] = b[i] - r[i];
    }
    let bnorm = norm2(b).max(f64::MIN_POSITIVE);
    let mut rz = {
        for i in 0..n {
            z[i] = minv[i] * r[i];
            p[i] = z[i];
        }
        dot_compensated(&r, &z)
    };

    for it in 0..max_iter {
        let rnorm = norm2(&r);
        if rnorm / bnorm <= tol {
            return CgReport { iterations: it, residual: rnorm / bnorm, converged: true };
        }
        a.spmv(&p, &mut ap);
        let pap = dot_compensated(&p, &ap);
        if !(pap > 0.0) {
            // Non-positive curvature means the matrix is not positive definite; CG's assumptions
            // are violated and continuing would produce numbers with no meaning.
            return CgReport { iterations: it, residual: rnorm / bnorm, converged: false };
        }
        let alpha = rz / pap;
        for i in 0..n {
            x[i] = f64::mul_add(alpha, p[i], x[i]);
            r[i] = f64::mul_add(-alpha, ap[i], r[i]);
        }
        for i in 0..n {
            z[i] = minv[i] * r[i];
        }
        let rz_next = dot_compensated(&r, &z);
        let beta = rz_next / rz;
        rz = rz_next;
        for i in 0..n {
            p[i] = f64::mul_add(beta, p[i], z[i]);
        }
    }
    let rnorm = norm2(&r);
    CgReport { iterations: max_iter, residual: rnorm / bnorm, converged: rnorm / bnorm <= tol }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lu::Lu;

    /// 1D Laplace with Dirichlet ends: the tridiagonal [-1, 2, -1], SPD, and the standard test
    /// operator for anything CG-shaped.
    fn laplace_1d(n: usize) -> Csr {
        let mut t = Vec::new();
        for i in 0..n {
            t.push((i, i, 2.0));
            if i > 0 {
                t.push((i, i - 1, -1.0));
            }
            if i + 1 < n {
                t.push((i, i + 1, -1.0));
            }
        }
        Csr::from_triplets(n, &t)
    }

    #[test]
    fn triplets_merge_duplicates_the_way_assembly_needs() {
        // Element assembly adds to the same entry repeatedly; the matrix must sum them, not keep
        // the last one.
        let a = Csr::from_triplets(2, &[(0, 0, 1.0), (0, 0, 2.0), (0, 1, -1.0), (1, 1, 4.0)]);
        assert_eq!(a.nnz(), 3);
        let d = a.to_dense();
        assert_eq!(d[0], 3.0, "duplicate entries must sum");
        assert_eq!(d[1], -1.0);
        assert_eq!(d[3], 4.0);
    }

    #[test]
    fn spmv_agrees_with_the_dense_product() {
        let a = laplace_1d(50);
        let x: Vec<f64> = (0..50).map(|i| ((i as f64) * 0.3).sin()).collect();
        let mut y = vec![0.0; 50];
        a.spmv(&x, &mut y);
        let d = a.to_dense();
        for r in 0..50 {
            let want: f64 = (0..50).map(|c| d[r * 50 + c] * x[c]).sum();
            assert!((y[r] - want).abs() < 1e-13, "row {r}");
        }
    }

    #[test]
    fn cg_agrees_with_a_direct_factorisation() {
        // The differential-oracle relationship the LU module was written for: two independent
        // solvers, same problem, answers must agree.
        let n = 120;
        let a = laplace_1d(n);
        let b: Vec<f64> = (0..n).map(|i| ((i as f64) * 0.11).cos()).collect();
        let direct = Lu::factor(&a.to_dense(), n).unwrap().solve(&b);
        let mut x = vec![0.0; n];
        let rep = cg(&a, &b, &mut x, 1e-14, 500);
        assert!(rep.converged, "{rep:?}");
        for i in 0..n {
            assert!((x[i] - direct[i]).abs() < 1e-9, "i={i}: cg {} lu {}", x[i], direct[i]);
        }
    }

    #[test]
    fn cg_refuses_an_indefinite_matrix_rather_than_returning_noise() {
        // Negating one diagonal makes the operator indefinite. CG's assumptions no longer hold and
        // it must say so, not iterate to max_iter and report a number.
        let mut t = vec![(0usize, 0usize, 1.0), (1, 1, -1.0)];
        t.push((0, 1, 0.0));
        let a = Csr::from_triplets(2, &t);
        let mut x = vec![0.0; 2];
        let rep = cg(&a, &[1.0, 1.0], &mut x, 1e-12, 50);
        assert!(!rep.converged);
        assert!(rep.iterations < 50, "should detect indefiniteness early, not spin");
    }

    #[test]
    fn asymmetry_detects_a_broken_stiffness_matrix() {
        assert!(laplace_1d(20).asymmetry() < 1e-15);
        let bad = Csr::from_triplets(2, &[(0, 1, 1.0), (1, 0, 2.0)]);
        assert!((bad.asymmetry() - 1.0).abs() < 1e-15);
    }

    #[test]
    fn cg_iterations_scale_with_the_square_root_of_the_condition_number() {
        // For the 1D Laplace, kappa ~ (n/pi)^2, so iterations should grow ~linearly in n, not
        // quadratically. This is the property that makes CG worth using at all.
        let mut counts = Vec::new();
        for n in [50usize, 100, 200] {
            let a = laplace_1d(n);
            let b = vec![1.0; n];
            let mut x = vec![0.0; n];
            counts.push(cg(&a, &b, &mut x, 1e-10, 5000).iterations);
        }
        let growth = counts[2] as f64 / counts[0] as f64;
        assert!(growth < 8.0, "iterations grew {growth}x for a 4x size increase: {counts:?}");
    }
}
