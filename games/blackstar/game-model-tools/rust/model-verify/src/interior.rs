//! interior — cybertronian bones, not speckles on a shell.
//!
//! ## What the existing generator does, and why it reads as noise
//!
//! `exengine/detail.c` distributes a greeble budget "proportional to surface
//! area", scatters sub-parts "across the part's faces", and pushes each one
//! "slightly out along the face normal". That is **surface** detail placed by
//! **density** — so it lands wherever there is area, in no relation to what the
//! body is, and the eye reads it as speckle because there is no structure to
//! read.
//!
//! ## What it was meant to be
//!
//! The bodies are hollow shells — 38–39% of Brawl's edges are open. That is not
//! a defect to work around, it is the whole look: when a panel swings away
//! during a transformation you should see **machinery** inside. Ribs down the
//! spine, actuator assemblies at the joints, a bone chain through each limb.
//!
//! So interior structure is placed from the **skeleton**, not from surface area:
//!
//! * **Ribs** in mirrored pairs along the spine, between the shoulder and hip
//!   landmarks, which is where a ribcage is.
//! * **Actuators** at the joints — shoulders, elbows, hips, knees — because that
//!   is where a machine needs one, and it is what a viewer expects to see when a
//!   limb bends.
//! * **A spine column** connecting them.
//!
//! Every piece is inset from the hull so it reads as *inside*, and pairs are
//! generated as pairs, so the anatomy pass sees the same body plan in the
//! interior that it sees in the armour.
//!
//! ## Why this fixes the artifacts too
//!
//! Two procedural plates scattered at the same depth z-fight. Structure placed
//! at distinct anatomical positions does not collide with itself, because the
//! positions are derived from a skeleton that has no two joints in one place.

use crate::profile::{HIP, KNEE, SHOULDER};

/// What a piece of interior structure is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Bone {
    /// A rib: one of a mirrored pair arcing out from the spine.
    Rib,
    /// The central column.
    Spine,
    /// A joint assembly — the thing you see working when a limb bends.
    Actuator,
    /// A strut running the length of a limb.
    Strut,
}

/// One piece, in body-local coordinates where Y is up and 0..1 spans the body.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Piece {
    pub kind: Bone,
    /// Centre, as fractions of the body's extent: x lateral, y height, z depth.
    pub at: [f64; 3],
    /// Half-extents, same units.
    pub half: [f64; 3],
    /// Which mirrored side, for the anatomy pass and for the U-flip on texture.
    pub right: bool,
}

/// How far inside the hull the structure sits, as a fraction of body width.
///
/// Not zero: structure flush with the shell pokes through it as the panel moves
/// and produces exactly the flickering artifacts the surface generator does.
pub const INSET: f64 = 0.12;

/// Ribs between the shoulder and hip landmarks. Nine pairs reads as a ribcage;
/// far more reads as corrugation.
pub const RIB_PAIRS: usize = 9;

/// The furthest any interior piece may reach from the centreline. Anything
/// beyond this is inside the armour rather than under it.
pub const HULL_LIMIT: f64 = 0.5 - INSET * 0.5;

/// Build the interior for a body.
///
/// `bulk` scales the ribcage's reach — a juggernaut chassis has a deeper chest
/// than a strider, and the chassis multipliers already in `chassis.json` are the
/// natural source.
pub fn build(bulk: f64) -> Vec<Piece> {
    let mut out = Vec::new();
    let bulk = bulk.clamp(0.4, 2.0);

    // spine: a column from hip to shoulder
    let spine_h = (SHOULDER + HIP) * 0.5;
    out.push(Piece {
        kind: Bone::Spine,
        at: [0.0, spine_h, 0.0],
        half: [0.035, (SHOULDER - HIP) * 0.5, 0.035],
        right: false,
    });

    // ribs: pairs arcing out from the spine, evenly spaced over the chest
    for i in 0..RIB_PAIRS {
        let t = (i as f64 + 0.5) / RIB_PAIRS as f64;
        let y = HIP + (SHOULDER - HIP) * t;
        // a ribcage is widest in the middle: taper at both ends, or it reads as
        // a barrel rather than a chest
        let taper = (std::f64::consts::PI * t).sin().max(0.25);
        // Bounded by the hull, not merely scaled by bulk. A rib spans from the
        // spine out to `reach`, so `reach` IS its far edge — multiplying it by
        // a juggernaut's bulk pushed the ribs straight through the armour,
        // which is the same flickering the surface generator produces and the
        // exact thing this module exists to avoid.
        let reach = ((0.5 - INSET) * taper * bulk).min(HULL_LIMIT);
        for right in [false, true] {
            out.push(Piece {
                kind: Bone::Rib,
                at: [if right { reach * 0.5 } else { -reach * 0.5 }, y, 0.0],
                // depth is bounded too: a deep chest is what bulk should buy,
                // but not at the cost of bursting the shell
                half: [reach * 0.5, 0.012, (0.10 * taper * bulk).min(HULL_LIMIT * 0.8)],
                right,
            });
        }
    }

    // actuators at the joints a viewer expects to see working
    for (h, name) in [(SHOULDER, Bone::Actuator), (HIP, Bone::Actuator),
                      (KNEE, Bone::Actuator)] {
        for right in [false, true] {
            let x = if right { 0.30 } else { -0.30 };
            out.push(Piece {
                kind: name,
                at: [x, h, 0.0],
                half: [0.055, 0.055, 0.055],
                right,
            });
        }
    }

    // limb struts: shoulder to hip is the torso's business, so these run the
    // legs, where a bending knee exposes the most interior
    for right in [false, true] {
        let x = if right { 0.30 } else { -0.30 };
        out.push(Piece {
            kind: Bone::Strut,
            at: [x, (HIP + KNEE) * 0.5, 0.0],
            half: [0.03, (HIP - KNEE) * 0.5, 0.03],
            right,
        });
    }
    out
}

/// Is a piece properly inside the hull?
///
/// Structure flush with the shell pokes through as the panel moves, which is
/// the same flickering the surface generator produces — so this is checked
/// rather than assumed.
pub fn is_inside(p: &Piece) -> bool {
    let far_x = p.at[0].abs() + p.half[0];
    let far_z = p.at[2].abs() + p.half[2];
    far_x <= 0.5 - INSET * 0.5 && far_z <= 0.5 - INSET * 0.5
}

/// Do two pieces overlap? Structure that intersects itself z-fights exactly the
/// way scattered greebles do.
pub fn overlaps(a: &Piece, b: &Piece) -> bool {
    (0..3).all(|i| (a.at[i] - b.at[i]).abs() < a.half[i] + b.half[i] - 1e-9)
}

/// Is an overlap between these two intended?
///
/// A rib meeting the spine is an ATTACHMENT, not a collision — that is how a
/// ribcage is built, and a generator that separated them would produce a
/// floating cage. The first version of the disjointness test did not know the
/// difference and flagged the one overlap the design requires.
pub fn is_attachment(a: &Piece, b: &Piece) -> bool {
    matches!((a.kind, b.kind),
        (Bone::Rib, Bone::Spine) | (Bone::Spine, Bone::Rib)
        | (Bone::Strut, Bone::Actuator) | (Bone::Actuator, Bone::Strut))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_piece_sits_inside_the_hull() {
        // Structure flush with the shell pokes through it as a panel moves and
        // produces exactly the flickering the surface generator does.
        for p in build(1.0) {
            assert!(is_inside(&p), "{p:?} reaches the hull");
        }
        // and at the widest chassis too
        for p in build(2.0) {
            assert!(is_inside(&p), "bulk 2.0: {p:?} reaches the hull");
        }
    }

    #[test]
    fn the_structure_does_not_intersect_itself() {
        // Two procedural plates at the same depth z-fight. Placing from a
        // skeleton avoids that by construction, but "by construction" is worth
        // checking.
        let pieces = build(1.0);
        let mut attachments = 0;
        for i in 0..pieces.len() {
            for j in (i + 1)..pieces.len() {
                if !overlaps(&pieces[i], &pieces[j]) { continue; }
                assert!(is_attachment(&pieces[i], &pieces[j]),
                        "{:?} collides with {:?}", pieces[i], pieces[j]);
                attachments += 1;
            }
        }
        // and the ribcage IS attached — a cage floating clear of the spine is
        // as wrong as one intersecting a leg
        assert!(attachments >= RIB_PAIRS, "ribs must meet the spine: {attachments}");
    }

    #[test]
    fn ribs_come_in_mirrored_pairs_so_the_anatomy_pass_sees_a_body_plan() {
        // The interior must read as the same body the armour describes, or the
        // symmetry pass finds one plan outside and noise inside.
        let ribs: Vec<Piece> = build(1.0).into_iter().filter(|p| p.kind == Bone::Rib).collect();
        assert_eq!(ribs.len(), RIB_PAIRS * 2);
        for r in ribs.iter().filter(|r| !r.right) {
            let twin = ribs.iter().find(|t| t.right && (t.at[1] - r.at[1]).abs() < 1e-9)
                .expect("every left rib has a right twin");
            assert!((twin.at[0] + r.at[0]).abs() < 1e-9, "mirrored in x");
            assert_eq!(twin.half, r.half, "and identical in size");
        }
    }

    #[test]
    fn the_ribcage_is_widest_in_the_middle() {
        // Even ribs read as a barrel; tapered ones read as a chest.
        let ribs: Vec<Piece> = build(1.0).into_iter()
            .filter(|p| p.kind == Bone::Rib && !p.right).collect();
        let widest = ribs.iter().map(|r| r.half[0]).fold(0.0f64, f64::max);
        let mid = ribs[ribs.len() / 2].half[0];
        assert!((mid - widest).abs() < 1e-6, "the middle rib is the widest");
        assert!(ribs[0].half[0] < widest * 0.7, "and the ends taper");
        assert!(ribs[ribs.len() - 1].half[0] < widest * 0.7);
    }

    #[test]
    fn actuators_sit_at_the_joints_a_viewer_expects_to_see_working() {
        // Not scattered by area: at the shoulder, hip and knee, because that is
        // where a machine needs one and where a bending limb exposes it.
        let acts: Vec<Piece> = build(1.0).into_iter()
            .filter(|p| p.kind == Bone::Actuator).collect();
        assert_eq!(acts.len(), 6, "three joints, both sides");
        for h in [SHOULDER, HIP, KNEE] {
            assert_eq!(acts.iter().filter(|a| (a.at[1] - h).abs() < 1e-9).count(), 2,
                       "a pair at height {h}");
        }
    }

    #[test]
    fn the_ribcage_spans_the_chest_and_nothing_else() {
        // Ribs below the hip or above the shoulder are not ribs.
        for r in build(1.0).into_iter().filter(|p| p.kind == Bone::Rib) {
            assert!(r.at[1] > HIP && r.at[1] < SHOULDER,
                    "rib at {} is outside the chest ({HIP}..{SHOULDER})", r.at[1]);
        }
    }

    #[test]
    fn bulk_deepens_the_chest_without_bursting_the_hull() {
        // chassis.json's multipliers are the natural source, and a juggernaut
        // must not push its ribs through its own armour.
        let slim = build(0.6);
        let heavy = build(1.8);
        let reach = |v: &[Piece]| v.iter().filter(|p| p.kind == Bone::Rib)
            .map(|p| p.at[0].abs() + p.half[0]).fold(0.0f64, f64::max);
        assert!(reach(&heavy) > reach(&slim), "heavier is deeper");
        for p in heavy { assert!(is_inside(&p)); }
        // and an absurd value is clamped rather than exploding
        for p in build(99.0) { assert!(is_inside(&p)); }
    }
}
