//! range — selecting a span of TIME across tracks.
//!
//! Studio's selection is a list of `ClipId`, which is the NLE's unit: you pick
//! whole clips and move them. An audio editor's unit is different and the
//! README says the audio side is informed by Audacity — there, you drag across
//! the waveform and operate on **the span you dragged**, which may be the middle
//! of one clip, or parts of several, or a region with no clip in it at all.
//!
//! Neither model subsumes the other, so this is a second selection rather than a
//! replacement. "Delete these two clips" and "delete from 1.2s to 3.4s on this
//! track" are different edits with different results, and an editor that
//! silently converted one into the other would destroy work at the boundaries.
//!
//! ## Empty is not the same as absent
//!
//! A zero-length range at the playhead is a real and useful state — it is where
//! a paste lands, and it is what a click without a drag produces. `None` means
//! nothing is selected and an operation should refuse; `Some(empty)` means a
//! point is selected and a paste has somewhere to go. Collapsing the two makes
//! click-to-position impossible.
//!
//! ## Normalised on construction, so nothing downstream has to check
//!
//! A drag can end left of where it began. Every consumer would otherwise need
//! its own `if end < start` — and the one that forgets produces a range whose
//! length is negative, which propagates into a trim as a clip that ends before
//! it starts.

use studio_core::{Ticks, TrackId};

/// A span of time on a set of tracks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TimeRange {
    start: Ticks,
    end: Ticks,
    /// Which tracks the span applies to. Empty means "all tracks", which is
    /// what a drag in the ruler above the tracks means.
    pub tracks: Vec<TrackId>,
}

impl TimeRange {
    /// Build from two points in any order.
    pub fn new(a: Ticks, b: Ticks, tracks: Vec<TrackId>) -> TimeRange {
        let (start, end) = if b.0 < a.0 { (b, a) } else { (a, b) };
        TimeRange { start, end, tracks }
    }

    /// A point selection at `t` — a click with no drag.
    pub fn point(t: Ticks, tracks: Vec<TrackId>) -> TimeRange {
        TimeRange { start: t, end: t, tracks }
    }

    pub fn start(&self) -> Ticks { self.start }
    pub fn end(&self) -> Ticks { self.end }
    pub fn len(&self) -> Ticks { Ticks(self.end.0 - self.start.0) }
    pub fn is_point(&self) -> bool { self.start == self.end }

    /// Does this range apply to a given track?
    ///
    /// An empty track list means all of them, so a ruler drag does not have to
    /// enumerate every track — and does not go stale when one is added.
    pub fn covers_track(&self, t: TrackId) -> bool {
        self.tracks.is_empty() || self.tracks.contains(&t)
    }

    /// Does the range overlap a clip's span on a track it covers?
    ///
    /// A POINT range overlaps nothing, deliberately. Clicking inside a clip
    /// positions the cursor; it does not select the clip, and treating it as an
    /// overlap would make a stray click delete audio.
    pub fn overlaps(&self, track: TrackId, pos: Ticks, len: Ticks) -> bool {
        if self.is_point() || !self.covers_track(track) { return false; }
        let clip_end = pos.0 + len.0;
        self.start.0 < clip_end && pos.0 < self.end.0
    }

    /// The part of a clip that falls inside the range, in clip-local ticks.
    ///
    /// This is what a trim or a delete actually operates on. Returned as
    /// clip-local rather than timeline-absolute because every edit op in
    /// `studio-core` is expressed against the clip's own start, and converting
    /// at each call site is how an off-by-one becomes a silent trim of the
    /// wrong second.
    pub fn intersect_local(&self, track: TrackId, pos: Ticks, len: Ticks)
        -> Option<(Ticks, Ticks)> {
        if !self.overlaps(track, pos, len) { return None; }
        let s = self.start.0.max(pos.0) - pos.0;
        let e = self.end.0.min(pos.0 + len.0) - pos.0;
        if e <= s { return None; }
        Some((Ticks(s), Ticks(e)))
    }

    /// Extend to include `t`, keeping the anchor.
    ///
    /// Used while dragging: the anchor is where the press landed and does not
    /// move, so dragging back past it flips the range rather than dragging the
    /// start along — which is what makes a drag correctable without lifting.
    pub fn extend_to(&self, anchor: Ticks, t: Ticks) -> TimeRange {
        TimeRange::new(anchor, t, self.tracks.clone())
    }

    /// Snap both edges through a caller-supplied snapper.
    ///
    /// Takes a function rather than reaching for `studio_egui::snap` so the
    /// range type stays independent of the view — the audio editor and the NLE
    /// snap to different things, and neither should be wired in here.
    pub fn snapped(&self, f: impl Fn(Ticks) -> Ticks) -> TimeRange {
        TimeRange::new(f(self.start), f(self.end), self.tracks.clone())
    }
}

/// What a range selection asks the host to do.
///
/// Named for the audio editor's vocabulary rather than the NLE's: these are
/// operations on a span, and each one means something different from its
/// clip-level namesake.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RangeOp {
    /// Remove the span and close the gap — everything after moves left.
    Delete,
    /// Remove the span and leave silence — nothing moves.
    Silence,
    /// Keep only the span; discard everything outside it.
    Trim,
    /// Split at both edges without removing anything.
    SplitAtEdges,
}

impl RangeOp {
    /// Does this operation shift material after the range?
    ///
    /// The distinction that matters most in an audio editor: delete-and-close
    /// moves everything after it, silence does not. Getting it wrong
    /// desynchronises every track the user did not have selected, and they find
    /// out minutes later.
    pub fn shifts_later_material(&self) -> bool { *self == RangeOp::Delete }

    /// Is this operation meaningful on a point selection?
    ///
    /// Only splitting. Deleting or silencing nothing is a no-op the user will
    /// read as a broken key, so the caller should refuse rather than perform it.
    pub fn valid_on_point(&self) -> bool { *self == RangeOp::SplitAtEdges }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(n: i64) -> Ticks { Ticks(n) }
    fn tracks() -> Vec<TrackId> { vec![TrackId(1)] }

    #[test]
    fn a_backwards_drag_is_normalised_on_construction() {
        // Every consumer would otherwise need its own `if end < start`, and the
        // one that forgets produces a negative length that reaches a trim as a
        // clip ending before it starts.
        let r = TimeRange::new(t(900), t(100), tracks());
        assert_eq!(r.start(), t(100));
        assert_eq!(r.end(), t(900));
        assert_eq!(r.len(), t(800));
    }

    #[test]
    fn a_point_selection_is_a_real_state_not_an_absent_one() {
        // It is where a paste lands, and what a click without a drag produces.
        let p = TimeRange::point(t(500), tracks());
        assert!(p.is_point());
        assert_eq!(p.len(), t(0));
    }

    #[test]
    fn a_point_selection_overlaps_nothing() {
        // Clicking inside a clip positions the cursor; treating it as an
        // overlap would make a stray click delete audio.
        let p = TimeRange::point(t(500), tracks());
        assert!(!p.overlaps(TrackId(1), t(0), t(1000)));
        assert_eq!(p.intersect_local(TrackId(1), t(0), t(1000)), None);
    }

    #[test]
    fn an_empty_track_list_means_all_tracks() {
        // A ruler drag should not have to enumerate every track, and must not
        // go stale when one is added.
        let all = TimeRange::new(t(0), t(100), vec![]);
        assert!(all.covers_track(TrackId(1)));
        assert!(all.covers_track(TrackId(99)));
    }

    #[test]
    fn a_track_outside_the_selection_is_untouched() {
        let r = TimeRange::new(t(0), t(1000), vec![TrackId(1)]);
        assert!(r.overlaps(TrackId(1), t(0), t(500)));
        assert!(!r.overlaps(TrackId(2), t(0), t(500)));
    }

    #[test]
    fn the_intersection_is_returned_in_clip_local_ticks() {
        // Every edit op in studio-core is expressed against the clip's own
        // start, and converting at each call site is how an off-by-one becomes
        // a silent trim of the wrong second.
        let r = TimeRange::new(t(1200), t(1800), tracks());
        // A clip starting at 1000, one second long.
        let (s, e) = r.intersect_local(TrackId(1), t(1000), t(1000)).expect("overlaps");
        assert_eq!(s, t(200));
        assert_eq!(e, t(800));
    }

    #[test]
    fn a_range_wider_than_the_clip_clamps_to_the_clip() {
        let r = TimeRange::new(t(0), t(10_000), tracks());
        let (s, e) = r.intersect_local(TrackId(1), t(1000), t(500)).expect("overlaps");
        assert_eq!(s, t(0));
        assert_eq!(e, t(500));
    }

    #[test]
    fn a_range_that_only_touches_an_edge_is_not_an_intersection() {
        // Abutting is not overlapping: a range ending exactly where a clip
        // starts must not trim a zero-length piece off it.
        let r = TimeRange::new(t(0), t(1000), tracks());
        assert!(!r.overlaps(TrackId(1), t(1000), t(500)));
        assert_eq!(r.intersect_local(TrackId(1), t(1000), t(500)), None);
    }

    #[test]
    fn extending_keeps_the_anchor_so_a_drag_can_be_corrected() {
        // The anchor is where the press landed. Dragging back past it flips the
        // range rather than dragging the start along, which is what lets
        // someone fix an overshoot without lifting.
        let r = TimeRange::point(t(500), tracks());
        let forward = r.extend_to(t(500), t(900));
        assert_eq!((forward.start(), forward.end()), (t(500), t(900)));
        let back = r.extend_to(t(500), t(100));
        assert_eq!((back.start(), back.end()), (t(100), t(500)));
    }

    #[test]
    fn snapping_goes_through_a_supplied_function() {
        // The audio editor and the NLE snap to different things, and neither
        // should be wired into the range type.
        let r = TimeRange::new(t(97), t(203), tracks());
        let snapped = r.snapped(|x| Ticks((x.0 as f64 / 100.0).round() as i64 * 100));
        assert_eq!(snapped.start(), t(100));
        assert_eq!(snapped.end(), t(200));
    }

    #[test]
    fn snapping_preserves_the_track_set() {
        let r = TimeRange::new(t(0), t(10), vec![TrackId(3), TrackId(7)]);
        assert_eq!(r.snapped(|x| x).tracks, vec![TrackId(3), TrackId(7)]);
    }

    #[test]
    fn delete_shifts_later_material_and_silence_does_not() {
        // The distinction that matters most in an audio editor. Getting it
        // wrong desynchronises every track the user did NOT have selected, and
        // they find out minutes later.
        assert!(RangeOp::Delete.shifts_later_material());
        assert!(!RangeOp::Silence.shifts_later_material());
        assert!(!RangeOp::Trim.shifts_later_material());
        assert!(!RangeOp::SplitAtEdges.shifts_later_material());
    }

    #[test]
    fn only_splitting_is_meaningful_on_a_point() {
        // Deleting nothing is a no-op the user reads as a broken key, so the
        // caller should refuse rather than perform it.
        assert!(RangeOp::SplitAtEdges.valid_on_point());
        assert!(!RangeOp::Delete.valid_on_point());
        assert!(!RangeOp::Silence.valid_on_point());
        assert!(!RangeOp::Trim.valid_on_point());
    }

    #[test]
    fn a_range_selection_is_not_a_clip_selection() {
        // "Delete these two clips" and "delete 1.2s to 3.4s" are different
        // edits with different results, which is why this is a second
        // selection rather than a replacement — an editor that silently
        // converted one to the other would destroy work at the boundaries.
        let r = TimeRange::new(t(1200), t(3400), tracks());
        // The range covers only the middle of this clip; a clip selection
        // would have taken all of it.
        let (s, e) = r.intersect_local(TrackId(1), t(0), t(5000)).expect("overlaps");
        assert!(s > t(0) && e < t(5000));
    }
}
