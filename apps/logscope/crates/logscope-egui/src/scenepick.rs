//! scenepick — making the physical UI an actual UI.
//!
//! ## What was missing
//!
//! The 3D scene had no hit testing of any kind: no pick, no raycast, no
//! intersect. It is a backdrop that happens to be lit. Nothing in it can be
//! pointed at, which is also why the cursor had nowhere to appear — there was
//! nothing for it to be *on*.
//!
//! Ornament without affordance is the failure mode of a "physical" interface:
//! it looks like it should respond, so people try, and nothing happens. That is
//! worse than a flat control, because a flat control does not make the promise.
//!
//! ## Screen space, not world space
//!
//! Picking happens after projection, and that is a deliberate choice rather
//! than an easier one. A sphere test in world space is tidier maths and gets
//! accessibility WRONG: an object twenty metres away projects to four pixels,
//! and four pixels is not a target anybody can hit. The minimum comes from
//! [`uikit_egui::adaptive`] — 44pt on touch, 28 on a pointer — and it is a
//! floor on the SCREEN radius, so a distant thing keeps a usable target while
//! staying visually small.
//!
//! That padding is why nearest-in-depth is not enough on its own: two padded
//! targets can overlap where the objects do not, so ties break toward the
//! smaller on-screen item. The big thing behind is easy to hit anyway.
//!
//! ## States
//!
//! [`State`] is the ordinary set — idle, hover, focus, pressed, disabled — and
//! the invariants that make it accessible are asserted rather than assumed:
//!
//! * FOCUS IS NOT HOVER. Hover does not exist for a keyboard or a touch
//!   screen, so anything shown only on hover is invisible to both. Focus has
//!   its own visible treatment.
//! * DISABLED IS NOT PICKABLE, and says so — a disabled control that silently
//!   swallows clicks reads as a broken one.
//! * Every state is visually distinct from every other by a margin, which is
//!   what stops "focused" and "hovered" being the same faint glow.
//!
//! Pure: geometry and state in, decisions out. No egui, no GL, no frame.

use crate::spatial::{OrbitCam, Vec3};

/// A pickable thing in the scene.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Target {
    /// Stable id, so a pick survives the scene being rebuilt.
    pub id: u32,
    /// World-space centre.
    pub at: Vec3,
    /// World-space radius of the pickable region.
    pub radius: f32,
    /// A disabled target is skipped by [`pick`] and reports [`State::Disabled`].
    pub enabled: bool,
}

/// Interaction state, in the order a control moves through it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State { Idle, Hover, Focus, Pressed, Disabled }

impl State {
    /// Outline width in points. Focus is the WIDEST because it is the only
    /// state a keyboard user can perceive; making it subtler than hover is the
    /// commonest way a 3D interface locks that user out.
    pub fn ring_width(self) -> f32 {
        match self {
            State::Idle => 0.0,
            State::Hover => 1.5,
            State::Focus => 3.0,
            State::Pressed => 2.0,
            State::Disabled => 0.0,
        }
    }

    /// How much the target lifts toward the viewer, in world units. Physical
    /// affordance: hover floats, pressed sinks BELOW rest so the click has a
    /// direction, and focus does not move at all — a focus ring that displaces
    /// the object makes keyboard navigation feel like the scene is lurching.
    pub fn lift(self) -> f32 {
        match self {
            State::Idle | State::Disabled => 0.0,
            State::Hover => 0.06,
            State::Focus => 0.0,
            State::Pressed => -0.03,
        }
    }

    /// Albedo multiplier. Disabled desaturates rather than dims alone: dimming
    /// on its own is indistinguishable from shadow in a lit 3D scene.
    pub fn tint(self) -> (f32, f32) {
        match self {
            State::Idle => (1.00, 1.00),
            State::Hover => (1.12, 1.00),
            State::Focus => (1.06, 1.00),
            State::Pressed => (0.88, 1.00),
            State::Disabled => (0.72, 0.35), // (brightness, saturation)
        }
    }

    pub fn is_interactive(self) -> bool { !matches!(self, State::Disabled) }
}

/// The screen-space radius a target must present, whatever its distance.
///
/// `min_touch_target` is a DIAMETER, so the floor is half of it.
pub fn min_screen_radius(min_touch_target: f32) -> f32 { min_touch_target * 0.5 }

/// Where a target lands on screen: `(x, y, radius, depth)`.
///
/// `None` when it is behind the camera. The radius is projected from the
/// world-space one and then raised to the accessible floor.
pub fn project_target(
    cam: &OrbitCam, t: &Target, cx: f32, cy: f32, scale: f32, min_touch_target: f32,
) -> Option<(f32, f32, f32, f32)> {
    let (sx, sy, depth) = cam.project(t.at, cx, cy, scale)?;
    // The same perspective divide `project` uses, applied to the radius.
    let projected = t.radius * scale / depth;
    let r = projected.max(min_screen_radius(min_touch_target));
    Some((sx, sy, r, depth))
}

/// The target under a screen point, or `None`.
///
/// Nearest in DEPTH wins among genuine overlaps; where only the accessibility
/// padding overlaps, the smaller on-screen target wins. Disabled targets are
/// skipped entirely — they are not pickable and must not shadow what is behind
/// them.
pub fn pick(
    cam: &OrbitCam, targets: &[Target], px: f32, py: f32,
    cx: f32, cy: f32, scale: f32, min_touch_target: f32,
) -> Option<u32> {
    let mut best: Option<(u32, f32, f32, bool)> = None; // id, depth, pointer distance, real_hit
    for t in targets.iter().filter(|t| t.enabled) {
        let Some((sx, sy, r, depth)) = project_target(cam, t, cx, cy, scale, min_touch_target)
        else { continue };
        let d = ((px - sx).powi(2) + (py - sy).powi(2)).sqrt();
        if d > r { continue }
        // Did the pointer land on the object itself, or only in its padding?
        let unpadded = t.radius * scale / depth;
        let real_hit = d <= unpadded;

        best = Some(match best {
            None => (t.id, depth, d, real_hit),
            Some(b) => {
                let better = match (real_hit, b.3) {
                    // A real hit always beats a padding-only hit, however deep.
                    (true, false) => true,
                    (false, true) => false,
                    // Both real: nearest in depth wins, as usual.
                    (true, true) => depth < b.1,
                    // Both padding-only: NEAREST CENTRE wins.
                    //
                    // This branch first compared screen radii, which cannot
                    // discriminate: a padding-only hit is by definition one
                    // where the floor exceeded the projected radius, so every
                    // padded target has exactly the floor radius and the
                    // comparison is always a tie. The test that was supposed to
                    // exercise it could not, which is how the dead branch
                    // surfaced. Distance to the centre is what the user means
                    // by "the one I was aiming at".
                    (false, false) => d < b.2,
                };
                if better { (t.id, depth, d, real_hit) } else { b }
            }
        });
    }
    best.map(|b| b.0)
}

/// The state a target should render in.
///
/// Focus is independent of the pointer: a target can be focused while the
/// pointer is elsewhere, and that must still be visible.
pub fn state_of(t: &Target, hovered: Option<u32>, focused: Option<u32>, pressed: bool) -> State {
    if !t.enabled { return State::Disabled }
    let is_hover = hovered == Some(t.id);
    if is_hover && pressed { return State::Pressed }
    if focused == Some(t.id) { return State::Focus }
    if is_hover { return State::Hover }
    State::Idle
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cam() -> OrbitCam { OrbitCam::default() }
    fn t(id: u32, at: Vec3, radius: f32) -> Target { Target { id, at, radius, enabled: true } }

    const TOUCH: f32 = 44.0;
    const CX: f32 = 400.0;
    const CY: f32 = 300.0;
    const SCALE: f32 = 600.0;

    fn at_centre(id: u32, radius: f32) -> Target {
        // The camera's target is what projects to screen centre.
        t(id, cam().target, radius)
    }

    #[test]
    fn a_point_on_a_target_picks_it() {
        let ts = [at_centre(7, 0.5)];
        assert_eq!(pick(&cam(), &ts, CX, CY, CX, CY, SCALE, TOUCH), Some(7));
    }

    #[test]
    fn a_point_far_away_picks_nothing() {
        let ts = [at_centre(7, 0.5)];
        assert_eq!(pick(&cam(), &ts, CX + 380.0, CY, CX, CY, SCALE, TOUCH), None);
    }

    /// The accessibility floor: a tiny target is still hittable.
    ///
    /// A 1cm object at scene distance projects to a couple of pixels. Without
    /// the floor it is unclickable with a mouse and impossible with a thumb —
    /// which is the whole reason picking happens after projection.
    #[test]
    fn a_tiny_distant_target_still_presents_a_touch_sized_hit_area() {
        let ts = [at_centre(1, 0.01)];
        let (_, _, r, _) = project_target(&cam(), &ts[0], CX, CY, SCALE, TOUCH).unwrap();
        assert!(r >= min_screen_radius(TOUCH), "projected radius {r} is under the floor");
        // 20 points off centre is inside a 44pt target and far outside the object.
        assert_eq!(pick(&cam(), &ts, CX + 20.0, CY, CX, CY, SCALE, TOUCH), Some(1));
    }

    /// A disabled target is not pickable AND does not shadow what is behind it.
    #[test]
    fn a_disabled_target_is_skipped_not_merely_ignored() {
        let mut front = at_centre(1, 0.5);
        front.enabled = false;
        // Co-located, so this cannot depend on the camera's orientation. My
        // first attempt offset the second target along world -z and it landed
        // BESIDE the first on screen rather than behind it — the default camera
        // has yaw 0.5, so a world axis is not a screen axis.
        let behind = at_centre(2, 0.5);
        let ts = [front, behind];
        assert_eq!(pick(&cam(), &ts, CX, CY, CX, CY, SCALE, TOUCH), Some(2),
                   "a disabled control that swallows the click reads as a broken one");
        assert_eq!(state_of(&ts[0], None, None, false), State::Disabled);
    }

    /// FOCUS IS NOT HOVER — the invariant that keeps a keyboard user in the UI.
    #[test]
    fn focus_is_visible_without_a_pointer_and_is_the_strongest_ring() {
        let target = at_centre(3, 0.5);
        // Focused with the pointer somewhere else entirely.
        assert_eq!(state_of(&target, None, Some(3), false), State::Focus);
        assert!(State::Focus.ring_width() > State::Hover.ring_width(),
                "a focus ring subtler than hover locks out everyone without a pointer");
        assert!(State::Focus.ring_width() > 0.0);
    }

    /// Press must be perceptible as a DIRECTION, not just a colour.
    #[test]
    fn pressed_sinks_and_hover_lifts() {
        assert!(State::Hover.lift() > 0.0);
        assert!(State::Pressed.lift() < 0.0, "a press with no downward travel has no physicality");
        assert_eq!(State::Focus.lift(), 0.0,
                   "moving the object on focus makes keyboard navigation lurch");
    }

    /// Every state must be distinguishable from every other by SOMETHING.
    ///
    /// Two states that differ only in a value nobody can see are one state with
    /// two names, and that is how "focused" and "hovered" end up identical.
    #[test]
    fn no_two_states_are_visually_identical() {
        let all = [State::Idle, State::Hover, State::Focus, State::Pressed, State::Disabled];
        for (i, a) in all.iter().enumerate() {
            for b in &all[i + 1..] {
                let sig = |s: &State| (s.ring_width().to_bits(), s.lift().to_bits(),
                                       s.tint().0.to_bits(), s.tint().1.to_bits());
                assert_ne!(sig(a), sig(b), "{a:?} and {b:?} render identically");
            }
        }
    }

    /// Disabled must desaturate, not only dim: in a lit scene, dimmer alone is
    /// indistinguishable from being in shadow.
    #[test]
    fn disabled_desaturates_rather_than_only_dimming() {
        let (bright, sat) = State::Disabled.tint();
        assert!(bright < 1.0 && sat < 0.5, "got brightness {bright}, saturation {sat}");
        assert!(!State::Disabled.is_interactive());
    }

    /// Where only padding overlaps, the nearest CENTRE wins.
    ///
    /// Both targets must be tiny for this to be reachable at all: a padded hit
    /// only happens when the floor exceeds the projected radius, so anything
    /// large enough to overlap on its own merits is a real hit instead.
    #[test]
    fn overlapping_padding_resolves_toward_the_nearer_centre() {
        let near = at_centre(1, 0.01);
        let far = t(2, Vec3::new(cam().target.x + 0.6, cam().target.y, cam().target.z), 0.01);
        let ts = [far, near];
        // Just off the first target's centre, inside both 22pt paddings.
        let got = pick(&cam(), &ts, CX + 4.0, CY, CX, CY, SCALE, TOUCH);
        assert_eq!(got, Some(1), "the pointer was nearly on top of target 1");
    }

    /// A real hit beats a padding-only hit even when it is further away.
    #[test]
    fn a_real_hit_beats_a_padded_one_behind_it() {
        let padded_near = at_centre(1, 0.01);
        let real_far = t(2, Vec3::new(cam().target.x + 0.25, cam().target.y, cam().target.z - 1.0), 0.6);
        let ts = [padded_near, real_far];
        // Land on the far object's actual projection, inside the near one's padding.
        let (sx, sy, _, _) = project_target(&cam(), &ts[1], CX, CY, SCALE, TOUCH).unwrap();
        assert_eq!(pick(&cam(), &ts, sx, sy, CX, CY, SCALE, TOUCH), Some(2),
                   "padding must not steal a click that landed on a real object");
    }
}
