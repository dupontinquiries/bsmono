//! voice_prefix — the politeness normaliser from `voice_intent.rs`.
//!
//! Speech is not typed input. *"Could you please just go to hacker news"* and
//! *"open hacker news"* are the same command, and every intent rule downstream
//! is written against the second form — so stripping the first is what makes
//! the rest of the parser tractable rather than a combinatorial mess.
//!
//! ## Longest first, and in a loop
//!
//! The C++ note: *"Order matters: longer forms first so we don't half-match. We
//! loop because users can stack them ('could you please just go to ...')."*
//!
//! Both halves are load-bearing. Matching `"could you "` before
//! `"could you please "` leaves a stray `"please "` that no rule expects; and
//! stripping once leaves the second prefix of a stacked phrase in place. People
//! genuinely say *"okay so could you please just..."*.
//!
//! ## Prose rejection is separate, and stricter than it looks
//!
//! A transcript is often narration rather than a command — someone talking near
//! an open mic. The original rejects an utterance whose **first word** is a
//! prose marker, and then rejects it again if *any* token is one. That second
//! check is aggressive on purpose: a false command is worse than a missed one,
//! because it navigates away from what you were doing.

/// Leading phrases stripped before intent matching.
///
/// Ordered longest-first within each family, which the loop depends on.
pub const PREFIXES: &[&str] = &[
    // Politeness questions.
    "could you please ", "would you please ", "can you please ", "will you please ",
    "could you kindly ", "would you kindly ",
    "could you ", "would you ", "can you ", "will you ",
    // First-person wishes.
    "i would like to ", "i want to ", "i need to ", "i would love to ",
    "i am going to ",
    "would it be possible to ", "is it possible to ",
    "do you think you could ", "do you mind ",
    "let me ", "lets ", "let's ",
    // Bare politeness and filler.
    "please ", "kindly ",
    "okay ", "ok ", "alright ", "all right ", "right ",
    "yeah ", "yep ", "yes ", "so ", "just ", "now ",
];

/// Words that mark narration rather than a command.
pub const PROSE: &[&str] = &[
    "the", "a", "an", "on", "with", "for", "of", "more", "like", "just",
    "effectively", "basically", "probably", "actually", "really", "host",
    "thing", "point", "bit", "you", "oh", "why", "what", "here", "there",
    "comment", "extra",
];

/// Strip leading politeness, repeatedly.
///
/// Lowercases first, because the prefix table is lowercase and a transcriber
/// that capitalises sentences would otherwise defeat every entry.
pub fn strip_politeness(input: &str) -> String {
    let mut s = input.trim().to_lowercase();
    // Bounded rather than `loop`: a pathological transcript of nothing but
    // "please please please" should terminate, and no real utterance stacks
    // more than a handful.
    for _ in 0..8 {
        let before = s.len();
        for p in PREFIXES {
            if let Some(rest) = s.strip_prefix(p) {
                s = rest.trim_start().to_string();
                break;
            }
            // A prefix as the WHOLE utterance: "please" with nothing after it
            // never matches "please " and survives to be parsed as a command.
            // Found by a test — the trailing space that stops "so" eating
            // "software" also stops a bare politeness word being recognised.
            if s == p.trim_end() {
                s.clear();
                break;
            }
        }
        if s.len() == before { break; }
    }
    s
}

/// Does this look like a command rather than narration?
///
/// Both checks from the original: the first word must not be a prose marker,
/// and no token may be one. Deliberately strict — a false command navigates
/// away from what the user was doing, which costs far more than a missed one
/// they can simply repeat.
pub fn looks_like_command(text: &str) -> bool {
    let words: Vec<&str> = text.split_whitespace().collect();
    if words.is_empty() { return false; }
    let norm = |w: &str| w.trim_end_matches([',', '.', '?', '!']).to_lowercase();
    if PROSE.contains(&norm(words[0]).as_str()) { return false; }
    !words.iter().any(|w| PROSE.contains(&norm(w).as_str()))
}

/// Normalise then classify.
///
/// Returns `None` when the utterance is narration, so a caller cannot act on
/// something that was never addressed to it.
pub fn normalise(input: &str) -> Option<String> {
    let s = strip_politeness(input);
    if s.is_empty() || !looks_like_command(&s) { return None; }
    Some(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_polite_request_reduces_to_the_bare_command() {
        // Every intent rule downstream is written against the bare form.
        assert_eq!(strip_politeness("Could you please open hacker news"),
                   "open hacker news");
        assert_eq!(strip_politeness("I would like to search rust async"),
                   "search rust async");
    }

    #[test]
    fn the_longest_prefix_wins_so_nothing_is_half_matched() {
        // Matching "could you " first would leave a stray "please " that no
        // rule expects.
        assert_eq!(strip_politeness("could you please reload"), "reload");
        assert!(!strip_politeness("could you please reload").starts_with("please"));
    }

    #[test]
    fn stacked_prefixes_are_all_stripped() {
        // "We loop because users can stack them" — people genuinely say this.
        assert_eq!(strip_politeness("okay so could you please just go back"), "go back");
        assert_eq!(strip_politeness("yeah alright please reload"), "reload");
    }

    #[test]
    fn stripping_terminates_on_a_pathological_transcript() {
        // A stuck transcriber emitting one word forever must not hang the
        // parser — which runs on the audio thread's output.
        let spam = "please ".repeat(500);
        let out = strip_politeness(&spam);
        assert!(out.len() <= spam.len());
    }

    #[test]
    fn capitalised_speech_is_still_matched() {
        // A transcriber that capitalises sentences would otherwise defeat every
        // lowercase entry in the table.
        assert_eq!(strip_politeness("Please Reload"), "reload");
    }

    #[test]
    fn narration_is_not_a_command() {
        // Someone talking near an open mic. A false command navigates away from
        // what they were doing, which costs far more than a missed one.
        assert!(!looks_like_command("the thing is you probably want more"));
        assert!(!looks_like_command("basically what I said"));
        assert!(!looks_like_command("a comment on the host"));
    }

    #[test]
    fn a_bare_command_passes() {
        assert!(looks_like_command("open hacker news"));
        assert!(looks_like_command("reload"));
        assert!(looks_like_command("scroll down"));
    }

    #[test]
    fn a_prose_word_anywhere_rejects_the_utterance() {
        // The second check, aggressive on purpose. "search for rust" is
        // rejected because "for" is a marker — a missed command the user
        // repeats, rather than a wrong one that acts.
        assert!(!looks_like_command("search for rust"));
        assert!(!looks_like_command("open the terminal"));
    }

    #[test]
    fn punctuation_does_not_hide_a_prose_word() {
        // A transcriber emits "the," and "you." freely; matching on the raw
        // token would let narration through the filter.
        assert!(!looks_like_command("you, open the terminal"));
        assert!(!looks_like_command("there. reload"));
    }

    #[test]
    fn an_empty_or_whitespace_utterance_is_not_a_command() {
        // Silence detected as speech is routine.
        assert!(!looks_like_command(""));
        assert!(!looks_like_command("   "));
        assert_eq!(normalise("   "), None);
    }

    #[test]
    fn an_utterance_that_is_only_politeness_yields_nothing() {
        // "please" alone is not a command, and returning an empty string would
        // have the caller match it against every rule.
        assert_eq!(normalise("please"), None);
        assert_eq!(normalise("okay so yeah"), None);
    }

    #[test]
    fn normalise_returns_none_for_narration_so_a_caller_cannot_act_on_it() {
        assert_eq!(normalise("could you please tell me about the thing"), None);
        assert_eq!(normalise("could you please reload"), Some("reload".into()));
    }

    #[test]
    fn every_prefix_ends_with_a_space() {
        // Without one, "so" would strip the first two letters of "software" and
        // hand the parser "ftware".
        for p in PREFIXES {
            assert!(p.ends_with(' '), "{p:?} would match mid-word");
            assert_eq!(*p, p.to_lowercase(), "{p:?} must be lowercase to match");
        }
    }

    #[test]
    fn a_word_starting_with_a_prefix_is_not_truncated() {
        // The property that space guards: "software" must survive "so ".
        assert_eq!(strip_politeness("software update"), "software update");
        assert_eq!(strip_politeness("justify the text"), "justify the text");
    }
}
