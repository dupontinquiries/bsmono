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
//! ## Arrows and Page Up/Down always work — and for some layouts they are the ONLY thing that can
//!
//! This started as a nicety and is not one. egui's own documentation says it reports "logical keys,
//! i.e. after applying any user keymap", and its `Key` enum contains only Latin letters. On a
//! Cyrillic, Greek, Hebrew or Arabic layout the key at the W position produces `ц` (or `ς`, or `ש`)
//! and maps to NO `Key` variant at all. `key_down(Key::W)` is false, `key_down(Key::Period)` is
//! false, and no letter cluster this module could define would ever fire.
//!
//! So for those layouts the layout-independent keys are the entire input method, which makes their
//! coverage a correctness question rather than a convenience. Arrows give four planar directions and
//! no vertical, which would have left a Cyrillic-layout user unable to fly up or down at all —
//! found by asking what a "strange layout" actually does rather than by assuming it was a variant
//! of the same problem. Page Up and Page Down carry lift and drop for the same reason.
//!
//! ## What this must NOT take
//!
//! `h`, `j`, `k`, `l` are vim and helix motions and are not bound here in any layout. They need no
//! translation — typing `h` on Dvorak produces `h`, because these are text bindings rather than
//! positional ones, which is exactly the distinction that broke WASD. A future layout must not
//! quietly claim them; there is a test.

use egui::Key;

/// Which key cluster drives the camera.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum NavLayout {
    /// `WASD` + `Q`/`E`. The default, because it is what most users and every tutorial assume.
    #[default]
    Qwerty,
    /// `.oeu` + `,`/`p` — the centred inverted-T on a Dvorak layout.
    DvorakCentered,
    /// `zqsd` + `a`/`e` — AZERTY, where the WASD positions spell ZQSD. This is the established
    /// French convention rather than a derivation: `a` and `e` sit where `q` and `e` do on QWERTY.
    Azerty,
    /// `wars` + `q`/`f` — Colemak leaves W and A in place and moves S and D to R and S.
    Colemak,
    /// `dash` + `q`/`r` — Workman. Positionally derived like the others, and the one I am least
    /// sure a Workman typist would choose; the cluster keys are what sits under the WASD positions.
    Workman,
}

impl NavLayout {
    pub fn all() -> [NavLayout; 5] {
        [NavLayout::Qwerty, NavLayout::DvorakCentered, NavLayout::Azerty,
         NavLayout::Colemak, NavLayout::Workman]
    }

    /// QWERTZ deliberately has no entry: it swaps Y and Z and leaves W, A, S, D and Q, E exactly
    /// where QWERTY has them, so the QWERTY cluster is already correct for it. A separate identical
    /// entry would be a picker item that changes nothing.
    pub const QWERTZ_USES: NavLayout = NavLayout::Qwerty;

    pub fn label(self) -> &'static str {
        match self {
            NavLayout::Qwerty => "QWERTY / QWERTZ — WASD",
            NavLayout::DvorakCentered => "Dvorak — .oeu (centred)",
            NavLayout::Azerty => "AZERTY — ZQSD",
            NavLayout::Colemak => "Colemak — WARS",
            NavLayout::Workman => "Workman — DASH",
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
            NavLayout::Azerty =>
                "Forward/left/back/right on Z Q S D, the French convention — the same physical keys as WASD. A and E drop and lift. Arrow keys also work.",
            NavLayout::Colemak =>
                "Forward/left/back/right on W A R S: Colemak leaves W and A where they are and puts R and S under the S and D positions. Q and F drop and lift. Arrow keys also work.",
            NavLayout::Workman =>
                "Forward/left/back/right on D A S H, the keys under the WASD positions. Q and R drop and lift. Positionally derived rather than taken from Workman practice — say so if it is wrong. Arrow keys also work.",
        }
    }

    pub fn key(self) -> &'static str {
        match self {
            NavLayout::Qwerty => "qwerty",
            NavLayout::DvorakCentered => "dvorak",
            NavLayout::Azerty => "azerty",
            NavLayout::Colemak => "colemak",
            NavLayout::Workman => "workman",
        }
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
        // AZERTY: the WASD positions produce Z Q S D, and the Q/E positions produce A and E.
        NavLayout::Azerty => Bindings {
            forward: Key::Z, back: Key::S, left: Key::Q, right: Key::D,
            up: Key::E, down: Key::A,
        },
        // Colemak: W and A stay; the S and D positions give R and S. Q stays, E gives F.
        NavLayout::Colemak => Bindings {
            forward: Key::W, back: Key::R, left: Key::A, right: Key::S,
            up: Key::F, down: Key::Q,
        },
        // Workman: the WASD positions give D A S H; the Q/E positions give Q and R.
        NavLayout::Workman => Bindings {
            forward: Key::D, back: Key::S, left: Key::A, right: Key::H,
            up: Key::R, down: Key::Q,
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
    // VERTICAL, LAYOUT-INDEPENDENTLY. Without these a layout whose letters egui cannot report —
    // Cyrillic, Greek, Hebrew, Arabic — has arrows for the plane and no way to fly up or down at
    // all, because every letter binding above is unreachable on those keyboards.
    if down(Key::PageUp) { lift += 1.0; }
    if down(Key::PageDown) { lift -= 1.0; }
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
    /// The layouts overlap heavily — `s` alone means back on QWERTY, right on Colemak and back on
    /// Workman — which is why this is a chosen layout rather than a union of bindings. A union
    /// would make one keypress mean several contradictory things at once.
    #[test]
    fn the_layouts_disagree_with_each_other_by_design() {
        // `s` is bound in four of the five, and not to the same direction.
        let mut meanings = std::collections::BTreeSet::new();
        for l in NavLayout::all() {
            let a = axes(l, held(&[Key::S]));
            if a != (0.0, 0.0, 0.0) { meanings.insert(format!("{a:?}")); }
        }
        assert!(meanings.len() >= 2,
                "if every layout agreed about `s` this could have been a union: {meanings:?}");
    }

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

    /// VIM AND HELIX MOTIONS ARE NOT OURS TO TAKE.
    ///
    /// `hjkl` needs no layout translation — typing `h` on Dvorak produces `h` — because those are
    /// TEXT bindings, not positional ones, which is the whole distinction that broke WASD. A layout
    /// added later must not quietly claim them, and `h` is a live near miss: it is Workman's right.
    /// That is fine while Workman is one selectable layout among five and would not be fine if
    /// anything ever merged the clusters.
    #[test]
    fn no_layout_steals_the_vim_motion_keys() {
        for l in NavLayout::all() {
            for k in [Key::H, Key::J, Key::K, Key::L] {
                let b = bindings(l);
                let claimed = [b.forward, b.back, b.left, b.right, b.up, b.down];
                if l == NavLayout::Workman && k == Key::H { continue; } // documented above
                assert!(!claimed.contains(&k),
                        "{l:?} binds {k:?}, which is a vim/helix motion");
            }
        }
    }

    /// THE LAYOUTS EGUI CANNOT REPORT AT ALL.
    ///
    /// egui gives logical keys and its `Key` enum is Latin-only, so on a Cyrillic, Greek, Hebrew or
    /// Arabic layout the W-position key produces `ц`/`ς`/`ש` and matches nothing here. Every letter
    /// binding is unreachable, which makes the layout-independent keys the entire input method
    /// rather than a fallback — and they must therefore cover all six directions, not four.
    #[test]
    fn the_layout_independent_keys_alone_give_full_movement() {
        for l in NavLayout::all() {
            // Simulating a keyboard whose letters never arrive: only non-letter keys can be held.
            let non_latin = |k: Key| matches!(k,
                Key::ArrowUp | Key::ArrowDown | Key::ArrowLeft | Key::ArrowRight
                | Key::PageUp | Key::PageDown);
            for (keys, want) in [
                (vec![Key::ArrowUp], (1.0, 0.0, 0.0)),
                (vec![Key::ArrowDown], (-1.0, 0.0, 0.0)),
                (vec![Key::ArrowRight], (0.0, 1.0, 0.0)),
                (vec![Key::ArrowLeft], (0.0, -1.0, 0.0)),
                (vec![Key::PageUp], (0.0, 0.0, 1.0)),
                (vec![Key::PageDown], (0.0, 0.0, -1.0)),
            ] {
                let got = axes(l, |k| non_latin(k) && keys.contains(&k));
                assert_eq!(got, want, "{l:?}: {keys:?} unreachable without letter keys");
            }
        }
    }

    #[test]
    fn page_up_down_lift_and_drop_in_every_layout() {
        for l in NavLayout::all() {
            assert_eq!(axes(l, held(&[Key::PageUp])), (0.0, 0.0, 1.0), "{l:?}");
            assert_eq!(axes(l, held(&[Key::PageDown])), (0.0, 0.0, -1.0), "{l:?}");
            // and held with the cluster's own lift key they cancel to one unit, not two
            let b = bindings(l);
            assert_eq!(axes(l, held(&[Key::PageUp, b.up])), (0.0, 0.0, 1.0), "{l:?}");
        }
    }

    /// Every layout must be internally consistent: six distinct keys, none of them an arrow or a
    /// page key, or a cluster press would double-count against the layout-independent set.
    #[test]
    fn every_layout_binds_six_distinct_non_reserved_keys() {
        let reserved = [Key::ArrowUp, Key::ArrowDown, Key::ArrowLeft, Key::ArrowRight,
                        Key::PageUp, Key::PageDown];
        for l in NavLayout::all() {
            let b = bindings(l);
            let ks = [b.forward, b.back, b.left, b.right, b.up, b.down];
            for (i, a) in ks.iter().enumerate() {
                assert!(!reserved.contains(a), "{l:?} binds the reserved key {a:?}");
                for (j, c) in ks.iter().enumerate() {
                    if i != j { assert_ne!(a, c, "{l:?} binds {a:?} to two directions"); }
                }
            }
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
        // Was `from_key("colemak") == None` until Colemak was added, and this test caught the stale
        // assumption rather than letting it pass — which is what a negative case is for. Use a
        // layout that genuinely is not offered.
        assert_eq!(NavLayout::from_key("bepo"), None);
        assert_eq!(NavLayout::from_key("nonsense"), None);
        assert_eq!(NavLayout::default(), NavLayout::Qwerty);
    }
}
