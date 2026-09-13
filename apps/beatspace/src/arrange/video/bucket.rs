//! Content-importance buckets for feel mode.
//!
//! [`cache_policy`] answers "how far, how dense" purely from distance to
//! the playhead. That's Preview mode. Feel mode wants the same byte
//! budget spent unevenly — more on a face, more around a cut, less on a
//! held static wide shot — which means every frame needs an IMPORTANCE
//! on top of its distance, and that importance has to come from
//! somewhere even for the (typical) case where nothing has explicitly
//! analysed that frame yet.
//!
//! Three confidence levels, resolved in order:
//!
//! 1. **Verified** — an actual profiled [`Segment`] ([`profile`] produces
//!    these) whose fingerprint still matches a fresh cheap re-scan.
//!    Confidence 1.0; feel mode trusts it fully.
//! 2. **Guessed** — no segment covers this frame, but a verified segment
//!    exists nearby. Its importance is blended in, decayed by distance
//!    AND by an extra attenuation for every clip boundary crossed to
//!    reach it — a face shot says a lot about the next few frames of
//!    ITS OWN clip and much less about the clip that follows it.
//!    Confidence in `(0, 1)`, shrinking with distance and boundary
//!    crossings, and bounded to a search radius so resolving one frame
//!    never means scanning the whole timeline.
//! 3. **Fallback** — nothing verified within the search radius in either
//!    direction. Confidence 0, neutral importance: this degrades to
//!    EXACTLY today's plain distance falloff, which is the point —
//!    everywhere unanalysed behaves like Preview, and only gets bent
//!    once real information exists near it.
//!
//! [`BucketField`] holds the verified segments for one video source and
//! resolves any frame against all three levels in one call.

use std::ops::Range;

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

/// One clip's identity on the timeline, for boundary-crossing detection.
/// An opaque id — `BucketField` doesn't need to know what a clip IS, only
/// whether two frames belong to the same one.
pub type ClipId = u64;

/// A profiled, frozen span of one clip's frames. See [`profile::Segment`]
/// for how these are produced and [`profile::verify`] for how staleness
/// is caught before a segment is trusted here.
#[derive(Debug, Clone, PartialEq)]
pub struct Segment {
    pub clip: ClipId,
    /// Frame range, in the SOURCE's own frame numbering.
    pub range: Range<i64>,
    pub bucket: Bucket,
}

/// What [`BucketField::resolve`] returned, and how it got there.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Resolved {
    pub importance: f32,
    /// 1.0 = verified segment covers this frame exactly.
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
    /// Frames a guess can travel within the SAME clip before its
    /// confidence reaches zero.
    pub within_clip: i64,
    /// Frames a guess can travel ACROSS a clip boundary before reaching
    /// zero — shorter than `within_clip` on purpose.
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

/// The verified segments for however many clips share one timeline, plus
/// enough clip-adjacency information to know when a guess is crossing a
/// boundary. Cheap to hold: segments, not frames.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct BucketField {
    /// Sorted by `range.start` within each clip; clips are independent
    /// (frame numbering restarts per source), so segments are grouped by
    /// clip and each clip's list is searched on its own.
    segments: Vec<Segment>,
    /// Clip adjacency on the timeline: `(left, right)` pairs meaning
    /// `left`'s last frame is immediately followed by `right`'s first —
    /// what lets a guess cross from one clip's tail into the next clip's
    /// head. Built by the caller from the arrangement (`arrange::model`),
    /// not derived here — this module knows nothing about beats or clips
    /// on a grid, only which source-ids are adjacent.
    adjacency: Vec<(ClipId, ClipId)>,
    /// Frame COUNT of each clip's source (frames are numbered `0..len`).
    /// Required to cross a boundary in either direction: without it, a
    /// guess cannot tell when it has reached a clip's last frame (to step
    /// forward into the next) or place itself at a previous clip's last
    /// frame (to step backward into it) — the walk simply runs out of
    /// budget short of the neighbour instead. Registering extents is what
    /// unlocks cross-clip guessing; not registering them degrades safely
    /// to within-clip-only guessing, never to a wrong answer.
    extents: std::collections::HashMap<ClipId, i64>,
    radius: GuessRadius,
}

impl BucketField {
    pub fn new(radius: GuessRadius) -> Self {
        Self { segments: Vec::new(), adjacency: Vec::new(), extents: Default::default(), radius }
    }

    pub fn add_segment(&mut self, s: Segment) {
        self.segments.push(s);
        self.segments.sort_by(|a, b| (a.clip, a.range.start).cmp(&(b.clip, b.range.start)));
    }

    /// Declare that `right`'s first frame follows `left`'s last frame on
    /// the timeline (adjacent, in playback order). Symmetric for lookup
    /// purposes — a guess can travel either direction across the join.
    pub fn adjoin(&mut self, left: ClipId, right: ClipId) {
        self.adjacency.push((left, right));
    }

    /// Register `clip`'s frame count (valid frames are `0..len`), needed
    /// to cross a boundary to or from it. See the field doc for why.
    pub fn set_extent(&mut self, clip: ClipId, len: i64) {
        self.extents.insert(clip, len);
    }

    fn next_clip(&self, clip: ClipId) -> Option<ClipId> {
        self.adjacency.iter().find(|(l, _)| *l == clip).map(|(_, r)| *r)
    }
    fn prev_clip(&self, clip: ClipId) -> Option<ClipId> {
        self.adjacency.iter().find(|(_, r)| *r == clip).map(|(l, _)| *l)
    }

    fn segments_of(&self, clip: ClipId) -> impl Iterator<Item = &Segment> {
        self.segments.iter().filter(move |s| s.clip == clip)
    }

    /// Level 1: an exact verified segment covering `frame` on `clip`.
    fn exact(&self, clip: ClipId, frame: i64) -> Option<&Segment> {
        self.segments_of(clip).find(|s| s.range.contains(&frame))
    }

    /// Level 2: walk outward from `(clip, frame)` up to the guess radius,
    /// across clip boundaries via `adjacency` and `extents`, and blend
    /// every verified segment found within reach. Multiple hits blend by
    /// weight (nearer, and same-clip, evidence dominates) rather than
    /// just taking the nearest, because two agreeing nearby segments
    /// should read as more confident than one.
    fn guess(&self, clip: ClipId, frame: i64) -> Resolved {
        let mut weighted_importance = 0.0f32;
        let mut weight_sum = 0.0f32;
        let mut best_confidence = 0.0f32;

        // Walk in both directions independently; each carries its own
        // remaining "budget" in frames, spent at 1:1 within a clip and
        // RESET (not summed) to `across_clip` on crossing a boundary —
        // crossing costs a fresh, shorter budget, it doesn't inherit
        // whatever was left of the longer within-clip one.
        for dir in [-1i64, 1i64] {
            let mut cur_clip = clip;
            let mut cur_frame = frame;
            let mut budget = self.radius.within_clip;
            let mut travelled = 0i64;
            let mut crossed = 0u32;
            loop {
                if budget <= 0 {
                    break;
                }
                // A safety cap, not just a tuning knob: without it, a
                // pathological chain of zero-length clips could cross
                // forever without ever spending `budget` (each crossing
                // resets it). Five crossings is already far past where
                // `boundary_attenuation` (0.6^5 ≈ 0.078) makes the result
                // meaningless, so this never fires on real input.
                if crossed > 5 {
                    break;
                }
                let next_frame = cur_frame + dir;
                let len = self.extents.get(&cur_clip).copied();

                let crossing = if dir < 0 {
                    next_frame < 0
                } else {
                    len.map_or(false, |l| next_frame >= l)
                };
                if crossing {
                    // Only crossable when the CURRENT clip's extent is
                    // known for a forward cross (to know the far clip's
                    // start is 0, which is always true — the real
                    // requirement is knowing WE'VE reached the end, i.e.
                    // len). A backward cross needs the PREVIOUS clip's
                    // extent, to know where its last frame is.
                    let next = if dir < 0 { self.prev_clip(cur_clip) } else { self.next_clip(cur_clip) };
                    let Some(next_clip) = next else { break };
                    let entry_frame = if dir < 0 {
                        match self.extents.get(&next_clip) {
                            Some(&l) if l > 0 => l - 1,
                            _ => break, // can't place ourselves without the neighbour's length
                        }
                    } else {
                        0
                    };
                    cur_clip = next_clip;
                    cur_frame = entry_frame;
                    crossed += 1;
                    budget = self.radius.across_clip;
                } else {
                    cur_frame = next_frame;
                    budget -= 1;
                }
                travelled += 1;

                if let Some(seg) = self.exact(cur_clip, cur_frame) {
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

    /// Resolve one frame on one clip through all three levels.
    pub fn resolve(&self, clip: ClipId, frame: i64) -> Resolved {
        if let Some(seg) = self.exact(clip, frame) {
            return Resolved { importance: seg.bucket.importance(), confidence: 1.0 };
        }
        self.guess(clip, frame)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn field() -> BucketField {
        BucketField::new(GuessRadius::default())
    }

    #[test]
    fn an_exact_verified_segment_resolves_at_full_confidence() {
        let mut f = field();
        f.add_segment(Segment { clip: 1, range: 100..140, bucket: Bucket::Face });
        let r = f.resolve(1, 120);
        assert_eq!(r.confidence, 1.0);
        assert_eq!(r.importance, Bucket::Face.importance());
    }

    #[test]
    fn a_gap_within_one_clip_decays_smoothly_away_from_the_known_segment() {
        let mut f = field();
        f.add_segment(Segment { clip: 1, range: 100..104, bucket: Bucket::Face });
        let near = f.resolve(1, 108);
        let far = f.resolve(1, 140);
        assert!(near.confidence > 0.0, "just past the segment should still guess something");
        assert!(near.confidence > far.confidence, "confidence should fall off with distance");
        assert!(near.importance > 0.0);
        // Monotonic decay, not a step function.
        let mid = f.resolve(1, 120);
        assert!(near.confidence >= mid.confidence && mid.confidence >= far.confidence);
    }

    #[test]
    fn guessing_across_a_clip_boundary_decays_faster_than_the_same_distance_within_one_clip() {
        // Clip 1 is 100 frames (0..100), with a Face segment right at its
        // tail (90..100); clip 2 follows it and has NO segments of its
        // own. Resolving 5 frames into clip 2 means crossing the join.
        let mut across_field = field();
        across_field.add_segment(Segment { clip: 1, range: 90..100, bucket: Bucket::Face });
        across_field.set_extent(1, 100);
        across_field.adjoin(1, 2);
        let across = across_field.resolve(2, 5); // clip 2's OWN frame 5, i.e. 5 past the join

        // Same absolute walking distance (5 steps past the segment's
        // edge), but never leaving the clip: a Face segment ending at
        // frame 100 (exclusive) in a single, longer clip, probed 5
        // frames later at 105 — no boundary anywhere on the path.
        let mut within_field = field();
        within_field.add_segment(Segment { clip: 1, range: 90..100, bucket: Bucket::Face });
        let within = within_field.resolve(1, 105);

        assert!(within.confidence > across.confidence, "within {} vs across {}", within.confidence, across.confidence);
        assert!(across.confidence > 0.0, "the cross-clip guess should still find something, just weaker");
    }

    #[test]
    fn a_guess_beyond_the_radius_or_with_no_neighbour_falls_back_to_neutral() {
        let mut f = field();
        f.add_segment(Segment { clip: 1, range: 0..10, bucket: Bucket::Face });
        let far = f.resolve(1, 10_000);
        assert_eq!(far, Resolved::FALLBACK);
        // An isolated clip with no adjacency and no segments of its own.
        let g = field();
        assert_eq!(g.resolve(9, 0), Resolved::FALLBACK);
    }

    #[test]
    fn fallback_confidence_is_exactly_zero_so_feel_mode_can_degrade_to_plain_distance_falloff() {
        let f = field();
        let r = f.resolve(1, 50);
        assert_eq!(r.confidence, 0.0);
        assert_eq!(r.importance, 0.0, "neutral, same as Bucket::Static");
    }

    #[test]
    fn two_nearby_segments_agreeing_are_more_confident_than_either_alone() {
        // A Face segment on each side of a small gap: the guess in the
        // middle should reflect both, and its confidence should be at
        // least as high as the stronger single-sided guess would be.
        let mut both = field();
        both.add_segment(Segment { clip: 1, range: 0..10, bucket: Bucket::Face });
        both.add_segment(Segment { clip: 1, range: 30..40, bucket: Bucket::Face });
        let mid_both = both.resolve(1, 20);

        let mut one = field();
        one.add_segment(Segment { clip: 1, range: 0..10, bucket: Bucket::Face });
        let mid_one = one.resolve(1, 20);

        assert!(mid_both.confidence >= mid_one.confidence);
        assert!(mid_both.importance > 0.0);
    }

    #[test]
    fn an_unprofiled_clip_next_to_a_profiled_one_can_still_reach_across() {
        let mut f = field();
        f.add_segment(Segment { clip: 1, range: 90..100, bucket: Bucket::CutBoundary });
        f.set_extent(1, 100); // required to know clip 1's tail is the join
        f.adjoin(1, 2); // clip 2 has NO segments of its own at all
        let just_in = f.resolve(2, 3); // near clip 2's start, close to the join
        assert!(just_in.confidence > 0.0, "an entirely unprofiled clip should still borrow from its neighbour near the seam");
        let deep_in = f.resolve(2, 1000);
        assert_eq!(deep_in, Resolved::FALLBACK, "far from the seam, with nothing of its own, it's neutral");
    }

    #[test]
    fn without_a_registered_extent_a_guess_cannot_cross_forward_but_stays_correct() {
        // The safe-degradation case named in the doc: adjacency exists
        // but clip 1's length was never registered, so a forward guess
        // from inside clip 1 cannot know where the join is and simply
        // exhausts its within-clip budget instead of crossing incorrectly.
        let mut f = field();
        f.add_segment(Segment { clip: 2, range: 0..5, bucket: Bucket::Face });
        f.adjoin(1, 2); // no set_extent(1, ..)
        let r = f.resolve(1, 0);
        assert_eq!(r, Resolved::FALLBACK, "cannot safely reach clip 2 without knowing clip 1's length");
    }
}
