//! thermal.rs — steady conduction, and the thermal stress it drives.
//!
//! Steady conduction is Laplace with thermal conductivity where permittivity was:
//!
//! ```text
//!   div(eps grad phi) = 0        electrostatics
//!   div(k   grad T  ) = 0        conduction
//! ```
//!
//! So `assemble` is reused verbatim with `k` in place of `eps`, and the analogy is exact rather
//! than decorative. The clearest demonstration is in the tests: the conductance of a cylindrical
//! shell is `2πk / ln(b/a)`, and the capacitance of a coaxial line is `2πε / ln(b/a)` — the *same
//! integral*, differing only in what the coefficient is called. A capacitance extractor is a
//! thermal resistance solver that has been asked a different question.
//!
//! Thermal stress is where the two physics meet, and it is the coupling that matters for real
//! hardware: an EUV mask absorbs a few watts and distorts by nanometres, a PCB via cracks because
//! copper and FR-4 expand at different rates. The mechanism is that a material wants to expand by
//! `α ΔT` and stress arises only from the part of that expansion the geometry will not permit.
//! Which gives the sharpest possible test: **unconstrained uniform heating must produce exactly
//! zero stress**, no matter how complicated the mesh.

use sim_num::sum_pairwise;

use crate::elastic::{b_matrix, Assumption};
use crate::mesh::Mesh;
use crate::p1::Element;

/// Heat flux in one element, W/m²: `q = −k grad T`, constant over a linear triangle.
pub fn heat_flux(mesh: &Mesh, t: usize, temp: &[f64], k: f64) -> [f64; 2] {
    let el = Element::of(mesh, t);
    let n = mesh.tri[t];
    let g = el.gradient([temp[n[0]], temp[n[1]], temp[n[2]]]);
    [-k * g[0], -k * g[1]]
}

/// Total heat crossing into the nodes of `boundary`, watts per unit depth.
///
/// Computed as the reaction `K·T` restricted to the boundary, rather than by differentiating the
/// temperature field and integrating the flux. The reaction is exact for the discrete problem —
/// it is what the assembled system says the boundary must supply — whereas differentiating a
/// linear element gives a piecewise-constant flux that only converges. For an energy balance
/// check, exact beats convergent.
pub fn boundary_heat(k_matrix: &sim_num::Csr, temp: &[f64], boundary: &[usize]) -> f64 {
    let mut reaction = vec![0.0; temp.len()];
    k_matrix.spmv(temp, &mut reaction);
    sum_pairwise(&boundary.iter().map(|&i| reaction[i]).collect::<Vec<_>>())
}

/// Thermal load vector from a temperature rise.
///
/// An element that heats by `ΔT` wants to expand by the free strain `ε₀ = [αΔT, αΔT, 0]` — thermal
/// expansion is dilatational, so it contributes no shear. The equivalent nodal forces are
/// `f = t A Bᵀ D ε₀`, and the resulting stress is `σ = D(ε − ε₀)`: only the *difference* between
/// what the material wanted and what the geometry allowed.
pub fn thermal_load(
    mesh: &Mesh,
    d: &[[f64; 3]; 3],
    thickness: f64,
    alpha: impl Fn(usize) -> f64,
    delta_t: impl Fn(usize) -> f64,
) -> Vec<f64> {
    let mut f = vec![0.0; 2 * mesh.nodes()];
    for t in 0..mesh.tri.len() {
        let el = Element::of(mesh, t);
        let b = b_matrix(&el);
        let e0 = alpha(t) * delta_t(t);
        // D·ε₀ — the stress the element would carry if fully restrained.
        let s0 = [d[0][0] * e0 + d[0][1] * e0, d[1][0] * e0 + d[1][1] * e0, 0.0];
        let w = thickness * el.area.abs();
        let n = mesh.tri[t];
        for a in 0..3 {
            for i in 0..2 {
                let row = 2 * a + i;
                let v: f64 = (0..3).map(|m| b[m][row] * s0[m]).sum();
                f[2 * n[a] + i] += w * v;
            }
        }
    }
    f
}

/// Stress in an element given a displacement field and a temperature rise.
///
/// `σ = D(ε − ε₀)`. Subtracting the free thermal strain is the whole of thermoelasticity, and
/// forgetting it produces enormous stresses in a body that is simply expanding freely.
pub fn thermal_stress(
    mesh: &Mesh,
    t: usize,
    u: &[f64],
    d: &[[f64; 3]; 3],
    alpha: f64,
    delta_t: f64,
) -> [f64; 3] {
    let el = Element::of(mesh, t);
    let b = b_matrix(&el);
    let n = mesh.tri[t];
    let mut ue = [0.0f64; 6];
    for a in 0..3 {
        ue[2 * a] = u[2 * n[a]];
        ue[2 * a + 1] = u[2 * n[a] + 1];
    }
    let e0 = alpha * delta_t;
    let mut eps = [0.0f64; 3];
    for i in 0..3 {
        eps[i] = (0..6).map(|j| b[i][j] * ue[j]).sum::<f64>();
    }
    let mech = [eps[0] - e0, eps[1] - e0, eps[2]];
    let mut sig = [0.0f64; 3];
    for i in 0..3 {
        sig[i] = (0..3).map(|j| d[i][j] * mech[j]).sum();
    }
    sig
}

/// Stress in a fully restrained body under uniform heating, plane stress: `−EαΔT/(1−ν)`.
/// Biaxial, hence the `1−ν`; the closed form the constrained test is checked against.
pub fn fully_restrained_stress(e: f64, nu: f64, alpha: f64, delta_t: f64, mode: Assumption) -> f64 {
    match mode {
        Assumption::PlaneStress => -e * alpha * delta_t / (1.0 - nu),
        Assumption::PlaneStrain => -e * alpha * delta_t / (1.0 - 2.0 * nu),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::elastic::{assemble as assemble_elastic, constitutive, solve as solve_elastic, Constraint};
    use crate::p1::{assemble, solve_dirichlet, Fixed};

    #[test]
    fn a_slab_conducts_with_a_linear_profile() {
        // The simplest exact solution: one-dimensional conduction between two fixed temperatures.
        let (w, h, k) = (0.05, 0.02, 45.0);
        let m = Mesh::rectangle(w, h, 20, 8);
        let stiff = assemble(&m, |_| k);
        let mut fixed = Vec::new();
        for (i, p) in m.xy.iter().enumerate() {
            if p[0].abs() < 1e-12 {
                fixed.push(Fixed { node: i, value: 400.0 });
            } else if (p[0] - w).abs() < 1e-12 {
                fixed.push(Fixed { node: i, value: 300.0 });
            }
        }
        let temp = solve_dirichlet(&stiff, &fixed, 1e-14).unwrap();
        for (i, p) in m.xy.iter().enumerate() {
            let want = 400.0 - 100.0 * p[0] / w;
            assert!((temp[i] - want).abs() < 1e-8, "node {i}: {} vs {want}", temp[i]);
        }
        // Flux is uniform and equals k*dT/L.
        let q = heat_flux(&m, 0, &temp, k);
        assert!((q[0] - k * 100.0 / w).abs() / (k * 100.0 / w) < 1e-9, "flux {q:?}");
        assert!(q[1].abs() < 1e-6);
    }

    #[test]
    fn heat_in_equals_heat_out() {
        // Conservation, checked on the discrete system rather than on the analytic solution: what
        // the hot boundary supplies must equal what the cold boundary removes, exactly.
        let (w, h, k) = (0.05, 0.02, 45.0);
        let m = Mesh::rectangle(w, h, 16, 6);
        let stiff = assemble(&m, |_| k);
        let (mut hot, mut cold) = (Vec::new(), Vec::new());
        let mut fixed = Vec::new();
        for (i, p) in m.xy.iter().enumerate() {
            if p[0].abs() < 1e-12 {
                hot.push(i);
                fixed.push(Fixed { node: i, value: 400.0 });
            } else if (p[0] - w).abs() < 1e-12 {
                cold.push(i);
                fixed.push(Fixed { node: i, value: 300.0 });
            }
        }
        let temp = solve_dirichlet(&stiff, &fixed, 1e-14).unwrap();
        let q_in = boundary_heat(&stiff, &temp, &hot);
        let q_out = boundary_heat(&stiff, &temp, &cold);
        assert!((q_in + q_out).abs() / q_in.abs() < 1e-10, "in {q_in} out {q_out}");
        // And it matches the analytic conductance kA/L.
        let want = k * h * 100.0 / w;
        assert!((q_in - want).abs() / want < 1e-9, "{q_in} vs {want}");
    }

    #[test]
    fn cylindrical_conductance_is_the_same_integral_as_coax_capacitance() {
        // The analogy made concrete. A cylindrical shell conducts 2*pi*k/ln(b/a) per unit length;
        // a coaxial line holds 2*pi*eps/ln(b/a). Same geometry, same operator, same solve — only
        // the name of the coefficient differs. Running both through the identical machinery and
        // getting C/eps == G/k is a stronger statement about the code than either check alone.
        let (a, b, k) = (1.0e-3, 4.0e-3, 12.0);
        let (nr, nt) = (24, 256);
        let m = Mesh::annulus(a, b, nr, nt);
        let inner: Vec<usize> = Mesh::annulus_ring(nt, 0).collect();
        let outer: Vec<usize> = Mesh::annulus_ring(nt, nr).collect();

        let stiff = assemble(&m, |_| k);
        let mut fixed: Vec<Fixed> = inner.iter().map(|&i| Fixed { node: i, value: 1.0 }).collect();
        fixed.extend(outer.iter().map(|&i| Fixed { node: i, value: 0.0 }));
        let temp = solve_dirichlet(&stiff, &fixed, 1e-14).unwrap();
        let g = boundary_heat(&stiff, &temp, &inner); // unit temperature difference, so Q = G

        let want = std::f64::consts::TAU * k / (b / a).ln();
        assert!((g - want).abs() / want < 3e-3, "conductance {g:.4} vs analytic {want:.4}");

        // And the electrostatic answer through the same code, scaled by its own coefficient.
        let r = crate::rlgc::extract(&m, &inner, &outer, |_| 1.0).unwrap();
        let c_over_eps = r.c / crate::rlgc::EPS0;
        let g_over_k = g / k;
        assert!(
            (c_over_eps - g_over_k).abs() / g_over_k < 5e-3,
            "C/eps = {c_over_eps:.4} should equal G/k = {g_over_k:.4}"
        );
    }

    #[test]
    fn free_expansion_produces_exactly_no_stress() {
        // The sharpest test in thermoelasticity. A body heated uniformly and free to move expands
        // and carries no stress at all. Forgetting to subtract the free thermal strain — the most
        // common error in a thermal stress implementation — produces enormous stresses here, and
        // they look entirely plausible.
        let (e, nu, alpha, dt) = (70e9, 0.33, 23.6e-6, 80.0);
        let m = Mesh::rectangle(0.04, 0.02, 8, 5);
        let d = constitutive(e, nu, Assumption::PlaneStress);
        let k = assemble_elastic(&m, &d, 1.0);
        let f = thermal_load(&m, &d, 1.0, |_| alpha, |_| dt);

        // Constrain only rigid-body motion: one corner fully, one more in y. Anything else would
        // restrain the expansion we are testing.
        let corner = (0..m.nodes())
            .find(|&i| m.xy[i][0].abs() < 1e-12 && m.xy[i][1].abs() < 1e-12)
            .unwrap();
        let xaxis = (0..m.nodes())
            .find(|&i| (m.xy[i][0] - 0.04).abs() < 1e-12 && m.xy[i][1].abs() < 1e-12)
            .unwrap();
        let fixed = vec![
            Constraint { dof: 2 * corner, value: 0.0 },
            Constraint { dof: 2 * corner + 1, value: 0.0 },
            Constraint { dof: 2 * xaxis + 1, value: 0.0 },
        ];
        let u = solve_elastic(&k, &f, &fixed, 1e-14).unwrap();

        let scale = e * alpha * dt; // the stress a fully restrained body would carry
        for t in 0..m.tri.len() {
            let s = thermal_stress(&m, t, &u, &d, alpha, dt);
            for v in s {
                assert!(v.abs() < scale * 1e-8, "element {t} stressed while free: {s:?}");
            }
        }
        // And it expanded by exactly alpha*dT.
        let far = (0..m.nodes())
            .find(|&i| (m.xy[i][0] - 0.04).abs() < 1e-12 && (m.xy[i][1] - 0.02).abs() < 1e-12)
            .unwrap();
        assert!(
            (u[2 * far] / (alpha * dt * 0.04) - 1.0).abs() < 1e-8,
            "free expansion should be alpha*dT*L, got {}",
            u[2 * far]
        );
    }

    #[test]
    fn full_restraint_reproduces_the_closed_form_thermal_stress() {
        // The opposite extreme: a body that cannot move at all carries -E alpha dT/(1-nu),
        // biaxially. Between this and free expansion lies every real thermal stress problem.
        let (e, nu, alpha, dt) = (70e9, 0.33, 23.6e-6, 80.0);
        let m = Mesh::rectangle(0.04, 0.02, 6, 4);
        let d = constitutive(e, nu, Assumption::PlaneStress);
        let k = assemble_elastic(&m, &d, 1.0);
        let f = thermal_load(&m, &d, 1.0, |_| alpha, |_| dt);
        let fixed: Vec<Constraint> = (0..m.nodes())
            .flat_map(|i| {
                [
                    Constraint { dof: 2 * i, value: 0.0 },
                    Constraint { dof: 2 * i + 1, value: 0.0 },
                ]
            })
            .collect();
        let u = solve_elastic(&k, &f, &fixed, 1e-14).unwrap();
        let want = fully_restrained_stress(e, nu, alpha, dt, Assumption::PlaneStress);
        for t in 0..m.tri.len() {
            let s = thermal_stress(&m, t, &u, &d, alpha, dt);
            assert!((s[0] - want).abs() / want.abs() < 1e-9, "sigma_x {} vs {want}", s[0]);
            assert!((s[1] - want).abs() / want.abs() < 1e-9, "sigma_y {} vs {want}", s[1]);
            assert!(s[2].abs() < want.abs() * 1e-9, "thermal expansion produces no shear");
        }
        assert!(want < 0.0, "heating a restrained body puts it in compression");
    }

    #[test]
    fn a_mismatched_pair_generates_stress_where_a_matched_pair_does_not() {
        // The PCB via failure in miniature: two bonded materials heated together are stressed only
        // by the DIFFERENCE in their expansion. Equal alphas must give the free-expansion answer.
        let (e, nu, dt) = (70e9, 0.3, 100.0);
        let m = Mesh::rectangle(0.02, 0.01, 10, 6);
        let d = constitutive(e, nu, Assumption::PlaneStress);
        let k = assemble_elastic(&m, &d, 1.0);
        let corner = (0..m.nodes())
            .find(|&i| m.xy[i][0].abs() < 1e-12 && m.xy[i][1].abs() < 1e-12)
            .unwrap();
        let xa = (0..m.nodes())
            .find(|&i| (m.xy[i][0] - 0.02).abs() < 1e-12 && m.xy[i][1].abs() < 1e-12)
            .unwrap();
        let fixed = vec![
            Constraint { dof: 2 * corner, value: 0.0 },
            Constraint { dof: 2 * corner + 1, value: 0.0 },
            Constraint { dof: 2 * xa + 1, value: 0.0 },
        ];
        let upper = |t: usize| m.centroid(t)[1] > 0.005;

        // Matched: both halves expand identically, so nothing is stressed.
        let f_same = thermal_load(&m, &d, 1.0, |_| 16e-6, |_| dt);
        let u_same = solve_elastic(&k, &f_same, &fixed, 1e-14).unwrap();
        let worst_same = (0..m.tri.len()).fold(0.0f64, |w, t| {
            let s = thermal_stress(&m, t, &u_same, &d, 16e-6, dt);
            w.max(s.iter().fold(0.0f64, |a, b| a.max(b.abs())))
        });

        // Mismatched: copper-like over FR-4-like, a factor of four apart.
        let alpha_of = |t: usize| if upper(t) { 16.5e-6 } else { 70e-6 };
        let f_diff = thermal_load(&m, &d, 1.0, alpha_of, |_| dt);
        let u_diff = solve_elastic(&k, &f_diff, &fixed, 1e-14).unwrap();
        let worst_diff = (0..m.tri.len()).fold(0.0f64, |w, t| {
            let s = thermal_stress(&m, t, &u_diff, &d, alpha_of(t), dt);
            w.max(s.iter().fold(0.0f64, |a, b| a.max(b.abs())))
        });

        assert!(worst_same < e * 16e-6 * dt * 1e-8, "matched pair must be stress-free");
        assert!(
            worst_diff > e * 1e-5 * dt * 0.05,
            "a 4x CTE mismatch must generate real stress, got {worst_diff:.3e}"
        );
    }

    #[test]
    fn material_library_thermal_data_flows_through() {
        // FR-4's through-plane CTE against copper's is the actual via-cracking mismatch.
        let fr4 = sim_mat::library::fr4();
        let cu = sim_mat::library::copper();
        let a_fr4 = fr4.thermal.unwrap().alpha_z.unwrap().value;
        let a_cu = cu.thermal.unwrap().alpha.value;
        let dt = 200.0; // ambient to reflow
        let mismatch = (a_fr4 - a_cu) * dt;
        assert!(mismatch > 0.005, "reflow mismatch strain is {mismatch:.4}, over half a percent");
        // Copper yields well before that strain is elastically accommodated, which is why the
        // failure is a fatigue crack rather than a clean fracture.
        let elastic_limit = cu.fracture.unwrap().yield_strength.unwrap().value
            / cu.elastic.e_inplane();
        assert!(mismatch > elastic_limit, "the via is driven into plasticity every reflow cycle");
    }
}
