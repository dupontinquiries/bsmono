//! Placement focus: where the player is looking becomes where the
//! model reads first.
//!
//! The model cannot see the screen. What it gets instead is a FOCUS:
//! an anchor block derived from the player's gaze, a scale tier
//! derived from how far away they are looking, a read region sized to
//! that tier, and a feature id that stays stable while the player
//! jitters but changes when the scale changes. The read region is the
//! "default file" the model opens before doing anything -- the voxel
//! equivalent of the current working directory.
//!
//! Inputs are engine-agnostic: eye position, look direction, and an
//! optional hit (block coords, face normal, distance). unendlich's
//! `raycast::RayHit` maps onto `GazeHit` field for field.
//!
//! SCALE TIERS
//!
//! Distance is the scale signal a first-person camera gives you for
//! free. Someone looking at a block two metres away is placing a
//! flower; someone looking at a hillside fifty blocks off is placing a
//! village. The tier sets the read radius, the default op sizes, and
//! the unit the model should think in.
//!
//!   Detail     hit < 8 blocks     read r=6     "a block, a doorway"
//!   Structure  8..32              read r=16    "a building, a tree"
//!   Landscape  32..96             read r=48    "a hill, a village"
//!   Horizon    > 96 or no hit     read r=96    "terrain shape only"
//!
//! FEATURE IDS
//!
//! `feature_id` hashes (anchor quantised to the tier's cell size, face
//! normal, tier). Small camera movement keeps the id; a change of
//! tier, or looking at a different cell, mints a new one. The chat
//! loop uses it to decide whether the model's cached read context is
//! still valid -- same id, reuse; new id, re-read. `expand()` steps up
//! one tier explicitly, for "no, bigger than that".

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScaleTier { Detail, Structure, Landscape, Horizon }

impl ScaleTier {
    pub fn read_radius(self) -> i32 {
        match self { Self::Detail => 6, Self::Structure => 16, Self::Landscape => 48, Self::Horizon => 96 }
    }
    /// Quantisation cell for the feature id: coarser tiers tolerate
    /// more gaze drift before the id changes.
    pub fn cell(self) -> i32 {
        match self { Self::Detail => 2, Self::Structure => 8, Self::Landscape => 24, Self::Horizon => 64 }
    }
    pub fn from_distance(d: Option<f32>) -> Self {
        match d {
            Some(x) if x < 8.0 => Self::Detail,
            Some(x) if x < 32.0 => Self::Structure,
            Some(x) if x < 96.0 => Self::Landscape,
            _ => Self::Horizon,
        }
    }
    pub fn expand(self) -> Self {
        match self {
            Self::Detail => Self::Structure,
            Self::Structure => Self::Landscape,
            _ => Self::Horizon,
        }
    }
    pub fn name(self) -> &'static str {
        match self { Self::Detail => "detail", Self::Structure => "structure",
                     Self::Landscape => "landscape", Self::Horizon => "horizon" }
    }
    /// Suggested default sizes the model should think in at this tier.
    pub fn typical(self) -> &'static str {
        match self {
            Self::Detail => "single blocks, 1-3 wide; a doorway, a torch, a flower bed",
            Self::Structure => "a building or tree, 5-20 blocks; towers r=3-5, houses 8x8 to 15x15",
            Self::Landscape => "a hill, a village, a grove; 30-80 blocks across; terraform first",
            Self::Horizon => "terrain shape only; nothing detailed is readable at this range",
        }
    }
}

/// What the raycast found. Mirrors unendlich `RayHit` minus the block
/// id (the focus reads it back through the world anyway).
#[derive(Debug, Clone, Copy)]
pub struct GazeHit {
    pub block: (i32, i32, i32),
    /// Unit normal of the face crossed, e.g. (0,1,0) for the top.
    pub face_normal: (i32, i32, i32),
    pub distance: f32,
}

#[derive(Debug, Clone, Copy)]
pub struct Gaze {
    pub eye: (f32, f32, f32),
    pub dir: (f32, f32, f32),
    pub hit: Option<GazeHit>,
}

/// The read region handed to the model.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Region {
    Sphere { center: (i32, i32, i32), radius: i32 },
    Box { min: (i32, i32, i32), max: (i32, i32, i32) },
}

impl Region {
    pub fn bounds(&self) -> ((i32, i32, i32), (i32, i32, i32)) {
        match *self {
            Region::Box { min, max } => (min, max),
            Region::Sphere { center: c, radius: r } =>
                ((c.0 - r, c.1 - r, c.2 - r), (c.0 + r, c.1 + r, c.2 + r)),
        }
    }
    pub fn contains(&self, p: (i32, i32, i32)) -> bool {
        match *self {
            Region::Box { min, max } =>
                p.0 >= min.0 && p.0 <= max.0 && p.1 >= min.1 && p.1 <= max.1 && p.2 >= min.2 && p.2 <= max.2,
            Region::Sphere { center: c, radius: r } => {
                let (dx, dy, dz) = (p.0 - c.0, p.1 - c.1, p.2 - c.2);
                dx * dx + dy * dy + dz * dz <= r * r
            }
        }
    }
}

#[derive(Debug, Clone)]
pub struct Focus {
    /// The block the model should treat as "here": the empty cell in
    /// front of the hit face, i.e. where a placed block would go.
    pub anchor: (i32, i32, i32),
    /// The surface block that was hit (None when looking at sky).
    pub surface: Option<(i32, i32, i32)>,
    pub tier: ScaleTier,
    pub region: Region,
    pub feature_id: String,
    /// True when the gaze hit nothing and the anchor was projected.
    pub projected: bool,
}

impl Focus {
    pub fn from_gaze(g: &Gaze) -> Self {
        Self::from_gaze_at_tier(g, None)
    }

    /// `tier_override` lets the chat loop expand or pin a tier
    /// independently of distance ("bigger", "zoom in").
    pub fn from_gaze_at_tier(g: &Gaze, tier_override: Option<ScaleTier>) -> Self {
        let (anchor, surface, projected) = match g.hit {
            Some(h) => {
                let a = (h.block.0 + h.face_normal.0,
                         h.block.1 + h.face_normal.1,
                         h.block.2 + h.face_normal.2);
                (a, Some(h.block), false)
            }
            None => {
                // Looking at sky: project the gaze to a plausible
                // distance so "build a tower over there" still has a
                // there. Clamped so the anchor is not a kilometre away.
                let d = 48.0f32;
                let len = (g.dir.0 * g.dir.0 + g.dir.1 * g.dir.1 + g.dir.2 * g.dir.2).sqrt().max(1e-6);
                let a = ((g.eye.0 + g.dir.0 / len * d).round() as i32,
                         (g.eye.1 + g.dir.1 / len * d).round() as i32,
                         (g.eye.2 + g.dir.2 / len * d).round() as i32);
                (a, None, true)
            }
        };
        let tier = tier_override.unwrap_or_else(|| ScaleTier::from_distance(g.hit.map(|h| h.distance)));
        let r = tier.read_radius();
        // A sphere for near tiers (a thing and its surroundings), a
        // ground-hugging box for landscape tiers (you care about a
        // footprint, not the sky above it).
        let region = match tier {
            ScaleTier::Detail | ScaleTier::Structure =>
                Region::Sphere { center: anchor, radius: r },
            _ => Region::Box {
                min: (anchor.0 - r, anchor.1 - 16, anchor.2 - r),
                max: (anchor.0 + r, anchor.1 + 32, anchor.2 + r),
            },
        };
        let normal = g.hit.map(|h| h.face_normal).unwrap_or((0, 1, 0));
        let feature_id = feature_id(anchor, normal, tier);
        Focus { anchor, surface, tier, region, feature_id, projected }
    }

    /// Step up a tier, keeping the anchor. New feature id.
    pub fn expand(&self) -> Self {
        let tier = self.tier.expand();
        let r = tier.read_radius();
        let region = match tier {
            ScaleTier::Detail | ScaleTier::Structure =>
                Region::Sphere { center: self.anchor, radius: r },
            _ => Region::Box {
                min: (self.anchor.0 - r, self.anchor.1 - 16, self.anchor.2 - r),
                max: (self.anchor.0 + r, self.anchor.1 + 32, self.anchor.2 + r),
            },
        };
        Focus { tier, region, feature_id: feature_id(self.anchor, (0, 1, 0), tier), ..self.clone() }
    }

    /// Resolve a focus-relative coordinate (`@dx,dz` in the IR) to
    /// world space. Y is left to the ground-finding logic.
    pub fn resolve_xz(&self, rel: (i32, i32)) -> (i32, i32) {
        (self.anchor.0 + rel.0, self.anchor.2 + rel.1)
    }

    /// The context block sent to the model before it plans. Short by
    /// design; the model spends its budget on reading the region, not
    /// on reading about the region.
    pub fn read_hint(&self, surface_name: Option<&str>) -> String {
        let (mn, mx) = self.region.bounds();
        let what = match (self.surface, surface_name) {
            (Some(s), Some(n)) => format!("looking at {n} at {:?}", s),
            (Some(s), None) => format!("looking at a block at {:?}", s),
            (None, _) => "looking at open sky; anchor is projected".to_string(),
        };
        format!(
            "FOCUS {} | {what} | anchor {:?} | tier {} ({})\n\
             READ FIRST: read_region {:?} {:?}, then probe_column at the anchor.\n\
             Coordinates written as @dx,dz in a plan are relative to the anchor.",
            self.feature_id, self.anchor, self.tier.name(), self.tier.typical(), mn, mx)
    }
}

fn feature_id(anchor: (i32, i32, i32), normal: (i32, i32, i32), tier: ScaleTier) -> String {
    let c = tier.cell();
    let q = (anchor.0.div_euclid(c), anchor.1.div_euclid(c), anchor.2.div_euclid(c));
    let mut h = DefaultHasher::new();
    (q, normal, tier.name()).hash(&mut h);
    format!("f{:08x}", (h.finish() & 0xffff_ffff) as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gaze(dist: f32) -> Gaze {
        Gaze {
            eye: (0.0, 70.0, 0.0),
            dir: (1.0, -0.2, 0.0),
            hit: Some(GazeHit { block: (10, 64, 0), face_normal: (0, 1, 0), distance: dist }),
        }
    }

    #[test]
    fn anchor_is_the_empty_cell_in_front_of_the_face() {
        let f = Focus::from_gaze(&gaze(10.0));
        assert_eq!(f.surface, Some((10, 64, 0)));
        assert_eq!(f.anchor, (10, 65, 0), "top face hit -> anchor one above");
        assert!(!f.projected);
    }

    #[test]
    fn distance_sets_the_tier_and_read_radius() {
        assert_eq!(Focus::from_gaze(&gaze(3.0)).tier, ScaleTier::Detail);
        assert_eq!(Focus::from_gaze(&gaze(20.0)).tier, ScaleTier::Structure);
        assert_eq!(Focus::from_gaze(&gaze(60.0)).tier, ScaleTier::Landscape);
        assert_eq!(Focus::from_gaze(&gaze(200.0)).tier, ScaleTier::Horizon);
        let s = Focus::from_gaze(&gaze(20.0));
        assert_eq!(s.region, Region::Sphere { center: (10, 65, 0), radius: 16 });
    }

    #[test]
    fn feature_id_survives_jitter_but_not_a_tier_change() {
        let a = Focus::from_gaze(&gaze(20.0));
        let mut g2 = gaze(21.0);
        g2.hit.as_mut().unwrap().block = (11, 64, 1);        // one block over
        let b = Focus::from_gaze(&g2);
        assert_eq!(a.feature_id, b.feature_id, "small gaze drift must keep the id");
        let c = Focus::from_gaze(&gaze(60.0));
        assert_ne!(a.feature_id, c.feature_id, "a tier change must mint a new id");
    }

    #[test]
    fn expand_steps_up_and_changes_id() {
        let a = Focus::from_gaze(&gaze(3.0));
        let b = a.expand();
        assert_eq!(b.tier, ScaleTier::Structure);
        assert_eq!(b.anchor, a.anchor, "expanding keeps the anchor");
        assert_ne!(a.feature_id, b.feature_id);
        assert!(b.region.contains(a.anchor));
    }

    #[test]
    fn sky_gaze_projects_an_anchor_instead_of_failing() {
        let g = Gaze { eye: (0.0, 70.0, 0.0), dir: (0.0, 0.0, 1.0), hit: None };
        let f = Focus::from_gaze(&g);
        assert!(f.projected);
        assert_eq!(f.anchor, (0, 70, 48));
        assert_eq!(f.tier, ScaleTier::Horizon);
    }

    #[test]
    fn relative_coordinates_resolve_against_the_anchor() {
        let f = Focus::from_gaze(&gaze(10.0));
        assert_eq!(f.resolve_xz((5, -3)), (15, -3));
    }

    #[test]
    fn landscape_region_is_a_ground_hugging_box() {
        let f = Focus::from_gaze(&gaze(60.0));
        match f.region {
            Region::Box { min, max } => {
                assert_eq!(max.1 - min.1, 48, "asymmetric: more above than below");
                assert!(min.1 < f.anchor.1 && max.1 > f.anchor.1);
            }
            _ => panic!("landscape should use a box"),
        }
    }

    #[test]
    fn read_hint_tells_the_model_what_to_do_first() {
        let f = Focus::from_gaze(&gaze(10.0));
        let h = f.read_hint(Some("grass"));
        assert!(h.contains("READ FIRST"));
        assert!(h.contains("read_region"));
        assert!(h.contains(&f.feature_id));
        assert!(h.contains("@dx,dz"), "must explain the relative syntax");
    }
}
