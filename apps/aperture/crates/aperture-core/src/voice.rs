//! voice — the text-side half of voice command support: turns a decoded
//! utterance into a structured command an editor shell can act on.
//!
//! This is deliberately NOT a speech pipeline. aperture has no microphone, no
//! ASR model, no wake-word detector — building one from scratch here would
//! duplicate `shared/voice-kit` and `apps/voiceterm/egui-frontend/src/
//! voice_routing.rs`'s hallucination-trimming, wake-prefix, and consumer-
//! arbitration work for no reason, badly, and without voiceterm's mobile
//! battery/posture-engine integration. Those own "how do we hear" and "who
//! gets to act". What THIS module owns is the piece neither of them does:
//! turning an already-decoded transcript into an EDITOR command.
//!
//! The shape mirrors voice_routing's `WAKE_PREFIX` idea one level down: a
//! command vocabulary has to be small and unambiguous, because every phrase
//! in it is one a person might say for an unrelated reason (dictating prose
//! that happens to contain the word "save") and have it misfire. This parser
//! does not try to be forgiving about that — an utterance either matches one
//! of a short, literal set of command shapes or it comes back `Unknown`,
//! same as voice_routing's own philosophy that a near-miss should not
//! execute anything.
//!
//! Wiring a real recognizer's dictation sink to [`parse`] is future work
//! (voiceterm's own `Listening::Commands` state is the model to follow: this
//! parser would be the intent matcher voice_routing.rs describes as
//! "suspended during dictation," not something that runs on every
//! transcript unconditionally). Until then, the palette's "Voice: run
//! command…" entry is a typed stand-in for what a transcript would say —
//! useful on its own for testing the command shapes, and for anyone who
//! wants voice-command muscle memory without a working microphone yet.

/// A parsed voice command. `Unknown` carries the original text back so a
/// caller can show it in a status message — "didn't understand: %s" is a
/// far better failure than silently doing nothing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum VoiceCommand {
    Save,
    Undo,
    Redo,
    SelectAll,
    Copy,
    Cut,
    Paste,
    NewLine,
    Escape,
    /// "find <query>" / "search for <query>".
    Find(String),
    /// "go to line <n>" / "line <n>", ONE-indexed the way a person says it.
    GoToLine(usize),
    /// "select next occurrence" / "add cursor" / "next cursor" — Ctrl+D.
    AddNextCursor,
    /// "select all occurrences" — Ctrl+Shift+L.
    SelectAllOccurrences,
    /// "type <words>" — the escape hatch for literal text, since almost
    /// nothing else in the vocabulary should be ambiguous with prose but
    /// prose itself has no other way in through a command parser.
    Type(String),
    /// "go to <name>" / "jump to <name>" resolved against a live VOCABULARY
    /// — the project's own symbol names — rather than a fixed word list.
    /// See [`parse_with_vocab`]; plain [`parse`] never produces this
    /// variant, since it has no vocabulary to match against.
    GoToSymbol(String),
    Unknown(String),
}

/// Parse one utterance. Case- and surrounding-whitespace-insensitive;
/// internal spacing is normalized (collapsed runs, so "go   to line   5"
/// still matches) because word-boundary noise like that is exactly what a
/// real recognizer produces and a command grammar that only accepts a
/// single canonical spacing would fail on real transcripts far more often
/// than on the clean strings a test suite tends to reach for.
pub fn parse(utterance: &str) -> VoiceCommand {
    let normalized = utterance.split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase();
    match normalized.as_str() {
        "save" | "save file" => return VoiceCommand::Save,
        "undo" => return VoiceCommand::Undo,
        "redo" => return VoiceCommand::Redo,
        "select all" => return VoiceCommand::SelectAll,
        "copy" => return VoiceCommand::Copy,
        "cut" => return VoiceCommand::Cut,
        "paste" => return VoiceCommand::Paste,
        "new line" | "next line" => return VoiceCommand::NewLine,
        "escape" | "cancel" | "never mind" => return VoiceCommand::Escape,
        "add cursor" | "next cursor" | "select next occurrence" => {
            return VoiceCommand::AddNextCursor
        }
        "select all occurrences" | "select occurrences" => {
            return VoiceCommand::SelectAllOccurrences
        }
        _ => {}
    }
    for prefix in ["find ", "search for "] {
        if let Some(rest) = normalized.strip_prefix(prefix) {
            let q = rest.trim();
            if !q.is_empty() {
                return VoiceCommand::Find(q.to_string());
            }
        }
    }
    for prefix in ["go to line ", "line "] {
        if let Some(rest) = normalized.strip_prefix(prefix) {
            if let Ok(n) = rest.trim().parse::<usize>() {
                if n > 0 {
                    return VoiceCommand::GoToLine(n);
                }
            }
        }
    }
    if let Some(rest) = normalized.strip_prefix("type ") {
        if !rest.is_empty() {
            // The ORIGINAL text, not `normalized` — lowercasing and
            // whitespace-collapsing are fine for matching a command shape
            // but would corrupt literal text the person is dictating
            // ("Type IndexOutOfBounds" should not become "type indexoutofbounds").
            let lower_prefix_len = utterance.to_lowercase().find("type ").map(|i| i + 5);
            if let Some(i) = lower_prefix_len {
                return VoiceCommand::Type(utterance[i..].trim().to_string());
            }
        }
    }
    VoiceCommand::Unknown(utterance.to_string())
}

// ── vocabulary buildup: navigating by a project's own names ────────────────
//
// The literal command grammar above never changes shape — that's the whole
// point of keeping it small and unforgiving. But "go to compute total"
// isn't a command word at all; it names something in THIS project, and
// what that something is changes every time a file opens or a function
// gets renamed. That's a different kind of matching problem — fuzzy,
// data-dependent — and it belongs in its own function rather than bolted
// onto `parse`'s literal-match `match` arms.

/// Like [`parse`], but tries a VOCABULARY-matched navigation command first
/// when the literal grammar doesn't recognize the utterance. `vocab` is
/// whatever the caller currently considers "things in this project with a
/// name" — `aperture_core::syntax::LineLexer::outline`'s symbol names is
/// the obvious source, grown incrementally as files open, exactly the
/// "vocabulary buildup" a project-aware voice interface needs and a fixed
/// command list structurally cannot provide.
pub fn parse_with_vocab(utterance: &str, vocab: &[String]) -> VoiceCommand {
    let cmd = parse(utterance);
    if !matches!(cmd, VoiceCommand::Unknown(_)) {
        return cmd;
    }
    let normalized = utterance.split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase();
    for prefix in ["go to ", "jump to ", "navigate to "] {
        if let Some(rest) = normalized.strip_prefix(prefix) {
            if let Some(name) = best_vocab_match(rest, vocab) {
                return VoiceCommand::GoToSymbol(name);
            }
        }
    }
    cmd
}

/// Match spoken words against a vocabulary of real identifiers.
///
/// Deliberately substring/prefix matching, not a general fuzzy scorer:
/// `uikit_egui::fuzzy` already exists and does that well, but it lives in
/// a UI crate `aperture-core` cannot depend on without giving up the
/// "headless, no egui" property every other module here keeps. A person
/// saying "compute total" for `compute_total` needs exactly one
/// transformation (join spoken words with underscores, since a
/// recognizer emits pauses as spaces, not literal underscores) and a
/// substring match against real names — reinventing full fuzzy scoring
/// for that one transformation would be solving a bigger problem than
/// this one has.
fn best_vocab_match(spoken: &str, vocab: &[String]) -> Option<String> {
    let joined = spoken.split_whitespace().collect::<Vec<_>>().join("_");
    let mut best: Option<(u32, &String)> = None;
    for name in vocab {
        let lname = name.to_lowercase();
        let score = if lname == spoken || lname == joined {
            3 // exact match, either spelled-out-with-spaces or joined
        } else if lname.starts_with(&joined) || lname.starts_with(spoken) {
            2 // a real prefix — "compute" matching "compute_total"
        } else if lname.contains(&joined) {
            1 // present somewhere in a longer name
        } else {
            continue;
        };
        if best.as_ref().is_none_or(|(s, _)| score > *s) {
            best = Some((score, name));
        }
    }
    best.map(|(_, name)| name.clone())
}

/// Recent commands per speaker — the shape "multiple people share one
/// voice-driven session" actually needs. Nothing here does audio: no
/// microphone, no diarization, no voiceprint. `speaker` is whatever
/// upstream identity a real pipeline would hand this layer (a
/// `SiteId`-shaped string, matching `aperture_collab`'s own naming, since
/// that crate is the monorepo's existing multi-participant identity model
/// and inventing a second one here would just be two things to keep in
/// sync). What THIS module owns is what happens once an utterance is
/// already attributed to someone: each speaker's "again"/"repeat that"
/// replays THEIR OWN last command, not whoever spoke most recently
/// session-wide — the one piece of multi-user behavior that is wrong by
/// default if you don't build it in on purpose.
pub struct VoiceSession {
    vocab: Vec<String>,
    /// Bounded per speaker — only the last command is ever read back
    /// (by "again"), so nothing needs more than that; kept as a small
    /// ring rather than a single `Option` only because a `VecDeque` makes
    /// "oldest falls off" free instead of a manual check at every push.
    history: std::collections::HashMap<String, std::collections::VecDeque<VoiceCommand>>,
}

const HISTORY_PER_SPEAKER: usize = 4;

impl VoiceSession {
    pub fn new() -> Self {
        VoiceSession { vocab: Vec::new(), history: std::collections::HashMap::new() }
    }

    /// Replace the live vocabulary — called whenever the caller's symbol
    /// source changes (a file opened, a background re-index finished).
    /// Wholesale replacement, not incremental add/remove: the caller
    /// already has to recompute the full set to know what to pass (an
    /// outline walk has no cheaper way to report "what got removed"), so
    /// asking it to diff against what this module remembers would just
    /// move the same work here with less information to do it well.
    pub fn set_vocab(&mut self, vocab: Vec<String>) {
        self.vocab = vocab;
    }

    /// Parse `utterance` as spoken by `speaker`, resolving "again"/"repeat
    /// that" against THAT speaker's own last command before falling
    /// through to vocabulary-aware parsing, and remembering the result
    /// under their name either way.
    pub fn handle(&mut self, speaker: &str, utterance: &str) -> VoiceCommand {
        let normalized = utterance.trim().to_lowercase();
        let cmd = if normalized == "again" || normalized == "repeat" || normalized == "repeat that" {
            self.history.get(speaker).and_then(|h| h.back()).cloned()
                .unwrap_or_else(|| VoiceCommand::Unknown(utterance.to_string()))
        } else {
            parse_with_vocab(utterance, &self.vocab)
        };
        let h = self.history.entry(speaker.to_string()).or_default();
        h.push_back(cmd.clone());
        while h.len() > HISTORY_PER_SPEAKER {
            h.pop_front();
        }
        cmd
    }
}

impl Default for VoiceSession {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_commands_match_case_and_whitespace_insensitively() {
        assert_eq!(parse("save"), VoiceCommand::Save);
        assert_eq!(parse("  SAVE  "), VoiceCommand::Save);
        assert_eq!(parse("Save File"), VoiceCommand::Save);
        assert_eq!(parse("undo"), VoiceCommand::Undo);
        assert_eq!(parse("REDO"), VoiceCommand::Redo);
    }

    #[test]
    fn internal_whitespace_noise_is_collapsed() {
        // What a real recognizer's word-timing gaps actually produce.
        assert_eq!(parse("go   to  line   5"), VoiceCommand::GoToLine(5));
        assert_eq!(parse("select  all"), VoiceCommand::SelectAll);
    }

    #[test]
    fn find_and_search_for_are_synonyms() {
        assert_eq!(parse("find total"), VoiceCommand::Find("total".to_string()));
        assert_eq!(parse("search for the total"), VoiceCommand::Find("the total".to_string()));
    }

    #[test]
    fn find_with_no_query_is_unknown_rather_than_an_empty_search() {
        // "find" alone is ambiguous — the person may have paused mid-command
        // — and an empty find query would just clear the bar with no
        // useful feedback about what happened.
        assert_eq!(parse("find"), VoiceCommand::Unknown("find".to_string()));
        assert_eq!(parse("find "), VoiceCommand::Unknown("find ".to_string()));
    }

    #[test]
    fn go_to_line_accepts_both_phrasings_and_rejects_nonsense() {
        assert_eq!(parse("go to line 12"), VoiceCommand::GoToLine(12));
        assert_eq!(parse("line 12"), VoiceCommand::GoToLine(12));
        assert_eq!(parse("go to line zero"), VoiceCommand::Unknown("go to line zero".to_string()),
            "spoken numbers aren't parsed yet — that's honestly future work, not silently wrong");
        assert_eq!(parse("go to line 0"), VoiceCommand::Unknown("go to line 0".to_string()),
            "line 0 doesn't exist in a 1-indexed world; falling through to Unknown beats jumping to line -1");
    }

    #[test]
    fn multicursor_phrasings_map_to_the_right_gesture() {
        assert_eq!(parse("add cursor"), VoiceCommand::AddNextCursor);
        assert_eq!(parse("next cursor"), VoiceCommand::AddNextCursor);
        assert_eq!(parse("select next occurrence"), VoiceCommand::AddNextCursor);
        assert_eq!(parse("select all occurrences"), VoiceCommand::SelectAllOccurrences);
    }

    #[test]
    fn type_preserves_the_original_case_and_spacing_of_the_dictated_text() {
        // The command word itself is match-insensitive, but what follows it
        // is content, not vocabulary, and must survive verbatim.
        assert_eq!(
            parse("Type IndexOutOfBounds"),
            VoiceCommand::Type("IndexOutOfBounds".to_string())
        );
        assert_eq!(
            parse("type   two   spaces"),
            VoiceCommand::Type("two   spaces".to_string()),
            "internal whitespace collapsing is a MATCHING concern, not something to apply to dictated content"
        );
    }

    #[test]
    fn an_unrecognized_utterance_carries_its_original_text_back() {
        // So a caller can show "didn't understand: ..." instead of doing
        // nothing silently — the same near-miss-should-not-execute
        // philosophy voice_routing.rs describes, applied to feedback.
        let u = "please refactor this for me";
        assert_eq!(parse(u), VoiceCommand::Unknown(u.to_string()));
    }

    #[test]
    fn escape_synonyms_all_map_to_the_same_command() {
        assert_eq!(parse("escape"), VoiceCommand::Escape);
        assert_eq!(parse("cancel"), VoiceCommand::Escape);
        assert_eq!(parse("never mind"), VoiceCommand::Escape);
    }

    // ── vocab buildup ────────────────────────────────────────────────────

    #[test]
    fn plain_parse_never_produces_go_to_symbol_with_no_vocab_given() {
        // parse() has no vocabulary to consult at all — this is the
        // contract parse_with_vocab's doc promises.
        assert_eq!(parse("go to compute total"), VoiceCommand::Unknown("go to compute total".to_string()));
    }

    #[test]
    fn spoken_underscores_as_spaces_resolve_to_the_real_identifier() {
        let vocab = vec!["compute_total".to_string(), "compute_tax".to_string()];
        assert_eq!(
            parse_with_vocab("go to compute total", &vocab),
            VoiceCommand::GoToSymbol("compute_total".to_string())
        );
    }

    #[test]
    fn jump_to_and_navigate_to_are_synonyms_for_go_to() {
        let vocab = vec!["compute_total".to_string()];
        assert_eq!(parse_with_vocab("jump to compute total", &vocab), VoiceCommand::GoToSymbol("compute_total".to_string()));
        assert_eq!(parse_with_vocab("navigate to compute total", &vocab), VoiceCommand::GoToSymbol("compute_total".to_string()));
    }

    #[test]
    fn a_partial_name_matches_by_prefix() {
        let vocab = vec!["compute_total".to_string(), "render_frame".to_string()];
        assert_eq!(parse_with_vocab("go to compute", &vocab), VoiceCommand::GoToSymbol("compute_total".to_string()));
    }

    #[test]
    fn an_exact_match_wins_over_a_mere_prefix_when_both_exist() {
        let vocab = vec!["total".to_string(), "total_with_tax".to_string()];
        assert_eq!(parse_with_vocab("go to total", &vocab), VoiceCommand::GoToSymbol("total".to_string()),
            "the exact name, not the longer one it happens to prefix-match");
    }

    #[test]
    fn a_name_with_no_vocabulary_match_is_unknown_not_a_guess() {
        let vocab = vec!["compute_total".to_string()];
        assert_eq!(
            parse_with_vocab("go to something_else_entirely", &vocab),
            VoiceCommand::Unknown("go to something_else_entirely".to_string())
        );
    }

    #[test]
    fn an_empty_vocabulary_never_matches() {
        assert_eq!(
            parse_with_vocab("go to anything", &[]),
            VoiceCommand::Unknown("go to anything".to_string())
        );
    }

    #[test]
    fn a_literal_command_still_wins_even_when_vocab_would_also_match() {
        // "save" is a real command word; it must never be shadowed by a
        // same-named vocabulary entry (an unlikely but real possibility —
        // a function actually named `save`).
        let vocab = vec!["save".to_string()];
        assert_eq!(parse_with_vocab("save", &vocab), VoiceCommand::Save);
    }

    // ── multi-user sessions ──────────────────────────────────────────────

    #[test]
    fn each_speaker_gets_independent_repeat_history() {
        let mut session = VoiceSession::new();
        session.set_vocab(vec!["compute_total".to_string(), "render_frame".to_string()]);
        session.handle("alice", "go to compute total");
        session.handle("bob", "go to render frame");

        // Bob saying "again" replays BOB's last command, not Alice's —
        // the one thing that is wrong by default in a shared session if
        // history isn't kept per speaker.
        assert_eq!(session.handle("bob", "again"), VoiceCommand::GoToSymbol("render_frame".to_string()));
        assert_eq!(session.handle("alice", "repeat that"), VoiceCommand::GoToSymbol("compute_total".to_string()));
    }

    #[test]
    fn repeat_with_no_prior_history_for_that_speaker_is_unknown() {
        let mut session = VoiceSession::new();
        assert_eq!(session.handle("carol", "again"), VoiceCommand::Unknown("again".to_string()));
    }

    #[test]
    fn history_is_bounded_per_speaker() {
        let mut session = VoiceSession::new();
        for i in 0..10 {
            session.handle("dana", &format!("go to line {}", i + 1));
        }
        // Only the MOST RECENT command needs to survive — "again" always
        // means "the last thing", never something further back — so this
        // just confirms the bound doesn't silently drop the newest entry
        // instead of the oldest.
        assert_eq!(session.handle("dana", "again"), VoiceCommand::GoToLine(10));
    }

    #[test]
    fn setting_vocab_takes_effect_on_the_next_command() {
        let mut session = VoiceSession::new();
        assert_eq!(session.handle("eve", "go to compute total"),
            VoiceCommand::Unknown("go to compute total".to_string()));
        session.set_vocab(vec!["compute_total".to_string()]);
        assert_eq!(session.handle("eve", "go to compute total"),
            VoiceCommand::GoToSymbol("compute_total".to_string()));
    }
}
