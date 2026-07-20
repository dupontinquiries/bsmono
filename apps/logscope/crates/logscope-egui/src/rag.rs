//! rag.rs — local, deterministic retrieval over LogScope's own corpus (voice-term Zettelkasten notes,
//! coordinator/chat transcripts, repo code, logs, scorecards). A "web search across your own database":
//! when the aggressive coordinator is problem-solving, the most relevant prior notes/decisions/log lines
//! get pulled in to boost context — but only after the user opts in via a card (see `cards::CardKind`).
//!
//! Method: **BM25** (Okapi BM25 lexical ranking). Chosen deliberately over embeddings for on-device use:
//!   * Deterministic + no model — no embedding index to build/version, no GPU contention with decode,
//!     reproducible results (a given query always ranks the same, which matters for a coordinator that
//!     must be debuggable).
//!   * Fast enough to run per-query over tens of thousands of chunks with a plain inverted index.
//!   * Fully unit-testable with zero I/O (this module is pure; the corpus adapters that read files live
//!     alongside and feed `Doc`s in).
//!
//! This file is the pure core: tokenize → index → score. No egui, no filesystem, no engine — so the
//! ranking math is tested in isolation and the same index serves every corpus (notes, code, logs…).

use std::collections::HashMap;

/// Where a document came from — so results can be labeled and the coordinator knows what it's reading.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Source {
    /// A voice-term Zettelkasten note.
    Note,
    /// A coordinator or chat transcript turn.
    Transcript,
    /// A chunk of repository source code.
    Code,
    /// A log line / block (e.g. an engine or worker log).
    Log,
    /// A scorecard / harness result.
    Scorecard,
}

impl Source {
    pub fn label(self) -> &'static str {
        match self {
            Source::Note => "note",
            Source::Transcript => "transcript",
            Source::Code => "code",
            Source::Log => "log",
            Source::Scorecard => "scorecard",
        }
    }
}

/// One indexed unit. `id` is stable/caller-assigned (e.g. "note:1a2b", "code:path#L10-40") so a result
/// points back at its origin; `title` is a short human label; `text` is the searchable body.
#[derive(Clone, Debug)]
pub struct Doc {
    pub id: String,
    pub source: Source,
    pub title: String,
    pub text: String,
}

/// A ranked hit: the doc index into the corpus, its BM25 score, and the matched query terms (for
/// highlighting / explaining WHY it surfaced — a coordinator injecting context should be able to say so).
#[derive(Clone, Debug, PartialEq)]
pub struct Hit {
    pub doc: usize,
    pub score: f32,
    pub terms: Vec<String>,
}

/// Lowercase alphanumeric tokenizer with a code-friendly split: identifiers break on non-alnum, so
/// `decode_attention_mh_q8` → [decode, attention, mh, q8] and `StagePolicy::plan` → [stagepolicy, plan].
/// Deterministic and dependency-free. Tokens shorter than 2 chars are dropped as noise except pure
/// digits (kept: "q8", "4096" carry meaning in this corpus).
pub fn tokenize(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    for ch in s.chars() {
        if ch.is_ascii_alphanumeric() {
            cur.push(ch.to_ascii_lowercase());
        } else if !cur.is_empty() {
            if cur.len() >= 2 { out.push(std::mem::take(&mut cur)); } else { cur.clear(); }
        }
    }
    if cur.len() >= 2 { out.push(cur); }
    out
}

/// An inverted-index BM25 store. Build once from a corpus, query many times.
pub struct Bm25 {
    docs: Vec<Doc>,
    /// term -> list of (doc index, term frequency in that doc)
    postings: HashMap<String, Vec<(usize, u32)>>,
    /// per-doc token length (for length normalization)
    doc_len: Vec<u32>,
    avg_len: f32,
    k1: f32,
    b: f32,
}

impl Bm25 {
    /// Standard BM25 parameters (k1=1.5, b=0.75) — the widely-used defaults; tunable if a corpus wants it.
    pub fn new(docs: Vec<Doc>) -> Self {
        Self::with_params(docs, 1.5, 0.75)
    }

    pub fn with_params(docs: Vec<Doc>, k1: f32, b: f32) -> Self {
        let mut postings: HashMap<String, Vec<(usize, u32)>> = HashMap::new();
        let mut doc_len = Vec::with_capacity(docs.len());
        for (di, d) in docs.iter().enumerate() {
            let toks = tokenize(&d.text);
            doc_len.push(toks.len() as u32);
            // term frequency within this doc
            let mut tf: HashMap<&str, u32> = HashMap::new();
            for t in &toks { *tf.entry(t.as_str()).or_insert(0) += 1; }
            for (t, f) in tf { postings.entry(t.to_string()).or_default().push((di, f)); }
        }
        let total: u64 = doc_len.iter().map(|&l| l as u64).sum();
        let avg_len = if docs.is_empty() { 0.0 } else { total as f32 / docs.len() as f32 };
        Bm25 { docs, postings, doc_len, avg_len, k1, b }
    }

    pub fn len(&self) -> usize { self.docs.len() }
    pub fn is_empty(&self) -> bool { self.docs.is_empty() }
    pub fn doc(&self, i: usize) -> &Doc { &self.docs[i] }

    /// Inverse document frequency for a term (Robertson/Sparck-Jones form, +1 smoothed so it's ≥0 and a
    /// term in every doc contributes ~0 rather than going negative).
    fn idf(&self, term: &str) -> f32 {
        let n = self.docs.len() as f32;
        let df = self.postings.get(term).map(|p| p.len()).unwrap_or(0) as f32;
        ((n - df + 0.5) / (df + 0.5) + 1.0).ln()
    }

    /// Rank documents for `query`, returning the top `k` hits by BM25 score (descending; ties broken by
    /// lower doc index for determinism). Only docs matching ≥1 query term are returned.
    pub fn search(&self, query: &str, k: usize) -> Vec<Hit> {
        self.search_filtered(query, k, |_| true)
    }

    /// `search`, but only over docs whose `Source` passes `keep` — so a caller can scope to notes-only,
    /// exclude code, etc., without building a second index.
    pub fn search_filtered(&self, query: &str, k: usize, keep: impl Fn(Source) -> bool) -> Vec<Hit> {
        if self.docs.is_empty() { return Vec::new(); }
        let q_terms = tokenize(query);
        if q_terms.is_empty() { return Vec::new(); }
        // dedup query terms but remember them for the Hit explanation
        let mut seen = std::collections::HashSet::new();
        let uniq: Vec<String> = q_terms.into_iter().filter(|t| seen.insert(t.clone())).collect();

        let mut acc: HashMap<usize, (f32, Vec<String>)> = HashMap::new();
        for term in &uniq {
            let idf = self.idf(term);
            if let Some(list) = self.postings.get(term) {
                for &(di, f) in list {
                    if !keep(self.docs[di].source) { continue; }
                    let tf = f as f32;
                    let len = self.doc_len[di] as f32;
                    let denom = tf + self.k1 * (1.0 - self.b + self.b * len / self.avg_len.max(1.0));
                    let contrib = idf * (tf * (self.k1 + 1.0)) / denom.max(1e-6);
                    let e = acc.entry(di).or_insert((0.0, Vec::new()));
                    e.0 += contrib;
                    e.1.push(term.clone());
                }
            }
        }
        let mut hits: Vec<Hit> = acc.into_iter()
            .map(|(doc, (score, terms))| Hit { doc, score, terms })
            .collect();
        // descending score, ties → lower doc index (deterministic)
        hits.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal)
            .then(a.doc.cmp(&b.doc)));
        hits.truncate(k);
        hits
    }
}

/// A compact, model-ready context block assembled from hits — what actually gets handed to the
/// coordinator when the user accepts the search card. Keeps each snippet bounded so injecting retrieved
/// context can't blow the window; labels each with its source and why it matched.
pub fn assemble_context(index: &Bm25, hits: &[Hit], per_snippet_chars: usize) -> String {
    if hits.is_empty() { return String::new(); }
    let mut out = String::from("Relevant context retrieved from your database:\n");
    for (n, h) in hits.iter().enumerate() {
        let d = index.doc(h.doc);
        let clip: String = {
            let t = d.text.trim();
            if t.chars().count() > per_snippet_chars {
                let s: String = t.chars().take(per_snippet_chars).collect();
                format!("{s}…")
            } else { t.to_string() }
        };
        out.push_str(&format!("\n[{}] ({}) {} — matched: {}\n{}\n",
            n + 1, d.source.label(), d.title, h.terms.join(", "), clip));
    }
    out
}

/// A corpus root to index: a directory, the `Source` its files represent, and the file extensions to
/// include. The builder walks it (bounded depth) and turns each matching file — or, for large files, each
/// bounded chunk — into a `Doc`. Kept separate from the pure BM25 core so the ranking stays I/O-free and
/// testable; this is the thin adapter that feeds real files in.
#[derive(Clone, Debug)]
pub struct CorpusRoot {
    pub dir: std::path::PathBuf,
    pub source: Source,
    /// lowercase extensions without the dot, e.g. ["md","txt"] for notes, ["rs"] for code.
    pub exts: Vec<String>,
}

/// Split a file's text into bounded chunks so one huge file (a long transcript, a big source file) becomes
/// several targeted `Doc`s rather than one blob that dilutes BM25's length normalization. Splits on blank
/// lines where possible, then hard-caps at `max_chars`. Pure — unit-tested.
pub fn chunk_text(text: &str, max_chars: usize) -> Vec<String> {
    let max = max_chars.max(200);
    let mut chunks = Vec::new();
    let mut cur = String::new();
    for para in text.split("\n\n") {
        if cur.chars().count() + para.chars().count() + 2 > max && !cur.is_empty() {
            chunks.push(std::mem::take(&mut cur));
        }
        if para.chars().count() > max {
            // a single oversized paragraph: hard-split it
            let mut rest = para;
            while rest.chars().count() > max {
                let cut: String = rest.chars().take(max).collect();
                chunks.push(cut);
                rest = &rest[rest.char_indices().nth(max).map(|(i, _)| i).unwrap_or(rest.len())..];
            }
            if !rest.is_empty() { cur.push_str(rest); }
        } else {
            if !cur.is_empty() { cur.push_str("\n\n"); }
            cur.push_str(para);
        }
    }
    if !cur.trim().is_empty() { chunks.push(cur); }
    chunks.into_iter().filter(|c| !c.trim().is_empty()).collect()
}

/// Build a `Doc` corpus from roots by walking each directory (bounded to `max_depth`, skipping hidden dirs
/// and anything over `max_file_bytes`), reading matching files, and chunking them. Best-effort: unreadable
/// files are skipped, not fatal. The `Bm25` index is then built from the returned Docs.
pub fn build_corpus(roots: &[CorpusRoot], max_depth: usize, max_file_bytes: u64, chunk_chars: usize) -> Vec<Doc> {
    let mut docs = Vec::new();
    for root in roots {
        walk_into(&root.dir, root, 0, max_depth, max_file_bytes, chunk_chars, &mut docs);
    }
    docs
}

fn walk_into(dir: &std::path::Path, root: &CorpusRoot, depth: usize, max_depth: usize,
             max_file_bytes: u64, chunk_chars: usize, out: &mut Vec<Doc>) {
    if depth > max_depth { return; }
    let Ok(entries) = std::fs::read_dir(dir) else { return; };
    for e in entries.flatten() {
        let path = e.path();
        let name = e.file_name();
        let name = name.to_string_lossy();
        if name.starts_with('.') { continue; } // skip hidden
        if path.is_dir() {
            if name == "node_modules" || name == "target" || name == ".git" { continue; }
            walk_into(&path, root, depth + 1, max_depth, max_file_bytes, chunk_chars, out);
        } else if path.is_file() {
            let ext = path.extension().map(|x| x.to_string_lossy().to_lowercase()).unwrap_or_default();
            if !root.exts.iter().any(|e| e == &ext) { continue; }
            if std::fs::metadata(&path).map(|m| m.len()).unwrap_or(u64::MAX) > max_file_bytes { continue; }
            let Ok(text) = std::fs::read_to_string(&path) else { continue; };
            let rel = path.strip_prefix(&root.dir).unwrap_or(&path).to_string_lossy().to_string();
            let chunks = chunk_text(&text, chunk_chars);
            let multi = chunks.len() > 1;
            for (ci, chunk) in chunks.into_iter().enumerate() {
                let (id, title) = if multi {
                    (format!("{}:{rel}#{ci}", root.source.label()), format!("{rel} (part {})", ci + 1))
                } else {
                    (format!("{}:{rel}", root.source.label()), rel.clone())
                };
                out.push(Doc { id, source: root.source, title, text: chunk });
            }
        }
    }
}

/// An opt-in search offer, prepared but NOT yet run — retrieval waits for the user's yes. The caller
/// suggests a `cards::CardKind::SearchDatabase` card with this text; on accept it calls
/// [`run_accepted_search`]. Separating "offer" from "run" is what makes this opt-in rather than auto-inject.
pub struct SearchOffer {
    /// Stable card key (dedup + dismissal memory): the same query won't re-nag once dismissed.
    pub key: String,
    pub title: String,
    pub body: String,
    /// The query to run if accepted (the coordinator's objective / current problem text).
    pub query: String,
    /// How many probable matches we saw when deciding to offer (for card body / score).
    pub probable_hits: usize,
}

/// Offer a search if the corpus plausibly helps. `min_hits` guards against nagging when nothing matches:
/// a cheap ranking pass counts relevance and we only offer if ≥ that many docs match. The ranked context
/// is discarded here — the injected context is only assembled on accept, so an ignored offer costs nothing.
pub fn offer_search(index: &Bm25, query: &str, min_hits: usize) -> Option<SearchOffer> {
    if index.is_empty() { return None; }
    let hits = index.search(query, 8);
    if hits.len() < min_hits.max(1) { return None; }
    // order-independent, deduped key so the same problem (any word order/casing) shares one dismissal key
    let mut norm = tokenize(query);
    norm.sort();
    norm.dedup();
    let key = format!("ragsearch:{}", norm.join("-"));
    let sources: std::collections::BTreeSet<&str> = hits.iter().take(5)
        .map(|h| index.doc(h.doc).source.label()).collect();
    let body = format!(
        "Found {} relevant item(s) in your database ({}). Search and add them to the coordinator's context?",
        hits.len(), sources.into_iter().collect::<Vec<_>>().join(", "));
    Some(SearchOffer { key, title: "Search your database?".into(), body, query: query.to_string(), probable_hits: hits.len() })
}

/// The user accepted the search card: run BM25 for real and assemble the bounded context block to inject
/// into the coordinator. `k` top hits, each clipped to `per_snippet_chars`. Empty if nothing matched.
pub fn run_accepted_search(index: &Bm25, query: &str, k: usize, per_snippet_chars: usize) -> String {
    let hits = index.search(query, k);
    assemble_context(index, &hits, per_snippet_chars)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn corpus() -> Vec<Doc> {
        vec![
            Doc { id: "note:1".into(), source: Source::Note, title: "windowed decode crash".into(),
                  text: "the M4 metal buffer object ceiling hangs the gpu when the sample window k is 128; cap it".into() },
            Doc { id: "code:1".into(), source: Source::Code, title: "governor.rs".into(),
                  text: "the governor autotunes the window k from live buffer objects; metal caps objects not bytes".into() },
            Doc { id: "log:1".into(), source: Source::Log, title: "asahi timeout".into(),
                  text: "asahi gpu timeout halted attempting recovery during prefill dispatch watchdog".into() },
            Doc { id: "score:1".into(), source: Source::Scorecard, title: "kv staging".into(),
                  text: "staged kv cache hot f32 window warm int8 tail two span attention validated".into() },
        ]
    }

    #[test]
    fn tokenizer_splits_identifiers_and_keeps_codey_tokens() {
        let t = tokenize("decode_attention_mh_q8 StagePolicy::plan 4096");
        assert!(t.contains(&"decode".to_string()));
        assert!(t.contains(&"q8".to_string()));
        assert!(t.contains(&"stagepolicy".to_string()));
        assert!(t.contains(&"plan".to_string()));
        assert!(t.contains(&"4096".to_string()));
        // single chars dropped as noise
        assert!(!t.iter().any(|s| s.len() < 2));
    }

    #[test]
    fn ranks_the_most_relevant_doc_first() {
        let idx = Bm25::new(corpus());
        let hits = idx.search("window buffer object ceiling metal", 4);
        assert!(!hits.is_empty());
        // the note is squarely about the window/buffer/metal ceiling → should top the code doc
        assert_eq!(idx.doc(hits[0].doc).id, "note:1");
    }

    #[test]
    fn matched_terms_are_reported_for_explanation() {
        let idx = Bm25::new(corpus());
        let hits = idx.search("asahi watchdog prefill", 2);
        let top = &hits[0];
        assert_eq!(idx.doc(top.doc).id, "log:1");
        assert!(top.terms.contains(&"asahi".to_string()));
        assert!(top.terms.contains(&"watchdog".to_string()));
    }

    #[test]
    fn source_filter_scopes_results() {
        let idx = Bm25::new(corpus());
        // same query, but only notes: must not return the code/log/score docs
        let hits = idx.search_filtered("window metal gpu staged", 5, |s| s == Source::Note);
        assert!(hits.iter().all(|h| idx.doc(h.doc).source == Source::Note));
    }

    #[test]
    fn empty_query_and_empty_corpus_are_safe() {
        let idx = Bm25::new(corpus());
        assert!(idx.search("", 5).is_empty());
        assert!(idx.search("   ::  ", 5).is_empty());
        let empty = Bm25::new(vec![]);
        assert!(empty.search("anything", 5).is_empty());
        assert!(empty.is_empty());
    }

    #[test]
    fn ranking_is_deterministic_across_runs() {
        let a = Bm25::new(corpus()).search("gpu window metal staged kv", 4);
        let b = Bm25::new(corpus()).search("gpu window metal staged kv", 4);
        assert_eq!(a, b, "same corpus + query must rank identically every run");
    }

    #[test]
    fn chunk_text_splits_large_files_and_keeps_small_ones_whole() {
        // a small text stays one chunk
        assert_eq!(chunk_text("short note about the governor", 500).len(), 1);
        // a big text splits into several bounded chunks
        let big = (0..50).map(|i| format!("paragraph {i} with several words of content here")).collect::<Vec<_>>().join("\n\n");
        let chunks = chunk_text(&big, 200);
        assert!(chunks.len() > 1, "large text should split");
        for c in &chunks { assert!(c.chars().count() <= 400, "each chunk bounded"); }
        // an oversized single paragraph is hard-split, not dropped
        let oneline = "x".repeat(1000);
        let hs = chunk_text(&oneline, 200);
        assert!(hs.len() >= 4);
    }

    #[test]
    fn offer_is_suppressed_when_too_few_hits() {
        let idx = Bm25::new(corpus());
        // a query with no corpus overlap → no offer
        assert!(offer_search(&idx, "quarterly marketing budget spreadsheet", 1).is_none());
        // a matching query with a high min_hits bar → suppressed
        assert!(offer_search(&idx, "gpu", 99).is_none());
        // a matching query that clears the bar → offered
        let off = offer_search(&idx, "gpu window metal staged", 1).expect("should offer");
        assert!(off.probable_hits >= 1);
        assert!(off.key.starts_with("ragsearch:"));
    }

    #[test]
    fn offer_key_is_stable_for_the_same_problem() {
        let idx = Bm25::new(corpus());
        // same terms, different order/casing → same dedup key (so dismissal sticks)
        let a = offer_search(&idx, "window metal gpu", 1).unwrap().key;
        let b = offer_search(&idx, "GPU  Window   Metal", 1).unwrap().key;
        assert_eq!(a, b, "same problem must yield the same card key for dismissal memory");
    }

    #[test]
    fn accepted_search_produces_injectable_context() {
        let idx = Bm25::new(corpus());
        let ctx = run_accepted_search(&idx, "asahi watchdog prefill timeout", 3, 80);
        assert!(ctx.contains("retrieved from your database"));
        assert!(ctx.contains("(log)"), "the asahi log should be in the injected context");
    }

    #[test]
    fn assembled_context_is_bounded_and_labeled() {
        let idx = Bm25::new(corpus());
        let hits = idx.search("window metal", 2);
        let ctx = assemble_context(&idx, &hits, 40);
        assert!(ctx.contains("(note)") || ctx.contains("(code)"));
        assert!(ctx.contains("matched:"));
        // each snippet clipped to ~40 chars + ellipsis, so the block stays small
        for line in ctx.lines().filter(|l| l.len() > 50) {
            assert!(line.chars().count() < 120, "snippet line should be bounded: {line}");
        }
    }
}
