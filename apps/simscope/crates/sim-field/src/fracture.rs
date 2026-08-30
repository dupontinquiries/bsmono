//! fracture.rs — linear elastic fracture mechanics by the J-integral.
//!
//! The crack-tip stress field is singular — it goes as `1/√r` — so no finite element resolves it,
//! and a method that reads stress *at* the tip is measuring its own mesh. The J-integral sidesteps
//! this entirely: it is a conserved quantity, so evaluating it on a contour *away* from the tip
//! gives the same answer as one hugging it. The energy flowing to the tip is captured by the far
//! field, which the mesh does resolve.
//!
//! That conservation is also the test. **Path independence needs no reference data**: compute `J`
//! over several different annular domains and they must agree. If they do not, something is wrong
//! with the implementation, the mesh, or the assumption of linear elasticity — and you learn that
//! without knowing the right answer.
//!
//! The domain form is used rather than the line-integral form, because a contour drawn through a
//! finite element mesh cuts elements at arbitrary places and samples a piecewise-constant stress
//! field at its worst. The equivalent domain integral spreads the same quantity over an annulus of
//! elements:
//!
//! ```text
//!   J = INTEGRAL_A [ sigma_ij du_i/dx_1  -  W delta_1j ] dq/dx_j  dA
//! ```
//!
//! with `q` a weight that is 1 on an inner radius and 0 on an outer one. For constant-strain
//! triangles every factor in that integrand is constant per element, so the integral is a sum.

use sim_num::sum_pairwise;

use crate::elastic::{element_stress, Assumption};
use crate::mesh::Mesh;
use crate::p1::Element;

/// A crack tip and the annulus the domain integral is taken over.
#[derive(Clone, Copy, Debug)]
pub struct Domain {
    pub tip: [f64; 2],
    /// `q = 1` inside this radius.
    pub r_inner: f64,
    /// `q = 0` outside this radius.
    pub r_outer: f64,
}

/// The J-integral over one domain, in J/m² (energy release rate per unit crack advance).
///
/// `symmetric_half` doubles the result, for the usual half-model where the crack plane is a
/// symmetry boundary — the domain then encloses half the tip and captures half the energy flux.
pub fn j_integral(
    mesh: &Mesh,
    u: &[f64],
    d: &[[f64; 3]; 3],
    dom: &Domain,
    symmetric_half: bool,
) -> f64 {
    // Nodal weight function: 1 inside, 0 outside, linear in between. Linear q makes dq/dx constant
    // per element, matching the constant stress and displacement gradient of a CST — every term in
    // the integrand is then exactly integrable.
    let q: Vec<f64> = mesh
        .xy
        .iter()
        .map(|p| {
            let r = (p[0] - dom.tip[0]).hypot(p[1] - dom.tip[1]);
            if r <= dom.r_inner {
                1.0
            } else if r >= dom.r_outer {
                0.0
            } else {
                (dom.r_outer - r) / (dom.r_outer - dom.r_inner)
            }
        })
        .collect();

    let terms: Vec<f64> = (0..mesh.tri.len())
        .map(|t| {
            let el = Element::of(mesh, t);
            let n = mesh.tri[t];
            let gq = el.gradient([q[n[0]], q[n[1]], q[n[2]]]);
            if gq[0] == 0.0 && gq[1] == 0.0 {
                return 0.0; // wholly inside or outside the annulus: contributes nothing
            }
            let (eps, sig) = element_stress(mesh, t, u, d);
            // Displacement gradients in the crack-advance direction, x.
            let gu = el.gradient([u[2 * n[0]], u[2 * n[1]], u[2 * n[2]]]);
            let gv = el.gradient([u[2 * n[0] + 1], u[2 * n[1] + 1], u[2 * n[2] + 1]]);
            // Strain energy density.
            let w = 0.5 * (sig[0] * eps[0] + sig[1] * eps[1] + sig[2] * eps[2]);
            let a1 = sig[0] * gu[0] + sig[2] * gv[0] - w;
            let a2 = sig[2] * gu[0] + sig[1] * gv[0];
            (a1 * gq[0] + a2 * gq[1]) * (mesh.double_area(t) / 2.0).abs()
        })
        .collect();

    let j = sum_pairwise(&terms);
    if symmetric_half {
        2.0 * j
    } else {
        j
    }
}

/// Mode I stress intensity from the energy release rate: `K_I = sqrt(J E')`.
pub fn k_from_j(j: f64, e: f64, nu: f64, mode: Assumption) -> f64 {
    let e_prime = match mode {
        Assumption::PlaneStress => e,
        Assumption::PlaneStrain => e / (1.0 - nu * nu),
    };
    (j.max(0.0) * e_prime).sqrt()
}

/// Handbook stress intensity for a single edge crack of length `a` in a strip of width `w` under
/// remote tension `sigma` — the reference this module is checked against.
///
/// The polynomial is the standard finite-width correction; the leading 1.12 is the free-surface
/// factor for an edge crack in a half-space.
pub fn edge_crack_k(sigma: f64, a: f64, w: f64) -> f64 {
    let r = a / w;
    let f = 1.12 - 0.231 * r + 10.55 * r * r - 21.72 * r.powi(3) + 30.39 * r.powi(4);
    f * sigma * (std::f64::consts::PI * a).sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::elastic::{assemble, constitutive, solve, Constraint};

    const E: f64 = 200e9;
    const NU: f64 = 0.3;

    /// Half-model of an edge-cracked strip: the crack plane is the bottom edge, restrained beyond
    /// the tip and free ahead of it. No node splitting is needed — a crack face is simply an
    /// unrestrained part of the symmetry boundary, which is both simpler and less error-prone than
    /// duplicating nodes.
    fn solve_edge_crack(w: f64, h: f64, a: f64, sigma: f64, nx: usize, ny: usize) -> (Mesh, Vec<f64>) {
        let m = Mesh::rectangle(w, h, nx, ny);
        let d = constitutive(E, NU, Assumption::PlaneStress);
        let k = assemble(&m, &d, 1.0);
        let tol = w * 1e-9;
        let mut fixed = Vec::new();
        for (i, p) in m.xy.iter().enumerate() {
            if p[1].abs() < tol && p[0] > a + tol {
                // Ligament: symmetry restraint.
                fixed.push(Constraint { dof: 2 * i + 1, value: 0.0 });
            }
        }
        // Remove the remaining horizontal rigid motion at the far end of the ligament.
        let anchor = (0..m.nodes())
            .find(|&i| m.xy[i][1].abs() < tol && (m.xy[i][0] - w).abs() < tol)
            .unwrap();
        fixed.push(Constraint { dof: 2 * anchor, value: 0.0 });

        // Remote tension on the top edge as consistent nodal forces.
        let top: Vec<usize> = (0..m.nodes()).filter(|&i| (m.xy[i][1] - h).abs() < tol).collect();
        let mut f = vec![0.0; 2 * m.nodes()];
        let dx = w / nx as f64;
        for &i in &top {
            let edge = if m.xy[i][0].abs() < tol || (m.xy[i][0] - w).abs() < tol { dx / 2.0 } else { dx };
            f[2 * i + 1] = sigma * edge;
        }
        let u = solve(&k, &f, &fixed, 1e-13).unwrap();
        (m, u)
    }

    #[test]
    fn j_is_path_independent() {
        // The self-test that needs no reference. J is conserved, so annuli at different radii must
        // return the same value — and if they do not, the result is not a J-integral whatever it
        // agrees with.
        let (w, h, a, sigma) = (0.05, 0.10, 0.01, 50e6);
        let (m, u) = solve_edge_crack(w, h, a, sigma, 80, 80);
        let d = constitutive(E, NU, Assumption::PlaneStress);
        let mut js = Vec::new();
        for (r1, r2) in [(0.004, 0.008), (0.006, 0.012), (0.008, 0.016), (0.010, 0.020)] {
            let dom = Domain { tip: [a, 0.0], r_inner: r1, r_outer: r2 };
            js.push(j_integral(&m, &u, &d, &dom, true));
        }
        let mean = js.iter().sum::<f64>() / js.len() as f64;
        assert!(mean > 0.0, "J must be positive for an opening crack: {js:?}");
        for j in &js {
            assert!(
                (j / mean - 1.0).abs() < 0.05,
                "J varies with path by more than 5%: {js:?}"
            );
        }
    }

    #[test]
    fn stress_intensity_matches_the_edge_crack_handbook() {
        // The reference check. CST elements do not resolve the 1/sqrt(r) singularity at all, which
        // is precisely why the J-integral is evaluated away from the tip — the far field carries
        // the information and the mesh resolves that.
        let (w, h, sigma) = (0.05, 0.10, 50e6);
        for a in [0.005, 0.010, 0.015] {
            let (m, u) = solve_edge_crack(w, h, a, sigma, 80, 80);
            let d = constitutive(E, NU, Assumption::PlaneStress);
            let dom = Domain { tip: [a, 0.0], r_inner: 0.006, r_outer: 0.012 };
            let j = j_integral(&m, &u, &d, &dom, true);
            let k = k_from_j(j, E, NU, Assumption::PlaneStress);
            let want = edge_crack_k(sigma, a, w);
            assert!(
                (k / want - 1.0).abs() < 0.12,
                "a={a}: K = {:.3} MPa*sqrt(m) vs handbook {:.3}",
                k / 1e6,
                want / 1e6
            );
        }
    }

    #[test]
    fn stress_intensity_scales_as_the_square_root_of_crack_length() {
        // The scaling that defines LEFM, checked independently of any handbook constant: doubling
        // the crack should raise K by sqrt(2), and the finite-width correction accounts for the
        // small excess.
        let (w, h, sigma) = (0.08, 0.16, 50e6);
        let d = constitutive(E, NU, Assumption::PlaneStress);
        let mut ks = Vec::new();
        for a in [0.006, 0.012] {
            let (m, u) = solve_edge_crack(w, h, a, sigma, 80, 80);
            let dom = Domain { tip: [a, 0.0], r_inner: 0.005, r_outer: 0.010 };
            ks.push(k_from_j(j_integral(&m, &u, &d, &dom, true), E, NU, Assumption::PlaneStress));
        }
        let ratio = ks[1] / ks[0];
        let expected = edge_crack_k(sigma, 0.012, w) / edge_crack_k(sigma, 0.006, w);
        assert!(
            (ratio / expected - 1.0).abs() < 0.08,
            "K ratio {ratio:.3} vs handbook ratio {expected:.3}"
        );
        assert!(ratio > 1.3, "K must grow with crack length: {ks:?}");
    }

    #[test]
    fn an_uncracked_ligament_releases_no_energy() {
        // With the whole bottom edge restrained there is no crack, so J must vanish. This catches
        // a domain integral that is picking up the general stress field rather than the singular
        // one — an error that would still produce plausible-looking numbers on a cracked mesh.
        let (w, h, sigma) = (0.05, 0.10, 50e6);
        let (m, u) = solve_edge_crack(w, h, 0.0, sigma, 60, 60);
        let d = constitutive(E, NU, Assumption::PlaneStress);
        let dom = Domain { tip: [0.008, 0.0], r_inner: 0.004, r_outer: 0.008 };
        let j = j_integral(&m, &u, &d, &dom, true);
        // Reference scale: what a real crack of this size would release.
        let (mc, uc) = solve_edge_crack(w, h, 0.008, sigma, 60, 60);
        let real = j_integral(&mc, &uc, &d, &dom, true);
        assert!(real > 0.0);
        assert!(j.abs() < real * 0.05, "uncracked J = {j:.3e} against a real {real:.3e}");
    }

    #[test]
    fn a_material_fails_when_k_reaches_its_toughness() {
        // The design question the whole module exists to answer, using the library: how large a
        // flaw can each material tolerate at a given stress? The ordering is the point — alumina
        // tolerates millimetre flaws, steel tolerates centimetres, and nacre sits far above the
        // mineral it is made of.
        let sigma = 100e6;
        let critical_flaw = |k_ic: f64| {
            // a from K = 1.12 sigma sqrt(pi a), the edge-crack form.
            (k_ic / (1.12 * sigma)).powi(2) / std::f64::consts::PI
        };
        let al2o3 = critical_flaw(sim_mat::library::alumina_99().fracture.unwrap().k_ic.value);
        let steel = critical_flaw(sim_mat::library::stainless_304().fracture.unwrap().k_ic.value);
        let aragonite = critical_flaw(sim_mat::library::aragonite().fracture.unwrap().k_ic.value);
        let nacre = critical_flaw(sim_mat::library::nacre().fracture.unwrap().k_ic.value);

        assert!(steel > al2o3 * 100.0, "steel should tolerate far larger flaws");
        assert!(al2o3 > aragonite * 100.0, "even a ceramic beats a single mineral crystal");
        assert!(
            nacre / aragonite > 100.0,
            "nacre tolerates {:.0}x the flaw size of aragonite — and none of that is in the mineral",
            nacre / aragonite
        );
        // The number that matters is not an arbitrary threshold but a COMPARISON, and picking one
        // turns the test into an argument. At nacre's own working strength, how large a flaw can
        // the mineral tolerate, and how does that compare with the size of a platelet?
        let nacre_strength = sim_mat::library::nacre()
            .fracture
            .unwrap()
            .ultimate_strength
            .unwrap()
            .value;
        let aragonite_kic = sim_mat::library::aragonite().fracture.unwrap().k_ic.value;
        let flaw_at_working_stress =
            (aragonite_kic / (1.12 * nacre_strength)).powi(2) / std::f64::consts::PI;
        let platelet_thickness = 0.5e-6;
        assert!(
            flaw_at_working_stress > platelet_thickness,
            "critical flaw {:.2} um vs platelet {:.2} um",
            flaw_at_working_stress * 1e6,
            platelet_thickness * 1e6
        );
        // A platelet is THINNER than the flaw the mineral could tolerate at that stress, so no
        // crack large enough to matter fits inside one. This is the nanoscale flaw-tolerance
        // argument, and it is a size effect rather than a material property — which is why it
        // cannot appear anywhere in sim-mat's library and has to be computed.
        assert!(
            flaw_at_working_stress / platelet_thickness > 1.2,
            "flaw tolerance margin is only {:.2}x",
            flaw_at_working_stress / platelet_thickness
        );
    }
}
