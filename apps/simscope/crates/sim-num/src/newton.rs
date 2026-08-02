//! newton.rs — one Newton–Raphson, reused by every layer.
//!
//! This is the architectural bet of the workspace: **power flow and nonlinear FEA are the same
//! solver wearing different physics.** Both reduce to "find x where F(x) = 0, given that you can
//! evaluate F and solve J·δ = −F". Power flow's F is the bus power mismatch; a nonlinear
//! structural problem's F is the internal-minus-external force residual; a drift-diffusion device
//! solve's F is the current continuity residual. The convergence machinery — damping, divergence
//! detection, the stopping test — is identical and worth writing once, carefully.
//!
//! Two decisions that matter more than they look:
//!
//! **The stopping test is on the infinity norm**, not the 2-norm. In power flow a tolerance is
//! "no bus is out by more than 1e-8 pu", and a single bad bus among a thousand must fail — in a
//! 2-norm it averages away. Same argument for a stress residual: local failure is the thing you
//! care about.
//!
//! **Convergence is checked before the step, not after.** A Newton iteration that starts already
//! converged should cost zero linear solves, and reporting "converged in 1 iteration" when the
//! initial guess was the answer is a lie about the work done.

use crate::kernel::norm_inf;

/// A nonlinear system. The implementor owns the state; Newton only orchestrates.
pub trait System {
    /// Residual F(x). Convergence is `‖F‖∞ ≤ tol`.
    fn residual(&mut self, x: &[f64], f_out: &mut [f64]);

    /// Solve J(x)·δ = −F for δ, writing δ into `delta`.
    ///
    /// The Jacobian is the implementor's business — assembled or matrix-free, dense or sparse,
    /// refactorised每 iteration or reused. Returning `false` means the linear solve failed
    /// (singular, non-finite) and Newton should stop rather than step into nonsense.
    fn solve_step(&mut self, x: &[f64], f: &[f64], delta: &mut [f64]) -> bool;

    /// Called after each accepted step. Power flow uses it for PV→PQ bus-type switching, which
    /// changes the *problem* mid-solve; returning true forces a residual re-evaluation and resets
    /// the convergence test, because a converged answer to the old problem says nothing about the
    /// new one.
    fn after_step(&mut self, _x: &mut [f64]) -> bool {
        false
    }
}

#[derive(Clone, Copy, Debug)]
pub struct NewtonOpts {
    /// Stop when ‖F‖∞ ≤ tol.
    pub tol: f64,
    pub max_iter: usize,
    /// Halve the step up to this many times when it fails to reduce the residual. Zero disables
    /// damping, which is what a textbook Newton does and what diverges on hard cases.
    pub max_backtracks: usize,
    /// Give up if the residual grows by more than this factor from its best value — a cheap,
    /// early divergence signal that beats waiting for max_iter on a hopeless case.
    pub divergence_factor: f64,
}

impl Default for NewtonOpts {
    fn default() -> Self {
        NewtonOpts { tol: 1e-8, max_iter: 30, max_backtracks: 8, divergence_factor: 1e6 }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Outcome {
    Converged,
    MaxIterations,
    Diverged,
    LinearSolveFailed,
}

#[derive(Clone, Debug)]
pub struct Report {
    pub outcome: Outcome,
    /// Residual ‖F‖∞ after each iteration, starting with the initial guess. This is the record
    /// that tells you whether Newton actually behaved: a correct implementation on a
    /// well-conditioned problem roughly squares its error each step, so this sequence should fall
    /// 1e-2, 1e-4, 1e-8, 1e-16. If it falls linearly, the Jacobian is wrong — and that diagnosis
    /// is only possible because the history is kept.
    pub residuals: Vec<f64>,
    pub iterations: usize,
    pub backtracks: usize,
}

impl Report {
    pub fn converged(&self) -> bool {
        self.outcome == Outcome::Converged
    }
    /// Observed convergence order: the exponent p in ‖e_{k+1}‖ ≈ C‖e_k‖^p, estimated as
    /// `ln(e2/e1) / ln(e1/e0)` over consecutive residuals. ~2 is quadratic and is the signature of
    /// a correct analytic Jacobian; ~1 means the derivatives are wrong and you are effectively
    /// running a fixed-point iteration.
    ///
    /// Residuals at or below `floor` are excluded. Once Newton reaches machine precision it
    /// cannot square its error any further, so the final triple always *understates* the order —
    /// an earlier version of this measured exactly that and reported 1.63 for a textbook-correct
    /// solve. The best rate over the valid triples is the honest statistic.
    pub fn observed_order_above(&self, floor: f64) -> Option<f64> {
        let r = &self.residuals;
        let mut best: Option<f64> = None;
        for w in r.windows(3) {
            let (a, b, c) = (w[0], w[1], w[2]);
            if !(a > floor && b > floor && c > floor) || !(a > b && b > c) {
                continue;
            }
            let p = (c / b).ln() / (b / a).ln();
            if p.is_finite() {
                best = Some(best.map_or(p, |x: f64| x.max(p)));
            }
        }
        best
    }

    /// `observed_order_above` with a floor scaled to the initial residual, which is the usual
    /// thing to want.
    pub fn observed_order(&self) -> Option<f64> {
        let start = self.residuals.first().copied().unwrap_or(1.0).abs().max(1.0);
        self.observed_order_above(start * 1e-14)
    }
}

/// Solve F(x) = 0 from the initial guess in `x`, which is overwritten with the solution.
pub fn solve<S: System>(sys: &mut S, x: &mut [f64], opts: NewtonOpts) -> Report {
    let n = x.len();
    let mut f = vec![0.0; n];
    let mut delta = vec![0.0; n];
    let mut trial = vec![0.0; n];
    let mut residuals = Vec::with_capacity(opts.max_iter + 1);
    let mut backtracks = 0usize;

    sys.residual(x, &mut f);
    let mut norm = norm_inf(&f);
    residuals.push(norm);
    let mut best = norm;

    for iter in 0..opts.max_iter {
        if norm <= opts.tol {
            return Report { outcome: Outcome::Converged, residuals, iterations: iter, backtracks };
        }
        if !norm.is_finite() || norm > best * opts.divergence_factor {
            return Report { outcome: Outcome::Diverged, residuals, iterations: iter, backtracks };
        }

        if !sys.solve_step(x, &f, &mut delta) || delta.iter().any(|d| !d.is_finite()) {
            return Report { outcome: Outcome::LinearSolveFailed, residuals, iterations: iter, backtracks };
        }

        // Damped step: accept the full Newton step when it reduces the residual, otherwise halve.
        // Full steps are what give quadratic convergence, so we always try 1.0 first and only pay
        // for backtracking when the problem demands it.
        let mut step = 1.0;
        let mut accepted = false;
        for attempt in 0..=opts.max_backtracks {
            for i in 0..n {
                trial[i] = x[i] + step * delta[i];
            }
            sys.residual(&trial, &mut f);
            let trial_norm = norm_inf(&f);
            if trial_norm < norm || attempt == opts.max_backtracks {
                x.copy_from_slice(&trial);
                norm = trial_norm;
                accepted = true;
                break;
            }
            step *= 0.5;
            backtracks += 1;
        }
        debug_assert!(accepted);

        // The problem itself may change here (bus-type switching). If it does, the residual we
        // just measured describes the old problem, so re-measure before testing convergence.
        if sys.after_step(x) {
            sys.residual(x, &mut f);
            norm = norm_inf(&f);
        }

        residuals.push(norm);
        best = best.min(norm);
    }

    let outcome = if norm <= opts.tol { Outcome::Converged } else { Outcome::MaxIterations };
    Report { outcome, residuals, iterations: opts.max_iter, backtracks }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Rosenbrock's function as a root problem — the standard hard case: a curved valley where
    /// the full Newton step overshoots badly from a distance.
    struct Rosenbrock;
    impl System for Rosenbrock {
        fn residual(&mut self, x: &[f64], f: &mut [f64]) {
            f[0] = 10.0 * (x[1] - x[0] * x[0]);
            f[1] = 1.0 - x[0];
        }
        fn solve_step(&mut self, x: &[f64], f: &[f64], d: &mut [f64]) -> bool {
            // J = [[-20x0, 10], [-1, 0]]; solve J·d = -f by back-substitution.
            //   row 2:  -d0      = -f1        => d0 = f1
            //   row 1:  -20x0·d0 + 10·d1 = -f0 => d1 = (-f0 + 20·x0·d0)/10
            // The sign on that second term was wrong in the first draft and the damping test
            // caught it — a wrong step direction still converges sometimes, just not from far away.
            d[0] = f[1];
            d[1] = (-f[0] + 20.0 * x[0] * d[0]) / 10.0;
            true
        }
    }

    /// A scalar cubic with a known root, used to measure the convergence order.
    struct Cubic;
    impl System for Cubic {
        fn residual(&mut self, x: &[f64], f: &mut [f64]) {
            f[0] = x[0] * x[0] * x[0] - 2.0 * x[0] - 5.0; // root near 2.0945514815
        }
        fn solve_step(&mut self, x: &[f64], f: &[f64], d: &mut [f64]) -> bool {
            let j = 3.0 * x[0] * x[0] - 2.0;
            if j.abs() < 1e-300 {
                return false;
            }
            d[0] = -f[0] / j;
            true
        }
    }

    #[test]
    fn newton_converges_quadratically_with_a_correct_jacobian() {
        let mut x = [2.0];
        let r = solve(&mut Cubic, &mut x, NewtonOpts { tol: 1e-14, ..Default::default() });
        assert!(r.converged(), "{r:?}");
        assert!((x[0] - 2.094_551_481_542_326).abs() < 1e-13);
        // The signature of correct derivatives: each residual is about the square of the last.
        let order = r.observed_order().expect("enough history");
        assert!(order > 1.7, "order {order} suggests the Jacobian is wrong");
        assert!(r.iterations <= 6, "quadratic convergence should be quick, took {}", r.iterations);
    }

    #[test]
    fn a_frozen_jacobian_converges_only_linearly() {
        // The diagnostic the residual history exists to support.
        //
        // Note what does NOT work as a test here: merely *scaling* the Jacobian wrong. The first
        // draft halved it and expected slow convergence; the backtracking line search simply
        // halved the step back and converged in six iterations. A line search compensates for a
        // scale error, so scale errors are invisible to the iteration count.
        //
        // Freezing the Jacobian at the initial point (the chord method) is a genuine linear
        // iteration: the direction stays right, the curvature information goes stale, and no
        // damping can recover the rate.
        struct Chord(f64);
        impl System for Chord {
            fn residual(&mut self, x: &[f64], f: &mut [f64]) {
                f[0] = x[0] * x[0] * x[0] - 2.0 * x[0] - 5.0;
            }
            fn solve_step(&mut self, _x: &[f64], f: &[f64], d: &mut [f64]) -> bool {
                d[0] = -f[0] / self.0;
                true
            }
        }
        let mut x = [2.0];
        let mut sys = Chord(3.0 * 2.0 * 2.0 - 2.0); // J frozen at x = 2
        let r = solve(&mut sys, &mut x, NewtonOpts { tol: 1e-14, max_iter: 200, ..Default::default() });
        assert!(r.converged(), "{r:?}");
        assert!(
            r.iterations > 8,
            "a frozen Jacobian must converge linearly, not in {} steps",
            r.iterations
        );
        let order = r.observed_order().expect("history");
        assert!(order < 1.4, "chord method should be ~linear, measured order {order}");
    }

    #[test]
    fn damping_rescues_a_start_where_the_full_step_overshoots() {
        let mut x = [-3.0, 9.0];
        let r = solve(&mut Rosenbrock, &mut x, NewtonOpts { tol: 1e-12, ..Default::default() });
        assert!(r.converged(), "{r:?}");
        assert!((x[0] - 1.0).abs() < 1e-10 && (x[1] - 1.0).abs() < 1e-10);
    }

    #[test]
    fn an_already_converged_guess_costs_no_linear_solves() {
        struct Counting(usize);
        impl System for Counting {
            fn residual(&mut self, x: &[f64], f: &mut [f64]) {
                f[0] = x[0];
            }
            fn solve_step(&mut self, _x: &[f64], _f: &[f64], d: &mut [f64]) -> bool {
                self.0 += 1;
                d[0] = 0.0;
                true
            }
        }
        let mut sys = Counting(0);
        let mut x = [0.0];
        let r = solve(&mut sys, &mut x, NewtonOpts::default());
        assert!(r.converged());
        assert_eq!(r.iterations, 0, "no work was needed");
        assert_eq!(sys.0, 0, "and none should have been done");
    }

    #[test]
    fn divergence_and_singularity_are_reported_not_looped_over() {
        struct Singular;
        impl System for Singular {
            fn residual(&mut self, _x: &[f64], f: &mut [f64]) {
                f[0] = 1.0;
            }
            fn solve_step(&mut self, _x: &[f64], _f: &[f64], _d: &mut [f64]) -> bool {
                false
            }
        }
        let mut x = [0.0];
        let r = solve(&mut Singular, &mut x, NewtonOpts::default());
        assert_eq!(r.outcome, Outcome::LinearSolveFailed);
        assert!(r.iterations < 2, "should stop immediately, not spin to max_iter");
    }
}
