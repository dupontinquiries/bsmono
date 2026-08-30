//! arclength.rs — following a response that doubles back.
//!
//! Newton with prescribed load fails at a limit point, because past the peak no load produces
//! equilibrium. Newton with prescribed displacement fails at a *snap-back*, where the structure
//! releases faster than it is being pulled and the response turns back in displacement too. Both
//! are real behaviours of softening structures, and neither is a numerical artefact — a cracking
//! interface in a compliant surround does exactly this.
//!
//! Arc-length control fixes it by making neither load nor displacement the independent variable.
//! Instead it advances a fixed distance along the equilibrium path in `(u, λ)` space, so the path
//! can turn however it likes and the solver simply follows it.
//!
//! The augmented system is bordered, and rather than factorise the bordered matrix the standard
//! decomposition is used: solve the tangent twice, once against the residual and once against the
//! reference load, then combine them so the arc-length constraint is satisfied. That reduces the
//! constraint to a **quadratic in one scalar**, and choosing between its two roots is the whole
//! difficulty — take the wrong one and the solver retraces the path it just came along.

/// A structure whose equilibrium path is to be traced.
pub trait ArcSystem {
    fn dim(&self) -> usize;
    /// Internal force at displacement `u`.
    fn internal(&mut self, u: &[f64], f: &mut [f64]);
    /// The reference external load pattern; the load factor scales it.
    fn external(&self) -> Vec<f64>;
    /// Solve `K(u) x = rhs` with the current tangent. `None` if the tangent is singular.
    fn solve_tangent(&mut self, u: &[f64], rhs: &[f64]) -> Option<Vec<f64>>;
    /// Commit path-dependent state once a step has converged.
    fn commit(&mut self, _u: &[f64]) {}
}

#[derive(Clone, Debug)]
pub struct ArcPoint {
    pub u: Vec<f64>,
    pub lambda: f64,
    pub iterations: usize,
}

#[derive(Clone, Copy, Debug)]
pub struct ArcOpts {
    /// Arc length per step, in displacement units.
    pub step: f64,
    pub max_steps: usize,
    pub max_iter: usize,
    /// Convergence tolerance on the residual, scaled to the reference load.
    pub tol: f64,
}

impl Default for ArcOpts {
    fn default() -> Self {
        ArcOpts { step: 1e-6, max_steps: 500, max_iter: 40, tol: 1e-10 }
    }
}

fn dot(a: &[f64], b: &[f64]) -> f64 {
    crate::kernel::dot_compensated(a, b)
}

/// Trace an equilibrium path from `(u0, 0)`.
pub fn trace<S: ArcSystem>(sys: &mut S, u0: &[f64], opts: ArcOpts) -> Vec<ArcPoint> {
    let n = sys.dim();
    let fext = sys.external();
    let scale = dot(&fext, &fext).sqrt().max(1e-300);
    let mut u = u0.to_vec();
    let mut lambda = 0.0f64;
    let mut path = vec![ArcPoint { u: u.clone(), lambda, iterations: 0 }];
    // Direction of the previous step, used to keep going forwards.
    let mut prev_du = vec![0.0; n];
    let mut prev_dlam = 0.0f64;

    for _ in 0..opts.max_steps {
        // Predictor: a step along the tangent to the load pattern.
        let Some(du_f) = sys.solve_tangent(&u, &fext) else { break };
        let norm = dot(&du_f, &du_f).sqrt();
        if !(norm > 0.0) {
            break;
        }
        let mut sign = 1.0;
        if dot(&prev_du, &du_f) < 0.0 || (prev_dlam < 0.0 && dot(&prev_du, &du_f) == 0.0) {
            sign = -1.0;
        }
        // On the very first step there is no history; go in the direction that raises the load.
        if path.len() == 1 {
            sign = 1.0;
        }
        let d_lam0 = sign * opts.step / norm;
        let mut delta_u: Vec<f64> = du_f.iter().map(|v| v * d_lam0).collect();
        let mut delta_lam = d_lam0;

        // Corrector.
        let mut converged = false;
        let mut iters = 0;
        for it in 0..opts.max_iter {
            iters = it + 1;
            let trial: Vec<f64> = (0..n).map(|i| u[i] + delta_u[i]).collect();
            let trial_lam = lambda + delta_lam;
            let mut fint = vec![0.0; n];
            sys.internal(&trial, &mut fint);
            let r: Vec<f64> = (0..n).map(|i| fint[i] - trial_lam * fext[i]).collect();
            if dot(&r, &r).sqrt() <= opts.tol * scale {
                converged = true;
                break;
            }
            let neg_r: Vec<f64> = r.iter().map(|v| -v).collect();
            let (Some(du_r), Some(du_f2)) = (
                sys.solve_tangent(&trial, &neg_r),
                sys.solve_tangent(&trial, &fext),
            ) else {
                break;
            };
            // Constraint |delta_u + du_r + dmu*du_f|^2 = step^2, a quadratic in dmu.
            let base: Vec<f64> = (0..n).map(|i| delta_u[i] + du_r[i]).collect();
            let a = dot(&du_f2, &du_f2);
            let b = 2.0 * dot(&base, &du_f2);
            let c = dot(&base, &base) - opts.step * opts.step;
            let disc = b * b - 4.0 * a * c;
            let dmu = if a.abs() < 1e-300 || disc < 0.0 {
                // No real root: fall back to the plain Newton correction. Rare, and letting the
                // arc length drift for one iteration beats abandoning the step.
                0.0
            } else {
                let s = disc.sqrt();
                let r1 = (-b + s) / (2.0 * a);
                let r2 = (-b - s) / (2.0 * a);
                // Pick the root that keeps going the way we came. Taking the other one makes the
                // solver retrace the path it just walked, which looks like convergence and is the
                // classic way an arc-length implementation silently produces a loop.
                let cand = |m: f64| {
                    let step: Vec<f64> = (0..n).map(|i| base[i] + m * du_f2[i]).collect();
                    dot(&step, &delta_u)
                };
                if cand(r1) >= cand(r2) { r1 } else { r2 }
            };
            for i in 0..n {
                delta_u[i] = base[i] + dmu * du_f2[i];
            }
            delta_lam += dmu;
        }
        if !converged {
            break;
        }
        for i in 0..n {
            u[i] += delta_u[i];
        }
        lambda += delta_lam;
        sys.commit(&u);
        prev_du = delta_u.clone();
        prev_dlam = delta_lam;
        path.push(ArcPoint { u: u.clone(), lambda, iterations: iters });
    }
    path
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A single degree of freedom with a softening force law: `F(u) = k u` up to `u0`, then falling
    /// linearly to zero at `uf`. In series with a spring of stiffness `ks`, which is what creates
    /// the snap-back when `ks` is small.
    struct SofteningSpring {
        k: f64,
        u0: f64,
        uf: f64,
    }

    impl SofteningSpring {
        fn force(&self, u: f64) -> f64 {
            if u <= self.u0 {
                self.k * u
            } else if u >= self.uf {
                0.0
            } else {
                self.k * self.u0 * (self.uf - u) / (self.uf - self.u0)
            }
        }
        fn stiffness(&self, u: f64) -> f64 {
            if u <= self.u0 {
                self.k
            } else if u >= self.uf {
                0.0
            } else {
                -self.k * self.u0 / (self.uf - self.u0)
            }
        }
    }

    impl ArcSystem for SofteningSpring {
        fn dim(&self) -> usize {
            1
        }
        fn internal(&mut self, u: &[f64], f: &mut [f64]) {
            f[0] = self.force(u[0]);
        }
        fn external(&self) -> Vec<f64> {
            vec![1.0]
        }
        fn solve_tangent(&mut self, u: &[f64], rhs: &[f64]) -> Option<Vec<f64>> {
            let kt = self.stiffness(u[0]);
            if kt.abs() < 1e-300 {
                return None;
            }
            Some(vec![rhs[0] / kt])
        }
    }

    #[test]
    fn it_traces_past_a_limit_point_where_load_control_cannot() {
        // Past the peak there is no load that produces equilibrium, so load control stops dead.
        // Arc-length keeps going, and the load factor must come back DOWN — that decrease is the
        // whole point.
        let mut sys = SofteningSpring { k: 1e6, u0: 1e-3, uf: 5e-3 };
        let path = trace(
            &mut sys,
            &[0.0],
            ArcOpts { step: 5e-5, max_steps: 300, tol: 1e-12, ..Default::default() },
        );
        assert!(path.len() > 50, "should have traced a long path, got {}", path.len());
        let peak = path.iter().fold(0.0f64, |m, p| m.max(p.lambda));
        assert!((peak / (1e6 * 1e-3) - 1.0).abs() < 0.05, "peak load {peak:e}");
        let last = path.last().unwrap();
        assert!(last.lambda < peak * 0.2, "load must fall away after the peak: {}", last.lambda);
        assert!(last.u[0] > sys.u0, "and displacement must keep growing: {}", last.u[0]);
    }

    #[test]
    fn displacement_increases_monotonically_when_there_is_no_snap_back() {
        // A pure limit point turns the load around but not the displacement. Checking that the
        // tracer does not invent a snap-back where none exists is as important as tracing one.
        let mut sys = SofteningSpring { k: 1e6, u0: 1e-3, uf: 5e-3 };
        let path = trace(
            &mut sys,
            &[0.0],
            ArcOpts { step: 5e-5, max_steps: 200, tol: 1e-12, ..Default::default() },
        );
        for w in path.windows(2) {
            assert!(
                w[1].u[0] >= w[0].u[0] - 1e-12,
                "displacement went backwards without a snap-back: {} then {}",
                w[0].u[0],
                w[1].u[0]
            );
        }
    }

    #[test]
    fn the_traced_path_satisfies_equilibrium_at_every_point() {
        // The check that matters: every point on the path must actually be a solution, not merely
        // a place the algorithm stopped.
        let mut sys = SofteningSpring { k: 1e6, u0: 1e-3, uf: 5e-3 };
        let path = trace(
            &mut sys,
            &[0.0],
            ArcOpts { step: 5e-5, max_steps: 200, tol: 1e-12, ..Default::default() },
        );
        for p in &path {
            let mut f = vec![0.0];
            sys.internal(&p.u, &mut f);
            let residual = f[0] - p.lambda;
            assert!(
                residual.abs() < 1e-6,
                "not in equilibrium at u = {:e}: residual {residual:e}",
                p.u[0]
            );
        }
    }

    #[test]
    fn steps_advance_by_the_requested_arc_length() {
        let mut sys = SofteningSpring { k: 1e6, u0: 1e-3, uf: 5e-3 };
        let step = 4e-5;
        let path = trace(
            &mut sys,
            &[0.0],
            ArcOpts { step, max_steps: 100, tol: 1e-12, ..Default::default() },
        );
        for w in path.windows(2).skip(1) {
            let d = (w[1].u[0] - w[0].u[0]).abs();
            assert!(
                (d / step - 1.0).abs() < 0.05,
                "step advanced {d:e}, expected {step:e}"
            );
        }
    }
}
