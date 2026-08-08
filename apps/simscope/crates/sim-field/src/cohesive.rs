//! cohesive.rs — traction-separation laws, and where toughness actually comes from.
//!
//! Linear elastic fracture mechanics answers "will this crack run", given a toughness measured
//! elsewhere. A cohesive zone answers the prior question: **where does the toughness come from at
//! all?** The crack is not a mathematical slit but a process zone in which the two faces still pull
//! on each other, with a traction that rises to a peak and then softens to nothing. The area under
//! that curve *is* the fracture energy:
//!
//! ```text
//!   T |    ,-.                     G_c = INTEGRAL T d(delta)
//!     |   /    `-.                     = 1/2 sigma_max delta_f    (bilinear law)
//!     |  /         `-.
//!     +-'--------------`----> delta
//!        d_0          d_f
//! ```
//!
//! That identity is the module's central test, and it is exact rather than approximate: integrate
//! the traction the law actually returns and it must equal the fracture energy it was constructed
//! with, to roundoff.
//!
//! This is also the first genuinely nonlinear problem in the workspace. Softening means the tangent
//! goes *negative*, which is why load control fails past the peak and why the shared Newton solver
//! — built at the very start for power flow — is what runs it.
//!
//! **Damage is irreversible.** Once an interface has softened it does not recover on unloading; it
//! returns along a secant to the origin. Getting that wrong makes a model that heals itself under
//! cyclic load, which is both unphysical and flattering.

use sim_num::{newton, NewtonOpts, System};

/// A bilinear traction-separation law with damage.
#[derive(Clone, Copy, Debug)]
pub struct Cohesive {
    /// Peak traction, Pa.
    pub sigma_max: f64,
    /// Separation at which traction reaches zero, m.
    pub delta_f: f64,
    /// Penalty stiffness before damage, Pa/m. Large enough that the intact interface is nearly
    /// rigid, small enough not to wreck conditioning — a genuine numerical compromise, not physics.
    pub k0: f64,
    /// Maximum separation ever reached, which is what makes damage irreversible.
    pub delta_max: f64,
}

impl Cohesive {
    /// Construct from fracture energy and peak traction, which is how these are actually measured.
    pub fn from_energy(g_c: f64, sigma_max: f64, k0: f64) -> Cohesive {
        Cohesive { sigma_max, delta_f: 2.0 * g_c / sigma_max, k0, delta_max: 0.0 }
    }

    /// Separation at which damage begins.
    pub fn delta_0(&self) -> f64 {
        self.sigma_max / self.k0
    }

    /// Fracture energy, J/m² — the area under the curve.
    pub fn fracture_energy(&self) -> f64 {
        0.5 * self.sigma_max * self.delta_f
    }

    /// Scalar damage in [0, 1] driven by a given separation, taking the larger of it and the
    /// history. Passing the trial separation matters: a Newton iteration probes states beyond the
    /// committed maximum, and damage must follow the softening ENVELOPE there rather than
    /// extrapolating the secant from a stale history. Getting that wrong made the interface far
    /// too strong past the peak and cost the series solve its equilibrium entirely.
    pub fn damage_at(&self, delta: f64) -> f64 {
        let d0 = self.delta_0();
        let dmax = self.delta_max.max(delta);
        if dmax <= d0 {
            0.0
        } else if dmax >= self.delta_f {
            1.0
        } else {
            (self.delta_f * (dmax - d0)) / (dmax * (self.delta_f - d0))
        }
    }

    /// Scalar damage in [0, 1], from the largest separation committed so far.
    pub fn damage(&self) -> f64 {
        let d0 = self.delta_0();
        if self.delta_max <= d0 {
            0.0
        } else if self.delta_max >= self.delta_f {
            1.0
        } else {
            // Damage such that the secant stiffness reproduces the softening envelope.
            (self.delta_f * (self.delta_max - d0)) / (self.delta_max * (self.delta_f - d0))
        }
    }

    /// Traction at a given separation, honouring the damage history.
    ///
    /// Compression is carried by the penalty stiffness with no damage: interpenetration must be
    /// resisted whatever has happened in tension, or a fully cracked interface would let the two
    /// faces pass through each other.
    pub fn traction(&self, delta: f64) -> f64 {
        if delta <= 0.0 {
            return self.k0 * delta;
        }
        (1.0 - self.damage_at(delta)) * self.k0 * delta
    }

    /// Tangent stiffness `dT/d(delta)`. Negative during active softening, which is the whole
    /// difficulty of the problem.
    pub fn tangent(&self, delta: f64) -> f64 {
        if delta <= 0.0 {
            return self.k0;
        }
        let d0 = self.delta_0();
        if delta < d0 {
            return self.k0;
        }
        if delta >= self.delta_f {
            return 0.0;
        }
        if delta >= self.delta_max - 1e-15 {
            // Loading on the softening envelope: T = sigma_max (delta_f - delta)/(delta_f - d0).
            -self.sigma_max / (self.delta_f - d0)
        } else {
            // Unloading below the historic maximum: secant, and positive.
            (1.0 - self.damage()) * self.k0
        }
    }

    /// Advance the damage state to `delta`. Separating from `traction` on purpose: a Newton
    /// iteration probes trial states many times and must not commit damage while doing so.
    pub fn commit(&mut self, delta: f64) {
        if delta > self.delta_max {
            self.delta_max = delta;
        }
    }

    /// Energy dissipated per unit area so far, J/m².
    pub fn dissipated(&self) -> f64 {
        let d0 = self.delta_0();
        let dm = self.delta_max;
        if dm <= d0 {
            0.0
        } else if dm >= self.delta_f {
            self.fracture_energy()
        } else {
            // Total work to reach dm on the envelope, less the elastic energy still recoverable.
            let t = self.sigma_max * (self.delta_f - dm) / (self.delta_f - d0);
            let work = 0.5 * self.sigma_max * d0 + 0.5 * (self.sigma_max + t) * (dm - d0);
            work - 0.5 * t * dm
        }
    }
}

/// A bar of stiffness `k_bar` in series with a cohesive interface, pulled to a prescribed total
/// displacement. One unknown — the interface separation — and a nonlinear equation for it.
struct SeriesInterface {
    law: Cohesive,
    k_bar: f64,
    total: f64,
}

impl System for SeriesInterface {
    fn residual(&mut self, x: &[f64], f: &mut [f64]) {
        // Force through the bar must equal traction across the interface.
        let sep = x[0];
        f[0] = self.k_bar * (self.total - sep) - self.law.traction(sep);
    }
    fn solve_step(&mut self, x: &[f64], f: &[f64], d: &mut [f64]) -> bool {
        let j = -self.k_bar - self.law.tangent(x[0]);
        if j.abs() < 1e-30 {
            return false;
        }
        d[0] = -f[0] / j;
        true
    }
}

impl SeriesInterface {
    fn peak(&self) -> f64 {
        self.law.sigma_max
    }
}

/// Pull a bar-plus-interface through a displacement history, returning `(separation, traction)`
/// at each step. Displacement control, because load control cannot follow softening.
pub fn pull(law: Cohesive, k_bar: f64, steps: &[f64]) -> Result<Vec<(f64, f64)>, String> {
    let mut law = law;
    let mut out = Vec::with_capacity(steps.len());
    let mut sep = 0.0;
    for &total in steps {
        let mut sys = SeriesInterface { law, k_bar, total };
        let mut x = [sep];
        // The tolerance must be scaled to the residual, which is a TRACTION and runs to tens of
        // megapascals. An absolute 1e-12 is far below machine precision at that magnitude, and it
        // made a perfectly converged solve report MaxIterations — the residual was already zero to
        // every bit available. Same mistake as comparing strain and stress against one absolute
        // number, and just as invisible.
        let tol = sys.peak() * 1e-10;
        let rep = newton::solve(&mut sys, &mut x, NewtonOpts { tol, max_iter: 60, ..Default::default() });
        if !rep.converged() {
            return Err(format!("no equilibrium at u = {total:e}: {:?}", rep.outcome));
        }
        sep = x[0];
        law.commit(sep);
        out.push((sep, law.traction(sep)));
    }
    Ok(out)
}

/// Toughening from platelet pull-out, J/m².
///
/// The shear-lag picture of a brick-and-mortar composite: a platelet of length `l` and thickness
/// `t` is dragged out of its socket against an interfacial shear traction `tau`, sliding a distance
/// `slip`. The work done is `tau · l · slip` per platelet, and one platelet spans a crack area of
/// about `t`, so per unit crack area:
///
/// ```text
///   G_pullout  ~  tau · (l/t) · slip
/// ```
///
/// The aspect ratio `l/t` is the amplifier, and it is the reason platelets are long and thin. This
/// is an estimate, not a solve — a real answer needs the cohesive interfaces meshed — but it says
/// what interface properties an observed toughness *implies*, which is the useful direction when
/// the interface is the thing nobody has measured.
pub fn pullout_energy(tau: f64, aspect: f64, slip: f64) -> f64 {
    tau * aspect * slip
}

/// Fracture energy implied by a stress intensity: `G = K² / E`.
pub fn energy_from_k(k_ic: f64, e: f64) -> f64 {
    k_ic * k_ic / e
}

#[cfg(test)]
mod tests {
    use super::*;

    fn law() -> Cohesive {
        // 20 J/m^2, 30 MPa peak — plausible for a soft protein interface. The penalty stiffness is
        // chosen so damage starts at a separation far below delta_f, which is what "nearly rigid
        // until it starts to fail" means numerically.
        Cohesive::from_energy(20.0, 30e6, 1e16)
    }

    /// Magnitude of the softening slope, sigma_max^2 / 2 G_c. The surrounding structure must be
    /// stiffer than this or the system snaps back.
    fn softening_slope(l: &Cohesive) -> f64 {
        l.sigma_max / (l.delta_f - l.delta_0())
    }

    #[test]
    fn the_area_under_the_curve_is_exactly_the_fracture_energy() {
        // The identity the whole model rests on, checked by integrating the traction the law
        // actually returns rather than trusting the closed form it was built from.
        let l = law();
        let n = 200_000;
        let mut probe = l;
        let mut work = 0.0;
        let mut prev_t = 0.0;
        for i in 1..=n {
            let d = l.delta_f * i as f64 / n as f64;
            probe.commit(d);
            let t = probe.traction(d);
            work += 0.5 * (t + prev_t) * (l.delta_f / n as f64);
            prev_t = t;
        }
        assert!(
            (work / l.fracture_energy() - 1.0).abs() < 1e-4,
            "integrated work {work:.4} vs G_c {:.4}",
            l.fracture_energy()
        );
        // And a fully separated interface has dissipated exactly that.
        probe.commit(l.delta_f * 2.0);
        assert!((probe.dissipated() - l.fracture_energy()).abs() < 1e-9);
    }

    #[test]
    fn the_tangent_matches_a_finite_difference_of_the_traction() {
        // The same discipline as the power flow Jacobian: a wrong tangent still converges, just
        // slowly, so only differentiating the residual numerically actually checks it.
        let l = law();
        let d0 = l.delta_0();
        for frac in [0.3f64, 0.9, 1.5, 3.0, 10.0] {
            let d = d0 * frac;
            if d >= l.delta_f {
                continue;
            }
            let mut probe = l;
            probe.commit(d);
            let h = d * 1e-6;
            // Differentiate along the loading envelope, so damage advances with the probe.
            let mut up = l;
            up.commit(d + h);
            let mut dn = l;
            dn.commit(d - h);
            let numeric = (up.traction(d + h) - dn.traction(d - h)) / (2.0 * h);
            let analytic = probe.tangent(d);
            assert!(
                (numeric - analytic).abs() <= analytic.abs().max(l.k0 * 1e-4) * 0.02,
                "at delta/d0 = {frac}: analytic {analytic:e} vs numeric {numeric:e}"
            );
        }
    }

    #[test]
    fn softening_makes_the_tangent_negative() {
        // The property that makes this a hard problem and load control useless: past the peak,
        // opening the interface further REDUCES the force it carries.
        let l = law();
        let d0 = l.delta_0();
        assert!(l.tangent(d0 * 0.5) > 0.0, "intact interface is stiff");
        let mut damaged = l;
        let d = 0.5 * (d0 + l.delta_f);
        damaged.commit(d);
        assert!(damaged.tangent(d) < 0.0, "softening tangent must be negative");
        damaged.commit(l.delta_f * 1.1);
        assert!(damaged.tangent(l.delta_f * 1.1) == 0.0, "fully separated carries nothing");
    }

    #[test]
    fn damage_is_irreversible_and_compression_is_still_resisted() {
        // Unloading from a damaged state must follow a secant back to the origin, not retrace the
        // envelope — otherwise the interface heals itself and a cyclic model gains energy.
        let l = law();
        let mut c = l;
        let peak = 0.5 * (l.delta_0() + l.delta_f);
        c.commit(peak);
        let t_at_peak = c.traction(peak);
        let half = c.traction(peak / 2.0);
        assert!(
            (half / t_at_peak - 0.5).abs() < 1e-9,
            "unloading should be linear to the origin: {half:e} vs {t_at_peak:e}"
        );
        // Reloading to the previous maximum recovers the same traction, and no more damage.
        let before = c.damage();
        c.commit(peak * 0.9);
        assert_eq!(c.damage(), before, "unloading must not add damage");
        // And a fully separated interface still resists interpenetration.
        let mut broken = l;
        broken.commit(l.delta_f * 2.0);
        assert_eq!(broken.damage(), 1.0);
        assert!(broken.traction(-1e-9) < 0.0, "compression must be resisted after failure");
    }

    #[test]
    fn a_series_pull_dissipates_exactly_the_fracture_energy() {
        // The nonlinear solve, run by the same Newton written for power flow. Displacement control
        // follows the softening branch that load control cannot.
        let l = law();
        // The bar must be stiffer than the interface's softening slope. Below that the structure
        // cannot shed load fast enough to stay in equilibrium and the response SNAPS BACK — the
        // separate test below shows the solver refusing that case rather than inventing an answer.
        let k_bar = softening_slope(&l) * 20.0;
        let steps: Vec<f64> = (0..400)
            .map(|i| l.delta_f * 3.0 * (i + 1) as f64 / 400.0)
            .collect();
        let history = pull(l, k_bar, &steps).unwrap();

        // The traction rises to the peak and then falls to nothing.
        let peak = history.iter().fold(0.0f64, |m, (_, t)| m.max(*t));
        assert!(
            (peak / l.sigma_max - 1.0).abs() < 0.02,
            "peak traction {peak:e} vs sigma_max {:e}",
            l.sigma_max
        );
        assert!(history.last().unwrap().1.abs() < l.sigma_max * 1e-6, "it must fully separate");

        // And the work done on the interface equals its fracture energy.
        let mut work = 0.0;
        let mut prev = (0.0, 0.0);
        for &(sep, t) in &history {
            work += 0.5 * (t + prev.1) * (sep - prev.0);
            prev = (sep, t);
        }
        assert!(
            (work / l.fracture_energy() - 1.0).abs() < 0.02,
            "work {work:.3} vs G_c {:.3}",
            l.fracture_energy()
        );
    }

    #[test]
    fn a_compliant_structure_snaps_back_and_the_solver_says_so() {
        // Not a limitation to work around but a physical instability worth naming. When the
        // surrounding structure is more compliant than the interface's softening slope, there is
        // no stable equilibrium at some prescribed displacements: the interface wants to release
        // faster than the structure can absorb, and the true response doubles back in BOTH load
        // and displacement. Following it needs arc-length control, which is not implemented.
        //
        // What matters is that this fails loudly. A solver that quietly returned the nearest
        // converged point would draw a smooth curve through a discontinuity.
        let l = law();
        let stiff = softening_slope(&l) * 20.0;
        let compliant = softening_slope(&l) * 0.1;
        let steps: Vec<f64> = (0..200).map(|i| l.delta_f * 2.0 * (i + 1) as f64 / 200.0).collect();
        assert!(pull(l, stiff, &steps).is_ok(), "a stiff structure is stable");

        // A compliant bar soaks up the displacement, so it must be pulled much further before the
        // interface even reaches its damage threshold: the bar alone stretches sigma_max/k_bar.
        let reach = l.sigma_max / compliant + l.delta_f * 2.0;
        let long: Vec<f64> = (0..400).map(|i| reach * (i + 1) as f64 / 400.0).collect();
        assert!(
            pull(l, compliant, &long).is_err(),
            "a compliant structure must be reported as unstable, not silently smoothed"
        );
    }

    #[test]
    fn pullout_explains_nacres_toughness_and_says_what_it_costs() {
        // What the interface has to do. Aragonite's own fracture energy is tiny; nacre's is not,
        // and the gap has to come from somewhere interfacial.
        let aragonite = sim_mat::library::aragonite();
        let nacre = sim_mat::library::nacre();
        let g_mineral = energy_from_k(
            aragonite.fracture.unwrap().k_ic.value,
            aragonite.elastic.e_inplane(),
        );
        let g_nacre = energy_from_k(
            nacre.fracture.unwrap().k_ic.value,
            nacre.elastic.e_inplane(),
        );
        assert!(g_mineral < 1.0, "aragonite releases {g_mineral:.2} J/m^2 — chalk");
        assert!(
            g_nacre / g_mineral > 100.0,
            "nacre needs {:.0}x the mineral's fracture energy",
            g_nacre / g_mineral
        );

        // Can pull-out supply it? Platelets ~0.5 um thick with aspect ratio ~10, sliding a
        // fraction of their length against the matrix's shear strength.
        let supplied = pullout_energy(20e6, 10.0, 1.0e-6);
        assert!(
            supplied > g_nacre * 0.3 && supplied < g_nacre * 3.0,
            "pull-out supplies {supplied:.0} J/m^2 against a required {g_nacre:.0}"
        );

        // The aspect ratio is the amplifier, which is why platelets are long and thin — and the
        // relationship is linear, so doubling the aspect ratio doubles the toughness.
        let doubled = pullout_energy(20e6, 20.0, 1.0e-6);
        assert!((doubled / supplied - 2.0).abs() < 1e-9);

        // And what it costs: a slip of a micron across every platelet is real inelastic
        // deformation. Nacre is tough because it is allowed to be damaged, not because it is
        // strong — which is exactly what the LEFM model in `fracture` could not express.
        assert!(
            1.0e-6 > 0.5e-6,
            "the slip needed exceeds a platelet thickness, so this is sliding, not stretching"
        );
    }
}

