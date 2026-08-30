//! rlgc.rs — from a field solve to the per-unit-length parameters of a transmission line.
//!
//! This is the join between what network analysis takes as given and what a field solve computes.
//! When a power flow reads `r_ohm_per_km` and `c_nf_per_km`, it is consuming the output of exactly
//! this calculation, performed once by somebody else on a cable cross-section. Level 1 computes
//! them instead of receiving them, which is what makes a PCB trace analysable with the same
//! machinery as a transmission line.
//!
//! Capacitance comes from energy rather than from surface charge. With the conductor at 1 V and
//! the return at 0 V:
//!
//! ```text
//!   W = 1/2 * INTEGRAL eps |grad phi|^2 dA        (per unit length)
//!   C = 2W / V^2 = 2W                              (since V = 1)
//! ```
//!
//! The energy route is preferred because it converges faster and more smoothly under refinement:
//! energy is a quadratic functional of the solution, so its error is O(h^2) where the field's own
//! error is only O(h). Extracting charge by differentiating the potential at the boundary throws
//! that advantage away.
//!
//! **Inductance comes free in a homogeneous medium.** For a TEM mode, `L * C = mu * eps`, so
//! `L = mu * eps / C` with no second solve. That identity is why a capacitance extractor is most
//! of an RLGC extractor, and it fails exactly where you would expect — a microstrip, whose field
//! is partly in the board and partly in the air, is not homogeneous and needs the second solve
//! with the dielectric removed.

use crate::mesh::Mesh;
use crate::p1::{assemble, energy, solve_dirichlet, Fixed};

/// Vacuum permittivity, F/m.
pub const EPS0: f64 = 8.854_187_812_8e-12;
/// Vacuum permeability, H/m.
pub const MU0: f64 = 1.256_637_062_12e-6;
/// Speed of light, m/s — equal to 1/sqrt(mu0 eps0) by definition of the SI units.
pub const C0: f64 = 299_792_458.0;

/// Per-unit-length parameters of a two-conductor line.
#[derive(Clone, Copy, Debug)]
pub struct Rlgc {
    /// Farads per metre.
    pub c: f64,
    /// Henries per metre.
    pub l: f64,
    /// Effective relative permittivity seen by the mode.
    pub eps_eff: f64,
    /// Characteristic impedance, ohms.
    pub z0: f64,
    /// Phase velocity, m/s.
    pub v_phase: f64,
}

/// Solve a cross-section and extract the line parameters.
///
/// `signal` and `ground` are node sets held at 1 V and 0 V. `eps_r(t)` gives the relative
/// permittivity of element `t`, which is how a mixed-dielectric cross-section is described.
///
/// The inductance uses the vacuum solve: `L = 1 / (c0^2 * C_vacuum)`. That is valid for
/// inhomogeneous cross-sections too, because the magnetic field does not see the dielectric —
/// which is precisely why a microstrip's phase velocity is neither `c0` nor `c0/sqrt(eps_r)`.
pub fn extract(
    mesh: &Mesh,
    signal: &[usize],
    ground: &[usize],
    eps_r: impl Fn(usize) -> f64 + Copy,
) -> Result<Rlgc, String> {
    let mut fixed: Vec<Fixed> = signal.iter().map(|&n| Fixed { node: n, value: 1.0 }).collect();
    fixed.extend(ground.iter().map(|&n| Fixed { node: n, value: 0.0 }));
    if signal.is_empty() || ground.is_empty() {
        return Err("need both a signal and a ground conductor".into());
    }

    // With dielectric: gives C.
    let k = assemble(mesh, |t| EPS0 * eps_r(t));
    let u = solve_dirichlet(&k, &fixed, 1e-13)?;
    let c = 2.0 * energy(mesh, &u, |t| EPS0 * eps_r(t));

    // Without dielectric: gives L, via L = 1/(c0^2 C_vac).
    let k0 = assemble(mesh, |_| EPS0);
    let u0 = solve_dirichlet(&k0, &fixed, 1e-13)?;
    let c_vac = 2.0 * energy(mesh, &u0, |_| EPS0);

    if !(c > 0.0 && c_vac > 0.0) {
        return Err(format!("non-physical capacitance: C={c:e}, C_vac={c_vac:e}"));
    }
    let l = 1.0 / (C0 * C0 * c_vac);
    Ok(Rlgc {
        c,
        l,
        eps_eff: c / c_vac,
        z0: (l / c).sqrt(),
        v_phase: 1.0 / (l * c).sqrt(),
    })
}

/// Analytic capacitance of an ideal coaxial line, F/m — the exact answer this module is checked
/// against.
pub fn coax_capacitance(a: f64, b: f64, eps_r: f64) -> f64 {
    std::f64::consts::TAU * EPS0 * eps_r / (b / a).ln()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn coax(nr: usize, nt: usize, a: f64, b: f64, eps_r: f64) -> Rlgc {
        let m = Mesh::annulus(a, b, nr, nt);
        let inner: Vec<usize> = Mesh::annulus_ring(nt, 0).collect();
        let outer: Vec<usize> = Mesh::annulus_ring(nt, nr).collect();
        extract(&m, &inner, &outer, |_| eps_r).unwrap()
    }

    #[test]
    fn coax_capacitance_matches_the_analytic_value() {
        let (a, b, er) = (1.0e-3, 3.0e-3, 1.0);
        let got = coax(24, 256, a, b, er).c;
        let want = coax_capacitance(a, b, er);
        let rel = (got - want).abs() / want;
        assert!(rel < 2e-3, "C = {got:e} vs analytic {want:e}, {:.3}% off", rel * 100.0);
    }

    #[test]
    fn the_energy_error_converges_at_the_expected_rate() {
        // P1 elements give O(h^2) convergence in energy, and measuring the RATE rather than the
        // error is what distinguishes a correct formulation from one that merely lands close on
        // one mesh.
        //
        // The subtlety that broke the first version of this test: refining the ANGULAR resolution
        // changes the geometry, because the mesh is an inscribed polygon and not a circle. So
        // comparing against the analytic circle while refining nt conflates discretisation error
        // with shape error, and the measured rate collapsed to ~1. Holding the polygon fixed and
        // refining only radially isolates the finite element convergence, which is what the O(h^2)
        // claim is actually about.
        let (a, b, er) = (1.0e-3, 4.0e-3, 2.2);
        let nt = 512;
        let reference = coax(96, nt, a, b, er).c; // effectively exact for this polygon
        let mut errs = Vec::new();
        for nr in [3usize, 6, 12] {
            let got = coax(nr, nt, a, b, er).c;
            errs.push((got - reference).abs() / reference);
        }
        for w in errs.windows(2) {
            let rate = (w[0] / w[1]).log2();
            assert!(
                rate > 1.6,
                "radial convergence rate {rate:.2}, expected ~2 for P1; errors {errs:?}"
            );
        }
        // The radially-converged value still carries the ANGULAR discretisation error, which this
        // study holds fixed on purpose — so it sits ~0.1%% above the true circle rather than at it.
        // Above, not below: a Dirichlet FEM solution minimises energy over a subspace of the true
        // trial space, so its energy is an upper bound and the capacitance converges from above.
        // That one-sidedness is a real property worth asserting.
        let circle = coax_capacitance(a, b, er);
        assert!(reference > circle, "FEM capacitance must bound the exact value from above");
        assert!(
            (reference - circle) / circle < 2e-3,
            "a 512-gon at nr=96 should be within ~0.1%% of the circle: {reference:e} vs {circle:e}"
        );
    }

    #[test]
    fn a_homogeneous_line_propagates_at_the_speed_of_light_in_its_dielectric() {
        // The physical identity LC = mu*eps in a homogeneous medium. If the inductance route is
        // wrong this fails immediately, and it is a much sharper check than comparing L to a
        // formula because it ties two independent solves together.
        for er in [1.0, 2.2, 4.4] {
            let r = coax(20, 192, 1.0e-3, 3.0e-3, er);
            let want_v = C0 / er.sqrt();
            assert!(
                (r.v_phase - want_v).abs() / want_v < 3e-3,
                "eps_r={er}: v = {:e}, expected {want_v:e}",
                r.v_phase
            );
            assert!((r.eps_eff - er).abs() / er < 3e-3, "eps_eff {} vs {er}", r.eps_eff);
        }
    }

    #[test]
    fn coax_impedance_matches_the_textbook_formula() {
        // Z0 = (1/2pi) sqrt(mu/eps) ln(b/a) — the number an RF engineer would look up, arrived at
        // here from a field solve with no closed form anywhere in the code path.
        // Dimensions chosen so the textbook value really is ~50 ohm: with PTFE (eps_r 2.1) the
        // required ratio is b/a = exp(50 * 2pi * sqrt(2.1) / 376.73) = 3.34. An earlier version of
        // this test used b/a = 4.6 and asserted "near 50 ohm", which is simply a 63 ohm cable —
        // the formula comparison passed and only the editorial claim was wrong.
        let (a, b, er) = (0.45e-3, 1.503e-3, 2.1);
        let r = coax(20, 192, a, b, er);
        let want = (MU0 / (EPS0 * er)).sqrt() * (b / a).ln() / std::f64::consts::TAU;
        assert!(
            (r.z0 - want).abs() / want < 4e-3,
            "Z0 = {:.2} ohm vs textbook {:.2} ohm",
            r.z0,
            want
        );
        assert!(r.z0 > 40.0 && r.z0 < 60.0, "should land near 50 ohm, got {:.1}", r.z0);
    }

    #[test]
    fn a_missing_conductor_is_refused() {
        let m = Mesh::annulus(1e-3, 2e-3, 4, 16);
        let inner: Vec<usize> = Mesh::annulus_ring(16, 0).collect();
        assert!(extract(&m, &inner, &[], |_| 1.0).is_err());
        assert!(extract(&m, &[], &inner, |_| 1.0).is_err());
    }
}
