//! nudge — predictive magnetism applied DURING path decoding.
//!
//! `src/gestures/hitbox_nudger.rs` was unported, and it is the piece that makes
//! a swipe keyboard feel forgiving rather than merely accurate.
//!
//! ## Why it is not the same as better recognition at the end
//!
//! The shape recognizer already disambiguates at `drag_end`. Nudging happens
//! earlier and changes something the recognizer cannot fix: **what raw key
//! sequence comes out of the path decode in the first place.** The original
//! states the failure exactly — a path aiming at `r` whose pivot lands nearer
//! `t` decodes as `stot`, and the dictionary lookup for `stor*` is degraded, so
//! *the right word may not even be in the top five*. No amount of post-hoc
//! ranking recovers a candidate that was never generated.
//!
//! ## Three caps, and they exist to stop a feedback loop
//!
//! Candidates produce nudges, nudges change the decode, a changed decode
//! produces different candidates. Left alone that oscillates, or worse locks
//! onto a wrong word that feels right by self-reinforcement. The original's
//! mitigations are ported exactly because they are the whole safety argument:
//!
//! * **A 1.2× ceiling.** Strong actual signal still wins; the nudge tips a
//!   wobble, it does not overrule a deliberate movement.
//! * **A per-candidate weight cap**, so one confident wrong candidate cannot
//!   supply the whole boost on its own.
//! * **Candidates come from the UNNUDGED decode** — the previous tick's — so
//!   this tick's nudges cannot influence the candidates that produced them.
//!
//! ## It is invisible, deliberately
//!
//! *"Not a UI feedback feature. Nudges aren't visible to the user."* No key
//! lights up. Showing the prediction would turn a forgiving keyboard into one
//! that argues with you, and would make every wrong guess a visible mistake
//! rather than a silently-absorbed one.

/// Ceiling on the magnetic boost: at most a 20% reduction in effective
/// distance.
pub const MAX_NUDGE: f64 = 1.20;

/// The most any single candidate may contribute.
pub const PER_CANDIDATE_WEIGHT_CAP: f64 = 0.05;

/// How many candidates are consulted.
pub const MAX_CANDIDATES_CONSIDERED: usize = 5;

/// Accumulated pull per key label.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Nudges {
    keys: Vec<(String, f64)>,
}

impl Nudges {
    pub fn is_empty(&self) -> bool { self.keys.is_empty() }
    pub fn nudged_key_count(&self) -> usize { self.keys.len() }

    /// Rebuild from the current candidate list and how far the path has got.
    ///
    /// `consumed` is the number of keys already decoded; the nudge targets the
    /// NEXT character of each candidate. Targeting the current one would pull
    /// toward a key the finger has already left.
    pub fn update_from_candidates(&mut self, candidates: &[(String, f64)],
                                  consumed: usize) {
        self.keys.clear();
        for (word, score) in candidates.iter().take(MAX_CANDIDATES_CONSIDERED) {
            let Some(next) = word.chars().nth(consumed) else { continue };
            // A candidate cannot contribute more than the cap however confident
            // it is, so one wrong-but-certain word cannot supply the entire
            // boost by itself.
            let w = score.clamp(0.0, 1.0) * PER_CANDIDATE_WEIGHT_CAP;
            let label = next.to_lowercase().to_string();
            match self.keys.iter_mut().find(|(k, _)| *k == label) {
                // Candidates agreeing on the next key SUM — that agreement is
                // the strongest evidence available and is exactly the case the
                // feature exists for ("store/stone/story" → r, n, r).
                Some((_, acc)) => *acc += w,
                None => self.keys.push((label, w)),
            }
        }
    }

    /// The multiplier for a key, always within `1.0..=MAX_NUDGE`.
    pub fn nudge_for(&self, label: &str) -> f64 {
        let l = label.to_lowercase();
        let acc = self.keys.iter().find(|(k, _)| *k == l).map(|(_, w)| *w).unwrap_or(0.0);
        (1.0 + acc).min(MAX_NUDGE)
    }

    /// Effective distance to a key: real distance divided by its pull.
    ///
    /// Division rather than subtraction so the effect scales — a fixed offset
    /// would swamp a near miss and do nothing for a far one, which is backwards.
    pub fn effective_distance(&self, label: &str, distance: f64) -> f64 {
        distance / self.nudge_for(label)
    }

    /// Pick the nearest key by NUDGED distance.
    ///
    /// Ties resolve to the first candidate in input order, so the result does
    /// not depend on hash iteration order — a keyboard that decoded differently
    /// between runs would be untestable and would feel haunted.
    pub fn snap<'a>(&self, keys: &'a [(&'a str, f64)]) -> Option<&'a str> {
        keys.iter()
            .map(|(label, d)| (*label, self.effective_distance(label, *d)))
            .fold(None::<(&str, f64)>, |best, (label, d)| match best {
                Some((_, bd)) if bd <= d => best,
                _ => Some((label, d)),
            })
            .map(|(label, _)| label)
    }

    pub fn clear(&mut self) { self.keys.clear(); }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn candidates() -> Vec<(String, f64)> {
        // The original's own example: after s, t, o the next key is r, n, r.
        vec![("store".into(), 1.0), ("stone".into(), 0.9), ("story".into(), 0.8)]
    }

    #[test]
    fn agreeing_candidates_pull_the_same_key_harder() {
        // "store" and "story" both want `r` next; that agreement is the
        // strongest evidence available and is the case the feature exists for.
        let mut n = Nudges::default();
        n.update_from_candidates(&candidates(), 3);
        assert!(n.nudge_for("r") > n.nudge_for("n"));
        assert!(n.nudge_for("n") > 1.0, "but the minority is still pulled");
    }

    #[test]
    fn the_boost_is_capped_at_twenty_percent() {
        // Strong actual signal must still win: the nudge tips a wobble, it does
        // not overrule a deliberate movement.
        let many: Vec<(String, f64)> = (0..50).map(|_| ("ra".to_string(), 1.0)).collect();
        let mut n = Nudges::default();
        n.update_from_candidates(&many, 0);
        assert!(n.nudge_for("r") <= MAX_NUDGE);
    }

    #[test]
    fn one_confident_candidate_cannot_supply_the_whole_boost() {
        // The per-candidate cap. Otherwise a single wrong-but-certain word
        // decides the decode on its own.
        let mut n = Nudges::default();
        n.update_from_candidates(&[("rat".into(), 1.0)], 0);
        assert!(n.nudge_for("r") <= 1.0 + PER_CANDIDATE_WEIGHT_CAP + 1e-9);
    }

    #[test]
    fn only_the_top_candidates_are_consulted() {
        // A long tail of weak candidates would otherwise accumulate into a real
        // pull toward whatever they happen to share.
        let many: Vec<(String, f64)> = (0..40).map(|i| {
            (format!("{}bc", if i < 5 { 'x' } else { 'q' }), 1.0)
        }).collect();
        let mut n = Nudges::default();
        n.update_from_candidates(&many, 0);
        assert_eq!(n.nudge_for("q"), 1.0, "beyond the fifth, nothing pulls");
        assert!(n.nudge_for("x") > 1.0);
    }

    #[test]
    fn the_nudge_targets_the_next_key_not_the_current_one() {
        // Pulling toward a key the finger has already left would drag the path
        // backwards.
        let mut n = Nudges::default();
        n.update_from_candidates(&candidates(), 3);
        assert_eq!(n.nudge_for("o"), 1.0, "already consumed");
        assert!(n.nudge_for("r") > 1.0);
    }

    #[test]
    fn a_candidate_shorter_than_the_path_contributes_nothing() {
        // "so" has no fourth character; indexing past its end must not panic or
        // wrap to the first.
        let mut n = Nudges::default();
        n.update_from_candidates(&[("so".into(), 1.0), ("sort".into(), 1.0)], 3);
        assert!(n.nudge_for("t") > 1.0);
        assert_eq!(n.nudged_key_count(), 1);
    }

    #[test]
    fn an_unnudged_key_is_exactly_neutral() {
        // Not 0.999 or 1.001: an untouched key must decode identically to a run
        // with the feature off, or the nudger changes behaviour it was never
        // asked about.
        let n = Nudges::default();
        assert_eq!(n.nudge_for("z"), 1.0);
        assert_eq!(n.effective_distance("z", 42.0), 42.0);
    }

    #[test]
    fn nudging_tips_a_wobble_between_adjacent_keys() {
        // The whole point, in the original's own example: a path aiming at `r`
        // whose pivot lands slightly nearer `t`.
        let mut n = Nudges::default();
        n.update_from_candidates(&candidates(), 3);
        let keys = [("t", 100.0), ("r", 104.0)];
        assert_eq!(n.snap(&keys), Some("r"), "the near miss is recovered");
    }

    #[test]
    fn nudging_does_not_overrule_a_clear_movement() {
        // A finger decisively on `t` still decodes as `t`, however much the
        // candidates want `r`.
        let mut n = Nudges::default();
        n.update_from_candidates(&candidates(), 3);
        let keys = [("t", 40.0), ("r", 160.0)];
        assert_eq!(n.snap(&keys), Some("t"));
    }

    #[test]
    fn the_pull_scales_with_distance_rather_than_offsetting_it() {
        // A fixed offset would swamp a near miss and do nothing for a far one,
        // which is backwards.
        let mut n = Nudges::default();
        n.update_from_candidates(&candidates(), 3);
        let near = 100.0 - n.effective_distance("r", 100.0);
        let far = 1000.0 - n.effective_distance("r", 1000.0);
        assert!(far > near * 5.0, "near {near} far {far}");
    }

    #[test]
    fn ties_resolve_deterministically() {
        // A keyboard that decoded differently between runs would be untestable
        // and would feel haunted.
        let n = Nudges::default();
        let keys = [("a", 50.0), ("b", 50.0)];
        assert_eq!(n.snap(&keys), Some("a"));
        assert_eq!(n.snap(&keys), Some("a"));
    }

    #[test]
    fn snapping_an_empty_key_set_is_none_rather_than_a_panic() {
        let n = Nudges::default();
        assert_eq!(n.snap(&[]), None);
    }

    #[test]
    fn clearing_returns_to_neutral() {
        // Between swipes: the previous word's predictions must not bias the
        // next one's first key.
        let mut n = Nudges::default();
        n.update_from_candidates(&candidates(), 3);
        assert!(!n.is_empty());
        n.clear();
        assert!(n.is_empty());
        assert_eq!(n.nudge_for("r"), 1.0);
    }

    #[test]
    fn rebuilding_replaces_rather_than_accumulates() {
        // Called every tick. Accumulating would grow the pull without bound and
        // blow through the cap by a different route.
        let mut n = Nudges::default();
        n.update_from_candidates(&candidates(), 3);
        let first = n.nudge_for("r");
        n.update_from_candidates(&candidates(), 3);
        assert_eq!(n.nudge_for("r"), first);
    }

    #[test]
    fn a_negative_or_wild_score_cannot_invert_the_pull() {
        // A recognizer bug must not turn magnetism into repulsion, which would
        // decode the one key the user was aiming at as the least likely.
        let mut n = Nudges::default();
        n.update_from_candidates(&[("rat".into(), -5.0), ("run".into(), 900.0)], 0);
        assert!(n.nudge_for("r") >= 1.0);
        assert!(n.nudge_for("r") <= MAX_NUDGE);
    }
}
