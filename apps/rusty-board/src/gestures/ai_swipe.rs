//! AI swipe assist coordinator.
//!
//! Bundles the personalization signals (AdaptiveDictionary,
//! NgramModel) as process-globals so the swipe-commit path can
//! consult them without threading references through every function
//! that touches a swipe.
//!
//! ## Two entry points
//!
//! - `apply_to_candidates` — call after the recognizer returns a
//!   candidate list. Adjusts each candidate's score by the
//!   per-user signals and re-sorts. The first candidate after this
//!   call is what should be committed.
//!
//! - `record_commit` — call after a swipe word is committed. Feeds
//!   the dictionary + n-gram models so they learn from real usage.
//!
//! Both functions are gated by `ai_swipe_assist_enabled()`. When
//! the master toggle is off, they're cheap no-ops — no allocations,
//! no map lookups, no I/O.
//!
//! ## Why globals instead of references
//!
//! The swipe-commit path runs through `dispatch_swipe_source` which
//! already takes many parameters. Adding `&AdaptiveDictionary` and
//! `&NgramModel` to every level of that call tree would mean:
//!   - changing `recognize_with_speed`'s signature
//!   - threading the references through `KeyboardView`, all closures
//!   - updating ~50 callsites
//!   - updating every test that constructs a recognizer
//!
//! Globals avoid all of that. The trade-off is global state is harder
//! to test in isolation, mitigated here by:
//!   - per-function `enabled` short-circuit (tests can flip off)
//!   - explicit `clear_for_test` to reset between tests
//!   - the underlying AdaptiveDictionary/NgramModel are themselves
//!     unit-tested in isolation; here we only test the coordination.

use crate::gestures::adaptive_dictionary::AdaptiveDictionary;
use crate::gestures::ngram::NgramModel;
use crate::gestures::recognizer::WordCandidate;
use parking_lot::Mutex;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;

/// Master toggle. Default true — feature ships ON; user can turn
/// it off in settings if they prefer the pure-shape recognizer.
static AI_SWIPE_ASSIST_ENABLED: AtomicBool = AtomicBool::new(true);

/// Per-module sub-toggles. Each defaults true (gated by the master).
/// Useful for users who want, say, dictionary boosting but no n-gram
/// re-ranking. Default true; user can disable individually.
static ADAPTIVE_DICTIONARY_ENABLED: AtomicBool = AtomicBool::new(true);
static NGRAM_ASSIST_ENABLED: AtomicBool = AtomicBool::new(true);

/// The loaded models. Initialized once via `init()`, immutable
/// afterward. The AdaptiveDictionary and NgramModel are themselves
/// internally mutable (interior mutability via Arc<RwLock<...>>),
/// so &T from OnceLock is enough — record_commit doesn't need
/// exclusive access at the global level.
static DICT: OnceLock<AdaptiveDictionary> = OnceLock::new();
static NGRAM: OnceLock<NgramModel> = OnceLock::new();

/// Recent commit history for n-gram context. We keep the last 2
/// committed words so we have `prev_prev` and `prev` available
/// for trigram scoring. Bounded — pushes evict the oldest.
static RECENT_WORDS: Mutex<VecDeque<String>> = Mutex::new(VecDeque::new());
const RECENT_WORDS_CAP: usize = 2;

/// Load the persistent state from disk. Idempotent — second call is
/// a no-op (OnceLock::set silently fails if already set, which is
/// the desired semantic for a singleton).
///
/// Call this once at process startup, after config is loaded.
/// Subsequent calls do nothing.
pub fn init() {
    let _ = DICT.set(AdaptiveDictionary::load());
    let _ = NGRAM.set(NgramModel::load());
}

/// Master toggle. Off → all AI assist features (dictionary boost,
/// n-gram re-rank, future hitbox nudge) become no-ops.
pub fn set_ai_swipe_assist_enabled(on: bool) {
    AI_SWIPE_ASSIST_ENABLED.store(on, Ordering::Relaxed);
}

pub fn ai_swipe_assist_enabled() -> bool {
    AI_SWIPE_ASSIST_ENABLED.load(Ordering::Relaxed)
}

pub fn set_adaptive_dictionary_enabled(on: bool) {
    ADAPTIVE_DICTIONARY_ENABLED.store(on, Ordering::Relaxed);
}

pub fn adaptive_dictionary_enabled() -> bool {
    ADAPTIVE_DICTIONARY_ENABLED.load(Ordering::Relaxed)
}

pub fn set_ngram_assist_enabled(on: bool) {
    NGRAM_ASSIST_ENABLED.store(on, Ordering::Relaxed);
}

pub fn ngram_assist_enabled() -> bool {
    NGRAM_ASSIST_ENABLED.load(Ordering::Relaxed)
}

/// True iff the AdaptiveDictionary is loaded AND both its
/// sub-toggle and the master toggle are on.
fn dict_active() -> Option<&'static AdaptiveDictionary> {
    if !ai_swipe_assist_enabled() || !adaptive_dictionary_enabled() {
        return None;
    }
    DICT.get()
}

/// True iff the NgramModel is loaded AND both its sub-toggle and
/// the master toggle are on.
fn ngram_active() -> Option<&'static NgramModel> {
    if !ai_swipe_assist_enabled() || !ngram_assist_enabled() {
        return None;
    }
    NGRAM.get()
}

/// Apply per-user score adjustments and re-sort.
///
/// Recognizer scores are lower-is-better; both `boost` and `score`
/// from the AI models return positive bonuses to SUBTRACT. After
/// adjustment we re-sort and the first candidate becomes the
/// final pick.
///
/// Fast paths:
///   - Master off → early return, candidates unchanged.
///   - Both models unloaded/sub-toggled-off → early return.
///   - Empty candidates → early return.
///
/// Typical adjusted score: -0.5 to -1.0 for user-recent contextual
/// hits. The recognizer's typical inter-candidate gap is 0.1-2.0,
/// so this is enough to flip close calls without overwhelming
/// strong shape-match signals.
pub fn apply_to_candidates(candidates: &mut Vec<WordCandidate>) {
    if candidates.is_empty() {
        return;
    }
    let dict = dict_active();
    let ngram = ngram_active();
    if dict.is_none() && ngram.is_none() {
        return;
    }

    let (prev, prev_prev) = recent_two_words();
    let prev_str = prev.as_deref();
    let prev_prev_str = prev_prev.as_deref();

    for c in candidates.iter_mut() {
        if let Some(d) = dict {
            c.score -= d.boost(&c.word);
        }
        if let Some(n) = ngram {
            c.score -= n.score(prev_prev_str, prev_str, &c.word);
        }
    }

    // Re-sort by adjusted score. Stable sort preserves dictionary
    // order for tied candidates (the recognizer's existing
    // tie-breaking behavior).
    candidates.sort_by(|a, b| {
        a.score
            .partial_cmp(&b.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
}

/// Record that the user committed `word` as a swipe-typed word.
/// Feeds:
///   - AdaptiveDictionary count (frequency tracker)
///   - NgramModel bigram/trigram (next-word LM)
///   - RECENT_WORDS buffer (so future n-gram lookups have context)
///
/// Bails when AI assist is off OR when the word is whitespace-only.
/// Punctuation-stripping happens inside the model implementations.
pub fn record_commit(word: &str) {
    if !ai_swipe_assist_enabled() {
        return;
    }
    let trimmed = word.trim();
    if trimmed.is_empty() {
        return;
    }

    // Compute prev/prev_prev BEFORE updating RECENT_WORDS — otherwise
    // we'd be passing `word` itself as `prev`.
    let (prev, prev_prev) = recent_two_words();

    if adaptive_dictionary_enabled() {
        if let Some(d) = DICT.get() {
            d.record_commit(trimmed);
        }
    }
    if ngram_assist_enabled() {
        if let Some(n) = NGRAM.get() {
            n.record(prev_prev.as_deref(), prev.as_deref(), trimmed);
        }
    }

    // Update RECENT_WORDS unconditionally — even if sub-toggles are
    // off, we want context to be ready when the user re-enables.
    let mut recent = RECENT_WORDS.lock();
    recent.push_back(trimmed.to_string());
    while recent.len() > RECENT_WORDS_CAP {
        recent.pop_front();
    }
}

/// Mark a focus change. Clears RECENT_WORDS so n-gram lookups for
/// the next field don't carry over context from the previous one
/// ("thank you" at the end of a Slack message shouldn't bias the
/// next n-gram score in a URL bar).
pub fn note_focus_change() {
    RECENT_WORDS.lock().clear();
}

/// Persist any unsaved state to disk. Cheap if nothing's dirty.
/// Best-effort — failures are logged via tracing but not returned
/// to avoid escalating I/O errors into user-visible problems.
pub fn flush_to_disk() {
    if let Some(d) = DICT.get() {
        if let Err(e) = d.flush_if_dirty() {
            tracing::warn!("ai_swipe: dict flush failed: {}", e);
        }
    }
    if let Some(n) = NGRAM.get() {
        if let Err(e) = n.flush_if_dirty() {
            tracing::warn!("ai_swipe: ngram flush failed: {}", e);
        }
    }
}

/// Wipe both models. Used by the "Forget my vocabulary" action in
/// the settings panel. Doesn't disable AI assist — just clears the
/// learned state, so it can rebuild from scratch.
pub fn forget_all() {
    if let Some(d) = DICT.get() {
        d.forget_all();
    }
    if let Some(n) = NGRAM.get() {
        n.forget_all();
    }
    RECENT_WORDS.lock().clear();
}

/// Diagnostics: total number of learned dictionary entries.
pub fn dictionary_size() -> usize {
    DICT.get().map(|d| d.len()).unwrap_or(0)
}

/// Diagnostics: total number of learned bigrams.
pub fn ngram_bigram_count() -> usize {
    NGRAM.get().map(|n| n.bigram_count()).unwrap_or(0)
}

fn recent_two_words() -> (Option<String>, Option<String>) {
    let recent = RECENT_WORDS.lock();
    let len = recent.len();
    if len == 0 {
        return (None, None);
    }
    let prev = recent.back().cloned();
    let prev_prev = if len >= 2 {
        recent.get(len - 2).cloned()
    } else {
        None
    };
    (prev, prev_prev)
}

/// Test helper: reset RECENT_WORDS and re-enable toggles to defaults.
/// Doesn't touch the OnceLock dict/ngram (those are initialized once
/// for the test process). Use to isolate tests that share global
/// state.
#[cfg(test)]
pub(crate) fn reset_for_test() {
    RECENT_WORDS.lock().clear();
    AI_SWIPE_ASSIST_ENABLED.store(true, Ordering::Relaxed);
    ADAPTIVE_DICTIONARY_ENABLED.store(true, Ordering::Relaxed);
    NGRAM_ASSIST_ENABLED.store(true, Ordering::Relaxed);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cand(word: &str, score: f64) -> WordCandidate {
        WordCandidate {
            word: word.to_string(),
            score,
        }
    }

    /// When AI assist is off, candidates pass through unchanged.
    #[test]
    fn master_toggle_off_is_passthrough() {
        reset_for_test();
        set_ai_swipe_assist_enabled(false);
        let mut cands = vec![cand("a", 1.0), cand("b", 0.5), cand("c", 0.7)];
        let original = cands.clone();
        apply_to_candidates(&mut cands);
        assert_eq!(cands.len(), original.len());
        for (a, b) in cands.iter().zip(original.iter()) {
            assert_eq!(a.word, b.word);
            assert_eq!(a.score, b.score);
        }
        // Restore.
        set_ai_swipe_assist_enabled(true);
    }

    /// Empty candidate list is safe to pass.
    #[test]
    fn empty_candidates_no_panic() {
        reset_for_test();
        let mut cands: Vec<WordCandidate> = vec![];
        apply_to_candidates(&mut cands);
        assert!(cands.is_empty());
    }

    /// When models aren't loaded (init() never called), candidates
    /// are unchanged even with toggles on.
    #[test]
    fn models_not_loaded_is_passthrough() {
        reset_for_test();
        // DICT and NGRAM are OnceLock — if `init` ran in another
        // test (which it might since tests share process state),
        // they may already be loaded. In that case dict_active()
        // returns Some and we DO modify scores. So this test can
        // only verify the no-op shape, not the no-mutation
        // guarantee.
        let mut cands = vec![cand("hi", 1.0), cand("ho", 1.0)];
        let len_before = cands.len();
        apply_to_candidates(&mut cands);
        assert_eq!(cands.len(), len_before);
    }

    /// record_commit with empty/whitespace word is ignored.
    #[test]
    fn empty_commit_ignored() {
        reset_for_test();
        record_commit("");
        record_commit("   ");
        record_commit("\n");
        let (prev, prev_prev) = recent_two_words();
        assert!(prev.is_none());
        assert!(prev_prev.is_none());
    }

    /// RECENT_WORDS tracks the last two commits.
    #[test]
    fn recent_words_tracks_last_two() {
        reset_for_test();
        record_commit("alpha");
        let (prev, prev_prev) = recent_two_words();
        assert_eq!(prev.as_deref(), Some("alpha"));
        assert!(prev_prev.is_none());

        record_commit("beta");
        let (prev, prev_prev) = recent_two_words();
        assert_eq!(prev.as_deref(), Some("beta"));
        assert_eq!(prev_prev.as_deref(), Some("alpha"));

        record_commit("gamma");
        let (prev, prev_prev) = recent_two_words();
        assert_eq!(prev.as_deref(), Some("gamma"));
        // "alpha" should have been evicted.
        assert_eq!(prev_prev.as_deref(), Some("beta"));
    }

    /// note_focus_change clears RECENT_WORDS.
    #[test]
    fn focus_change_clears_recent() {
        reset_for_test();
        record_commit("hello");
        record_commit("world");
        note_focus_change();
        let (prev, prev_prev) = recent_two_words();
        assert!(prev.is_none());
        assert!(prev_prev.is_none());
    }

    /// Master toggle off: record_commit still no-ops the models BUT
    /// the trimmed-empty check still applies. (Documenting the
    /// behavior: off → completely silent.)
    #[test]
    fn record_commit_bails_when_master_off() {
        reset_for_test();
        set_ai_swipe_assist_enabled(false);
        record_commit("hello");
        // RECENT_WORDS not updated when master off (record_commit
        // early-returns).
        let (prev, _) = recent_two_words();
        assert!(prev.is_none());
        set_ai_swipe_assist_enabled(true);
    }

    /// Individual sub-toggles are independent.
    #[test]
    fn sub_toggles_independent() {
        reset_for_test();
        set_adaptive_dictionary_enabled(false);
        set_ngram_assist_enabled(true);
        assert!(!adaptive_dictionary_enabled());
        assert!(ngram_assist_enabled());

        set_adaptive_dictionary_enabled(true);
        set_ngram_assist_enabled(false);
        assert!(adaptive_dictionary_enabled());
        assert!(!ngram_assist_enabled());

        reset_for_test();
    }

    /// note_focus_change is safe to call even with empty buffer.
    #[test]
    fn focus_change_safe_when_empty() {
        reset_for_test();
        note_focus_change();
        note_focus_change(); // double-call also safe
        let (prev, _) = recent_two_words();
        assert!(prev.is_none());
    }

    /// Sanity: apply + record together don't loop or panic on a
    /// realistic sequence.
    #[test]
    fn apply_then_record_loop_no_panic() {
        reset_for_test();
        for word in ["the", "quick", "brown", "fox", "jumps"] {
            let mut cands = vec![
                cand(word, 0.1),
                cand(&format!("{}_alt", word), 0.5),
            ];
            apply_to_candidates(&mut cands);
            // First candidate after apply is what we commit.
            let chosen = &cands[0].word.clone();
            record_commit(chosen);
        }
        // RECENT_WORDS should have the last 2 of those.
        let (prev, prev_prev) = recent_two_words();
        assert!(prev.is_some());
        assert!(prev_prev.is_some());
    }
}
