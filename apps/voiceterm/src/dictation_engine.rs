//! dictation_engine: one object that owns a dictation session end to
//! end, so the Qt side has one method to call instead of five modules
//! to wire together correctly.
//!
//! ## Why a facade, when each piece is already tested
//!
//! Every layer below has its own tests and they all pass. The defects
//! that actually turned up while building this stack were all in the
//! SEAMS — a control id the router could not parse, an option list
//! whose namespacing was nobody's job, a `Route` variant the status
//! model did not handle. Each module was correct; the composition was
//! not.
//!
//! Leaving the composition to the caller means writing it again in QML,
//! in whatever the CLI does, and in every test — and getting it subtly
//! different each time. Specifically, a hand-wired caller has to
//! remember that:
//!
//! - `Session` and `BufferSink` each hold a `Mode`, and `Ctl::SetMode`
//!   has to reach both. Miss one and the router thinks you are dictating
//!   code while the sink is still fixing prose capitalization.
//! - The symbol table only improves if inserted text is merged back
//!   into it.
//! - A repair can FAIL, and the readout must not claim it worked.
//! - `next_utterance()` is per audio burst, not per result, or the fast
//!   path's duplicate guard never clears.
//!
//! None of those are hard. All of them are easy to get wrong once and
//! never notice.
//!
//! PURE: std only. No timers — the caller supplies `now_ms`.

use crate::code_symbols::{prompt_for, PromptKind, SymbolTable};
use crate::dictate::{plan_with, segment_strict, spoken_code_with, BufferSink, Mode, Piece, Placement};
use crate::repair::Repair;
use crate::dictation_session::{DropReason, Policy, Recognition, Route, Session, Tier};
use crate::voice_status::{Event, Status};

/// What one recognition did.
#[derive(Debug, Clone, PartialEq)]
pub struct Outcome {
    /// The router's decision, for logging and for the caller's own UI.
    pub route: Route,
    /// Whether the buffer actually changed. False for drops, for hints,
    /// and for repairs that did not apply.
    pub applied: bool,
    /// The session ended on this result.
    pub ended: bool,
    /// A (misheard, intended) pair the user just volunteered by
    /// correcting us.
    ///
    /// Returned rather than recorded because `confusion_table` holds a
    /// global Mutex and writes to disk, and this crate's pure half must
    /// stay pure. The impure caller does:
    ///
    /// ```text
    /// if let Some((heard, meant)) = outcome.teach {
    ///     confusion_table::learn_from_confirmation(&heard, &meant);
    /// }
    /// ```
    ///
    /// Only emitted when the repair actually applied — a correction that
    /// did not land is not evidence of anything.
    ///
    /// Feeding this in is safe even for pairs that are not really
    /// mishearings, because `learn_from_confirmation` applies its own
    /// similarity filter and `correct()` requires MIN_CONFIRMATIONS
    /// before it acts. Verified against the real table: "read to
    /// strong" -> "read to string" is learned, while "three" -> "nine"
    /// and "read to end" -> "read to string" are dropped as too
    /// dissimilar. That distinction is exactly the one that matters —
    /// it separates "you misheard me" from "I changed my mind" — and it
    /// means this side does not need its own acoustic-plausibility
    /// check.
    pub teach: Option<(String, String)>,
}

/// A live dictation session over one buffer.
pub struct Engine {
    session: Session,
    sink: BufferSink,
    syms: SymbolTable,
    status: Status,
    /// Budget for the whisper prompt, in characters.
    prompt_budget: usize,
}

impl Engine {
    /// `buffer` is the document being dictated into; the symbol table is
    /// seeded from it so the first utterance already benefits from the
    /// identifiers on screen.
    pub fn new(
        buffer: &str,
        cursor: usize,
        mode: Mode,
        policy: Policy,
        cols: usize,
        max_lines: usize,
    ) -> Self {
        let mut e = Engine {
            session: Session::new(mode, policy),
            sink: BufferSink::new(buffer, cursor).with_mode(mode),
            syms: SymbolTable::from_source(buffer),
            status: Status::new(cols, max_lines),
            prompt_budget: 600,
        };
        e.sync_mode();
        e
    }

    // ── inputs ──────────────────────────────────────────────────────

    /// A new audio burst began. Must be called per BURST, not per
    /// result: the fast path and whisper both report on the same burst,
    /// and the duplicate guard depends on the distinction.
    pub fn capture_start(&mut self, now_ms: u64) {
        self.session.next_utterance();
        self.status.on(Event::CaptureStart, now_ms);
    }

    pub fn level(&mut self, level: f32, now_ms: u64) {
        self.status.on(Event::Level(level), now_ms);
    }

    /// Phonetic preview text — display only, never applied.
    pub fn preview(&mut self, text: &str, now_ms: u64) {
        self.status.on(Event::Preview(text.to_string()), now_ms);
    }

    pub fn silence(&mut self, now_ms: u64) {
        self.status.on(Event::Silence, now_ms);
    }

    pub fn tick(&mut self, now_ms: u64) {
        self.status.tick(now_ms);
    }

    /// Route one recognition and apply whatever it means.
    pub fn recognize(&mut self, tier: Tier, text: &str, confidence: f32, now_ms: u64) -> Outcome {
        let r = Recognition { text: text.to_string(), tier, confidence };
        let route = self.session.route(&r);

        // Captured before the repair runs: afterwards the buffer holds
        // the corrected text and the misheard version is gone.
        let unit_before = self.sink.last_unit().map(|s| s.to_string());

        let applied = match &route {
            Route::Content(t) => {
                // segment_strict, not segment: a recognizer classifies a
                // whole burst and never sees half an utterance, so the
                // loose scanner would find controls inside content the
                // user meant literally.
                let pieces = segment_strict(t);
                let edits = plan_with(&pieces, self.sink.mode, Placement::Cursor, Some(&self.syms));
                let before = self.sink.text.len();
                self.sink.apply(&edits);
                let changed = self.sink.text.len() != before;
                if changed {
                    // Only the inserted text, not a full rescan: the
                    // measured full rebuild is cheap but not free, and
                    // merging is what makes a name the user keeps saying
                    // climb the frequency ranking.
                    if let Some(u) = self.sink.last_unit() {
                        let u = u.to_string();
                        // merge_identifiers, not merge_source: feeding
                        // our own prose back in taught the table that
                        // "state" is an identifier, which then beat the
                        // buffer's `State` in path position.
                        self.syms.merge_identifiers(&u);
                    }
                }
                changed
            }
            Route::Control(c) => {
                let edits = plan_with(
                    &[Piece::Control(*c)],
                    self.sink.mode,
                    Placement::Cursor,
                    Some(&self.syms),
                );
                self.sink.apply(&edits);
                self.sync_mode_from_sink();
                true
            }
            Route::Repair(rep) => self.apply_repair(rep),
            Route::Dropped(_) => false,
        };

        // The readout must not claim a repair worked when it did not.
        // Neither the router (which does not touch the buffer) nor the
        // status model (which does not see the sink) can know this on
        // its own — it is exactly the kind of fact only the composition
        // has.
        let shown = if matches!(route, Route::Repair(_)) && !applied {
            Route::Dropped(DropReason::NotApplicable)
        } else {
            route.clone()
        };
        self.status.on(
            Event::Result { tier, text: text.to_string(), route: shown },
            now_ms,
        );

        let teach = match (&route, applied) {
            (Route::Repair(rep), true) => unit_before
                .as_deref()
                .and_then(|u| crate::repair::training_pair(u, rep)),
            _ => None,
        };

        Outcome { route, applied, ended: self.session.ended(), teach }
    }

    /// Apply a repair, accounting for the fact that what is IN the
    /// buffer is the transformed text, not the words that were spoken.
    ///
    /// "change three to five" against `max_retries: 3,` fails naively:
    /// the numeral transform already turned "three" into `3`, so the
    /// target the user named is not there. Same for any identifier the
    /// symbol table rewrote — you said "read to string", the buffer
    /// holds `read_to_string`.
    ///
    /// So a Change is tried against the TRANSFORMED forms of its
    /// operands first, then the literal ones. Transformed first because
    /// that is what the user is looking at; literal as a fallback for
    /// prose and for words the transform leaves alone.
    fn apply_repair(&mut self, rep: &Repair) -> bool {
        if let Repair::Change { from, to } = rep {
            let tf = |w: &str, e: &Engine| {
                spoken_code_with(w, e.sink.mode, Some(&e.syms))
            };
            let (tfrom, tto) = (tf(from, self), tf(to, self));
            if (tfrom.as_str(), tto.as_str()) != (from.as_str(), to.as_str()) {
                let t = Repair::Change { from: tfrom, to: tto };
                if self.sink.repair(&t) {
                    return true;
                }
            }
        }
        self.sink.repair(rep)
    }

    /// Shadow comparison for a burst where both tiers produced text.
    /// Call it even while the fast path is disabled — that is how the
    /// evidence for enabling it accumulates.
    pub fn observe_shadow(&mut self, fast_text: &str, whisper_text: &str) {
        self.session.observe_shadow(fast_text, whisper_text);
    }

    /// Rebuild the symbol table from scratch. For when the buffer
    /// changed underneath us (file reloaded, user typed) rather than
    /// through this engine.
    pub fn reseed(&mut self, buffer: &str, cursor: usize) {
        self.sink = BufferSink::new(buffer, cursor).with_mode(self.sink.mode);
        self.syms = SymbolTable::from_source(buffer);
    }

    // ── outputs ─────────────────────────────────────────────────────

    pub fn text(&self) -> &str {
        &self.sink.text
    }
    pub fn cursor(&self) -> usize {
        self.sink.cursor
    }
    pub fn mode(&self) -> Mode {
        self.sink.mode
    }
    pub fn ended(&self) -> bool {
        self.session.ended()
    }
    pub fn status(&self) -> &Status {
        &self.status
    }
    pub fn session(&self) -> &Session {
        &self.session
    }
    pub fn symbols(&self) -> &SymbolTable {
        &self.syms
    }

    /// The whisper `initial_prompt` for the next utterance, given the
    /// active grammar. `None` means set no prompt.
    pub fn whisper_prompt(&self, grammar_label: &str, lang: &str) -> Option<PromptKind> {
        Some(prompt_for(grammar_label, lang, Some(&self.syms), self.prompt_budget))
    }

    // ── the thing the caller would forget ───────────────────────────

    /// Session and sink each carry a Mode. This keeps them equal.
    fn sync_mode(&mut self) {
        self.sink.mode = self.session.mode;
    }

    /// `Ctl::SetMode` lands on the sink (it arrives as an `Edit`), so
    /// the session has to be told.
    fn sync_mode_from_sink(&mut self) {
        self.session.mode = self.sink.mode;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dictate::Ctl;
    use crate::repair::Repair;
    use crate::voice_status::Note;

    fn eng(buffer: &str) -> Engine {
        Engine::new(
            buffer,
            buffer.len(),
            Mode::Code,
            Policy { fast_enabled: true, ..Default::default() },
            40,
            3,
        )
    }

    fn say(e: &mut Engine, text: &str, t: u64) -> Outcome {
        e.capture_start(t);
        e.recognize(Tier::Whisper, text, 1.0, t + 1200)
    }

    /// Deterministic 64-bit LCG. A dependency-free PRNG so the fuzz
    /// below is reproducible: a failure that only shows up on one
    /// machine is not a failure anyone will fix.
    struct Rng(u64);
    impl Rng {
        fn next(&mut self) -> u64 {
            self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            self.0 >> 11
        }
        fn pick<'a, T>(&mut self, xs: &'a [T]) -> &'a T {
            &xs[(self.next() % xs.len() as u64) as usize]
        }
    }

    /// Realistic dictation, expectations written BEFORE running.
    ///
    /// Everything else in this repo tests correctness — that a function
    /// does what it says. This measures QUALITY: does dictating a real
    /// line of Rust actually produce that line? The first run scored
    /// 6/10, and the four failures were worth more than the six passes:
    /// single-word symbol resolution turning "path" into `Path`,
    /// numerals shredding "f sixty four" into `f 60 4`, and the engine
    /// poisoning its own symbol table with its own prose.
    ///
    /// Expectations are written first and NOT adjusted to match output,
    /// or this becomes a change-detector instead of a measurement. When
    /// a case here is genuinely unreasonable to expect, delete it and
    /// say why — do not weaken it.
    #[test]
    fn realistic_dictation_fixtures() {
        struct Case {
            name: &'static str,
            buf: &'static str,
            mode: Mode,
            say: &'static [&'static str],
            want: &'static str,
        }
        let cases: &[Case] = &[
            Case {
                name: "let binding with call",
                buf: "fn read_to_string(p:&Path)->Result<String>{}\n",
                mode: Mode::Code,
                say: &["let raw equals read to string open paren path close paren question mark semicolon"],
                want: "let raw = read_to_string(path)?;",
            },
            Case {
                name: "struct literal",
                buf: "struct UserStore{max_retries:usize,timeout:u64}\n",
                mode: Mode::Code,
                say: &[
                    "let store equals user store open brace",
                    "new line", "indent",
                    "max retries colon three comma",
                    "new line", "timeout colon thirty comma",
                    "new line", "dedent", "close brace semicolon",
                ],
                want: "let store = UserStore {\n    max_retries: 3,\n    timeout: 30,\n};",
            },
            Case {
                name: "fn signature with primitive types",
                buf: "",
                mode: Mode::Code,
                say: &["pub fn snake compute average open paren snake values colon ampersand open bracket close bracket f sixty four close paren arrow f sixty four open brace"],
                want: "pub fn compute_average(values: &[] f64) -> f64 {",
            },
            Case {
                name: "match arm with enum paths",
                buf: "enum State{Idle,Running}\n",
                mode: Mode::Code,
                say: &[
                    "match state open brace",
                    "new line", "indent",
                    "state double colon idle fat arrow zero comma",
                    "new line", "state double colon running fat arrow one comma",
                    "new line", "dedent", "close brace",
                ],
                want: "match state {\n    State::Idle => 0,\n    State::Running => 1,\n}",
            },
            Case {
                name: "string literal",
                buf: "",
                mode: Mode::Code,
                say: &["let name equals quote Paris quote semicolon"],
                want: "let name = \"Paris\";",
            },
            Case {
                name: "method chain",
                buf: "fn as_str(){} fn to_owned(){}\n",
                mode: Mode::Code,
                say: &["let s equals raw dot as str open paren close paren dot to owned open paren close paren semicolon"],
                want: "let s = raw.as_str().to_owned();",
            },
            Case {
                name: "prose then code",
                buf: "",
                mode: Mode::Prose,
                say: &[
                    "This computes the average.",
                    "code mode", "new line",
                    "let n equals values dot len open paren close paren semicolon",
                ],
                want: "This computes the average.\nlet n = values.len();",
            },
            Case {
                name: "prose paragraph",
                buf: "",
                mode: Mode::Prose,
                say: &[
                    "We tried three approaches.",
                    "The second one worked best.",
                    "It was faster and simpler.",
                ],
                want: "We tried three approaches. The second one worked best. It was faster and simpler.",
            },
            Case {
                name: "repair after misrecognition",
                buf: "fn read_to_end(){}\n",
                mode: Mode::Code,
                say: &[
                    "let n equals read to string semicolon",
                    "replace read to string with read to end",
                ],
                want: "let n = read_to_end;",
            },
            Case {
                name: "const with screaming case",
                buf: "",
                mode: Mode::Code,
                say: &["const screaming max retries colon usize equals five semicolon"],
                want: "const MAX_RETRIES: usize = 5;",
            },
        ];

        let mut failures = Vec::new();
        for c in cases {
            let mut e = Engine::new(c.buf, c.buf.len(), c.mode, Policy::default(), 40, 3);
            let mut t = 0u64;
            for u in c.say {
                e.capture_start(t);
                e.recognize(Tier::Whisper, u, 1.0, t + 300);
                t += 1000;
            }
            let got = &e.text()[c.buf.len()..];
            if got != c.want {
                failures.push(format!("\n  {}\n    want {:?}\n    got  {:?}", c.name, c.want, got));
            }
        }
        assert!(
            failures.is_empty(),
            "{} of {} fixtures regressed:{}",
            failures.len(),
            cases.len(),
            failures.join("")
        );
    }

    /// Prose dictation fixtures. The code fixtures said nothing about
    /// this half, and prose fails differently: whisper's per-utterance
    /// capitalization, its punctuation guesses, and sentence boundaries
    /// hidden behind closing marks.
    ///
    /// First run scored 10/12 and found a real bug (a closing quote hid
    /// the period behind it) plus one decision that measuring reversed
    /// (lowercased proper nouns).
    #[test]
    fn prose_dictation_fixtures() {
        let cases: &[(&str, &[&str], &str)] = &[
            (
                "plain continuation",
                &["the quick brown fox", "Jumped over the lazy dog"],
                "the quick brown fox jumped over the lazy dog",
            ),
            ("sentence boundary", &["She left.", "Then it rained."], "She left. Then it rained."),
            (
                "closing quote hides the period",
                &["He said \"hello.\"", "Then he left."],
                "He said \"hello.\" Then he left.",
            ),
            (
                "abbreviation mid-sentence",
                &["We saw Dr.", "Smith at the clinic."],
                "We saw Dr. Smith at the clinic.",
            ),
            (
                "etc mid-sentence",
                &["We brought food, drinks, etc.", "And a tent."],
                "We brought food, drinks, etc. And a tent.",
            ),
            (
                "closing paren then continue",
                &["It rained (a lot)", "Which ruined the trip."],
                "It rained (a lot) which ruined the trip.",
            ),
            (
                "comma continuation",
                &["Because it rained,", "We stayed inside."],
                "Because it rained, we stayed inside.",
            ),
            (
                "proper noun keeps its capital",
                &["we flew to", "Paris in the spring"],
                "we flew to Paris in the spring",
            ),
            (
                "acronym continuation",
                &["the server uses", "HTTP for everything"],
                "the server uses HTTP for everything",
            ),
            ("question then answer", &["Did it work?", "Yes, mostly."], "Did it work? Yes, mostly."),
            (
                "numeric continuation",
                &["we tried", "3 different approaches"],
                "we tried 3 different approaches",
            ),
            ("the pronoun I", &["yesterday", "I went home"], "yesterday I went home"),
        ];

        let mut failures = Vec::new();
        for (name, say, want) in cases {
            let mut e = Engine::new("", 0, Mode::Prose, Policy::default(), 40, 3);
            let mut t = 0u64;
            for u in *say {
                e.capture_start(t);
                e.recognize(Tier::Whisper, u, 1.0, t + 300);
                t += 1000;
            }
            if e.text() != *want {
                failures.push(format!("\n  {}\n    want {:?}\n    got  {:?}", name, want, e.text()));
            }
        }
        assert!(
            failures.is_empty(),
            "{} of {} prose fixtures regressed:{}",
            failures.len(),
            cases.len(),
            failures.join("")
        );
    }

    /// The same fixtures, degraded the way whisper actually degrades
    /// them. This is the robustness half of the measurement: the clean
    /// fixtures say the transform is right, these say whether it
    /// survives contact with a real recognizer.
    ///
    /// Corruptions are one-at-a-time and drawn from observed whisper
    /// behaviour, not invented: disfluencies transcribed literally,
    /// homophone substitutions for symbol words, split compounds, and
    /// the sentence capital whisper puts on every utterance.
    ///
    /// Two cases here are expected to stay BROKEN, and are asserted as
    /// broken. A dropped word and a duplicated word cannot be repaired
    /// without guessing at every bare noun in the stream, and a visible
    /// artifact the speaker fixes with one phrase beats a silent wrong
    /// guess.
    #[test]
    fn degraded_transcripts_mostly_recover() {
        let buf = "fn read_to_string(p:&Path)->Result<String>{}\n";
        let go = |say: &str| {
            let mut e = Engine::new(buf, buf.len(), Mode::Code, Policy::default(), 40, 3);
            e.capture_start(0);
            e.recognize(Tier::Whisper, say, 1.0, 300);
            e.text()[buf.len()..].to_string()
        };

        // recovered
        for (name, say, want) in [
            ("leading filler", "um let x equals five semicolon", "let x = 5;"),
            ("mid filler", "let x equals uh five semicolon", "let x = 5;"),
            ("filler pair", "let x you know equals five semicolon", "let x = 5;"),
            ("colon as column", "let x column i thirty two equals five semicolon", "let x: i32 = 5;"),
            ("comma as coma", "foo open paren a coma b close paren", "foo(a, b)"),
            ("paren as parent", "foo open parent a close parent", "foo(a)"),
            ("brace as braise", "if x open braise", "if x {"),
            ("split semicolon", "let x equals five semi colon", "let x = 5;"),
            ("equals as equal", "let x equal five semicolon", "let x = 5;"),
            ("sentence capital", "Let x equals five semicolon", "let x = 5;"),
            ("trailing period", "let x equals five semicolon.", "let x = 5;"),
        ] {
            assert_eq!(go(say), want, "{name}");
        }

        // deliberately NOT recovered
        assert_eq!(
            go("foo paren a close paren"),
            "foo paren a)",
            "a dropped word must stay visibly wrong rather than be guessed at"
        );
        assert_eq!(
            go("let let x equals five semicolon"),
            "let let x = 5;",
            "a duplicated word is the speaker's to fix"
        );
    }

    #[test]
    fn random_utterance_streams_never_corrupt_the_buffer() {
        // The engine does byte-index arithmetic throughout — insert_str,
        // replace_range, slicing for the spacer and the last unit. Every
        // hand-written test used ASCII, which is exactly the input that
        // cannot expose a char-boundary bug. This one deliberately does
        // not.
        let corpus = [
            "let x equals five",
            "café noir",
            "Le café était à côté",
            "日本語のテキスト",
            "new line",
            "indent",
            "dedent",
            "scratch that",
            "code mode",
            "prose mode",
            "make that snake case",
            "capitalize that",
            "uppercase that",
            "change five to six",
            "change absent to x",
            "spell it romeo echo alpha delta",
            "i said something else",
            "join that",
            "read to string open paren close paren",
            "quote hello quote",
            "",
            "   ",
            "ΑΒΓ δεζ",
            "emoji 🙂 mid sentence",
        ];
        let buffers = ["", "fn read_to_string() {}\n", "café_noir\n日本\n", "  indented"];
        let tiers = [Tier::Fast, Tier::Whisper];

        let mut rng = Rng(0x5EED);
        for session in 0..400u64 {
            let buf = *rng.pick(&buffers);
            // cursors deliberately include non-boundary and out-of-range
            let cursor = (rng.next() as usize) % (buf.len() + 4);
            let mut e = Engine::new(
                buf,
                cursor,
                *rng.pick(&[Mode::Prose, Mode::Code]),
                Policy { fast_enabled: session % 2 == 0, ..Default::default() },
                *rng.pick(&[8usize, 20, 40]),
                *rng.pick(&[1usize, 3]),
            );
            let mut now = 0u64;
            for _ in 0..12 {
                now += 500;
                e.capture_start(now);
                e.level((rng.next() % 300) as f32 / 100.0 - 0.5, now);
                if rng.next() % 3 == 0 {
                    e.preview(*rng.pick(&corpus), now);
                }
                let tier = *rng.pick(&tiers);
                let conf = (rng.next() % 120) as f32 / 100.0;
                e.recognize(tier, *rng.pick(&corpus), conf, now + 300);
                e.tick(now + 400);

                // invariants
                let t = e.text();
                assert!(t.is_char_boundary(e.cursor()), "cursor split a char: {t:?}");
                assert!(e.cursor() <= t.len(), "cursor past the end");
                assert!(e.status().slot_lines >= 1);
                assert!((0.0..=1.0).contains(&e.status().level), "level escaped its range");
                let _ = e.status().wire();
            }
        }
    }

    #[test]
    fn reseed_with_a_hostile_cursor_is_safe() {
        let mut e = eng("");
        e.reseed("héllo 日本", 3);
        assert!(e.text().is_char_boundary(e.cursor()));
        e.reseed("héllo", 9999);
        assert_eq!(e.cursor(), "héllo".len());
    }

    #[test]
    fn content_inserts_and_uses_the_buffers_identifiers() {
        let mut e = eng("fn read_to_string() {} struct UserStore {}\n");
        say(&mut e, "let s equals read to string open paren close paren semicolon", 0);
        assert!(e.text().ends_with("let s = read_to_string();"), "{}", e.text());
    }

    #[test]
    fn set_mode_reaches_both_the_session_and_the_sink() {
        // the seam a hand-wired caller forgets
        let mut e = eng("");
        assert_eq!(e.mode(), Mode::Code);
        let o = say(&mut e, "prose mode", 0);
        assert_eq!(o.route, Route::Control(Ctl::SetMode(Mode::Prose)));
        assert_eq!(e.mode(), Mode::Prose);
        assert_eq!(e.session().mode, Mode::Prose, "session still thinks it is code");
        // and the change takes effect on the next utterance
        say(&mut e, "Hello there", 2000);
        assert_eq!(e.text(), "Hello there", "prose mode must not run the symbol transform");
    }

    #[test]
    fn dictated_names_join_the_symbol_table() {
        let mut e = eng("");
        // no table entry yet, so the words stay words
        say(&mut e, "compute_average_score", 0);
        assert_eq!(e.text(), "compute_average_score");
        // now the name is known and a spoken run resolves to it
        say(&mut e, "compute average score", 2000);
        assert!(e.text().ends_with("compute_average_score"), "{}", e.text());
    }

    #[test]
    fn a_failed_repair_does_not_claim_success() {
        // the bug only the composition can see: the router says Repair,
        // the sink says it did nothing, and the readout must agree with
        // the sink
        let mut e = eng("");
        say(&mut e, "read to string", 0);
        let o = say(&mut e, "change absent to x", 2000);
        assert!(matches!(o.route, Route::Repair(Repair::Change { .. })));
        assert!(!o.applied);
        assert_eq!(
            e.status().note,
            Some(Note::Ignored(DropReason::NotApplicable)),
            "the readout must agree with the sink, not with the router"
        );
        assert_eq!(e.text(), "read to string", "and nothing changed");
    }

    #[test]
    fn change_names_the_spoken_form_but_the_buffer_holds_the_transformed_one() {
        let mut e = eng("");
        say(&mut e, "retries colon three comma", 0);
        assert_eq!(e.text(), "retries: 3,");
        // the user says "three"; the buffer contains "3"
        let o = say(&mut e, "change three to five", 2000);
        assert!(o.applied, "the repair must see through the numeral transform");
        assert_eq!(e.text(), "retries: 5,");
    }

    #[test]
    fn change_sees_through_the_symbol_table_too() {
        let mut e = eng("fn read_to_string() {} fn read_to_end() {}");
        say(&mut e, "read to string", 0);
        assert!(e.text().ends_with("read_to_string"));
        // "change X to Y" is ambiguous when X contains "to"; the
        // replace/with phrasing exists for exactly this
        let o = say(&mut e, "replace read to string with read to end", 2000);
        assert!(o.applied, "{}", e.text());
        assert!(e.text().ends_with("read_to_end"), "{}", e.text());
    }

    #[test]
    fn a_genuinely_absent_target_still_fails() {
        let mut e = eng("");
        say(&mut e, "retries colon three comma", 0);
        let o = say(&mut e, "change nine to two", 2000);
        assert!(!o.applied);
        assert_eq!(e.text(), "retries: 3,");
    }

    #[test]
    fn a_correction_yields_a_training_pair() {
        let mut e = eng("");
        say(&mut e, "read to end", 0);
        let o = say(&mut e, "no I meant read to string", 2000);
        assert!(o.applied);
        assert_eq!(
            o.teach,
            Some(("read to end".into(), "read to string".into())),
            "the misheard text must be captured BEFORE the repair overwrites it"
        );
        assert_eq!(e.text(), "read to string");
    }

    #[test]
    fn a_reshaping_repair_teaches_nothing() {
        let mut e = eng("");
        say(&mut e, "read to string", 0);
        let o = say(&mut e, "make that snake case", 2000);
        assert!(o.applied);
        assert_eq!(o.teach, None, "whisper heard this correctly; only the shape was wrong");
    }

    #[test]
    fn a_failed_repair_teaches_nothing() {
        let mut e = eng("");
        say(&mut e, "read to string", 0);
        let o = say(&mut e, "change absent to x", 2000);
        assert!(!o.applied);
        assert_eq!(o.teach, None, "a correction that did not land is not evidence");
    }

    #[test]
    fn ordinary_content_teaches_nothing() {
        let mut e = eng("");
        let o = say(&mut e, "let x equals five", 0);
        assert!(o.applied);
        assert_eq!(o.teach, None);
    }

    #[test]
    fn a_successful_repair_is_marked() {
        let mut e = eng("");
        say(&mut e, "read to string", 0);
        let o = say(&mut e, "make that snake case", 2000);
        assert!(o.applied);
        assert_eq!(e.status().note, Some(Note::Repaired));
        assert_eq!(e.text(), "read_to_string");
    }

    #[test]
    fn the_fast_path_burst_guard_clears_per_capture() {
        let mut e = eng("");
        e.capture_start(0);
        assert_eq!(e.recognize(Tier::Fast, "new line", 0.99, 250).route, Route::Control(Ctl::NewLine));
        // whisper catching up on the SAME burst must not fire it twice
        let late = e.recognize(Tier::Whisper, "newline", 1.0, 1400);
        assert_eq!(late.route, Route::Dropped(DropReason::AlreadyActed));
        assert!(!late.applied);
        assert_eq!(e.text(), "\n");
        // a new burst is fair game
        e.capture_start(2000);
        assert_eq!(
            e.recognize(Tier::Fast, "new line", 0.99, 2250).route,
            Route::Control(Ctl::NewLine)
        );
        assert_eq!(e.text(), "\n\n");
    }

    #[test]
    fn fast_path_content_is_never_inserted() {
        let mut e = eng("");
        e.capture_start(0);
        let o = e.recognize(Tier::Fast, "let x equals five", 0.99, 250);
        assert_eq!(o.route, Route::Dropped(DropReason::FastPathContent));
        assert!(!o.applied);
        assert_eq!(e.text(), "");
    }

    #[test]
    fn content_is_classified_whole_utterance_only() {
        // "write a new line to the log" is content, not a NewLine
        let mut e = Engine::new("", 0, Mode::Prose, Policy::default(), 40, 3);
        let o = say(&mut e, "write a new line to the log", 0);
        assert!(matches!(o.route, Route::Content(_)));
        assert_eq!(e.text(), "write a new line to the log");
    }

    #[test]
    fn stop_ends_the_session_and_further_speech_is_inert() {
        let mut e = eng("");
        say(&mut e, "hello", 0);
        let o = say(&mut e, "stop dictating", 2000);
        assert!(o.ended);
        assert!(e.ended());
        let after = say(&mut e, "more words", 4000);
        assert_eq!(after.route, Route::Dropped(DropReason::Ended));
        assert_eq!(e.text(), "hello");
    }

    #[test]
    fn the_whisper_prompt_follows_the_grammar() {
        let e = eng("fn read_to_string() { UserStore::new(); }");
        match e.whisper_prompt("dictation", "en").unwrap() {
            PromptKind::Dictation(p) => assert!(p.contains("read to string"), "{p}"),
            other => panic!("expected Dictation, got {other:?}"),
        }
        assert_eq!(e.whisper_prompt("terminal", "en").unwrap(), PromptKind::Commands);
        assert_eq!(e.whisper_prompt("dictation", "fr").unwrap(), PromptKind::None);
    }

    #[test]
    fn reseed_keeps_the_mode_but_replaces_the_buffer() {
        let mut e = eng("");
        say(&mut e, "prose mode", 0);
        e.reseed("fn helper_name() {}", 19);
        assert_eq!(e.mode(), Mode::Prose, "mode is session state, not buffer state");
        assert_eq!(e.symbols().resolve(&["helper", "name"]), Some("helper_name"));
    }

    #[test]
    fn status_tracks_the_whole_utterance() {
        let mut e = eng("");
        e.capture_start(0);
        e.level(0.4, 50);
        e.preview("let ex equal", 250);
        assert!(e.status().dim, "a preview is provisional");
        e.recognize(Tier::Whisper, "let x equals five", 1.0, 1400);
        assert!(!e.status().dim);
        assert_eq!(e.status().tier, Some(Tier::Whisper));
        assert_eq!(e.text(), "let x = 5");
    }

    #[test]
    fn a_full_session_composes() {
        let mut e = eng("struct UserStore { max_retries: usize }\n");
        let script = [
            "let store equals user store open brace",
            "new line",
            "indent",
            "max retries colon three comma",
            "new line",
            "dedent",
            "close brace semicolon",
            "stop dictating",
        ];
        for (i, u) in script.iter().enumerate() {
            say(&mut e, u, i as u64 * 2000);
        }
        assert!(e.ended());
        let produced = &e.text()[40..];
        assert_eq!(produced, "let store = UserStore {\n    max_retries: 3,\n};");
    }
}
