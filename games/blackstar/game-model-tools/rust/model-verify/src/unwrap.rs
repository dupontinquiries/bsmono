//! unwrap — give a hollow shell a UV set it never had.
//!
//! 547 models in this corpus carry an all-zero UV attribute: Brawl is one of
//! them, 8,938 vertices sharing a single (0,0). They cannot receive a texture
//! at all until they have coordinates, so this is the gate in front of the whole
//! "upscale everything to Brawl's level" path — and Brawl himself is behind it.
//!
//! ## Why projection rather than a real unwrap
//!
//! A proper unwrap cuts seams and flattens charts, and doing that well is a
//! research problem. But these parts are **hollow shells** — 38-39% of Brawl's
//! edges are open — and a shell has a dominant facing direction. Projecting from
//! that direction gives a usable, seam-free coordinate for the great majority of
//! a plate-like part, which is what armour panels mostly are.
//!
//! The projection axis is the part's own **shortest bounding extent**: a plate
//! is thin in one direction and that direction is its normal. Choosing the
//! longest instead smears the texture along the part, which is the classic
//! wrong answer here.
//!
//! ## Mirroring, which is the part specific to this corpus
//!
//! A mirrored twin is the same mesh reflected, so projecting it identically
//! makes the texture read **backwards** on one side of every vehicle. The U
//! coordinate is flipped for the right-hand member of each pair. That single
//! flag is why unwrapping has to be aware of the body plan rather than being a
//! per-mesh operation.

/// A per-part UV projection.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Projection {
    /// Axis projected ALONG (the part's thin direction).
    pub axis: usize,
    /// The two axes that become U and V.
    pub u_axis: usize,
    pub v_axis: usize,
    /// Bounds in those axes, for normalising into 0..1.
    pub u_min: f64,
    pub u_span: f64,
    pub v_min: f64,
    pub v_span: f64,
    /// Flip U — set for the mirrored twin so the texture does not read
    /// backwards on one side of the vehicle.
    pub flip_u: bool,
}

impl Projection {
    /// Build from a part's points. `flip_u` comes from the body plan, not from
    /// the geometry: only the anatomy pass knows which member of a pair is the
    /// reflected one.
    pub fn fit(pts: &[[f64; 3]], flip_u: bool) -> Projection {
        let mut lo = [f64::MAX; 3];
        let mut hi = [f64::MIN; 3];
        for p in pts { for i in 0..3 { lo[i] = lo[i].min(p[i]); hi[i] = hi[i].max(p[i]); } }
        let span = [hi[0]-lo[0], hi[1]-lo[1], hi[2]-lo[2]];
        // The thin axis is the plate's normal. Choosing the LONGEST instead
        // smears the texture along the part, which is the classic wrong answer.
        let axis = (0..3).min_by(|a, b| span[*a].partial_cmp(&span[*b]).unwrap()).unwrap();
        let (u_axis, v_axis) = match axis { 0 => (1, 2), 1 => (0, 2), _ => (0, 1) };
        Projection {
            axis, u_axis, v_axis,
            u_min: lo[u_axis], u_span: span[u_axis].max(1e-9),
            v_min: lo[v_axis], v_span: span[v_axis].max(1e-9),
            flip_u,
        }
    }

    /// Project one point into 0..1 UV.
    pub fn uv(&self, p: [f64; 3]) -> [f32; 2] {
        let mut u = (p[self.u_axis] - self.u_min) / self.u_span;
        let v = (p[self.v_axis] - self.v_min) / self.v_span;
        if self.flip_u { u = 1.0 - u; }
        [u.clamp(0.0, 1.0) as f32, v.clamp(0.0, 1.0) as f32]
    }

    /// How much of the part this projection actually suits, 0..1.
    ///
    /// A flat plate projects perfectly; a tube or a box wraps around the
    /// projection axis and part of it lands on top of itself. Reporting that
    /// is the difference between a UV set someone can trust and one that hides
    /// its own overlaps — a wrapped region gets a mirrored, doubled texture and
    /// looks like a bug in the art rather than in the unwrap.
    pub fn flatness(&self, pts: &[[f64; 3]]) -> f64 {
        if pts.is_empty() { return 0.0; }
        let mut lo = f64::MAX;
        let mut hi = f64::MIN;
        for p in pts { lo = lo.min(p[self.axis]); hi = hi.max(p[self.axis]); }
        let thickness = hi - lo;
        let breadth = self.u_span.max(self.v_span);
        // 1 for a plane, falling as the part gets as thick as it is wide
        (1.0 - thickness / breadth.max(1e-9)).clamp(0.0, 1.0)
    }
}

/// Below this, a part is too chunky for a single projection and wants splitting
/// or a box projection. Reported rather than silently accepted.
pub const FLAT_ENOUGH: f64 = 0.55;

/// Distinct colours for body-plan groups, so a rendered model shows what the
/// anatomy pass believed.
///
/// Mirrored twins share a HUE and differ in brightness: the pair should read as
/// one limb at a glance, while still being separable when checking that left
/// and right were matched correctly.
pub fn plan_colour(group: usize, right_hand: bool) -> [u8; 3] {
    let h = (group as f64 * 0.618_033_988_75) % 1.0;
    let (s, v) = (0.68, if right_hand { 0.72 } else { 1.0 });
    let i = (h * 6.0).floor();
    let f = h * 6.0 - i;
    let (p, q, t) = (v * (1.0 - s), v * (1.0 - s * f), v * (1.0 - s * (1.0 - f)));
    let (r, g, b) = match i as i32 % 6 {
        0 => (v, t, p), 1 => (q, v, p), 2 => (p, v, t),
        3 => (p, q, v), 4 => (t, p, v), _ => (v, p, q),
    };
    [(r * 255.0) as u8, (g * 255.0) as u8, (b * 255.0) as u8]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A flat plate in the XY plane, thin in Z.
    fn plate() -> Vec<[f64; 3]> {
        let mut v = Vec::new();
        for i in 0..5 { for j in 0..5 {
            v.push([i as f64 * 0.25, j as f64 * 0.5, (i % 2) as f64 * 0.01]);
        }}
        v
    }

    #[test]
    fn the_projection_axis_is_the_parts_thin_direction() {
        // A plate is thin in one direction and that direction is its normal.
        // Projecting along the LONGEST axis smears the texture along the part,
        // which is the classic wrong answer.
        let p = Projection::fit(&plate(), false);
        assert_eq!(p.axis, 2, "thin in Z, so project along Z");
        assert!(p.u_axis != 2 && p.v_axis != 2);
    }

    #[test]
    fn uvs_fill_the_zero_to_one_range() {
        // A texture applied to a part using a fraction of the range wastes
        // resolution; one that exceeds it tiles unexpectedly.
        let pts = plate();
        let p = Projection::fit(&pts, false);
        let uvs: Vec<[f32; 2]> = pts.iter().map(|q| p.uv(*q)).collect();
        let (mut lu, mut hu, mut lv, mut hv) = (1.0f32, 0.0f32, 1.0f32, 0.0f32);
        for [u, v] in &uvs {
            lu = lu.min(*u); hu = hu.max(*u); lv = lv.min(*v); hv = hv.max(*v);
        }
        assert!(lu < 0.01 && hu > 0.99, "u spans the range: {lu}..{hu}");
        assert!(lv < 0.01 && hv > 0.99, "v spans the range: {lv}..{hv}");
    }

    #[test]
    fn a_mirrored_twin_gets_its_u_flipped() {
        // Projecting a reflected mesh identically makes the texture read
        // BACKWARDS on one side of every vehicle — the reason unwrapping here
        // has to know the body plan rather than being a per-mesh operation.
        let pts = plate();
        let l = Projection::fit(&pts, false);
        let r = Projection::fit(&pts, true);
        for q in &pts {
            let a = l.uv(*q);
            let b = r.uv(*q);
            assert!((a[0] + b[0] - 1.0).abs() < 1e-6, "u is mirrored: {} vs {}", a[0], b[0]);
            assert!((a[1] - b[1]).abs() < 1e-6, "and v is not");
        }
    }

    #[test]
    fn flatness_reports_when_a_part_is_too_chunky_for_one_projection() {
        // A wrapped region gets a mirrored, doubled texture and looks like a bug
        // in the art rather than in the unwrap. Better to say so.
        let flat = plate();
        let p = Projection::fit(&flat, false);
        assert!(p.flatness(&flat) > 0.9, "a plate is flat: {}", p.flatness(&flat));
        assert!(p.flatness(&flat) > FLAT_ENOUGH);

        // a cube is as thick as it is wide
        let cube: Vec<[f64; 3]> = (0..8).map(|i|
            [(i & 1) as f64, ((i >> 1) & 1) as f64, ((i >> 2) & 1) as f64]).collect();
        let c = Projection::fit(&cube, false);
        assert!(c.flatness(&cube) < 0.1, "a cube is not flat: {}", c.flatness(&cube));
        assert!(c.flatness(&cube) < FLAT_ENOUGH, "and it is reported as unsuitable");
    }

    #[test]
    fn a_degenerate_part_does_not_produce_nan() {
        // All points identical: a real thing in this corpus, where some
        // components are a handful of coincident verts.
        let same = vec![[1.0, 1.0, 1.0]; 6];
        let p = Projection::fit(&same, false);
        let uv = p.uv([1.0, 1.0, 1.0]);
        assert!(uv[0].is_finite() && uv[1].is_finite(), "{uv:?}");
        assert!(p.flatness(&same).is_finite());
        // and an empty part is answered rather than panicking
        let e = Projection::fit(&[], false);
        assert!(e.uv([0.0; 3])[0].is_finite());
    }

    #[test]
    fn mirrored_twins_share_a_hue_so_a_pair_reads_as_one_limb() {
        // The colour coding exists to CHECK the anatomy pass by eye: a pair
        // should look like one limb, while still being separable.
        let l = plan_colour(3, false);
        let r = plan_colour(3, true);
        assert_ne!(l, r, "distinguishable");
        // same hue means the channel ORDER is the same, only the level differs
        let ord = |c: [u8; 3]| {
            let mut i = [0, 1, 2];
            i.sort_by_key(|k| std::cmp::Reverse(c[*k]));
            i
        };
        assert_eq!(ord(l), ord(r), "same hue: {l:?} vs {r:?}");
        // and different groups are actually different
        assert_ne!(plan_colour(0, false), plan_colour(1, false));
        assert_ne!(plan_colour(1, false), plan_colour(2, false));
    }
}
