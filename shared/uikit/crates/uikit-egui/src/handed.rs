//! handed — put the controls under the thumb that is actually holding it.
//!
//! `settings_kit::InputPrefs::handedness` has existed, with a settings
//! screen to set it and a double-tilt gesture to flip it, and NOTHING in
//! any app read it. A left-handed user could set the preference and watch
//! every control stay on the right. This is the missing half: the
//! preference turned into layout decisions the widgets can act on.
//!
//! Three decisions cover nearly every case:
//!
//! * [`side`] — which edge a primary control belongs on.
//! * [`row_layout`] — the `egui::Layout` for a row of controls, so they
//!   pack from the dominant side and the important one lands under the
//!   thumb.
//! * [`mirror_x`] — for anything painted by hand rather than laid out.
//!
//! The rule for CENTRED things is that they stay centred: a shutter button
//! is centred because it is the one control a thumb finds without looking,
//! and mirroring it would be worse for both hands. [`primary_offset`] is
//! how far a centred control leans, which is a nudge and not a move.

use settings_kit::Handedness;

/// Which edge a control sits on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Left,
    Right,
}

impl Side {
    pub fn is_left(self) -> bool { matches!(self, Side::Left) }
}

/// The edge a PRIMARY control belongs on: the dominant hand's side, because
/// that is the thumb that reaches without the grip shifting.
pub fn side(h: Handedness) -> Side {
    match h {
        Handedness::Right => Side::Right,
        Handedness::Left => Side::Left,
    }
}

/// The edge for SECONDARY controls — the opposite side, so a mis-reach hits
/// nothing destructive. Cancel goes here; send goes on [`side`].
pub fn secondary_side(h: Handedness) -> Side {
    match side(h) {
        Side::Right => Side::Left,
        Side::Left => Side::Right,
    }
}

/// The layout for a row of controls: items pack from the dominant edge, so
/// the FIRST one added is the one under the thumb.
pub fn row_layout(h: Handedness) -> egui::Layout {
    match side(h) {
        Side::Right => egui::Layout::right_to_left(egui::Align::Center),
        Side::Left => egui::Layout::left_to_right(egui::Align::Center),
    }
}

/// Mirror an x coordinate within `width` when the user is left-handed. For
/// hand-painted chrome, where there is no layout to flip.
pub fn mirror_x(h: Handedness, x: f32, width: f32) -> f32 {
    match h {
        Handedness::Right => x,
        Handedness::Left => width - x,
    }
}

/// How far a CENTRED primary control leans toward the dominant side, in
/// points. A lean, not a move: the shutter stays findable without looking
/// for either hand, which is the whole reason it is centred.
pub const LEAN: f32 = 18.0;

pub fn primary_offset(h: Handedness) -> f32 {
    match side(h) {
        Side::Right => LEAN,
        Side::Left => -LEAN,
    }
}

/// Centre `item_w` within `avail_w`, leaning by [`LEAN`], and clamped so the
/// control never leaves the container however narrow it gets. Returns the
/// left edge.
///
/// This is the function that fixes "the buttons are not centred": laying a
/// control out with a hand-written `add_space` puts it wherever the spacer
/// was measured for, and it stops being centred the moment the width
/// changes. Deriving the offset from the actual widths cannot drift.
pub fn centred_x(avail_w: f32, item_w: f32, h: Handedness) -> f32 {
    let centre = (avail_w - item_w) * 0.5;
    let leaned = centre + primary_offset(h);
    leaned.clamp(0.0, (avail_w - item_w).max(0.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_primary_control_follows_the_dominant_hand_and_the_secondary_opposes_it() {
        assert_eq!(side(Handedness::Right), Side::Right);
        assert_eq!(side(Handedness::Left), Side::Left);
        assert_eq!(secondary_side(Handedness::Right), Side::Left);
        assert_eq!(secondary_side(Handedness::Left), Side::Right);
        // A destructive control must never share a side with the primary one.
        for h in [Handedness::Right, Handedness::Left] {
            assert_ne!(side(h), secondary_side(h));
        }
    }

    #[test]
    fn a_centred_control_is_centred_at_every_width_and_leans_the_right_way() {
        for avail in [200.0_f32, 360.0, 400.0, 1280.0] {
            let w = 72.0;
            let r = centred_x(avail, w, Handedness::Right);
            let l = centred_x(avail, w, Handedness::Left);
            let centre = (avail - w) * 0.5;
            assert!(r > centre && l < centre, "leans toward the hand at {avail}");
            // Symmetric about the centre.
            assert!(((r - centre) + (l - centre)).abs() < 1e-3, "{r} {l} {centre}");
            // Always inside.
            for x in [r, l] {
                assert!(x >= 0.0 && x + w <= avail + 1e-3, "{x} escapes {avail}");
            }
        }
    }

    #[test]
    fn a_control_wider_than_its_container_is_pinned_rather_than_pushed_out() {
        // The case a hand-written `add_space` gets wrong: at 100pt of room a
        // 72pt button plus an 18pt lean would sit at 32, which is fine — but
        // a 96pt button would be pushed past the edge without the clamp.
        assert_eq!(centred_x(100.0, 96.0, Handedness::Right), 4.0);
        assert_eq!(centred_x(50.0, 96.0, Handedness::Right), 0.0, "never negative");
        assert_eq!(centred_x(50.0, 96.0, Handedness::Left), 0.0);
    }

    #[test]
    fn mirroring_is_an_involution_and_a_no_op_for_the_right_hand() {
        let w = 400.0;
        assert_eq!(mirror_x(Handedness::Right, 30.0, w), 30.0);
        assert_eq!(mirror_x(Handedness::Left, 30.0, w), 370.0);
        // Mirroring twice returns the original, whichever hand.
        for h in [Handedness::Right, Handedness::Left] {
            let once = mirror_x(h, 123.0, w);
            assert!((mirror_x(h, once, w) - 123.0).abs() < 1e-4);
        }
    }

    #[test]
    fn the_row_layout_packs_from_the_hand_that_holds_the_phone() {
        // right_to_left means the first item added is rightmost — under a
        // right thumb. The assertion is on the direction, since that is the
        // thing a future edit could invert without any test noticing.
        assert!(row_layout(Handedness::Right).main_dir == egui::Direction::RightToLeft);
        assert!(row_layout(Handedness::Left).main_dir == egui::Direction::LeftToRight);
    }
}
