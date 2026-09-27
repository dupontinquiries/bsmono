//! steno — mounts rusty-board's stenotype engine.
//!
//! `src/steno/{mod,expansion}.rs` (pairing window, phoneme/brief tables,
//! word expansion) has zero `gtk4`/`glib` references — checked directly,
//! not assumed — so, same decision as `config` and `voice`, the port
//! mounts it by `#[path]` instead of reimplementing the pairing state
//! machine and tables. One implementation, no drift.
//!
//! ## What this file does — and does not — do
//!
//! This is the mount only. It makes `steno::engine::{StenoEngine,
//! StenoButton, StenoGesture, StenoStroke, StenoCommit, global}` available
//! to the rest of the crate and confirms (by compiling + running the
//! engine's own test suite under this crate) that the file really is
//! GTK-free. It does NOT yet:
//!
//! - Define a Stenotype layout (the L1/L2/R1/R2 four-key grid) for
//!   `real_layout.rs`/`to_render` to produce — there is currently no
//!   render-layer key that carries a `steno` button id.
//! - Route gestures: turn a key's tap / cardinal-swipe into
//!   `engine::StenoButton` + `engine::StenoGesture` and feed
//!   `engine::global().feed_gesture(...)`. The GTK side does this in
//!   `keyboard_view.rs` (search `feed_gesture` / `surface_steno_commit`)
//!   — reusing the SAME cardinal-swipe classifier this port already has
//!   wired up for the alphabet cardinal grid, just with different output
//!   handling (a paired-stroke commit instead of a single character).
//! - Surface `StenoCommit`/word-expansion candidates in `microterm.rs`'s
//!   status strip. `microterm::Content` already has room for this in
//!   principle (PORT_STATUS.md notes the strip covers
//!   "status/voice-partial/steno-candidates/LLM-prompt"), but nothing in
//!   this crate constructs that variant from a live `StenoCommit` yet.
//!
//! Each of those is its own multi-file change (layout data + a
//! `demo_app` gesture-dispatch arm + microterm wiring), and — like the
//! cardinal-gesture placement fix before it — deserves reading the GTK
//! reference's actual wiring first rather than guessing the shape.
//! Tracked as the next step, not attempted in this pass.

#[path = "../../src/steno/mod.rs"]
pub mod engine;

use std::sync::Mutex;
use std::time::{Duration, Instant};

/// A stroke fed into the engine that didn't pair immediately, waiting out
/// `engine::PAIR_WINDOW_MS` before this crate flushes it as a solo.
///
/// The GTK build gets this for free from a `glib::timeout_add_local` fired
/// at feed time (see `keyboard_view.rs`'s `handle_steno_gesture`). This
/// crate has no such per-call timer — `demo_app` is an immediate-mode loop
/// with no scheduler — so the deadline is tracked here instead and a
/// per-frame `poll_pending` call (see below) checks whether it has passed.
struct Pending {
    side: engine::StenoSide,
    gen: u64,
    deadline: Instant,
}

static PENDING: Mutex<Option<Pending>> = Mutex::new(None);

/// Feed one steno gesture (a decoded [`crate::real_layout::Press::Steno`])
/// into the engine.
///
/// Returns `Some(commit)` immediately when the gesture paired with the
/// opposite thumb's pending stroke. Otherwise the stroke goes pending —
/// `poll_pending` must be called on subsequent frames for the solo-commit
/// path (a lone stroke past the pairing window) to ever fire; without that
/// call site wired into the app's per-frame update, solo strokes silently
/// never commit. That wiring is NOT part of this module and is not done
/// yet — see `steno.rs`'s module doc.
pub fn feed(button_id: &str, gesture_code: &str) -> Option<engine::StenoCommit> {
    let Some(button) = engine::StenoButton::parse(button_id) else {
        return None;
    };
    let Some(gesture) = parse_gesture(gesture_code) else {
        return None;
    };
    let side = button.side();
    let stroke = engine::StenoStroke { button, gesture };
    let (commit, gen) = engine::global().feed_gesture(stroke);
    if let Some(commit) = commit {
        // Paired immediately — any older pending stroke on either side is
        // now moot (feed_gesture's own generation bump already stales it
        // out on the engine side; clear our copy too so poll_pending
        // doesn't hold a deadline for a stroke that already resolved).
        *PENDING.lock().unwrap() = None;
        return Some(commit);
    }
    *PENDING.lock().unwrap() = Some(Pending {
        side,
        gen,
        deadline: Instant::now() + Duration::from_millis(engine::PAIR_WINDOW_MS),
    });
    None
}

/// Call once per frame (from anywhere — it's not tied to a specific key's
/// draw call) to flush a solo stroke whose pairing window has elapsed.
/// Returns `None` on every frame there's nothing to flush, which is the
/// common case — cheap enough to call unconditionally.
/// How long until `poll_pending` has something to flush, if a stroke is
/// waiting. A host that sleeps between frames asks for a frame at this
/// deadline instead of ticking on a timer to find out.
pub fn pending_remaining() -> Option<Duration> {
    let slot = PENDING.lock().unwrap();
    slot.as_ref().map(|p| p.deadline.saturating_duration_since(Instant::now()))
}

pub fn poll_pending() -> Option<engine::StenoCommit> {
    let mut slot = PENDING.lock().unwrap();
    let ready = matches!(&*slot, Some(p) if Instant::now() >= p.deadline);
    if !ready {
        return None;
    }
    let p = slot.take().unwrap();
    engine::global().flush_solo(p.side, p.gen)
}

fn parse_gesture(code: &str) -> Option<engine::StenoGesture> {
    match code {
        "tap" => Some(engine::StenoGesture::Tap),
        "N" => Some(engine::StenoGesture::SwipeN),
        "E" => Some(engine::StenoGesture::SwipeE),
        "S" => Some(engine::StenoGesture::SwipeS),
        "W" => Some(engine::StenoGesture::SwipeW),
        _ => None,
    }
}

/// Turn a `StenoCommit` into what the microterm strip should show,
/// mirroring GTK's `surface_steno_commit` (`keyboard_view.rs`) exactly:
/// briefs toast their description, phoneme commits with no dictionary
/// match show the raw buffer, and phoneme commits with candidates show
/// the tappable suggestion strip.
///
/// Returns `None` for nothing worth surfacing (there's no such case
/// today — every commit has at least a description or a buffer — but the
/// signature leaves room without every caller needing a default branch).
pub fn commit_to_microterm(commit: &engine::StenoCommit) -> Option<crate::microterm::Content> {
    if commit.was_brief {
        return Some(crate::microterm::Content::Status(commit.description.clone()));
    }
    let buffer = engine::global().buffer_snapshot().join(" ");
    if commit.candidates.is_empty() {
        return Some(crate::microterm::Content::Status(format!("[{buffer}]")));
    }
    let candidates = commit
        .candidates
        .iter()
        .map(|c| crate::microterm::StenoCandidate { word: c.word.clone(), fuzzy: !c.exact })
        .collect();
    Some(crate::microterm::Content::StenoSuggestions { buffer, candidates })
}

#[cfg(test)]
mod tests {
    use super::*;

    // `feed`/`poll_pending` go through `engine::global()`, a genuine
    // process-wide singleton (matching the GTK app's design — there is
    // only ever one steno engine in a running app). That means these
    // tests share state with each other AND run on cargo's default
    // multi-threaded test runner, so two tests interleaving would corrupt
    // each other's pending-stroke bookkeeping (e.g. two different left-
    // side taps from two different tests racing for the engine's single
    // per-side pending slot). Serialize them against this lock rather
    // than against real isolation, since `global()` has none to offer.
    static TEST_LOCK: Mutex<()> = Mutex::new(());

    /// Two opposite-side strokes fed back to back pair immediately — no
    /// polling needed, matching `engine`'s own
    /// `opposite_sides_pair_immediately` test one layer up.
    #[test]
    fn a_pair_commits_without_polling() {
        let _guard = TEST_LOCK.lock().unwrap();
        let commit = feed("L1", "tap");
        assert!(commit.is_none(), "first stroke of a pair is pending");
        let commit = feed("R1", "tap");
        assert!(commit.is_some(), "opposite side completes the pair");
    }

    /// A lone stroke commits only once the pairing window has actually
    /// elapsed, and only via `poll_pending` — not on `feed` itself.
    #[test]
    fn a_lone_stroke_flushes_as_solo_only_after_the_window_and_only_via_poll() {
        let _guard = TEST_LOCK.lock().unwrap();
        let commit = feed("L2", "tap");
        assert!(commit.is_none());
        assert!(poll_pending().is_none(), "window hasn't elapsed yet");
        std::thread::sleep(Duration::from_millis(engine::PAIR_WINDOW_MS + 20));
        let commit = poll_pending();
        assert!(commit.is_some(), "solo should flush once the window has passed");
    }

    #[test]
    fn an_unparseable_button_or_gesture_is_ignored_not_panicking() {
        let _guard = TEST_LOCK.lock().unwrap();
        assert!(feed("not-a-button", "tap").is_none());
        assert!(feed("L1", "not-a-gesture").is_none());
    }

    #[test]
    fn a_brief_commit_becomes_a_status_toast() {
        let _guard = TEST_LOCK.lock().unwrap();
        let commit = engine::StenoCommit {
            text: "the ".into(),
            was_brief: true,
            description: "L1.tap + R1.tap -> the".into(),
            candidates: vec![],
        };
        assert_eq!(
            commit_to_microterm(&commit),
            Some(crate::microterm::Content::Status("L1.tap + R1.tap -> the".into())),
        );
    }

    #[test]
    fn a_phoneme_commit_with_candidates_becomes_suggestions() {
        let _guard = TEST_LOCK.lock().unwrap();
        engine::global().clear_buffer();
        let commit = engine::StenoCommit {
            text: "k".into(),
            was_brief: false,
            description: "L2.N -> k".into(),
            candidates: vec![
                engine::expansion::Candidate { word: "cat".into(), exact: true },
                engine::expansion::Candidate { word: "cot".into(), exact: false },
            ],
        };
        match commit_to_microterm(&commit) {
            Some(crate::microterm::Content::StenoSuggestions { candidates, .. }) => {
                assert_eq!(candidates.len(), 2);
                assert!(!candidates[0].fuzzy);
                assert!(candidates[1].fuzzy);
            }
            other => panic!("expected StenoSuggestions, got {other:?}"),
        }
    }

    #[test]
    fn a_phoneme_commit_with_no_candidates_shows_the_raw_buffer() {
        let _guard = TEST_LOCK.lock().unwrap();
        engine::global().clear_buffer();
        let commit = engine::StenoCommit {
            text: "z".into(),
            was_brief: false,
            description: "L1.W -> z".into(),
            candidates: vec![],
        };
        assert_eq!(
            commit_to_microterm(&commit),
            Some(crate::microterm::Content::Status("[]".into())),
            "empty buffer_snapshot (cleared above) renders as an empty bracket pair"
        );
    }
}
