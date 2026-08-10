//! mail_selector — the selector grammar from `mail_intent.rs`, ported.
//!
//! Turns *"my last couple emails from bob"* into a count and a sender. Every
//! mail intent except `OpenDrafts` carries one of these, so it is the piece the
//! rest of the grammar is built on.
//!
//! ## A separate grammar on purpose
//!
//! The original explains the seam: *"keeping the mail grammar in the mail
//! module avoids bloating the global intent matcher and lets the two evolve
//! independently"*, with `voice_intent::parse` delegating in one line. That is
//! the right shape — mail vocabulary is large and specialised, and folding it
//! into the general matcher makes every unrelated utterance pay for it.
//!
//! It is also *"deliberately pure and dependency-free so it unit-tests without
//! a session or network"*. Resolving *"from bob"* into message ids needs a live
//! account; deciding that the phrase MEANS "from bob, most recent two" does
//! not, and separating them is what makes the grammar testable at all.
//!
//! ## "A couple" is two
//!
//! The C++ note is explicit, and it is the kind of thing a reimplementation
//! gets wrong by treating unrecognised words as one. Someone saying *"my last
//! couple emails"* and getting a single message will assume the feature is
//! broken rather than that the word was unknown.

/// How many messages a bare "last email" opens.
pub const DEFAULT_COUNT: usize = 1;
/// Ceiling on "all", so a broad phrase cannot open a hundred panes.
pub const MAX_COUNT: usize = 20;

/// A relative date window.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum When { Today, Yesterday, ThisWeek, LastWeek }

/// What a phrase selects.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Selector {
    pub count: usize,
    pub from: Option<String>,
    pub about: Option<String>,
    pub when: Option<When>,
    /// True when the phrase said "all" rather than a specific small number.
    pub all: bool,
}

/// Number words the grammar accepts.
///
/// A short list rather than a full spell-out parser: these are the numbers
/// people actually say about email, and a longer table would add ambiguity
/// ("second" is an ordinal AND a unit of time) for words nobody uses here.
pub fn word_to_num(w: &str) -> Option<usize> {
    Some(match w {
        // "A couple" is TWO. Falling back to one would give a user asking for
        // a couple of emails exactly one, which reads as the feature being
        // broken rather than the word being unknown.
        "couple" | "two" => 2,
        "one" | "a" | "an" => 1,
        "three" => 3, "four" => 4, "five" => 5, "six" => 6, "seven" => 7,
        "eight" => 8, "nine" => 9, "ten" => 10,
        "fourteen" => 14, "twenty" => 20, "thirty" => 30,
        // "a few" is deliberately three, which is what it means in English and
        // what a user expects to see open.
        "few" => 3,
        _ => return w.parse::<usize>().ok().filter(|n| *n > 0),
    })
}

/// Words that end a captured phrase.
///
/// Without these, "emails from bob about invoices" captures a sender of
/// "bob about invoices" and matches nobody.
pub const STOP_WORDS: &[&str] = &[
    "about", "regarding", "re", "from", "to", "with", "containing",
    "today", "yesterday", "this", "last", "week",
];

/// Capture the words after `marker`, stopping at a stop word.
pub fn extract_after(text: &str, marker: &str) -> Option<String> {
    let words: Vec<&str> = text.split_whitespace().collect();
    let i = words.iter().position(|w| *w == marker)?;
    let captured: Vec<&str> = words[i + 1..]
        .iter()
        .take_while(|w| !STOP_WORDS.contains(&w.to_lowercase().as_str()))
        .copied()
        .collect();
    if captured.is_empty() { None } else { Some(captured.join(" ")) }
}

/// The sender, from "from X" or "sent by X".
pub fn extract_sender(text: &str) -> Option<String> {
    extract_after(text, "from").or_else(|| extract_after(text, "by"))
}

/// The date window, if the phrase names one.
///
/// Checked longest-first: "last week" must not match "last" and become a
/// count word, which would turn a date phrase into a quantity.
pub fn extract_when(text: &str) -> Option<When> {
    let t = text.to_lowercase();
    if t.contains("last week") { return Some(When::LastWeek); }
    if t.contains("this week") { return Some(When::ThisWeek); }
    if t.contains("yesterday") { return Some(When::Yesterday); }
    if t.contains("today") { return Some(When::Today); }
    None
}

/// Parse a selector out of a phrase.
pub fn parse_selector(text: &str) -> Selector {
    let t = text.to_lowercase();
    let words: Vec<&str> = t.split_whitespace().collect();

    let all = words.iter().any(|w| *w == "all" || *w == "every");

    // A quantity is only a quantity when it sits before a noun the grammar
    // knows. Scanning for any number would read the "2" in "from bob2" as a
    // count.
    let nouns = ["email", "emails", "message", "messages", "mail", "ones"];
    let mut count = None;
    for (i, w) in words.iter().enumerate() {
        if nouns.contains(w) && i > 0 {
            if let Some(n) = word_to_num(words[i - 1]) { count = Some(n); break; }
            // "last couple emails" — the quantity may sit two back.
            if i > 1 {
                if let Some(n) = word_to_num(words[i - 2]) { count = Some(n); break; }
            }
        }
    }

    let count = if all {
        MAX_COUNT
    } else {
        count.unwrap_or(DEFAULT_COUNT).clamp(1, MAX_COUNT)
    };

    Selector {
        count,
        from: extract_sender(&t),
        about: extract_after(&t, "about").or_else(|| extract_after(&t, "regarding")),
        when: extract_when(&t),
        all,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_hero_phrase_parses() {
        // "open my last couple emails from bob" — the example the module
        // header leads with.
        let s = parse_selector("open my last couple emails from bob");
        assert_eq!(s.count, 2);
        assert_eq!(s.from.as_deref(), Some("bob"));
        assert!(!s.all);
    }

    #[test]
    fn a_couple_is_two_not_one() {
        // The note is explicit, and it is what a reimplementation gets wrong by
        // treating unknown words as one. A user asking for a couple and getting
        // one assumes the feature is broken rather than the word unknown.
        assert_eq!(word_to_num("couple"), Some(2));
        assert_eq!(parse_selector("my last couple emails").count, 2);
    }

    #[test]
    fn a_few_is_three() {
        // What it means in English, and what a user expects to see open.
        assert_eq!(word_to_num("few"), Some(3));
        assert_eq!(parse_selector("show me a few messages").count, 3);
    }

    #[test]
    fn a_bare_request_opens_one() {
        assert_eq!(parse_selector("open my last email from alice").count, DEFAULT_COUNT);
    }

    #[test]
    fn digits_work_as_well_as_words() {
        // People type as often as they speak here.
        assert_eq!(parse_selector("open 5 emails from bob").count, 5);
        assert_eq!(word_to_num("12"), Some(12));
    }

    #[test]
    fn zero_is_not_a_count() {
        // "0 emails" is a typo or a misheard word, and opening nothing while
        // reporting success is worse than opening one.
        assert_eq!(word_to_num("0"), None);
        assert_eq!(parse_selector("open 0 emails").count, DEFAULT_COUNT);
    }

    #[test]
    fn all_is_capped_so_a_broad_phrase_cannot_open_a_hundred_panes() {
        let s = parse_selector("open all emails from bob");
        assert!(s.all);
        assert_eq!(s.count, MAX_COUNT);
    }

    #[test]
    fn an_absurd_number_is_clamped_rather_than_honoured() {
        assert_eq!(parse_selector("open 5000 emails").count, MAX_COUNT);
    }

    #[test]
    fn a_sender_stops_at_the_next_clause() {
        // Without stop words this captures "bob about invoices" and matches
        // nobody.
        let s = parse_selector("emails from bob about invoices");
        assert_eq!(s.from.as_deref(), Some("bob"));
        assert_eq!(s.about.as_deref(), Some("invoices"));
    }

    #[test]
    fn a_sender_stops_before_a_date_phrase() {
        // "from bob yesterday" is a sender and a window, not a sender called
        // "bob yesterday".
        let s = parse_selector("emails from bob yesterday");
        assert_eq!(s.from.as_deref(), Some("bob"));
        assert_eq!(s.when, Some(When::Yesterday));
    }

    #[test]
    fn last_week_is_a_window_not_a_quantity() {
        // Checked longest-first so "last week" does not match "last" and get
        // read as a count word.
        let s = parse_selector("emails from last week");
        assert_eq!(s.when, Some(When::LastWeek));
        assert_eq!(s.count, DEFAULT_COUNT);
    }

    #[test]
    fn a_number_not_next_to_a_noun_is_not_a_count() {
        // Scanning for any digit would read the 2 in a name or an address as a
        // quantity.
        let s = parse_selector("open my last email from bob2");
        assert_eq!(s.count, DEFAULT_COUNT);
        assert_eq!(s.from.as_deref(), Some("bob2"));
    }

    #[test]
    fn a_multi_word_sender_survives() {
        // "from the build server" is one sender.
        assert_eq!(parse_selector("emails from build server").from.as_deref(),
                   Some("build server"));
    }

    #[test]
    fn a_phrase_with_no_sender_leaves_it_unset() {
        // None rather than an empty string, so a caller cannot search for a
        // sender named "".
        let s = parse_selector("show me unread mail");
        assert_eq!(s.from, None);
        assert_eq!(s.about, None);
    }

    #[test]
    fn a_trailing_marker_with_nothing_after_it_captures_nothing() {
        // A transcription cut short mid-phrase — "emails from" — must not
        // produce an empty sender that matches every message.
        assert_eq!(extract_after("emails from", "from"), None);
        assert_eq!(parse_selector("emails from").from, None);
    }

    #[test]
    fn case_does_not_matter() {
        let s = parse_selector("Open My Last COUPLE Emails From Bob");
        assert_eq!(s.count, 2);
        assert_eq!(s.from.as_deref(), Some("bob"));
    }
}
