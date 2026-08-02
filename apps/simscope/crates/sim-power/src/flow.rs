//! flow.rs — AC power flow by Newton–Raphson in polar coordinates.
//!
//! The state is `x = [θ for PV and PQ buses ; |V| for PQ buses]` and the residual is the power
//! mismatch `S_calc(x) − S_spec`. The slack bus contributes no unknowns and no equations: its
//! angle is the reference and its power is whatever balances the network.
//!
//! Polar rather than rectangular because the specified quantities are naturally polar — a
//! generator holds |V|, a slack holds θ — so PV buses drop out of the Jacobian by simply not
//! having a |V| column, with no constraint equations needed.

use sim_num::{newton, Lu, NewtonOpts, Report, System};

use crate::case::Case;

/// The Newton system for one network.
pub struct PowerFlow<'a> {
    pub case: &'a Case,
    pvpq: Vec<usize>,
    pq: Vec<usize>,
    /// Working voltage state, full length, including the buses Newton does not solve for.
    vm: Vec<f64>,
    va: Vec<f64>,
    /// Jacobian, dense row-major. Rebuilt each iteration; see the note on `solve_step`.
    j: Vec<f64>,
}

impl<'a> PowerFlow<'a> {
    pub fn new(case: &'a Case) -> PowerFlow<'a> {
        let pvpq = case.pvpq();
        let pq = case.pq();
        let m = pvpq.len() + pq.len();
        PowerFlow {
            vm: case.vm.clone(),
            va: case.va.clone(),
            j: vec![0.0; m * m],
            pvpq,
            pq,
            case,
        }
    }

    /// Number of unknowns.
    pub fn dim(&self) -> usize {
        self.pvpq.len() + self.pq.len()
    }

    /// A flat start: every unknown angle at zero, every unknown magnitude at one. This is the
    /// standard starting point and it is deliberately *not* the fixture's stored solution —
    /// starting from the answer would validate nothing.
    pub fn flat_start(&mut self) {
        for &i in &self.pvpq {
            self.va[i] = 0.0;
        }
        for &i in &self.pq {
            self.vm[i] = 1.0;
        }
    }

    /// Pack the working voltages into the Newton state vector.
    pub fn state(&self) -> Vec<f64> {
        let mut x = Vec::with_capacity(self.dim());
        x.extend(self.pvpq.iter().map(|&i| self.va[i]));
        x.extend(self.pq.iter().map(|&i| self.vm[i]));
        x
    }

    fn unpack(&mut self, x: &[f64]) {
        for (k, &i) in self.pvpq.iter().enumerate() {
            self.va[i] = x[k];
        }
        let off = self.pvpq.len();
        for (k, &i) in self.pq.iter().enumerate() {
            self.vm[i] = x[off + k];
        }
    }

    /// The converged (or current) voltages, full length.
    pub fn voltages(&self) -> (Vec<f64>, Vec<f64>) {
        (self.vm.clone(), self.va.clone())
    }

    /// Build the polar Jacobian at the current working point.
    ///
    /// These are the textbook expressions. They are written out rather than assembled from complex
    /// matrix products because the off-by-a-sign failure mode is invisible in the iteration count
    /// — a wrong Jacobian still converges, just linearly — so the finite-difference test in this
    /// module is the only thing standing between "it works" and "it is right".
    fn build_jacobian(&mut self) {
        let cs = self.case;
        let (npvpq, npq) = (self.pvpq.len(), self.pq.len());
        let m = npvpq + npq;
        self.j.iter_mut().for_each(|v| *v = 0.0);

        let s = cs.injections(&self.vm, &self.va);

        for (r, &i) in self.pvpq.iter().enumerate() {
            for (col, &jb) in self.pvpq.iter().enumerate() {
                self.j[r * m + col] = self.dp_dtheta(i, jb, &s);
            }
            for (col, &jb) in self.pq.iter().enumerate() {
                self.j[r * m + npvpq + col] = self.dp_dvm(i, jb, &s);
            }
        }
        for (r, &i) in self.pq.iter().enumerate() {
            let row = npvpq + r;
            for (col, &jb) in self.pvpq.iter().enumerate() {
                self.j[row * m + col] = self.dq_dtheta(i, jb, &s);
            }
            for (col, &jb) in self.pq.iter().enumerate() {
                self.j[row * m + npvpq + col] = self.dq_dvm(i, jb, &s);
            }
        }
    }

    fn dp_dtheta(&self, i: usize, j: usize, s: &[crate::cplx::C]) -> f64 {
        let y = self.case.y(i, j);
        if i == j {
            -s[i].im - y.im * self.vm[i] * self.vm[i]
        } else {
            let t = self.va[i] - self.va[j];
            self.vm[i] * self.vm[j] * (y.re * t.sin() - y.im * t.cos())
        }
    }

    fn dp_dvm(&self, i: usize, j: usize, s: &[crate::cplx::C]) -> f64 {
        let y = self.case.y(i, j);
        if i == j {
            s[i].re / self.vm[i] + y.re * self.vm[i]
        } else {
            let t = self.va[i] - self.va[j];
            self.vm[i] * (y.re * t.cos() + y.im * t.sin())
        }
    }

    fn dq_dtheta(&self, i: usize, j: usize, s: &[crate::cplx::C]) -> f64 {
        let y = self.case.y(i, j);
        if i == j {
            s[i].re - y.re * self.vm[i] * self.vm[i]
        } else {
            let t = self.va[i] - self.va[j];
            -self.vm[i] * self.vm[j] * (y.re * t.cos() + y.im * t.sin())
        }
    }

    fn dq_dvm(&self, i: usize, j: usize, s: &[crate::cplx::C]) -> f64 {
        let y = self.case.y(i, j);
        if i == j {
            s[i].im / self.vm[i] - y.im * self.vm[i]
        } else {
            let t = self.va[i] - self.va[j];
            self.vm[i] * (y.re * t.sin() - y.im * t.cos())
        }
    }
}

impl System for PowerFlow<'_> {
    fn residual(&mut self, x: &[f64], f: &mut [f64]) {
        self.unpack(x);
        let s = self.case.injections(&self.vm, &self.va);
        for (k, &i) in self.pvpq.iter().enumerate() {
            f[k] = s[i].re - self.case.p_spec[i];
        }
        let off = self.pvpq.len();
        for (k, &i) in self.pq.iter().enumerate() {
            f[off + k] = s[i].im - self.case.q_spec[i];
        }
    }

    fn solve_step(&mut self, x: &[f64], f: &[f64], delta: &mut [f64]) -> bool {
        self.unpack(x);
        self.build_jacobian();
        // Refactorised every iteration. That is the honest cost of an exact Jacobian, and it is
        // what buys quadratic convergence; a dishonest optimisation here (reusing the factors)
        // trades iterations for factorisations and usually loses.
        let Ok(lu) = Lu::factor(&self.j, self.dim()) else {
            return false;
        };
        let rhs: Vec<f64> = f.iter().map(|v| -v).collect();
        delta.copy_from_slice(&lu.solve(&rhs));
        true
    }
}

/// Solve a case from a flat start. Returns the Newton report and the converged voltages.
pub fn solve(case: &Case, opts: NewtonOpts) -> (Report, Vec<f64>, Vec<f64>) {
    let mut pf = PowerFlow::new(case);
    pf.flat_start();
    let mut x = pf.state();
    let report = newton::solve(&mut pf, &mut x, opts);
    let (vm, va) = pf.voltages();
    (report, vm, va)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::case::BusType;

    fn load(name: &str) -> Case {
        let path = format!("{}/../../fixtures/{name}.json", env!("CARGO_MANIFEST_DIR"));
        Case::from_json(&std::fs::read_to_string(&path).unwrap()).unwrap()
    }

    const CASES: [&str; 6] = ["case9", "case14", "case30", "case57", "case118", "case300"];

    #[test]
    fn the_analytic_jacobian_matches_finite_differences() {
        // The test that matters most in this crate. A wrong Jacobian still converges — linearly
        // instead of quadratically — so no amount of "it gets the right answer" validates it.
        // Only differentiating the residual numerically does.
        for name in ["case9", "case14", "case30"] {
            let cs = load(name);
            let mut pf = PowerFlow::new(&cs);
            pf.flat_start();
            // Perturb off the flat start so the derivatives are not evaluated at a symmetric point
            // where a sign error could cancel.
            let mut x = pf.state();
            for (k, v) in x.iter_mut().enumerate() {
                *v += 0.03 * ((k as f64) * 0.7).sin();
            }
            let n = x.len();
            pf.unpack(&x);
            pf.build_jacobian();
            let analytic = pf.j.clone();

            let mut f0 = vec![0.0; n];
            pf.residual(&x, &mut f0);
            // Central differences: O(h²) accurate, so h ~ eps^(1/3) balances truncation against
            // roundoff and gives ~1e-10 agreement for a correct Jacobian.
            let h = 1e-5;
            let mut worst = 0.0f64;
            for col in 0..n {
                let mut xp = x.clone();
                let mut xm = x.clone();
                xp[col] += h;
                xm[col] -= h;
                let (mut fp, mut fm) = (vec![0.0; n], vec![0.0; n]);
                pf.residual(&xp, &mut fp);
                pf.residual(&xm, &mut fm);
                for row in 0..n {
                    let numeric = (fp[row] - fm[row]) / (2.0 * h);
                    let a = analytic[row * n + col];
                    worst = worst.max((numeric - a).abs());
                }
            }
            assert!(
                worst < 1e-6,
                "{name}: analytic Jacobian differs from finite differences by {worst:.3e}"
            );
        }
    }

    #[test]
    fn every_case_converges_to_pandapowers_answer_from_a_flat_start() {
        for name in CASES {
            let cs = load(name);
            let (report, vm, va) = solve(&cs, NewtonOpts { tol: 1e-10, ..Default::default() });
            assert!(report.converged(), "{name}: {report:?}");

            let (ref_vm, ref_va) = cs.solution.clone().unwrap();
            let mut worst_vm = 0.0f64;
            let mut worst_va = 0.0f64;
            for i in 0..cs.n {
                worst_vm = worst_vm.max((vm[i] - ref_vm[i]).abs());
                // Angles are only defined up to the reference; both are anchored at the slack, so
                // they are directly comparable.
                worst_va = worst_va.max((va[i] - ref_va[i]).abs());
            }
            assert!(
                worst_vm < 1e-6,
                "{name}: |V| differs from pandapower by {worst_vm:.3e}"
            );
            assert!(
                worst_va < 1e-6,
                "{name}: angle differs from pandapower by {worst_va:.3e} rad"
            );
        }
    }

    #[test]
    fn convergence_is_quadratic_on_every_case() {
        // The behavioural signature of a correct Jacobian, checked on real networks rather than
        // on a scalar toy. Anything near 1.0 would mean the derivatives are wrong even though the
        // answer above came out right.
        for name in CASES {
            let cs = load(name);
            let (report, _, _) = solve(&cs, NewtonOpts { tol: 1e-10, ..Default::default() });
            let order = report.observed_order().unwrap_or(0.0);
            assert!(
                order > 1.6,
                "{name}: convergence order {order:.2}, residuals {:?}",
                report.residuals
            );
            assert!(
                report.iterations <= 8,
                "{name}: took {} iterations, expected quadratic",
                report.iterations
            );
            assert_eq!(report.backtracks, 0, "{name}: a flat start should not need damping");
        }
    }

    #[test]
    fn the_converged_state_satisfies_power_balance_at_every_solved_bus() {
        // Independent of the reference: whatever we converged to must actually be a solution of
        // the equations we wrote down.
        let cs = load("case118");
        let (_, vm, va) = solve(&cs, NewtonOpts { tol: 1e-10, ..Default::default() });
        let s = cs.injections(&vm, &va);
        for i in 0..cs.n {
            if matches!(cs.bus_type[i], BusType::Pv | BusType::Pq) {
                assert!((s[i].re - cs.p_spec[i]).abs() < 1e-9, "bus {i} P");
            }
            if cs.bus_type[i] == BusType::Pq {
                assert!((s[i].im - cs.q_spec[i]).abs() < 1e-9, "bus {i} Q");
            }
        }
    }
}
