//! p1.rs — linear triangles, and the integral everything else rests on.
//!
//! This is the whole of "the lower-level integrals" for a Laplace problem, and it fits on a page
//! because for a linear triangle the integrand is constant.
//!
//! On a triangle with vertices 1, 2, 3, the P1 shape function `N_i` is the unique linear function
//! that is 1 at vertex `i` and 0 at the other two. Writing `2A` for twice the signed area:
//!
//! ```text
//!   b_1 = y_2 - y_3      c_1 = x_3 - x_2
//!   b_2 = y_3 - y_1      c_2 = x_1 - x_3        grad N_i = (b_i, c_i) / 2A
//!   b_3 = y_1 - y_2      c_3 = x_2 - x_1
//! ```
//!
//! `grad N_i` is *constant over the element* — that is the entire reason P1 is easy. So the
//! element stiffness integral has no quadrature in it at all:
//!
//! ```text
//!   K_ij = eps * INTEGRAL grad N_i . grad N_j dA
//!        = eps * A * (b_i b_j + c_i c_j) / (2A)^2
//!        = eps * (b_i b_j + c_i c_j) / (4A)
//! ```
//!
//! Everything harder in FEA — quadratic elements, curved geometry, nonlinear materials — is this
//! same expression with a quadrature rule where the closed form used to be. Which is why this is
//! the right place to start: the integral is exact, so any error in the assembly, the boundary
//! conditions or the solve has nowhere to hide.

use sim_num::{cg, Csr};

use crate::mesh::Mesh;

/// A Dirichlet condition: node `i` is pinned to `value`.
#[derive(Clone, Copy, Debug)]
pub struct Fixed {
    pub node: usize,
    pub value: f64,
}

/// The gradient coefficients and area of one element.
pub struct Element {
    pub b: [f64; 3],
    pub c: [f64; 3],
    /// Signed area.
    pub area: f64,
}

impl Element {
    pub fn of(mesh: &Mesh, t: usize) -> Element {
        let [i, j, k] = mesh.tri[t];
        let (p, q, r) = (mesh.xy[i], mesh.xy[j], mesh.xy[k]);
        let two_a = mesh.double_area(t);
        Element {
            b: [q[1] - r[1], r[1] - p[1], p[1] - q[1]],
            c: [r[0] - q[0], p[0] - r[0], q[0] - p[0]],
            area: two_a / 2.0,
        }
    }

    /// The 3x3 element stiffness matrix, scaled by permittivity.
    pub fn stiffness(&self, eps: f64) -> [[f64; 3]; 3] {
        let mut k = [[0.0; 3]; 3];
        let f = eps / (4.0 * self.area);
        for i in 0..3 {
            for j in 0..3 {
                k[i][j] = f * (self.b[i] * self.b[j] + self.c[i] * self.c[j]);
            }
        }
        k
    }

    /// Gradient of a field given its three nodal values — constant over the element.
    pub fn gradient(&self, u: [f64; 3]) -> [f64; 2] {
        let s = 1.0 / (2.0 * self.area);
        [
            s * (self.b[0] * u[0] + self.b[1] * u[1] + self.b[2] * u[2]),
            s * (self.c[0] * u[0] + self.c[1] * u[1] + self.c[2] * u[2]),
        ]
    }
}

/// Assembled global system, before boundary conditions.
pub fn assemble(mesh: &Mesh, eps: impl Fn(usize) -> f64) -> Csr {
    let mut trip = Vec::with_capacity(mesh.tri.len() * 9);
    for t in 0..mesh.tri.len() {
        let e = Element::of(mesh, t);
        let ke = e.stiffness(eps(t));
        let n = mesh.tri[t];
        for i in 0..3 {
            for j in 0..3 {
                trip.push((n[i], n[j], ke[i][j]));
            }
        }
    }
    Csr::from_triplets(mesh.nodes(), &trip)
}

/// Apply Dirichlet conditions by elimination and solve.
///
/// Pinned rows and columns are replaced by the identity and their contribution moved to the
/// right-hand side. Symmetry is preserved — which is not cosmetic, it is what keeps the matrix
/// admissible to CG. The lazy alternative (zero the row only) breaks symmetry and silently turns
/// a solvable SPD system into one CG cannot handle.
pub fn solve_dirichlet(k: &Csr, fixed: &[Fixed], tol: f64) -> Result<Vec<f64>, String> {
    // Without at least one Dirichlet node the Laplace operator is singular: any constant is in
    // its null space, so the solution is determined only up to an additive constant. CG will not
    // catch this — with a zero right-hand side it returns the zero vector, which is *a* solution
    // and looks like success. Refuse structurally instead of relying on the solver to notice.
    if fixed.is_empty() {
        return Err("no Dirichlet condition: the potential is defined only up to a constant".into());
    }
    let n = k.n;
    let mut is_fixed = vec![false; n];
    let mut value = vec![0.0; n];
    for f in fixed {
        is_fixed[f.node] = true;
        value[f.node] = f.value;
    }

    let mut rhs = vec![0.0; n];
    // Move the known columns to the right-hand side.
    for r in 0..n {
        if is_fixed[r] {
            continue;
        }
        let mut acc = 0.0;
        for p in k.row[r]..k.row[r + 1] {
            let c = k.col[p];
            if is_fixed[c] {
                acc -= k.val[p] * value[c];
            }
        }
        rhs[r] = acc;
    }

    let mut trip = Vec::with_capacity(k.nnz());
    for r in 0..n {
        if is_fixed[r] {
            trip.push((r, r, 1.0));
            rhs[r] = value[r];
            continue;
        }
        for p in k.row[r]..k.row[r + 1] {
            let c = k.col[p];
            if !is_fixed[c] {
                trip.push((r, c, k.val[p]));
            }
        }
    }
    let a = Csr::from_triplets(n, &trip);
    if a.asymmetry() > 1e-9 {
        return Err(format!("system is not symmetric ({:.2e}); CG cannot solve it", a.asymmetry()));
    }

    let mut u = value.clone();
    let rep = cg(&a, &rhs, &mut u, tol, 20 * n.max(50));
    if !rep.converged {
        return Err(format!("CG did not converge: residual {:.2e} after {} iterations", rep.residual, rep.iterations));
    }
    Ok(u)
}

/// Field energy: W = 1/2 * INTEGRAL eps |grad u|^2 dA, summed over elements.
pub fn energy(mesh: &Mesh, u: &[f64], eps: impl Fn(usize) -> f64) -> f64 {
    let terms: Vec<f64> = (0..mesh.tri.len())
        .map(|t| {
            let e = Element::of(mesh, t);
            let n = mesh.tri[t];
            let g = e.gradient([u[n[0]], u[n[1]], u[n[2]]]);
            0.5 * eps(t) * (g[0] * g[0] + g[1] * g[1]) * e.area
        })
        .collect();
    sim_num::sum_pairwise(&terms)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn element_stiffness_rows_sum_to_zero() {
        // A constant potential produces no field, so K times the all-ones vector must vanish
        // exactly. This is the algebraic statement that the element has no spurious stiffness, and
        // it catches most sign and index errors in the b/c coefficients immediately.
        let m = Mesh::rectangle(1.7, 0.9, 3, 3);
        for t in 0..m.tri.len() {
            let k = Element::of(&m, t).stiffness(2.5);
            for row in k {
                assert!(row.iter().sum::<f64>().abs() < 1e-12, "row sum {:?}", row);
            }
        }
    }

    #[test]
    fn the_gradient_of_a_known_linear_field_is_exact() {
        // P1 reproduces linear fields exactly, so grad of (3 + 2x - 5y) must be (2, -5) in every
        // element, to roundoff.
        let m = Mesh::annulus(1.0, 2.0, 4, 12);
        for t in 0..m.tri.len() {
            let e = Element::of(&m, t);
            let n = m.tri[t];
            let u = [0usize, 1, 2].map(|k| {
                let p = m.xy[n[k]];
                3.0 + 2.0 * p[0] - 5.0 * p[1]
            });
            let g = e.gradient(u);
            assert!((g[0] - 2.0).abs() < 1e-10 && (g[1] + 5.0).abs() < 1e-10, "{g:?}");
        }
    }

    #[test]
    fn the_patch_test_passes() {
        // THE canonical finite element verification. Pin every boundary node to a linear field and
        // solve; the interior must reproduce that field exactly. A code that fails the patch test
        // is broken no matter how plausible its pictures look, because it cannot represent even a
        // uniform field — and a uniform field is what every problem looks like locally under
        // refinement, which is why passing it is a precondition for converging at all.
        let (nx, ny) = (7, 5);
        let m = Mesh::rectangle(2.0, 1.3, nx, ny);
        let exact = |p: [f64; 2]| 0.7 + 1.9 * p[0] - 2.3 * p[1];

        let mut fixed = Vec::new();
        for (i, p) in m.xy.iter().enumerate() {
            let on_edge = p[0].abs() < 1e-12
                || (p[0] - 2.0).abs() < 1e-12
                || p[1].abs() < 1e-12
                || (p[1] - 1.3).abs() < 1e-12;
            if on_edge {
                fixed.push(Fixed { node: i, value: exact(*p) });
            }
        }
        let k = assemble(&m, |_| 1.0);
        let u = solve_dirichlet(&k, &fixed, 1e-14).unwrap();
        let worst = m
            .xy
            .iter()
            .enumerate()
            .fold(0.0f64, |w, (i, p)| w.max((u[i] - exact(*p)).abs()));
        assert!(worst < 1e-10, "patch test failed by {worst:.3e}");
    }

    #[test]
    fn a_system_with_no_dirichlet_condition_is_refused_up_front() {
        // Pure Neumann Laplace is singular — every constant is in the null space. Notably CG does
        // NOT detect this: with a zero right-hand side it returns the zero vector at iteration
        // zero and reports success, because zero genuinely is a solution. It is just not the only
        // one. An earlier version of this test expected the solver to fail and was wrong about
        // where the error has to be caught.
        let m = Mesh::rectangle(1.0, 1.0, 4, 4);
        let k = assemble(&m, |_| 1.0);
        let err = solve_dirichlet(&k, &[], 1e-12).unwrap_err();
        assert!(err.contains("up to a constant"), "{err}");
        // One pinned node is enough to make it well posed.
        assert!(solve_dirichlet(&k, &[Fixed { node: 0, value: 0.0 }], 1e-12).is_ok());
    }

    #[test]
    fn energy_of_a_uniform_field_matches_the_hand_calculation() {
        // u = x over a w-by-h rectangle gives |grad u| = 1 everywhere, so W = 1/2 eps w h.
        let (w, h, eps) = (2.0, 1.5, 3.3);
        let m = Mesh::rectangle(w, h, 6, 4);
        let u: Vec<f64> = m.xy.iter().map(|p| p[0]).collect();
        let got = energy(&m, &u, |_| eps);
        assert!((got - 0.5 * eps * w * h).abs() < 1e-12, "{got}");
    }
}
