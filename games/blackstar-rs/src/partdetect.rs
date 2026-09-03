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
/// Same measurement as `elongation`, for callers that already have
/// plain positions rather than full `Vertex`es (`loadbearing.rs`,
/// which works in `Vec3` since it needs no normal/uv data). Small
/// enough to keep separate rather than route `elongation` itself
/// through a shared Vec3 core that `extents`/`taper` would also need
/// touching for no benefit to them.
pub fn elongation_of_points(points: &[Vec3]) -> f32 {
    if points.is_empty() {
        return 0.0;
    }
    let mut best = 0.0f32;
    for axis in AXES {
        let (pa, pb) = perp_pair(axis);
        let (mut lo, mut hi) = (f32::MAX, f32::MIN);
        let (mut a_lo, mut a_hi) = (f32::MAX, f32::MIN);
        let (mut b_lo, mut b_hi) = (f32::MAX, f32::MIN);
        for &p in points {
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
        let long = (hi - lo).max(0.0);
        let cross = ((a_hi - a_lo).max(b_hi - b_lo)).max(1e-6);
        best = best.max(long / cross);
    }
    best
}

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

/// One prim, as `leg_symmetry_under`/`forward_sign_hint` need it: its
/// geometry and what `classify` already thinks it is (computed once
/// by the caller, which typically already ran `classify` for other
/// reasons — this does not recompute it).
pub struct KindedPrim<'a> {
    pub verts: &'a [Vertex],
    pub kind: Kind,
}

/// Fraction of `points` that have a mirror-image partner across
/// `left_right` (through `center`), within `tolerance` — a real
/// structural signature that legs come in pairs, independent of
/// `orient.rs`'s whole-body mass-distribution `up_score`. 1.0 means
/// every point found a partner (a symmetric quadruped/biped/hexapod
/// leg layout); 0.0 means none did (scattered, or a genuinely
/// asymmetric design, or the axis choice feeding this is wrong).
///
/// Deliberately excludes a point from matching ITSELF as its own
/// mirror — a single leg sitting exactly on the centreline would
/// otherwise score a hollow 1.0 by matching its own reflection, which
/// is a leg with no partner, not a symmetric pair.
pub fn leg_symmetry_fraction(points: &[Vec3], left_right: Vec3, center: Vec3, tolerance: f32) -> f32 {
    if points.is_empty() {
        return 0.0;
    }
    let mut matched = 0;
    for (i, &p) in points.iter().enumerate() {
        let rel = p - center;
        let mirrored = rel - 2.0 * rel.dot(left_right) * left_right + center;
        let found = points.iter().enumerate().any(|(j, &q)| {
            j != i && (q - mirrored).length() <= tolerance
        });
        if found {
            matched += 1;
        }
    }
    matched as f32 / points.len() as f32
}

/// Cross-checks a chosen up-axis against `orient.rs`'s own, using a
/// DIFFERENT signal than `up_score`'s whole-body mass distribution:
/// do the parts `classify` calls legs/wheels under this axis actually
/// come in mirrored pairs? Real agreement between two independent
/// measurements is a stronger claim than either alone — this does not
/// replace `orient::Guess::up_confident`, it is a second opinion a
/// caller can compare it against.
///
/// Returns `None` when fewer than 2 leg/wheel candidates exist (no
/// pairing question to ask), `Some(fraction)` otherwise.
pub fn leg_symmetry_under(
    prims: &[KindedPrim],
    up: Vec3,
    left_right: Vec3,
    body_lo: f32,
    body_hi: f32,
    body_center: Vec3,
) -> Option<f32> {
    let contacts: Vec<Vec3> = prims
        .iter()
        .filter(|p| matches!(p.kind, Kind::Leg | Kind::Wheel))
        .filter_map(|p| {
            p.verts.iter().map(|v| Vec3::from_array(v.pos))
                .min_by(|a, b| a.dot(up).partial_cmp(&b.dot(up)).unwrap())
        })
        .collect();
    if contacts.len() < 2 {
        return None;
    }
    let span = (body_hi - body_lo).max(1e-3);
    // tolerance scales with the body's own size -- a fixed metres
    // value would be too loose for a tiny drone and too tight for a
    // building-sized walker
    let tolerance = span * 0.20;
    Some(leg_symmetry_fraction(&contacts, left_right, body_center, tolerance))
}

/// `orient.rs`'s own doc names the gap this fills: forward's AXIS is
/// resolved by mirror symmetry, but its SIGN — which end is the front
/// — "is not resolved by this heuristic and is left for the confidence
/// flag... rather than silently guess." This is the first real attempt
/// at that sign, not a replacement for the confidence flag: a
/// cockpit or head sitting elevated and biased toward one end is a
/// real, common design convention (visibility for a cockpit, a
/// sensor/eye line for a head) in both vehicle and humanoid forms —
/// common, not universal. Some real designs put the cabin aft.
///
/// Picks the most head/cockpit-LIKE candidate among `Kind::Body`
/// prims — the one sitting highest (by `ground_fraction`) among those
/// also reasonably centred (not a side-mounted lump) — and reports
/// which side of the body's own centre, along `forward`, it sits on.
/// `Some(1.0)`: keep `forward`'s current sign, the candidate is on the
/// positive side. `Some(-1.0)`: flip it. `None`: no candidate stood
/// out enough to have an opinion, which callers should treat the same
/// as `orient::Guess::forward_confident == false` — a reason to fall
/// back to the confidence flag, not a silent default.
pub fn forward_sign_hint(prims: &[KindedPrim], up: Vec3, forward: Vec3, body_lo: f32, body_hi: f32) -> Option<f32> {
    let mut best: Option<(f32, Vec3)> = None; // (elevation, centroid)
    for p in prims {
        if p.kind != Kind::Body || p.verts.is_empty() {
            continue;
        }
        let ground = ground_fraction(p.verts, up, body_lo, body_hi);
        // "elevated" only -- a torso is Kind::Body too and usually sits
        // in the middle of the height range, not a useful head/cockpit
        // candidate; requiring the TOP quarter keeps this from picking
        // whichever blob happens to be biggest instead of highest.
        const HEAD_LIKE_MIN: f32 = 0.72;
        if ground < HEAD_LIKE_MIN {
            continue;
        }
        let mut centroid = Vec3::ZERO;
        for v in p.verts {
            centroid += Vec3::from_array(v.pos);
        }
        centroid /= p.verts.len() as f32;
        let take = match best {
            None => true,
            Some((be, _)) => ground > be,
        };
        if take {
            best = Some((ground, centroid));
        }
    }
    let (_, centroid) = best?;
    let along = centroid.dot(forward);
    Some(if along >= 0.0 { 1.0 } else { -1.0 })
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

    #[test]
    fn forward_sign_hint_follows_the_elevated_head_like_blob() {
        // A body spanning y -2..2. A torso-ish blob near the middle
        // height, and a small "head" blob near the top, offset toward
        // +Z. Both are Kind::Body (blob() shape, unelongated).
        let torso: Vec<Vertex> = blob(); // centred at origin, y -0.3..0.3 -- NOT top-quarter
        let mut head = Vec::new();
        for x in [-0.1, 0.1] {
            for y in [1.7, 1.9] { // well within the top quarter of -2..2
                for z in [2.7, 2.9] { // offset toward +Z
                    head.push(vtx(x, y, z));
                }
            }
        }
        let prims = vec![
            KindedPrim { verts: &torso, kind: Kind::Body },
            KindedPrim { verts: &head, kind: Kind::Body },
        ];
        let hint = forward_sign_hint(&prims, Vec3::Y, Vec3::Z, -2.0, 2.0);
        assert_eq!(hint, Some(1.0), "the head sits on the +Z side, so +Z should be confirmed as forward");

        // Same body, head offset toward -Z instead -- the hint should flip.
        let mut head_back = Vec::new();
        for x in [-0.1, 0.1] {
            for y in [1.7, 1.9] {
                for z in [-2.9, -2.7] {
                    head_back.push(vtx(x, y, z));
                }
            }
        }
        let prims2 = vec![
            KindedPrim { verts: &torso, kind: Kind::Body },
            KindedPrim { verts: &head_back, kind: Kind::Body },
        ];
        let hint2 = forward_sign_hint(&prims2, Vec3::Y, Vec3::Z, -2.0, 2.0);
        assert_eq!(hint2, Some(-1.0), "the head sits on the -Z side, so the +Z guess should be flipped");
    }

    #[test]
    fn forward_sign_hint_ignores_non_body_and_non_elevated_candidates() {
        // A leg (elongated, near the ground) offset toward +Z should
        // NOT be picked as a head candidate even though it is the only
        // other prim -- kind and elevation both gate this, not just
        // "the other thing in the body."
        let torso = blob();
        let mut leg_verts = Vec::new();
        for i in 0..16 {
            let a = i as f32 / 16.0 * std::f32::consts::TAU;
            let (s, c) = a.sin_cos();
            leg_verts.push(vtx(c * 0.1, -1.9, s * 0.1 + 2.0));
            leg_verts.push(vtx(c * 0.1, -1.0, s * 0.1 + 2.0));
        }
        let prims = vec![
            KindedPrim { verts: &torso, kind: Kind::Body },
            KindedPrim { verts: &leg_verts, kind: Kind::Leg },
        ];
        let hint = forward_sign_hint(&prims, Vec3::Y, Vec3::Z, -2.0, 2.0);
        assert_eq!(hint, None, "a low leg should not be mistaken for a head, even offset toward +Z");
    }

    #[test]
    fn leg_symmetry_fraction_of_a_perfect_quadruped_is_one() {
        // four legs at the corners of a square, centred on the origin
        // -- every leg's mirror across X lands on another leg exactly
        let legs = [
            Vec3::new(-1.0, -1.8, -1.0), Vec3::new(1.0, -1.8, -1.0),
            Vec3::new(-1.0, -1.8, 1.0), Vec3::new(1.0, -1.8, 1.0),
        ];
        let f = leg_symmetry_fraction(&legs, Vec3::X, Vec3::ZERO, 0.05);
        assert_eq!(f, 1.0, "a perfect quadruped's legs should all find a mirror partner");
    }

    #[test]
    fn leg_symmetry_fraction_of_scattered_legs_is_low() {
        // three legs with no mirror structure at all
        let legs = [Vec3::new(-2.0, -1.8, -1.0), Vec3::new(0.3, -1.8, 3.0), Vec3::new(1.7, -1.8, -0.5)];
        let f = leg_symmetry_fraction(&legs, Vec3::X, Vec3::ZERO, 0.05);
        assert!(f < 0.5, "scattered legs with no left-right structure should score low: {f}");
    }

    #[test]
    fn a_single_leg_on_the_centreline_does_not_falsely_match_itself() {
        // one leg sitting exactly on the mirror axis -- its own
        // reflection is itself, which must not count as "found a
        // partner": a lone centreline leg has no partner
        let legs = [Vec3::new(0.0, -1.8, 0.0)];
        let f = leg_symmetry_fraction(&legs, Vec3::X, Vec3::ZERO, 0.05);
        assert_eq!(f, 0.0, "a single centreline leg must not match its own reflection");
    }

    #[test]
    fn leg_symmetry_under_needs_at_least_two_candidates() {
        let torso = blob();
        let leg = {
            let mut v = Vec::new();
            for i in 0..8 {
                let a = i as f32 / 8.0 * std::f32::consts::TAU;
                let (s, c) = a.sin_cos();
                v.push(vtx(c * 0.1 - 1.0, -1.9, s * 0.1));
                v.push(vtx(c * 0.1 - 1.0, -1.0, s * 0.1));
            }
            v
        };
        let prims = vec![
            KindedPrim { verts: &torso, kind: Kind::Body },
            KindedPrim { verts: &leg, kind: Kind::Leg },
        ];
        let out = leg_symmetry_under(&prims, Vec3::Y, Vec3::X, -2.0, 2.0, Vec3::ZERO);
        assert_eq!(out, None, "a single leg candidate should not produce a symmetry score at all");
    }

    #[test]
    fn leg_symmetry_under_finds_a_symmetric_pair() {
        let torso = blob();
        let leg_at = |x: f32| {
            let mut v = Vec::new();
            for i in 0..8 {
                let a = i as f32 / 8.0 * std::f32::consts::TAU;
                let (s, c) = a.sin_cos();
                v.push(vtx(c * 0.1 + x, -1.9, s * 0.1));
                v.push(vtx(c * 0.1 + x, -1.0, s * 0.1));
            }
            v
        };
        let left = leg_at(-1.0);
        let right = leg_at(1.0);
        let prims = vec![
            KindedPrim { verts: &torso, kind: Kind::Body },
            KindedPrim { verts: &left, kind: Kind::Leg },
            KindedPrim { verts: &right, kind: Kind::Leg },
        ];
        let out = leg_symmetry_under(&prims, Vec3::Y, Vec3::X, -2.0, 2.0, Vec3::ZERO);
        assert_eq!(out, Some(1.0), "a genuinely mirrored leg pair should score 1.0");
    }
}

