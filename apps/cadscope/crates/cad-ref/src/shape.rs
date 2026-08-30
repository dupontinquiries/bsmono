//! shape.rs — "arms like `velgorn`", reduced to something a turtle can be steered toward.
//!
//! The verb this serves is the one §17.2 kept for last and defined by what it must *not* do: it
//! does not paste the reference mesh. That is `import`, a separate and honest verb. `like` extracts
//! a **proportion summary** — where the form's long axis runs, how thick it is along that axis, how
//! symmetric it is — so what comes out is *your* shape at the reference's proportions rather than
//! somebody else's geometry wearing your material.
//!
//! The summary is deliberately the same shape as a spine: an axis, and a radius at each station
//! along it. That is not a coincidence, it is the point — `to_spine` hands the result straight to
//! the Grow model in `cad_kernel::spine`, so a reference becomes something you can steer.
//!
//! What this is not: a medial axis. A real skeletonisation would follow a branching form into its
//! branches; this projects everything onto one dominant axis, so an arm reads well and a hand with
//! five fingers reads as one thick stub. PARITY says so plainly.

use std::path::Path;

use cad_kernel::sdf::Section;
use cad_kernel::spine::{Node, Spine};
use glam::{Mat3, Vec3};

/// A reference form's proportions, in the file's own units.
#[derive(Clone, Debug, PartialEq)]
pub struct Proportions {
    pub name: String,
    /// Centre of mass of the vertices (not of the volume — vertices are what we have).
    pub centroid: Vec3,
    /// Dominant axis, by power iteration on the vertex covariance.
    pub axis: Vec3,
    /// A perpendicular, so the profile has a frame rather than an arbitrary rotation.
    pub side: Vec3,
    /// Extent along `axis`.
    pub length: f32,
    /// Silhouette radius at each station along the axis, evenly spaced from one end to the other.
    pub radii: Vec<f32>,
    /// Best bilateral agreement across either principal plane: 1.0 is perfectly balanced. This
    /// answers "does the form have a symmetry plane at all", which is the question "arms like X"
    /// cares about.
    pub symmetry: f32,
    /// Agreement across the *other* principal plane. A form symmetric about one plane and lopsided
    /// about the other — most limbs, most brackets — shows a high `symmetry` and a low
    /// `symmetry_worst`, and reporting only the first would hide the lopsidedness entirely.
    pub symmetry_worst: f32,
    pub vertices: u32,
}

impl Proportions {
    /// The widest station, and where along the length it sits (0 at one end, 1 at the other).
    pub fn waist(&self) -> (f32, f32) {
        let mut best = (0.0f32, 0.0f32);
        for (i, r) in self.radii.iter().enumerate() {
            if *r > best.0 {
                best = (*r, i as f32 / (self.radii.len().max(2) - 1) as f32);
            }
        }
        best
    }

    /// Ratio of length to greatest thickness — the single number that says "long and thin" or
    /// "squat", and the one most worth carrying across into a different shape.
    pub fn slenderness(&self) -> f32 {
        let max_r = self.radii.iter().cloned().fold(0.0f32, f32::max);
        if max_r <= 1e-6 { 0.0 } else { self.length / (max_r * 2.0) }
    }

    /// Hand the proportions to the Grow model: stations along the axis carrying the sampled radii.
    /// `scale` rescales the whole profile, which is how "like velgorn, but wrist-sized" works.
    pub fn to_spine(&self, blend: f32, scale: f32) -> Spine {
        let n = self.radii.len().max(2);
        let start = self.centroid - self.axis * (self.length * 0.5);
        let step = self.length / (n - 1) as f32;
        let nodes = self
            .radii
            .iter()
            .enumerate()
            .map(|(i, r)| Node {
                pos: (start + self.axis * (step * i as f32)) * scale,
                up: self.side,
                radius: (r * scale).max(1e-4),
                section: Section::Circle,
            })
            .collect();
        Spine { nodes, blend }
    }
}

/// Dominant eigenvector of a symmetric matrix by power iteration. Enough for an axis; this is not
/// a general eigensolver and does not pretend to be.
fn dominant(m: Mat3, seed: Vec3) -> Vec3 {
    let mut v = seed.normalize_or(Vec3::X);
    for _ in 0..48 {
        let next = m * v;
        if next.length_squared() < 1e-20 {
            return v;
        }
        v = next.normalize();
    }
    v
}

/// Extract proportions from a set of points.
pub fn from_points(name: &str, pts: &[Vec3], stations: usize) -> Option<Proportions> {
    if pts.len() < 4 {
        return None;
    }
    let n = pts.len() as f32;
    let centroid = pts.iter().copied().fold(Vec3::ZERO, |a, b| a + b) / n;

    // Covariance of the centred points.
    let mut c = Mat3::ZERO;
    for p in pts {
        let d = *p - centroid;
        c += Mat3::from_cols(d * d.x, d * d.y, d * d.z);
    }
    c *= 1.0 / n;

    let axis = dominant(c, Vec3::X);
    // Deflate and take the second axis, so `side` is the genuine secondary spread rather than an
    // arbitrary perpendicular — it is the frame a profile is measured in.
    let lambda = axis.dot(c * axis);
    let deflated = c - Mat3::from_cols(axis * axis.x, axis * axis.y, axis * axis.z) * lambda;
    let mut side = dominant(deflated, Vec3::Y);
    side = (side - axis * side.dot(axis)).normalize_or(axis.any_orthonormal_vector());
    let third = axis.cross(side).normalize_or(Vec3::Z);

    // Project onto the axis and bin.
    let ts: Vec<f32> = pts.iter().map(|p| (*p - centroid).dot(axis)).collect();
    let (mut lo, mut hi) = (f32::INFINITY, f32::NEG_INFINITY);
    for t in &ts {
        lo = lo.min(*t);
        hi = hi.max(*t);
    }
    let length = hi - lo;
    if !length.is_finite() || length <= 1e-9 {
        return None;
    }

    let stations = stations.clamp(2, 512);
    // Silhouette radius per station: the farthest point from the axis in that slice, since a
    // profile is what the outline does, not what the average vertex does.
    let mut radii = vec![0.0f32; stations];
    // Two split axes, because a form can be symmetric about one principal plane and lopsided about
    // the other, and measuring only one would call such a form symmetric.
    let mut sides = [[vec![0.0f32; stations], vec![0.0f32; stations]], [vec![0.0f32; stations], vec![0.0f32; stations]]];
    for (p, t) in pts.iter().zip(ts.iter()) {
        let f = ((t - lo) / length * (stations - 1) as f32).round();
        let bin = (f as usize).min(stations - 1);
        let d = *p - centroid - axis * *t;
        let r = d.length();
        radii[bin] = radii[bin].max(r);
        for (k, n) in [third, side].into_iter().enumerate() {
            let half = if d.dot(n) >= 0.0 { 0 } else { 1 };
            sides[k][half][bin] = sides[k][half][bin].max(r);
        }
    }
    // Carry a radius across empty stations rather than reporting a pinch that is only a sampling
    // gap — a coarse mesh has slices with no vertices in them.
    let mut last = 0.0f32;
    for r in radii.iter_mut() {
        if *r <= 0.0 {
            *r = last;
        } else {
            last = *r;
        }
    }

    let score = |halves: &[Vec<f32>; 2]| {
        let mut sum = 0.0;
        let mut n = 0.0;
        for i in 0..stations {
            let (a, b) = (halves[0][i], halves[1][i]);
            let m = a.max(b);
            if m > 1e-6 {
                sum += 1.0 - (a - b).abs() / m;
                n += 1.0;
            }
        }
        if n > 0.0 { sum / n } else { 0.0 }
    };
    let s0 = score(&sides[0]);
    let s1 = score(&sides[1]);

    Some(Proportions {
        name: name.to_string(),
        centroid,
        axis,
        side,
        length,
        radii,
        symmetry: s0.max(s1),
        symmetry_worst: s0.min(s1),
        vertices: pts.len() as u32,
    })
}

/// Read an OBJ's vertices and extract proportions. Only OBJ, because it is the only format in the
/// corpus we read geometry from — an IQM is indexed from its header alone and has no vertices to
/// measure, which the caller must report rather than paper over.
pub fn from_obj(path: &Path, name: &str, stations: usize) -> Option<Proportions> {
    let is_obj = path.extension().and_then(|e| e.to_str()).map(|e| e.eq_ignore_ascii_case("obj"));
    if is_obj != Some(true) {
        return None;
    }
    let text = std::fs::read_to_string(path).ok()?;
    let pts: Vec<Vec3> = text
        .lines()
        .filter_map(|l| {
            let rest = l.trim().strip_prefix("v ")?;
            let c: Vec<f32> = rest.split_whitespace().take(3).filter_map(|t| t.parse().ok()).collect();
            (c.len() == 3).then(|| Vec3::new(c[0], c[1], c[2]))
        })
        .collect();
    from_points(name, &pts, stations)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Points on a cone-ish tube along +X, radius tapering 2.0 -> 0.5 over length 10.
    fn tapered(n: usize) -> Vec<Vec3> {
        let mut pts = Vec::new();
        for i in 0..n {
            let t = i as f32 / (n - 1) as f32;
            let r = 2.0 + (0.5 - 2.0) * t;
            for k in 0..16 {
                let a = k as f32 / 16.0 * std::f32::consts::TAU;
                pts.push(Vec3::new(t * 10.0 - 5.0, r * a.cos(), r * a.sin()));
            }
        }
        pts
    }

    #[test]
    fn finds_the_long_axis_and_the_taper() {
        let p = from_points("tube", &tapered(24), 8).expect("proportions");
        assert!(p.axis.x.abs() > 0.99, "dominant axis should be X, got {:?}", p.axis);
        assert!((p.length - 10.0).abs() < 0.2, "length {}", p.length);
        // Radii must fall from about 2 to about 0.5, in whichever direction the axis points.
        let first = p.radii.first().copied().unwrap();
        let last = p.radii.last().copied().unwrap();
        let (thick, thin) = if first > last { (first, last) } else { (last, first) };
        assert!((thick - 2.0).abs() < 0.15, "thick end {thick}");
        assert!((thin - 0.5).abs() < 0.15, "thin end {thin}");
        // A body of revolution is bilaterally symmetric.
        assert!(p.symmetry > 0.95, "symmetry {}", p.symmetry);
        assert!(p.symmetry_worst > 0.95, "a body of revolution is symmetric about both planes");
        // Long and thin: 10 long against 4 across at the widest.
        assert!((p.slenderness() - 2.5).abs() < 0.2, "slenderness {}", p.slenderness());
    }

    #[test]
    fn a_lopsided_form_scores_lower_symmetry() {
        let mut pts = tapered(24);
        // Push one side out: now the silhouette is one-sided.
        for p in pts.iter_mut() {
            if p.z > 0.0 {
                p.z *= 3.0;
            }
        }
        let p = from_points("lopsided", &pts, 8).unwrap();
        // It still has one symmetry plane — stretching one side leaves the other axis balanced,
        // and the principal frame simply rotates to suit. What must register is the *other* plane.
        assert!(
            p.symmetry_worst < 0.85,
            "worst-plane symmetry {} should register the imbalance (best {})",
            p.symmetry_worst,
            p.symmetry
        );
    }

    #[test]
    fn proportions_become_a_steerable_spine() {
        let p = from_points("tube", &tapered(24), 10).unwrap();
        let spine = p.to_spine(0.1, 1.0);
        assert_eq!(spine.nodes.len(), 10);
        // The spine must span the reference's length and carry its taper.
        assert!((spine.length() - p.length).abs() < 0.3, "spine length {}", spine.length());
        let sdf = spine.to_sdf().expect("a spine of ten stations is a solid");
        // Its centre is solid, and a point well outside the widest radius is not.
        assert!(sdf.eval(p.centroid) < 0.0);
        assert!(sdf.eval(p.centroid + p.side * 8.0) > 0.0);
    }

    #[test]
    fn scaling_the_profile_scales_the_result() {
        let p = from_points("tube", &tapered(24), 6).unwrap();
        let small = p.to_spine(0.0, 0.25);
        let full = p.to_spine(0.0, 1.0);
        let ratio = small.nodes[0].radius / full.nodes[0].radius;
        assert!((ratio - 0.25).abs() < 1e-4, "ratio {ratio}");
        assert!((small.length() / full.length() - 0.25).abs() < 1e-3);
    }

    #[test]
    fn degenerate_input_is_declined_not_guessed() {
        assert!(from_points("empty", &[], 8).is_none());
        // All points coincident: no axis exists.
        assert!(from_points("dot", &[Vec3::ONE; 12], 8).is_none());
    }
}
