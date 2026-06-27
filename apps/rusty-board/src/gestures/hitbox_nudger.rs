//! Hitbox nudging — the "subtle gesture manipulation" piece.
//!
//! ## What this is
//!
//! During a swipe, the system has a rough idea of what word the
//! user is heading toward (from the shape recognizer's current
//! top candidates). If the top candidates are `["store", "stone",
//! "story"]` and the user has just passed through `s, t, o`, then
//! the NEXT key they're aiming for is almost certainly one of
//! `r, n, r` → strongly `r`.
//!
//! Instead of just predicting and waiting, we apply that prediction
//! to the path-decoding step: keys we expect the user to pass
//! through get a small "magnetic" pull (effective distance reduced
//! by up to 20%). When the user's actual finger trajectory wobbles
//! slightly between two adjacent keys, the nudge tips it toward the
//! predicted one.
//!
//! ## Why this is better than just better recognition at the end
//!
//! The shape recognizer at drag_end already does post-hoc
//! disambiguation. Nudging is DIFFERENT: it changes what raw key
//! sequence comes out of `decode_swipe_path` in the first place.
//! If the raw decode is wrong, the recognizer's candidate list is
//! built from a wrong starting point — the right word might not
//! even be in the top-5.
//!
//! Nudging recovers cases where:
//!   - User intended `r` but their path's pivot landed slightly
//!     closer to `t`. Without nudging, raw decode = "stot" and the
//!     dictionary lookup for "stor*" words is degraded. With
//!     nudging biased toward `r` (predicted from candidates), pivot
//!     snaps to `r`, raw decode = "stor", lookup finds "store"
//!     cleanly.
//!
//! ## What this is NOT
//!
//! Not a UI feedback feature. Nudges aren't visible to the user.
//! There's no "the keyboard highlighted r and I tapped it" — the
//! nudging is purely internal to path decoding. Users perceive it
//! as "the keyboard just feels more forgiving."
//!
//! ## Risk: feedback loops
//!
//! If candidates → nudges → different raw decode → different
//! candidates → different nudges, the system could oscillate or
//! lock onto a wrong word that "feels right" by self-reinforcement.
//! Mitigations:
//!   - Nudges are capped at 1.2× (max 20% boost). Strong actual
//!     signal still wins.
//!   - Candidates used for nudging are computed from the UNNUDGED
//!     raw decode (the previous tick's), so this tick's nudges
//!     can't influence this tick's candidates.
//!   - Maximum N candidates considered per tick (currently 5) —
//!     bounded influence.

use crate::gestures::recognizer::WordCandidate;
use crate::gestures::swipe::{KeyHitbox, SwipePoint};
use std::collections::HashMap;

/// Maximum nudge multiplier. A nudge of 1.2 means "this key's
/// effective distance is divided by 1.2" → ~17% margin gain over
/// adjacent keys.
const MAX_NUDGE: f64 = 1.20;

/// Per-candidate weight cap. Without this, having 5 candidates
/// all predicting the same next-char could stack to absurd nudges.
const PER_CANDIDATE_WEIGHT_CAP: f64 = 0.05;

/// Maximum candidates to consider when computing nudges. Beyond
/// this, contributions get vanishingly small AND cost grows.
const MAX_CANDIDATES_CONSIDERED: usize = 5;

/// A bag of per-key nudge multipliers. Empty = no nudges (pure
/// pass-through of the existing snap logic). Lookup key is the
/// key label, lowercased.
#[derive(Debug, Default, Clone)]
pub struct HitboxNudger {
    nudges: HashMap<char, f64>,
}

impl HitboxNudger {
    /// Empty nudger — no key gets any boost. Equivalent to the
    /// existing un-nudged behavior.
    pub fn new() -> Self {
        Self::default()
    }

    /// Recompute nudges from the current candidate list and how
    /// many characters the user has decoded so far. Replaces any
    /// previous nudge state.
    ///
    /// `chars_so_far` is the length of the raw-decoded sequence
    /// from the path up to NOW (excluding the next predicted
    /// character). For an empty path it's 0 and we predict the
    /// FIRST character; for a path that's so far decoded to
    /// "sto" it's 3 and we predict character index 3 of each
    /// candidate.
    pub fn update_from_candidates(
        &mut self,
        candidates: &[WordCandidate],
        chars_so_far: usize,
    ) {
        self.nudges.clear();
        if candidates.is_empty() {
            return;
        }

        // For each candidate, find the next char it predicts.
        // Weight the contribution inversely with the candidate's
        // recognizer score (lower score → higher weight, since
        // recognizer scores are lower-is-better).
        //
        // Normalize against the best candidate so candidates with
        // similar scores all contribute meaningfully, but
        // candidates that are way worse don't move the needle.
        let best_score = candidates[0].score.max(0.001);

        for cand in candidates.iter().take(MAX_CANDIDATES_CONSIDERED) {
            let Some(next_ch) = cand.word.chars().nth(chars_so_far) else {
                continue;
            };
            let next_ch = next_ch.to_ascii_lowercase();

            // Weight: best candidate gets PER_CANDIDATE_WEIGHT_CAP,
            // worse candidates linearly less. A candidate twice as
            // bad gets half the weight. Floor at 0.
            let ratio = (best_score / cand.score.max(0.001)).clamp(0.0, 1.0);
            let weight = ratio * PER_CANDIDATE_WEIGHT_CAP;
            *self.nudges.entry(next_ch).or_insert(0.0) += weight;
        }

        // Convert accumulated weights to multiplicative nudges and
        // cap. A weight of 0 stays 1.0 (no boost); higher weights
        // grow toward MAX_NUDGE.
        for v in self.nudges.values_mut() {
            *v = (1.0 + *v).min(MAX_NUDGE);
        }
    }

    /// Look up the nudge multiplier for a key by its label's first
    /// character. Returns 1.0 (no boost) for un-nudged keys.
    pub fn nudge_for(&self, label: &str) -> f64 {
        label
            .chars()
            .next()
            .map(|c| c.to_ascii_lowercase())
            .and_then(|c| self.nudges.get(&c))
            .copied()
            .unwrap_or(1.0)
    }

    /// True if the nudger has no active nudges (e.g. just-
    /// constructed, or candidates were empty). Useful as a
    /// short-circuit in the swipe handler.
    pub fn is_empty(&self) -> bool {
        self.nudges.is_empty() || self.nudges.values().all(|&v| v <= 1.0001)
    }

    /// Number of distinct keys with active nudges. Diagnostics.
    pub fn nudged_key_count(&self) -> usize {
        self.nudges.values().filter(|&&v| v > 1.0001).count()
    }
}

/// Snap a point to the nudge-weighted nearest key. Equivalent to
/// the existing `min_by_distance` logic but with each key's
/// effective distance divided by its nudge factor.
///
/// Returns the key's label (already lowercased per existing
/// snap behavior). None if no swipe-target keys exist.
pub fn snap_with_nudge<'a>(
    x: f64,
    y: f64,
    keys: &'a [KeyHitbox],
    nudger: &HitboxNudger,
) -> Option<&'a KeyHitbox> {
    keys.iter()
        .filter(|k| k.key.swipe_target)
        .min_by(|a, b| {
            let da = a.distance_to(x, y) / nudger.nudge_for(&a.key.label);
            let db = b.distance_to(x, y) / nudger.nudge_for(&b.key.label);
            da.partial_cmp(&db).unwrap_or(std::cmp::Ordering::Equal)
        })
}

/// Decode a swipe path using nudge-weighted snapping. Mirrors
/// `swipe::pivots_to_word` but with the per-key nudge applied at
/// the distance-comparison step. The pivot extraction is identical
/// to the existing path — we only nudge the snap.
///
/// When `nudger.is_empty()`, this is byte-for-byte equivalent to
/// `decode_swipe_path` (the nudge of 1.0 leaves distances unchanged
/// up to floating-point identity).
pub fn decode_with_nudge(
    path: &[SwipePoint],
    keys: &[KeyHitbox],
    nudger: &HitboxNudger,
) -> String {
    if path.is_empty() || keys.is_empty() {
        return String::new();
    }
    let pivots = crate::gestures::swipe::extract_pivots(path);
    let mut word = String::new();
    let mut last_key_label: Option<String> = None;

    for point in &pivots {
        let Some(hit) = snap_with_nudge(point.x, point.y, keys, nudger) else {
            continue;
        };
        let label = hit.key.label.clone();
        if last_key_label.as_ref() != Some(&label) {
            word.push_str(&label);
            last_key_label = Some(label);
        }
    }
    word
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gestures::recognizer::WordCandidate;
    use crate::keyboard::Key;

    fn cand(word: &str, score: f64) -> WordCandidate {
        WordCandidate {
            word: word.to_string(),
            score,
        }
    }

    fn key(label: &str, x: f64, y: f64) -> KeyHitbox {
        KeyHitbox {
            key: Key::character(label),
            x,
            y,
            width: 50.0,
            height: 50.0,
        }
    }

    #[test]
    fn empty_nudger_no_boost() {
        let n = HitboxNudger::new();
        assert_eq!(n.nudge_for("a"), 1.0);
        assert_eq!(n.nudge_for("z"), 1.0);
        assert!(n.is_empty());
        assert_eq!(n.nudged_key_count(), 0);
    }

    #[test]
    fn no_candidates_clears_nudges() {
        let mut n = HitboxNudger::new();
        n.update_from_candidates(&[cand("hi", 0.1)], 0);
        assert!(!n.is_empty());
        n.update_from_candidates(&[], 0);
        assert!(n.is_empty());
    }

    #[test]
    fn next_char_gets_boosted() {
        let mut n = HitboxNudger::new();
        let cs = vec![
            cand("store", 0.1),
            cand("stone", 0.15),
            cand("story", 0.2),
        ];
        // chars_so_far=3 → next is 'r', 'n', 'r'
        n.update_from_candidates(&cs, 3);
        // 'r' appears twice → bigger nudge than 'n'
        assert!(n.nudge_for("r") > n.nudge_for("n"));
        // Both > 1.0
        assert!(n.nudge_for("r") > 1.0);
        assert!(n.nudge_for("n") > 1.0);
        // Other keys are not boosted
        assert_eq!(n.nudge_for("x"), 1.0);
        assert_eq!(n.nudge_for("a"), 1.0);
    }

    #[test]
    fn nudge_capped_at_max() {
        let mut n = HitboxNudger::new();
        // 5 candidates ALL predicting 'r' as next char with the
        // same low score — extreme reinforcement.
        let cs = vec![
            cand("r_one", 0.01),
            cand("r_two", 0.01),
            cand("r_three", 0.01),
            cand("r_four", 0.01),
            cand("r_five", 0.01),
        ];
        n.update_from_candidates(&cs, 0);
        assert!(n.nudge_for("r") <= MAX_NUDGE + 1e-9);
        // Still positive even with cap.
        assert!(n.nudge_for("r") > 1.0);
    }

    #[test]
    fn candidates_beyond_path_dont_contribute() {
        let mut n = HitboxNudger::new();
        // Both candidates are SHORTER than the path so far.
        let cs = vec![cand("ab", 0.1), cand("xy", 0.2)];
        n.update_from_candidates(&cs, 5);
        // No nudges since neither has a char at index 5.
        assert!(n.is_empty());
    }

    #[test]
    fn case_insensitive() {
        let mut n = HitboxNudger::new();
        n.update_from_candidates(&[cand("Apple", 0.1)], 0);
        // 'A' → lowercase 'a'
        let a = n.nudge_for("a");
        let big_a = n.nudge_for("A");
        assert_eq!(a, big_a);
        assert!(a > 1.0);
    }

    #[test]
    fn worse_candidates_contribute_less() {
        let mut n = HitboxNudger::new();
        let cs = vec![
            cand("apple", 0.1),  // score 0.1 → next char 'a'... wait,
            cand("banana", 5.0), // chars_so_far=0 means next is 'a' and 'b'
        ];
        n.update_from_candidates(&cs, 0);
        // 'a' has the best candidate; 'b' has only a much worse one.
        let na = n.nudge_for("a");
        let nb = n.nudge_for("b");
        assert!(na > nb);
    }

    #[test]
    fn nudge_count_diagnostic() {
        let mut n = HitboxNudger::new();
        let cs = vec![
            cand("apple", 0.1),
            cand("banana", 0.2),
            cand("cherry", 0.3),
        ];
        n.update_from_candidates(&cs, 0);
        // Three distinct first chars → three nudges.
        assert_eq!(n.nudged_key_count(), 3);
    }

    #[test]
    fn nudger_clone_independent() {
        let mut n1 = HitboxNudger::new();
        n1.update_from_candidates(&[cand("foo", 0.1)], 0);
        let n2 = n1.clone();
        assert_eq!(n1.nudge_for("f"), n2.nudge_for("f"));
        // Mutating n1 doesn't affect n2.
        n1.update_from_candidates(&[], 0);
        assert!(n1.is_empty());
        assert!(!n2.is_empty());
    }

    // ──── snap_with_nudge / decode_with_nudge ────────────────────

    #[test]
    fn snap_falls_through_to_nearest_when_empty() {
        // Two keys, 'a' at (0,0) and 'b' at (100,0). Point at (40,0).
        // Without nudges, 'a' is closer (dist 40 vs 60).
        let keys = vec![key("a", 0.0, 0.0), key("b", 100.0, 0.0)];
        let n = HitboxNudger::new();
        let hit = snap_with_nudge(40.0, 0.0, &keys, &n).unwrap();
        assert_eq!(hit.key.label, "a");
    }

    #[test]
    fn snap_can_flip_with_strong_nudge() {
        // Same setup; point at (45,0). Without nudge: 'a' wins (45
        // vs 55). With strong nudge on 'b': 'b' should win.
        let keys = vec![key("a", 0.0, 0.0), key("b", 100.0, 0.0)];
        let mut n = HitboxNudger::new();
        // Top candidate "ab" at chars_so_far=1 → next char 'b'.
        n.update_from_candidates(&[cand("ab", 0.01)], 1);
        // Verify the nudge is meaningful (> 1)
        assert!(n.nudge_for("b") > 1.0);
        // But: the cap is 1.05 with a single candidate at top
        // score, so effective dist for 'b' is 55/1.05 ≈ 52.4. 'a'
        // still has 45/1.0 = 45. 'a' still wins. This documents
        // that single-candidate single-cap nudges are subtle by
        // design.
        let hit = snap_with_nudge(45.0, 0.0, &keys, &n).unwrap();
        assert_eq!(hit.key.label, "a");
    }

    #[test]
    fn snap_flips_with_reinforced_nudge_near_threshold() {
        // Two adjacent keys, 50px wide each, side-by-side at y=0.
        // 'a' at x=0..50 (center 25, 25), 'b' at x=50..100 (center
        // 75, 25). Query point (50, 25) — exactly equidistant
        // from both centers (distance 25 each).
        //
        // Without nudge: ties go to whichever the min_by visits
        // first, which is non-deterministic-looking but happens
        // to be 'a' here (iteration order).
        //
        // With reinforced 'b' nudge (~1.15): 'b' effective = 25/1.15
        // ≈ 21.7, 'a' effective = 25/1.0 = 25.  'b' wins.
        let keys = vec![key("a", 0.0, 0.0), key("b", 50.0, 0.0)];
        let mut n = HitboxNudger::new();
        // Five candidates all predicting 'b' next, all top-tier
        // scores (so each contributes the full PER_CANDIDATE_WEIGHT_CAP).
        let cs = vec![
            cand("ab", 0.05),
            cand("ab", 0.05),
            cand("abby", 0.05),
            cand("able", 0.05),
            cand("abide", 0.05),
        ];
        n.update_from_candidates(&cs, 1);
        // 5 candidates × 0.05 cap = 0.25 weight → nudge = min(1.25, 1.20) = 1.20
        assert!(
            n.nudge_for("b") >= 1.15,
            "expected reinforced nudge on 'b', got {}",
            n.nudge_for("b")
        );
        let hit = snap_with_nudge(50.0, 25.0, &keys, &n).unwrap();
        assert_eq!(hit.key.label, "b");
    }

    #[test]
    fn decode_with_empty_nudger_matches_unnudged() {
        // Two pivots at points clearly snapping to specific keys.
        // Use the swipe module's own pivot extraction by passing
        // a synthetic path of two well-separated points.
        let keys = vec![
            key("h", 0.0, 0.0),
            key("i", 200.0, 0.0),
        ];
        let path = vec![
            SwipePoint::with_timestamp(0.0, 0.0, 0),
            SwipePoint::with_timestamp(200.0, 0.0, 100),
        ];
        let n = HitboxNudger::new();
        let nudged = decode_with_nudge(&path, &keys, &n);
        let plain = crate::gestures::swipe::decode_swipe_path(&path, &keys);
        assert_eq!(nudged, plain);
    }
}
