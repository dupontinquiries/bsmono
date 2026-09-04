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
}
