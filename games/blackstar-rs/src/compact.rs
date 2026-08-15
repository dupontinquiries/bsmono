//! COMPACTION — the fold as geometry, not as a slide.
//!
//! `xform.rs`'s morph lerps vertex i of pose k toward vertex i of pose
//! k+1. That is smooth, cheap, and it is exactly the trick this project
//! set out not to do: vertices travel in straight lines through each
//! other, nothing is conserved, and a panel that should have folded
//! simply stretches across the gap and un-stretches on the far side. It
//! reads as a slide because it IS a slide. The reference material's own
//! answer — the AllSpark compacting matter into cubes, and cubes
//! unfolding into machines — is a better one, and it is buildable:
//! matter goes somewhere on the way, and the somewhere is a lattice.
//!
//! What this module adds is a DISPLACEMENT FIELD layered over the
//! existing lerp, not a replacement for it:
//!
//! * a compaction curve `c(t)` that is 0 at both ends of the fold and
//!   peaks in the middle. The endpoints are therefore untouched — the
//!   vehicle at rest and the robot at rest are bit-identical to what
//!   they are today, which is the property that makes this safe to
//!   layer onto a shipped morph rather than a rewrite of it;
//! * at peak, every vertex is pulled toward the centre of the voxel
//!   cell it occupies. A surface made of arbitrary triangles becomes a
//!   surface made of cube faces, mid-fold, and relaxes back out.
//!
//! Two families, because the reference does two different things and
//! they should not be one knob:
//!
//! * `Style::Shrink` — cells collapse toward the body's own centre as
//!   they quantise. Panels stack inward, the silhouette contracts, and
//!   the body reads as folding into a smaller volume. This is the one
//!   for vehicle panels: a door does not stretch into a shoulder, it
//!   compacts and re-emerges.
//! * `Style::Grow` — cells quantise in place and push OUTWARD along
//!   their own normal before relaxing. Mass appears to be added rather
//!   than rearranged, which is what the reference does for limbs and
//!   weapons that have no vehicle-mode counterpart to fold from.
//!
//! ## The thin-surface problem, solved by the same mechanism
//!
//! This project's own thin-shell audit (`blackstar/tools/
//! thin_shell_check.py`) found real geometry with near-zero thickness —
//! a "barrel" measuring 1.049 x 0.267 x 0.005. A zero-thickness shell
//! is invisible edge-on, shades wrong, and reads as a paper cut-out at
//! exactly the moment a transformation puts it side-on to the camera.
//!
//! `min_thickness` addresses it inside the compaction rather than as a
//! separate mesh-repair pass: a vertex whose cell is degenerate on one
//! axis gets pushed off the degenerate plane by half a cell as the
//! compaction peaks. A flat shell gains real volume exactly while the
//! camera is most likely to catch it edge-on, and returns to its
//! authored (flat) form at rest, where the artist presumably meant it.
//! This does not repair the source asset — the audit's suspects still
//! need looking at — but it stops the fold from showcasing them.

use glam::Vec3;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Style {
    /// panels compact inward — vehicle bodywork folding into a smaller volume
    Shrink,
    /// mass quantises in place and pushes outward — limbs and weapons
    /// that have no vehicle-mode counterpart to fold from
    Grow,
}

#[derive(Clone, Copy, Debug)]
pub struct Params {
    pub style: Style,
    /// voxel edge length, in the model's own units. Larger = blockier,
    /// more obviously cubic. This is the single most visible dial.
    pub cell: f32,
    /// how much of the way to the quantised position a vertex travels
    /// at peak compaction, 0..1. 1.0 is fully cubic mid-fold; lower
    /// values keep some of the original silhouette through the middle.
    pub strength: f32,
    /// cells with an extent below this on any axis get pushed off the
    /// degenerate plane during compaction — see the module doc's
    /// thin-surface note. 0 disables.
    pub min_thickness: f32,
}

impl Default for Params {
    /// TUNED BY EYE, in the browser viewer (`tools/foldviewer.html`),
    /// across six body types — skiff, tank, jet, hauler, crawler,
    /// cycle. These are not guessed defaults: cell 0.35 with strength
    /// 0.55 was the pair that read as geometric compaction on every
    /// rig rather than as noise on the small ones and mush on the
    /// large ones. The first pass here (0.22 / 0.85) was picked before
    /// anything could be seen and was too fine and too strong: the
    /// cells were below the size of the greebles they were quantising,
    /// so the effect read as jitter instead of as cubes.
    fn default() -> Params {
        Params { style: Style::Shrink, cell: 0.35, strength: 0.55, min_thickness: 0.04 }
    }
}

/// The compaction curve: 0 at both ends, 1 at the middle. `sin(pi*t)`
/// raised to a power — the power sharpens the peak so the body spends
/// most of the fold recognisable and only briefly cubic, which is what
/// the reference footage actually does. A flat-topped curve looks like
/// the model got stuck.
pub fn curve(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    // max(0) before powf is load-bearing, not defensive: in f32,
    // sin(PI) is a small NEGATIVE number (~-8.7e-8), and a negative
    // base with a fractional exponent is NaN. Without the clamp every
    // vertex in the mesh becomes NaN on the exact frame a fold
    // completes — caught by the endpoint test below, which is the
    // reason that test asserts on the endpoints rather than trusting
    // the maths to be obviously right.
    let s = (std::f32::consts::PI * t).sin().max(0.0);
    s.powf(1.6)
}

/// Quantise a position to the centre of its voxel cell.
pub fn cell_centre(p: Vec3, cell: f32) -> Vec3 {
    let c = cell.max(1e-4);
    Vec3::new(
        (p.x / c).floor() * c + c * 0.5,
        (p.y / c).floor() * c + c * 0.5,
        (p.z / c).floor() * c + c * 0.5,
    )
}

/// Displace one vertex for the fold position `t`.
///
/// `pos` is the already-lerped position from `xform::Morph::sample` —
/// this runs after the existing blend, so the two compose rather than
/// competing. `nrm` is that vertex's blended normal, used by
/// `Style::Grow` for its outward push and by the thin-surface rule to
/// know which way "off the plane" is. `centre` is the body's own centre
/// for this prim, which `Style::Shrink` pulls toward.
pub fn displace(pos: Vec3, nrm: Vec3, centre: Vec3, t: f32, p: &Params) -> Vec3 {
    let c = curve(t) * p.strength.clamp(0.0, 1.0);
    if c <= 1e-5 {
        return pos; // endpoints untouched, exactly
    }
    let quantised = cell_centre(pos, p.cell);
    let mut target = quantised;

    match p.style {
        Style::Shrink => {
            // cells stack toward the body centre as they quantise: the
            // silhouette contracts instead of just going blocky in place
            let pull = 0.35 * c;
            target = quantised.lerp(centre, pull);
        }
        Style::Grow => {
            // quantise in place, then push out along the normal — mass
            // reads as appearing rather than being rearranged
            target = quantised + nrm.normalize_or(Vec3::Y) * (p.cell * 0.5 * c);
        }
    }

    // thin-surface rescue: a cell that is degenerate on an axis gets
    // half a cell of thickness along the vertex normal while compacted
    if p.min_thickness > 1e-5 {
        let cellspan = p.cell;
        if cellspan < p.min_thickness {
            target += nrm.normalize_or(Vec3::Y) * (p.min_thickness * 0.5 * c);
        }
    }

    pos.lerp(target, c)
}

/// Whether a prim's own bounding box is degenerate enough on some axis
/// that it will read as a paper cut-out edge-on. Mirrors the standalone
/// audit tool's ratio test (`thin_shell_check.py`), kept here so the
/// engine can make the same judgement at runtime without the tool.
pub fn is_thin_shell(lo: Vec3, hi: Vec3, ratio: f32) -> bool {
    let d = hi - lo;
    let dims = [d.x.abs(), d.y.abs(), d.z.abs()];
    let longest = dims.iter().cloned().fold(0.0f32, f32::max);
    let shortest = dims.iter().cloned().fold(f32::MAX, f32::min);
    longest > 1e-6 && shortest / longest < ratio
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_curve_leaves_both_endpoints_exactly_untouched() {
        // the property that makes this safe to layer onto a shipped
        // morph: a vehicle at rest and a robot at rest are unchanged
        assert!(curve(0.0).abs() < 1e-6);
        assert!(curve(1.0).abs() < 1e-6);
        assert!(curve(0.5) > 0.9, "the middle of the fold barely compacts: {}", curve(0.5));
    }

    #[test]
    fn displacement_is_identity_at_the_endpoints_for_both_styles() {
        let p_shrink = Params { style: Style::Shrink, ..Default::default() };
        let p_grow = Params { style: Style::Grow, ..Default::default() };
        let v = Vec3::new(0.37, -1.24, 2.9);
        let n = Vec3::Y;
        let c = Vec3::ZERO;
        for params in [&p_shrink, &p_grow] {
            for t in [0.0, 1.0] {
                let out = displace(v, n, c, t, params);
                assert!((out - v).length() < 1e-5, "endpoint moved at t={t}: {out:?} vs {v:?}");
            }
        }
    }

    #[test]
    fn mid_fold_actually_quantises_toward_cell_centres() {
        let p = Params { style: Style::Shrink, cell: 0.5, strength: 1.0, min_thickness: 0.0 };
        // a vertex deliberately far from its cell centre
        let v = Vec3::new(0.01, 0.01, 0.01);
        let out = displace(v, Vec3::Y, Vec3::ZERO, 0.5, &p);
        assert!((out - v).length() > 0.05, "mid-fold did not move the vertex at all");
        // and it moved toward the quantised lattice, not somewhere random
        let q = cell_centre(v, 0.5);
        assert!(
            (out - q).length() < (v - q).length(),
            "moved away from the cell centre: v={v:?} q={q:?} out={out:?}"
        );
    }

    #[test]
    fn shrink_contracts_toward_the_centre_and_grow_does_not() {
        let base = Params { cell: 0.4, strength: 1.0, min_thickness: 0.0, style: Style::Shrink };
        let grow = Params { style: Style::Grow, ..base };
        let centre = Vec3::ZERO;
        let v = Vec3::new(3.0, 0.0, 0.0); // well out from centre
        let n = Vec3::X; // pointing outward
        let s = displace(v, n, centre, 0.5, &base);
        let g = displace(v, n, centre, 0.5, &grow);
        assert!(
            s.length() < g.length(),
            "shrink should sit closer to the centre than grow: shrink={} grow={}",
            s.length(),
            g.length()
        );
    }

    #[test]
    fn a_degenerate_cell_gains_thickness_only_while_compacted() {
        let p = Params { style: Style::Shrink, cell: 0.01, strength: 1.0, min_thickness: 0.2 };
        let v = Vec3::new(0.0, 0.0, 0.0);
        let n = Vec3::Y;
        // at rest: untouched, the artist's flat surface stays flat
        let rest = displace(v, n, Vec3::ZERO, 0.0, &p);
        assert!((rest - v).length() < 1e-5);
        // mid-fold: pushed off the degenerate plane along the normal
        let mid = displace(v, n, Vec3::ZERO, 0.5, &p);
        assert!(mid.y.abs() > 0.01, "a paper-thin cell got no rescue thickness: {mid:?}");
    }

    #[test]
    fn the_thin_shell_test_agrees_with_the_audit_tools_findings() {
        // the real outlier the audit found: a "barrel" at 1.049 x 0.267
        // x 0.005 -- ratio 0.005/1.049 = 0.0047, far under any sane bar
        let lo = Vec3::ZERO;
        let hi = Vec3::new(1.049, 0.267, 0.005);
        assert!(is_thin_shell(lo, hi, 0.05), "the known-bad barrel was not flagged");
        // and a legitimately chunky part is not flagged
        let cube_hi = Vec3::new(1.0, 0.9, 0.8);
        assert!(!is_thin_shell(lo, cube_hi, 0.05), "a solid part was wrongly flagged");
    }

    #[test]
    fn strength_zero_disables_the_whole_effect() {
        let p = Params { strength: 0.0, ..Default::default() };
        let v = Vec3::new(1.0, 2.0, 3.0);
        for t in [0.0, 0.25, 0.5, 0.75, 1.0] {
            let out = displace(v, Vec3::Y, Vec3::ZERO, t, &p);
            assert!((out - v).length() < 1e-5, "strength=0 still moved a vertex at t={t}");
        }
    }
}
