//! Keeping graph labels readable when nodes crowd.
//!
//! From a screenshot of the running app: in the chat graph the turn labels
//! ("14t: create a rust crate th", "221t: rust use std::colle", …) overlap each
//! other badly enough that several are unreadable, because each is drawn at a
//! fixed offset from its node and the nodes cluster. The reference design for
//! the same panel keeps every card legible, and the difference is not styling —
//! it is that nothing currently checks whether two labels occupy the same
//! space.
//!
//! Pure geometry, so it is testable without a GPU and independent of whichever
//! renderer draws the graph.
//!
//! ## The approach, and what it deliberately is not
//!
//! Not force-directed relaxation: that moves labels every frame as the layout
//! settles, and text that drifts while you are reading it is worse than text
//! that overlaps. This is deterministic — a fixed ladder of candidate offsets
//! per node, first non-colliding one wins, and identical input gives identical
//! output. A label that cannot be placed is DROPPED rather than stacked, and
//! the caller is told how many, because a graph with three readable labels and
//! an honest "+7 more" beats ten illegible ones.

/// A rectangle in screen space. Deliberately not `egui::Rect` so this module
/// stays testable and reusable outside a paint pass.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Box2 {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Box2 {
    pub fn overlaps(&self, o: &Box2) -> bool {
        // Touching edges do not count: two labels flush against each other are
        // readable, and treating that as a collision throws away placements
        // that are fine.
        self.x < o.x + o.w && o.x < self.x + self.w && self.y < o.y + o.h && o.y < self.y + self.h
    }
    /// Grown by `pad` on every side — the gutter that keeps adjacent labels
    /// from reading as one run of text.
    pub fn padded(&self, pad: f32) -> Box2 {
        Box2 { x: self.x - pad, y: self.y - pad, w: self.w + 2.0 * pad, h: self.h + 2.0 * pad }
    }
}

/// Where a label may sit relative to its node, in preference order.
///
/// Right-of-node first because that is where a left-to-right reader expects it
/// and where the current renderer already puts it; the rest fan out so a dense
/// cluster spreads instead of piling.
const OFFSETS: [(f32, f32); 8] = [
    (10.0, -6.0),   // right, slightly above centre
    (10.0, 8.0),    // right, below
    (10.0, -20.0),  // right, well above
    (-10.0, -6.0),  // left (label is right-aligned by the caller)
    (-10.0, 8.0),
    (0.0, -22.0),   // straight above
    (0.0, 16.0),    // straight below
    (10.0, 22.0),
];

/// One label the caller wants drawn.
#[derive(Debug, Clone, Copy)]
pub struct Want {
    /// Index back into the caller's own node list.
    pub id: usize,
    /// Node centre in screen space.
    pub x: f32,
    pub y: f32,
    /// Measured text size.
    pub w: f32,
    pub h: f32,
    /// Higher wins a contested spot. The caller should pass something
    /// meaningful — recency, or "is the selected node" — because when labels
    /// compete this decides which one the user gets to read.
    pub priority: f32,
}

/// A placement decision.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Placed {
    pub id: usize,
    pub at: Box2,
    /// True when the label sits left of its node, so the caller right-aligns it.
    pub flipped: bool,
}

/// Place what fits; report what did not.
///
/// `bounds` clips to the visible panel — a label placed outside it is as lost
/// as one that was dropped, and pretending otherwise inflates the count the
/// caller shows the user.
pub fn place(wants: &[Want], bounds: Box2, pad: f32) -> (Vec<Placed>, usize) {
    // Highest priority first, stable on id so the result is deterministic
    // frame to frame. Non-determinism here would make labels flicker between
    // positions on a graph that has not changed.
    let mut order: Vec<&Want> = wants.iter().collect();
    order.sort_by(|a, b| {
        b.priority
            .partial_cmp(&a.priority)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.id.cmp(&b.id))
    });

    let mut taken: Vec<Box2> = Vec::with_capacity(order.len());
    let mut out: Vec<Placed> = Vec::with_capacity(order.len());
    let mut dropped = 0usize;

    for w in order {
        let mut placed = false;
        for (dx, dy) in OFFSETS {
            let flipped = dx < 0.0;
            let x = if flipped { w.x + dx - w.w } else { w.x + dx };
            let b = Box2 { x, y: w.y + dy - w.h * 0.5, w: w.w, h: w.h };
            if !inside(&b, &bounds) {
                continue;
            }
            let p = b.padded(pad);
            if taken.iter().any(|t| t.overlaps(&p)) {
                continue;
            }
            taken.push(p);
            out.push(Placed { id: w.id, at: b, flipped });
            placed = true;
            break;
        }
        if !placed {
            dropped += 1;
        }
    }
    // Back into caller order so the result is easy to zip against the input.
    out.sort_by_key(|p| p.id);
    (out, dropped)
}

fn inside(b: &Box2, bounds: &Box2) -> bool {
    b.x >= bounds.x
        && b.y >= bounds.y
        && b.x + b.w <= bounds.x + bounds.w
        && b.y + b.h <= bounds.y + bounds.h
}

#[cfg(test)]
mod tests {
    use super::*;

    fn w(id: usize, x: f32, y: f32) -> Want {
        Want { id, x, y, w: 80.0, h: 12.0, priority: 0.0 }
    }
    const BOUNDS: Box2 = Box2 { x: 0.0, y: 0.0, w: 800.0, h: 600.0 };

    #[test]
    fn labels_that_would_overlap_are_moved_not_stacked() {
        // Three nodes within a few pixels — the screenshot case.
        let ws = [w(0, 100.0, 100.0), w(1, 104.0, 102.0), w(2, 108.0, 104.0)];
        let (placed, dropped) = place(&ws, BOUNDS, 2.0);
        assert_eq!(dropped, 0, "all three should fit somewhere");
        for i in 0..placed.len() {
            for j in (i + 1)..placed.len() {
                assert!(!placed[i].at.overlaps(&placed[j].at),
                    "{:?} overlaps {:?}", placed[i], placed[j]);
            }
        }
    }

    #[test]
    fn an_unplaceable_label_is_dropped_and_counted_not_drawn_anyway() {
        // Ten labels on top of each other: eight candidate offsets, so at least
        // two must be reported rather than piled. An honest "+2 more" beats ten
        // illegible ones.
        let ws: Vec<Want> = (0..10).map(|i| w(i, 200.0, 200.0)).collect();
        let (placed, dropped) = place(&ws, BOUNDS, 2.0);
        assert_eq!(placed.len() + dropped, 10, "every label is accounted for");
        assert!(dropped >= 2, "got {dropped}");
    }

    #[test]
    fn priority_decides_who_gets_the_readable_spot() {
        // When labels compete, the caller's ranking must win — otherwise the
        // selected node's label is the one that disappears.
        let mut a = w(0, 200.0, 200.0);
        let mut b = w(1, 200.0, 200.0);
        a.priority = 0.0;
        b.priority = 9.0;
        let (placed, _) = place(&[a, b], BOUNDS, 2.0);
        let hi = placed.iter().find(|p| p.id == 1).unwrap();
        let lo = placed.iter().find(|p| p.id == 0).unwrap();
        // The higher-priority label gets the first (preferred) offset.
        assert!(hi.at.x > 200.0 && (hi.at.y - (200.0 - 6.0 - 6.0)).abs() < 0.01,
            "{hi:?} should hold the preferred slot");
        assert_ne!(hi.at, lo.at);
    }

    #[test]
    fn a_label_that_would_leave_the_panel_is_not_placed_there() {
        // Off-screen is as lost as dropped; counting it as placed would make
        // the "+n more" the user sees wrong.
        let ws = [w(0, 795.0, 300.0)];
        let (placed, _) = place(&ws, BOUNDS, 2.0);
        for p in &placed {
            assert!(p.at.x >= 0.0 && p.at.x + p.at.w <= 800.0, "{p:?} escapes the panel");
        }
    }

    #[test]
    fn a_left_placement_is_reported_so_the_caller_can_right_align() {
        // A label flipped to the left of its node must be drawn right-aligned or
        // it runs back over the node it belongs to.
        let ws: Vec<Want> = (0..4).map(|i| w(i, 400.0, 300.0)).collect();
        let (placed, _) = place(&ws, BOUNDS, 2.0);
        assert!(placed.iter().any(|p| p.flipped), "a crowded cluster should use the left side");
        for p in placed.iter().filter(|p| p.flipped) {
            assert!(p.at.x + p.at.w <= 400.0, "a flipped label must end left of the node: {p:?}");
        }
    }

    #[test]
    fn the_same_input_places_identically_every_time() {
        // Labels that move between frames on an unchanged graph are worse than
        // labels that overlap — you cannot read text that is sliding.
        let ws: Vec<Want> = (0..6).map(|i| w(i, 300.0 + i as f32, 300.0)).collect();
        let a = place(&ws, BOUNDS, 2.0);
        let b = place(&ws, BOUNDS, 2.0);
        assert_eq!(a.0, b.0);
        assert_eq!(a.1, b.1);
    }

    #[test]
    fn touching_edges_do_not_count_as_a_collision() {
        let a = Box2 { x: 0.0, y: 0.0, w: 10.0, h: 10.0 };
        let b = Box2 { x: 10.0, y: 0.0, w: 10.0, h: 10.0 };
        assert!(!a.overlaps(&b));
    }
}
