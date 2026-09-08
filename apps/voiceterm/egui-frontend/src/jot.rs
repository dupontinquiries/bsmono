//! jot — dictation that goes into the note instead of the command parser.
//!
//! `TODO_notes_clip.md` step 2: *"a toggled 'jot mode': route transcription straight into the
//! active Note as `Block::Prose` (append), instead of through the command intent parser. End
//! on a wake-word / 'done jotting'. The two-tier whisper path already produces the text; this
//! just changes the SINK."*
//!
//! So the whole feature is a decision about where a transcript goes, and that decision is
//! what lives here. Nothing in this module speaks, listens, or writes a file.
//!
//! ## Why the exit phrase needs care
//!
//! A person finishing a thought says "…and that fixed it, done jotting" in one breath. If the
//! exit phrase only counts as a whole utterance, that transcript is filed as prose ending in
//! "done jotting" and the mode stays on — so the next sentence, which was meant as a command,
//! is filed too. Handling it only at the end of an utterance is the difference between a mode
//! you can leave and one you have to remember to leave twice.
//!
//! So: the phrase is looked for at the END of the transcript, the text before it is kept, and
//! the mode ends. An utterance that is ONLY the phrase keeps nothing.
//!
//! ## Why not anywhere in the middle
//!
//! Because "I was done jotting notes about the parser" is prose about jotting. A phrase that
//! ends the mode from the middle of a sentence would truncate the note at a word the speaker
//! used as content. The end is where a person puts a command they are addressing to a
//! machine.

/// The phrases that end jot mode.
///
/// Several, because a speaker does not remember one exact form and a mode that will not exit
/// on "stop jotting" is a mode you have to kill the app to leave. Matched case- and
/// punctuation-insensitively.
pub const EXIT_PHRASES: &[&str] =
    &["done jotting", "stop jotting", "end jotting", "done jotting notes", "end note"];

/// What to do with one transcript while jot mode is on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Jotted {
    /// Append this prose to the note; stay in jot mode.
    Append(String),
    /// Append this prose (if any), then leave jot mode.
    AppendAndExit(Option<String>),
}

/// Normalise for matching: lower case, and trailing punctuation off.
///
/// Whisper punctuates. "Done jotting." and "done jotting" are the same instruction, and a
/// mode that stayed on because the transcriber added a full stop would be maddening in a way
/// nobody would think to report.
fn normalise(s: &str) -> String {
    s.trim()
        .trim_end_matches(|c: char| c.is_ascii_punctuation())
        .trim()
        .to_lowercase()
}

/// Route one transcript.
pub fn route(transcript: &str) -> Jotted {
    let norm = normalise(transcript);
    for p in EXIT_PHRASES {
        if norm == *p {
            // Nothing but the phrase: end the mode, file nothing.
            return Jotted::AppendAndExit(None);
        }
        // At the END, with a word boundary before it: "…and that fixed it, done jotting".
        if let Some(stem) = norm.strip_suffix(p) {
            let stem_trimmed = stem.trim_end_matches(|c: char| c.is_whitespace() || c == ',');
            if stem_trimmed.len() < stem.len() || stem.is_empty() {
                let kept = keep_prefix(transcript, stem_trimmed.len());
                return Jotted::AppendAndExit((!kept.trim().is_empty()).then_some(kept));
            }
        }
    }
    Jotted::Append(transcript.trim().to_string())
}

/// The first `n` normalised characters, taken from the ORIGINAL text.
///
/// The note keeps what the speaker said, capitals and punctuation included — `normalise` is
/// for matching, and filing the lower-cased form would turn every note into shouty-inverse
/// prose.
fn keep_prefix(original: &str, n: usize) -> String {
    // `normalise` only lower-cases and trims, so character positions line up with the
    // original's from the front. Taken by chars, not bytes: a note with an accent in it must
    // not be cut mid-codepoint.
    original.trim().chars().take(n).collect::<String>().trim_end_matches([',', ' ']).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ordinary_speech_is_filed_as_prose() {
        assert_eq!(route("the linker was the problem"), Jotted::Append("the linker was the problem".into()));
        // Trimmed, but otherwise untouched: capitals and punctuation are what the speaker
        // said, and filing a lower-cased form would turn every note into odd prose.
        assert_eq!(route("  Rebuilt with LTO off.  "), Jotted::Append("Rebuilt with LTO off.".into()));
    }

    /// The one that matters: a person finishing a thought says it in one breath. If the exit
    /// only counted as a whole utterance, this would be filed as prose ending in "done
    /// jotting" and the mode would stay on — so the NEXT sentence, meant as a command, gets
    /// filed too.
    #[test]
    fn an_exit_at_the_end_of_a_sentence_keeps_the_sentence_and_leaves() {
        assert_eq!(
            route("and that fixed it, done jotting"),
            Jotted::AppendAndExit(Some("and that fixed it".into()))
        );
        // Whisper punctuates. A mode that stayed on because the transcriber added a full stop
        // would be maddening in a way nobody would think to report.
        assert_eq!(route("Done jotting."), Jotted::AppendAndExit(None));
        assert_eq!(route("done jotting"), Jotted::AppendAndExit(None));
    }

    /// A speaker does not remember one exact form, and a mode that will not exit on "stop
    /// jotting" is one you have to kill the app to leave.
    #[test]
    fn several_phrasings_all_work() {
        for p in EXIT_PHRASES {
            assert!(
                matches!(route(p), Jotted::AppendAndExit(None)),
                "{p:?} did not end jot mode"
            );
            assert!(
                matches!(route(&format!("something, {p}")), Jotted::AppendAndExit(Some(_))),
                "{p:?} did not end it at the end of a sentence"
            );
        }
    }

    /// "I was done jotting notes about the parser" is PROSE about jotting. Ending the mode
    /// from mid-sentence would truncate the note at a word the speaker used as content.
    #[test]
    fn the_phrase_in_the_middle_is_just_words() {
        let s = "I was done jotting notes about the parser earlier";
        assert_eq!(route(s), Jotted::Append(s.to_string()));
        // And a phrase at the START is not an exit either — it is how someone describes what
        // they are about to do.
        let s = "done jotting is what I said last time";
        assert_eq!(route(s), Jotted::Append(s.to_string()));
    }

    /// The kept text comes from the ORIGINAL, so accents survive and nothing is cut
    /// mid-codepoint.
    #[test]
    fn the_kept_prose_is_what_was_actually_said() {
        assert_eq!(
            route("Café notes are Fine, done jotting"),
            Jotted::AppendAndExit(Some("Café notes are Fine".into()))
        );
        // An utterance that is only whitespace and the phrase files nothing rather than an
        // empty block — a note full of blank paragraphs is worse than one without them.
        assert_eq!(route("   done jotting  "), Jotted::AppendAndExit(None));
    }
}
