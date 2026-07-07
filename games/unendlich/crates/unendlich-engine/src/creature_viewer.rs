//! UI-agnostic creature-viewer model — the shared core behind the standalone
//! egui "stats tool" and the in-game inspector overlay. It turns a
//! [`CreatureBlueprint`] into (a) a flat list of 2D boxes projected
//! orthographically into a unit square, ready for any 2D painter to scale into a
//! panel, and (b) a compact stats readout. Keeping this pure means the egui shell
//! is a thin wrapper over geometry that's fully tested here, with no windowing
//! needed to verify it.

use crate::creature_design::{BlueprintBox, BoxRole, CreatureBlueprint};

/// Which orthographic view to project. The eye looks down the *depth* axis.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ViewAxis {
    /// Looking along Z: horizontal = X (forward), vertical = Y (up). The profile.
    Side,
    /// Looking along X: horizontal = Z (right), vertical = Y (up). Head-on.
    Front,
    /// Looking down Y: horizontal = X (forward), vertical = Z (right). Plan view.
    Top,
}

impl ViewAxis {
    /// (horizontal-axis index, vertical-axis index, depth-axis index) into a [x,y,z].
    fn axes(self) -> (usize, usize, usize) {
        match self {
            ViewAxis::Side => (0, 1, 2),
            ViewAxis::Front => (2, 1, 0),
            ViewAxis::Top => (0, 2, 1),
        }
    }
}

/// A blueprint box projected into the unit square `[0,1]²`, y pointing *down*
/// (screen convention). `depth` orders boxes back-to-front for the painter.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ProjectedBox {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub color: [f32; 3],
    pub depth: f32,
    pub role: u8,
    /// T08-R27 scale-bumpiness `[0,1]` carried from the genome — a 3D consumer
    /// drives skin normal/displacement from it; the 2D painter uses it as a
    /// light per-box shading cue (see `project`).
    pub bumpiness: f32,
}

/// A flat colour for each box role — the palette the viewer paints with.
pub fn role_color(role: BoxRole) -> [f32; 3] {
    match role {
        BoxRole::Body | BoxRole::Segment => [0.55, 0.42, 0.30],
        BoxRole::Neck => [0.60, 0.47, 0.34],
        BoxRole::Head => [0.66, 0.52, 0.38],
        BoxRole::Snout => [0.70, 0.56, 0.42],
        BoxRole::Eye => [0.95, 0.93, 0.55],
        BoxRole::Jaw => [0.62, 0.40, 0.34],
        BoxRole::Horn => [0.85, 0.82, 0.74],
        BoxRole::Leg => [0.48, 0.36, 0.26],
        BoxRole::Wing => [0.42, 0.46, 0.62],
        BoxRole::Tail => [0.52, 0.40, 0.30],
        BoxRole::Ridge => [0.40, 0.50, 0.42],
        BoxRole::Hair => [0.36, 0.55, 0.32],
    }
}

/// Deterministic `[0,1)` hash of an integer (splitmix64-style), for stable
/// per-box pattern jitter.
fn hash_unit(x: u64) -> f32 {
    // r193 (guide 4.1): one splitmix64 — util::hash_util. Byte-identical.
    crate::util::hash_util::mix64_unit(x)
}

/// Modulate a base box colour by the creature's skin pattern (T08-R27). `idx`
/// is the box's index in the blueprint and `depth` its depth-axis offset, so
/// the overlay is deterministic and stable per box. `Plain` is the identity;
/// the others blend toward `accent` (the pattern colour gene) in their
/// characteristic layout (transverse bands / blotches / net).
pub fn pattern_modulate(
    base: [f32; 3],
    kind: crate::creature_design::PatternKind,
    accent: [f32; 3],
    idx: usize,
    depth: f32,
) -> [f32; 3] {
    use crate::creature_design::PatternKind as P;
    let mix = |t: f32| {
        let t = t.clamp(0.0, 1.0);
        [
            base[0] + (accent[0] - base[0]) * t,
            base[1] + (accent[1] - base[1]) * t,
            base[2] + (accent[2] - base[2]) * t,
        ]
    };
    match kind {
        P::Plain => base,
        P::Banded => mix(if idx % 2 == 0 { 0.0 } else { 0.55 }),
        P::Mottled => mix(hash_unit(idx as u64 ^ (depth.to_bits() as u64).wrapping_mul(0x9E37_79B9)) * 0.7),
        P::Reticulated => {
            let h = hash_unit((idx as u64).wrapping_mul(0x2545_F491_4F6C_DD1D) ^ depth.to_bits() as u64);
            mix(if h > 0.6 { 0.75 } else { 0.08 })
        }
    }
}

/// Project every box of `bp` into the unit square for the given view, uniformly
/// scaled (so the creature keeps its proportions) and centered. Returned
/// back-to-front so a painter can draw them in order.
pub fn project(bp: &CreatureBlueprint, view: ViewAxis) -> Vec<ProjectedBox> {
    let (ui, vi, di) = view.axes();
    if bp.boxes.is_empty() {
        return Vec::new();
    }
    // World-space bounds of the projection.
    // r193 (guide 4.2): two shared axis folds.
    let (umin, umax) = crate::util::bbox::axis_bounds(
        bp.boxes.iter().map(|b| (b.offset[ui], b.half[ui])),
    );
    let (vmin, vmax) = crate::util::bbox::axis_bounds(
        bp.boxes.iter().map(|b| (b.offset[vi], b.half[vi])),
    );
    let span = (umax - umin).max(vmax - vmin).max(1e-6);
    // Center the (possibly non-square) bounds within the unit square.
    let uoff = (1.0 - (umax - umin) / span) * 0.5;
    let voff = (1.0 - (vmax - vmin) / span) * 0.5;

    let mut out: Vec<ProjectedBox> = bp
        .boxes
        .iter()
        .enumerate()
        .map(|(i, b): (usize, &BlueprintBox)| {
            let w = (2.0 * b.half[ui]) / span;
            let h = (2.0 * b.half[vi]) / span;
            let x = uoff + (b.offset[ui] - b.half[ui] - umin) / span;
            // Screen y points down, so the world *top* (vmax side) maps to small y.
            let y = voff + (vmax - (b.offset[vi] + b.half[vi])) / span;
            let depth = b.offset[di];
            // T08-R27: overlay the skin pattern, then apply a light bumpiness
            // shading cue. (The 3D consumer drives real normal/displacement from
            // `bumpiness`; the 2D projector can only approximate it as shading.)
            let mut color = pattern_modulate(b.color, bp.pattern, bp.pattern_color, i, depth);
            if bp.scale_bumpiness > 0.0 {
                let shade = 1.0 + (hash_unit(i as u64) - 0.5) * 0.3 * bp.scale_bumpiness;
                for c in &mut color {
                    *c = (*c * shade).clamp(0.0, 1.0);
                }
            }
            ProjectedBox {
                x,
                y,
                w,
                h,
                color,
                depth,
                role: b.role as u8,
                bumpiness: bp.scale_bumpiness,
            }
        })
        .collect();
    // Back-to-front: smaller depth first (drawn under), larger depth last (on top).
    out.sort_by(|a, b| a.depth.partial_cmp(&b.depth).unwrap_or(std::cmp::Ordering::Equal));
    out
}

/// Compact readout for the stats panel.
#[derive(Clone, Debug, PartialEq)]
pub struct ViewerStats {
    pub arch: &'static str,
    /// Elemental affinity label, or "none" when the creature carries none.
    pub element: &'static str,
    pub age: &'static str,
    pub looseness: f32,
    pub standing_height: f32,
    pub box_count: usize,
    /// World bounding-box dimensions: forward length (X), width (Z), height (Y).
    pub length: f32,
    pub width: f32,
    pub height: f32,
}

/// Derive the stats readout from a hatched blueprint.
pub fn stats(bp: &CreatureBlueprint) -> ViewerStats {
    // r193 (guide 4.2): three shared axis folds.
    let ax = |i: usize| crate::util::bbox::axis_bounds(
        bp.boxes.iter().map(move |b| (b.offset[i], b.half[i])),
    );
    let ((xmin, xmax), (ymin, ymax), (zmin, zmax)) = (ax(0), ax(1), ax(2));
    let dims = if bp.boxes.is_empty() {
        (0.0, 0.0, 0.0)
    } else {
        (xmax - xmin, zmax - zmin, ymax - ymin)
    };
    ViewerStats {
        arch: bp.arch,
        element: if bp.element.is_empty() { "none" } else { bp.element },
        age: bp.age,
        looseness: bp.looseness,
        standing_height: bp.standing_height(),
        box_count: bp.boxes.len(),
        length: dims.0,
        width: dims.1,
        height: dims.2,
    }
}

/// The viewer's mutable selection state — what the egui panel edits. Pure, so
/// the "build the egg → hatch" path is testable without any UI. `arche`/`element`
/// of `None` mean "let the seed decide" (random roll), matching [`EggSpec`].
#[derive(Clone, Copy, Debug)]
pub struct ViewerState {
    pub seed: u64,
    pub arche: Option<crate::creature_design::Arche>,
    pub element: Option<crate::sim::elements::Affinity>,
    pub view: ViewAxis,
    /// Environment switch mask fed to the genome's epigenetic expression (the
    /// genetics viz lets you toggle cold/heat/etc. and watch traits adapt).
    pub env: crate::genetics::EnvSwitches,
}

impl Default for ViewerState {
    fn default() -> Self {
        ViewerState { seed: 1, arche: None, element: None, view: ViewAxis::Side, env: 0 }
    }
}

impl ViewerState {
    pub fn new() -> Self {
        Self::default()
    }

    /// The genome implied by the current seed (a wild roll for this seed).
    pub fn genome(&self) -> crate::genetics::Genome {
        crate::genetics::Genome::from_seed(self.seed)
    }

    /// The egg this selection describes.
    pub fn egg_spec(&self) -> crate::creature_design::EggSpec {
        crate::creature_design::EggSpec { choice: self.arche, element: self.element, seed: self.seed }
    }

    /// Hatch the current selection into a blueprint to display.
    pub fn hatch(&self) -> CreatureBlueprint {
        self.egg_spec().hatch()
    }

    /// Advance to the next seed (the "reroll" button), deterministically.
    pub fn reroll(&mut self) {
        self.seed = self.seed.wrapping_mul(0x9E37_79B9_7F4A_7C15).wrapping_add(0x1234_5678);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::creature_design::{Arche, EggSpec};

    fn bp() -> CreatureBlueprint {
        EggSpec::chosen(Arche::Quadruped, 7).hatch()
    }

    #[test]
    fn projection_is_one_box_each_inside_the_unit_square() {
        let b = bp();
        let p = project(&b, ViewAxis::Side);
        assert_eq!(p.len(), b.boxes.len());
        for r in &p {
            assert!(r.x >= -1e-4 && r.x <= 1.0 + 1e-4, "x in unit: {}", r.x);
            assert!(r.y >= -1e-4 && r.y <= 1.0 + 1e-4, "y in unit: {}", r.y);
            assert!(r.x + r.w <= 1.0 + 1e-4, "x+w in unit: {}", r.x + r.w);
            assert!(r.y + r.h <= 1.0 + 1e-4, "y+h in unit: {}", r.y + r.h);
            assert!(r.w >= 0.0 && r.h >= 0.0);
        }
    }

    #[test]
    fn projection_preserves_proportions_uniformly() {
        // A single box: its on-screen w:h must match its world X:Y half-extent
        // ratio (uniform scale, no per-axis stretch).
        let b = bp();
        let p = project(&b, ViewAxis::Side);
        // Find the torso (role Body) and compare against its world extents.
        let torso_world = &b.boxes[0];
        let torso_proj = p.iter().find(|r| r.role == BoxRole::Body as u8).unwrap();
        let world_ratio = torso_world.half[0] / torso_world.half[1];
        let proj_ratio = torso_proj.w / torso_proj.h;
        assert!((world_ratio - proj_ratio).abs() < 1e-3, "world {world_ratio} vs proj {proj_ratio}");
    }

    #[test]
    fn higher_in_the_world_is_higher_on_screen() {
        // Two stacked boxes; the upper one must get the smaller screen y.
        let mut b = bp();
        b.boxes.truncate(0);
        b.boxes.push(BlueprintBox { offset: [0.0, 2.0, 0.0], half: [0.5, 0.5, 0.5], color: [0.7, 0.5, 0.4], role: BoxRole::Head });
        b.boxes.push(BlueprintBox { offset: [0.0, 0.0, 0.0], half: [0.5, 0.5, 0.5], color: [0.5, 0.4, 0.3], role: BoxRole::Body });
        let p = project(&b, ViewAxis::Side);
        let head = p.iter().find(|r| r.role == BoxRole::Head as u8).unwrap();
        let body = p.iter().find(|r| r.role == BoxRole::Body as u8).unwrap();
        assert!(head.y < body.y, "head ({}) sits above body ({}) on screen", head.y, body.y);
    }

    #[test]
    fn stats_summarize_the_blueprint() {
        let dragon = EggSpec::chosen(Arche::Dragon, 3).with_element(Some(crate::sim::elements::Affinity::FIRE)).hatch();
        let s = stats(&dragon);
        assert!(s.arch.starts_with("dragon"));
        assert_eq!(s.element, "fire");
        assert_eq!(s.box_count, dragon.boxes.len());
        assert!(s.standing_height > 0.0 && s.length > 0.0 && s.height > 0.0);
        // An elementless fauna reports "none", not an empty string.
        let fauna = EggSpec::chosen(Arche::Biped, 3).hatch();
        assert_eq!(stats(&fauna).element, "none");
    }

    #[test]
    fn viewer_state_drives_the_egg_and_reroll_changes_the_seed() {
        let mut st = ViewerState::new();
        st.arche = Some(Arche::Dragon);
        st.element = Some(crate::sim::elements::Affinity::ICE);
        // The state's egg matches an equivalent hand-built spec.
        let from_state = st.hatch();
        let direct = EggSpec::chosen(Arche::Dragon, st.seed).with_element(Some(crate::sim::elements::Affinity::ICE)).hatch();
        assert_eq!(from_state.arch, direct.arch);
        assert_eq!(from_state.element, "ice");
        // Reroll advances the seed (and is deterministic).
        let before = st.seed;
        st.reroll();
        assert_ne!(st.seed, before);
        let mut st2 = ViewerState { seed: before, ..st };
        st2.reroll();
        assert_eq!(st.seed, st2.seed, "reroll is deterministic");
    }

    #[test]
    fn plain_pattern_is_identity_others_blend_in_range() {
        use crate::creature_design::PatternKind;
        let base = [0.4f32, 0.3, 0.2];
        let accent = [0.9f32, 0.1, 0.7];
        // Plain leaves the colour untouched.
        assert_eq!(pattern_modulate(base, PatternKind::Plain, accent, 3, 1.0), base);
        // The others stay in gamut and actually differ from the base somewhere.
        for kind in [PatternKind::Banded, PatternKind::Mottled, PatternKind::Reticulated] {
            let mut any_diff = false;
            for i in 0..16usize {
                let c = pattern_modulate(base, kind, accent, i, i as f32 * 0.5);
                for ch in c {
                    assert!((0.0..=1.0).contains(&ch), "{:?} colour in gamut", kind);
                }
                if c != base {
                    any_diff = true;
                }
            }
            assert!(any_diff, "{:?} should modulate some boxes", kind);
        }
    }

    #[test]
    fn projection_carries_bumpiness_from_the_genome() {
        let mut b = bp();
        b.scale_bumpiness = 0.8;
        let p = project(&b, ViewAxis::Side);
        assert!(p.iter().all(|r| (r.bumpiness - 0.8).abs() < 1e-6), "bumpiness surfaced on every box");
    }
}
