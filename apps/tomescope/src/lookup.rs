//! Offline word lookup: a small bundled glossary plus per-book index-term
//! lookup (from the book's own back-of-book index, if the format
//! supplied one — currently FB2/EPUB via `IndexTerm`). No network access;
//! this is intentionally not a full dictionary, just enough for quick
//! in-context definitions of common English words and any book-supplied
//! glossary terms.

use crate::content::{BookContent, IndexTerm};
use std::collections::HashMap;

/// A tiny built-in glossary. Real deployments would ship a bundled
/// wordlist asset; this is a representative seed covering common
/// literary/academic vocabulary so the lookup path has something to show
/// even with zero external assets.
pub fn builtin_glossary() -> HashMap<&'static str, &'static str> {
    let pairs: &[(&str, &str)] = &[
        ("ephemeral", "Lasting for a very short time."),
        ("ubiquitous", "Present, appearing, or found everywhere."),
        ("paradigm", "A typical example or pattern of something; a model."),
        ("juxtapose", "To place two things side by side, especially for contrasting effect."),
        ("cognizant", "Having knowledge or being aware of something."),
        ("empirical", "Based on observation or experience rather than theory."),
        ("axiom", "A statement regarded as self-evidently true, taken as a starting point."),
        ("corollary", "A proposition that follows naturally from one already proven."),
        ("heuristic", "A practical method not guaranteed to be optimal, used for quick solutions."),
        ("anachronism", "Something placed in the wrong historical time period."),
        ("verisimilitude", "The appearance of being true or real."),
        ("dichotomy", "A division into two contrasting parts."),
        ("nascent", "Just coming into existence and beginning to develop."),
        ("mercurial", "Subject to sudden or unpredictable changes of mood."),
        ("quixotic", "Extremely idealistic, unrealistic, and impractical."),
        ("synthesis", "The combination of ideas to form a theory or system."),
        ("catalyst", "A person or thing that precipitates an event or change."),
        ("dogmatic", "Inclined to lay down principles as incontrovertibly true."),
        ("pragmatic", "Dealing with things sensibly and realistically."),
        ("plausible", "Seeming reasonable or probable."),
    ];
    pairs.iter().copied().collect()
}

pub struct Lookup {
    glossary: HashMap<&'static str, &'static str>,
}

impl Default for Lookup {
    fn default() -> Self {
        Self {
            glossary: builtin_glossary(),
        }
    }
}

#[derive(Clone, Debug)]
pub enum LookupResult {
    Glossary { term: String, definition: String },
    BookIndex { term: String, locations: Vec<usize> },
    NotFound { term: String },
}

impl Lookup {
    /// Look up a word: check the book's own index first (more specific to
    /// this text), then fall back to the built-in glossary.
    pub fn lookup(&self, word: &str, content: &BookContent) -> LookupResult {
        let normalized = word.trim().to_lowercase();
        let normalized = normalized.trim_matches(|c: char| !c.is_alphanumeric());
        if normalized.is_empty() {
            return LookupResult::NotFound { term: word.to_string() };
        }

        let index_hits: Vec<usize> = content
            .index_terms
            .iter()
            .filter(|t: &&IndexTerm| t.term.to_lowercase() == normalized)
            .flat_map(|t| t.blocks.clone())
            .collect();
        if !index_hits.is_empty() {
            return LookupResult::BookIndex {
                term: normalized.to_string(),
                locations: index_hits,
            };
        }

        if let Some(def) = self.glossary.get(normalized) {
            return LookupResult::Glossary {
                term: normalized.to_string(),
                definition: def.to_string(),
            };
        }

        LookupResult::NotFound {
            term: normalized.to_string(),
        }
    }

    /// Full-text prefix search over the glossary, for a "browse dictionary"
    /// panel.
    pub fn glossary_prefix(&self, prefix: &str) -> Vec<(&'static str, &'static str)> {
        let prefix = prefix.to_lowercase();
        let mut out: Vec<(&'static str, &'static str)> = self
            .glossary
            .iter()
            .filter(|(k, _)| k.starts_with(&prefix))
            .map(|(k, v)| (*k, *v))
            .collect();
        out.sort_by_key(|(k, _)| *k);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::content::BookContent;

    fn empty_content() -> BookContent {
        BookContent {
            blocks: vec![],
            toc: vec![],
            images: vec![],
            footnotes: vec![],
            index_terms: vec![IndexTerm {
                term: "Heuristic".to_string(),
                blocks: vec![3, 8],
            }],
            raster: None,
            page_labels: vec![],
        }
    }

    #[test]
    fn book_index_takes_priority() {
        let lookup = Lookup::default();
        let content = empty_content();
        match lookup.lookup("heuristic", &content) {
            LookupResult::BookIndex { locations, .. } => assert_eq!(locations, vec![3, 8]),
            _ => panic!("expected book index hit"),
        }
    }

    #[test]
    fn glossary_fallback() {
        let lookup = Lookup::default();
        let content = BookContent {
            blocks: vec![],
            toc: vec![],
            images: vec![],
            footnotes: vec![],
            index_terms: vec![],
            raster: None,
            page_labels: vec![],
        };
        match lookup.lookup("ephemeral", &content) {
            LookupResult::Glossary { definition, .. } => assert!(definition.contains("short time")),
            _ => panic!("expected glossary hit"),
        }
    }

    #[test]
    fn unknown_word_not_found() {
        let lookup = Lookup::default();
        let content = BookContent {
            blocks: vec![],
            toc: vec![],
            images: vec![],
            footnotes: vec![],
            index_terms: vec![],
            raster: None,
            page_labels: vec![],
        };
        matches!(lookup.lookup("zzzznotaword", &content), LookupResult::NotFound { .. });
    }
}
