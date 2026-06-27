//! Stenotype input engine.
//!
//! Implements the "two buttons per thumb, paired-stroke" steno
//! mode (see `LayoutStyle::Stenotype` in layouts/mod.rs and the
//! design notes in the steno-visualizer.html prototype).
//!
//! # Wire model
//!
//! - The GTK layer reports `feed_gesture(side, button_id, gesture)`
//!   when the user taps or swipes one of the four steno buttons.
//! - The engine holds a pending slot per side (L, R) and a
//!   timestamp.
//! - If a gesture on the OTHER side arrives within
//!   `PAIR_WINDOW_MS`, the two are committed as a paired stroke
//!   and looked up in the phoneme/brief tables.
//! - If no pair arrives within the window, a glib timer commits
//!   the lone gesture as a "solo stroke" (single phoneme).
//!
//! The engine itself is GTK-agnostic — it returns `Commit` events
//! that the UI layer turns into actual text commits via the
//! engine→vkbd bridge.

pub mod expansion;

use parking_lot::Mutex;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tracing::{debug, info};

/// How long after one thumb fires we'll wait for the other thumb
/// before committing the first as a solo stroke. 400ms matches
/// what feels natural in the visualizer; tunable later.
pub const PAIR_WINDOW_MS: u64 = 400;

/// How many word-expansion candidates to surface per stroke. The
/// suggestion UI shows at most this many; 4 fits a phone ribbon
/// without crowding.
pub const MAX_CANDIDATES: usize = 4;

/// Which thumb the gesture came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StenoSide {
    L,
    R,
}

impl StenoSide {
    pub fn other(self) -> Self {
        match self {
            StenoSide::L => StenoSide::R,
            StenoSide::R => StenoSide::L,
        }
    }
}

/// Which physical button on the steno surface. L1/L2 are the two
/// left-side buttons, R1/R2 the two right-side. `1` is the outer
/// button (further from screen center), `2` is the inner.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StenoButton {
    L1,
    L2,
    R1,
    R2,
}

impl StenoButton {
    /// Parse from the string carried in `KeyAction::Steno("L1")` etc.
    pub fn parse(s: &str) -> Option<StenoButton> {
        match s {
            "L1" => Some(StenoButton::L1),
            "L2" => Some(StenoButton::L2),
            "R1" => Some(StenoButton::R1),
            "R2" => Some(StenoButton::R2),
            _ => None,
        }
    }

    pub fn side(self) -> StenoSide {
        match self {
            StenoButton::L1 | StenoButton::L2 => StenoSide::L,
            StenoButton::R1 | StenoButton::R2 => StenoSide::R,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            StenoButton::L1 => "L1",
            StenoButton::L2 => "L2",
            StenoButton::R1 => "R1",
            StenoButton::R2 => "R2",
        }
    }
}

/// Gesture flavor for steno. Tap and four cardinals only — matches
/// the user's design choice of 5 outcomes per button.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StenoGesture {
    Tap,
    SwipeN,
    SwipeE,
    SwipeS,
    SwipeW,
}

impl StenoGesture {
    pub fn as_str(self) -> &'static str {
        match self {
            StenoGesture::Tap => "tap",
            StenoGesture::SwipeN => "N",
            StenoGesture::SwipeE => "E",
            StenoGesture::SwipeS => "S",
            StenoGesture::SwipeW => "W",
        }
    }
}

/// A single (button, gesture) outcome.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct StenoStroke {
    pub button: StenoButton,
    pub gesture: StenoGesture,
}

impl StenoStroke {
    /// Compact string form: "L1.tap", "R2.S", etc. Used as the
    /// hash key for the phoneme/brief tables.
    pub fn key(self) -> String {
        format!("{}.{}", self.button.as_str(), self.gesture.as_str())
    }
}

/// The output of a stroke commit — what should appear in the
/// editor and what to display in the microterm.
#[derive(Debug, Clone)]
pub struct StenoCommit {
    /// Text to commit into the editor. Briefs are 1+ words; phoneme
    /// commits are usually 1-2 letters. May include a trailing
    /// space for briefs (which conventionally insert their own
    /// inter-word spacing).
    pub text: String,
    /// Whether this was a brief (vs raw phoneme[s]). Used by the
    /// microterm to display briefs with extra emphasis.
    pub was_brief: bool,
    /// The pair description for the microterm — e.g.
    /// "L1.tap + R1.tap → the".
    pub description: String,
    /// Word-expansion candidates for the phoneme buffer AS OF this
    /// commit. The UI surfaces these as tappable suggestions; the
    /// user can accept one (committing the word and clearing the
    /// buffer) or keep stroking to refine. Empty for brief commits
    /// (briefs are already whole words) and when no dictionary entry
    /// matches the accumulated phonemes.
    pub candidates: Vec<expansion::Candidate>,
}

/// Per-side pending state.
#[derive(Debug, Clone, Copy)]
struct PendingStroke {
    stroke: StenoStroke,
    /// Monotonic timestamp of when this stroke was registered.
    /// Used both for pair-window timing and for the generation
    /// counter so we can detect "is this still the same pending
    /// stroke I scheduled the timeout for?"
    ts: Instant,
    /// Generation counter — increments every time a stroke lands
    /// on this side. The glib timeout closure captures the gen
    /// at scheduling time; if `current_gen != captured_gen` when
    /// the timeout fires, the timeout is stale (the stroke was
    /// already committed via pairing or replaced by a newer one).
    gen: u64,
}

/// The engine. One per process, accessed via [`global`].
pub struct StenoEngine {
    /// Pending stroke per side, awaiting pair or timeout.
    pending: Mutex<HashMap<StenoSide, PendingStroke>>,
    /// Generation counter per side. Bumped on every stroke arrival.
    /// Used by the timeout closures to detect stale firings.
    gen_l: AtomicU64,
    gen_r: AtomicU64,
    /// Phoneme tables — keyed by `StenoStroke::key()`.
    phoneme_table: HashMap<String, &'static str>,
    /// Brief table — keyed by `${L.key()}+${R.key()}` for paired
    /// strokes. Brief values may contain spaces and include
    /// trailing punctuation per Plover convention.
    brief_table: HashMap<String, &'static str>,
    /// Accumulated phonemes for the current word-in-progress. Each
    /// resolved stroke appends its phoneme(s) here; the expansion
    /// lookup runs against this buffer to surface word candidates.
    /// Cleared when a word is accepted or a word-boundary stroke
    /// fires. When empty, no word is in progress.
    phoneme_buffer: Mutex<Vec<String>>,
}

impl StenoEngine {
    pub(crate) fn new() -> Self {
        Self {
            pending: Mutex::new(HashMap::new()),
            gen_l: AtomicU64::new(0),
            gen_r: AtomicU64::new(0),
            phoneme_table: build_phoneme_table(),
            brief_table: build_brief_table(),
            phoneme_buffer: Mutex::new(Vec::new()),
        }
    }

    /// Generation snapshot for the given side; used by the GTK
    /// layer's timeout closure to ignore stale firings.
    pub fn current_gen(&self, side: StenoSide) -> u64 {
        match side {
            StenoSide::L => self.gen_l.load(Ordering::Relaxed),
            StenoSide::R => self.gen_r.load(Ordering::Relaxed),
        }
    }

    fn bump_gen(&self, side: StenoSide) -> u64 {
        match side {
            StenoSide::L => self.gen_l.fetch_add(1, Ordering::Relaxed) + 1,
            StenoSide::R => self.gen_r.fetch_add(1, Ordering::Relaxed) + 1,
        }
    }

    /// Feed a gesture into the engine. Returns:
    ///
    ///   - `Some(commit)` if this gesture paired with a pending
    ///     stroke from the opposite side and the pair has been
    ///     resolved
    ///   - `None` if this gesture is now the pending stroke for
    ///     its side, awaiting either a pair or a timeout commit
    ///
    /// On `None` the caller is responsible for scheduling a
    /// timer of `PAIR_WINDOW_MS` to call `flush_solo(side, gen)`
    /// if the stroke is still pending then.
    pub fn feed_gesture(&self, stroke: StenoStroke) -> (Option<StenoCommit>, u64) {
        let side = stroke.button.side();
        let gen = self.bump_gen(side);
        let now = Instant::now();
        let mut pending = self.pending.lock();
        let other_side = side.other();

        // Replace any existing pending on THIS side. If one was
        // there, it is dropped (see the same-side-replacement
        // rationale further down): the most recent gesture is
        // assumed to be the user's intent. The displaced stroke's
        // own flush_solo timer will fire later but find that gen
        // has been bumped — it returns None (stale closure check),
        // so no double-commit. The displaced is logged for
        // diagnostic visibility only.
        let displaced = pending.insert(
            side,
            PendingStroke {
                stroke,
                ts: now,
                gen,
            },
        );

        // Pair check: is the other side pending and within window?
        if let Some(other) = pending.get(&other_side).copied() {
            let gap = now.duration_since(other.ts);
            if gap < Duration::from_millis(PAIR_WINDOW_MS) {
                // Pair! Resolve.
                let (l, r) = match side {
                    StenoSide::L => (stroke, other.stroke),
                    StenoSide::R => (other.stroke, stroke),
                };
                pending.remove(&StenoSide::L);
                pending.remove(&StenoSide::R);
                drop(pending);
                // Edge case: if a prior same-side stroke was
                // displaced by THIS one before pairing (user did
                // e.g. L1 then L2 then R, all fast), that displaced
                // stroke is lost. The intent is genuinely ambiguous
                // — "did they mean L1+R or L2+R?" — and the most
                // recent same-side gesture is the better guess, so
                // we keep it and drop the older. Rare in practice;
                // logged at debug for diagnosis only.
                if let Some(d) = displaced {
                    debug!(
                        "steno: dropped displaced same-side stroke {} (paired with newer)",
                        d.stroke.key()
                    );
                }
                return (Some(self.resolve_pair(l, r)), gen);
            }
        }

        // No pair yet. The pending entry holds; caller schedules
        // a timeout for `PAIR_WINDOW_MS` then calls flush_solo.
        if let Some(d) = displaced {
            info!(
                "steno: displaced pending {} on same side, awaiting new",
                d.stroke.key()
            );
        }
        (None, gen)
    }

    /// Called by the timeout closure: if the pending stroke on
    /// `side` still has generation `captured_gen` (i.e. nothing
    /// has happened on that side since the closure was scheduled),
    /// commit it as a solo stroke. Otherwise no-op (stale closure).
    pub fn flush_solo(&self, side: StenoSide, captured_gen: u64) -> Option<StenoCommit> {
        let current = self.current_gen(side);
        if current != captured_gen {
            debug!(
                "steno: stale flush_solo for side {:?} (gen {} vs {})",
                side, captured_gen, current
            );
            return None;
        }
        let mut pending = self.pending.lock();
        let entry = pending.remove(&side)?;
        drop(pending);
        Some(self.resolve_solo(entry.stroke))
    }

    fn resolve_pair(&self, l: StenoStroke, r: StenoStroke) -> StenoCommit {
        let key = format!("{}+{}", l.key(), r.key());
        if let Some(brief) = self.brief_table.get(&key).copied() {
            // A brief is a complete word — it ends the current
            // word-in-progress. Clear the phoneme buffer so the next
            // stroke starts fresh.
            self.phoneme_buffer.lock().clear();
            return StenoCommit {
                text: format!("{} ", brief),
                was_brief: true,
                description: format!("{} → «{}»", key, brief),
                candidates: Vec::new(),
            };
        }
        let l_phoneme = self.phoneme_table.get(&l.key()).copied().unwrap_or("?");
        let r_phoneme = self.phoneme_table.get(&r.key()).copied().unwrap_or("?");
        let text = format!("{}{}", l_phoneme, r_phoneme);
        // Append both phonemes to the buffer and look up candidates.
        let candidates = {
            let mut buf = self.phoneme_buffer.lock();
            if l_phoneme != "?" {
                buf.push(l_phoneme.to_string());
            }
            if r_phoneme != "?" {
                buf.push(r_phoneme.to_string());
            }
            expansion::lookup(&buf, MAX_CANDIDATES)
        };
        StenoCommit {
            text: text.clone(),
            was_brief: false,
            description: format!("{} → {}", key, text),
            candidates,
        }
    }

    fn resolve_solo(&self, s: StenoStroke) -> StenoCommit {
        let key = s.key();
        let phoneme = self.phoneme_table.get(&key).copied().unwrap_or("?");
        let candidates = {
            let mut buf = self.phoneme_buffer.lock();
            if phoneme != "?" {
                buf.push(phoneme.to_string());
            }
            expansion::lookup(&buf, MAX_CANDIDATES)
        };
        StenoCommit {
            text: phoneme.to_string(),
            was_brief: false,
            description: format!("{} (solo) → {}", key, phoneme),
            candidates,
        }
    }

    /// Result of accepting a suggestion: how many characters of
    /// already-committed raw phonemes to delete, plus the word to
    /// insert (with trailing space). As the user strokes, each
    /// phoneme is committed to the editor immediately (so they see
    /// progress). Accepting a word must therefore first BACKSPACE
    /// those raw phonemes, then insert the chosen word — otherwise
    /// "k a t" + accept "cat" would leave "katcat".
    ///
    /// `backspaces` is the total char count of the buffered phonemes
    /// (e.g. ["k","a","ng"] → 1+1+2 = 4), counted in Unicode scalar
    /// values to match how the editor counts a backspace.
    pub fn accept_word(&self, word: &str) -> AcceptedWord {
        let mut buf = self.phoneme_buffer.lock();
        let backspaces = buf.iter().map(|p| p.chars().count()).sum();
        buf.clear();
        drop(buf);
        // Invalidate any in-flight solo-flush: without this, a pending
        // stroke whose flush timer is still scheduled would fire after
        // accept, append a phoneme to the now-empty buffer, and emit a
        // stray commit after the accepted word ("cat" → "cat t").
        self.invalidate_pending();
        AcceptedWord {
            backspaces,
            text: format!("{} ", word),
        }
    }

    /// Clear the phoneme buffer (word boundary — e.g. the user
    /// stroked space, started a new word, or focus changed). Also
    /// invalidates pending strokes + their flush timers, so a stale
    /// solo-flush can't resurrect a phoneme into the cleared buffer.
    pub fn clear_buffer(&self) {
        self.phoneme_buffer.lock().clear();
        self.invalidate_pending();
    }

    /// Drop any pending (unpaired) strokes and bump BOTH generation
    /// counters so every outstanding `flush_solo` closure sees a
    /// generation mismatch and no-ops. Used whenever the word-in-
    /// progress is force-ended (accept, clear, focus change).
    fn invalidate_pending(&self) {
        self.pending.lock().clear();
        self.gen_l.fetch_add(1, Ordering::Relaxed);
        self.gen_r.fetch_add(1, Ordering::Relaxed);
    }

    /// Snapshot the current phoneme buffer (for diagnostics / the
    /// microterm to show the word-in-progress).
    pub fn buffer_snapshot(&self) -> Vec<String> {
        self.phoneme_buffer.lock().clone()
    }
}

/// Returned by [`StenoEngine::accept_word`]. See that method.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcceptedWord {
    /// Number of characters of raw phoneme text to delete before
    /// inserting `text`.
    pub backspaces: usize,
    /// The accepted word with a trailing space.
    pub text: String,
}

/// Per-button phoneme tables, matching the visualizer prototype:
///   L1 (outer-left, "onset A"):   t / n s l r   (tap / N E S W)
///   L2 (inner-left, "onset B"):   d / k m p b
///   R1 (inner-right, "vowel"):    ə / a e o i
///   R2 (outer-right, "coda"):     u / n t d ŋ
///
/// Phoneme letters use ASCII approximations of IPA so they commit
/// cleanly as text. `ŋ` becomes `ng`, `ə` becomes `uh`. The user
/// will see actual letters in the editor; the IPA is only for our
/// internal documentation.
fn build_phoneme_table() -> HashMap<String, &'static str> {
    let mut t = HashMap::with_capacity(20);
    // L1
    t.insert("L1.tap".into(), "t");
    t.insert("L1.N".into(),   "n");
    t.insert("L1.E".into(),   "s");
    t.insert("L1.S".into(),   "l");
    t.insert("L1.W".into(),   "r");
    // L2
    t.insert("L2.tap".into(), "d");
    t.insert("L2.N".into(),   "k");
    t.insert("L2.E".into(),   "m");
    t.insert("L2.S".into(),   "p");
    t.insert("L2.W".into(),   "b");
    // R1
    t.insert("R1.tap".into(), "uh");
    t.insert("R1.N".into(),   "a");
    t.insert("R1.E".into(),   "e");
    t.insert("R1.S".into(),   "o");
    t.insert("R1.W".into(),   "i");
    // R2
    t.insert("R2.tap".into(), "u");
    t.insert("R2.N".into(),   "n");
    t.insert("R2.E".into(),   "t");
    t.insert("R2.S".into(),   "d");
    t.insert("R2.W".into(),   "ng");
    t
}

/// Brief table for the highest-frequency English words. Tiny v1
/// set — matches the visualizer prototype so we can validate
/// against the same gestures.
fn build_brief_table() -> HashMap<String, &'static str> {
    let mut t = HashMap::with_capacity(10);
    t.insert("L1.tap+R1.tap".into(), "the");
    t.insert("L1.N+R1.tap".into(),   "and");
    t.insert("L1.N+R1.E".into(),     "in");
    t.insert("L1.E+R1.E".into(),     "of");
    t.insert("L1.W+R1.tap".into(),   "or");
    t.insert("L2.tap+R1.tap".into(), "a");
    t.insert("L1.tap+R2.E".into(),   "it");
    t.insert("L2.E+R1.E".into(),     "me");
    t.insert("L1.tap+R1.W".into(),   "to");
    t.insert("L2.tap+R1.E".into(),   "do");
    t
}

/// Process-wide global. The GTK layer feeds gestures here and the
/// resulting StenoCommit gets turned into an `EngineInput::StenoCommit`
/// (verbatim text commit, no auto-space — so phonemes chain
/// directly into syllables).
pub fn global() -> Arc<StenoEngine> {
    use once_cell::sync::Lazy;
    static GLOBAL: Lazy<Arc<StenoEngine>> = Lazy::new(|| Arc::new(StenoEngine::new()));
    GLOBAL.clone()
}

#[cfg(test)]
mod tests {
    //! Tests for the steno pair-stroke engine. Pairing uses
    //! `Instant::now()` deltas internally, but back-to-back feeds in
    //! a test run complete in microseconds — far under PAIR_WINDOW_MS
    //! — so same-test pairs reliably pair and the generation-counter
    //! solo logic is deterministic.

    use super::*;

    fn stroke(button: StenoButton, gesture: StenoGesture) -> StenoStroke {
        StenoStroke { button, gesture }
    }

    #[test]
    fn button_parse_roundtrip() {
        for s in ["L1", "L2", "R1", "R2"] {
            let b = StenoButton::parse(s).unwrap();
            assert_eq!(b.as_str(), s);
        }
        assert!(StenoButton::parse("Z9").is_none());
    }

    #[test]
    fn button_side_assignment() {
        assert_eq!(StenoButton::L1.side(), StenoSide::L);
        assert_eq!(StenoButton::L2.side(), StenoSide::L);
        assert_eq!(StenoButton::R1.side(), StenoSide::R);
        assert_eq!(StenoButton::R2.side(), StenoSide::R);
    }

    #[test]
    fn first_gesture_is_pending_not_committed() {
        let eng = StenoEngine::new();
        // A lone left-side gesture has no right-side partner yet, so
        // it should go pending (None) and hand back a generation.
        let (commit, gen) = eng.feed_gesture(stroke(StenoButton::L1, StenoGesture::Tap));
        assert!(commit.is_none());
        assert!(gen >= 1);
    }

    #[test]
    fn opposite_sides_pair_immediately() {
        let eng = StenoEngine::new();
        // Left pending, then right arrives within the window → pair.
        let (none, _) = eng.feed_gesture(stroke(StenoButton::L1, StenoGesture::Tap));
        assert!(none.is_none());
        let (commit, _) = eng.feed_gesture(stroke(StenoButton::R1, StenoGesture::Tap));
        let commit = commit.expect("L1.tap + R1.tap should pair");
        // L1.tap+R1.tap is a brief → "the".
        assert!(commit.was_brief);
        assert_eq!(commit.text, "the ");
    }

    #[test]
    fn non_brief_pair_emits_concatenated_phonemes() {
        let eng = StenoEngine::new();
        eng.feed_gesture(stroke(StenoButton::L1, StenoGesture::SwipeE)); // s
        let (commit, _) = eng.feed_gesture(stroke(StenoButton::R2, StenoGesture::SwipeE)); // t
        let commit = commit.expect("should pair");
        assert!(!commit.was_brief);
        // L1.E = "s", R2.E = "t" → "st".
        assert_eq!(commit.text, "st");
    }

    #[test]
    fn flush_solo_commits_when_generation_unchanged() {
        let eng = StenoEngine::new();
        let (none, gen) = eng.feed_gesture(stroke(StenoButton::L1, StenoGesture::Tap));
        assert!(none.is_none());
        // No further activity on the L side → the captured gen still
        // matches → flush commits the lone phoneme.
        let commit = eng.flush_solo(StenoSide::L, gen).expect("solo flush");
        assert_eq!(commit.text, "t"); // L1.tap = "t"
        assert!(!commit.was_brief);
    }

    #[test]
    fn flush_solo_is_noop_when_generation_advanced() {
        let eng = StenoEngine::new();
        let (_none, gen1) = eng.feed_gesture(stroke(StenoButton::L1, StenoGesture::Tap));
        // A second L-side gesture advances the generation, displacing
        // the first pending. The stale timeout (captured gen1) must
        // NOT commit.
        let (_none2, _gen2) = eng.feed_gesture(stroke(StenoButton::L2, StenoGesture::Tap));
        assert!(eng.flush_solo(StenoSide::L, gen1).is_none());
    }

    #[test]
    fn flush_solo_after_pair_is_stale() {
        let eng = StenoEngine::new();
        let (_none, gen) = eng.feed_gesture(stroke(StenoButton::L1, StenoGesture::Tap));
        // Pair consumes the pending L stroke.
        let (commit, _) = eng.feed_gesture(stroke(StenoButton::R1, StenoGesture::Tap));
        assert!(commit.is_some());
        // The L-side timeout that was scheduled for the now-paired
        // stroke must be stale → no double commit.
        assert!(eng.flush_solo(StenoSide::L, gen).is_none());
    }

    #[test]
    fn stroke_key_format() {
        assert_eq!(stroke(StenoButton::L1, StenoGesture::Tap).key(), "L1.tap");
        assert_eq!(stroke(StenoButton::R2, StenoGesture::SwipeW).key(), "R2.W");
    }

    #[test]
    fn phoneme_buffer_accumulates_across_solos() {
        let eng = StenoEngine::new();
        // Stroke /k/ then /a/ then /t/ as solos. After the third,
        // the buffer should be [k,a,t] and "cat" should be a
        // candidate.
        // L2.N = "k"
        let (_n, g1) = eng.feed_gesture(stroke(StenoButton::L2, StenoGesture::SwipeN));
        eng.flush_solo(StenoSide::L, g1);
        // R1.N = "a"
        let (_n2, g2) = eng.feed_gesture(stroke(StenoButton::R1, StenoGesture::SwipeN));
        eng.flush_solo(StenoSide::R, g2);
        // R2.E = "t"
        let (_n3, g3) = eng.feed_gesture(stroke(StenoButton::R2, StenoGesture::SwipeE));
        let commit = eng.flush_solo(StenoSide::R, g3).expect("solo");
        assert_eq!(eng.buffer_snapshot(), vec!["k", "a", "t"]);
        assert!(
            commit.candidates.iter().any(|c| c.word == "cat"),
            "expected 'cat' candidate, got {:?}",
            commit.candidates
        );
    }

    #[test]
    fn brief_clears_buffer() {
        let eng = StenoEngine::new();
        // Accumulate a phoneme first.
        let (_n, g1) = eng.feed_gesture(stroke(StenoButton::L2, StenoGesture::SwipeN));
        eng.flush_solo(StenoSide::L, g1);
        assert!(!eng.buffer_snapshot().is_empty());
        // Now fire a brief pair (L1.tap + R1.tap = "the").
        let (_n2, _g) = eng.feed_gesture(stroke(StenoButton::L1, StenoGesture::Tap));
        let (commit, _) = eng.feed_gesture(stroke(StenoButton::R1, StenoGesture::Tap));
        let commit = commit.expect("pair");
        assert!(commit.was_brief);
        // Brief is a whole word → buffer cleared.
        assert!(eng.buffer_snapshot().is_empty());
    }

    #[test]
    fn accept_word_clears_buffer_and_spaces() {
        let eng = StenoEngine::new();
        let (_n, g1) = eng.feed_gesture(stroke(StenoButton::L2, StenoGesture::SwipeN));
        eng.flush_solo(StenoSide::L, g1);
        // One phoneme committed: L2.N = "k" = 1 char.
        let accepted = eng.accept_word("king");
        assert_eq!(accepted.text, "king ");
        assert_eq!(accepted.backspaces, 1, "one phoneme 'k' = 1 char to delete");
        assert!(eng.buffer_snapshot().is_empty());
    }

    #[test]
    fn accept_word_counts_multichar_phonemes() {
        let eng = StenoEngine::new();
        // R2.W = "ng" (2 chars) — verify backspace count uses char
        // length, not phoneme count.
        let (_n, g1) = eng.feed_gesture(stroke(StenoButton::R2, StenoGesture::SwipeW));
        eng.flush_solo(StenoSide::R, g1);
        let accepted = eng.accept_word("sing");
        assert_eq!(accepted.backspaces, 2, "'ng' is 2 chars");
    }

    #[test]
    fn clear_buffer_resets() {
        let eng = StenoEngine::new();
        let (_n, g1) = eng.feed_gesture(stroke(StenoButton::L2, StenoGesture::SwipeN));
        eng.flush_solo(StenoSide::L, g1);
        eng.clear_buffer();
        assert!(eng.buffer_snapshot().is_empty());
    }

    #[test]
    fn accept_invalidates_pending_solo_flush() {
        // Reproduces the "cat t" bug: a pending stroke whose flush
        // timer is still scheduled when the user accepts a word must
        // NOT fire afterward and append a stray phoneme.
        let eng = StenoEngine::new();
        // Stroke L solo → pending, capture its generation as the
        // scheduled flush closure would.
        let (none, captured_gen) =
            eng.feed_gesture(stroke(StenoButton::L1, StenoGesture::Tap));
        assert!(none.is_none());
        // User taps a suggestion before the flush timer fires.
        let _ = eng.accept_word("the");
        // The still-scheduled flush now runs — it must see a stale
        // generation and no-op rather than committing the lone "t".
        assert!(
            eng.flush_solo(StenoSide::L, captured_gen).is_none(),
            "flush after accept must be stale — no stray phoneme"
        );
        assert!(eng.buffer_snapshot().is_empty());
    }

    #[test]
    fn clear_buffer_invalidates_pending_solo_flush() {
        // Same guarantee for clear_buffer (focus change / word
        // boundary): a pending stroke's flush must not resurrect.
        let eng = StenoEngine::new();
        let (none, captured_gen) =
            eng.feed_gesture(stroke(StenoButton::R1, StenoGesture::Tap));
        assert!(none.is_none());
        eng.clear_buffer();
        assert!(eng.flush_solo(StenoSide::R, captured_gen).is_none());
    }
}
