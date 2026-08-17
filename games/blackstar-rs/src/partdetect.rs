//! PART DETECTION — what a prim physically IS, from its geometry alone.
//!
//! `orient.rs` answers "which way is up" for a WHOLE body. This answers
//! a different question, per PRIM: is this thing shaped like a leg, a
//! wheel, a thruster, a wing? The two are meant to compose — a part
//! guess is only meaningful in a body whose up-axis is already known,
//! since "near the ground" and "sticking out sideways" are both
//! relative to that axis.
//!
//! ## What is measured fact, and what is interpretation
//!
//! `elongation`, `taper` and `ground_fraction` are plain geometric
//! measurements — bounding-box ratios and cross-section comparisons.
//! They are tested against synthetic shapes with a known right answer
//! (a cone, a disc, a rod) and there is nothing to disagree with once
//! the numbers are checked.
//!
//! `classify` is the part this module is NOT overclaiming: it combines
//! those measurements into a labelled guess (Leg, Wheel, Thruster, ...)
//! using thresholds picked by reasoning about the shapes, not measured
//! against a labelled dataset of real bot parts — this codebase has no
//! such dataset, and I have no renderer here to eyeball results against
//! real chassis and correct the thresholds by looking. Treat `classify`
//! as a starting point to tune against real models, not a finished
//! classifier — the same honesty `orient.rs`'s own doc comment insists
//! on for its up/forward guess ("nothing here claims perfection... a
//! real, checkable guess").
//!
//! ## What this deliberately does not claim
//!
//! Load-bearing analysis and aerodynamic analysis (intake vs. outtake
//! for a thruster, specifically) are NOT attempted here. A nozzle's
//! taper direction is measurable (`taper` does exactly that), but
//! mapping "wider at the outward end" to "this is an exhaust, thrust
//! points this way" is a real aerodynamics claim I cannot verify
//! against anything — there is no reference set of real intake/outtake
//! geometry in this codebase to check the mapping against, and getting
//! it backwards would be worse than not claiming it: a confidently
//! wrong thrust direction is harder to notice and fix than an absent
//! one. `taper` is exposed as the measurement; the directional claim on
//! top of it is left undone rather than guessed.

use glam::Vec3;

use crate::meshgen::Vertex;
use crate::orient::{perp_pair, AXES};

/// Bounding half-extents of a prim along an arbitrary axis and its
/// perpendicular pair, reused by every measurement below so they all
/// agree on the same box rather than each computing their own.
fn extents(verts: &[Vertex], axis: Vec3) -> (f32, f32, f32, f32, f32, f32) {
    let (pa, pb) = perp_pair(axis);
    let (mut lo, mut hi) = (f32::MAX, f32::MIN);
    let (mut a_lo, mut a_hi) = (f32::MAX, f32::MIN);
    let (mut b_lo, mut b_hi) = (f32::MAX, f32::MIN);
    for v in verts {
        let p = Vec3::from_array(v.pos);
        let h = p.dot(axis);
        lo = lo.min(h);
        hi = hi.max(h);
        let x = p.dot(pa);
        a_lo = a_lo.min(x);
        a_hi = a_hi.max(x);
        let y = p.dot(pb);
        b_lo = b_lo.min(y);
        b_hi = b_hi.max(y);
    }
    (lo, hi, a_lo, a_hi, b_lo, b_hi)
}

/// How limb-like a prim's SHAPE is: the ratio of its longest axis to
/// its cross-section. High (>2-3) reads as a strut, leg, barrel or
/// nozzle; low (near 1) reads as a blob — a torso, a cockpit pod, a
/// wheel hub viewed from the axle. Direction-agnostic on purpose: a
/// leg lying sideways and a leg standing up are the same shape, so
/// this checks every candidate long axis and keeps the best one.
pub fn elongation(verts: &[Vertex]) -> f32 {
    if verts.is_empty() {
        return 0.0;
    }
    let mut best = 0.0f32;
    for axis in AXES {
        let (lo, hi, a_lo, a_hi, b_lo, b_hi) = extents(verts, axis);
        let long = (hi - lo).max(0.0);
        let cross = ((a_hi - a_lo).max(b_hi - b_lo)).max(1e-6);
        best = best.max(long / cross);
    }
    best
}

/// Which end of a prim is WIDER, along `axis`. Positive means the
/// positive-axis end has the larger cross-section (a cone flaring
/// toward +axis); negative means the reverse; near zero means the
/// prim does not taper along this axis at all (a cylinder, a box).
///
/// Compares the outer quarter of the prim's own extent at each end,
/// not just the single widest vertex at each tip — a real taper is a
/// property of a region, and a single stray vertex (a rivet, a sensor
/// nub) at one tip should not flip the sign of an otherwise-uniform
/// barrel.
pub fn taper(verts: &[Vertex], axis: Vec3) -> f32 {
    if verts.len() < 2 {
        return 0.0;
    }
    let (pa, pb) = perp_pair(axis);
    let (lo, hi, ..) = extents(verts, axis);
    let span = (hi - lo).max(1e-6);
    let band = span * 0.25;
    let cross_at = |want_low: bool| -> f32 {
        let (mut a_lo, mut a_hi) = (f32::MAX, f32::MIN);
        let (mut b_lo, mut b_hi) = (f32::MAX, f32::MIN);
        let mut any = false;
        for v in verts {
            let p = Vec3::from_array(v.pos);
            let h = p.dot(axis);
            let inside = if want_low { h <= lo + band } else { h >= hi - band };
            if !inside {
                continue;
            }
            any = true;
            let x = p.dot(pa);
            a_lo = a_lo.min(x);
            a_hi = a_hi.max(x);
            let y = p.dot(pb);
            b_lo = b_lo.min(y);
            b_hi = b_hi.max(y);
        }
        if !any {
            return 0.0;
        }
        ((a_hi - a_lo).max(0.0)).max((b_hi - b_lo).max(0.0))
    };
    let lo_cross = cross_at(true);
    let hi_cross = cross_at(false);
    let denom = (lo_cross + hi_cross).max(1e-6);
    (hi_cross - lo_cross) / denom
}

/// Where a prim sits within the WHOLE BODY's height range along `up`,
/// 0 at the body's own lowest point and 1 at its highest — relative to
/// the body, not the prim's own extent, because "near the ground" is
/// a claim about the whole figure. `body_lo`/`body_hi` are the whole
/// body's extent along `up`, from the caller (typically the same pass
/// that already computed `orient::up_score`'s winning axis).
pub fn ground_fraction(verts: &[Vertex], up: Vec3, body_lo: f32, body_hi: f32) -> f32 {
    if verts.is_empty() {
        return 0.5;
    }
    let span = (body_hi - body_lo).max(1e-6);
    let mut sum = 0.0;
    for v in verts {
        let h = Vec3::from_array(v.pos).dot(up);
        sum += ((h - body_lo) / span).clamp(0.0, 1.0);
    }
    sum / verts.len() as f32
}

/// How far a prim's centre sits from the body's vertical centreline
/// (the `up` axis through `body_center`), as a fraction of the body's
/// own widest horizontal extent. High values are candidates for
/// laterally-mounted parts — wings, side thrusters, shoulder-mounted
/// weapons; low values sit near the spine — torso, cockpit, a
/// centreline tail thruster.
pub fn lateral_offset(verts: &[Vertex], up: Vec3, body_center: Vec3, body_half_width: f32) -> f32 {
    if verts.is_empty() || body_half_width <= 0.0 {
        return 0.0;
    }
    let mut c = Vec3::ZERO;
    for v in verts {
        c += Vec3::from_array(v.pos);
    }
    c /= verts.len() as f32;
    let rel = c - body_center;
    let horizontal = rel - rel.dot(up) * up;
    (horizontal.length() / body_half_width).min(2.0)
}

/// A labelled guess at what a prim is — see the module doc for how
/// much to trust this versus the plain measurements it is built from.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    /// long, near the bottom of the body, close to the centreline —
    /// a leg, a tread run, a landing strut
    Leg,
    /// short and wide relative to its own thickness, near the bottom
    /// — a wheel or hoverwheel hub
    Wheel,
    /// long, tapered, NOT near the bottom — a thruster, nozzle or gun
    /// barrel; `taper` on the same prim says which end is wider, but
    /// not what that means aerodynamically (see module doc)
    Thruster,
    /// long, far from the centreline, not near the bottom or tapered
    /// enough to read as a thruster — a wing, fin or side-mounted arm
    Wing,
    /// not elongated at all — a torso, hub or cockpit pod
    Body,
}

/// Combine the measurements above into one labelled guess. Thresholds
/// are reasoned about, not fit to labelled data — see the module doc.
pub fn classify(
    verts: &[Vertex],
    up: Vec3,
    body_lo: f32,
    body_hi: f32,
    body_center: Vec3,
    body_half_width: f32,
) -> Kind {
    let elong = elongation(verts);
    let ground = ground_fraction(verts, up, body_lo, body_hi);
    let lateral = lateral_offset(verts, up, body_center, body_half_width);
    let tap = taper(verts, up).abs();

    const ELONGATED: f32 = 1.8;
    const NEAR_GROUND: f32 = 0.35;
    const FAR_LATERAL: f32 = 0.55;
    const TAPERED: f32 = 0.25;

    if elong < ELONGATED {
        return Kind::Body;
    }
    if ground < NEAR_GROUND {
        // low, elongated: a leg (near the spine) or a wheel (short and
        // wide rather than rod-like even though it cleared ELONGATED
        // by its rim-to-hub ratio) -- a true wheel reads far less
        // elongated than a leg in practice, so the remaining split is
        // by how much MORE elongated it is, not a second measurement
        return if elong > ELONGATED * 1.6 { Kind::Leg } else { Kind::Wheel };
    }
    if tap > TAPERED && lateral < FAR_LATERAL {
        return Kind::Thruster;
    }
    if lateral > FAR_LATERAL {
        return Kind::Wing;
    }
    Kind::Body
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vtx(x: f32, y: f32, z: f32) -> Vertex {
        Vertex { pos: [x, y, z], nrm: [0.0, 1.0, 0.0], uv: [0.0, 0.0] }
    }

    /// A thin rod along Z, radius 0.1, from z=-1 to z=1.
    fn rod() -> Vec<Vertex> {
        let mut v = Vec::new();
        for i in 0..16 {
            let a = i as f32 / 16.0 * std::f32::consts::TAU;
            let (s, c) = a.sin_cos();
            v.push(vtx(c * 0.1, s * 0.1, -1.0));
            v.push(vtx(c * 0.1, s * 0.1, 1.0));
        }
        v
    }

    /// A cone along Z: narrow at z=-1 (radius 0.05), wide at z=1 (radius 0.5).
    fn cone() -> Vec<Vertex> {
        let mut v = Vec::new();
        for i in 0..16 {
            let a = i as f32 / 16.0 * std::f32::consts::TAU;
            let (s, c) = a.sin_cos();
            v.push(vtx(c * 0.05, s * 0.05, -1.0));
            v.push(vtx(c * 0.5, s * 0.5, 1.0));
        }
        v
    }

    /// A flat wide disc in the XY plane, thin along Z -- a wheel viewed
    /// face-on: wide cross-section, short axial extent.
    fn disc() -> Vec<Vertex> {
        let mut v = Vec::new();
        for i in 0..16 {
            let a = i as f32 / 16.0 * std::f32::consts::TAU;
            let (s, c) = a.sin_cos();
            v.push(vtx(c * 0.5, s * 0.5, -0.05));
            v.push(vtx(c * 0.5, s * 0.5, 0.05));
        }
        v
    }

    /// A near-cube blob -- a torso or cockpit pod.
    fn blob() -> Vec<Vertex> {
        let mut v = Vec::new();
        for x in [-0.3, 0.3] {
            for y in [-0.3, 0.3] {
                for z in [-0.3, 0.3] {
                    v.push(vtx(x, y, z));
                }
            }
        }
        v
    }

    #[test]
    fn a_rod_is_far_more_elongated_than_a_blob() {
        assert!(elongation(&rod()) > 5.0, "rod: {}", elongation(&rod()));
        assert!(elongation(&blob()) < 1.5, "blob: {}", elongation(&blob()));
    }

    #[test]
    fn a_disc_is_short_and_wide_not_elongated_along_its_own_axis() {
        // the disc's LONGEST axis is in-plane (diameter 1.0), its
        // SHORTEST is the thin Z extent (0.1) -- elongation picks the
        // best (longest-over-cross) reading, which here is dominated
        // by the flat, wide face rather than the thin edge
        let e = elongation(&disc());
        assert!(e < 2.0, "a disc should not read as a rod-like limb: {e}");
    }

    #[test]
    fn a_cone_tapers_toward_its_wide_end() {
        let t = taper(&cone(), Vec3::Z);
        assert!(t > 0.5, "the cone widens toward +Z, taper should be strongly positive: {t}");
        let t_rev = taper(&cone(), -Vec3::Z);
        assert!(t_rev < -0.5, "reversing the axis should flip the sign: {t_rev}");
    }

    #[test]
    fn a_rod_does_not_taper() {
        let t = taper(&rod(), Vec3::Z);
        assert!(t.abs() < 0.05, "a uniform rod should read as untapered: {t}");
    }

    #[test]
    fn ground_fraction_reads_relative_to_the_whole_body() {
        // a prim sitting at the very bottom of a body that spans 0..10
        let low_part: Vec<Vertex> = vec![vtx(0.0, 0.5, 0.0), vtx(0.1, 0.6, 0.0)];
        let g = ground_fraction(&low_part, Vec3::Y, 0.0, 10.0);
        assert!(g < 0.1, "a part at y=0.5..0.6 in a 0..10 body should read near the ground: {g}");
        let high_part: Vec<Vertex> = vec![vtx(0.0, 9.5, 0.0), vtx(0.1, 9.6, 0.0)];
        let g2 = ground_fraction(&high_part, Vec3::Y, 0.0, 10.0);
        assert!(g2 > 0.9, "a part near the top should read near 1.0: {g2}");
    }

    #[test]
    fn lateral_offset_grows_with_distance_from_the_spine() {
        let centre = Vec3::ZERO;
        let near: Vec<Vertex> = vec![vtx(0.05, 0.0, 0.0)];
        let far: Vec<Vertex> = vec![vtx(2.0, 0.0, 0.0)];
        let l_near = lateral_offset(&near, Vec3::Y, centre, 1.0);
        let l_far = lateral_offset(&far, Vec3::Y, centre, 1.0);
        assert!(l_far > l_near, "a part 2m out should read further than one 5cm out: {l_far} vs {l_near}");
    }

    #[test]
    fn classification_matches_the_shapes_it_was_reasoned_from() {
        // A leg: rod-shaped, near the body's bottom, near the spine.
        // Built directly along Y (the `up` axis this test uses) rather
        // than reusing rod() (built along Z) with an axis remap -- an
        // earlier version of this test remapped axes by compressing
        // the rod's own length into a tiny range instead of preserving
        // it, which produced a squat, unelongated shape and failed for
        // a reason that had nothing to do with classify() being wrong.
        let mut leg_verts = Vec::new();
        for i in 0..16 {
            let a = i as f32 / 16.0 * std::f32::consts::TAU;
            let (s, c) = a.sin_cos();
            leg_verts.push(vtx(c * 0.1, -1.9, s * 0.1));
            leg_verts.push(vtx(c * 0.1, -1.0, s * 0.1));
        }
        let leg_kind = classify(&leg_verts, Vec3::Y, -2.0, 2.0, Vec3::ZERO, 1.0);
        assert_eq!(leg_kind, Kind::Leg, "a low, elongated, centred part should read as a leg");

        // A wing: elongated, high up, far from the spine.
        let mut wing_verts = Vec::new();
        for i in 0..16 {
            let a = i as f32 / 16.0 * std::f32::consts::TAU;
            let (s, c) = a.sin_cos();
            wing_verts.push(vtx(2.0, 0.5 + s * 0.1, c * 0.1));
            wing_verts.push(vtx(3.5, 0.5 + s * 0.1, c * 0.1));
        }
        let wing_kind = classify(&wing_verts, Vec3::Y, -2.0, 2.0, Vec3::ZERO, 1.0);
        assert_eq!(wing_kind, Kind::Wing, "a part far from the spine, not near the ground, should read as a wing");

        // A body: the blob, wherever it sits.
        let body_kind = classify(&blob(), Vec3::Y, -2.0, 2.0, Vec3::ZERO, 1.0);
        assert_eq!(body_kind, Kind::Body, "an unelongated blob should read as a body/hub part");
    }
}
