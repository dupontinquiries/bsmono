//! homogenize.rs — computing what a microstructure behaves like.
//!
//! Effective properties of a composite are a *solver output*, not a lookup. You load a
//! representative volume element, measure its average response, and read off the stiffness that a
//! homogeneous material would need to behave the same way. `sim-mat::mixture` was written first,
//! deliberately, because it supplies the acceptance test: any effective modulus outside the
//! Voigt–Reuss bracket is provably wrong, with no reference data required.
//!
//! The recipe is three solves. Apply each unit macroscopic strain in turn — `εxx`, `εyy`, `γxy` —
//! and the volume-averaged stress it produces is the corresponding column of the effective
//! stiffness matrix. Structurally this is the same operation as capacitance extraction: impose a
//! unit load, integrate the response over the domain, divide.
//!
//! **Boundary conditions decide what you get, and this uses the over-stiff one.** Prescribing
//! `u = ε·x` on the whole boundary forces every boundary point to follow the macroscopic field
//! exactly, which suppresses the local fluctuation a real microstructure would show. That is a
//! kinematically admissible field, so by the minimum-energy principle it over-constrains and the
//! result is an **upper bound** — approaching the true value from above as the RVE grows to contain
//! more repeat units. Periodic boundary conditions give the correct answer at any size and need
//! multi-point constraints, which are not implemented; the convergence with RVE size is measured
//! here instead of assumed away.

use sim_mat::mixture::Phase;
use sim_num::sum_pairwise;

use crate::elastic::{assemble, constitutive, element_stress, solve, Assumption, Constraint};
use crate::mesh::Mesh;

/// A representative volume element: a mesh with a constitutive matrix per element.
pub struct Rve<'a> {
    pub mesh: &'a Mesh,
    /// Constitutive matrix for element `t`.
    pub d: Box<dyn Fn(usize) -> [[f64; 3]; 3] + 'a>,
    /// Domain extents, used to impose the macroscopic field and to normalise the average.
    pub lo: [f64; 2],
    pub hi: [f64; 2],
}

impl<'a> Rve<'a> {
    /// A two-phase RVE where `is_second` selects the second phase by element.
    pub fn two_phase(
        mesh: &'a Mesh,
        a: Phase,
        b: Phase,
        is_second: impl Fn(usize) -> bool + 'a,
    ) -> Rve<'a> {
        let da = constitutive(a.e, a.nu, Assumption::PlaneStress);
        let db = constitutive(b.e, b.nu, Assumption::PlaneStress);
        let (mut lo, mut hi) = ([f64::INFINITY; 2], [f64::NEG_INFINITY; 2]);
        for p in &mesh.xy {
            for k in 0..2 {
                lo[k] = lo[k].min(p[k]);
                hi[k] = hi[k].max(p[k]);
            }
        }
        Rve { mesh, d: Box::new(move |t| if is_second(t) { db } else { da }), lo, hi }
    }

    fn area(&self) -> f64 {
        sum_pairwise(
            &(0..self.mesh.tri.len())
                .map(|t| (self.mesh.double_area(t) / 2.0).abs())
                .collect::<Vec<_>>(),
        )
    }

    /// Volume fraction actually occupied by elements satisfying `pred` — worth measuring rather
    /// than assuming, since a meshed microstructure rarely hits its nominal fraction exactly.
    pub fn fraction(&self, pred: impl Fn(usize) -> bool) -> f64 {
        let total = self.area();
        let part = sum_pairwise(
            &(0..self.mesh.tri.len())
                .filter(|&t| pred(t))
                .map(|t| (self.mesh.double_area(t) / 2.0).abs())
                .collect::<Vec<_>>(),
        );
        part / total
    }
}

/// The effective 3×3 stiffness of an RVE under kinematic boundary conditions.
pub fn effective_stiffness(rve: &Rve) -> Result<[[f64; 3]; 3], String> {
    let mesh = rve.mesh;
    let n = mesh.nodes();
    let area = rve.area();
    let eps_mag = 1e-6; // small enough to stay linear, large enough to stay above roundoff

    // Boundary nodes: anything on the bounding rectangle.
    let tol = ((rve.hi[0] - rve.lo[0]) + (rve.hi[1] - rve.lo[1])) * 1e-9;
    let on_boundary: Vec<usize> = (0..n)
        .filter(|&i| {
            let p = mesh.xy[i];
            (p[0] - rve.lo[0]).abs() < tol
                || (p[0] - rve.hi[0]).abs() < tol
                || (p[1] - rve.lo[1]).abs() < tol
                || (p[1] - rve.hi[1]).abs() < tol
        })
        .collect();
    if on_boundary.len() < 3 {
        return Err("RVE has no identifiable boundary".into());
    }

    let k = {
        // Assemble once with a per-element D. `assemble` takes a single matrix, so build the
        // triplets here rather than restricting the RVE to one material.
        let mut trip = Vec::with_capacity(mesh.tri.len() * 36);
        for t in 0..mesh.tri.len() {
            let el = crate::p1::Element::of(mesh, t);
            let ke = crate::elastic::element_stiffness(&el, &(rve.d)(t), 1.0);
            let nn = mesh.tri[t];
            for a in 0..3 {
                for b in 0..3 {
                    for i in 0..2 {
                        for j in 0..2 {
                            trip.push((2 * nn[a] + i, 2 * nn[b] + j, ke[2 * a + i][2 * b + j]));
                        }
                    }
                }
            }
        }
        sim_num::Csr::from_triplets(2 * n, &trip)
    };
    let _ = assemble; // the single-material path stays available for ordinary solves

    let mut c_eff = [[0.0f64; 3]; 3];
    for case in 0..3 {
        // Macroscopic strain for this load case, as [exx, eyy, gxy].
        let mut e_mac = [0.0f64; 3];
        e_mac[case] = eps_mag;
        // The displacement field that realises it: u = exx*x + gxy/2*y, v = eyy*y + gxy/2*x.
        let field = |p: [f64; 2]| {
            [
                e_mac[0] * p[0] + 0.5 * e_mac[2] * p[1],
                e_mac[1] * p[1] + 0.5 * e_mac[2] * p[0],
            ]
        };
        let fixed: Vec<Constraint> = on_boundary
            .iter()
            .flat_map(|&i| {
                let u = field(mesh.xy[i]);
                [
                    Constraint { dof: 2 * i, value: u[0] },
                    Constraint { dof: 2 * i + 1, value: u[1] },
                ]
            })
            .collect();
        let u = solve(&k, &vec![0.0; 2 * n], &fixed, 1e-14)?;

        // Volume-averaged stress is the column of C_eff for this case.
        for i in 0..3 {
            let terms: Vec<f64> = (0..mesh.tri.len())
                .map(|t| {
                    let (_, sig) = element_stress_with(mesh, t, &u, &(rve.d)(t));
                    sig[i] * (mesh.double_area(t) / 2.0).abs()
                })
                .collect();
            c_eff[i][case] = sum_pairwise(&terms) / area / eps_mag;
        }
    }
    Ok(c_eff)
}

/// `element_stress` with an explicit per-element constitutive matrix.
fn element_stress_with(mesh: &Mesh, t: usize, u: &[f64], d: &[[f64; 3]; 3]) -> ([f64; 3], [f64; 3]) {
    element_stress(mesh, t, u, d)
}

/// Effective Young's modulus in x from an effective stiffness matrix, under plane stress:
/// invert `C` and take `1/S11`.
pub fn e_from_stiffness(c: &[[f64; 3]; 3]) -> Result<f64, String> {
    let flat: Vec<f64> = c.iter().flat_map(|r| r.iter().copied()).collect();
    let lu = sim_num::Lu::factor(&flat, 3).map_err(|e| format!("stiffness is singular: {e:?}"))?;
    // First column of the compliance matrix.
    let s = lu.solve(&[1.0, 0.0, 0.0]);
    if s[0].abs() < 1e-30 {
        return Err("degenerate compliance".into());
    }
    Ok(1.0 / s[0])
}

/// Asymmetry of an effective stiffness matrix, relative to its largest entry. Should be zero:
/// `C_eff` inherits symmetry from the underlying operator, so a non-symmetric result means the
/// averaging or the load cases are wrong.
pub fn asymmetry(c: &[[f64; 3]; 3]) -> f64 {
    let scale = c.iter().flatten().fold(0.0f64, |m, v| m.max(v.abs())).max(1e-30);
    let mut worst = 0.0f64;
    for i in 0..3 {
        for j in 0..3 {
            worst = worst.max((c[i][j] - c[j][i]).abs() / scale);
        }
    }
    worst
}

#[cfg(test)]
mod tests {
    use super::*;
    use sim_mat::mixture::{is_admissible, reuss, voigt};


    fn phase(e: f64, nu: f64, f: f64) -> Phase {
        Phase { fraction: f, e, nu }
    }

    #[test]
    fn a_homogeneous_rve_returns_exactly_its_own_stiffness() {
        // The degenerate case that must be exact: with one material everywhere, homogenisation is
        // the identity. Any error in the averaging, the load cases or the strain magnitude shows
        // up here before anything more interesting can hide it.
        let m = Mesh::rectangle(1.0, 1.0, 8, 8);
        let p = phase(200e9, 0.3, 1.0);
        let rve = Rve::two_phase(&m, p, p, |_| false);
        let c = effective_stiffness(&rve).unwrap();
        let want = constitutive(200e9, 0.3, Assumption::PlaneStress);
        for i in 0..3 {
            for j in 0..3 {
                assert!(
                    (c[i][j] - want[i][j]).abs() <= want[0][0] * 1e-9,
                    "C[{i}][{j}] = {} vs {}",
                    c[i][j],
                    want[i][j]
                );
            }
        }
        assert!(asymmetry(&c) < 1e-12);
        assert!((e_from_stiffness(&c).unwrap() - 200e9).abs() / 200e9 < 1e-9);
    }

    #[test]
    fn layer_orientation_changes_the_answer_by_an_order_of_magnitude() {
        // Topology sensitivity is the real content: the same two phases at the same fractions give
        // wildly different stiffness depending on whether the layers run along the load or across
        // it. A homogenisation that got this wrong would still respect the bounds.
        //
        // I originally expected layers-along-load to ATTAIN the Voigt bound exactly, and measured
        // 165 GPa against 170.7. The shortfall is real and the expectation was wrong: kinematic
        // boundary conditions fix the displacement only on the boundary, so while every layer does
        // carry the same exx, the transverse strain eyy is suppressed at the boundary and free in
        // the interior. Since sigma_xx = Q11 exx + Q12 eyy, that interior Poisson contraction pulls
        // the average stress a few percent below the uniform-strain value. Voigt is attained only
        // when BOTH strain components are uniform, which this boundary condition does not impose.
        let m = Mesh::rectangle(1.0, 1.0, 16, 16);
        let (stiff, soft) = (phase(300e9, 0.25, 0.5), phase(20e9, 0.25, 0.5));
        let q11 = |p: Phase| p.e / (1.0 - p.nu * p.nu);
        let voigt_c11 = 0.5 * q11(stiff) + 0.5 * q11(soft);
        let reuss_c11 = 1.0 / (0.5 / q11(stiff) + 0.5 / q11(soft));

        // Layers along x: parallel, near Voigt.
        let along = Rve::two_phase(&m, stiff, soft, |t| m.centroid(t)[1] > 0.5);
        let c_along = effective_stiffness(&along).unwrap()[0][0];
        // Layers across x: series, near Reuss.
        let across = Rve::two_phase(&m, stiff, soft, |t| m.centroid(t)[0] > 0.5);
        let c_across = effective_stiffness(&across).unwrap()[0][0];

        assert!(c_along <= voigt_c11 * 1.001, "cannot exceed uniform strain: {c_along:e}");
        assert!(
            c_along > voigt_c11 * 0.95,
            "layers along the load should sit just under Voigt: {c_along:e} vs {voigt_c11:e}"
        );
        assert!(
            c_across < c_along * 0.5,
            "layers across the load must be far softer: {c_across:e} vs {c_along:e}"
        );
        assert!(c_across > reuss_c11 * 0.99, "but never below Reuss: {c_across:e}");
    }

    #[test]
    fn every_result_lies_inside_the_voigt_reuss_bracket() {
        // The acceptance test the mixture module was written for. No reference data, no tuning —
        // anything outside the bracket is provably wrong.
        let m = Mesh::rectangle(1.0, 1.0, 12, 12);
        for frac in [0.25f64, 0.5, 0.75] {
            let a = phase(300e9, 0.25, frac);
            let b = phase(20e9, 0.25, 1.0 - frac);
            let rve = Rve::two_phase(&m, a, b, |t| m.centroid(t)[1] > frac);
            let c = effective_stiffness(&rve).unwrap();
            let e_eff = e_from_stiffness(&c).unwrap();
            let ps = vec![a, b];
            assert!(
                is_admissible(&ps, e_eff),
                "f={frac}: E_eff {e_eff:e} outside [{:e}, {:e}]",
                reuss(&ps),
                voigt(&ps)
            );
            assert!(asymmetry(&c) < 1e-9, "C_eff must be symmetric");
        }
    }

    #[test]
    fn kinematic_boundary_conditions_over_stiffen_and_relax_as_the_rve_grows() {
        // Prescribing u = eps*x on the whole boundary suppresses local fluctuation, so the result
        // is an upper bound that must come DOWN as the RVE contains more repeat units. Measuring
        // that convergence is the honest alternative to claiming periodic boundary conditions we
        // have not implemented.
        let (stiff, soft) = (phase(300e9, 0.25, 0.5), phase(20e9, 0.25, 0.5));
        let mut moduli = Vec::new();
        for repeats in [1usize, 2, 4] {
            // Layers perpendicular to the x load: a Reuss arrangement, where the boundary
            // constraint bites hardest.
            let m = Mesh::rectangle(1.0, 1.0, 16 * repeats, 16);
            let stripe = move |t: usize, mesh: &Mesh| {
                let x = mesh.centroid(t)[0];
                ((x * repeats as f64 * 2.0).floor() as usize) % 2 == 1
            };
            let rve = Rve::two_phase(&m, stiff, soft, |t| stripe(t, &m));
            let c = effective_stiffness(&rve).unwrap();
            moduli.push(e_from_stiffness(&c).unwrap());
        }
        for w in moduli.windows(2) {
            assert!(w[1] <= w[0] * 1.001, "more repeat units must not stiffen: {moduli:?}");
        }
        assert!(moduli[2] < moduli[0], "and should relax overall: {moduli:?}");
        // Still an upper bound on the true (Reuss-like) answer throughout.
        let ps = vec![stiff, soft];
        for e in &moduli {
            assert!(*e >= reuss(&ps) * 0.99, "below Reuss is impossible: {e:e}");
        }
    }

    #[test]
    fn nacres_stiffness_is_reachable_but_its_toughness_is_not() {
        // Brick-and-mortar: mineral platelets offset row to row, with a thin compliant matrix.
        // The offset is what makes it more than a stack of layers — load transfers between
        // platelets by shearing the matrix, which is why nacre is stiff despite a soft phase.
        //
        // What this computes is STIFFNESS, and it lands in the right region. What it cannot
        // compute is toughness, because every interface here is perfectly bonded — platelets
        // cannot slide, deflect a crack, or pull out. This test asserts both halves, so nobody
        // reads the stiffness agreement as evidence about the property that actually matters.
        let m = Mesh::rectangle(4.0, 1.0, 64, 24);
        let aragonite = phase(100e9, 0.30, 0.95);
        let matrix = phase(2e9, 0.40, 0.05);
        // Rows of platelets, offset by half a platelet each row, separated by thin matrix.
        let is_matrix = |t: usize| {
            let c = m.centroid(t);
            let row = (c[1] * 6.0).floor();
            let in_row_gap = (c[1] * 6.0).fract() > 0.90; // horizontal mortar layer
            let offset = if (row as i64) % 2 == 0 { 0.0 } else { 0.5 };
            let in_col_gap = ((c[0] + offset) * 4.0).fract() > 0.94; // vertical mortar
            in_row_gap || in_col_gap
        };
        let rve = Rve::two_phase(&m, aragonite, matrix, is_matrix);
        let f_matrix = rve.fraction(is_matrix);
        assert!(f_matrix > 0.05 && f_matrix < 0.25, "mortar fraction {f_matrix:.3}");

        let c = effective_stiffness(&rve).unwrap();
        let e_eff = e_from_stiffness(&c).unwrap();

        // The bound has to be applied to the STIFFNESS, not to Young's modulus, and finding that
        // out was instructive. The scalar Voigt/Reuss bracket in sim-mat averages E directly,
        // which is only equivalent to the tensor bound when the phases share a Poisson ratio.
        // Aragonite is 0.30 and the organic matrix is 0.40, and with that difference the extracted
        // E_eff came out marginally ABOVE the scalar Voigt value while remaining perfectly
        // admissible — because E is obtained by INVERTING C, and the inequality does not survive
        // the inversion when the Poisson ratios differ. The rigorous statement is on Q11.
        let q11 = |p: Phase| p.e / (1.0 - p.nu * p.nu);
        let voigt_c11 = (1.0 - f_matrix) * q11(aragonite) + f_matrix * q11(matrix);
        let reuss_c11 = 1.0 / ((1.0 - f_matrix) / q11(aragonite) + f_matrix / q11(matrix));
        assert!(
            c[0][0] <= voigt_c11 * 1.001 && c[0][0] >= reuss_c11 * 0.999,
            "C11 {:e} outside [{reuss_c11:e}, {voigt_c11:e}]",
            c[0][0]
        );
        // Measured nacre is 60-80 GPa in plane; a bonded brick-and-mortar model should be in that
        // neighbourhood, which is the most this can honestly claim.
        assert!(
            e_eff > 40e9 && e_eff < 110e9,
            "brick-and-mortar stiffness {:.1} GPa is outside the plausible band",
            e_eff / 1e9
        );

        // And the part that cannot be reached from here. Every interface in this model is
        // perfectly bonded, so no platelet can slide, no crack can be deflected, and nothing can
        // pull out — the three mechanisms nacre's toughness is made of.
        let n_tough = sim_mat::library::nacre().fracture.unwrap().k_ic.value;
        let a_tough = sim_mat::library::aragonite().fracture.unwrap().k_ic.value;
        assert!(
            n_tough / a_tough > 10.0,
            "the toughness this model cannot produce is a factor of {:.0}",
            n_tough / a_tough
        );
    }
}
