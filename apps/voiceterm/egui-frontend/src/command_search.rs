//! command_search — the tldr-backed command classifier from `command_index.rs`.
//!
//! When spoken text matches none of the structured intents, it is scored
//! against an index of tldr pages and the best match is offered as a candidate.
//! The indexing and the disk cache belong to the shell; the **scoring** is the
//! judgement, and it is what decides whether "delete the old logs" surfaces
//! `rm` or something alarming.
//!
//! ## Why tldr rather than man pages, in the original's own accounting
//!
//! tldr entries are *"short (~5-10 hand-picked usage examples per command),
//! written in natural language ... and already ranked by usefulness. Man pages
//! are verbose, jargon-heavy, and structured for reference rather than search.
//! Indexing tldr is ~10MB of text vs ~100MB for man pages."*
//!
//! That is three arguments and a measurement, and the last one is why it is not
//! close: a voice classifier that costs 100MB of index is one nobody enables.
//!
//! ## Synonyms exist because people speak verbs, not commands
//!
//! Nobody says "grep the logs" out loud when they mean "search the logs". The
//! table maps spoken verbs onto command vocabulary — and synonym hits are
//! **discounted against direct ones**, because "search" meaning `grep` is an
//! inference while "grep" meaning `grep` is not.
//!
//! ## The example contribution is capped, and the cap is the interesting part
//!
//! Each matching example adds 0.4, but the total from examples is capped at
//! 0.6. Without the cap a command with ten examples outranks a command that is
//! simply the right answer — tldr pages vary in length, so uncapped example
//! scoring measures how well-documented a command is rather than how well it
//! matches.

/// Score for the query's first word being exactly the command name.
pub const W_NAME_EXACT: f32 = 0.7;
/// ...or a prefix of it, for words of three characters or more.
pub const W_NAME_PREFIX: f32 = 0.4;
/// Per token found in the one-line summary.
pub const W_SUMMARY: f32 = 0.25;
/// Per token found in an example.
pub const W_EXAMPLE: f32 = 0.4;
/// Ceiling on the total example contribution — see the module docs.
pub const EXAMPLE_CAP: f32 = 0.6;
/// A synonym hit is worth this fraction of a direct one.
pub const SYNONYM_WEIGHT: f32 = 0.6;
/// Shortest prefix that may match a command name.
pub const MIN_PREFIX: usize = 3;

/// Spoken verbs and the command vocabulary they stand in for.
pub const VERB_SYNONYMS: &[(&str, &[&str])] = &[
    ("show", &["display", "list", "print", "view", "see"]),
    ("display", &["show", "list", "print", "view"]),
    ("list", &["show", "display", "print", "ls"]),
    ("find", &["search", "locate", "grep", "look"]),
    ("search", &["find", "grep", "locate", "look"]),
    ("delete", &["remove", "rm", "destroy", "drop"]),
    ("remove", &["delete", "rm", "drop"]),
    ("kill", &["stop", "terminate", "end"]),
    ("stop", &["kill", "terminate", "end", "halt"]),
    ("start", &["run", "launch", "begin", "open"]),
    ("run", &["start", "execute", "launch"]),
    ("create", &["make", "new", "add"]),
    ("make", &["create", "new", "build"]),
];

/// Words carrying no signal, dropped before scoring.
pub const STOPWORDS: &[&str] = &[
    "the", "a", "an", "of", "to", "in", "on", "for", "with", "and", "or",
    "my", "me", "i", "it", "that", "this", "is", "are", "be", "please",
];

pub fn is_stopword(s: &str) -> bool { STOPWORDS.iter().any(|w| *w == s) }

/// Split an utterance into scoring tokens.
///
/// Punctuation splits rather than being stripped: a transcriber emits
/// "logs," and "files." freely, and a token carrying a comma matches nothing.
pub fn tokenize(s: &str) -> Vec<String> {
    s.to_lowercase()
        .split(|c: char| !c.is_ascii_alphanumeric() && c != '-')
        .filter(|t| !t.is_empty() && !is_stopword(t))
        .map(|t| t.to_string())
        .collect()
}

/// A token and everything it might stand for, with weights.
///
/// The token itself weighs 1.0; its synonyms less, because "search" meaning
/// `grep` is an inference and "grep" meaning `grep` is not.
pub fn expand(token: &str) -> Vec<(String, f32)> {
    let mut out = vec![(token.to_string(), 1.0)];
    if let Some((_, syns)) = VERB_SYNONYMS.iter().find(|(k, _)| *k == token) {
        for s in *syns {
            if *s != token { out.push((s.to_string(), SYNONYM_WEIGHT)); }
        }
    }
    out
}

/// One indexed command.
#[derive(Clone, Debug, PartialEq)]
pub struct CommandDoc {
    pub name: String,
    pub summary: String,
    pub examples: Vec<String>,
    /// Whether the command is actually installed.
    pub on_path: bool,
}

/// A scored candidate, with the reasons that produced it.
///
/// The reasons are not decoration: this surfaces a command the user did not
/// name, so "why is it suggesting this" has to be answerable.
#[derive(Clone, Debug, PartialEq)]
pub struct CommandMatch {
    pub name: String,
    pub score: f32,
    pub reasons: Vec<String>,
}

/// Score one document against an utterance.
pub fn score_doc(query: &str, doc: &CommandDoc) -> CommandMatch {
    let tokens = tokenize(query);
    let mut score = 0.0f32;
    let mut reasons = Vec::new();

    if let Some(first) = tokens.first() {
        if *first == doc.name {
            score += W_NAME_EXACT;
            reasons.push(format!("'{first}' is the command"));
        } else if doc.name.starts_with(first.as_str()) && first.len() >= MIN_PREFIX {
            score += W_NAME_PREFIX;
            reasons.push(format!("'{first}' prefix-matches '{}'", doc.name));
        }
    }

    let summary = doc.summary.to_lowercase();
    for t in &tokens {
        for (word, w) in expand(t) {
            if summary.contains(&word) {
                score += W_SUMMARY * w;
                reasons.push(if w < 1.0 {
                    format!("'{t}' (as '{word}') is in the description")
                } else {
                    format!("'{word}' is in the description")
                });
                break;
            }
        }
    }

    // Examples: capped, because tldr pages vary in length and uncapped
    // scoring would measure documentation rather than fit.
    let mut ex_score = 0.0f32;
    for t in &tokens {
        for (word, w) in expand(t) {
            if doc.examples.iter().any(|e| e.to_lowercase().contains(&word)) {
                ex_score += W_EXAMPLE * w;
                break;
            }
        }
    }
    if ex_score > 0.0 {
        score += ex_score.min(EXAMPLE_CAP);
        reasons.push("matches its usage examples".to_string());
    }

    CommandMatch { name: doc.name.clone(), score, reasons }
}

/// Rank an index against an utterance.
///
/// Installed commands break ties. Suggesting something the user does not have
/// is a suggestion that fails when taken, so between two equal answers the one
/// that will actually run wins.
pub fn search(query: &str, docs: &[CommandDoc], top_n: usize) -> Vec<CommandMatch> {
    let mut scored: Vec<(CommandMatch, bool)> = docs
        .iter()
        .map(|d| (score_doc(query, d), d.on_path))
        .filter(|(m, _)| m.score > 0.0)
        .collect();
    scored.sort_by(|a, b| {
        b.0.score.partial_cmp(&a.0.score).unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| b.1.cmp(&a.1))
            .then_with(|| a.0.name.cmp(&b.0.name))
    });
    scored.into_iter().take(top_n).map(|(m, _)| m).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc(name: &str, summary: &str, examples: &[&str], on_path: bool) -> CommandDoc {
        CommandDoc {
            name: name.into(), summary: summary.into(),
            examples: examples.iter().map(|s| s.to_string()).collect(), on_path,
        }
    }

    fn corpus() -> Vec<CommandDoc> {
        vec![
            doc("rm", "Remove files or directories", &["rm -r directory", "rm -f file"], true),
            doc("grep", "Find patterns in files", &["grep pattern file"], true),
            doc("du", "Display disk usage", &["du -sh directory", "du -a"], true),
            doc("ripgrep", "Search recursively for a pattern", &["rg pattern"], false),
        ]
    }

    #[test]
    fn naming_the_command_outweighs_describing_it() {
        // "grep the logs" should land on grep, not on something whose summary
        // merely mentions patterns.
        let hits = search("grep the logs", &corpus(), 3);
        assert_eq!(hits[0].name, "grep");
        assert!(hits[0].score >= W_NAME_EXACT);
    }

    #[test]
    fn a_spoken_verb_reaches_the_command_through_a_synonym() {
        // Nobody says "grep the logs" out loud when they mean "search the
        // logs" — the synonym table is what makes the classifier usable by
        // voice at all.
        let hits = search("search for a pattern", &corpus(), 3);
        assert!(hits.iter().any(|h| h.name == "grep" || h.name == "ripgrep"), "{hits:?}");
    }

    #[test]
    fn a_synonym_hit_scores_less_than_a_direct_one() {
        // "search" meaning grep is an inference; "grep" meaning grep is not.
        let d = doc("grep", "Find patterns in files", &[], true);
        let direct = score_doc("find patterns", &d).score;
        let inferred = score_doc("search patterns", &d).score;
        assert!(direct > inferred, "direct {direct} vs inferred {inferred}");
    }

    #[test]
    fn stopwords_carry_no_signal() {
        assert_eq!(tokenize("show me the files"), vec!["show", "files"]);
        assert!(tokenize("the a of to").is_empty());
    }

    #[test]
    fn punctuation_splits_rather_than_sticking_to_a_token() {
        // A transcriber emits "logs," and "files." freely, and a token
        // carrying a comma matches nothing.
        assert_eq!(tokenize("delete the logs, please."), vec!["delete", "logs"]);
        assert_eq!(tokenize("run make-thing"), vec!["run", "make-thing"],
                   "a hyphen is part of a command name, not a separator");
    }

    #[test]
    fn a_short_word_does_not_prefix_match() {
        // Two letters would make "du" prefix-match half the index and "rm"
        // match "rmdir", "rmmod" and anything else beginning rm.
        let d = doc("ripgrep", "Search recursively", &[], true);
        assert_eq!(score_doc("ri", &d).score, 0.0);
        let m = score_doc("rip", &d);
        assert!(m.score >= W_NAME_PREFIX, "three characters is enough: {m:?}");
    }

    #[test]
    fn the_example_contribution_is_capped() {
        // Without the cap, a command with ten examples outranks one that is
        // simply the right answer — uncapped example scoring measures how
        // well-documented a command is, not how well it matches.
        let many = doc("x", "", &["alpha", "beta", "gamma", "delta", "epsilon"], true);
        let m = score_doc("alpha beta gamma delta epsilon", &many);
        assert!(m.score <= EXAMPLE_CAP + 1e-5, "examples uncapped: {}", m.score);
    }

    #[test]
    fn an_installed_command_breaks_a_tie() {
        // Suggesting something the user does not have is a suggestion that
        // fails when taken.
        let both = vec![
            doc("aaa_missing", "Search recursively for a pattern", &[], false),
            doc("zzz_present", "Search recursively for a pattern", &[], true),
        ];
        let hits = search("search recursively pattern", &both, 2);
        assert_eq!(hits[0].name, "zzz_present", "{hits:?}");
    }

    #[test]
    fn ranking_is_stable_for_equal_candidates() {
        // A picker that reshuffles equal rows between calls is one you choose
        // the wrong entry from.
        let c = corpus();
        let a: Vec<String> = search("display usage", &c, 4).into_iter().map(|m| m.name).collect();
        let b: Vec<String> = search("display usage", &c, 4).into_iter().map(|m| m.name).collect();
        assert_eq!(a, b);
    }

    #[test]
    fn a_zero_score_is_not_offered() {
        // Surfacing a top match that matched nothing is worse than surfacing
        // nothing: it reads as a confident answer.
        assert!(search("xyzzy plugh", &corpus(), 5).is_empty());
        assert!(search("", &corpus(), 5).is_empty());
    }

    #[test]
    fn every_match_carries_its_reasons() {
        // This surfaces a command the user did not name, so "why is it
        // suggesting this" has to be answerable.
        for m in search("delete files", &corpus(), 3) {
            assert!(!m.reasons.is_empty(), "{} suggested with no reason", m.name);
        }
    }

    #[test]
    fn the_synonym_table_is_symmetric_where_it_claims_to_be() {
        // show/display, find/search, delete/remove, kill/stop, start/run and
        // create/make are listed as pairs; an asymmetry means one phrasing
        // works and its mirror silently does not.
        for (a, b) in [("show", "display"), ("find", "search"), ("delete", "remove"),
                       ("kill", "stop"), ("start", "run"), ("create", "make")] {
            let fwd = VERB_SYNONYMS.iter().find(|(k, _)| *k == a).map(|(_, v)| v.contains(&b));
            let rev = VERB_SYNONYMS.iter().find(|(k, _)| *k == b).map(|(_, v)| v.contains(&a));
            assert_eq!(fwd, Some(true), "{a} does not reach {b}");
            assert_eq!(rev, Some(true), "{b} does not reach {a}");
        }
    }

    #[test]
    fn a_token_does_not_list_itself_as_its_own_synonym() {
        // It would double-count the direct hit and make one verb outweigh a
        // command name.
        for (k, _) in VERB_SYNONYMS {
            let exp = expand(k);
            assert_eq!(exp.iter().filter(|(w, _)| w == k).count(), 1, "{k} expands to itself twice");
        }
    }
}
