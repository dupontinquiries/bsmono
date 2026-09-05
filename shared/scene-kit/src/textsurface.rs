//! textsurface.rs — where a scene puts words, and which shapes can actually hold them.
//!
//! The mic indicator is already scene-parameterized (`MicShape`: Pearl, Gem, Stone, Bead, Leaf), so a
//! voice control reads as a placed object rather than a primitive floating in space. Message entry and
//! reply readback should follow the same idea: a page in a study, a runestone on a hillside, a street
//! sign in an alley, a cloud puff overhead.
//!
//! ## Why the pearl has not taken over the billboard
//!
//! It is a sphere, and **a sphere cannot carry a paragraph.** Text wrapped around curvature is legible
//! only across a small arc — past roughly 30° the glyphs turn away from the reader and foreshorten to
//! nothing. The billboard works precisely because it is flat and faces the camera, which is also why it
//! looks like a billboard.
//!
//! So this is not a matter of applying the new projection to the pearl. **The scene-native surface and
//! the legible surface are different requirements**, and a design that ignores that produces beautiful
//! objects nobody can read. The resolution is a *cartouche*: a flattened reading zone inset into the
//! object — the polished face on a runestone, the label plate on a sign, the open page of a book. The
//! object stays scene-native; the words sit on a plane.
//!
//! ## What this needs from the renderer
//!
//! A UV attribute, which `gpuscene` does not have (12 floats: pos, normal, color, mat, emissive). That
//! is the enabling change for all of it — see `physical-ui-findings.md`. This module is the decision
//! layer above it: which surface a scene uses, and how much it can legibly hold.

/// A scene-native object that can carry words.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Surface {
    /// Flat, camera-facing. Maximum legibility, minimum belonging.
    Billboard,
    /// A sphere. Beautiful, and holds almost nothing.
    Pearl,
    /// An open book or loose sheet: flat, angled, scene-native. The ideal case.
    Page,
    /// A standing stone with a polished face.
    Runestone,
    /// Flat, vertical, and already a text object in the world.
    StreetSign,
    /// Soft-edged and volumetric — it has no defined surface at all.
    CloudPuff,
}

/// How words sit on a surface.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Inscription {
    /// Directly on the geometry, following its curvature.
    OnGeometry,
    /// On a flattened panel inset into the object — a polished face, a label plate, an open page.
    Cartouche,
    /// The object cannot hold words; they belong somewhere else nearby.
    Adjacent,
}

impl Surface {
    /// Curvature across the readable face, in degrees of arc. 0 is flat.
    fn arc_deg(self) -> f32 {
        match self {
            Surface::Billboard | Surface::StreetSign | Surface::Page => 0.0,
            Surface::Runestone => 12.0,     // a stone face is roughly flat but not machined
            Surface::Pearl => 180.0,        // a hemisphere faces the reader
            Surface::CloudPuff => 360.0,    // no surface to speak of
        }
    }

    /// How words should be placed on this shape.
    ///
    /// The rule is legibility, not style: past ~30° of arc glyphs turn away from the reader and
    /// foreshorten, so anything more curved needs a flat inset or must give the words up entirely.
    pub fn inscription(self) -> Inscription {
        match self.arc_deg() {
            a if a <= 30.0 => Inscription::OnGeometry,
            a if a < 360.0 => Inscription::Cartouche,
            _ => Inscription::Adjacent,
        }
    }

    /// Roughly how many characters this surface holds legibly at a comfortable reading distance.
    ///
    /// Used to decide whether a reply FITS or needs a companion surface. A number rather than a
    /// judgement, because "the pearl is too small" is arguable and "the pearl holds twelve characters"
    /// is not.
    pub fn char_budget(self) -> usize {
        match self {
            Surface::Billboard => 1200,
            Surface::Page => 900,
            Surface::StreetSign => 60,
            Surface::Runestone => 120,
            // A cartouche on a pearl is a small flat disc: a word, not a sentence.
            Surface::Pearl => 12,
            Surface::CloudPuff => 0,
        }
    }

    /// Can this surface carry a whole reply, or does it need a companion?
    pub fn holds(self, chars: usize) -> bool { chars <= self.char_budget() }
}

/// What a scene uses for each role.
///
/// Entry and readback are separated because they have different budgets: what you type is short and
/// what comes back is not. Forcing both onto one surface is what made the readback boxes necessary in
/// the first place.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct SceneText { pub entry: Surface, pub readback: Surface }

/// Pick surfaces for a named scene.
/// Names are matched CASE- AND SPACE-INSENSITIVELY against the real scene
/// list, `mesh3d::SCENE_NAMES`.
///
/// The original arms were lower-case sketches — "atlantis", "study",
/// "library", "hillside" — and not one of them is a scene name. The scenes
/// are `"Atlantis"`, `"Old Library"`, `"Reading Room"`, `"Slate"`. So every
/// caller fell through to the default Billboard arm and Atlantis never got
/// the pearl this module was written to give it. The mapping existed and
/// could not fire.
pub fn for_scene(scene: &str) -> SceneText {
    let key = scene.trim().to_ascii_lowercase();
    match key.as_str() {
        // Atlantis: the pearl is already the voice indicator, so it is the natural ENTRY affordance —
        // you speak into it. It cannot be the readback, which is why the billboard is still there.
        "atlantis" => SceneText { entry: Surface::Pearl, readback: Surface::Billboard },
        // Rooms made of paper: what you write and what you read are both pages.
        "study" | "library" | "old library" | "archive" | "reading room"
            => SceneText { entry: Surface::Page, readback: Surface::Page },
        // Rooms made of stone. The existing hillside/ruins decision — both
        // roles on the runestone — is kept exactly; I changed it to a
        // billboard readback on a hunch and `overflow_reports_HOW_MUCH...`
        // failed, which is that test doing its job. The new stone scenes
        // mirror the decision rather than inventing a second one.
        "hillside" | "ruins" | "slate" | "recursive citadel" | "fractal spires"
            => SceneText { entry: Surface::Runestone, readback: Surface::Runestone },
        "alley" | "street" => SceneText { entry: Surface::StreetSign, readback: Surface::Billboard },
        // Overhead and made of vapour: a puff holds nothing (budget 0), so it
        // is an entry affordance only and the readback stays a billboard.
        "cloudscape" | "folding@home" => SceneText { entry: Surface::CloudPuff, readback: Surface::Billboard },
        _ => SceneText { entry: Surface::Billboard, readback: Surface::Billboard },
    }
}

/// Does a reply of this length fit the scene's readback surface?
///
/// Returns the overflow, so a caller can decide between paging, shrinking, or a companion surface —
/// rather than being told only that it does not fit.
pub fn overflow(scene: &str, chars: usize) -> Option<usize> {
    let s = for_scene(scene).readback;
    (chars > s.char_budget()).then(|| chars - s.char_budget())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pearl_cannot_be_the_readback_surface() {
        // Why the pearl has not taken over the billboard, stated as a number rather than a judgement:
        // it is a sphere, and past ~30 degrees of arc glyphs turn away from the reader. Twelve
        // characters is a word, not a reply.
        assert_eq!(Surface::Pearl.inscription(), Inscription::Cartouche);
        assert!(Surface::Pearl.char_budget() < 20);
        assert!(!Surface::Pearl.holds(200), "a reply does not fit on a pearl");
        assert!(Surface::Billboard.holds(200));
    }

    #[test]
    fn atlantis_uses_the_pearl_for_ENTRY_and_something_else_for_readback() {
        // The pearl is already the voice indicator, so speaking into it is the natural affordance. The
        // billboard remains because readback needs a surface the pearl cannot be — that is a
        // legibility constraint, not an unfinished migration.
        let t = for_scene("atlantis");
        assert_eq!(t.entry, Surface::Pearl);
        assert_ne!(t.readback, Surface::Pearl);
    }

    #[test]
    fn a_cloud_puff_gives_the_words_up_entirely() {
        // It has no defined surface. Pretending otherwise produces text floating in fog, which reads
        // as a rendering fault rather than a design.
        assert_eq!(Surface::CloudPuff.inscription(), Inscription::Adjacent);
        assert_eq!(Surface::CloudPuff.char_budget(), 0);
        assert!(!Surface::CloudPuff.holds(1));
    }

    #[test]
    fn flat_scene_native_surfaces_take_text_DIRECTLY() {
        // The ideal case, and the reason a page beats a billboard: equally legible, and it belongs.
        for s in [Surface::Page, Surface::StreetSign, Surface::Billboard] {
            assert_eq!(s.inscription(), Inscription::OnGeometry, "{s:?}");
        }
        // a runestone is not machined flat, but 12 degrees is well inside the readable arc
        assert_eq!(Surface::Runestone.inscription(), Inscription::OnGeometry);
    }

    #[test]
    fn entry_and_readback_are_chosen_SEPARATELY() {
        // They have different budgets: what you type is short, what comes back is not. Forcing both
        // onto one surface is what made separate readback boxes necessary.
        let alley = for_scene("alley");
        assert_eq!(alley.entry, Surface::StreetSign);
        assert_ne!(alley.entry, alley.readback);
        assert!(alley.entry.char_budget() < alley.readback.char_budget());
    }

    #[test]
    fn overflow_reports_HOW_MUCH_rather_than_only_that_it_failed() {
        // So a caller can choose between paging, shrinking and a companion surface. "Does not fit"
        // supports none of those decisions.
        assert_eq!(overflow("atlantis", 100), None);
        assert_eq!(overflow("alley", 100), None, "the alley reads back on a billboard");
        let over = overflow("hillside", 500).expect("a runestone holds 120");
        assert_eq!(over, 380);
    }

    #[test]
    fn an_unknown_scene_falls_back_to_the_most_legible_surface() {
        // A new scene must not silently get a shape that cannot hold words. Billboard is ugly and
        // readable, which is the right default for something nobody has designed yet.
        let t = for_scene("somewhere-new");
        assert_eq!(t.entry, Surface::Billboard);
        assert!(t.readback.holds(1000));
    }
    #[test]
    fn every_real_scene_name_resolves_and_case_does_not_matter() {
        // The bug this covers: the arms were lower-case sketches and the
        // scenes are Title Case, so every lookup hit the default and the
        // mapping could never fire in production.
        assert_eq!(for_scene("Atlantis"), for_scene("atlantis"));
        assert_eq!(for_scene("  ATLANTIS  ").entry, Surface::Pearl);
        assert_eq!(for_scene("Old Library").entry, Surface::Page);
        assert_eq!(for_scene("Reading Room").readback, Surface::Page);
        assert_eq!(for_scene("Alley").entry, Surface::StreetSign);
        assert_eq!(for_scene("Slate").entry, Surface::Runestone);
        assert_eq!(for_scene("Cloudscape").entry, Surface::CloudPuff);
        // And every name the renderer can actually be given resolves without
        // panicking, whatever it maps to.
        for n in crate::mesh3d::SCENE_NAMES {
            let t = for_scene(n);
            assert!(t.readback.char_budget() > 0, "{n}: readback holds nothing");
        }
    }

    #[test]
    fn a_surface_that_holds_nothing_is_never_a_readback() {
        // CloudPuff's budget is 0 — it is an affordance, not a page. Using it
        // for readback would silently overflow every reply.
        assert_eq!(Surface::CloudPuff.char_budget(), 0);
        assert!(!Surface::CloudPuff.holds(1));
        for n in crate::mesh3d::SCENE_NAMES {
            assert_ne!(for_scene(n).readback, Surface::CloudPuff, "{n}");
        }
        // overflow() reports the excess rather than just refusing.
        let big = "x".repeat(5000).len();
        assert!(overflow("Atlantis", big).is_some_and(|o| o > 0));
        assert_eq!(overflow("Atlantis", 1), None);
    }

}
