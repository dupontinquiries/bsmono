//! springboard — a swipe that leaves the letters and comes back typed a space.
//!
//! `docs/single-hand-mode.md` §3. The letter region's perimeter IS the spacebar: swipe a
//! word, cross the bottom or a side edge, come back, swipe the next word. No lifting, no
//! reaching for a space key, which is the whole point on a phone held in one hand.
//!
//! ## The algorithm, and the two rules that make it usable
//!
//! Walk the path; each segment either crosses the letter region's boundary or does not. An
//! **out** crossing followed by an **in** crossing is one space delimiter, and the path is
//! cut there — each piece goes to the word recognizer on its own.
//!
//! **Hysteresis.** An out-crossing only counts if the path then travels at least
//! [`MIN_OUT_DWELL`] inside the edge zone before returning. Without it, a swipe that grazes
//! the boundary between two letters — which happens constantly on the bottom row — inserts
//! a space in the middle of a word. The doc names 12px; the rejection is the feature.
//!
//! **The top edge does not participate.** Crossing it is reserved for the digit arc and the
//! minimize fling ([`crate::topedge`]), and a swipe that goes up there is terminated rather
//! than segmented — otherwise reaching for a digit would also type whatever the path
//! happened to spell on the way.
//!
//! ## What this returns, and what it does not do
//!
//! [`segment`] returns index ranges into the path plus the crossing points, and nothing
//! else. It does not run the recognizer (that is `swipe.rs`'s), and it does not draw the
//! ripple the doc asks for at each delimiter — but it reports where one belongs, because a
//! delimiter the user cannot see is a space they will not learn to make on purpose.

use egui::{Pos2, Rect};

/// How far inside the edge zone a path must travel for an out-crossing to count (§3).
pub const MIN_OUT_DWELL: f32 = 12.0;

/// Which edge a crossing happened on. The top is listed so it can be REFUSED by name
/// rather than by falling through a `_`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Edge {
    Bottom,
    Left,
    Right,
    /// Reserved for the digit arc and the minimize fling.
    Top,
}

impl Edge {
    /// Does this edge insert a space?
    pub fn is_springboard(self) -> bool {
        !matches!(self, Edge::Top)
    }
}

/// Where the path left the letter region, for the ripple the doc asks for.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Delimiter {
    pub at: Pos2,
    pub edge: Edge,
    /// The path index the following segment starts at.
    pub resumes: usize,
}

/// The outcome of segmenting one swipe.
#[derive(Debug, Clone, PartialEq)]
pub struct Segments {
    /// Index ranges into the path, one per word to recognise.
    pub ranges: Vec<std::ops::Range<usize>>,
    /// One per delimiter that fired, in order.
    pub delimiters: Vec<Delimiter>,
    /// The path ended outside the letters with no closing crossing: recognise what there is
    /// and append a space (§3, step 5).
    pub trailing_space: bool,
    /// The path crossed the TOP edge, so the whole gesture belongs to `topedge` and nothing
    /// here should be recognised at all.
    pub terminated_by_top: bool,
}

impl Segments {
    /// Nothing to say: one word, no spaces.
    fn plain(len: usize) -> Segments {
        Segments {
            ranges: if len == 0 { Vec::new() } else { vec![0..len] },
            delimiters: Vec::new(),
            trailing_space: false,
            terminated_by_top: false,
        }
    }
}

/// Which edge a point outside `letters` is nearest — or `None` when it is inside.
pub fn outside_edge(letters: Rect, p: Pos2) -> Option<Edge> {
    if letters.contains(p) {
        return None;
    }
    // Above the region is the top edge whatever else is true: the digit and minimize
    // gestures own that direction, and treating a high-and-left point as a side crossing
    // would let a digit arc also insert a space.
    if p.y < letters.top() {
        return Some(Edge::Top);
    }
    if p.y > letters.bottom() {
        return Some(Edge::Bottom);
    }
    if p.x < letters.left() {
        return Some(Edge::Left);
    }
    Some(Edge::Right)
}

/// Segment a completed swipe path.
pub fn segment(path: &[Pos2], letters: Rect) -> Segments {
    if path.len() < 2 {
        return Segments::plain(path.len());
    }
    let mut out = Segments::plain(0);
    let mut seg_start = 0usize;
    // Where the path left the letters, and how far it has travelled since.
    let mut exit: Option<(usize, Pos2, Edge, f32)> = None;

    for i in 0..path.len() {
        let here = path[i];
        match (outside_edge(letters, here), &mut exit) {
            // Still inside, and was inside: nothing to do.
            (None, None) => {}
            // Left the region.
            (Some(edge), None) => {
                if edge == Edge::Top {
                    // The whole gesture belongs to `topedge`. Reporting the segments so far
                    // would let a digit arc also type a word.
                    return Segments {
                        ranges: Vec::new(),
                        delimiters: Vec::new(),
                        trailing_space: false,
                        terminated_by_top: true,
                    };
                }
                exit = Some((i, here, edge, 0.0));
            }
            // Outside, and still outside: accumulate the dwell.
            (Some(edge), Some((_, _, e, dwell))) => {
                if edge == Edge::Top {
                    return Segments {
                        ranges: Vec::new(),
                        delimiters: Vec::new(),
                        trailing_space: false,
                        terminated_by_top: true,
                    };
                }
                *e = edge;
                *dwell += (here - path[i - 1]).length();
            }
            // Came back in.
            (None, Some((idx, at, edge, dwell))) => {
                // HYSTERESIS: a graze is not a delimiter. Without this, a swipe that clips
                // the boundary between two letters — which happens constantly on the bottom
                // row — puts a space in the middle of a word.
                if *dwell >= MIN_OUT_DWELL && edge.is_springboard() {
                    out.ranges.push(seg_start..*idx);
                    out.delimiters.push(Delimiter { at: *at, edge: *edge, resumes: i });
                    seg_start = i;
                }
                exit = None;
            }
        }
    }

    // Ended outside with no closing crossing.
    if let Some((idx, _, edge, dwell)) = exit {
        if dwell >= MIN_OUT_DWELL && edge.is_springboard() {
            out.ranges.push(seg_start..idx);
            out.trailing_space = true;
            return out;
        }
        // A graze at the very end is not a space; the segment runs to the end of the path.
    }
    out.ranges.push(seg_start..path.len());
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn letters() -> Rect {
        Rect::from_min_size(Pos2::new(0.0, 0.0), egui::vec2(400.0, 200.0))
    }

    fn p(x: f32, y: f32) -> Pos2 {
        Pos2::new(x, y)
    }

    /// The gesture the feature exists for: swipe a word, dip below the keys, swipe the next.
    #[test]
    fn out_and_back_across_the_bottom_is_one_space() {
        let l = letters();
        let path = vec![
            p(10.0, 100.0),
            p(60.0, 120.0),
            // out below the bottom, and far enough along it to count
            p(80.0, 215.0),
            p(110.0, 218.0),
            // back in
            p(140.0, 150.0),
            p(180.0, 120.0),
        ];
        let s = segment(&path, l);
        assert_eq!(s.ranges.len(), 2, "{s:?}");
        assert_eq!(s.ranges[0], 0..2);
        assert_eq!(s.ranges[1], 4..6);
        assert_eq!(s.delimiters.len(), 1);
        assert_eq!(s.delimiters[0].edge, Edge::Bottom);
        assert_eq!(s.delimiters[0].resumes, 4);
        assert!(!s.trailing_space && !s.terminated_by_top);
    }

    /// The hysteresis rule, which is the difference between a usable feature and one that
    /// puts spaces inside words. A swipe along the bottom row grazes the boundary
    /// constantly.
    #[test]
    fn a_graze_is_not_a_space() {
        let l = letters();
        let path = vec![
            p(10.0, 190.0),
            p(40.0, 202.0),  // barely out
            p(44.0, 203.0),  // travels 4px out — under the 12px floor
            p(70.0, 190.0),  // back in
            p(120.0, 180.0),
        ];
        let s = segment(&path, l);
        assert_eq!(s.ranges, vec![0..5], "a graze inserted a space: {s:?}");
        assert!(s.delimiters.is_empty());

        // The same shape with a real excursion IS a delimiter — so the test is measuring
        // the dwell and not just refusing everything.
        let path = vec![
            p(10.0, 190.0),
            p(40.0, 205.0),
            p(70.0, 208.0),
            p(70.0, 190.0),
            p(120.0, 180.0),
        ];
        assert_eq!(segment(&path, l).delimiters.len(), 1);
    }

    /// "The top edge does NOT participate": crossing it is a digit or a minimize, and
    /// segmenting it as well would let a digit arc type whatever the path spelled on the
    /// way up.
    #[test]
    fn the_top_edge_terminates_rather_than_segments() {
        let l = letters();
        let path = vec![p(100.0, 50.0), p(105.0, 20.0), p(108.0, -30.0), p(110.0, -35.0)];
        let s = segment(&path, l);
        assert!(s.terminated_by_top);
        assert!(s.ranges.is_empty(), "a top crossing produced words: {s:?}");
        assert!(s.delimiters.is_empty());
        assert!(!Edge::Top.is_springboard());
        for e in [Edge::Bottom, Edge::Left, Edge::Right] {
            assert!(e.is_springboard());
        }
        // Above the region is TOP even when it is also left of it, or a digit arc leaving
        // at an angle would count as a side crossing.
        assert_eq!(outside_edge(l, p(-20.0, -20.0)), Some(Edge::Top));
    }

    /// §3 step 5: a path that ends outside recognises what it has and appends a space.
    #[test]
    fn ending_outside_the_letters_is_a_trailing_space() {
        let l = letters();
        let path = vec![p(10.0, 100.0), p(80.0, 150.0), p(120.0, 215.0), p(150.0, 220.0)];
        let s = segment(&path, l);
        assert!(s.trailing_space);
        assert_eq!(s.ranges, vec![0..2], "the word should stop where the path left: {s:?}");

        // But only if it actually dwelt out there — a path ending on a graze is one word.
        let path = vec![p(10.0, 100.0), p(80.0, 150.0), p(120.0, 203.0)];
        let s = segment(&path, l);
        assert!(!s.trailing_space, "a graze at the end became a space: {s:?}");
        assert_eq!(s.ranges, vec![0..3]);
    }

    /// Both side edges work, and two excursions are two spaces.
    #[test]
    fn the_sides_are_springboards_too_and_delimiters_accumulate() {
        let l = letters();
        let path = vec![
            p(20.0, 100.0),
            p(-15.0, 100.0),
            p(-18.0, 115.0), // dwelt outside the left edge
            p(30.0, 110.0),  // back in
            p(200.0, 110.0),
            p(415.0, 110.0),
            p(418.0, 125.0), // and out the right
            p(300.0, 120.0), // back in
            p(320.0, 130.0),
        ];
        let s = segment(&path, l);
        assert_eq!(s.delimiters.len(), 2, "{s:?}");
        assert_eq!(s.delimiters[0].edge, Edge::Left);
        assert_eq!(s.delimiters[1].edge, Edge::Right);
        assert_eq!(s.ranges.len(), 3);
    }

    /// A swipe that never leaves the letters is one word, and a degenerate path is not a
    /// crash.
    #[test]
    fn an_ordinary_swipe_is_untouched() {
        let l = letters();
        let path = vec![p(10.0, 100.0), p(80.0, 120.0), p(150.0, 90.0)];
        let s = segment(&path, l);
        assert_eq!(s.ranges, vec![0..3]);
        assert!(s.delimiters.is_empty() && !s.trailing_space && !s.terminated_by_top);

        assert!(segment(&[], l).ranges.is_empty());
        assert_eq!(segment(&[p(1.0, 1.0)], l).ranges, vec![0..1]);
    }
}

// ─────────────────────────── the space zones ───────────────────────────

/// A tappable strip outside the letter region that emits a space (§4).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Zone {
    pub rect: Rect,
    pub edge: Edge,
    /// Stays tappable during an active swipe. Only the bottom zone does, per the doc: "in
    /// case the user wants to end mid-swipe". Suppressing the side zones while swiping is
    /// what stops a word's own path from tapping one on its way past.
    pub live_while_swiping: bool,
}

/// `~24px wide` (§4).
pub const SIDE_W: f32 = 24.0;
/// The thinnest bottom zone worth offering.
pub const MIN_ZONE_H: f32 = 12.0;

/// The space zones for a board, given where the letters are and which side (if any) the
/// board is compacted to.
///
/// The bottom zone is there when the surface has room under the keys (a trimmed surface
/// has none, and then the bottom row's own space bar is the space). A side zone appears
/// only on the side the compaction
/// FREED — which is the job the freed strip has been waiting for since `halfboard` landed,
/// and the reason that module returns it rather than leaving a gap.
///
/// The doc says "only present in right-handed single-hand mode" for the right zone, and the
/// mirror for the left. Note which way round that is: a RIGHT-handed board is anchored right
/// and frees the LEFT, so the reachable strip — the one a right thumb can hit — is on the
/// left. Following the doc's geometry rather than its wording here, because a space zone
/// under the palm is not a space zone.
pub fn zones(board: Rect, letters: Rect, half: crate::halfboard::Side) -> Vec<Zone> {
    let mut out = Vec::new();
    // NO BOTTOM ZONE. There was a 36pt "␣" strip under the keys wherever the
    // surface had room; on the phone it sat over the buttons below the board
    // (it was never a real key, and the bottom row already has a spacebar).
    // Swiping out past the bottom edge still ends a word: that is `SwipeState`'s
    // exit test, not this strip. Only the one-handed side strip remains.
    if let Some(free) = half.free_zone(board) {
        let (rect, edge) = match half {
            // Anchored left: the strip is on the right.
            crate::halfboard::Side::Left => (
                Rect::from_min_max(
                    Pos2::new(free.left(), letters.top()),
                    Pos2::new((free.left() + SIDE_W).min(free.right()), letters.bottom()),
                ),
                Edge::Right,
            ),
            crate::halfboard::Side::Right => (
                Rect::from_min_max(
                    Pos2::new((free.right() - SIDE_W).max(free.left()), letters.top()),
                    Pos2::new(free.right(), letters.bottom()),
                ),
                Edge::Left,
            ),
            crate::halfboard::Side::Off => return out,
        };
        out.push(Zone { rect, edge, live_while_swiping: false });
    }
    out
}

/// The zone a tap at `p` lands in, if any. `swiping` suppresses the side zones.
pub fn zone_at(zones: &[Zone], p: Pos2, swiping: bool) -> Option<Zone> {
    zones
        .iter()
        .find(|z| z.rect.contains(p) && (!swiping || z.live_while_swiping))
        .copied()
}

#[cfg(test)]
mod zone_tests {
    use super::*;
    use crate::halfboard::Side;

    fn board() -> Rect {
        Rect::from_min_size(Pos2::ZERO, egui::vec2(500.0, 260.0))
    }
    fn letters() -> Rect {
        Rect::from_min_size(Pos2::ZERO, egui::vec2(500.0, 200.0))
    }

    /// Full width: no zone at all. The bottom strip overlapped the buttons
    /// under the board on the phone and was removed.
    #[test]
    fn a_full_width_board_has_no_space_zone() {
        let z = zones(board(), letters(), Side::Off);
        assert!(z.is_empty(), "{z:?}");
        assert!(zone_at(&z, Pos2::new(250.0, 215.0), false).is_none(), "under the keys taps nothing");
    }

    /// THE STRIP HAS TO BE OUTSIDE THE LETTERS, or a swipe through it delimits nothing.
    ///
    /// `zones` puts a space zone in the strip `halfboard` frees, and it could be TAPPED —
    /// but `SwipeState::update` built its letter region from the full board width, so a path
    /// into the strip was still "inside the letters" and never produced an exit. The zone was
    /// a button, not a springboard, which is the half that matters: the whole point is
    /// swiping a word, brushing the strip, and swiping the next one without lifting.
    #[test]
    fn a_path_into_the_freed_strip_delimits_a_word() {
        let board = Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(400.0, 200.0));
        let side = crate::halfboard::Side::Left;
        let keys = side.board_rect(board);
        let strip = side.free_zone(board).expect("a left half frees a right strip");
        assert!(strip.min.x >= keys.max.x - 1.0, "the strip should be beside the keys");

        // The letter region as `update_half` computes it: the KEYS' rect, minus the last row.
        let letters = Rect::from_min_max(
            keys.min,
            egui::pos2(keys.max.x, keys.max.y - 40.0),
        );

        // A swipe: inside the keys, out through the strip, back into the keys.
        let path = vec![
            egui::pos2(keys.min.x + 20.0, keys.min.y + 20.0),
            egui::pos2(keys.max.x - 10.0, keys.min.y + 20.0),
            // TWO points inside the strip, not one. `dwell` accumulates between consecutive
            // OUTSIDE samples, so a single point out there is a graze by construction and the
            // hysteresis correctly refuses it — a real finger crossing an 88pt strip is
            // sampled several times. My first version of this test had one point and failed,
            // which was the test being unrealistic rather than the code being wrong.
            egui::pos2(strip.min.x + 10.0, keys.min.y + 25.0),
            egui::pos2(strip.center().x, keys.min.y + 30.0),
            egui::pos2(keys.max.x - 10.0, keys.min.y + 40.0),
            egui::pos2(keys.min.x + 20.0, keys.min.y + 40.0),
        ];
        let seg = segment(&path, letters);
        assert_eq!(seg.ranges.len(), 2, "the strip did not split the swipe: {seg:?}");
        assert_eq!(seg.delimiters[0].edge, Edge::Right, "{seg:?}");

        // And with the FULL BOARD as the letter region — the bug — the same path is one word.
        let whole = Rect::from_min_max(board.min, egui::pos2(board.max.x, board.max.y - 40.0));
        assert_eq!(segment(&path, whole).ranges.len(), 1, "the premise: the old rect swallowed it");
    }

    /// THE DOUBLE-FIRE. When the keys reach the bottom of the board there is no bottom zone
    /// at all, and no zone ever contains a point inside the letters rect -- a tap on the
    /// bottom row's ⌫ must not also be a tap on a space zone.
    #[test]
    fn no_zone_lies_on_the_keys() {
        let b = board();
        // Keys fill the board: nothing below them.
        assert!(zones(b, b, Side::Off).is_empty(), "a zone with no room under the keys");
        // A sliver under the keys is not a zone either.
        let l = Rect::from_min_max(b.min, Pos2::new(b.max.x, b.max.y - 6.0));
        assert!(zones(b, l, Side::Off).is_empty(), "a 6pt sliver was offered as a zone");
        // With room, the zone starts exactly where the keys end.
        for side in [Side::Off, Side::Left, Side::Right] {
            let keys = side.board_rect(b);
            let l = Rect::from_min_max(Pos2::new(keys.left(), 0.0), Pos2::new(keys.right(), 200.0));
            for z in zones(b, l, side) {
                let inner = z.rect.shrink(0.01);
                assert!(!inner.intersects(l), "{side:?}: zone {:?} overlaps the keys {l:?}", z.rect);
            }
        }
    }

    /// The freed strip finally has a job — this is what `halfboard::free_zone` was returning
    /// it for.
    #[test]
    fn compaction_adds_a_side_zone_in_the_freed_strip() {
        let (b, l) = (board(), letters());
        for (side, want_edge) in [(Side::Left, Edge::Right), (Side::Right, Edge::Left)] {
            let keys = side.board_rect(b);
            let letters_here = Rect::from_min_max(
                Pos2::new(keys.left(), l.top()),
                Pos2::new(keys.right(), l.bottom()),
            );
            let z = zones(b, letters_here, side);
            assert_eq!(z.len(), 1, "{side:?}: {z:?}");
            let strip = z[0];
            assert_eq!(strip.edge, want_edge);
            assert!((strip.rect.width() - SIDE_W).abs() < 0.01);
            assert!(
                side.free_zone(b).unwrap().contains(strip.rect.center()),
                "{side:?}: the strip is not in the freed space"
            );
            // A right-anchored board frees the LEFT, so the reachable strip is on the left —
            // a space zone under the palm is not a space zone.
            match side {
                Side::Left => assert!(strip.rect.left() > keys.right() - 1.0),
                Side::Right => assert!(strip.rect.right() < keys.left() + 1.0),
                Side::Off => unreachable!(),
            }
        }
    }

    /// A side zone is suppressed during a swipe, so a word's own path cannot tap one on its
    /// way past.
    #[test]
    fn the_side_zone_goes_quiet_while_swiping() {
        let (b, l) = (board(), letters());
        let keys = Side::Left.board_rect(b);
        let letters_here = Rect::from_min_max(Pos2::new(keys.left(), l.top()), Pos2::new(keys.right(), l.bottom()));
        let z = zones(b, letters_here, Side::Left);
        let side_point = z[0].rect.center();
        assert!(zone_at(&z, side_point, false).is_some(), "tappable at rest");
        assert!(zone_at(&z, side_point, true).is_none(), "a swipe past it tapped it");
    }
}

// ─────────────────────────── visual feedback ───────────────────────────
//
// §3's ripple and §4's post-word flash. Both are pure functions of AGE, so the caller keeps
// a start time and asks; nothing here reads a clock or draws.
//
// These are teaching aids, not decoration, and the doc says so of each: the ripple "teaches
// the user that the crossing counted", the flash "teaches the multi-anchor design". A
// springboard space that happens invisibly is a feature nobody learns they have — which is
// the state this port was in for two commits, with the crossings detected and nothing
// marking them.

/// `~250ms fade-out` (§3).
pub const RIPPLE_MS: f32 = 250.0;
/// How far the ripple grows, in points.
pub const RIPPLE_R: f32 = 22.0;
/// `150ms growing … 150ms fading back` (§4).
pub const PULSE_UP_MS: f32 = 150.0;
pub const PULSE_DOWN_MS: f32 = 150.0;
/// The zone nearest the last touch pulses `~30%` harder (§4).
pub const NEAREST_BOOST: f32 = 0.3;

/// A delimiter ripple's radius and alpha at `age_ms`, or `None` once it is over.
///
/// Grows while it fades: a circle that only fades reads as a dimming dot, and the growth is
/// what makes it read as something leaving the crossing point.
pub fn ripple(age_ms: f32) -> Option<(f32, f32)> {
    if !(0.0..RIPPLE_MS).contains(&age_ms) {
        return None;
    }
    let t = age_ms / RIPPLE_MS;
    // Ease out: fast at the start, so the mark appears AT the crossing rather than drifting
    // away from it.
    let eased = 1.0 - (1.0 - t) * (1.0 - t);
    Some((RIPPLE_R * eased, 1.0 - t))
}

/// A space zone's `(opacity, scale)` at `age_ms` after a word was committed.
///
/// `nearest` adds the doc's 30% to the opacity — "hinting this one is closest" — and is
/// deliberately not applied to the scale: two zones growing by different amounts reads as a
/// layout glitch, while two at different brightnesses reads as emphasis.
pub fn pulse(age_ms: f32, nearest: bool) -> Option<(f32, f32)> {
    let total = PULSE_UP_MS + PULSE_DOWN_MS;
    if !(0.0..total).contains(&age_ms) {
        return None;
    }
    let t = if age_ms < PULSE_UP_MS {
        age_ms / PULSE_UP_MS
    } else {
        1.0 - (age_ms - PULSE_UP_MS) / PULSE_DOWN_MS
    };
    let boost = if nearest { 1.0 + NEAREST_BOOST } else { 1.0 };
    Some(((t * boost).clamp(0.0, 1.0), 1.0 + 0.1 * t))
}

/// Which zone is nearest a point, for the pulse's emphasis. `None` when there are none.
pub fn nearest_zone(zones: &[Zone], to: Pos2) -> Option<usize> {
    zones
        .iter()
        .enumerate()
        .min_by(|(_, a), (_, b)| {
            let da = a.rect.center().distance(to);
            let db = b.rect.center().distance(to);
            da.partial_cmp(&db).unwrap_or(std::cmp::Ordering::Equal)
        })
        .map(|(i, _)| i)
}

#[cfg(test)]
mod feedback_tests {
    use super::*;

    /// "Flash a small ripple at the crossing point. ~250ms fade-out." It grows while it
    /// fades — a circle that only fades reads as a dimming dot rather than something
    /// leaving the crossing.
    #[test]
    fn the_ripple_grows_as_it_fades_and_then_stops() {
        let (r0, a0) = ripple(0.0).expect("alive at birth");
        let (r1, a1) = ripple(RIPPLE_MS * 0.5).unwrap();
        let (r2, a2) = ripple(RIPPLE_MS * 0.9).unwrap();
        assert!(r0 < r1 && r1 < r2, "it did not grow: {r0} {r1} {r2}");
        assert!(a0 > a1 && a1 > a2, "it did not fade: {a0} {a1} {a2}");
        assert!((a0 - 1.0).abs() < 1e-6, "it should start at full alpha");
        assert!(r2 <= RIPPLE_R + 0.01);

        // Over, and never negative-aged.
        assert_eq!(ripple(RIPPLE_MS), None, "it outlived its window");
        assert_eq!(ripple(RIPPLE_MS + 100.0), None);
        assert_eq!(ripple(-1.0), None);

        // Eased out: more than half the growth happens in the first half.
        assert!(r1 > RIPPLE_R * 0.5, "not eased — the mark drifts away from the crossing");
    }

    /// "150ms growing to full opacity + 1.1x scale, 150ms fading back."
    #[test]
    fn the_pulse_rises_and_falls_symmetrically() {
        let peak = pulse(PULSE_UP_MS, false).expect("alive at the peak");
        assert!((peak.0 - 1.0).abs() < 1e-6, "it did not reach full opacity: {peak:?}");
        assert!((peak.1 - 1.1).abs() < 1e-6, "it did not reach 1.1x: {peak:?}");

        let up = pulse(PULSE_UP_MS * 0.5, false).unwrap();
        let down = pulse(PULSE_UP_MS + PULSE_DOWN_MS * 0.5, false).unwrap();
        assert!((up.0 - down.0).abs() < 1e-6, "the rise and fall are not mirror images");
        assert!(up.0 < peak.0 && down.0 < peak.0);

        assert_eq!(pulse(PULSE_UP_MS + PULSE_DOWN_MS, false), None, "it outlived its window");
        assert_eq!(pulse(-1.0, false), None);
    }

    /// "The zone nearest the user's last touch pulses with ~30% extra intensity." Opacity
    /// only: two zones growing by different amounts reads as a layout glitch, two at
    /// different brightnesses reads as emphasis.
    #[test]
    fn the_nearest_zone_is_brighter_but_not_bigger() {
        let plain = pulse(PULSE_UP_MS * 0.5, false).unwrap();
        let near = pulse(PULSE_UP_MS * 0.5, true).unwrap();
        assert!(near.0 > plain.0, "the boost did nothing");
        assert!((near.0 / plain.0 - (1.0 + NEAREST_BOOST)).abs() < 1e-5);
        assert!((near.1 - plain.1).abs() < 1e-6, "the nearest zone grew more than the others");
        // And it cannot exceed full opacity, or the fill saturates and the emphasis is lost.
        assert!(pulse(PULSE_UP_MS, true).unwrap().0 <= 1.0);
    }

    #[test]
    fn the_nearest_zone_is_the_one_nearest() {
        let board = Rect::from_min_size(Pos2::ZERO, egui::vec2(500.0, 260.0));
        let letters = Rect::from_min_size(Pos2::ZERO, egui::vec2(390.0, 200.0));
        let z = zones(board, letters, crate::halfboard::Side::Left);
        assert_eq!(z.len(), 1);
        assert_eq!(nearest_zone(&z, Pos2::new(430.0, 100.0)), Some(0));
        assert_eq!(nearest_zone(&[], Pos2::ZERO), None);
    }
}
