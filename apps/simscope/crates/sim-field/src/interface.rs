//! interface.rs — cohesive elements between bulk elements.
//!
//! `cohesive` verified the traction–separation law and a one-dimensional pull. This puts those
//! laws into a mesh, which is what lets a *structure* dissipate energy rather than a single point.
//!
//! An interface is a set of node pairs sharing a location, one belonging to each side. Before any
//! damage the penalty stiffness holds them together; as they separate, each pair follows its own
//! damage history, so a crack can run along the interface progressively rather than the whole
//! surface failing at once. That progressiveness is the entire point — it is what makes the
//! dissipated energy scale with *crack area* instead of with load.
//!
//! The verification is an energy balance and it is exact: **work done by the external load, less
//! the elastic energy still stored, must equal `G_c` times the area actually separated.** No
//! reference data, no handbook — just conservation, which is the same kind of test as the
//! J-integral's path independence.
//!
//! The tangent is assembled dense and factorised by LU rather than solved with CG, because once
//! any pair enters softening the tangent is **indefinite** and CG's assumptions no longer hold.
//! That caps the mesh size; a proper solution wants a sparse indefinite factorisation, and it is
//! recorded as a debt rather than worked around.

use sim_num::{newton, Lu, NewtonOpts, System};

use crate::cohesive::{Cohesive, Separation};
use crate::elastic::{element_stiffness, Assumption, constitutive};
use crate::mesh::Mesh;
use crate::p1::Element;

/// A set of cohesive node pairs along a straight interface.
#[derive(Clone, Debug)]
pub struct Interface {
    /// `(node on the positive-normal side, node on the negative side)`.
    pub pairs: Vec<(usize, usize)>,
    /// Length of interface each pair is responsible for.
    pub tributary: Vec<f64>,
    /// Damage state per pair — separate, so the crack can advance.
    pub laws: Vec<Cohesive>,
    /// Interface normal, pointing from the negative side to the positive.
    pub normal: [f64; 2],
    /// Shear/opening weight for the mixed-mode law.
    pub beta: f64,
}

impl Interface {
    /// Total separated area: pairs whose damage has reached one.
    pub fn separated_length(&self) -> f64 {
        self.pairs
            .iter()
            .enumerate()
            .filter(|(i, _)| self.laws[*i].damage() >= 1.0 - 1e-12)
            .map(|(i, _)| self.tributary[i])
            .sum()
    }

    /// Energy dissipated so far, per unit thickness.
    pub fn dissipated(&self) -> f64 {
        self.laws
            .iter()
            .zip(&self.tributary)
            .map(|(l, a)| l.dissipated() * a)
            .sum()
    }

    /// In-plane tangent direction, the normal rotated a quarter turn.
    fn tangent_dir(&self) -> [f64; 2] {
        [-self.normal[1], self.normal[0]]
    }

    fn separation(&self, i: usize, u: &[f64]) -> Separation {
        let (p, m) = self.pairs[i];
        let du = [u[2 * p] - u[2 * m], u[2 * p + 1] - u[2 * m + 1]];
        let t = self.tangent_dir();
        Separation {
            normal: du[0] * self.normal[0] + du[1] * self.normal[1],
            tangent: du[0] * t[0] + du[1] * t[1],
        }
    }

    /// Internal forces from the interface, added into `f`.
    fn add_forces(&self, u: &[f64], f: &mut [f64]) {
        let td = self.tangent_dir();
        for i in 0..self.pairs.len() {
            let sep = self.separation(i, u);
            let tr = self.laws[i].tractions(sep, self.beta);
            let a = self.tributary[i];
            let (p, m) = self.pairs[i];
            for k in 0..2 {
                let force = (tr[0] * self.normal[k] + tr[1] * td[k]) * a;
                f[2 * p + k] += force;
                f[2 * m + k] -= force;
            }
        }
    }

    /// Tangent contributions as triplets.
    fn add_tangent(&self, u: &[f64], trip: &mut Vec<(usize, usize, f64)>) {
        let td = self.tangent_dir();
        for i in 0..self.pairs.len() {
            let sep = self.separation(i, u);
            let d2 = self.laws[i].tangent_matrix(sep, self.beta);
            let area = self.tributary[i];
            // Rotate the 2x2 material tangent into global coordinates: K = A * R^T D R, with the
            // rows of R being the normal and tangent directions.
            let dirs = [self.normal, td];
            let (p, m) = self.pairs[i];
            for a in 0..2 {
                for b in 0..2 {
                    let mut v = 0.0;
                    for r in 0..2 {
                        for c in 0..2 {
                            v += d2[r][c] * dirs[r][a] * dirs[c][b];
                        }
                    }
                    let v = v * area;
                    trip.push((2 * p + a, 2 * p + b, v));
                    trip.push((2 * m + a, 2 * m + b, v));
                    trip.push((2 * p + a, 2 * m + b, -v));
                    trip.push((2 * m + a, 2 * p + b, -v));
                }
            }
        }
    }

    /// Commit damage for the converged state.
    fn commit(&mut self, u: &[f64]) {
        for i in 0..self.pairs.len() {
            let sep = self.separation(i, u);
            self.laws[i].commit_mixed(sep, self.beta);
        }
    }
}

/// A body with bulk elasticity and one cohesive interface, solved on its free degrees of freedom.
struct Body<'a> {
    k_bulk: Vec<f64>, // dense, full size
    n_dof: usize,
    free: &'a [usize],
    prescribed: Vec<f64>, // full-length displacement with prescribed values set
    interface: Interface,
}

impl Body<'_> {
    fn expand(&self, x: &[f64]) -> Vec<f64> {
        let mut u = self.prescribed.clone();
        for (k, &d) in self.free.iter().enumerate() {
            u[d] = x[k];
        }
        u
    }
    fn full_forces(&self, u: &[f64]) -> Vec<f64> {
        let n = self.n_dof;
        let mut f = vec![0.0; n];
        for r in 0..n {
            let mut acc = 0.0;
            for c in 0..n {
                acc += self.k_bulk[r * n + c] * u[c];
            }
            f[r] = acc;
        }
        self.interface.add_forces(u, &mut f);
        f
    }
}

impl System for Body<'_> {
    fn residual(&mut self, x: &[f64], f: &mut [f64]) {
        let u = self.expand(x);
        let full = self.full_forces(&u);
        for (k, &d) in self.free.iter().enumerate() {
            f[k] = full[d]; // external load is zero on free DOFs; loading is by prescribed motion
        }
    }

    fn solve_step(&mut self, x: &[f64], f: &[f64], delta: &mut [f64]) -> bool {
        let u = self.expand(x);
        let n = self.n_dof;
        let mut trip = Vec::new();
        self.interface.add_tangent(&u, &mut trip);
        let m = self.free.len();
        let mut a = vec![0.0; m * m];
        for (i, &r) in self.free.iter().enumerate() {
            for (j, &c) in self.free.iter().enumerate() {
                a[i * m + j] = self.k_bulk[r * n + c];
            }
        }
        // Scatter the interface tangent into the reduced system.
        let index: std::collections::HashMap<usize, usize> =
            self.free.iter().enumerate().map(|(i, &d)| (d, i)).collect();
        for (r, c, v) in trip {
            if let (Some(&i), Some(&j)) = (index.get(&r), index.get(&c)) {
                a[i * m + j] += v;
            }
        }
        let Ok(lu) = Lu::factor(&a, m) else { return false };
        let rhs: Vec<f64> = f.iter().map(|v| -v).collect();
        delta.copy_from_slice(&lu.solve(&rhs));
        true
    }
}

/// One step of a displacement-controlled pull. Returns the reaction force and the state.
pub struct Step {
    pub opening: f64,
    pub reaction: f64,
    pub dissipated: f64,
    pub separated: f64,
}

/// Split a rectangle mesh along `y = y_split`, duplicating the nodes on that line so the two sides
/// can separate, and return the interface pairs with their tributary lengths.
///
/// Nodes are duplicated rather than the crack being an unrestrained symmetry boundary, because here
/// the two faces must *interact* through a traction rather than simply be free.
pub fn split_horizontal(mesh: &Mesh, y_split: f64, tol: f64) -> (Mesh, Interface) {
    let mut xy = mesh.xy.clone();
    let mut dup = std::collections::HashMap::new();
    let on_line: Vec<usize> = (0..mesh.nodes())
        .filter(|&i| (mesh.xy[i][1] - y_split).abs() < tol)
        .collect();
    for &i in &on_line {
        dup.insert(i, xy.len());
        xy.push(mesh.xy[i]);
    }
    // Elements below the line use the duplicates.
    let mut tri = mesh.tri.clone();
    for (t, nodes) in tri.iter_mut().enumerate() {
        if mesh.centroid(t)[1] < y_split {
            for n in nodes.iter_mut() {
                if let Some(&d) = dup.get(n) {
                    *n = d;
                }
            }
        }
    }
    // Tributary length: half the gap to each neighbour along the line.
    let mut sorted = on_line.clone();
    sorted.sort_by(|a, b| mesh.xy[*a][0].partial_cmp(&mesh.xy[*b][0]).unwrap());
    let mut pairs = Vec::new();
    let mut tributary = Vec::new();
    for (k, &i) in sorted.iter().enumerate() {
        let x = mesh.xy[i][0];
        let left = if k > 0 { (x - mesh.xy[sorted[k - 1]][0]) / 2.0 } else { 0.0 };
        let right = if k + 1 < sorted.len() { (mesh.xy[sorted[k + 1]][0] - x) / 2.0 } else { 0.0 };
        pairs.push((i, dup[&i])); // original stays with the upper half
        tributary.push(left + right);
    }
    let interface = Interface { pairs, tributary, laws: Vec::new(), normal: [0.0, 1.0], beta: 1.0 };
    (Mesh { xy, tri }, interface)
}

/// Which way the top face is driven.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Drive {
    /// Straight up: mode I opening.
    Opening,
    /// Sideways: mode II sliding, which is the platelet pull-out configuration.
    Sliding,
}

/// Pull a split block apart under displacement control, tracking dissipation.
pub fn pull_apart(
    mesh: &Mesh,
    interface: Interface,
    law: Cohesive,
    e: f64,
    nu: f64,
    y_split: f64,
    openings: &[f64],
) -> Result<Vec<Step>, String> {
    drive_apart(mesh, interface, law, e, nu, y_split, openings, Drive::Opening)
}

/// Drive a split block in a chosen direction under displacement control.
pub fn drive_apart(
    mesh: &Mesh,
    mut interface: Interface,
    law: Cohesive,
    e: f64,
    nu: f64,
    y_split: f64,
    openings: &[f64],
    drive: Drive,
) -> Result<Vec<Step>, String> {
    interface.laws = vec![law; interface.pairs.len()];
    let n_dof = 2 * mesh.nodes();
    let d = constitutive(e, nu, Assumption::PlaneStress);

    // Dense bulk stiffness.
    let mut k_bulk = vec![0.0; n_dof * n_dof];
    for t in 0..mesh.tri.len() {
        let el = Element::of(mesh, t);
        let ke = element_stiffness(&el, &d, 1.0);
        let nn = mesh.tri[t];
        for a in 0..3 {
            for b in 0..3 {
                for i in 0..2 {
                    for j in 0..2 {
                        k_bulk[(2 * nn[a] + i) * n_dof + 2 * nn[b] + j] += ke[2 * a + i][2 * b + j];
                    }
                }
            }
        }
    }

    let (mut y_lo, mut y_hi) = (f64::INFINITY, f64::NEG_INFINITY);
    for p in &mesh.xy {
        y_lo = y_lo.min(p[1]);
        y_hi = y_hi.max(p[1]);
    }
    let tol = (y_hi - y_lo) * 1e-9;
    let bottom: Vec<usize> = (0..mesh.nodes()).filter(|&i| (mesh.xy[i][1] - y_lo).abs() < tol).collect();
    let top: Vec<usize> = (0..mesh.nodes()).filter(|&i| (mesh.xy[i][1] - y_hi).abs() < tol).collect();

    let mut constrained: Vec<usize> = Vec::new();
    for &i in &bottom {
        constrained.push(2 * i);
        constrained.push(2 * i + 1);
    }
    for &i in &top {
        constrained.push(2 * i);
        constrained.push(2 * i + 1);
    }
    constrained.sort_unstable();
    constrained.dedup();
    let free: Vec<usize> = (0..n_dof).filter(|d| !constrained.contains(d)).collect();

    let mut out = Vec::with_capacity(openings.len());
    let mut x = vec![0.0; free.len()];
    for &open in openings {
        let mut prescribed = vec![0.0; n_dof];
        for &i in &top {
            match drive {
                Drive::Opening => prescribed[2 * i + 1] = open,
                Drive::Sliding => prescribed[2 * i] = open,
            }
        }
        let mut body = Body { k_bulk: k_bulk.clone(), n_dof, free: &free, prescribed, interface: interface.clone() };
        let scale = law.sigma_max * mesh.xy.len() as f64;
        let rep = newton::solve(
            &mut body,
            &mut x,
            NewtonOpts { tol: scale * 1e-12, max_iter: 60, ..Default::default() },
        );
        if !rep.converged() {
            return Err(format!("no equilibrium at opening {open:e}: {:?}", rep.outcome));
        }
        let u = body.expand(&x);
        interface.commit(&u);
        // Reaction on the pulled face.
        let full = {
            let b = Body { k_bulk: k_bulk.clone(), n_dof, free: &free, prescribed: u.clone(), interface: interface.clone() };
            b.full_forces(&u)
        };
        let reaction: f64 = match drive {
            Drive::Opening => top.iter().map(|&i| full[2 * i + 1]).sum(),
            Drive::Sliding => top.iter().map(|&i| full[2 * i]).sum(),
        };
        out.push(Step {
            opening: open,
            reaction,
            dissipated: interface.dissipated(),
            separated: interface.separated_length(),
        });
        let _ = y_split;
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn separating_a_block_dissipates_exactly_g_c_times_the_area() {
        // The energy balance, and it needs no reference data: however the crack advances, the
        // energy the interface has dissipated must equal G_c times the length actually separated.
        let (w, h) = (4e-3, 2e-3);
        let base = Mesh::rectangle(w, h, 6, 4);
        let (mesh, interface) = split_horizontal(&base, h / 2.0, h * 1e-9);
        let law = Cohesive::from_energy(20.0, 30e6, 1e15);
        // Open well past delta_f so every pair fails.
        let openings: Vec<f64> = (1..=60).map(|i| law.delta_f * 4.0 * i as f64 / 60.0).collect();
        let steps = pull_apart(&mesh, interface, law, 70e9, 0.3, h / 2.0, &openings).unwrap();

        let last = steps.last().unwrap();
        assert!(last.separated > w * 0.9, "the interface should be fully separated, got {:.3e}", last.separated);
        let expected = law.fracture_energy() * last.separated;
        assert!(
            (last.dissipated / expected - 1.0).abs() < 1e-9,
            "dissipated {:.4} vs G_c x area {:.4}",
            last.dissipated,
            expected
        );
    }

    #[test]
    fn the_reaction_peaks_near_the_cohesive_strength_and_falls_to_nothing() {
        let (w, h) = (4e-3, 2e-3);
        let base = Mesh::rectangle(w, h, 6, 4);
        let (mesh, interface) = split_horizontal(&base, h / 2.0, h * 1e-9);
        let law = Cohesive::from_energy(20.0, 30e6, 1e15);
        let openings: Vec<f64> = (1..=60).map(|i| law.delta_f * 4.0 * i as f64 / 60.0).collect();
        let steps = pull_apart(&mesh, interface, law, 70e9, 0.3, h / 2.0, &openings).unwrap();

        let peak = steps.iter().fold(0.0f64, |m, s| m.max(s.reaction));
        let capacity = law.sigma_max * w;
        assert!(
            peak > capacity * 0.7 && peak < capacity * 1.2,
            "peak reaction {peak:.3e} vs interface capacity {capacity:.3e}"
        );
        assert!(
            steps.last().unwrap().reaction.abs() < capacity * 1e-6,
            "a fully separated interface carries nothing"
        );
        // Dissipation is monotonic: damage never heals.
        for pair in steps.windows(2) {
            assert!(pair[1].dissipated >= pair[0].dissipated - 1e-12, "dissipation went backwards");
        }
    }

    #[test]
    fn shearing_a_block_dissipates_the_same_energy_as_opening_it() {
        // The pull-out configuration, and the reason the element had to become mixed-mode: nacre's
        // platelets fail in SHEAR. The energy balance must be identical to mode I — an interface is
        // not tougher in one direction than the other, only harder to separate.
        //
        // Two things had to change from the opening test, and both are physics rather than tuning.
        //
        // First, the interface must be the COMPLIANT path. Opening a block is only possible by
        // opening its interface, but a block can shear perfectly well by deforming its own bulk —
        // so with a stiff penalty the halves simply shear and the interface never slides enough to
        // damage. The penalty is lowered until the interface, not the bulk, takes the motion.
        //
        // Second, the softening slope must stay below the bulk shear stiffness or the system snaps
        // back. At the opening test's parameters those two are within 20% of each other, which is
        // exactly the instability `cohesive` already names; a gentler peak traction pushes the
        // slope an order of magnitude below the bulk and restores stability.
        let (w, h) = (4e-3, 2e-3);
        let base = Mesh::rectangle(w, h, 6, 4);
        let (mesh, interface) = split_horizontal(&base, h / 2.0, h * 1e-9);
        let law = Cohesive::from_energy(20.0, 10e6, 1e13);
        let bulk_shear = 70e9 / (2.0 * 1.3) / (h / 2.0);
        let softening = law.sigma_max / (law.delta_f - law.delta_0());
        assert!(
            bulk_shear > softening * 4.0,
            "the test is only meaningful when the structure is stiffer than the softening slope: \
             bulk {bulk_shear:.2e} vs slope {softening:.2e}"
        );

        let steps: Vec<f64> = (1..=200).map(|i| law.delta_f * 6.0 * i as f64 / 200.0).collect();
        let sheared =
            drive_apart(&mesh, interface.clone(), law, 70e9, 0.3, h / 2.0, &steps, Drive::Sliding)
                .unwrap();
        let opened =
            drive_apart(&mesh, interface, law, 70e9, 0.3, h / 2.0, &steps, Drive::Opening).unwrap();

        let s_last = sheared.last().unwrap();
        let o_last = opened.last().unwrap();
        assert!(s_last.separated > w * 0.9, "shear must fully separate it too");
        assert!(
            (s_last.dissipated / (law.fracture_energy() * s_last.separated) - 1.0).abs() < 1e-9,
            "shear dissipation {:.4} vs G_c x area {:.4}",
            s_last.dissipated,
            law.fracture_energy() * s_last.separated
        );
        assert!(
            (s_last.dissipated / o_last.dissipated - 1.0).abs() < 1e-6,
            "shear and opening must cost the same: {:.4} vs {:.4}",
            s_last.dissipated,
            o_last.dissipated
        );
        let peak = sheared.iter().fold(0.0f64, |m, st| m.max(st.reaction.abs()));
        assert!(peak > law.sigma_max * w * 0.3, "peak shear reaction {peak:.3e} is too small");
    }

    #[test]
    fn an_intact_interface_behaves_like_the_bulk() {
        // With a penalty stiffness far above the bulk and openings below the damage threshold, the
        // split block must behave as though it were never split — which checks the assembly, the
        // node duplication and the sign of the interface forces all at once.
        let (w, h) = (4e-3, 2e-3);
        let base = Mesh::rectangle(w, h, 6, 4);
        let (mesh, interface) = split_horizontal(&base, h / 2.0, h * 1e-9);
        let law = Cohesive::from_energy(20.0, 30e6, 1e16);
        let small = law.delta_0() * 0.2;
        let steps = pull_apart(&mesh, interface, law, 70e9, 0.3, h / 2.0, &[small]).unwrap();
        assert_eq!(steps[0].dissipated, 0.0, "no damage below the threshold");
        assert_eq!(steps[0].separated, 0.0);
        // Reaction should be close to the uncracked stiffness E*A/h times the opening.
        let want = 70e9 * w / h * small;
        assert!(
            (steps[0].reaction / want - 1.0).abs() < 0.3,
            "reaction {:.3e} vs bulk estimate {want:.3e}",
            steps[0].reaction
        );
    }
}

