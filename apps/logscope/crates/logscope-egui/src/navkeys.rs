//! navkeys — which keys fly the camera, by keyboard layout.
//!
//! ## Why this cannot be "also accept .oeu"
//!
//! egui reports LOGICAL keys, not physical scancodes: on a Dvorak layout the key at QWERTY's `W`
//! position arrives as `Comma`, not `W`. So a Dvorak typist gets no freecam movement at all today,
//! and the obvious fix — accept both sets — is wrong in a way that is easy to miss:
//!
//! QWERTY freecam uses `E` for UP. The Dvorak centred cluster `.oeu` uses `e` for BACK. Accepting
//! both simultaneously makes one physical keypress mean two contradictory things, and the person
//! most affected is exactly the person the change was for.
//!
//! So it is a LAYOUT, chosen once, not a superset of bindings.
//!
//! ## The mapping, and why these keys
//!
//! Dvorak's left hand is
//!
//! ```text
//!   '  ,  .  p  y
//!   a  o  e  u  i
//!   ;  q  j  k  x
//! ```
//!
//! `.oeu` is a true inverted-T: `.` sits directly above `e`, with `o` to its left and `u` to its
//! right — the same shape WASD makes on QWERTY, which is why Dvorak users pick it rather than
//! contorting to the physical WASD positions.
//!
//! Vertical flight follows the same principle. On QWERTY, `Q` and `E` are the top-row keys directly
//! above `A` and `D`. On Dvorak the keys directly above `o` and `u` are `,` and `p`. So down/up are
//! `,`/`p`: not an arbitrary pick, the same two physical positions relative to the cluster.
//!
//! ## Arrows always work
//!
//! In both layouts, and that is the accessibility half rather than the personalisation half. Arrow
//! keys are layout-independent, they are what a screen-reader user or someone on an unfamiliar
//! machine will try first, and they cost nothing to support because they collide with nothing.

use egui::Key;

/// Which key cluster drives the camera.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum NavLayout {
    /// `WASD` + `Q`/`E`. The default, because it is what most users and every tutorial assume.
    #[default]
    Qwerty,
    /// `.oeu` + `,`/`p` — the centred inverted-T on a Dvorak layout.
    DvorakCentered,
}

impl NavLayout {
    pub fn all() -> [NavLayout; 2] { [NavLayout::Qwerty, NavLayout::DvorakCentered] }

    pub fn label(self) -> &'static str {
        match self {
            NavLayout::Qwerty => "QWERTY — WASD",
            NavLayout::DvorakCentered => "Dvorak — .oeu (centred)",
        }
    }

    /// What the picker shows under the name, so the choice does not need explaining elsewhere.
    pub fn hint(self) -> &'static str {
        match self {
            NavLayout::Qwerty => "Forward/left/back/right on WASD, Q and E drop and lift. Arrow keys also work.",
            NavLayout::DvorakCentered =>
                "Forward/left/back/right on . o e u — the same inverted-T shape WASD makes, in \
Dvorak's home position. Comma and P drop and lift, matching where Q and E sit relative to WASD. \
Arrow keys also work.",
        }
    }

    pub fn key(self) -> &'static str {
        match self { NavLayout::Qwerty => "qwerty", NavLayout::DvorakCentered => "dvorak" }
    }
    pub fn from_key(s: &str) -> Option<NavLayout> {
        NavLayout::all().into_iter().find(|l| l.key().eq_ignore_ascii_case(s.trim()))
    }
}

/// The six directions a freecam moves in.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Bindings {
    pub forward: Key,
    pub back: Key,
    pub left: Key,
    pub right: Key,
    pub up: Key,
    pub down: Key,
}

pub fn bindings(layout: NavLayout) -> Bindings {
    match layout {
        NavLayout::Qwerty => Bindings {
            forward: Key::W, back: Key::S, left: Key::A, right: Key::D,
            up: Key::E, down: Key::Q,
        },
        NavLayout::DvorakCentered => Bindings {
            forward: Key::Period, back: Key::E, left: Key::O, right: Key::U,
            // `,` and `p` are directly above `o` and `u`, exactly as Q and E sit above A and D.
            up: Key::P, down: Key::Comma,
        },
    }
}

/// Movement as three axes: (forward+/back−, right+/left−, up+/down−).
///
/// Takes a predicate rather than egui's `InputState` so the mapping is testable without a UI — the
/// same seam the rest of this crate uses for anything that would otherwise need a frame.
pub fn axes(layout: NavLayout, down: impl Fn(Key) -> bool) -> (f32, f32, f32) {
    let b = bindings(layout);
    let mut fwd = 0.0f32;
    let mut strafe = 0.0f32;
    let mut lift = 0.0f32;
    // Layout cluster.
    if down(b.forward) { fwd += 1.0; }
    if down(b.back) { fwd -= 1.0; }
    if down(b.right) { strafe += 1.0; }
    if down(b.left) { strafe -= 1.0; }
    if down(b.up) { lift += 1.0; }
    if down(b.down) { lift -= 1.0; }
    // ARROWS, ALWAYS, IN EVERY LAYOUT. Layout-independent and collision-free, so there is no reason
    // to make someone configure anything before they can move.
    if down(Key::ArrowUp) { fwd += 1.0; }
    if down(Key::ArrowDown) { fwd -= 1.0; }
    if down(Key::ArrowRight) { strafe += 1.0; }
    if down(Key::ArrowLeft) { strafe -= 1.0; }
    // Opposite keys held together cancel rather than accumulating, so a stuck key cannot make the
    // camera drift at double speed.
    (fwd.clamp(-1.0, 1.0), strafe.clamp(-1.0, 1.0), lift.clamp(-1.0, 1.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn held(keys: &[Key]) -> impl Fn(Key) -> bool + '_ { move |k| keys.contains(&k) }

    #[test]
    fn qwerty_is_unchanged() {
        let l = NavLayout::Qwerty;
        assert_eq!(axes(l, held(&[Key::W])), (1.0, 0.0, 0.0));
        assert_eq!(axes(l, held(&[Key::S])), (-1.0, 0.0, 0.0));
        assert_eq!(axes(l, held(&[Key::D])), (0.0, 1.0, 0.0));
        assert_eq!(axes(l, held(&[Key::A])), (0.0, -1.0, 0.0));
        assert_eq!(axes(l, held(&[Key::E])), (0.0, 0.0, 1.0));
        assert_eq!(axes(l, held(&[Key::Q])), (0.0, 0.0, -1.0));
    }

    #[test]
    fn dvorak_centred_maps_the_same_inverted_t() {
        let l = NavLayout::DvorakCentered;
        assert_eq!(axes(l, held(&[Key::Period])), (1.0, 0.0, 0.0), ". is forward");
        assert_eq!(axes(l, held(&[Key::E])), (-1.0, 0.0, 0.0), "e is back");
        assert_eq!(axes(l, held(&[Key::U])), (0.0, 1.0, 0.0), "u is right");
        assert_eq!(axes(l, held(&[Key::O])), (0.0, -1.0, 0.0), "o is left");
        assert_eq!(axes(l, held(&[Key::P])), (0.0, 0.0, 1.0), "p lifts, as e does on qwerty");
        assert_eq!(axes(l, held(&[Key::Comma])), (0.0, 0.0, -1.0), ", drops, as q does on qwerty");
    }

    /// THE COLLISION THAT MAKES THIS A LAYOUT RATHER THAN EXTRA KEYS. `E` means UP on QWERTY and
    /// BACK on Dvorak-centred. Accepting both sets at once would make one keypress do two
    /// contradictory things — to the exact user the feature is for.
    #[test]
    fn e_means_different_things_in_the_two_layouts() {
        assert_eq!(axes(NavLayout::Qwerty, held(&[Key::E])), (0.0, 0.0, 1.0));
        assert_eq!(axes(NavLayout::DvorakCentered, held(&[Key::E])), (-1.0, 0.0, 0.0));
        // and the two clusters really are disjoint apart from that deliberate overlap
        let q = bindings(NavLayout::Qwerty);
        let d = bindings(NavLayout::DvorakCentered);
        let qs = [q.forward, q.back, q.left, q.right, q.up, q.down];
        let ds = [d.forward, d.back, d.left, d.right, d.up, d.down];
        let shared: Vec<Key> = qs.iter().copied().filter(|k| ds.contains(k)).collect();
        assert_eq!(shared, vec![Key::E], "only E overlaps, and it means opposite things: {shared:?}");
    }

    #[test]
    fn arrows_work_in_every_layout() {
        for l in NavLayout::all() {
            assert_eq!(axes(l, held(&[Key::ArrowUp])), (1.0, 0.0, 0.0), "{l:?}");
            assert_eq!(axes(l, held(&[Key::ArrowDown])), (-1.0, 0.0, 0.0), "{l:?}");
            assert_eq!(axes(l, held(&[Key::ArrowRight])), (0.0, 1.0, 0.0), "{l:?}");
            assert_eq!(axes(l, held(&[Key::ArrowLeft])), (0.0, -1.0, 0.0), "{l:?}");
        }
    }

    #[test]
    fn a_binding_and_an_arrow_together_do_not_double_the_speed() {
        // Someone using both hands should not fly twice as fast by accident.
        assert_eq!(axes(NavLayout::Qwerty, held(&[Key::W, Key::ArrowUp])), (1.0, 0.0, 0.0));
        assert_eq!(axes(NavLayout::DvorakCentered, held(&[Key::Period, Key::ArrowUp])), (1.0, 0.0, 0.0));
    }

    #[test]
    fn opposite_keys_cancel() {
        assert_eq!(axes(NavLayout::Qwerty, held(&[Key::W, Key::S])), (0.0, 0.0, 0.0));
        assert_eq!(axes(NavLayout::DvorakCentered, held(&[Key::O, Key::U])), (0.0, 0.0, 0.0));
        assert_eq!(axes(NavLayout::Qwerty, held(&[Key::Q, Key::E])), (0.0, 0.0, 0.0));
    }

    #[test]
    fn diagonals_combine() {
        let (f, s, _) = axes(NavLayout::DvorakCentered, held(&[Key::Period, Key::U]));
        assert_eq!((f, s), (1.0, 1.0), "forward-right should be both, not one");
    }

    #[test]
    fn nothing_held_is_no_movement() {
        for l in NavLayout::all() { assert_eq!(axes(l, held(&[])), (0.0, 0.0, 0.0)); }
    }

    #[test]
    fn layout_keys_round_trip() {
        for l in NavLayout::all() {
            assert_eq!(NavLayout::from_key(l.key()), Some(l));
            assert_eq!(NavLayout::from_key(&l.key().to_uppercase()), Some(l));
            assert!(l.hint().contains("Arrow keys also work"), "{l:?}: the fallback must be stated");
        }
        assert_eq!(NavLayout::from_key("colemak"), None);
        assert_eq!(NavLayout::default(), NavLayout::Qwerty);
    }
}
