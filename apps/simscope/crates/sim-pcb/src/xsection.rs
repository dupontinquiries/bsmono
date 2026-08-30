//! xsection.rs — a board cross-section, meshed and solved.
//!
//! The input is how a fabricator describes a board: layers with thicknesses and permittivities,
//! and a trace of some width on some layer. The output is what a signal integrity engineer needs:
//! characteristic impedance, effective permittivity, propagation delay. In between is the L1 field
//! solve, unchanged.
//!
//! Two modelling decisions worth stating, because both are approximations and both are visible in
//! the results:
//!
//! **The domain is a shielded box.** A real microstrip is open above, with fields extending to
//! infinity; a finite mesh cannot represent that. Enclosing the cross-section in a grounded box far
//! enough away converges to the open case from below in impedance — the lid adds capacitance. The
//! box height is a parameter and `sweep_box_height` exists so that convergence can be watched
//! rather than assumed.
//!
//! **Traces are zero-thickness.** Finite copper thickness raises capacitance by a few percent for
//! typical geometries. Adding it means meshing the conductor's sides, which the tensor grid can do
//! but which is not done here.

use sim_field::{extract, Mesh, Rlgc};

/// One dielectric layer, bottom to top.
#[derive(Clone, Copy, Debug)]
pub struct Layer {
    pub thickness: f64,
    pub eps_r: f64,
}

/// A board cross-section: a stack of dielectrics, with ground planes and one signal trace.
#[derive(Clone, Debug)]
pub struct Stackup {
    pub layers: Vec<Layer>,
    /// Height of air modelled above the stack. Zero means the top of the stack is a ground plane
    /// (stripline); positive means an open surface shielded by a lid at that height (microstrip).
    pub air_above: f64,
    /// Whether the topmost boundary is a ground plane. Always true here — the difference between
    /// stripline and microstrip is whether `air_above` is zero, not whether a lid exists.
    pub ground_top: bool,
}

impl Stackup {
    /// A microstrip: one substrate layer with air and a distant lid above.
    pub fn microstrip(h: f64, eps_r: f64, air: f64) -> Stackup {
        Stackup { layers: vec![Layer { thickness: h, eps_r }], air_above: air, ground_top: true }
    }

    /// A microstrip on a named material from `sim-mat`, using its **in-plane** permittivity.
    ///
    /// Which permittivity is the point. FR-4 is a woven composite and its in-plane and
    /// through-plane values differ by about 10%; a microstrip's field runs mostly vertically
    /// through the substrate, so through-plane is arguably the better choice, and the fact that
    /// there is a choice at all is invisible when the number is a bare `4.4` in a struct literal.
    /// Taking it from the material makes the assumption nameable.
    pub fn from_material(h: f64, mat: &sim_mat::Material, air: f64) -> Result<Stackup, String> {
        let e = mat.electrical.ok_or_else(|| format!("{} has no electrical data", mat.name))?;
        let eps = e
            .eps_r
            .ok_or_else(|| format!("{} has no permittivity; it is not a dielectric", mat.name))?;
        Ok(Stackup { layers: vec![Layer { thickness: h, eps_r: eps.value }], air_above: air, ground_top: true })
    }

    /// A stripline: trace centred between two ground planes, fully embedded.
    pub fn stripline(b: f64, eps_r: f64) -> Stackup {
        Stackup {
            layers: vec![
                Layer { thickness: b / 2.0, eps_r },
                Layer { thickness: b / 2.0, eps_r },
            ],
            air_above: 0.0,
            ground_top: true,
        }
    }

    pub fn total_height(&self) -> f64 {
        self.layers.iter().map(|l| l.thickness).sum::<f64>() + self.air_above
    }

    /// Height of the interface above layer `k` — where a trace on that layer sits.
    pub fn interface_y(&self, k: usize) -> f64 {
        self.layers.iter().take(k + 1).map(|l| l.thickness).sum()
    }

    /// Relative permittivity at height `y`. Air above the stack is 1.
    pub fn eps_at(&self, y: f64) -> f64 {
        let mut acc = 0.0;
        for l in &self.layers {
            acc += l.thickness;
            if y < acc {
                return l.eps_r;
            }
        }
        1.0
    }
}

/// A trace: width `w`, sitting on the interface above layer `on_layer`.
#[derive(Clone, Copy, Debug)]
pub struct Trace {
    pub width: f64,
    pub on_layer: usize,
}

/// Meshing controls.
#[derive(Clone, Copy, Debug)]
pub struct MeshOpts {
    /// Half-width of the modelled domain, in multiples of the stack height.
    pub side_margin: f64,
    /// Cells across the trace width.
    pub across_trace: usize,
    /// Cells through the thinnest layer.
    pub through_layer: usize,
}

impl Default for MeshOpts {
    fn default() -> Self {
        MeshOpts { side_margin: 8.0, across_trace: 12, through_layer: 8 }
    }
}

/// Build the tensor grid for a cross-section.
///
/// Grid lines are placed *at* every dielectric interface and at both trace edges, then filled in
/// between. That is the reason `Mesh::structured` takes explicit coordinates: an interface between
/// nodes cannot be represented, and the resulting error would look like physics.
fn grid(stack: &Stackup, trace: &Trace, opts: &MeshOpts) -> (Vec<f64>, Vec<f64>) {
    let h = stack.total_height();
    let half = (h * opts.side_margin).max(trace.width * 4.0);
    let (x0, x1) = (-trace.width / 2.0, trace.width / 2.0);
    let fine = trace.width / opts.across_trace as f64;

    // Geometric expansion outward from the trace edges.
    //
    // The first version stepped straight from a uniform coarse region into the fine trace region,
    // a 15x jump in cell size in one step, landing the worst elements exactly at the trace edge
    // where the field is strongest and where the r^(-1/3) edge singularity lives. That is not a
    // small effect: it put stripline impedance 33% low. Since a Dirichlet FEM solution bounds
    // energy from above, under-resolution always OVERESTIMATES capacitance and therefore
    // UNDERESTIMATES impedance — the error had the sign the theory predicts.
    let expand = |v: &mut Vec<f64>, from: f64, to: f64, first: f64| {
        let dir = (to - from).signum();
        let mut x = from;
        let mut step = first;
        while (to - x) * dir > step * 1.5 {
            x += dir * step;
            v.push(x);
            step *= 1.2; // gentle growth: 20% per cell keeps neighbouring elements comparable
        }
    };

    let mut xs = vec![x0];
    expand(&mut xs, x0, -half, fine);
    xs.push(-half);
    xs.sort_by(|a, b| a.partial_cmp(b).unwrap());
    for k in 1..=opts.across_trace {
        xs.push(x0 + trace.width * k as f64 / opts.across_trace as f64);
    }
    expand(&mut xs, x1, half, fine);
    xs.push(half);
    xs.sort_by(|a, b| a.partial_cmp(b).unwrap());
    xs.dedup_by(|a, b| (*a - *b).abs() < fine * 1e-6);

    // Vertically: resolve each layer, and grade the air region away from the trace since the field
    // decays with height.
    let mut ys = vec![0.0];
    for l in &stack.layers {
        let base = *ys.last().unwrap();
        for k in 1..=opts.through_layer {
            ys.push(base + l.thickness * k as f64 / opts.through_layer as f64);
        }
    }
    if stack.air_above > 0.0 {
        let base = *ys.last().unwrap();
        let sub = stack.layers.iter().map(|l| l.thickness).sum::<f64>();
        let mut y = base;
        let mut step = sub / opts.through_layer as f64;
        while base + stack.air_above - y > step * 1.5 {
            y += step;
            ys.push(y);
            step *= 1.25;
        }
        ys.push(base + stack.air_above);
    }
    (xs, ys)
}

/// Solve a cross-section and return its line parameters.
pub fn solve(stack: &Stackup, trace: &Trace, opts: &MeshOpts) -> Result<Rlgc, String> {
    if trace.on_layer >= stack.layers.len() {
        return Err(format!(
            "trace is on layer {} but the stack has {}",
            trace.on_layer,
            stack.layers.len()
        ));
    }
    let (xs, ys) = grid(stack, trace, opts);
    let mesh = Mesh::structured(&xs, &ys);
    let tol = trace.width * 1e-9;

    // Signal: the trace itself.
    let ty = stack.interface_y(trace.on_layer);
    let signal = mesh.nodes_on_segment([-trace.width / 2.0, ty], [trace.width / 2.0, ty], tol);
    if signal.len() < 3 {
        return Err(format!("trace missed the grid: only {} nodes found", signal.len()));
    }

    // Ground: bottom plane, top plane, and the side walls of the shielding box.
    let (x_lo, x_hi) = (xs[0], *xs.last().unwrap());
    let y_hi = *ys.last().unwrap();
    let mut ground = mesh.nodes_on_segment([x_lo, 0.0], [x_hi, 0.0], tol);
    ground.extend(mesh.nodes_on_segment([x_lo, y_hi], [x_hi, y_hi], tol));
    ground.extend(mesh.nodes_on_segment([x_lo, 0.0], [x_lo, y_hi], tol));
    ground.extend(mesh.nodes_on_segment([x_hi, 0.0], [x_hi, y_hi], tol));
    ground.sort_unstable();
    ground.dedup();
    ground.retain(|g| !signal.contains(g));

    extract(&mesh, &signal, &ground, |t| stack.eps_at(mesh.centroid(t)[1]))
}

/// Solve the same cross-section at increasing lid heights, so the approach to the open case can be
/// observed instead of assumed.
pub fn sweep_box_height(stack: &Stackup, trace: &Trace, margins: &[f64]) -> Vec<(f64, f64)> {
    margins
        .iter()
        .filter_map(|&m| {
            let opts = MeshOpts { side_margin: m, ..Default::default() };
            let mut s = stack.clone();
            if s.air_above > 0.0 {
                s.air_above = s.layers.iter().map(|l| l.thickness).sum::<f64>() * m;
            }
            solve(&s, trace, &opts).ok().map(|r| (m, r.z0))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::formulas;

    #[test]
    fn stripline_effective_permittivity_is_exactly_the_dielectric() {
        // The sharpest check available at this level, because it is exact physics rather than a
        // curve fit: a stripline is homogeneous, so the mode sees the substrate and nothing else.
        // Any leak in the mixed-dielectric path — a mis-assigned element, an interface off the
        // grid — shows up here immediately.
        for er in [2.2, 4.4, 10.2] {
            let stack = Stackup::stripline(1.0e-3, er);
            let trace = Trace { width: 0.5e-3, on_layer: 0 };
            let r = solve(&stack, &trace, &MeshOpts::default()).unwrap();
            assert!(
                (r.eps_eff - er).abs() / er < 2e-3,
                "eps_r = {er}: solver says eps_eff = {}",
                r.eps_eff
            );
            // And therefore the phase velocity is c0/sqrt(er) exactly.
            let want_v = sim_field::C0 / er.sqrt();
            assert!((r.v_phase - want_v).abs() / want_v < 2e-3);
        }
    }

    #[test]
    fn stripline_impedance_lands_within_the_formulas_stated_accuracy() {
        let (b, er) = (1.0e-3, 4.2);
        for w in [0.4e-3, 0.6e-3, 0.9e-3] {
            let stack = Stackup::stripline(b, er);
            let trace = Trace { width: w, on_layer: 0 };
            let r = solve(&stack, &trace, &MeshOpts::default()).unwrap();
            let want = formulas::stripline_z0(w, b, er);
            let rel = (r.z0 - want).abs() / want;
            assert!(rel < 0.08, "w={w:e}: solver {:.1} vs Wheeler {:.1} ({:.1}%)", r.z0, want, rel * 100.0);
        }
    }

    #[test]
    fn microstrip_permittivity_sits_between_air_and_substrate() {
        // The defining property of an inhomogeneous line, and the thing a homogeneous solver would
        // get wrong by returning either 1 or er.
        let er = 4.4;
        let stack = Stackup::microstrip(0.2e-3, er, 0.2e-3 * 12.0);
        let trace = Trace { width: 0.38e-3, on_layer: 0 };
        let r = solve(&stack, &trace, &MeshOpts::default()).unwrap();
        assert!(r.eps_eff > 1.2 && r.eps_eff < er, "eps_eff {} not between air and {er}", r.eps_eff);
        // Hammerstad for the open case; the shielded solve should be in the same neighbourhood.
        let want = formulas::microstrip_eps_eff(0.38e-3, 0.2e-3, er);
        assert!(
            (r.eps_eff - want).abs() / want < 0.12,
            "eps_eff {} vs Hammerstad {want}",
            r.eps_eff
        );
    }

    #[test]
    fn a_taller_shielding_box_raises_the_impedance_toward_the_open_case() {
        // The lid adds capacitance, so a low box under-reports Z0. Watching the sweep converge is
        // the honest way to use a finite domain for an open problem — the alternative is picking a
        // margin and hoping.
        let stack = Stackup::microstrip(0.2e-3, 4.4, 0.2e-3 * 4.0);
        let trace = Trace { width: 0.38e-3, on_layer: 0 };
        let sweep = sweep_box_height(&stack, &trace, &[2.0, 4.0, 8.0, 16.0]);
        assert_eq!(sweep.len(), 4);
        for w in sweep.windows(2) {
            assert!(w[1].1 > w[0].1, "Z0 should rise with box height: {sweep:?}");
        }
        // And the increments must shrink — otherwise it is not converging to anything.
        let d1 = sweep[1].1 - sweep[0].1;
        let d3 = sweep[3].1 - sweep[2].1;
        assert!(d3 < d1 * 0.6, "box-height convergence stalled: {sweep:?}");
    }

    #[test]
    fn wider_traces_lower_the_impedance_in_the_solver_too() {
        let stack = Stackup::microstrip(0.2e-3, 4.4, 0.2e-3 * 12.0);
        let mut last = f64::INFINITY;
        for w in [0.2e-3, 0.4e-3, 0.8e-3] {
            let r = solve(&stack, &Trace { width: w, on_layer: 0 }, &MeshOpts::default()).unwrap();
            assert!(r.z0 < last, "Z0 rose with width: {} after {last}", r.z0);
            last = r.z0;
        }
    }

    #[test]
    fn a_stackup_can_be_built_from_the_material_library() {
        // The cross-layer join: L2 no longer carries its own idea of what FR-4 is.
        let fr4 = sim_mat::library::fr4();
        let stack = Stackup::from_material(0.2e-3, &fr4, 0.2e-3 * 12.0).unwrap();
        let r = solve(&stack, &Trace { width: 0.38e-3, on_layer: 0 }, &MeshOpts::default()).unwrap();
        assert!(r.z0 > 30.0 && r.z0 < 70.0, "FR-4 microstrip Z0 came out {:.1}", r.z0);
        assert!(r.eps_eff > 1.0 && r.eps_eff < 4.4);

        // A conductor is not a dielectric, and asking for its permittivity must fail rather than
        // silently returning a default.
        let cu = sim_mat::library::copper();
        assert!(Stackup::from_material(1e-3, &cu, 0.0).is_err());
    }

    #[test]
    fn a_trace_that_misses_the_grid_is_reported() {
        let stack = Stackup::microstrip(0.2e-3, 4.4, 1.0e-3);
        // interface_y does not bounds-check — take(k+1) on a shorter list silently returns the
        // top of the stack — so a bad layer index would otherwise land on a real surface and
        // produce a plausible number. The check has to be explicit.
        let trace = Trace { width: 0.38e-3, on_layer: 7 };
        let err = solve(&stack, &trace, &MeshOpts::default()).unwrap_err();
        assert!(err.contains("layer 7"), "{err}");
    }
}
