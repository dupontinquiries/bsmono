//! profile — every signal about a model, in one place.
//!
//! No single measurement decides whether a model can be used. Symmetry says
//! whether there is a body plan; weldedness says whether components mean
//! anything; UVs say whether it can take a texture; openness says whether
//! mirroring will invert its normals; and the **reference skeleton** says
//! whether it is posed. Any one of them alone has led this work down a wrong
//! path at some point:
//!
//! * unwelded meshes reported "no repeats" — weldedness would have said why;
//! * the hovertank reported "asymmetric" — the component profile shows it is
//!   65% one hull, so there is nothing to be symmetric *with*;
//! * Brawl's two forms were compared as a rigid rearrangement, but the robot is
//!   in a **posed stance**, so part of the difference is pose rather than
//!   transformation. Only a neutral reference reveals that.
//!
//! ## The reference
//!
//! `dude.iqm` is the corpus's only rigged model: 20 joints, Y-up, in a T-pose.
//! Measured from it, with joint positions resolved properly through the parent
//! rotations (translations are in the parent's local space — summing them
//! without the quaternions puts the toes above the head, which is what a first
//! attempt produced):
//!
//! | landmark | height |
//! |---|---|
//! | head | 100% |
//! | shoulders | 93.3% |
//! | hips | 68.7% |
//! | knees | 38.2% |
//! | feet | 3.3% |
//!
//! A model whose mass sits at those heights is neutrally posed. One whose legs
//! thicken in depth, or whose limb mass shifts fore-and-aft with height, is
//! posed — Brawl's robot moves from 0.32 to 0.52 in depth between ankle and hip.

/// Landmark heights from `dude.iqm`, as fractions of total height.
pub const HEAD: f64 = 1.000;
pub const SHOULDER: f64 = 0.933;
pub const HIP: f64 = 0.687;
pub const KNEE: f64 = 0.382;
pub const FOOT: f64 = 0.033;

/// Everything measured about one model.
#[derive(Debug, Clone, PartialEq)]
pub struct Profile {
    pub verts: usize,
    pub components: usize,
    /// Vertices in the single largest component. When this dominates, the model
    /// is one welded lump and no part-based method applies to it.
    pub largest_component: usize,
    /// Mirrored pairs found, and the fraction of vertices they explain.
    pub pairs: usize,
    pub paired_fraction: f64,
    /// Edges belonging to exactly one triangle, over all edges.
    pub open_fraction: f64,
    /// True when the mesh is unwelded — every triangle carrying private
    /// vertices. Openness and component counts are both meaningless until this
    /// is false.
    pub unwelded: bool,
    /// Distinct UVs over vertices. Zero means the model cannot take a texture.
    pub uv_fraction: f64,
    /// How far limb mass drifts fore-and-aft between low and high — 0 for a
    /// neutral stance, rising with a bent or striding pose.
    pub pose_drift: f64,
}

impl Profile {
    /// Is this model one welded lump rather than an assembly?
    pub fn is_monolithic(&self) -> bool {
        self.verts > 0 && self.largest_component as f64 / self.verts as f64 > 0.5
    }

    /// Can the anatomy pass work on it?
    pub fn has_body_plan(&self) -> bool {
        !self.is_monolithic() && self.paired_fraction >= 0.25
    }

    /// Is it posed rather than neutral? Above this, comparing it to another
    /// form as a rigid rearrangement will attribute pose to the transformation
    /// and get both wrong.
    pub fn is_posed(&self) -> bool { self.pose_drift > POSE_TOL }

    /// Can it receive a generated texture without further work?
    pub fn needs_unwrap(&self) -> bool { self.uv_fraction < 0.01 }

    /// A mirrored copy of a shell has inverted normals, so a transfer must flag
    /// the flip. Solids do not care.
    pub fn mirror_flips_normals(&self) -> bool { !self.unwelded && self.open_fraction > 0.05 }

    /// One line saying what can actually be done with this model. The point of
    /// gathering the signals together: a verdict, not six numbers to interpret.
    pub fn verdict(&self) -> &'static str {
        if self.unwelded { "weld first — component and openness figures are meaningless" }
        else if self.is_monolithic() { "one welded hull — needs mesh segmentation, not part matching" }
        else if !self.has_body_plan() { "no usable symmetry — fall back to the reference skeleton" }
        else if self.is_posed() { "posed — separate pose from transform before comparing forms" }
        else { "usable: body plan found, neutral stance" }
    }
}

/// Fore-and-aft drift above which a model counts as posed. Brawl's robot
/// measures 0.20 (ankle 0.32 to hip 0.52); a T-posed figure is near zero.
pub const POSE_TOL: f64 = 0.10;

/// Measure fore-and-aft drift of limb mass with height.
///
/// `bands` are (height_fraction, depth_centre) samples up ONE LIMB. A vertical
/// limb holds depth constant; a bent one leans, and the slope IS the pose.
///
/// The emphasis matters. Sampled over a whole model this measures the
/// SILHOUETTE, not the stance: a corpus run that banded entire models reported
/// Brawl's tank as "posed" at 0.41 drift, which is meaningless — a tank has no
/// pose, it just narrows toward the top. The caller must isolate a limb first,
/// which means the body plan has to be found before pose can be measured. That
/// ordering is the point of gathering the signals together rather than reading
/// any one of them alone.
pub fn pose_drift(bands: &[(f64, f64)]) -> f64 {
    if bands.len() < 2 { return 0.0; }
    let lo = bands.iter().map(|b| b.1).fold(f64::MAX, f64::min);
    let hi = bands.iter().map(|b| b.1).fold(f64::MIN, f64::max);
    hi - lo
}

/// Where a landmark sits on this model, if its proportions match the reference.
///
/// Used to say "the knees should be here" on a model with no skeleton — which
/// is the whole reason to keep a reference around.
pub fn landmark_height(model_lo: f64, model_span: f64, fraction: f64) -> f64 {
    model_lo + model_span * fraction
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base() -> Profile {
        Profile {
            verts: 10_000, components: 60, largest_component: 300,
            pairs: 20, paired_fraction: 0.7, open_fraction: 0.38,
            unwelded: false, uv_fraction: 0.0, pose_drift: 0.02,
        }
    }

    #[test]
    fn an_unwelded_mesh_is_reported_first_because_it_invalidates_the_rest() {
        // Component counts and openness are both meaningless on a mesh where
        // every triangle carries private vertices — scorponok's rib reads 100%
        // open and 241,530 components, neither of which means anything.
        let p = Profile { unwelded: true, ..base() };
        assert!(p.verdict().contains("weld first"));
        assert!(!p.mirror_flips_normals(), "openness cannot be trusted yet");
    }

    #[test]
    fn a_monolithic_model_is_told_apart_from_an_asymmetric_one() {
        // The hovertank is not "a model with no symmetry" — it is 65% ONE
        // welded hull, so there are no parts to be symmetric with. Different
        // diagnosis, different fix.
        let hull = Profile { largest_component: 6_500, paired_fraction: 0.03, ..base() };
        assert!(hull.is_monolithic());
        assert!(hull.verdict().contains("segmentation"));

        let scattered = Profile { largest_component: 400, paired_fraction: 0.03, ..base() };
        assert!(!scattered.is_monolithic());
        assert!(scattered.verdict().contains("reference skeleton"));
    }

    #[test]
    fn a_posed_model_is_flagged_before_it_is_compared_to_another_form() {
        // Brawl's robot drifts 0.20 fore-and-aft between ankle and hip. Treating
        // that as part of the transformation attributes pose to the fold and
        // gets both wrong.
        let posed = Profile { pose_drift: 0.20, ..base() };
        assert!(posed.is_posed());
        assert!(posed.verdict().contains("posed"));
        let neutral = Profile { pose_drift: 0.02, ..base() };
        assert!(!neutral.is_posed());
        assert!(neutral.verdict().contains("usable"));
    }

    #[test]
    fn pose_drift_measures_the_lean_not_the_thickness() {
        // A vertical limb holds its depth-centre constant however thick it is;
        // a bent one leans. The slope is what distinguishes them.
        let straight = [(0.1, 0.50), (0.2, 0.50), (0.3, 0.51), (0.4, 0.50)];
        assert!(pose_drift(&straight) < 0.02);
        // Brawl's robot, measured
        let brawl = [(0.10, 0.32), (0.20, 0.41), (0.30, 0.45), (0.40, 0.52)];
        assert!(pose_drift(&brawl) > POSE_TOL, "{}", pose_drift(&brawl));
        // and one sample cannot show a slope
        assert_eq!(pose_drift(&[(0.1, 0.5)]), 0.0);
        assert_eq!(pose_drift(&[]), 0.0);
    }

    #[test]
    fn the_reference_places_landmarks_on_a_model_with_no_skeleton() {
        // dude.iqm is the corpus's only rig, so it is the only source of "the
        // knees should be about here" for the other 2,132 models.
        let (lo, span) = (0.0, 2.0);
        assert!((landmark_height(lo, span, HIP) - 1.374).abs() < 1e-6);
        assert!((landmark_height(lo, span, KNEE) - 0.764).abs() < 1e-6);
        // and the landmarks are in the order a body has them
        assert!(HEAD > SHOULDER && SHOULDER > HIP && HIP > KNEE && KNEE > FOOT);
    }

    #[test]
    fn a_shell_needs_the_mirror_flag_and_a_solid_does_not() {
        // A mirrored copy of an open shell has inverted normals and renders
        // inside-out; a closed solid does not care.
        assert!(base().mirror_flips_normals(), "38% open is a shell");
        let solid = Profile { open_fraction: 0.0, ..base() };
        assert!(!solid.mirror_flips_normals());
    }

    #[test]
    fn the_texture_question_is_answered_separately_from_the_geometry_one() {
        // A model can have a perfect body plan and still be untextured — 547 of
        // them are — and the two need different work.
        let p = base();
        assert!(p.has_body_plan());
        assert!(p.needs_unwrap(), "all-zero UVs");
        let textured = Profile { uv_fraction: 0.5, ..base() };
        assert!(!textured.needs_unwrap());
    }
}
