//! elastic.rs — 2D linear elasticity on constant-strain triangles.
//!
//! The same triangle as `p1`, with two unknowns per node instead of one. That is not a coincidence
//! worth glossing over: the geometric coefficients `b_i` and `c_i` are identical, because in both
//! problems they are just the gradients of the shape functions. Laplace contracts them into a
//! scalar; elasticity arranges them into a strain–displacement matrix. Everything else — assembly,
//! Dirichlet elimination, the CG solve — is reused unchanged.
//!
//! ```text
//!            [ b1  0  b2  0  b3  0 ]
//!   B = 1/2A [  0 c1   0 c2   0 c3 ]        eps = B u,   sigma = D eps
//!            [ c1 b1  c2 b2  c3 b3 ]
//! ```
//!
//! `B` is constant over the element, so `K_e = t·A·Bᵀ D B` needs no quadrature — the same reason
//! P1 Laplace was exact. The cost is that strain is *constant per element*, which is why these are
//! called constant-strain triangles and why they are poor in bending: a linear displacement field
//! cannot represent the curvature a beam actually has, so a coarse mesh comes out too stiff. That
//! is a real limitation, it converges away, and the tests measure the convergence rather than
//! pretending it is not there.

use sim_mat::{Elastic, Material};
use sim_num::{cg, Csr};

use crate::mesh::Mesh;
use crate::p1::Element;

/// Which two-dimensional idealisation is being made.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Assumption {
    /// Thin in z, free to contract: out-of-plane stress is zero. A plate, a PCB, a membrane.
    PlaneStress,
    /// Long in z, constrained: out-of-plane strain is zero. A dam, a buried pipe, a cross-section
    /// of something extruded. Stiffer than plane stress for the same material, because the
    /// constraint prevents Poisson contraction.
    PlaneStrain,
}

/// The 3×3 constitutive matrix relating stress to engineering strain.
pub fn constitutive(e: f64, nu: f64, mode: Assumption) -> [[f64; 3]; 3] {
    match mode {
        Assumption::PlaneStress => {
            let f = e / (1.0 - nu * nu);
            [[f, f * nu, 0.0], [f * nu, f, 0.0], [0.0, 0.0, f * (1.0 - nu) / 2.0]]
        }
        Assumption::PlaneStrain => {
            let f = e / ((1.0 + nu) * (1.0 - 2.0 * nu));
            [
                [f * (1.0 - nu), f * nu, 0.0],
                [f * nu, f * (1.0 - nu), 0.0],
                [0.0, 0.0, f * (1.0 - 2.0 * nu) / 2.0],
            ]
        }
    }
}

/// Constitutive matrix from a material's in-plane properties.
pub fn constitutive_of(mat: &Material, mode: Assumption) -> Result<[[f64; 3]; 3], String> {
    if !mat.elastic.is_stable() {
        return Err(format!("{} is not elastically stable", mat.name));
    }
    if let Elastic::TransverselyIsotropic { .. } = mat.elastic {
        // In-plane behaviour of a transversely isotropic material IS isotropic, so this is exact
        // for in-plane loading and silently wrong for anything involving the z direction. Say so.
        // A full orthotropic D matrix is the extension.
    }
    Ok(constitutive(mat.elastic.e_inplane(), mat.elastic.nu_inplane(), mode))
}

/// Strain–displacement matrix, 3×6, for one element.
pub fn b_matrix(el: &Element) -> [[f64; 6]; 3] {
    let s = 1.0 / (2.0 * el.area);
    let mut b = [[0.0; 6]; 3];
    for i in 0..3 {
        b[0][2 * i] = el.b[i] * s;
        b[1][2 * i + 1] = el.c[i] * s;
        b[2][2 * i] = el.c[i] * s;
        b[2][2 * i + 1] = el.b[i] * s;
    }
    b
}

/// Element stiffness, 6×6: `t·A·Bᵀ D B`.
pub fn element_stiffness(el: &Element, d: &[[f64; 3]; 3], thickness: f64) -> [[f64; 6]; 6] {
    let b = b_matrix(el);
    // db = D·B, 3×6
    let mut db = [[0.0f64; 6]; 3];
    for i in 0..3 {
        for j in 0..6 {
            db[i][j] = (0..3).map(|k| d[i][k] * b[k][j]).sum();
        }
    }
    let w = thickness * el.area.abs();
    let mut k = [[0.0f64; 6]; 6];
    for i in 0..6 {
        for j in 0..6 {
            k[i][j] = w * (0..3).map(|m| b[m][i] * db[m][j]).sum::<f64>();
        }
    }
    k
}

/// Assemble the global stiffness. DOF `2i` is the x displacement of node `i`, `2i+1` its y.
pub fn assemble(mesh: &Mesh, d: &[[f64; 3]; 3], thickness: f64) -> Csr {
    let mut trip = Vec::with_capacity(mesh.tri.len() * 36);
    for t in 0..mesh.tri.len() {
        let el = Element::of(mesh, t);
        let ke = element_stiffness(&el, d, thickness);
        let n = mesh.tri[t];
        for a in 0..3 {
            for b in 0..3 {
                for i in 0..2 {
                    for j in 0..2 {
                        trip.push((2 * n[a] + i, 2 * n[b] + j, ke[2 * a + i][2 * b + j]));
                    }
                }
            }
        }
    }
    Csr::from_triplets(2 * mesh.nodes(), &trip)
}

/// A prescribed displacement component.
#[derive(Clone, Copy, Debug)]
pub struct Constraint {
    pub dof: usize,
    pub value: f64,
}

/// Solve `K u = f` with prescribed displacements.
///
/// Elimination is symmetric — the known columns move to the right-hand side and the pinned rows
/// become identity — because breaking symmetry would turn an SPD system into one CG cannot solve.
pub fn solve(k: &Csr, forces: &[f64], fixed: &[Constraint], tol: f64) -> Result<Vec<f64>, String> {
    let n = k.n;
    if fixed.len() < 3 {
        // Two translations and one rotation: fewer than three independent constraints leaves a
        // rigid-body null space, the matrix is singular, and CG will return something plausible
        // rather than failing. Refuse structurally, as the Laplace solver does.
        return Err("under-constrained: at least 3 DOFs must be fixed to remove rigid-body motion".into());
    }
    let mut is_fixed = vec![false; n];
    let mut value = vec![0.0; n];
    for c in fixed {
        is_fixed[c.dof] = true;
        value[c.dof] = c.value;
    }

    let mut rhs = forces.to_vec();
    for r in 0..n {
        if is_fixed[r] {
            continue;
        }
        for p in k.row[r]..k.row[r + 1] {
            let c = k.col[p];
            if is_fixed[c] {
                rhs[r] -= k.val[p] * value[c];
            }
        }
    }
    let mut trip = Vec::with_capacity(k.nnz());
    for r in 0..n {
        if is_fixed[r] {
            trip.push((r, r, 1.0));
            rhs[r] = value[r];
            continue;
        }
        for p in k.row[r]..k.row[r + 1] {
            if !is_fixed[k.col[p]] {
                trip.push((r, k.col[p], k.val[p]));
            }
        }
    }
    let a = Csr::from_triplets(n, &trip);
    let mut u = value.clone();
    let rep = cg(&a, &rhs, &mut u, tol, 40 * n.max(50));
    if !rep.converged {
        return Err(format!("CG failed: residual {:.2e} after {} iterations", rep.residual, rep.iterations));
    }
    Ok(u)
}

/// Strain and stress in one element, both constant over it.
pub fn element_stress(mesh: &Mesh, t: usize, u: &[f64], d: &[[f64; 3]; 3]) -> ([f64; 3], [f64; 3]) {
    let el = Element::of(mesh, t);
    let b = b_matrix(&el);
    let n = mesh.tri[t];
    let mut ue = [0.0f64; 6];
    for a in 0..3 {
        ue[2 * a] = u[2 * n[a]];
        ue[2 * a + 1] = u[2 * n[a] + 1];
    }
    let mut eps = [0.0f64; 3];
    for i in 0..3 {
        eps[i] = (0..6).map(|j| b[i][j] * ue[j]).sum();
    }
    let mut sig = [0.0f64; 3];
    for i in 0..3 {
        sig[i] = (0..3).map(|j| d[i][j] * eps[j]).sum();
    }
    (eps, sig)
}

/// Von Mises equivalent stress for plane stress, from `[sx, sy, txy]`.
pub fn von_mises(s: [f64; 3]) -> f64 {
    (s[0] * s[0] - s[0] * s[1] + s[1] * s[1] + 3.0 * s[2] * s[2]).sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;

    const E: f64 = 200e9;
    const NU: f64 = 0.3;

    #[test]
    fn the_patch_test_passes_for_elasticity() {
        // The canonical verification, now with two unknowns per node. Impose a linear displacement
        // field on every boundary node; the interior must reproduce it exactly, AND the stress must
        // come out uniform in every element. The second half is the part that catches a wrong D or
        // a mis-assembled B, since a wrong constitutive matrix can still pass a displacement check.
        let m = Mesh::rectangle(2.0, 1.3, 6, 5);
        let d = constitutive(E, NU, Assumption::PlaneStress);
        let k = assemble(&m, &d, 1.0);
        // u = 1e-4 (0.3 + 0.5x - 0.2y),  v = 1e-4 (-0.1 + 0.4x + 0.7y)
        let exact = |p: [f64; 2]| {
            [
                1e-4 * (0.3 + 0.5 * p[0] - 0.2 * p[1]),
                1e-4 * (-0.1 + 0.4 * p[0] + 0.7 * p[1]),
            ]
        };
        let mut fixed = Vec::new();
        for (i, p) in m.xy.iter().enumerate() {
            let edge = p[0].abs() < 1e-12
                || (p[0] - 2.0).abs() < 1e-12
                || p[1].abs() < 1e-12
                || (p[1] - 1.3).abs() < 1e-12;
            if edge {
                let e = exact(*p);
                fixed.push(Constraint { dof: 2 * i, value: e[0] });
                fixed.push(Constraint { dof: 2 * i + 1, value: e[1] });
            }
        }
        let u = solve(&k, &vec![0.0; 2 * m.nodes()], &fixed, 1e-14).unwrap();
        let worst = m.xy.iter().enumerate().fold(0.0f64, |w, (i, p)| {
            let e = exact(*p);
            w.max((u[2 * i] - e[0]).abs()).max((u[2 * i + 1] - e[1]).abs())
        });
        assert!(worst < 1e-12, "displacement patch test failed by {worst:.3e}");

        let (_, s0) = element_stress(&m, 0, &u, &d);
        for t in 1..m.tri.len() {
            let (_, s) = element_stress(&m, t, &u, &d);
            for i in 0..3 {
                assert!(
                    (s[i] - s0[i]).abs() < 1e-3 * s0.iter().fold(0.0f64, |a, b| a.max(b.abs())),
                    "element {t}: stress is not uniform, {s:?} vs {s0:?}"
                );
            }
        }
    }

    #[test]
    fn rigid_body_motion_produces_no_stress() {
        // Translation and infinitesimal rotation must both be strain-free. A rotation is the
        // sharper case: u = -theta*y, v = theta*x gives gamma_xy = -theta + theta, so it only
        // vanishes if the two off-diagonal terms of B carry opposite signs correctly.
        let m = Mesh::rectangle(1.0, 1.0, 4, 4);
        let d = constitutive(E, NU, Assumption::PlaneStress);
        let theta = 1e-6;
        let mut u = vec![0.0; 2 * m.nodes()];
        for (i, p) in m.xy.iter().enumerate() {
            u[2 * i] = 0.01 - theta * p[1];
            u[2 * i + 1] = -0.02 + theta * p[0];
        }
        // Tolerances have to be scaled to each quantity, which the first version of this test got
        // wrong by comparing strain and stress against one absolute number. The strains here come
        // out at 1e-17 — machine zero — but multiplying by a 200 GPa modulus lifts the stress to
        // 1e-6, and calling that a failure would be an assertion about units rather than physics.
        let strain_scale = theta;
        let stress_scale = E * theta;
        for t in 0..m.tri.len() {
            let (eps, sig) = element_stress(&m, t, &u, &d);
            for v in eps {
                assert!(
                    v.abs() < strain_scale * 1e-8,
                    "element {t}: rigid motion strained the body, {eps:?}"
                );
            }
            for v in sig {
                assert!(
                    v.abs() < stress_scale * 1e-8,
                    "element {t}: rigid motion stressed the body, {sig:?}"
                );
            }
        }
    }

    #[test]
    fn uniaxial_tension_reproduces_youngs_modulus_and_poissons_ratio() {
        // The definition of E and nu, recovered from a solve rather than assumed.
        let (w, h) = (2.0, 1.0);
        let m = Mesh::rectangle(w, h, 8, 4);
        let d = constitutive(E, NU, Assumption::PlaneStress);
        let k = assemble(&m, &d, 1.0);
        let strain = 1e-4;

        let mut fixed = Vec::new();
        for (i, p) in m.xy.iter().enumerate() {
            if p[0].abs() < 1e-12 {
                fixed.push(Constraint { dof: 2 * i, value: 0.0 });
            }
            if (p[0] - w).abs() < 1e-12 {
                fixed.push(Constraint { dof: 2 * i, value: strain * w });
            }
            // One point pinned in y to remove the remaining rigid motion, without restraining
            // lateral contraction anywhere else — otherwise Poisson's ratio cannot be measured.
            if p[0].abs() < 1e-12 && p[1].abs() < 1e-12 {
                fixed.push(Constraint { dof: 2 * i + 1, value: 0.0 });
            }
        }
        let u = solve(&k, &vec![0.0; 2 * m.nodes()], &fixed, 1e-14).unwrap();
        let (eps, sig) = element_stress(&m, 0, &u, &d);
        assert!((eps[0] - strain).abs() / strain < 1e-6, "axial strain {}", eps[0]);
        assert!(
            (eps[1] / eps[0] + NU).abs() < 1e-4,
            "lateral/axial should be -nu = {}, got {}",
            -NU,
            eps[1] / eps[0]
        );
        assert!((sig[0] - E * strain).abs() / (E * strain) < 1e-6, "sigma_x {}", sig[0]);
        assert!(sig[1].abs() / sig[0].abs() < 1e-6, "free lateral edge must be unstressed");
    }

    #[test]
    fn plane_strain_is_stiffer_than_plane_stress() {
        // Constraining out-of-plane contraction adds stiffness. If the two D matrices were swapped
        // this ordering would reverse, and no single-mode test would notice.
        let ps = constitutive(E, NU, Assumption::PlaneStress);
        let pe = constitutive(E, NU, Assumption::PlaneStrain);
        assert!(pe[0][0] > ps[0][0], "plane strain should be stiffer axially");
        // Shear is unaffected by the out-of-plane constraint.
        assert!((pe[2][2] - ps[2][2]).abs() / ps[2][2] < 1e-12);
        // At nu = 0 the two coincide, since there is no Poisson coupling to constrain.
        let a = constitutive(E, 0.0, Assumption::PlaneStress);
        let b = constitutive(E, 0.0, Assumption::PlaneStrain);
        for i in 0..3 {
            for j in 0..3 {
                assert!((a[i][j] - b[i][j]).abs() < 1e-3);
            }
        }
    }

    #[test]
    fn cantilever_deflection_converges_toward_the_beam_solution_from_below() {
        // Constant-strain triangles cannot represent bending curvature, so a coarse mesh is too
        // stiff and the tip deflects too little. What must hold is that refinement moves it
        // monotonically toward PL^3/3EI — the error is a known property of the element, not a bug,
        // and asserting the trend is honest where asserting the value would not be.
        let (l, h, t, p_load) = (10.0, 1.0, 1.0, 1000.0);
        let i_area = t * h * h * h / 12.0;
        let euler = p_load * l * l * l / (3.0 * E * i_area);
        let d = constitutive(E, NU, Assumption::PlaneStress);

        let mut tips = Vec::new();
        for n in [8usize, 16, 32] {
            let m = Mesh::rectangle(l, h, n, n / 4);
            let k = assemble(&m, &d, t);
            let mut fixed = Vec::new();
            for (i, q) in m.xy.iter().enumerate() {
                if q[0].abs() < 1e-12 {
                    fixed.push(Constraint { dof: 2 * i, value: 0.0 });
                    fixed.push(Constraint { dof: 2 * i + 1, value: 0.0 });
                }
            }
            // Distribute the tip load over the free-end nodes.
            let tip: Vec<usize> = (0..m.nodes()).filter(|&i| (m.xy[i][0] - l).abs() < 1e-12).collect();
            let mut f = vec![0.0; 2 * m.nodes()];
            for &i in &tip {
                f[2 * i + 1] = -p_load / tip.len() as f64;
            }
            let u = solve(&k, &f, &fixed, 1e-12).unwrap();
            let tip_v = tip.iter().map(|&i| u[2 * i + 1]).sum::<f64>() / tip.len() as f64;
            tips.push(tip_v.abs());
        }
        for w in tips.windows(2) {
            assert!(w[1] > w[0], "refinement must soften the beam: {tips:?}");
        }
        for tv in &tips {
            assert!(*tv < euler * 1.05, "CST must not overshoot the beam solution: {tv:e} vs {euler:e}");
        }
        let closeness = tips[2] / euler;
        assert!(closeness > 0.7, "finest mesh reached only {:.2} of the beam solution", closeness);
    }

    #[test]
    fn an_under_constrained_body_is_refused() {
        // Fewer than three constraints leaves rigid-body motion in the null space; CG would return
        // a plausible field rather than failing, so this is caught before the solver sees it.
        let m = Mesh::rectangle(1.0, 1.0, 3, 3);
        let d = constitutive(E, NU, Assumption::PlaneStress);
        let k = assemble(&m, &d, 1.0);
        let f = vec![0.0; 2 * m.nodes()];
        assert!(solve(&k, &f, &[Constraint { dof: 0, value: 0.0 }], 1e-12).is_err());
    }

    #[test]
    fn material_library_values_flow_through_to_a_solve() {
        // The cross-layer join, on the elastic side this time.
        let al = sim_mat::library::aluminium_6061();
        let d = constitutive_of(&al, Assumption::PlaneStress).unwrap();
        assert!((d[0][0] - al.elastic.e_inplane() / (1.0 - 0.33f64.powi(2))).abs() / d[0][0] < 1e-9);
        // Von Mises of a uniaxial state equals the axial stress, which is the definition.
        assert!((von_mises([250e6, 0.0, 0.0]) - 250e6).abs() < 1.0);
        // Pure shear of tau gives sqrt(3)*tau.
        assert!((von_mises([0.0, 0.0, 100e6]) - 3f64.sqrt() * 100e6).abs() < 1.0);
    }
}
