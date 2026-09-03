//! voice_status: the model behind the voice-feedback readout — what the
//! status surface shows at each moment of an utterance.
//!
//! ## The problem this solves
//!
//! Recognition is two-tier: a phonetic preview lands at ~250ms and
//! whisper finalizes at ~1000–1800ms. So the text on screen CHANGES
//! under the reader, sometimes into something quite different. Rendered
//! naively that reads as a glitch, and the user learns to distrust the
//! readout.
//!
//! Three rules, all enforced here rather than in QML:
//!
//! 1. **Preview and final share one slot.** Same position, same size.
//!    Preview is marked `dim`; the final is solid. Nothing moves when
//!    the swap happens — only the styling changes.
//! 2. **The slot never shrinks mid-utterance.** `slot_lines` is a
//!    high-water mark reset only at capture start. A three-line preview
//!    replaced by a one-line final leaves two blank lines rather than
//!    yanking the layout up. Reflow is the thing that reads as broken;
//!    whitespace is not.
//! 3. **The tier is visible.** When the acoustic fast path answers
//!    without whisper, the user must be able to tell, because that is
//!    the path that will occasionally be confidently wrong and they
//!    need to learn its signature.
//!
//! ## Placement
//!
//! This is STATUS, not control: no thumb required, and it must survive
//! the on-screen keyboard. On a phone that argues for the top of the
//! screen, not the bottom — a bottom-anchored readout is occluded by the
//! IME exactly while you are dictating, and `Qt.inputMethod.keyboardRectangle`
//! reports an empty rect on several wlroots/phosh compositors, so you
//! cannot reliably reposition around it. Actions stay at the bottom
//! where thumbs are. This module makes no placement decision; it just
//! produces something small enough to live in a strip.
//!
//! PURE: std only, no timers of its own — the caller supplies `now_ms`.

use crate::dictation_session::{DropReason, Route, Tier};

/// What the readout is doing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    /// Not listening.
    Idle,
    /// Capturing, nothing recognized yet.
    Listening,
    /// Showing a phonetic preview; whisper still running.
    Preview,
    /// A result has landed and been acted on.
    Settled,
}

/// A transient badge alongside the transcript — why something did or did
/// not happen. Transient because a status line that accumulates becomes
/// a log, and a log in a strip is noise.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Note {
    /// A control verb fired.
    Acted,
    /// The last unit was fixed rather than re-dictated.
    Repaired,
    /// Heard, deliberately not acted on.
    Ignored(DropReason),
    /// Session finished.
    Ended,
}

/// How long a transient note stays up. Long enough to read one short
/// phrase, short enough that it is gone before the next utterance
/// finishes.
pub const NOTE_TTL_MS: u64 = 2500;

/// Events the readout reacts to.
#[derive(Debug, Clone)]
pub enum Event {
    /// Capture began — a new utterance. Resets the anti-reflow
    /// high-water mark.
    CaptureStart,
    /// Smoothed mic level, 0..1.
    Level(f32),
    /// Phonetic preview text (the ~250ms tier).
    Preview(String),
    /// A recognition result and what the router decided about it.
    Result { tier: Tier, text: String, route: Route },
    /// Capture stopped with nothing recognized.
    Silence,
}

/// The renderable state. QML binds to these fields and does no logic.
#[derive(Debug, Clone)]
pub struct Status {
    pub phase: Phase,
    /// Text for the transcript slot (already tail-trimmed).
    pub text: String,
    /// Render dimmed — this is a preview that may still change.
    pub dim: bool,
    /// Which recognizer produced `text`, when one has.
    pub tier: Option<Tier>,
    /// Smoothed 0..1 mic level.
    pub level: f32,
    pub note: Option<Note>,
    note_at: u64,
    /// Anti-reflow high-water mark: lines the slot must reserve for the
    /// utterance in flight. Never decreases until the next
    /// `CaptureStart`.
    pub slot_lines: usize,
    cols: usize,
    max_lines: usize,
}

impl Status {
    /// `cols` is the strip's width in characters and `max_lines` the
    /// most it may ever grow to. Both are layout facts the caller knows
    /// and this module does not.
    pub fn new(cols: usize, max_lines: usize) -> Self {
        Status {
            phase: Phase::Idle,
            text: String::new(),
            dim: false,
            tier: None,
            level: 0.0,
            note: None,
            note_at: 0,
            slot_lines: 1,
            cols: cols.max(8),
            max_lines: max_lines.max(1),
        }
    }

    pub fn on(&mut self, ev: Event, now_ms: u64) {
        match ev {
            Event::CaptureStart => {
                self.phase = Phase::Listening;
                self.text.clear();
                self.dim = false;
                self.tier = None;
                self.note = None;
                // the only place the slot is allowed to shrink
                self.slot_lines = 1;
            }
            Event::Level(l) => {
                self.level = l.clamp(0.0, 1.0);
            }
            Event::Preview(t) => {
                self.set_text(&t);
                self.phase = Phase::Preview;
                self.dim = true;
                self.tier = None;
            }
            Event::Result { tier, text, route } => {
                self.phase = Phase::Settled;
                self.tier = Some(tier);
                match route {
                    Route::Content(c) => {
                        self.set_text(&c);
                        self.dim = false;
                        self.note = None;
                    }
                    Route::Control(_) => {
                        self.set_text(&text);
                        self.dim = false;
                        self.set_note(Note::Acted, now_ms);
                    }
                    Route::Repair(_) => {
                        // Show the repair phrase, not the repaired text:
                        // the buffer already shows the result, and
                        // echoing it here would say the same thing twice
                        // in two places that can disagree.
                        self.set_text(&text);
                        self.dim = false;
                        self.set_note(Note::Repaired, now_ms);
                    }
                    Route::Dropped(r) => {
                        // Still show what was heard. A drop with no text
                        // is indistinguishable from not being heard at
                        // all, which is the worst failure mode a voice
                        // UI has.
                        self.set_text(&text);
                        self.dim = true;
                        let n = if r == DropReason::Ended { Note::Ended } else { Note::Ignored(r) };
                        self.set_note(n, now_ms);
                    }
                }
            }
            Event::Silence => {
                self.phase = Phase::Idle;
                self.dim = true;
                self.level = 0.0;
            }
        }
    }

    /// Expire the transient note. Call on the UI tick.
    pub fn tick(&mut self, now_ms: u64) {
        if self.note.is_some() && now_ms.saturating_sub(self.note_at) >= NOTE_TTL_MS {
            self.note = None;
        }
    }

    fn set_note(&mut self, n: Note, now_ms: u64) {
        self.note = Some(n);
        self.note_at = now_ms;
    }

    fn set_text(&mut self, t: &str) {
        self.text = tail(t.trim(), self.cols * self.max_lines);
        let n = wrapped_lines(&self.text, self.cols).min(self.max_lines);
        // high-water only
        self.slot_lines = self.slot_lines.max(n).max(1);
    }

    /// Wire format for QML, one field per tab:
    /// `phase\ttext\tdim\ttier\tlevel\tnote\tslot_lines`.
    /// Matches the existing backend convention of shipping a flat row
    /// rather than a QObject per update.
    pub fn wire(&self) -> String {
        let tier = match self.tier {
            Some(Tier::Fast) => "fast",
            Some(Tier::Whisper) => "whisper",
            None => "",
        };
        let note = match self.note {
            None => String::new(),
            Some(Note::Acted) => "acted".into(),
            Some(Note::Repaired) => "repaired".into(),
            Some(Note::Ended) => "ended".into(),
            Some(Note::Ignored(r)) => format!("ignored:{}", drop_label(r)),
        };
        format!(
            "{}\t{}\t{}\t{}\t{:.3}\t{}\t{}",
            phase_label(self.phase),
            self.text.replace('\t', " ").replace('\n', " "),
            if self.dim { 1 } else { 0 },
            tier,
            self.level,
            note,
            self.slot_lines
        )
    }
}

fn phase_label(p: Phase) -> &'static str {
    match p {
        Phase::Idle => "idle",
        Phase::Listening => "listening",
        Phase::Preview => "preview",
        Phase::Settled => "settled",
    }
}

/// Short, user-facing reason. Not jargon: the strip is read at a glance
/// mid-sentence, and "FastPathContent" means nothing to the person
/// speaking.
pub fn drop_label(r: DropReason) -> &'static str {
    match r {
        DropReason::Empty => "nothing heard",
        // The expected path for every content utterance, so it is
        // phrased as waiting rather than as a rejection.
        DropReason::FastPathContent => "waiting for transcription",
        DropReason::LowConfidence => "not sure",
        DropReason::Ended => "dictation ended",
        DropReason::AlreadyActed => "already done",
    }
}

/// Keep the LAST `max` chars, on a word boundary where possible. The
/// tail, not the head: during dictation the interesting words are the
/// ones just spoken.
pub fn tail(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let chars: Vec<char> = s.chars().collect();
    let start = chars.len() - max;
    let slice: String = chars[start..].iter().collect();
    match slice.find(' ') {
        Some(i) if i < max / 4 => format!("…{}", &slice[i + 1..]),
        _ => format!("…{}", slice),
    }
}

/// Lines this text occupies at `cols` width, wrapping on words.
pub fn wrapped_lines(s: &str, cols: usize) -> usize {
    if s.is_empty() {
        return 1;
    }
    let cols = cols.max(1);
    let mut lines = 1;
    let mut used = 0;
    for w in s.split_whitespace() {
        let n = w.chars().count();
        if used == 0 {
            used = n;
        } else if used + 1 + n <= cols {
            used += 1 + n;
        } else {
            lines += 1;
            used = n;
        }
        // a single word longer than the line wraps hard
        while used > cols {
            lines += 1;
            used -= cols;
        }
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dictate::Ctl;

    fn st() -> Status {
        Status::new(20, 3)
    }

    #[test]
    fn preview_is_dim_and_final_is_solid_in_the_same_slot() {
        let mut s = st();
        s.on(Event::CaptureStart, 0);
        s.on(Event::Preview("let ex equal".into()), 100);
        assert_eq!(s.phase, Phase::Preview);
        assert!(s.dim);
        s.on(
            Event::Result {
                tier: Tier::Whisper,
                text: "let x equals five".into(),
                route: Route::Content("let x equals five".into()),
            },
            1300,
        );
        assert_eq!(s.phase, Phase::Settled);
        assert!(!s.dim);
        assert_eq!(s.text, "let x equals five");
    }

    #[test]
    fn slot_never_shrinks_within_an_utterance() {
        let mut s = st(); // 20 cols
        s.on(Event::CaptureStart, 0);
        s.on(Event::Preview("one two three four five six seven".into()), 100);
        let wide = s.slot_lines;
        assert!(wide >= 2, "expected a multi-line preview, got {wide}");
        s.on(
            Event::Result { tier: Tier::Whisper, text: "ok".into(), route: Route::Content("ok".into()) },
            1300,
        );
        assert_eq!(s.slot_lines, wide, "the layout must not yank upward");
    }

    #[test]
    fn slot_resets_on_the_next_capture() {
        let mut s = st();
        s.on(Event::CaptureStart, 0);
        s.on(Event::Preview("one two three four five six seven".into()), 100);
        assert!(s.slot_lines > 1);
        s.on(Event::CaptureStart, 2000);
        assert_eq!(s.slot_lines, 1);
    }

    #[test]
    fn slot_is_capped_at_max_lines() {
        let mut s = Status::new(10, 2);
        s.on(Event::CaptureStart, 0);
        s.on(Event::Preview("aaa bbb ccc ddd eee fff ggg hhh iii".into()), 10);
        assert!(s.slot_lines <= 2);
    }

    #[test]
    fn tier_is_visible_when_the_fast_path_answers() {
        let mut s = st();
        s.on(Event::CaptureStart, 0);
        s.on(
            Event::Result {
                tier: Tier::Fast,
                text: "new line".into(),
                route: Route::Control(Ctl::NewLine),
            },
            250,
        );
        assert_eq!(s.tier, Some(Tier::Fast));
        assert!(s.wire().contains("\tfast\t"));
    }

    #[test]
    fn a_drop_still_shows_what_was_heard() {
        let mut s = st();
        s.on(Event::CaptureStart, 0);
        s.on(
            Event::Result {
                tier: Tier::Fast,
                text: "indent".into(),
                route: Route::Dropped(DropReason::LowConfidence),
            },
            250,
        );
        // silence here would be indistinguishable from not being heard
        assert_eq!(s.text, "indent");
        assert!(s.dim);
        assert_eq!(s.note, Some(Note::Ignored(DropReason::LowConfidence)));
    }

    #[test]
    fn the_expected_fast_path_drop_reads_as_waiting() {
        assert_eq!(drop_label(DropReason::FastPathContent), "waiting for transcription");
    }

    #[test]
    fn a_repair_is_marked_and_not_dimmed() {
        use crate::repair::Repair;
        let mut s = st();
        s.on(Event::CaptureStart, 0);
        s.on(
            Event::Result {
                tier: Tier::Whisper,
                text: "make that snake case".into(),
                route: Route::Repair(Repair::Capitalize),
            },
            1200,
        );
        assert_eq!(s.note, Some(Note::Repaired));
        assert!(!s.dim);
        // the phrase, not the result — the buffer shows the result
        assert_eq!(s.text, "make that snake case");
        assert!(s.wire().contains("repaired"));
    }

    #[test]
    fn notes_expire() {
        let mut s = st();
        s.on(Event::CaptureStart, 0);
        s.on(
            Event::Result { tier: Tier::Fast, text: "new line".into(), route: Route::Control(Ctl::NewLine) },
            1000,
        );
        assert_eq!(s.note, Some(Note::Acted));
        s.tick(1000 + NOTE_TTL_MS - 1);
        assert_eq!(s.note, Some(Note::Acted));
        s.tick(1000 + NOTE_TTL_MS);
        assert_eq!(s.note, None);
    }

    #[test]
    fn level_is_clamped() {
        let mut s = st();
        s.on(Event::Level(1.7), 0);
        assert_eq!(s.level, 1.0);
        s.on(Event::Level(-0.2), 0);
        assert_eq!(s.level, 0.0);
    }

    #[test]
    fn long_text_keeps_the_tail() {
        let t = tail("alpha beta gamma delta epsilon zeta", 14);
        assert!(t.starts_with('…'));
        assert!(t.ends_with("zeta"), "{t}");
        assert_eq!(tail("short", 20), "short");
    }

    #[test]
    fn wrapped_lines_counts_word_wrap() {
        assert_eq!(wrapped_lines("", 10), 1);
        assert_eq!(wrapped_lines("abc def", 10), 1);
        assert_eq!(wrapped_lines("abcde fghij", 10), 2);
        // a single overlong word wraps hard rather than looping forever
        assert_eq!(wrapped_lines("aaaaaaaaaaaaaaaaaaaaa", 10), 3);
    }

    #[test]
    fn wire_is_flat_and_single_line() {
        let mut s = st();
        s.on(Event::CaptureStart, 0);
        s.on(Event::Preview("a\tb\nc".into()), 10);
        let w = s.wire();
        assert_eq!(w.matches('\t').count(), 6, "{w}");
        assert!(!w.contains('\n'));
    }

    #[test]
    fn silence_returns_to_idle_without_clearing_the_text() {
        let mut s = st();
        s.on(Event::CaptureStart, 0);
        s.on(Event::Preview("maybe".into()), 100);
        s.on(Event::Silence, 900);
        assert_eq!(s.phase, Phase::Idle);
        assert_eq!(s.level, 0.0);
        // what was heard stays visible, dimmed
        assert_eq!(s.text, "maybe");
        assert!(s.dim);
    }
}
