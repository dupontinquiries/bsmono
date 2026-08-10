//! summary_scope — the scope setting and chunk gating from `web_summary.rs`.
//!
//! ## One atomic, two consumers, no third state
//!
//! Off / Terminal / Web / Full is *"a single segmented control deciding which
//! summary paths run"*, and the original is explicit about why it is one value
//! rather than two booleans: the terminal path asks `terminal_enabled()`, the
//! web path asks `web_enabled()`, and *"both derive from the single SCOPE
//! atomic so the control is the one source of truth"*.
//!
//! Two independent flags would allow a state the control cannot display, and
//! then the UI shows one thing while the paths do another. Worth preserving
//! exactly.
//!
//! ## A chunk has to earn its LLM call
//!
//! Summarising costs a model round trip, so a chunk is gated first: content-word
//! ratio above 0.18 **and** at least six distinct content words. The second
//! condition is the one that looks redundant and is not — the original says it
//! *"guards against a short chunk with one repeated keyword passing the ratio
//! test"*, which is exactly what a nav bar or a cookie banner looks like.
//!
//! ## Deduplication keeps the FIRST of a near-duplicate pair
//!
//! *"Earlier chunk = earlier in the page = usually the cleaner statement."*
//! Keeping the last would tend to keep the footer's restatement over the
//! article's own sentence.

/// Which summary paths run.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Scope { Off, Terminal, Web, Full }

impl Scope {
    pub const ALL: [Scope; 4] = [Scope::Off, Scope::Terminal, Scope::Web, Scope::Full];

    pub fn label(self) -> &'static str {
        match self {
            Scope::Off => "Off", Scope::Terminal => "Terminal",
            Scope::Web => "Web", Scope::Full => "Full",
        }
    }

    /// Does the terminal-block path run?
    pub fn terminal_enabled(self) -> bool {
        matches!(self, Scope::Terminal | Scope::Full)
    }

    /// Does the page path run?
    pub fn web_enabled(self) -> bool {
        matches!(self, Scope::Web | Scope::Full)
    }

    /// Parse a stored setting. Unknown values fall back to the legacy
    /// behaviour rather than to Off: a settings file from an older build has
    /// no scope key, and defaulting to Off would silently switch off a feature
    /// the user had been using.
    pub fn parse(s: &str) -> Scope {
        match s.trim().to_ascii_lowercase().as_str() {
            "off" => Scope::Off,
            "web" => Scope::Web,
            "full" => Scope::Full,
            _ => Scope::Terminal,
        }
    }

    pub fn slug(self) -> &'static str {
        match self {
            Scope::Off => "off", Scope::Terminal => "terminal",
            Scope::Web => "web", Scope::Full => "full",
        }
    }
}

/// Chunking geometry.
pub const CHUNK_CHARS: usize = 1_500;
/// Later text is dropped rather than summarised — a page long enough to need a
/// seventh chunk is one whose first six already say what it is about, and the
/// cost is six model calls either way.
pub const MAX_CHUNKS: usize = 6;

/// Gating thresholds.
pub const MIN_CONTENT_RATIO: f32 = 0.18;
pub const MIN_DISTINCT_CONTENT: usize = 6;
/// Cosine at or above which two summaries are the same statement.
pub const DEDUP_THRESHOLD: f32 = 0.80;

const STOPWORDS: &[&str] = &[
    "the", "a", "an", "and", "or", "but", "of", "to", "in", "on", "for",
    "with", "at", "by", "from", "as", "is", "are", "was", "were", "be",
    "this", "that", "it", "its", "you", "your", "we", "our", "all",
];

/// Is a token a content word? Non-stopword, three characters or more, and not
/// a bare number — a page of prices is not a page about anything.
pub fn is_content_word(t: &str) -> bool {
    t.len() >= 3
        && !STOPWORDS.contains(&t)
        && !t.chars().all(|c| c.is_ascii_digit())
}

fn tokens(s: &str) -> Vec<String> {
    s.to_lowercase()
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|t| !t.is_empty())
        .map(|t| t.to_string())
        .collect()
}

/// Is this chunk worth a model round trip?
pub fn chunk_is_informative(chunk: &str) -> bool {
    let toks = tokens(chunk);
    if toks.is_empty() { return false; }
    let content: Vec<&String> = toks.iter().filter(|t| is_content_word(t)).collect();
    let ratio = content.len() as f32 / toks.len() as f32;
    let mut distinct: Vec<&str> = content.iter().map(|s| s.as_str()).collect();
    distinct.sort_unstable();
    distinct.dedup();
    ratio >= MIN_CONTENT_RATIO && distinct.len() >= MIN_DISTINCT_CONTENT
}

/// Split page text into at most `MAX_CHUNKS` chunks, on whitespace boundaries.
///
/// Words are never split: a chunk ending mid-word hands the model a fragment it
/// will faithfully summarise as if it were a word.
pub fn chunk(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    for word in text.split_whitespace() {
        if !cur.is_empty() && cur.len() + 1 + word.len() > CHUNK_CHARS {
            out.push(std::mem::take(&mut cur));
            if out.len() >= MAX_CHUNKS { return out; }
        }
        if !cur.is_empty() { cur.push(' '); }
        cur.push_str(word);
    }
    if !cur.trim().is_empty() && out.len() < MAX_CHUNKS { out.push(cur); }
    out
}

/// Cosine similarity over content-word sets.
pub fn similarity(a: &str, b: &str) -> f32 {
    let set = |s: &str| {
        let mut v: Vec<String> = tokens(s).into_iter().filter(|t| is_content_word(t)).collect();
        v.sort();
        v.dedup();
        v
    };
    let (sa, sb) = (set(a), set(b));
    if sa.is_empty() || sb.is_empty() { return 0.0; }
    let shared = sa.iter().filter(|t| sb.contains(t)).count() as f32;
    shared / (sa.len() as f32).sqrt() / (sb.len() as f32).sqrt()
}

/// Drop near-duplicate summaries, keeping the FIRST of each pair.
///
/// Order preserved, because the summaries are read as a list and reordering
/// them would break the page's own sequence.
pub fn dedup(summaries: &[String]) -> Vec<String> {
    let mut kept: Vec<String> = Vec::new();
    for s in summaries {
        if s.trim().is_empty() { continue; }
        if kept.iter().any(|k| similarity(k, s) >= DEDUP_THRESHOLD) { continue; }
        kept.push(s.clone());
    }
    kept
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_setting_drives_both_paths() {
        // Two booleans would allow a state the segmented control cannot
        // display, and then the UI shows one thing while the paths do another.
        assert!(!Scope::Off.terminal_enabled() && !Scope::Off.web_enabled());
        assert!(Scope::Terminal.terminal_enabled() && !Scope::Terminal.web_enabled());
        assert!(!Scope::Web.terminal_enabled() && Scope::Web.web_enabled());
        assert!(Scope::Full.terminal_enabled() && Scope::Full.web_enabled());
    }

    #[test]
    fn every_combination_of_the_two_paths_is_reachable() {
        // The four-way control is exactly the product of two booleans — if it
        // were not, some combination would be unreachable from the UI.
        let mut seen: Vec<(bool, bool)> =
            Scope::ALL.iter().map(|s| (s.terminal_enabled(), s.web_enabled())).collect();
        seen.sort();
        seen.dedup();
        assert_eq!(seen.len(), 4, "two scopes collapse to one behaviour");
    }

    #[test]
    fn an_older_settings_file_keeps_the_legacy_behaviour() {
        // A file with no scope key must not silently switch summaries OFF for
        // someone who was using them.
        assert_eq!(Scope::parse(""), Scope::Terminal);
        assert_eq!(Scope::parse("nonsense"), Scope::Terminal);
        assert_eq!(Scope::parse("off"), Scope::Off);
    }

    #[test]
    fn every_scope_round_trips_through_its_slug() {
        for s in Scope::ALL {
            assert_eq!(Scope::parse(s.slug()), s, "{}", s.label());
        }
    }

    #[test]
    fn a_nav_bar_does_not_earn_a_model_call() {
        // Short, repetitive, mostly stopwords — the ratio test alone can be
        // fooled by it, which is why the distinct floor exists.
        assert!(!chunk_is_informative("Home About Home About Home About"));
        assert!(!chunk_is_informative("the the the the the the the the"));
        assert!(!chunk_is_informative(""));
    }

    #[test]
    fn one_repeated_keyword_does_not_pass_the_ratio_test() {
        // The original's stated reason for MIN_DISTINCT_CONTENT: "guards
        // against a short chunk with one repeated keyword passing the ratio
        // test". A cookie banner is exactly this shape.
        let spam = "cookies cookies cookies cookies cookies cookies cookies";
        let toks: Vec<String> = spam.split_whitespace().map(|s| s.to_string()).collect();
        assert!(toks.iter().all(|t| is_content_word(t)), "ratio is 1.0 here");
        assert!(!chunk_is_informative(spam), "it passed on ratio alone");
    }

    #[test]
    fn real_prose_is_informative() {
        let text = "The compiler emits a diagnostic when the borrow checker \
                    rejects a program because two references alias mutable state.";
        assert!(chunk_is_informative(text));
    }

    #[test]
    fn a_page_of_numbers_is_not_a_page_about_anything() {
        // Bare numerals are excluded from content words: a price table would
        // otherwise read as dense prose.
        assert!(!is_content_word("2024"));
        assert!(!is_content_word("19"));
        assert!(is_content_word("2024a"));
        assert!(!chunk_is_informative("12 34 56 78 90 11 22 33 44 55 66"));
    }

    #[test]
    fn chunking_never_splits_a_word() {
        // A chunk ending mid-word hands the model a fragment it will
        // faithfully summarise as if it were a word.
        let text = "supercalifragilistic ".repeat(400);
        for c in chunk(&text) {
            assert!(!c.ends_with("supercalifrag"), "split mid-word: {}", &c[c.len().saturating_sub(20)..]);
            for w in c.split_whitespace() {
                assert_eq!(w, "supercalifragilistic");
            }
        }
    }

    #[test]
    fn chunking_stops_at_the_cap_rather_than_growing_with_the_page() {
        // Six model calls is the ceiling; a page long enough to need a seventh
        // has already said what it is about.
        let huge = "word ".repeat(100_000);
        assert_eq!(chunk(&huge).len(), MAX_CHUNKS);
    }

    #[test]
    fn a_short_page_is_one_chunk_and_an_empty_page_is_none() {
        assert_eq!(chunk("a short page about rust").len(), 1);
        assert!(chunk("").is_empty());
        assert!(chunk("   \n  ").is_empty());
    }

    #[test]
    fn dedup_keeps_the_earlier_statement() {
        // "Earlier chunk = earlier in the page = usually the cleaner
        // statement." Keeping the last would favour a footer's restatement
        // over the article's own sentence.
        let s = vec![
            "The borrow checker rejects aliasing mutable references".to_string(),
            "Borrow checker rejects aliasing mutable references entirely".to_string(),
            "Cargo builds the workspace with incremental compilation".to_string(),
        ];
        let out = dedup(&s);
        assert_eq!(out.len(), 2);
        assert!(out[0].starts_with("The borrow checker"), "kept the later one: {out:?}");
    }

    #[test]
    fn dedup_preserves_order() {
        // The summaries are read as a list; reordering breaks the page's own
        // sequence.
        let s: Vec<String> = ["alpha beta gamma delta", "epsilon zeta eta theta",
                              "iota kappa lambda mu"].iter().map(|x| x.to_string()).collect();
        assert_eq!(dedup(&s), s);
    }

    #[test]
    fn dedup_drops_blank_summaries() {
        // A model returning nothing should not leave a gap in the list.
        let s = vec!["real content here about things".into(), "  ".into(), String::new()];
        assert_eq!(dedup(&s).len(), 1);
    }

    #[test]
    fn similarity_is_symmetric_and_bounded() {
        let a = "the borrow checker rejects aliasing references";
        let b = "borrow checker rejects aliasing mutable references";
        let s = similarity(a, b);
        assert!((s - similarity(b, a)).abs() < 1e-6, "asymmetric");
        assert!((0.0..=1.0001).contains(&s), "out of range: {s}");
        assert!(similarity(a, "cargo builds workspaces incrementally") < DEDUP_THRESHOLD);
        assert_eq!(similarity("", "anything"), 0.0);
    }
}
