//! laminate.rs — classical laminate theory: the right formulation for a thin layered plate.
//!
//! The CST bending study concluded that for thin structures more elements will not help, because
//! near-square cells at L/h = 1000 cost tens of thousands of elements per cross-section. This is
//! the alternative it pointed to: stop meshing through the thickness and integrate through it
//! analytically instead.
//!
//! A laminate's response is then three matrices relating in-plane strain and curvature to force
//! and moment resultants:
//!
//! ```text
//!   [ N ]   [ A  B ] [ eps0 ]        A = SUM Q_k (z_k - z_k-1)          extensional
//!   [   ] = [      ] [      ]        B = 1/2 SUM Q_k (z_k^2 - ...)      coupling
//!   [ M ]   [ B  D ] [ kappa ]       D = 1/3 SUM Q_k (z_k^3 - ...)      bending
//! ```
//!
//! `B` is the interesting one. It couples stretching to bending, and it vanishes identically for a
//! stack that is symmetric about its mid-plane — which is why a symmetric PCB stackup lies flat on
//! reflow and an asymmetric one potato-chips. That is exact and structural, not a numerical
//! result, which makes it the sharpest available test of the implementation.
//!
//! Layers are treated as in-plane isotropic. For the materials here that is not an approximation
//! being smuggled in: a transversely isotropic material — FR-4, a nacre tablet stack — is by
//! definition isotropic *in the plane*, so its in-plane constants are exactly what a lamina needs.
//! Ply angles and genuinely orthotropic laminae are the extension.

use sim_mat::Material;

/// One layer of a stack.
#[derive(Clone, Copy, Debug)]
pub struct Ply {
    pub thickness: f64,
    /// In-plane Young's modulus, Pa.
    pub e: f64,
    pub nu: f64,
    /// In-plane CTE, 1/K.
    pub alpha: f64,
}

impl Ply {
    pub fn of(mat: &Material, thickness: f64) -> Result<Ply, String> {
        let th = mat.thermal.ok_or_else(|| format!("{} has no thermal data", mat.name))?;
        Ok(Ply {
            thickness,
            e: mat.elastic.e_inplane(),
            nu: mat.elastic.nu_inplane(),
            alpha: th.alpha.value,
        })
    }

    /// Reduced stiffness `Q` for an in-plane isotropic lamina — the plane-stress matrix.
    fn q(&self) -> [[f64; 3]; 3] {
        let f = self.e / (1.0 - self.nu * self.nu);
        [[f, f * self.nu, 0.0], [f * self.nu, f, 0.0], [0.0, 0.0, f * (1.0 - self.nu) / 2.0]]
    }
}

/// A stack, listed from the bottom face upward.
#[derive(Clone, Debug)]
pub struct Laminate {
    pub plies: Vec<Ply>,
}

/// The response of a laminate: in-plane strain and curvature.
#[derive(Clone, Copy, Debug)]
pub struct Response {
    /// Mid-plane strain `[eps_x, eps_y, gamma_xy]`.
    pub strain: [f64; 3],
    /// Curvature `[kappa_x, kappa_y, kappa_xy]`, 1/m.
    pub curvature: [f64; 3],
}

impl Response {
    /// Out-of-plane deflection of a plate of side `l` bowing at this curvature, measured centre to
    /// edge: `w = kappa L^2 / 8`. The number a warp specification is written against.
    pub fn bow(&self, l: f64) -> f64 {
        self.curvature[0].abs().max(self.curvature[1].abs()) * l * l / 8.0
    }
}

impl Laminate {
    pub fn new(plies: Vec<Ply>) -> Laminate {
        Laminate { plies }
    }

    pub fn thickness(&self) -> f64 {
        self.plies.iter().map(|p| p.thickness).sum()
    }

    /// Ply interface heights measured from the mid-plane, bottom to top.
    fn z(&self) -> Vec<f64> {
        let half = self.thickness() / 2.0;
        let mut z = vec![-half];
        for p in &self.plies {
            z.push(z.last().unwrap() + p.thickness);
        }
        z
    }

    /// The A, B and D matrices.
    pub fn abd(&self) -> ([[f64; 3]; 3], [[f64; 3]; 3], [[f64; 3]; 3]) {
        let z = self.z();
        let mut a = [[0.0f64; 3]; 3];
        let mut b = [[0.0f64; 3]; 3];
        let mut d = [[0.0f64; 3]; 3];
        for (k, ply) in self.plies.iter().enumerate() {
            let q = ply.q();
            let (z0, z1) = (z[k], z[k + 1]);
            let (t1, t2, t3) = (z1 - z0, z1 * z1 - z0 * z0, z1 * z1 * z1 - z0 * z0 * z0);
            for i in 0..3 {
                for j in 0..3 {
                    a[i][j] += q[i][j] * t1;
                    b[i][j] += q[i][j] * t2 / 2.0;
                    d[i][j] += q[i][j] * t3 / 3.0;
                }
            }
        }
        (a, b, d)
    }

    /// Thermal force and moment resultants for a uniform temperature change.
    pub fn thermal_resultants(&self, delta_t: f64) -> ([f64; 3], [f64; 3]) {
        let z = self.z();
        let mut n = [0.0f64; 3];
        let mut m = [0.0f64; 3];
        for (k, ply) in self.plies.iter().enumerate() {
            let q = ply.q();
            // Free thermal strain is dilatational: no shear component.
            let e0 = [ply.alpha * delta_t, ply.alpha * delta_t, 0.0];
            let s: Vec<f64> = (0..3).map(|i| (0..3).map(|j| q[i][j] * e0[j]).sum()).collect();
            let (z0, z1) = (z[k], z[k + 1]);
            for i in 0..3 {
                n[i] += s[i] * (z1 - z0);
                m[i] += s[i] * (z1 * z1 - z0 * z0) / 2.0;
            }
        }
        (n, m)
    }

    /// Solve the 6x6 system for mid-plane strain and curvature under given resultants.
    pub fn solve(&self, n: [f64; 3], m: [f64; 3]) -> Result<Response, String> {
        let (a, b, d) = self.abd();
        let mut k = [[0.0f64; 6]; 6];
        for i in 0..3 {
            for j in 0..3 {
                k[i][j] = a[i][j];
                k[i][j + 3] = b[i][j];
                k[i + 3][j] = b[i][j];
                k[i + 3][j + 3] = d[i][j];
            }
        }
        let flat: Vec<f64> = k.iter().flat_map(|r| r.iter().copied()).collect();
        let rhs: Vec<f64> = n.iter().chain(m.iter()).copied().collect();
        let lu = sim_num::Lu::factor(&flat, 6).map_err(|e| format!("ABD is singular: {e:?}"))?;
        let x = lu.solve(&rhs);
        Ok(Response {
            strain: [x[0], x[1], x[2]],
            curvature: [x[3], x[4], x[5]],
        })
    }

    /// Warp from a uniform temperature change, free of external load — the reflow problem.
    pub fn thermal_response(&self, delta_t: f64) -> Result<Response, String> {
        let (n, m) = self.thermal_resultants(delta_t);
        self.solve(n, m)
    }

    /// Whether the stack is symmetric about its mid-plane, which is what makes `B` vanish.
    pub fn is_symmetric(&self) -> bool {
        let n = self.plies.len();
        (0..n / 2).all(|i| {
            let (a, b) = (self.plies[i], self.plies[n - 1 - i]);
            (a.thickness - b.thickness).abs() < 1e-12 * a.thickness.max(1e-30)
                && (a.e - b.e).abs() < 1e-6 * a.e
                && (a.alpha - b.alpha).abs() < 1e-18
        })
    }
}

/// Timoshenko's 1925 bimetal strip curvature — the exact closed form for two bonded layers under
/// a uniform temperature change. The reference this module is checked against.
pub fn timoshenko_bimetal(t1: f64, t2: f64, e1: f64, e2: f64, a1: f64, a2: f64, dt: f64) -> f64 {
    let h = t1 + t2;
    let m = t1 / t2;
    let n = e1 / e2;
    let num = 6.0 * (a2 - a1) * dt * (1.0 + m).powi(2);
    let den = h * (3.0 * (1.0 + m).powi(2) + (1.0 + m * n) * (m * m + 1.0 / (m * n)));
    num / den
}

#[cfg(test)]
mod tests {
    use super::*;

    fn iso(t: f64, e: f64, nu: f64, alpha: f64) -> Ply {
        Ply { thickness: t, e, nu, alpha }
    }

    #[test]
    fn a_symmetric_stack_has_exactly_no_bending_coupling() {
        // The structural test. B couples stretching to bending, and it vanishes identically for a
        // stack symmetric about its mid-plane — exactly, not approximately. This is why a
        // symmetric PCB stackup lies flat through reflow and an asymmetric one warps, and any
        // error in the z bookkeeping shows up here immediately.
        let lam = Laminate::new(vec![
            iso(35e-6, 117e9, 0.34, 16.5e-6),
            iso(200e-6, 22e9, 0.15, 14e-6),
            iso(35e-6, 117e9, 0.34, 16.5e-6),
        ]);
        assert!(lam.is_symmetric());
        let (_, b, _) = lam.abd();
        let (a, _, _) = lam.abd();
        let scale = a[0][0];
        for i in 0..3 {
            for j in 0..3 {
                assert!(b[i][j].abs() < scale * 1e-12, "B[{i}][{j}] = {} should vanish", b[i][j]);
            }
        }
        // And therefore uniform heating produces strain but no curvature at all.
        let r = lam.thermal_response(200.0).unwrap();
        assert!(r.strain[0] > 0.0, "it should still expand");
        for k in r.curvature {
            assert!(k.abs() < 1e-9, "a symmetric laminate must not warp: {:?}", r.curvature);
        }
    }

    #[test]
    fn an_asymmetric_stack_warps_and_matches_timoshenko_exactly() {
        // Two bonded layers under uniform heating, against the 1925 closed form — and they agree
        // to twelve figures, which surprised me. I expected a biaxial correction of 1/(1-nu),
        // since Timoshenko's derivation is a one-dimensional strip free to contract laterally
        // while laminate theory is a plate that cannot.
        //
        // There is no correction, and the reason is worth keeping: Timoshenko's curvature depends
        // on the moduli ONLY through their ratio n = E1/E2. The plate constraint replaces every E
        // with E/(1-nu), and when both layers share a Poisson ratio that leaves n unchanged — so
        // the plate and beam curvatures coincide identically. The prediction was wrong; the code
        // was right, and finding out which was which took measuring.
        let (e1, e2, a1, a2, dt) = (117e9, 22e9, 16.5e-6, 70e-6, 100.0);
        for (t1, t2) in [(35e-6, 200e-6), (70e-6, 200e-6), (35e-6, 400e-6), (10e-6, 1.5e-3)] {
            let lam = Laminate::new(vec![iso(t1, e1, 0.3, a1), iso(t2, e2, 0.3, a2)]);
            assert!(!lam.is_symmetric());
            let k_fem = lam.thermal_response(dt).unwrap().curvature[0].abs();
            let k_ref = timoshenko_bimetal(t1, t2, e1, e2, a1, a2, dt).abs();
            assert!(
                (k_fem / k_ref - 1.0).abs() < 1e-9,
                "t1={t1:e} t2={t2:e}: CLT {k_fem:e} vs Timoshenko {k_ref:e}"
            );
        }
        // And with UNEQUAL Poisson ratios the equivalence breaks, which confirms the explanation
        // rather than leaving it as a story that happens to fit.
        let lam = Laminate::new(vec![iso(35e-6, e1, 0.45, a1), iso(200e-6, e2, 0.10, a2)]);
        let k_fem = lam.thermal_response(dt).unwrap().curvature[0].abs();
        let k_ref = timoshenko_bimetal(35e-6, 200e-6, e1, e2, a1, a2, dt).abs();
        assert!(
            (k_fem / k_ref - 1.0).abs() > 0.05,
            "mismatched Poisson ratios should break the coincidence, ratio {:.4}",
            k_fem / k_ref
        );
    }

    #[test]
    fn a_single_layer_reproduces_plate_stiffness() {
        // Degenerate case with an exact answer: A = Q h and D = Q h^3/12, the textbook plate
        // bending stiffness. If the z integration were wrong by a factor this would show.
        let (e, nu, h) = (200e9, 0.3, 1.5e-3);
        let lam = Laminate::new(vec![iso(h, e, nu, 12e-6)]);
        let (a, b, d) = lam.abd();
        let q11 = e / (1.0 - nu * nu);
        assert!((a[0][0] - q11 * h).abs() / (q11 * h) < 1e-12);
        assert!((d[0][0] - q11 * h * h * h / 12.0).abs() / d[0][0] < 1e-12);
        for i in 0..3 {
            for j in 0..3 {
                assert!(b[i][j].abs() < a[0][0] * 1e-12, "a single layer is symmetric");
            }
        }
    }

    #[test]
    fn free_expansion_of_a_uniform_plate_produces_alpha_dt_and_no_stress_resultant() {
        // The laminate analogue of the free-expansion test: a single layer heated uniformly
        // expands by exactly alpha*dT with no curvature.
        let (alpha, dt) = (23.6e-6, 80.0);
        let lam = Laminate::new(vec![iso(1e-3, 70e9, 0.33, alpha)]);
        let r = lam.thermal_response(dt).unwrap();
        assert!((r.strain[0] - alpha * dt).abs() / (alpha * dt) < 1e-12, "{:?}", r.strain);
        assert!((r.strain[1] - alpha * dt).abs() / (alpha * dt) < 1e-12);
        assert!(r.strain[2].abs() < 1e-18, "no shear from dilatational expansion");
        assert!(r.curvature.iter().all(|k| k.abs() < 1e-12));
    }

    #[test]
    fn a_real_pcb_stackup_warps_by_a_recognisable_amount() {
        // The FR-4 problem end to end, from the material library: copper on one face only, heated
        // from ambient to reflow. A 100 mm board should bow by a fraction of a millimetre — the
        // order of magnitude an assembly engineer would recognise, and enough to fail coplanarity.
        let cu = sim_mat::library::copper();
        let fr4 = sim_mat::library::fr4();
        let lam = Laminate::new(vec![
            Ply::of(&fr4, 1.5e-3).unwrap(),
            Ply::of(&cu, 35e-6).unwrap(),
        ]);
        let r = lam.thermal_response(200.0).unwrap();
        let bow = r.bow(0.1);
        assert!(bow > 1e-5 && bow < 3e-3, "100 mm board bowed {:.3} mm", bow * 1e3);

        // Balancing the stack with copper on both faces removes the warp entirely, which is the
        // actual design rule this theory exists to justify.
        let balanced = Laminate::new(vec![
            Ply::of(&cu, 35e-6).unwrap(),
            Ply::of(&fr4, 1.5e-3).unwrap(),
            Ply::of(&cu, 35e-6).unwrap(),
        ]);
        assert!(balanced.is_symmetric());
        assert!(balanced.thermal_response(200.0).unwrap().bow(0.1) < bow * 1e-6);
    }

    #[test]
    fn film_on_substrate_approaches_the_stoney_limit() {
        // Stoney's formula is an ASYMPTOTIC result: curvature goes as 1/t_s^2 only when the film
        // is negligibly thin compared with the substrate. Measured across 0.5-2 mm with a 20 um
        // film the exponent is 1.70, not 2, and asserting 2 there would be demanding that a limit
        // hold outside the limit. What must happen is that the exponent MOVES TOWARD 2 as the
        // thickness ratio improves, and that is the honest test.
        let (e_f, e_s, a_f, a_s, dt) = (117e9, 22e9, 16.5e-6, 70e-6, 100.0);
        let curvature = |t_s: f64, t_f: f64| {
            Laminate::new(vec![iso(t_s, e_s, 0.3, a_s), iso(t_f, e_f, 0.3, a_f)])
                .thermal_response(dt)
                .unwrap()
                .curvature[0]
                .abs()
        };
        let exponent = |t_f: f64, lo: f64, hi: f64| (curvature(lo, t_f) / curvature(hi, t_f)).log2() / (hi / lo).log2();

        let thick_film = exponent(50e-6, 0.5e-3, 2.0e-3); // ratio 0.1 to 0.025
        let thin_film = exponent(2e-6, 2.0e-3, 8.0e-3); // ratio 0.001 to 0.00025
        assert!(thin_film > thick_film, "thinner films should approach Stoney: {thin_film:.3} vs {thick_film:.3}");
        assert!(
            (thin_film - 2.0).abs() < 0.05,
            "in the thin-film limit the exponent must be 2, measured {thin_film:.3}"
        );
        // And monotonic decrease with substrate thickness regardless of regime.
        let mut prev = f64::INFINITY;
        for t_s in [0.5e-3, 1.0e-3, 2.0e-3, 4.0e-3] {
            let k = curvature(t_s, 20e-6);
            assert!(k < prev, "thicker substrates must warp less");
            prev = k;
        }
    }
}
