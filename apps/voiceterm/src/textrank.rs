// src/textrank.rs
//
// v0.1.429: TextRank extractive summarization — PageRank applied
// to sentences. Builds a graph where sentences are nodes and
// edge weights are word-overlap similarity, then runs power
// iteration to find the most "central" sentences. The top-ranked
// ones, returned in original document order, ARE the summary.
//
// This is a strict quality upgrade over "first N sentences": it
// picks sentences by importance (how much they share with the
// rest of the document), not by position. Pure Rust, no model,
// no network — runs in single-digit milliseconds on a page, so
// it never times out the way the local LLMs do (observed: 8 s
// timeouts on tinyllama / gemma:2b / phi3:mini).
//
// References: Mihalcea & Tarau, "TextRank: Bringing Order into
// Texts" (2004). The classic unsupervised extractive summarizer
// behind pre-LLM tools like gensim.summarize and sumy.

#![allow(dead_code)]

/// PageRank damping factor (standard 0.85).
const DAMPING: f32 = 0.85;
/// Max power-iteration steps. Convergence is usually < 20.
const MAX_ITERS: usize = 50;
/// Convergence threshold on the L1 delta between iterations.
const CONVERGENCE: f32 = 1e-4;
/// Sentences shorter than this (in words) are dropped before
/// ranking — they're usually nav fragments / headings, not prose.
const MIN_SENTENCE_WORDS: usize = 4;

/// v0.1.438: stopword check delegates to the shared
/// nlp::is_stopword (single source of truth). Was a local
/// near-identical list; nlp's list is a superset (adds web
/// boilerplate that's also fine to drop from sentence-similarity
/// word sets — those words shouldn't drive centrality anyway).
fn is_stopword(w: &str) -> bool {
    crate::nlp::is_stopword(w)
}

/// Split text into sentence spans on . ! ? boundaries, keeping
/// the terminator. (Same simple heuristic the extractive lede
/// uses — good enough for ranking; not linguistically perfect.)
pub fn split_sentences(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    for c in text.chars() {
        cur.push(c);
        if matches!(c, '.' | '!' | '?') {
            out.push(std::mem::take(&mut cur));
        }
    }
    if !cur.trim().is_empty() {
        out.push(cur);
    }
    out
}

/// The content-word set of a sentence (lowercased, stopword- and
/// short-token-filtered). Used for similarity.
fn word_set(sentence: &str) -> std::collections::HashSet<String> {
    let mut set = std::collections::HashSet::new();
    for raw in sentence.split(|c: char| !c.is_alphanumeric()) {
        let w = raw.to_lowercase();
        if w.chars().count() < 3 {
            continue;
        }
        if is_stopword(&w) {
            continue;
        }
        set.insert(w);
    }
    set
}

/// Similarity between two sentences: shared content words
/// normalized by the log of their lengths (the Mihalcea-Tarau
/// normalization, which prevents long sentences from dominating
/// purely by having more words). Returns 0 when either is empty
/// or there's no overlap.
fn similarity(
    a: &std::collections::HashSet<String>,
    b: &std::collections::HashSet<String>,
) -> f32 {
    if a.is_empty() || b.is_empty() {
        return 0.0;
    }
    let overlap = a.intersection(b).count() as f32;
    if overlap == 0.0 {
        return 0.0;
    }
    let denom = (a.len() as f32).ln() + (b.len() as f32).ln();
    if denom <= 0.0 {
        // Both sentences have 1 content word; ln(1)+ln(1)=0.
        // Fall back to raw overlap so we don't divide by zero.
        return overlap;
    }
    overlap / denom
}

/// Rank sentences by TextRank and return the indices (into the
/// filtered sentence list) of the top `top_k`, in ASCENDING
/// order (so the summary reads in document order). Internal —
/// `summarize` wraps this with sentence text.
fn rank(sentences: &[String], top_k: usize) -> Vec<usize> {
    let n = sentences.len();
    if n == 0 {
        return Vec::new();
    }
    if n <= top_k {
        return (0..n).collect();
    }

    let sets: Vec<_> = sentences.iter().map(|s| word_set(s)).collect();

    // Weighted adjacency + row sums (out-weights) for the
    // weighted PageRank update.
    let mut adj = vec![vec![0.0f32; n]; n];
    let mut out_sum = vec![0.0f32; n];
    for i in 0..n {
        for j in (i + 1)..n {
            let w = similarity(&sets[i], &sets[j]);
            if w > 0.0 {
                adj[i][j] = w;
                adj[j][i] = w;
                out_sum[i] += w;
                out_sum[j] += w;
            }
        }
    }

    // Power iteration. score_i = (1-d)/n + d * Σ_j (w_ji / outsum_j) * score_j
    let mut score = vec![1.0f32 / n as f32; n];
    let base = (1.0 - DAMPING) / n as f32;
    for _ in 0..MAX_ITERS {
        let mut next = vec![base; n];
        for j in 0..n {
            if out_sum[j] <= 0.0 {
                // Dangling node: distribute its score uniformly so
                // mass isn't lost.
                let share = DAMPING * score[j] / n as f32;
                for v in next.iter_mut() {
                    *v += share;
                }
                continue;
            }
            let contrib = DAMPING * score[j] / out_sum[j];
            for i in 0..n {
                if adj[j][i] > 0.0 {
                    next[i] += contrib * adj[j][i];
                }
            }
        }
        // L1 delta for convergence.
        let delta: f32 = next.iter().zip(&score).map(|(a, b)| (a - b).abs()).sum();
        score = next;
        if delta < CONVERGENCE {
            break;
        }
    }

    // Top-k by score (tie → earlier sentence), then re-sort the
    // chosen indices ascending for document order.
    let mut idx: Vec<usize> = (0..n).collect();
    idx.sort_by(|&a, &b| {
        score[b]
            .partial_cmp(&score[a])
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.cmp(&b))
    });
    idx.truncate(top_k);
    idx.sort_unstable();
    idx
}

/// Produce an extractive summary: the `max_sentences` most
/// central sentences (TextRank), joined in document order. Short
/// fragments are filtered before ranking. Falls back to the
/// leading sentences if filtering leaves too few. Returns empty
/// for empty / fragment-only input.
pub fn summarize(text: &str, max_sentences: usize) -> String {
    let raw = split_sentences(text);
    // Keep only prose-length sentences for ranking, remembering
    // their text.
    let candidates: Vec<String> = raw
        .into_iter()
        .map(|s| s.trim().to_string())
        .filter(|s| s.split_whitespace().count() >= MIN_SENTENCE_WORDS)
        .collect();
    if candidates.is_empty() {
        return String::new();
    }
    let k = max_sentences.max(1);
    let picks = rank(&candidates, k);
    picks
        .iter()
        .map(|&i| candidates[i].as_str())
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_and_fragments() {
        assert_eq!(summarize("", 3), "");
        // All sentences are < MIN_SENTENCE_WORDS.
        assert_eq!(summarize("Hi. Yo. Ok.", 3), "");
    }

    #[test]
    fn short_doc_returns_all_in_order() {
        let text = "The first sentence has enough words here. \
                    The second sentence also has plenty of words.";
        let s = summarize(text, 3);
        // Both kept (n <= k), in order.
        assert!(s.contains("first sentence"));
        assert!(s.contains("second sentence"));
        assert!(s.find("first").unwrap() < s.find("second").unwrap());
    }

    #[test]
    fn picks_central_sentence_over_outlier() {
        // Three sentences about "rust memory safety" + one
        // unrelated. With top_k=2 the unrelated outlier should be
        // dropped in favor of the topically-central ones.
        let text = "Rust guarantees memory safety without a garbage collector. \
                    Memory safety in Rust comes from the borrow checker. \
                    The borrow checker enforces memory ownership rules at compile time. \
                    Bananas are an excellent source of dietary potassium today.";
        let s = summarize(text, 2);
        assert!(!s.contains("Bananas"),
            "unrelated outlier should not be central: got {:?}", s);
        // At least one of the memory-safety sentences is present.
        assert!(s.to_lowercase().contains("memory") || s.to_lowercase().contains("borrow"));
    }

    #[test]
    fn output_in_document_order() {
        // Build a doc where the most-central sentences are NOT the
        // first ones, then assert output preserves source order.
        let text = "Intro line with some generic filler words here. \
                    Cats and dogs are common household pets worldwide. \
                    Dogs are loyal pets and dogs enjoy walks with owners. \
                    Pets like cats and dogs bring companionship to people. \
                    Closing remark that is mostly unrelated boilerplate text.";
        let s = summarize(text, 2);
        // Whatever two are chosen, they must appear in the same
        // relative order as in the source.
        let positions: Vec<usize> = ["Intro line", "Cats and dogs", "Dogs are loyal",
            "Pets like cats", "Closing remark"]
            .iter()
            .filter_map(|frag| s.find(frag))
            .collect();
        let mut sorted = positions.clone();
        sorted.sort_unstable();
        assert_eq!(positions, sorted, "summary must preserve document order");
    }

    #[test]
    fn similarity_basic() {
        let a = word_set("rust memory safety borrow checker");
        let b = word_set("memory safety comes from rust");
        let c = word_set("bananas potassium fruit snack");
        assert!(similarity(&a, &b) > 0.0, "overlapping sentences similar");
        assert_eq!(similarity(&a, &c), 0.0, "disjoint sentences not similar");
    }

    #[test]
    fn respects_max_sentences() {
        let text = "Alpha content sentence with enough words to keep it. \
                    Beta content sentence with enough words to keep it. \
                    Gamma content sentence with enough words to keep it. \
                    Delta content sentence with enough words to keep it.";
        let s = summarize(text, 2);
        // At most 2 sentence terminators in the output.
        let periods = s.matches('.').count();
        assert!(periods <= 2, "should pick at most 2 sentences, got {}", periods);
    }

    #[test]
    fn handles_single_content_word_sentences() {
        // ln(1)+ln(1)=0 path — must not panic / NaN.
        let text = "Apples apples apples apples here. \
                    Oranges oranges oranges oranges here. \
                    Apples oranges apples oranges mixed.";
        let s = summarize(text, 2);
        assert!(!s.is_empty());
    }
}
