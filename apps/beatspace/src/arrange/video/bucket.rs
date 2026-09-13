//! Content-importance buckets for feel mode.
//!
//! [`cache_policy`] answers "how far, how dense" purely from distance to
//! the playhead. That's Preview mode. Feel mode wants the same byte
//! budget spent unevenly — more on a face, more around a cut, less on a
//! held static wide shot — which means every frame needs an IMPORTANCE
//! on top of its distance, resolved at three confidence levels (see
//! [`Resolved`]).
//!
//! ## Two kinds of fact, held two different ways
//!
//! This is the part that makes bucket-guessing safe to build editing
//! features on top of, which is the whole reason it's worth doing
//! properly:
//!
//! * **A [`Segment`] is a fact about a SOURCE** — a piece of decoded
//!   content, addressed by absolute source-frame numbers. It does not
//!   know or care where that source is used on the timeline. Splitting,
//!   trimming, moving, or duplicating a clip that plays this source
//!   changes nothing about the source itself, so segments are never
//!   migrated, patched, or invalidated by any of those edits — only by
//!   the source's own content actually changing, which
//!   [`profile::verify`]'s fingerprint check catches. This is the frozen,
//!   hand-maintained half.
//! * **An [`Instance`] (which timeline clip plays which window of which
//!   source) and adjacency (which instances are next to which) are facts
//!   about the ARRANGEMENT.** These change on every structural edit, so
//!   they are never hand-maintained — [`BucketField::rebuild`] throws the
//!   old ones away and recomputes them fresh from the live timeline every
//!   time, the same recompute-don't-patch discipline
//!   `cache_policy::desired_set` already uses for the playhead shell.
//!   Stale adjacency is a bug that can't happen if adjacency is never
//!   kept, only ever just-computed.
//!
//! The consequence: Split, Trim, Duplicate, and Delete cannot desync a
//! `BucketField` from reality even in principle, because segments never
//! reference the thing those edits change. The only edit category that
//! CAN invalidate a segment is replacing what a source plays (re-linking,
//! re-encoding) — which is `profile::verify`'s job, not this module's.
//! The property tests at the bottom pin this against the REAL editor in
//! `arrange::cmd`, not a stand-in.
//!
//! ## Three confidence levels
//!
//! 1. **Verified** — a [`Segment`] covers this source frame, and its
//!    profile is still fresh. Confidence 1.0.
//! 2. **Guessed** — no segment covers it, but one is reachable by walking
//!    outward through this instance and, if the walk runs out of room,
//!    across a freshly-derived adjacency edge into a neighbouring
//!    instance. Decayed by distance and by an extra attenuation per
//!    boundary crossed — a face shot says a lot about its own next few
//!    frames, much less about whatever clip comes after it. Bounded to a
//!    search radius, so resolving one frame is never a timeline-wide scan.
//! 3. **Fallback** — nothing verified within reach. Confidence exactly
//!    0, neutral importance: this degrades to EXACTLY the plain distance
//!    falloff, which is the point — everywhere unanalysed behaves like
//!    Preview, and only bends once real information exists near it.

use std::collections::HashMap;
use std::ops::Range;

/// Identifies a piece of underlying content — a video source, addressed
/// the way `arrange::render::Library` addresses audio sources: an index
/// (or any other stable id a caller assigns). Two clip instances that
/// play the same source share this id and therefore share its segments
/// automatically, with no extra bookkeeping — a duplicated clip is
/// profiled for free.
pub type SourceId = u64;

/// Identifies one clip INSTANCE on the timeline. Deliberately the same
/// shape as `arrange::model::ClipId` (a `u64`) so a caller can reuse the
/// arrangement's own ids directly rather than maintaining a second id
/// space, but this module takes no dependency on `arrange::model` itself
/// — see the module doc on why instance state is never hand-maintained.
pub type InstanceId = u64;

/// A coarse content class. Ordered by ascending importance — later
/// variants are more expensive to get wrong, not "bigger" in any other
/// sense.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Bucket {
    /// Little or no frame-to-frame change: a held wide shot, a title card.
    Static,
    /// Sustained motion: a pan, action, anything that judders visibly if
    /// the frame rate the eye perceives drops.
    Motion,
    /// Within a couple of seconds of a detected hard cut on either side —
    /// the moment a viewer's eye is freshest and stutters are loudest.
    CutBoundary,
    /// A skin-tone-heavy region — a CRUDE proxy for "there is probably a
    /// person in frame," not a face detector. Named as an approximation
    /// deliberately: nothing downstream should treat this as face
    /// recognition, tracking, or anything a real model would be needed
    /// for.
    Face,
}

impl Bucket {
    /// Baseline importance, before distance or confidence. `Static` is
    /// the neutral value the pure-distance fallback (level 3) uses.
    pub fn importance(self) -> f32 {
        match self {
            Bucket::Static => 0.0,
            Bucket::Motion => 0.35,
            Bucket::CutBoundary => 0.6,
            Bucket::Face => 1.0,
        }
    }
}

/// A profiled, frozen span of one SOURCE's frames — see the module doc's
/// "two kinds of fact" for why this never references a timeline clip.
#[derive(Debug, Clone, PartialEq)]
pub struct Segment {
    pub source: SourceId,
    /// Frame range, in the SOURCE's own absolute frame numbering.
    pub range: Range<i64>,
    pub bucket: Bucket,
}

/// What [`BucketField::resolve`] returned, and how it got there.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Resolved {
    pub importance: f32,
    /// 1.0 = a verified segment covers this frame exactly.
    /// (0, 1) = guessed from a nearby verified segment.
    /// 0.0 = fallback; behave exactly like plain distance-based Preview.
    pub confidence: f32,
}

impl Resolved {
    pub const FALLBACK: Resolved = Resolved { importance: 0.0, confidence: 0.0 };
}

/// How far a guess reaches, and how it decays. Frame counts, so callers
/// scale by fps the same way [`cache_policy::Shells::for_fps`] does.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GuessRadius {
    /// Frames a guess can travel within the SAME instance before its
    /// confidence reaches zero.
    pub within_clip: i64,
    /// Frames a guess can travel ACROSS an instance boundary before
    /// reaching zero — shorter than `within_clip` on purpose.
    pub across_clip: i64,
}

impl Default for GuessRadius {
    fn default() -> Self {
        // ~2s within a clip at 24fps says "this shot is still probably a
        // face"; ~0.4s across a cut says "the very next couple of frames
        // of whatever comes next might still be reacting to it," and no
        // further.
        Self { within_clip: 48, across_clip: 10 }
    }
}

/// One clip instance's placement, as a caller derives it from ITS OWN
/// timeline representation for [`BucketField::rebuild`]. Deliberately
/// generic (not `arrange::model::Clip`) so this module stays decoupled
/// from any one timeline shape, present or future.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Placement {
    pub instance: InstanceId,
    pub source: SourceId,
    /// Where in the source this instance's frame 0 begins (a trimmed
    /// head shifts this forward), and how many frames it plays.
    pub source_offset: i64,
    pub len: i64,
    /// Which track it's on — adjacency is only ever derived between
    /// clips on the SAME track; two clips that merely start around the
    /// same time on different tracks are simultaneous, not sequential,
    /// and say nothing about each other.
    pub track: u64,
    /// Position on the timeline, in the caller's own units (beats,
    /// samples, whatever) — used only to order clips and measure gaps
    /// for adjacency, then discarded.
    pub start: f64,
    pub end: f64,
}

/// One instance's mapping from its own local frame numbering (`0..len`)
/// to the source it plays.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Instance {
    source: SourceId,
    source_offset: i64,
    len: i64,
}

/// The verified segments for however many sources appear on one
/// timeline, plus the CURRENT instance placement and adjacency — always
/// exactly what [`BucketField::rebuild`] last computed, never
/// hand-patched. See the module doc.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct BucketField {
    /// Segments, grouped implicitly by `source` at query time. Untouched
    /// by `rebuild` — this is the hand-maintained half.
    segments: Vec<Segment>,
    /// Rebuilt from scratch by every `rebuild` call.
    instances: HashMap<InstanceId, Instance>,
    adjacency: Vec<(InstanceId, InstanceId)>,
    radius: GuessRadius,
}

impl BucketField {
    pub fn new(radius: GuessRadius) -> Self {
        Self { segments: Vec::new(), instances: HashMap::new(), adjacency: Vec::new(), radius }
    }

    /// Add a verified segment for a source. Independent of the timeline —
    /// call this once per profiled source, whenever profiling finishes or
    /// a re-profile replaces stale segments; never in response to an
    /// editing command.
    pub fn add_segment(&mut self, s: Segment) {
        self.segments.push(s);
    }

    /// Drop every segment for `source` — the caller's response to
    /// `profile::verify` reporting it stale, before adding fresh ones.
    pub fn clear_segments_for(&mut self, source: SourceId) {
        self.segments.retain(|s| s.source != source);
    }

    /// Replace ALL instance placement and adjacency with what
    /// `placements` says right now. Call this after every structural
    /// edit — split, trim, move, duplicate, delete, undo, redo — with the
    /// full, current set of clips on the timeline. Cheap: this is a sort
    /// and a linear scan per track, not per frame.
    ///
    /// `gap_tolerance` is how close two clips' ends must be, in the
    /// caller's timeline units, to still count as adjacent for guessing —
    /// exactly touching, or close enough that grid snapping didn't quite
    /// close a real cut.
    pub fn rebuild(&mut self, placements: &[Placement], gap_tolerance: f64) {
        self.instances.clear();
        self.adjacency.clear();
        for p in placements {
            self.instances.insert(p.instance, Instance { source: p.source, source_offset: p.source_offset, len: p.len });
        }
        let mut by_track: HashMap<u64, Vec<&Placement>> = HashMap::new();
        for p in placements {
            by_track.entry(p.track).or_default().push(p);
        }
        for track_clips in by_track.values_mut() {
            track_clips.sort_by(|a, b| a.start.partial_cmp(&b.start).unwrap_or(std::cmp::Ordering::Equal));
            for w in track_clips.windows(2) {
                let (a, b) = (w[0], w[1]);
                if (b.start - a.end).abs() <= gap_tolerance {
                    self.adjacency.push((a.instance, b.instance));
                }
            }
        }
    }

    fn next_instance(&self, id: InstanceId) -> Option<InstanceId> {
        self.adjacency.iter().find(|(l, _)| *l == id).map(|(_, r)| *r)
    }
    fn prev_instance(&self, id: InstanceId) -> Option<InstanceId> {
        self.adjacency.iter().find(|(_, r)| *r == id).map(|(l, _)| *l)
    }

    /// Level 1: an exact verified segment covering `instance`'s
    /// `local_frame`, via that instance's current source mapping.
    fn exact(&self, instance: InstanceId, local_frame: i64) -> Option<&Segment> {
        let inst = self.instances.get(&instance)?;
        let abs = inst.source_offset + local_frame;
        self.segments.iter().find(|s| s.source == inst.source && s.range.contains(&abs))
    }

    /// Level 2: walk outward from `(instance, local_frame)` up to the
    /// guess radius, across adjacency into neighbouring instances, and
    /// blend every verified segment found within reach.
    fn guess(&self, instance: InstanceId, local_frame: i64) -> Resolved {
        let mut weighted_importance = 0.0f32;
        let mut weight_sum = 0.0f32;
        let mut best_confidence = 0.0f32;

        for dir in [-1i64, 1i64] {
            if !self.instances.contains_key(&instance) {
                return Resolved::FALLBACK;
            }
            let mut cur = instance;
            let mut cur_frame = local_frame;
            let mut budget = self.radius.within_clip;
            let mut travelled = 0i64;
            let mut crossed = 0u32;
            loop {
                if budget <= 0 || crossed > 5 {
                    break;
                }
                let Some(cur_inst) = self.instances.get(&cur) else { break };
                let next_frame = cur_frame + dir;
                let crossing = if dir < 0 { next_frame < 0 } else { next_frame >= cur_inst.len };
                if crossing {
                    let neighbour = if dir < 0 { self.prev_instance(cur) } else { self.next_instance(cur) };
                    let Some(next_id) = neighbour else { break };
                    let Some(next_inst) = self.instances.get(&next_id) else { break };
                    cur = next_id;
                    cur_frame = if dir < 0 { next_inst.len - 1 } else { 0 };
                    crossed += 1;
                    budget = self.radius.across_clip;
                } else {
                    cur_frame = next_frame;
                    budget -= 1;
                }
                travelled += 1;

                if let Some(seg) = self.exact(cur, cur_frame) {
                    let clip_reach = if crossed == 0 { self.radius.within_clip } else { self.radius.across_clip };
                    let boundary_attenuation = 0.6f32.powi(crossed as i32);
                    let decay = (1.0 - travelled as f32 / clip_reach.max(1) as f32).max(0.0);
                    let w = decay * boundary_attenuation;
                    if w > 0.0 {
                        weighted_importance += seg.bucket.importance() * w;
                        weight_sum += w;
                        best_confidence = best_confidence.max(w);
                    }
                    break; // one hit per direction is enough evidence
                }
            }
        }

        if weight_sum <= 0.0 {
            return Resolved::FALLBACK;
        }
        Resolved { importance: (weighted_importance / weight_sum).clamp(0.0, 1.0), confidence: best_confidence.clamp(0.0, 1.0) }
    }

    /// Resolve one frame of one CURRENT timeline instance through all
    /// three levels. `instance` must be one `rebuild` was last called
    /// with, or this returns fallback (an instance `rebuild` doesn't know
    /// about has no placement to resolve through).
    pub fn resolve(&self, instance: InstanceId, local_frame: i64) -> Resolved {
        if !self.instances.contains_key(&instance) {
            return Resolved::FALLBACK;
        }
        if let Some(seg) = self.exact(instance, local_frame) {
            return Resolved { importance: seg.bucket.importance(), confidence: 1.0 };
        }
        self.guess(instance, local_frame)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn field() -> BucketField {
        BucketField::new(GuessRadius::default())
    }

    fn place(instance: InstanceId, source: SourceId, offset: i64, len: i64, track: u64, start: f64) -> Placement {
        Placement { instance, source, source_offset: offset, len, track, start, end: start + len as f64 }
    }

    #[test]
    fn an_exact_verified_segment_resolves_at_full_confidence() {
        let mut f = field();
        f.add_segment(Segment { source: 100, range: 0..40, bucket: Bucket::Face });
        f.rebuild(&[place(1, 100, 0, 200, 0, 0.0)], 0.0);
        let r = f.resolve(1, 20);
        assert_eq!(r.confidence, 1.0);
        assert_eq!(r.importance, Bucket::Face.importance());
    }

    #[test]
    fn a_gap_within_one_instance_decays_smoothly_away_from_the_known_segment() {
        let mut f = field();
        f.add_segment(Segment { source: 1, range: 0..4, bucket: Bucket::Face });
        f.rebuild(&[place(1, 1, 0, 200, 0, 0.0)], 0.0);
        let near = f.resolve(1, 8);
        let far = f.resolve(1, 40);
        assert!(near.confidence > 0.0);
        assert!(near.confidence > far.confidence, "near {} far {}", near.confidence, far.confidence);
    }

    #[test]
    fn guessing_across_an_instance_boundary_decays_faster_than_the_same_distance_within_one() {
        // Instance 1 plays 100 frames of source 1, with a Face segment at
        // its tail; instance 2 plays a DIFFERENT source with no segments
        // of its own and is placed immediately after instance 1.
        let mut across = field();
        across.add_segment(Segment { source: 1, range: 90..100, bucket: Bucket::Face });
        across.rebuild(&[place(1, 1, 0, 100, 0, 0.0), place(2, 2, 0, 500, 0, 100.0)], 0.0);
        let across_r = across.resolve(2, 5); // 5 frames into instance 2

        let mut within = field();
        within.add_segment(Segment { source: 1, range: 90..100, bucket: Bucket::Face });
        within.rebuild(&[place(1, 1, 0, 500, 0, 0.0)], 0.0); // one long instance, no boundary
        let within_r = within.resolve(1, 105); // same absolute distance, no crossing

        assert!(within_r.confidence > across_r.confidence);
        assert!(across_r.confidence > 0.0);
    }

    #[test]
    fn fallback_is_neutral_and_exact_for_an_unknown_instance() {
        let f = field();
        assert_eq!(f.resolve(999, 0), Resolved::FALLBACK);
    }

    // ---------------------------------------------------------------
    // The property this whole design exists for: driving the REAL editor
    // (arrange::cmd) through structural edits must never require touching
    // a segment, and adjacency must always reflect whatever the timeline
    // looks like AFTER the edit — never what it looked like before.
    // ---------------------------------------------------------------

    use crate::arrange::cmd::{Command, Editor};
    use crate::arrange::model::SourceRef;

    /// Turn the editor's current timeline into `Placement`s, exactly as a
    /// real caller would before calling `rebuild`. `SourceRef` (a
    /// `Beat(usize)`/`File(usize)`) is folded into one `u64` `SourceId`
    /// space so the two kinds never collide.
    fn placements_from(ed: &Editor) -> Vec<Placement> {
        let source_id = |r: SourceRef| -> SourceId {
            match r {
                SourceRef::Beat(i) => i as u64,
                SourceRef::File(i) => 0x8000_0000_0000_0000 | i as u64,
            }
        };
        // An arbitrary, fixed frames-per-beat for these tests, so a
        // clip's `len`/`offset` (in beats) convert to whole frame counts.
        const FPB: f64 = 100.0;
        ed.timeline
            .tracks
            .iter()
            .flat_map(|t| t.clips.iter().map(move |c| (t.id, c)))
            .map(|(track, c)| Placement {
                instance: c.id,
                source: source_id(c.source),
                source_offset: (c.offset * FPB).round() as i64,
                len: (c.len * FPB).round() as i64,
                track,
                start: c.start,
                end: c.end(),
            })
            .collect()
    }

    #[test]
    fn splitting_a_clip_leaves_every_frames_resolution_unchanged() {
        let mut e = Editor::default();
        let t0 = e.timeline.tracks[0].id;
        let a = e.apply(Command::Place { track: t0, source: SourceRef::Beat(0), start: 0.0, source_beats: 4.0, exact: false, lock: None }).unwrap().created[0];

        let mut field = field();
        // A segment covering the middle of the 400-frame source (4 beats
        // * 100 frames/beat in this test's arbitrary scale).
        field.add_segment(Segment { source: 0, range: 150..250, bucket: Bucket::Face });
        field.rebuild(&placements_from(&e), 0.0);
        let before: Vec<Resolved> = (0..400).map(|f| field.resolve(a, f)).collect();

        let r = e.apply(Command::Split { clip: a, beat: 2.0 }).unwrap();
        let b = r.created[0];
        field.rebuild(&placements_from(&e), 0.0);

        // The left half (instance `a`) still covers source frames 0..200,
        // the right half (instance `b`) covers 200..400 — same absolute
        // frames, same segment, resolved through whichever instance now
        // owns them. No segment was touched by the split.
        for f in 0..200i64 {
            assert_eq!(field.resolve(a, f), before[f as usize], "frame {f} after split (left half)");
        }
        for f in 200..400i64 {
            let local = f - 200;
            assert_eq!(field.resolve(b, local), before[f as usize], "frame {f} after split (right half, local {local})");
        }
    }

    #[test]
    fn trimming_a_clip_does_not_touch_segments_and_still_resolves_the_visible_frames_correctly() {
        let mut e = Editor::default();
        let t0 = e.timeline.tracks[0].id;
        let a = e.apply(Command::Place { track: t0, source: SourceRef::File(0), start: 0.0, source_beats: 4.0, exact: false, lock: None }).unwrap().created[0];

        let mut field = field();
        field.add_segment(Segment { source: 0x8000_0000_0000_0000, range: 50..150, bucket: Bucket::CutBoundary });
        field.rebuild(&placements_from(&e), 0.0);

        // Head-trim by one beat (100 frames at this test's scale): the
        // instance now plays source frames 100..400.
        e.apply(Command::Trim { clip: a, start: 1.0, len: 3.0, offset: 1.0 }).unwrap();
        field.rebuild(&placements_from(&e), 0.0);

        // Source frame 120 is now local frame 20 (120 - 100 offset), and
        // must still resolve to the same segment.
        assert_eq!(field.resolve(a, 20).importance, Bucket::CutBoundary.importance());
        assert_eq!(field.resolve(a, 20).confidence, 1.0);
    }

    #[test]
    fn moving_a_clip_changes_adjacency_but_never_a_segment() {
        let mut e = Editor::default();
        let t0 = e.timeline.tracks[0].id;
        let a = e.apply(Command::Place { track: t0, source: SourceRef::Beat(0), start: 0.0, source_beats: 2.0, exact: false, lock: None }).unwrap().created[0];
        e.apply(Command::Place { track: t0, source: SourceRef::Beat(1), start: 2.0, source_beats: 2.0, exact: false, lock: None }).unwrap();

        let mut field = field();
        field.add_segment(Segment { source: 1, range: 0..10, bucket: Bucket::Face }); // Beat(1)'s source
        field.rebuild(&placements_from(&e), 0.0);
        // a and its neighbour are adjacent (touching at beat 2); the
        // neighbour's Face segment is reachable by a guess from a's tail.
        let a_tail_before = field.resolve(a, 199); // last local frame of a (200 frames long)
        assert!(a_tail_before.confidence > 0.0, "a should see its neighbour's Face segment across the join");

        // A fresh arrangement where a's neighbour instead plays a source
        // with NO segments — same shape, different adjacency — proves the
        // guess result follows whatever `rebuild` was last given, not
        // anything remembered from before.
        let mut e2 = Editor::default();
        let t0b = e2.timeline.tracks[0].id;
        let a2 = e2.apply(Command::Place { track: t0b, source: SourceRef::Beat(0), start: 0.0, source_beats: 2.0, exact: false, lock: None }).unwrap().created[0];
        e2.apply(Command::Place { track: t0b, source: SourceRef::Beat(0), start: 2.0, source_beats: 2.0, exact: false, lock: None }).unwrap();
        field.rebuild(&placements_from(&e2), 0.0);
        let a_tail_after = field.resolve(a2, 199);
        assert_eq!(a_tail_after, Resolved::FALLBACK, "the new neighbour's source has no Face segment, so the guess must find nothing");
    }

    #[test]
    fn duplicating_a_clip_shares_its_sources_segments_for_free() {
        let mut e = Editor::default();
        let t0 = e.timeline.tracks[0].id;
        let a = e.apply(Command::Place { track: t0, source: SourceRef::Beat(0), start: 0.0, source_beats: 2.0, exact: false, lock: None }).unwrap().created[0];

        let mut field = field();
        field.add_segment(Segment { source: 0, range: 50..150, bucket: Bucket::Face });
        field.rebuild(&placements_from(&e), 0.0);
        let original = field.resolve(a, 100);

        let r = e.apply(Command::Duplicate { clips: vec![a], delta_beats: 2.0 }).unwrap();
        let dup = r.created[0];
        // No new segment was added anywhere — only the timeline changed.
        field.rebuild(&placements_from(&e), 0.0);
        let duplicate_resolution = field.resolve(dup, 100);

        assert_eq!(original, duplicate_resolution, "the duplicate shares its source's profile with zero extra bookkeeping");
    }

    #[test]
    fn deleting_a_clip_and_undoing_leaves_its_sources_segments_intact() {
        let mut e = Editor::default();
        let t0 = e.timeline.tracks[0].id;
        let a = e.apply(Command::Place { track: t0, source: SourceRef::Beat(0), start: 0.0, source_beats: 2.0, exact: false, lock: None }).unwrap().created[0];

        let mut field = field();
        field.add_segment(Segment { source: 0, range: 0..200, bucket: Bucket::Face });
        field.rebuild(&placements_from(&e), 0.0);
        assert_eq!(field.resolve(a, 50).confidence, 1.0);

        e.apply(Command::Delete { clips: vec![a] }).unwrap();
        field.rebuild(&placements_from(&e), 0.0);
        // The instance is gone; resolving it is a defined fallback, not a
        // panic, and the segment itself is untouched (still there for
        // whatever NEW instance next plays source 0).
        assert_eq!(field.resolve(a, 50), Resolved::FALLBACK);

        e.undo();
        field.rebuild(&placements_from(&e), 0.0);
        assert_eq!(field.resolve(a, 50).confidence, 1.0, "the segment survived the delete/undo round trip untouched");
    }
}
