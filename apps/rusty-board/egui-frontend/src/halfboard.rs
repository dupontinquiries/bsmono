//! halfboard — the keyboard compacted to one thumb's reach.
//!
//! `docs/single-hand-mode.md` calls this the signature ergonomic feature: on a phone over
//! six inches a thumb reaches a fan-shaped arc, not a rectangle, and a full-width board puts
//! the far corner outside it. The port has none of it.
//!
//! ## What this is, and what it is not
//!
//! The doc's full subsystem is a reach POLYGON in normalised coordinates, with keys
//! compacted into the arc. That needs per-key placement the renderer does not have — it lays
//! rows out proportionally across a rect — so this is the part that fits the renderer it has:
//! **anchor the board to a corner and narrow it**. Every key keeps its relative position and
//! the whole board comes within reach, which is the property that matters; the arc's curved
//! edge is a refinement on top, not the mechanism.
//!
//! Naming it `HalfBoard` rather than `ThumbArc` so nobody reads it as the doc's polygon
//! already being here.
//!
//! ## The freed strip is not empty space
//!
//! Compacting to the left leaves a column on the right. The doc gives that region a meaning
//! — "multi-anchor space", a tap zone for space — and a strip that swallows taps silently
//! would be worse than one that does nothing, because the keyboard would look like it had
//! stopped responding along one edge. [`Side::free_zone`] returns it so a caller can give it
//! a job, and [`Side::is_in_free_zone`] answers for a point.
//!
//! ## Why a fraction and not a polygon
//!
//! `WIDTH` is 0.78 of the board. Wider and the far keys are still a stretch; narrower and the
//! keys get small enough that the error rate replaces the reach problem with a different one.
//! It is a starting point that can be tuned against a real thumb, not a measured constant,
//! and it is called out here so the number is not read as more than it is.

use egui::Rect;

/// Which side the board is anchored to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Side {
    /// Full width. The default: a keyboard that starts compacted is one that has decided
    /// which hand you hold the phone in.
    #[default]
    Off,
    /// Anchored left, for a left thumb.
    Left,
    /// Anchored right, for a right thumb.
    Right,
}

/// Fraction of the board's width a compacted board occupies.
pub const WIDTH: f32 = 0.78;

/// Below this the compacted board is too narrow to type on, and compaction is refused —
/// on a small phone in landscape the full board is already within reach.
pub const MIN_PT: f32 = 240.0;

impl Side {
    pub fn is_on(self) -> bool {
        self != Side::Off
    }

    /// Cycle Off → Left → Right → Off, which is what one button can drive.
    pub fn next(self) -> Side {
        match self {
            Side::Off => Side::Left,
            Side::Left => Side::Right,
            Side::Right => Side::Off,
        }
    }

    /// The rect the keys are laid out in.
    ///
    /// Returns `board` unchanged when compaction is off — or when the result would be too
    /// narrow to type on, because a board that becomes unusable in the name of reach has
    /// solved the wrong problem.
    pub fn board_rect(self, board: Rect) -> Rect {
        if !self.is_on() {
            return board;
        }
        let w = board.width() * WIDTH;
        if w < MIN_PT {
            return board;
        }
        match self {
            Side::Left => Rect::from_min_size(board.min, egui::vec2(w, board.height())),
            Side::Right => Rect::from_min_size(
                egui::pos2(board.max.x - w, board.min.y),
                egui::vec2(w, board.height()),
            ),
            Side::Off => board,
        }
    }

    /// The strip the compaction freed, if any. `None` when nothing was freed — including
    /// when compaction was refused for being too narrow, so a caller cannot end up drawing
    /// a zone beside a board that is still full width.
    pub fn free_zone(self, board: Rect) -> Option<Rect> {
        let keys = self.board_rect(board);
        if keys == board {
            return None;
        }
        Some(match self {
            Side::Left => Rect::from_min_max(egui::pos2(keys.max.x, board.min.y), board.max),
            Side::Right => Rect::from_min_max(board.min, egui::pos2(keys.min.x, board.max.y)),
            Side::Off => return None,
        })
    }

    /// Is this point in the freed strip?
    pub fn is_in_free_zone(self, board: Rect, p: egui::Pos2) -> bool {
        self.free_zone(board).is_some_and(|z| z.contains(p))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn board() -> Rect {
        Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(400.0, 220.0))
    }

    /// The whole point: the far edge comes within reach, and every key keeps its relative
    /// position.
    #[test]
    fn compacting_anchors_the_board_to_the_chosen_side() {
        let b = board();
        let l = Side::Left.board_rect(b);
        assert_eq!(l.min.x, b.min.x, "a left-anchored board must touch the left edge");
        assert!(l.max.x < b.max.x, "nothing was freed");
        assert_eq!(l.height(), b.height(), "compaction is horizontal only");

        let r = Side::Right.board_rect(b);
        assert_eq!(r.max.x, b.max.x);
        assert!(r.min.x > b.min.x);
        assert!((l.width() - r.width()).abs() < 0.01, "the two sides must be the same size");

        // Off changes nothing at all.
        assert_eq!(Side::Off.board_rect(b), b);
        assert_eq!(Side::Off.free_zone(b), None);
    }

    /// The freed strip has a meaning in the doc — a tap zone for space — so it is returned
    /// rather than left as a gap. A strip that silently swallowed taps would look like the
    /// keyboard had stopped responding along one edge.
    #[test]
    fn the_freed_strip_is_on_the_other_side_and_covers_what_the_keys_gave_up() {
        let b = board();
        for side in [Side::Left, Side::Right] {
            let keys = side.board_rect(b);
            let zone = side.free_zone(b).expect("something was freed");
            assert!((keys.width() + zone.width() - b.width()).abs() < 0.01, "{side:?}: a gap or an overlap");
            assert!(!keys.intersects(zone.shrink(0.5)), "{side:?}: the zone overlaps the keys");
            assert_eq!(zone.height(), b.height());
        }
        // On the correct side.
        assert!(Side::Left.free_zone(b).unwrap().min.x > b.center().x, "left-anchored frees the RIGHT");
        assert!(Side::Right.free_zone(b).unwrap().max.x < b.center().x);

        // And a point lands where you would expect.
        assert!(Side::Left.is_in_free_zone(b, egui::pos2(395.0, 100.0)));
        assert!(!Side::Left.is_in_free_zone(b, egui::pos2(10.0, 100.0)));
        assert!(!Side::Off.is_in_free_zone(b, egui::pos2(395.0, 100.0)), "nothing is freed when off");
    }

    /// A board that becomes unusable in the name of reach has solved the wrong problem. On a
    /// small screen the full board is already within reach.
    #[test]
    fn compaction_is_refused_when_it_would_leave_too_little_to_type_on() {
        let small = Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(280.0, 200.0));
        assert!(small.width() * WIDTH < MIN_PT, "the premise: {}", small.width() * WIDTH);
        assert_eq!(Side::Left.board_rect(small), small, "it compacted a board that cannot take it");
        assert_eq!(
            Side::Left.free_zone(small),
            None,
            "a zone was offered beside a board that is still full width"
        );

        // Just above the floor it does compact.
        let big = Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(MIN_PT / WIDTH + 1.0, 200.0));
        assert_ne!(Side::Left.board_rect(big), big);
    }

    /// One button drives it, so the cycle has to come back around.
    #[test]
    fn the_side_cycles_through_off() {
        let mut s = Side::default();
        assert_eq!(s, Side::Off, "a keyboard that starts compacted has picked a hand for you");
        s = s.next();
        assert_eq!(s, Side::Left);
        s = s.next();
        assert_eq!(s, Side::Right);
        s = s.next();
        assert_eq!(s, Side::Off, "no way back to full width");
        assert!(!Side::Off.is_on() && Side::Left.is_on() && Side::Right.is_on());
    }
}
