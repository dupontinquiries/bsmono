//! docroute.rs — pick the context a model actually needs, and say why.
//!
//! A small model kept going by a large one needs the right few files, not the repo. Embeddings would
//! be the obvious answer and are the wrong first move here: they need a model resident, they need the
//! index rebuilt when a file changes, and they cannot explain a choice. This is lexical, runs in
//! microseconds, and every selection carries its reason.
//!
//! ## Rarity, not frequency
//!
//! A file matching `arena_place` is worth far more than one matching `let` or `self`, and raw hit
//! counts get that backwards — the biggest file wins every query because it contains the most of
//! everything. Scoring by how RARE a term is across the corpus is the whole difference between
//! "returns the right file" and "returns lib.rs".
//!
//! ## Budget, not top-k
//!
//! The caller has a token budget, not a document count. Three small files may fit where one large one
//! does not, and a top-k that returns a 200 KB file for a 4k-token window has answered the wrong
//! question.
//!
//! ## Why each choice is reported
//!
//! A router that silently picks the wrong file looks exactly like a model that cannot reason. The
//! terms that earned a selection come back with it, so a bad answer can be traced to bad retrieval
//! instead of blamed on the model.

use std::collections::HashMap;

#[derive(Clone, PartialEq, Debug)]
pub struct Doc {
    pub id: String,
    /// Approximate cost of including this document, in tokens.
    pub tokens: usize,
    /// Lowercased terms, already split by the caller — the tokenizer is the caller's business.
    pub terms: Vec<String>,
}

#[derive(Clone, PartialEq, Debug)]
pub struct Selected {
    pub id: String,
    pub score: f32,
    /// The terms that earned it, strongest first. Empty is impossible — a doc with no matching term
    /// is never selected.
    pub matched: Vec<String>,
}

/// How many documents contain each term.
fn document_frequency(docs: &[Doc]) -> HashMap<&str, usize> {
    let mut df: HashMap<&str, usize> = HashMap::new();
    for d in docs {
        let mut seen: Vec<&str> = Vec::new();
        for t in &d.terms {
            if !seen.contains(&t.as_str()) {
                seen.push(t);
                *df.entry(t.as_str()).or_insert(0) += 1;
            }
        }
    }
    df
}

/// Score every document against the query and return what fits in `budget_tokens`.
///
/// Greedy by score, skipping documents that do not fit rather than stopping at the first — a 50-token
/// file after a 5000-token one is still worth having, and stopping early would drop it for no reason.
pub fn route(query: &[String], docs: &[Doc], budget_tokens: usize) -> Vec<Selected> {
    if docs.is_empty() || query.is_empty() { return Vec::new(); }
    let df = document_frequency(docs);
    let n = docs.len() as f32;

    let mut scored: Vec<Selected> = docs.iter().filter_map(|d| {
        let mut per_term: Vec<(String, f32)> = Vec::new();
        for q in query {
            let hits = d.terms.iter().filter(|t| t.as_str() == q.as_str()).count();
            if hits == 0 { continue; }
            // RARITY. A term in every document carries no information about which to pick, and
            // log(n/df) drives that to zero rather than merely down.
            let idf = (n / *df.get(q.as_str()).unwrap_or(&1) as f32).ln().max(0.0);
            if idf <= 0.0 { continue; }
            // Diminishing returns on repeats: a file mentioning a symbol forty times is not forty
            // times more relevant than one mentioning it twice, and without this the largest file
            // wins every query.
            per_term.push((q.clone(), idf * (1.0 + (hits as f32).ln())));
        }
        if per_term.is_empty() { return None; }
        per_term.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        Some(Selected {
            id: d.id.clone(),
            score: per_term.iter().map(|(_, s)| s).sum(),
            matched: per_term.into_iter().map(|(t, _)| t).collect(),
        })
    }).collect();

    scored.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));

    let mut out = Vec::new();
    let mut used = 0usize;
    for s in scored {
        let cost = docs.iter().find(|d| d.id == s.id).map(|d| d.tokens).unwrap_or(0);
        if used + cost > budget_tokens { continue; }
        used += cost;
        out.push(s);
    }
    out
}

/// Split text into routing terms: lowercased, and identifier-aware.
///
/// `arena_place` yields `arena_place`, `arena` and `place`, because a query may name the symbol or
/// either half of it, and a router that only matches the whole thing misses `place` entirely.
pub fn terms(text: &str) -> Vec<String> {
    let mut v = Vec::new();
    for raw in text.split(|c: char| !(c.is_alphanumeric() || c == '_')) {
        if raw.len() < 2 { continue; }
        let low = raw.to_lowercase();
        if low.contains('_') {
            for part in low.split('_') {
                if part.len() >= 2 { v.push(part.to_string()); }
            }
        }
        v.push(low);
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc(id: &str, tokens: usize, body: &str) -> Doc {
        Doc { id: id.into(), tokens, terms: terms(body) }
    }

    /// A small corpus where the obviously-right answer is not the biggest file.
    fn corpus() -> Vec<Doc> {
        vec![
            doc("arena.rs", 100, "fn arena_place slab mapped buffer offset slab slab"),
            doc("lib.rs", 5000, "let self fn use mod pub let self fn buffer let self fn use mod"),
            doc("tokenizer.rs", 200, "fn encode decode special tokens vocab byte level"),
        ]
    }

    #[test]
    fn a_rare_term_beats_a_common_one_even_in_a_smaller_file() {
        // The failure this scoring exists to avoid: raw hit counts hand every query to the biggest
        // file, because it contains the most of everything.
        let r = route(&terms("arena_place"), &corpus(), 10_000);
        assert_eq!(r[0].id, "arena.rs");
        assert!(r[0].matched.contains(&"arena".to_string()));
    }

    #[test]
    fn a_term_in_EVERY_document_selects_nothing() {
        // "buffer" appears in arena.rs and lib.rs; a term in all documents carries no information
        // about which to pick and must score zero rather than merely low.
        let docs = vec![
            doc("a.rs", 10, "buffer buffer"),
            doc("b.rs", 10, "buffer"),
        ];
        assert!(route(&terms("buffer"), &docs, 1000).is_empty());
    }

    #[test]
    fn REPEATS_have_diminishing_returns() {
        // A file naming a symbol forty times is not forty times more relevant than one naming it
        // twice. Without this the largest file wins again by another route.
        // A third document WITHOUT the term, or idf is ln(2/2)=0 and the rule above correctly scores
        // both at zero — my first fixture here had the term in every doc and failed for that reason,
        // which is the rule working rather than the scoring being wrong.
        let docs = vec![
            doc("few.rs", 10, "slab other words here"),
            doc("many.rs", 10, "slab slab slab slab slab slab slab slab"),
            doc("none.rs", 10, "unrelated content entirely"),
        ];
        let r = route(&terms("slab"), &docs, 1000);
        assert_eq!(r[0].id, "many.rs");
        assert!(r[0].score < r[1].score * 4.0, "8x the hits must not be 4x the score");
    }

    #[test]
    fn the_BUDGET_is_tokens_not_a_document_count() {
        // The caller has a context window, not a top-k. A 5000-token file that would blow the window
        // must be skipped even when it scores.
        let r = route(&terms("arena_place buffer encode"), &corpus(), 350);
        assert!(r.iter().all(|s| s.id != "lib.rs"), "lib.rs is 5000 tokens: {r:?}");
        assert!(r.iter().any(|s| s.id == "arena.rs"));
    }

    #[test]
    fn a_small_doc_AFTER_a_large_one_is_still_taken() {
        // Greedy-with-skip rather than stop-at-first-overflow. A 50-token file behind a 5000-token
        // one is still worth having and stopping early would drop it for nothing.
        let docs = vec![
            doc("big.rs", 9000, "slab slab slab"),
            doc("small.rs", 50, "slab"),
            doc("none.rs", 10, "unrelated content entirely"),
        ];
        let r = route(&terms("slab"), &docs, 1000);
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].id, "small.rs");
    }

    #[test]
    fn every_selection_carries_the_terms_that_EARNED_it() {
        // A router that silently picks the wrong file looks exactly like a model that cannot reason.
        let r = route(&terms("encode vocab"), &corpus(), 10_000);
        assert_eq!(r[0].id, "tokenizer.rs");
        assert!(!r[0].matched.is_empty());
        assert!(r[0].matched.iter().any(|m| m == "encode" || m == "vocab"));
    }

    #[test]
    fn an_identifier_matches_by_its_PARTS_as_well_as_whole() {
        // A query may name the symbol or either half of it, and matching only the whole thing misses
        // "place" entirely.
        let t = terms("arena_place");
        assert!(t.contains(&"arena_place".to_string()));
        assert!(t.contains(&"arena".to_string()));
        assert!(t.contains(&"place".to_string()));
    }

    #[test]
    fn a_query_matching_nothing_returns_nothing_rather_than_the_biggest_file() {
        // The failure mode of a router with no floor: it always returns something, and something
        // irrelevant is worse than an empty context the model can ask about.
        assert!(route(&terms("quantum entanglement"), &corpus(), 10_000).is_empty());
    }
}
