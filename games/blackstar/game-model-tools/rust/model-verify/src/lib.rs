//! model-verify — did the model survive the conversion?
//!
//! 2,133 models cannot be checked by eye, and "looks right" is not a claim you
//! can act on when the next step is levelling the whole corpus to one standard.
//! This checks it arithmetically.
//!
//! ## Why the check can be exact rather than approximate
//!
//! Instancing stores a component once and places it with a **rigid transform**
//! — rotation plus translation, no scale, no shear. A rigid transform preserves
//! every distance in the mesh, so the reconstruction is not "close to" the
//! original, it **is** the original up to float rounding. That makes the
//! tolerance meaningful: anything above a few ULPs is a bug, not a tolerance to
//! be widened until the test passes.
//!
//! That is the whole reason this technique is safe on authored geometry where
//! the procedural part-kit approach is not: nothing is being re-expressed, only
//! de-duplicated.
//!
//! ## What is actually checked
//!
//! * **The transform is recovered, not trusted.** [`kabsch`] solves for the
//!   rotation that best maps one component onto another; if the components are
//!   genuinely the same shape, the residual is at float precision. A converter
//!   that wrote a subtly wrong matrix is caught here rather than in a renderer.
//! * **Winding.** A *mirrored* transform (`det(R) < 0`) maps the vertices
//!   perfectly and silently flips every normal, so the model renders inside-out
//!   with correct geometry. Vertex matching alone will not catch it; the
//!   determinant will.
//! * **Topology.** Component count and per-component vertex counts must survive,
//!   because a reconstruction can match every vertex position while having
//!   welded two components into one.

/// A rigid transform: rotation (row-major 3×3) then translation.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rigid {
    pub r: [[f64; 3]; 3],
    pub t: [f64; 3],
}

impl Rigid {
    pub const IDENTITY: Rigid = Rigid {
        r: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
        t: [0.0; 3],
    };

    pub fn apply(&self, p: [f64; 3]) -> [f64; 3] {
        let mut o = [0.0; 3];
        for i in 0..3 {
            o[i] = self.r[i][0] * p[0] + self.r[i][1] * p[1] + self.r[i][2] * p[2] + self.t[i];
        }
        o
    }

    /// `det(R)`. Must be **+1** for a true rotation. A value near −1 means the
    /// transform includes a reflection: the vertices will land perfectly and
    /// every face will be wound backwards.
    pub fn determinant(&self) -> f64 {
        let m = &self.r;
        m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1])
            - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0])
            + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0])
    }

    pub fn preserves_winding(&self) -> bool { self.determinant() > 0.0 }
}

fn centroid(v: &[[f64; 3]]) -> [f64; 3] {
    let n = v.len().max(1) as f64;
    let mut c = [0.0; 3];
    for p in v { for i in 0..3 { c[i] += p[i]; } }
    for i in 0..3 { c[i] /= n; }
    c
}

/// Recover the rigid transform mapping `from` onto `to`, by Kabsch.
///
/// Both slices must be the same length and in **corresponding order** — this
/// solves for the transform, it does not solve correspondence. An instancing
/// pass knows the correspondence because it produced it; verifying a mesh whose
/// vertex order was shuffled is a different and much harder problem, and this
/// deliberately does not pretend to do it.
///
/// The rotation is extracted by polar decomposition of the covariance matrix,
/// iterated — enough for verification, and it avoids a full SVD dependency in a
/// tool whose job is to be trivially auditable.
pub fn kabsch(from: &[[f64; 3]], to: &[[f64; 3]]) -> Option<Rigid> {
    if from.len() != to.len() || from.is_empty() { return None; }
    let (cf, ct) = (centroid(from), centroid(to));
    // covariance H = sum (from - cf)^T (to - ct)
    let mut h = [[0.0f64; 3]; 3];
    for (a, b) in from.iter().zip(to) {
        for i in 0..3 {
            for j in 0..3 {
                h[i][j] += (a[i] - cf[i]) * (b[j] - ct[j]);
            }
        }
    }
    // polar decomposition: iterate R <- (R + (R^-T)) / 2, converging to the
    // orthogonal factor of H^T
    let mut r = transpose(h);
    for _ in 0..48 {
        let inv_t = match invert(r) { Some(m) => transpose(m), None => return None };
        let mut next = [[0.0f64; 3]; 3];
        let mut delta = 0.0;
        for i in 0..3 {
            for j in 0..3 {
                next[i][j] = 0.5 * (r[i][j] + inv_t[i][j]);
                delta += (next[i][j] - r[i][j]).abs();
            }
        }
        r = next;
        if delta < 1e-15 { break; }
    }
    let mut t = [0.0; 3];
    for i in 0..3 {
        t[i] = ct[i] - (r[i][0] * cf[0] + r[i][1] * cf[1] + r[i][2] * cf[2]);
    }
    Some(Rigid { r, t })
}

fn transpose(m: [[f64; 3]; 3]) -> [[f64; 3]; 3] {
    let mut o = [[0.0; 3]; 3];
    for i in 0..3 { for j in 0..3 { o[i][j] = m[j][i]; } }
    o
}

fn invert(m: [[f64; 3]; 3]) -> Option<[[f64; 3]; 3]> {
    let d = m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1])
        - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0])
        + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0]);
    if d.abs() < 1e-18 { return None; }
    let mut o = [[0.0; 3]; 3];
    o[0][0] = (m[1][1] * m[2][2] - m[1][2] * m[2][1]) / d;
    o[0][1] = (m[0][2] * m[2][1] - m[0][1] * m[2][2]) / d;
    o[0][2] = (m[0][1] * m[1][2] - m[0][2] * m[1][1]) / d;
    o[1][0] = (m[1][2] * m[2][0] - m[1][0] * m[2][2]) / d;
    o[1][1] = (m[0][0] * m[2][2] - m[0][2] * m[2][0]) / d;
    o[1][2] = (m[0][2] * m[1][0] - m[0][0] * m[1][2]) / d;
    o[2][0] = (m[1][0] * m[2][1] - m[1][1] * m[2][0]) / d;
    o[2][1] = (m[0][1] * m[2][0] - m[0][0] * m[2][1]) / d;
    o[2][2] = (m[0][0] * m[1][1] - m[0][1] * m[1][0]) / d;
    Some(o)
}

/// Per-instance verdict.
#[derive(Debug, Clone, PartialEq)]
pub struct Verdict {
    /// Worst per-vertex distance between reconstruction and original.
    pub max_error: f64,
    /// `det(R)` — must be ~+1.
    pub determinant: f64,
    pub verts: usize,
    pub winding_ok: bool,
    pub within_tolerance: bool,
}

impl Verdict {
    pub fn passed(&self) -> bool { self.within_tolerance && self.winding_ok }
}

/// A rigid transform is lossless, so the only error possible is float rounding.
/// Widening this past ~1e-5 on unit-scale geometry means accepting a real
/// difference and calling it a rounding error.
pub const EPSILON: f64 = 1e-5;

/// Verify that `unique` placed by `xf` reproduces `original`.
pub fn verify_instance(original: &[[f64; 3]], unique: &[[f64; 3]], xf: &Rigid) -> Verdict {
    let mut max_error: f64 = 0.0;
    for (o, u) in original.iter().zip(unique) {
        let p = xf.apply(*u);
        let d = ((p[0] - o[0]).powi(2) + (p[1] - o[1]).powi(2) + (p[2] - o[2]).powi(2)).sqrt();
        max_error = max_error.max(d);
    }
    let det = xf.determinant();
    Verdict {
        max_error,
        determinant: det,
        verts: original.len().min(unique.len()),
        winding_ok: det > 0.0,
        within_tolerance: original.len() == unique.len() && max_error <= EPSILON,
    }
}

/// Recover the transform AND verify it in one step — the normal entry point,
/// because a transform the tool solved for itself cannot be a transform the
/// converter got wrong.
pub fn verify_pair(original: &[[f64; 3]], unique: &[[f64; 3]]) -> Option<Verdict> {
    let xf = kabsch(unique, original)?;
    Some(verify_instance(original, unique, &xf))
}

/// Topology check: a reconstruction can match every vertex position while
/// having welded two components into one, which vertex matching cannot see.
pub fn topology_ok(before: &[usize], after: &[usize]) -> bool {
    if before.len() != after.len() { return false; }
    let (mut a, mut b) = (before.to_vec(), after.to_vec());
    a.sort_unstable();
    b.sort_unstable();
    a == b
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An asymmetric blob — symmetric test shapes let a wrong rotation pass.
    fn shape() -> Vec<[f64; 3]> {
        vec![[0.0,0.0,0.0],[1.0,0.0,0.0],[0.0,2.0,0.0],[0.0,0.0,3.0],
             [1.0,2.0,0.5],[0.25,0.5,1.5],[-0.5,1.0,0.75],[2.0,-1.0,0.25]]
    }

    fn rot_z(a: f64) -> Rigid {
        Rigid { r: [[a.cos(), -a.sin(), 0.0], [a.sin(), a.cos(), 0.0], [0.0, 0.0, 1.0]], t: [0.0; 3] }
    }

    fn place(v: &[[f64;3]], xf: &Rigid) -> Vec<[f64;3]> { v.iter().map(|p| xf.apply(*p)).collect() }

    #[test]
    fn a_rotated_instance_reconstructs_exactly_not_approximately() {
        // The claim the whole technique rests on: a rigid transform is lossless,
        // so the residual is float rounding and nothing else.
        let unique = shape();
        for angle in [0.1, 0.7, 1.5708, 3.0, 5.9] {
            let mut xf = rot_z(angle);
            xf.t = [3.5, -2.0, 0.75];
            let original = place(&unique, &xf);
            let v = verify_pair(&original, &unique).expect("solvable");
            assert!(v.passed(), "angle {angle}: {v:?}");
            assert!(v.max_error < 1e-9, "residual should be float noise, got {}", v.max_error);
            assert!((v.determinant - 1.0).abs() < 1e-9, "a rotation has det +1");
        }
    }

    #[test]
    fn a_mirrored_transform_is_caught_even_though_every_vertex_lands_perfectly() {
        // THE bug vertex-matching alone cannot see: the geometry is right and
        // every face is wound backwards, so the model renders inside-out.
        let unique = shape();
        let mirror = Rigid { r: [[-1.0,0.0,0.0],[0.0,1.0,0.0],[0.0,0.0,1.0]], t: [0.0;3] };
        let original = place(&unique, &mirror);
        let v = verify_instance(&original, &unique, &mirror);
        assert!(v.within_tolerance, "the vertices DO match — that is the trap");
        assert!(!v.winding_ok, "but the winding is flipped");
        assert!(!v.passed(), "so the instance must fail overall");
        assert!(v.determinant < 0.0, "det {} should be negative", v.determinant);
    }

    #[test]
    fn a_wrong_transform_is_rejected_rather_than_tolerated() {
        let unique = shape();
        let truth = { let mut x = rot_z(0.9); x.t = [1.0, 2.0, 3.0]; x };
        let original = place(&unique, &truth);
        // a converter that wrote a slightly wrong angle
        let wrong = { let mut x = rot_z(0.9 + 0.01); x.t = [1.0, 2.0, 3.0]; x };
        let v = verify_instance(&original, &unique, &wrong);
        assert!(!v.passed(), "a 0.01 rad error must fail: max_error {}", v.max_error);
        assert!(v.max_error > EPSILON);
        // and a wrong translation likewise
        let shifted = { let mut x = truth; x.t[1] += 0.001; x };
        assert!(!verify_instance(&original, &unique, &shifted).passed());
    }

    #[test]
    fn identity_is_the_trivial_case_and_still_has_to_pass() {
        let unique = shape();
        let v = verify_pair(&unique, &unique).expect("solvable");
        assert!(v.passed());
        assert!(v.max_error < 1e-12);
        assert_eq!(v.verts, 8);
    }

    #[test]
    fn a_mismatched_vertex_count_fails_instead_of_comparing_a_prefix() {
        // Zip would silently compare the first N and report success on a
        // reconstruction that lost half the mesh.
        let unique = shape();
        let short = &unique[..5];
        let v = verify_instance(&unique, short, &Rigid::IDENTITY);
        assert!(!v.within_tolerance, "different lengths cannot pass");
        assert!(!v.passed());
        assert!(kabsch(short, &unique).is_none(), "and solving is refused outright");
        assert!(kabsch(&[], &[]).is_none());
    }

    #[test]
    fn topology_must_survive_even_when_every_vertex_matches() {
        // A reconstruction can place every vertex correctly while having welded
        // two components into one — invisible to position checks.
        assert!(topology_ok(&[17, 17, 71, 96], &[96, 17, 71, 17]), "order does not matter");
        assert!(!topology_ok(&[17, 17, 71], &[34, 71]), "two components welded into one");
        assert!(!topology_ok(&[17, 71], &[17, 71, 17]), "an extra component appeared");
        assert!(topology_ok(&[], &[]));
    }

    #[test]
    fn the_tolerance_is_a_bug_threshold_not_a_fudge_factor() {
        // Documenting intent in a test: at unit scale, 1e-5 is already orders of
        // magnitude above float rounding for a rigid transform. Anything failing
        // by more than this is a real difference.
        assert!(EPSILON >= 1e-6 && EPSILON <= 1e-4);
        let unique = shape();
        let xf = rot_z(1.0);
        let original = place(&unique, &xf);
        let v = verify_instance(&original, &unique, &xf);
        assert!(v.max_error * 1000.0 < EPSILON, "a correct transform is far inside it");
    }
}
