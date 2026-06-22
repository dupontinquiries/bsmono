//! Populate the reverse-anchored context graph from real text (pure, no egui), so the retrieval
//! primitives operate on an actual corpus instead of synthetic ids — the precondition for the
//! Batch 9 attention-graph scheduling. Words are the tokens; a sliding window inserts, for each
//! position, (preceding `window` words → this word) as (context → endpoint). `min_context` then
//! answers "how many preceding words are needed to uniquely determine this continuation" on the real
//! distribution — small for a distinctive phrase, larger where the language is ambiguous.

use std::collections::HashMap;
use crate::context_graph::ContextGraph;

pub struct Corpus {
    vocab: HashMap<String, u32>,
    inv: Vec<String>,
    graph: ContextGraph,
    inserted: usize,
    ctx_tokens: usize, // total context tokens inserted (Σ context lengths) — for the naive baseline
}

impl Default for Corpus { fn default() -> Self { Self::new() } }

impl Corpus {
    pub fn new() -> Self { Self { vocab: HashMap::new(), inv: Vec::new(), graph: ContextGraph::new(), inserted: 0, ctx_tokens: 0 } }

    fn intern(&mut self, w: &str) -> u32 {
        if let Some(&i) = self.vocab.get(w) { return i; }
        let i = self.inv.len() as u32;
        self.vocab.insert(w.to_string(), i);
        self.inv.push(w.to_string());
        i
    }

    /// Lowercased alphanumeric word tokens.
    pub fn tokenize(text: &str) -> Vec<String> {
        text.split(|c: char| !c.is_alphanumeric()).filter(|s| !s.is_empty()).map(|s| s.to_lowercase()).collect()
    }

    /// Ingest `text`: for each word position, insert (preceding ≤`window` words → this word).
    pub fn ingest(&mut self, text: &str, window: usize) {
        let toks = Self::tokenize(text);
        let ids: Vec<u32> = toks.iter().map(|w| self.intern(w)).collect();
        for i in 0..ids.len() {
            let start = i.saturating_sub(window);
            self.graph.insert(&ids[start..i], ids[i]);
            self.inserted += 1;
            self.ctx_tokens += i - start;
        }
    }

    pub fn vocab_size(&self) -> usize { self.inv.len() }
    pub fn graph_nodes(&self) -> usize { self.graph.node_count() }
    pub fn tokens_ingested(&self) -> usize { self.inserted }

    /// Nodes a zero-sharing index would use: root + one anchor step per insert + every context token.
    pub fn naive_nodes(&self) -> usize { 1 + self.inserted + self.ctx_tokens }
    /// Fraction of nodes saved vs the naive baseline (endpoint anchors dedup by word + shared
    /// backward suffixes merge). >0 means the reverse-anchored index is doing real work.
    pub fn compression(&self) -> f32 {
        let naive = self.naive_nodes();
        if naive == 0 { return 0.0; }
        1.0 - self.graph_nodes() as f32 / naive as f32
    }

    /// Fewest preceding words that uniquely determine the LAST word of `phrase` (None if any word is
    /// out of vocabulary). Lower = the continuation is pinned by a short, distinctive context.
    pub fn min_context(&self, phrase: &str, window: usize) -> Option<usize> {
        let toks = Self::tokenize(phrase);
        if toks.is_empty() { return None; }
        let ids: Vec<u32> = toks.iter().filter_map(|w| self.vocab.get(w).copied()).collect();
        if ids.len() != toks.len() { return None; } // unknown word → can't query
        let endpoint = *ids.last().unwrap();
        let start = (ids.len() - 1).saturating_sub(window);
        Some(self.graph.minimal_context(&ids[start..ids.len() - 1], endpoint))
    }

    /// How many distinct contexts lead INTO `word` (branching/frequency at that endpoint).
    pub fn endpoint_load(&self, word: &str) -> Option<u32> {
        self.vocab.get(word).map(|&id| self.graph.endpoint_load(id))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // A short paragraph with deliberate repetition + one unique phrase, standing in for real text.
    const TEXT: &str = "the model reads the context and the model writes a token. \
        the context grows as the model writes more tokens. a quick brown fox is a unique phrase here. \
        the model keeps the context small by reading only what the context needs. \
        the context the context the model the model the token the token.";

    #[test]
    fn ingestion_shares_structure() {
        let mut c = Corpus::new();
        c.ingest(TEXT, 4);
        assert!(c.vocab_size() < c.tokens_ingested(), "words repeat: vocab {} < tokens {}", c.vocab_size(), c.tokens_ingested());
        // reverse-anchored index saves nodes vs the zero-sharing baseline (endpoint anchors dedup by
        // word + shared backward suffixes merge). Real text with window=4 shares modestly but >0.
        let comp = c.compression();
        assert!(comp > 0.05, "expected some node sharing vs naive, compression = {:.3}", comp);
        assert!(c.graph_nodes() < c.naive_nodes(), "nodes {} must be < naive {}", c.graph_nodes(), c.naive_nodes());
    }

    #[test]
    fn distinctive_phrase_needs_little_context() {
        let mut c = Corpus::new();
        c.ingest(TEXT, 4);
        // "brown" appears once and is only ever followed by "fox" → the continuation is pinned by a
        // very short context.
        let m = c.min_context("brown fox", 4).expect("in vocab");
        assert!(m <= 1, "a unique bigram should uniquify in ≤1 backward token, got {m}");
    }

    #[test]
    fn ambiguous_continuation_costs_more_or_unknown() {
        let mut c = Corpus::new();
        c.ingest(TEXT, 4);
        // "the" is followed by many different words; predicting a specific one needs more than zero
        // context (or never uniquifies). Either way it should be ≥ the distinctive case.
        let amb = c.min_context("the model", 4).expect("in vocab");
        let distinct = c.min_context("brown fox", 4).expect("in vocab");
        assert!(amb >= distinct, "ambiguous continuation ({amb}) should need ≥ distinctive ({distinct})");
    }

    #[test]
    fn frequent_endpoint_has_higher_load() {
        let mut c = Corpus::new();
        c.ingest(TEXT, 4);
        let the = c.endpoint_load("the").unwrap_or(0);
        let fox = c.endpoint_load("fox").unwrap_or(0);
        assert!(the > fox, "a frequent word ('the' load {the}) should exceed a rare one ('fox' load {fox})");
    }

    #[test]
    fn out_of_vocab_query_is_none() {
        let mut c = Corpus::new();
        c.ingest(TEXT, 4);
        assert!(c.min_context("nonexistent zorblax", 4).is_none());
    }
}
