//! Letter-region boundary and path segmentation for springboard space.
//!
//! See `docs/single-hand-mode.md` for the design intent. tl;dr: the
//! letter region's outer perimeter is the spacebar. Crossing it during
//! a swipe segments the swipe path into multiple words separated by
//! spaces.
//!
//! This module's two main contributions:
//!
//!   1. `LetterBoundary` — describes where the letter region's edges
//!      are, with semantic tags for each edge (Top / Bottom / Left /
//!      Right). Built from a layout's rendered hitboxes.
//!
//!   2. `segment_path` — takes a swipe path and a boundary, returns
//!      a list of (segment, end-edge) tuples where each segment is
//!      a portion of the path between boundary crossings.
//!
//! Edges are semantic, not just geometric. The `Top` edge has special
//! meaning (digit gesture / minimize gesture) and is intentionally
//! NOT treated as a springboard-space delimiter. The `Bottom`, `Left`,
//! and `Right` edges all produce spaces when crossed.

use crate::gestures::swipe::{KeyHitbox, SwipePoint};

/// Which edge of the letter region a crossing happened on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BoundaryEdge {
    /// Above the top row. Reserved for digit / minimize gestures.
    /// NOT a springboard-space delimiter.
    Top,
    /// Below the bottom row. The conventional spacebar.
    Bottom,
    /// Left of the leftmost key in any row.
    LeftSide,
    /// Right of the rightmost key in any row.
    RightSide,
}

impl BoundaryEdge {
    /// True when crossing this edge during a swipe should be
    /// interpreted as a word-delimiter space.
    pub fn is_springboard_space(self) -> bool {
        !matches!(self, BoundaryEdge::Top)
    }
}

/// The outer perimeter of the letter-key region, with semantic edge
/// tags. For now, we model this as the simple bounding box of all
/// letter-class hitboxes; future compaction work will extend this to
/// a polygon when single-hand mode is active.
#[derive(Debug, Clone)]
pub struct LetterBoundary {
    /// Bounds inclusive of all letter-class keys.
    pub left: f64,
    pub right: f64,
    pub top: f64,
    pub bottom: f64,
}

impl LetterBoundary {
    /// Build a boundary from a slice of hitboxes. We deliberately
    /// include ALL hitboxes (not just `KeyAction::Character` ones)
    /// because for non-character keys like Backspace, Return, etc.,
    /// the user can still legitimately swipe through them — the
    /// "letter region" is the whole visible keyboard, not just
    /// alphabetic keys.
    ///
    /// If `boxes` is empty, returns None.
    pub fn from_hitboxes(boxes: &[KeyHitbox]) -> Option<Self> {
        if boxes.is_empty() {
            return None;
        }
        let mut left = f64::MAX;
        let mut right = f64::MIN;
        let mut top = f64::MAX;
        let mut bottom = f64::MIN;
        for b in boxes {
            left = left.min(b.x);
            right = right.max(b.x + b.width);
            top = top.min(b.y);
            bottom = bottom.max(b.y + b.height);
        }
        Some(Self {
            left,
            right,
            top,
            bottom,
        })
    }

    /// True if the point is inside the letter region.
    pub fn contains(&self, x: f64, y: f64) -> bool {
        x >= self.left && x <= self.right && y >= self.top && y <= self.bottom
    }

    /// Given a point outside the region, classify which edge zone it's
    /// in. Returns None if the point is actually inside.
    pub fn classify_outside(&self, x: f64, y: f64) -> Option<BoundaryEdge> {
        if self.contains(x, y) {
            return None;
        }
        // Decide by which side(s) the point lies on. For corner
        // overlaps, prefer the vertical edges (Top/Bottom) over the
        // horizontal ones because the user's intent in corner regions
        // is usually about going off the top (minimize) or bottom
        // (space) rather than the sides.
        if y < self.top {
            Some(BoundaryEdge::Top)
        } else if y > self.bottom {
            Some(BoundaryEdge::Bottom)
        } else if x < self.left {
            Some(BoundaryEdge::LeftSide)
        } else {
            Some(BoundaryEdge::RightSide)
        }
    }
}

/// A segment of a swipe path, between two boundary crossings (or
/// between the start of the path and the first crossing, or between
/// the last crossing and the end of the path).
#[derive(Debug, Clone)]
pub struct PathSegment {
    /// Slice of points belonging to this segment. Always at least one
    /// point.
    pub points: Vec<SwipePoint>,
    /// The edge crossed *after* this segment, if any. None for the
    /// final segment if the path ended inside the letter region.
    pub crossed_into: Option<BoundaryEdge>,
}

/// Minimum distance (in screen pixels) the finger must travel inside
/// an edge zone before we count the crossing as deliberate. Rejects
/// grazes near the boundary.
pub const MIN_OUT_DWELL_PX: f64 = 12.0;

/// Segment a swipe path into pieces separated by boundary crossings.
///
/// Algorithm:
///
///   1. Walk the path. Track whether we're currently "inside" or
///      "outside" the letter region, based on the previous point.
///   2. On an inside→outside transition, mark a candidate crossing.
///      Don't commit yet; we need to see whether the finger dwells
///      in the edge zone.
///   3. While outside: accumulate distance traveled. If the finger
///      re-enters the letter region before reaching `MIN_OUT_DWELL_PX`
///      of accumulated distance OR the user lifts: discard the
///      candidate. Otherwise: commit the crossing.
///   4. On a committed crossing followed by an outside→inside
///      transition: start a new segment, the previous one ends at
///      the crossing.
///   5. If the path ends in an edge zone after a committed crossing,
///      the final segment "crossed into" that edge (signals a
///      trailing space).
///
/// Returns the segment list. Empty input yields an empty list.
pub fn segment_path(path: &[SwipePoint], boundary: &LetterBoundary) -> Vec<PathSegment> {
    if path.is_empty() {
        return vec![];
    }

    let mut segments: Vec<PathSegment> = vec![];
    let mut current: Vec<SwipePoint> = vec![];
    // State while traversing.
    enum State {
        Inside,
        // While outside, track the edge we're outside on, and how far
        // we've traveled within that edge zone.
        Outside {
            edge: BoundaryEdge,
            entry_x: f64,
            entry_y: f64,
            dwell: f64,
        },
    }
    let mut state = if boundary.contains(path[0].x, path[0].y) {
        State::Inside
    } else {
        State::Outside {
            edge: boundary
                .classify_outside(path[0].x, path[0].y)
                .unwrap_or(BoundaryEdge::Bottom),
            entry_x: path[0].x,
            entry_y: path[0].y,
            dwell: 0.0,
        }
    };
    current.push(path[0]);

    for i in 1..path.len() {
        let p = path[i];
        // We can't reassign `state` from inside a `match &mut state`
        // arm while bindings into it are still live. So each iteration
        // computes a "deferred action" describing what to do *after*
        // the match block ends, then applies it once the match borrow
        // is released. This is the standard trick for replacing a
        // match-borrowed enum variant.
        enum Action {
            None,
            EnterOutside(BoundaryEdge, f64, f64),
            // edge to commit (if any) on a confirmed crossing.
            // None inner = it was a graze; just continue current segment
            // Some inner = commit a crossing with this edge
            ReturnToInside(Option<BoundaryEdge>),
        }
        let action = match &mut state {
            State::Inside => {
                if boundary.contains(p.x, p.y) {
                    current.push(p);
                    Action::None
                } else {
                    // Inside → Outside. Classify the edge we just left
                    // through. We DON'T close the segment yet — wait
                    // until we know whether this was a real crossing or
                    // just a graze.
                    let edge = boundary
                        .classify_outside(p.x, p.y)
                        .unwrap_or(BoundaryEdge::Bottom);
                    // Add p to current speculatively. If the crossing
                    // commits, we'll trim it back off below.
                    current.push(p);
                    Action::EnterOutside(edge, p.x, p.y)
                }
            }
            State::Outside {
                edge,
                entry_x,
                entry_y,
                dwell,
            } => {
                let dx = p.x - *entry_x;
                let dy = p.y - *entry_y;
                *dwell += (dx * dx + dy * dy).sqrt();
                if boundary.contains(p.x, p.y) {
                    // Outside → Inside. Did we dwell enough to count?
                    let committed_edge =
                        if *dwell >= MIN_OUT_DWELL_PX && edge.is_springboard_space() {
                            Some(*edge)
                        } else {
                            None
                        };
                    if let Some(crossed) = committed_edge {
                        // Commit a crossing. Close current segment,
                        // start a new one beginning at p.
                        let mut seg = std::mem::take(&mut current);
                        // Trim trailing points that are outside the
                        // region — they belong to edge transit, not to
                        // either word.
                        while let Some(last) = seg.last() {
                            if !boundary.contains(last.x, last.y) {
                                seg.pop();
                            } else {
                                break;
                            }
                        }
                        if !seg.is_empty() {
                            segments.push(PathSegment {
                                points: seg,
                                crossed_into: Some(crossed),
                            });
                        }
                        current.push(p);
                        Action::ReturnToInside(Some(crossed))
                    } else {
                        // Graze — drop the outside excursion entirely.
                        // Continue the current segment with p.
                        current.push(p);
                        Action::ReturnToInside(None)
                    }
                } else {
                    // Still outside. Advance entry / continue
                    // accumulating dwell. Also record the point so
                    // we can emit a trailing-edge final segment if
                    // the path ends here.
                    *entry_x = p.x;
                    *entry_y = p.y;
                    current.push(p);
                    Action::None
                }
            }
        };

        // Match's mutable borrow of `state` is now released. Apply the
        // deferred state transition.
        match action {
            Action::None => {}
            Action::EnterOutside(edge, ex, ey) => {
                state = State::Outside {
                    edge,
                    entry_x: ex,
                    entry_y: ey,
                    dwell: 0.0,
                };
            }
            Action::ReturnToInside(_committed) => {
                state = State::Inside;
            }
        }
    }

    // End of path. Close out the last segment.
    let final_edge = if let State::Outside { edge, dwell, .. } = state {
        // If the path ended outside the region with enough dwell, mark
        // the segment as terminating in a space.
        if dwell >= MIN_OUT_DWELL_PX && edge.is_springboard_space() {
            Some(edge)
        } else {
            None
        }
    } else {
        None
    };

    // Trim trailing outside points if the final segment is just a tail
    // of edge-zone hover that won't commit.
    if final_edge.is_none() {
        while let Some(last) = current.last() {
            if !boundary.contains(last.x, last.y) {
                current.pop();
            } else {
                break;
            }
        }
    }

    if !current.is_empty() {
        segments.push(PathSegment {
            points: current,
            crossed_into: final_edge,
        });
    }

    segments
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keyboard::Key;

    fn pt(x: f64, y: f64) -> SwipePoint {
        SwipePoint::new(x, y)
    }

    fn hb(x: f64, y: f64, w: f64, h: f64) -> KeyHitbox {
        KeyHitbox {
            key: Key::character("x"),
            x,
            y,
            width: w,
            height: h,
        }
    }

    #[test]
    fn boundary_from_grid() {
        let boxes = vec![
            hb(0.0, 0.0, 30.0, 30.0),
            hb(30.0, 0.0, 30.0, 30.0),
            hb(0.0, 30.0, 30.0, 30.0),
            hb(30.0, 30.0, 30.0, 30.0),
        ];
        let b = LetterBoundary::from_hitboxes(&boxes).unwrap();
        assert_eq!(b.left, 0.0);
        assert_eq!(b.right, 60.0);
        assert_eq!(b.top, 0.0);
        assert_eq!(b.bottom, 60.0);
        assert!(b.contains(15.0, 15.0));
        assert!(!b.contains(70.0, 15.0));
    }

    #[test]
    fn classify_outside_picks_top_for_high_y() {
        let b = LetterBoundary {
            left: 0.0,
            right: 100.0,
            top: 20.0,
            bottom: 80.0,
        };
        assert_eq!(b.classify_outside(50.0, 10.0), Some(BoundaryEdge::Top));
        assert_eq!(b.classify_outside(50.0, 90.0), Some(BoundaryEdge::Bottom));
        assert_eq!(b.classify_outside(-5.0, 50.0), Some(BoundaryEdge::LeftSide));
        assert_eq!(b.classify_outside(150.0, 50.0), Some(BoundaryEdge::RightSide));
        assert_eq!(b.classify_outside(50.0, 50.0), None);
    }

    #[test]
    fn single_segment_no_crossings() {
        let b = LetterBoundary {
            left: 0.0,
            right: 100.0,
            top: 0.0,
            bottom: 100.0,
        };
        let path = vec![pt(10.0, 10.0), pt(50.0, 50.0), pt(90.0, 90.0)];
        let segs = segment_path(&path, &b);
        assert_eq!(segs.len(), 1);
        assert_eq!(segs[0].points.len(), 3);
        assert!(segs[0].crossed_into.is_none());
    }

    #[test]
    fn cross_through_bottom_creates_two_segments() {
        let b = LetterBoundary {
            left: 0.0,
            right: 100.0,
            top: 0.0,
            bottom: 100.0,
        };
        // Inside → out below → dwell → inside again.
        let path = vec![
            pt(50.0, 50.0),  // inside
            pt(50.0, 80.0),  // still inside
            pt(50.0, 110.0), // outside below
            pt(50.0, 125.0), // dwell > 12px
            pt(50.0, 95.0),  // back inside
            pt(50.0, 70.0),  // continue
        ];
        let segs = segment_path(&path, &b);
        assert_eq!(segs.len(), 2);
        assert_eq!(segs[0].crossed_into, Some(BoundaryEdge::Bottom));
        assert!(segs[1].crossed_into.is_none());
    }

    #[test]
    fn top_edge_crossing_does_not_split() {
        // Crossing the Top edge is NOT a springboard space — it's
        // reserved for digit / minimize. segment_path should NOT
        // split on it.
        let b = LetterBoundary {
            left: 0.0,
            right: 100.0,
            top: 20.0,
            bottom: 100.0,
        };
        let path = vec![
            pt(50.0, 50.0),  // inside
            pt(50.0, 30.0),  // still inside (above 20)
            pt(50.0, 5.0),   // outside top
            pt(50.0, -10.0), // dwell
            pt(50.0, 25.0),  // back inside
        ];
        let segs = segment_path(&path, &b);
        // Top crossings are filtered; the whole thing should be one
        // segment from the path's perspective (the springboard logic
        // above the boundary calls top a non-delimiter).
        assert_eq!(segs.len(), 1, "top crossings must not segment the path");
    }

    #[test]
    fn graze_does_not_split() {
        // Very brief excursion below boundary that doesn't dwell.
        let b = LetterBoundary {
            left: 0.0,
            right: 100.0,
            top: 0.0,
            bottom: 100.0,
        };
        let path = vec![
            pt(50.0, 95.0),
            pt(50.0, 102.0), // outside but only 7px out
            pt(50.0, 95.0),  // back in
        ];
        let segs = segment_path(&path, &b);
        assert_eq!(segs.len(), 1, "graze should not split");
        assert!(segs[0].crossed_into.is_none());
    }

    #[test]
    fn trailing_edge_marks_final_space() {
        let b = LetterBoundary {
            left: 0.0,
            right: 100.0,
            top: 0.0,
            bottom: 100.0,
        };
        let path = vec![
            pt(50.0, 50.0),
            pt(50.0, 95.0),
            pt(50.0, 110.0), // outside
            pt(50.0, 125.0), // dwell
        ];
        let segs = segment_path(&path, &b);
        assert_eq!(segs.len(), 1);
        assert_eq!(segs[0].crossed_into, Some(BoundaryEdge::Bottom));
    }
}
