//! SUPERFORM — what a Fragment of the Lattice does to a body.
//!
//! `docs/films/CANON_DECISIONS.md` §4 settles the raid artifact: a fragment
//! grants unbounded local voxel authority, which is `compact.rs`'s
//! `Style::Grow` curve held at peak instead of relaxing to zero. It also
//! settles how the bodies are made, and that half is the one with a trap in
//! it:
//!
//! > Super bodies are derived, not authored. Follow `clones.rs`: one measured
//! > silhouette, eight height-bands of mass fraction plus width and depth,
//! > others generated as principled mutations with a test holding each to the
//! > reference.
//!
//! The trap is that "bigger and meaner" is easy to write and produces seven
//! bodies that are the same body. `clones.rs` avoided it by stating each
//! nature as a DIFFERENCE from the ice reference, so the family reads as one
//! anatomy bent seven ways. A super form has to preserve that: a super
//! cairnback must still be more cairnback than it is anything else, or the
//! fragment has flattened the roster into one silhouette wearing seven
//! palettes.
//!
//! [`recognisable`] is that property as a test rather than an intention, and
//! it is the assertion this module exists to satisfy: for every element, the
//! super body's profile is closer to its OWN base than to any other clone's
//! base. Not "close to the base" — closer than to any rival, which is what
//! recognisability actually means and what a tolerance number does not
//! capture.
//!
//! ## The first design failed this test, and the failure is the useful part
//!
//! The obvious transformation is "taller and broader, breadth faster than
//! height, mass planted lower". It is written down here because it is what I
//! wrote, and `recognisable` rejected it at a margin of **-0.98 for Ice**:
//! super Ice read as Fire.
//!
//! The reason is that BREADTH IS THE AXIS THE ROSTER IS SPREAD ALONG. Air is
//! 0.7, Ice 1.0, Fire 1.15, Rock 1.45. Making every body broader by a constant
//! factor slides each one along that axis into its broader neighbour — and
//! `profile` is height-normalised, so growing breadth faster than height is
//! *exactly* a move along the identity axis and nothing else. The mutation was
//! a roster-flattener wearing the vocabulary of a power-up.
//!
//! ## What works instead: the fragment makes a body MORE ITSELF
//!
//! Two separable things, which is why the first attempt could not be tuned
//! into working — it conflated them.
//!
//! * **Uniform growth** is `height_m` alone. `model_from` derives everything
//!   from `height_m / ICE_REF_HEIGHT_V`, so scaling it scales the whole body
//!   and leaves the silhouette untouched. Size costs no identity.
//! * **Amplified nature** pushes each knob further from the ice reference
//!   rather than in a fixed direction: `breadth`, `thickness` and `mass_shift`
//!   move AWAY from Ice's values by [`AMPLIFY`]. Rock gets squatter, Air gets
//!   thinner, Fire's mass sinks further. Ice, being the reference, amplifies
//!   to itself — correctly, since Ice's nature is to be the baseline.
//!
//! That is also the better fiction. A fragment grants authority over matter,
//! not mass: what it should look like is a body able to hold more of what it
//! already is.
//!
//! The remaining knobs:
//!
//! * **more burr.** Surface break-up is where the voxel authority is visible —
//!   `compact.rs`'s `Grow` makes mass look added rather than rearranged, and
//!   burr is that at silhouette scale.
//! * **horns lengthen but do not multiply.** Adding crown protrusions changes
//!   which creature it is; lengthening the ones it has does not.
//! * **skew is preserved exactly.** Vine slumps asymmetric and that is its
//!   identity; a super vine that straightened up would be a different clone.
//!
//! ## What this module does NOT do
//!
//! It does not animate. Non-uniform band scaling changes skeleton proportions,
//! so the rig needs RETARGETING rather than a scale multiplier — canon names
//! that as the actual work in this feature, more than the mesh — and nothing
//! here touches it. It also does not hold the `Grow` curve at peak; that is a
//! separate state in the fold, because `compact.rs` is only safe to layer onto
//! the shipped morph while its endpoints stay bit-identical.

use glam::Vec3;

use crate::clones::{self, distance, model_from, profile, Band, CloneSpec, CLONES};
use crate::weapons::Element;

/// Uniform growth. `model_from` scales the whole body off `height_m`, so this
/// is size WITHOUT identity drift — the silhouette is height-normalised and
/// does not notice.
pub const SUPER_SCALE: f32 = 1.35;
/// How much further from the ice reference each nature's knobs are pushed.
/// 1.0 would be no change; this is the "more itself" factor and it is the
/// whole difference between a power-up and a roster-flattener.
pub const AMPLIFY: f32 = 1.30;
/// Surface break-up added, clamped at 1.
pub const SUPER_BURR: f32 = 0.25;
/// Horn lengthening. Count is untouched on purpose.
pub const SUPER_HORN: f32 = 1.30;

/// Push a value further from the reference's value for that knob.
#[inline]
fn away_from(v: f32, reference: f32) -> f32 {
    reference + (v - reference) * AMPLIFY
}

/// The same nature under a fragment's authority.
pub fn super_spec(base: &CloneSpec) -> CloneSpec {
    CloneSpec {
        element: base.element,
        name: base.name,
        height_m: base.height_m * SUPER_SCALE,
        // Ice is the reference: its breadth and thickness are 1.0 and its
        // mass sits centred, so amplifying leaves it alone and bends everyone
        // else further into their own shape.
        breadth: away_from(base.breadth, 1.0),
        thickness: away_from(base.thickness, 1.0),
        mass_shift: away_from(base.mass_shift, 0.0),
        horns: (base.horns.0, base.horns.1 * SUPER_HORN, base.horns.2),
        // identity, not decoration
        skew: base.skew,
        burr: (base.burr + SUPER_BURR).min(1.0),
    }
}

/// The super body for an element, as a point cloud in metres, feet at y=0 —
/// the same shape `clones::model` returns, so a renderer needs no new path.
pub fn model(e: Element) -> Vec<Vec3> {
    let base = clones::spec(e).expect("every element has a clone");
    model_from(&super_spec(base))
}

/// Silhouette of the super body for an element.
pub fn super_profile(e: Element) -> [Band; 8] {
    profile(&model(e))
}

/// Is the super body still recognisably its own clone — closer to its own
/// base silhouette than to any other clone's.
///
/// Returns the margin: own distance subtracted from the nearest rival's.
/// Positive means recognisable, and larger means less ambiguous.
pub fn recognisable(e: Element) -> f32 {
    let sup = super_profile(e);
    let own = distance(&sup, &profile(&clones::model(e)));
    let rival = CLONES
        .iter()
        .filter(|c| c.element != e)
        .map(|c| distance(&sup, &profile(&clones::model(c.element))))
        .fold(f32::INFINITY, f32::min);
    rival - own
}

#[cfg(test)]
mod tests {
    use super::*;

    fn elements() -> impl Iterator<Item = Element> {
        CLONES.iter().map(|c| c.element)
    }

    #[test]
    fn every_clone_has_a_super_and_it_is_a_real_body() {
        for e in elements() {
            let pts = model(e);
            assert!(pts.len() > 500, "{e:?} super body has only {} points", pts.len());
            assert!(pts.iter().all(|p| p.is_finite()), "{e:?} produced non-finite points");
            let lowest = pts.iter().map(|p| p.y).fold(f32::INFINITY, f32::min);
            assert!(lowest.abs() < 0.05, "{e:?} does not stand on the floor: {lowest}");
        }
    }

    #[test]
    fn a_super_body_is_still_recognisably_its_own_clone() {
        // THE ASSERTION THIS MODULE EXISTS FOR. Not "close to its base" — a
        // tolerance number would pass for a roster that had collapsed into one
        // silhouette. Closer to its own base than to any rival is what
        // recognisability means.
        for e in elements() {
            let margin = recognisable(e);
            assert!(
                margin > 0.0,
                "{e:?} under a fragment reads as a different clone (margin {margin:.4})"
            );
        }
    }

    #[test]
    fn a_super_body_is_bigger_than_its_base() {
        for e in elements() {
            let base = clones::model(e);
            let sup = model(e);
            let span = |pts: &[Vec3]| {
                let (mut lo, mut hi) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
                for p in pts {
                    lo = lo.min(*p);
                    hi = hi.max(*p);
                }
                hi - lo
            };
            let (b, s) = (span(&base), span(&sup));
            assert!(s.y > b.y, "{e:?} super is not taller: {} vs {}", s.y, b.y);
            assert!(s.x > b.x, "{e:?} super is not broader: {} vs {}", s.x, b.x);
        }
    }


    #[test]
    fn the_fragment_amplifies_a_nature_rather_than_pushing_every_body_one_way() {
        // The corrected design, asserted directly. A constant broadening slid
        // each clone into its broader neighbour, because breadth IS the axis
        // the roster is spread along; amplification moves each one further
        // from the reference in its own direction instead.
        for c in CLONES.iter() {
            let s = super_spec(c);
            let base_gap = (c.breadth - 1.0).abs();
            let super_gap = (s.breadth - 1.0).abs();
            assert!(
                super_gap >= base_gap - 1e-6,
                "{:?} moved TOWARD the reference breadth: {base_gap:.3} -> {super_gap:.3}",
                c.element
            );
            // and the direction is preserved: a broad clone does not become thin
            assert_eq!(
                (c.breadth - 1.0).is_sign_negative(),
                (s.breadth - 1.0).is_sign_negative(),
                "{:?} crossed the reference breadth",
                c.element
            );
        }
    }

    #[test]
    fn recognisability_has_real_margin_rather_than_squeaking_past() {
        // A margin barely above zero would mean the roster is one retune away
        // from collapsing. Recorded as a number so a future knob change that
        // erodes it fails here instead of shipping.
        let worst = elements().map(recognisable).fold(f32::INFINITY, f32::min);
        assert!(worst > 0.05, "thinnest recognisability margin is {worst:.4}");
    }

    #[test]
    fn a_low_slung_clone_sinks_further_and_a_high_one_rises_further() {
        // Amplification is directional, so this is NOT "everyone plants" — a
        // body that carries its mass high carries it higher. Checked on the
        // generated cloud rather than the spec knob so a sign error in
        // `silhouette`'s shift would show.
        let centroid = |pts: &[Vec3]| {
            let h = pts.iter().map(|p| p.y).fold(f32::MIN, f32::max).max(1e-6);
            pts.iter().map(|p| p.y).sum::<f32>() / pts.len() as f32 / h
        };
        for c in CLONES.iter().filter(|c| c.mass_shift.abs() > 0.05) {
            let base = centroid(&clones::model(c.element));
            let sup = centroid(&model(c.element));
            if c.mass_shift < 0.0 {
                assert!(sup <= base + 0.01, "{:?} carries low but rose: {base:.3} -> {sup:.3}", c.element);
            } else {
                assert!(sup >= base - 0.01, "{:?} carries high but sank: {base:.3} -> {sup:.3}", c.element);
            }
        }
    }

    #[test]
    fn identity_knobs_survive_the_fragment() {
        for c in CLONES.iter() {
            let s = super_spec(c);
            assert_eq!(s.element, c.element);
            assert_eq!(s.skew, c.skew, "{:?} lost its asymmetry", c.element);
            assert_eq!(s.horns.0, c.horns.0, "{:?} grew or lost crown horns", c.element);
            assert!(s.burr <= 1.0, "{:?} burr overflowed", c.element);
        }
    }

    #[test]
    fn the_derivation_is_deterministic() {
        // Bodies are generated rather than stored, so two runs must agree or
        // every client sees a different silhouette.
        for e in elements() {
            let a = model(e);
            let b = model(e);
            assert_eq!(a.len(), b.len());
            assert!(a.iter().zip(&b).all(|(p, q)| p == q), "{e:?} is not deterministic");
        }
    }

    #[test]
    fn the_roster_does_not_collapse_under_the_fragment() {
        // The family-level version of recognisability: seven supers must still
        // be seven distinct silhouettes, not one shape in seven palettes.
        let profiles: Vec<_> = elements().map(super_profile).collect();
        for i in 0..profiles.len() {
            for j in (i + 1)..profiles.len() {
                let d = distance(&profiles[i], &profiles[j]);
                assert!(d > 0.05, "supers {i} and {j} are the same silhouette (d {d:.4})");
            }
        }
    }
}
