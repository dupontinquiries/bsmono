// src/nlp.rs
//
// v0.1.430: fast classical NLP primitives — pure Rust, no model,
// no network, no deps. All run in well under the local-LLM
// timeout window (the 8 s timeouts that motivated this whole
// direction). Three pieces:
//
//   1. RAKE  — Rapid Automatic Keyword Extraction. Multi-word
//      keyPHRASES (co-occurrence within stopword-delimited
//      candidate phrases), scored by word degree / frequency.
//      Upgrades the word smush from scattered single words to
//      real phrases ("machine learning safety", not three
//      separate tokens).
//
//   2. TF-IDF — term frequency × inverse document frequency over
//      a background document-frequency (DF) table. Surfaces what
//      is DISTINCTIVE about one document vs the corpus, instead
//      of just frequent. The DF table is a plain HashMap the
//      caller persists/builds from the page archive.
//
//   3. HashingVectorizer — fixed-dim sparse hashed bag-of-words
//      + cosine similarity. No vocabulary needed; powers
//      sentence/page similarity, dedup, "find similar".
//
// References: Rose et al. 2010 (RAKE); Spärck Jones 1972 (IDF);
// the hashing trick (Weinberger et al. 2009).

#![allow(dead_code)]

// ── Shared lexicon ──────────────────────────────────────────

/// Stopwords used as phrase delimiters (RAKE) and dropped from
/// keyword/vector tokens. Web boilerplate included so nav text
/// doesn't pollute results.
pub const STOPWORDS: &[&str] = &[
    "the", "a", "an", "and", "or", "but", "of", "to", "in", "on",
    "at", "for", "with", "by", "from", "as", "is", "are", "was",
    "were", "be", "been", "being", "this", "that", "these", "those",
    "it", "its", "we", "you", "your", "our", "they", "their", "he",
    "she", "his", "her", "i", "me", "my", "us", "them", "if", "then",
    "than", "so", "no", "not", "can", "will", "would", "should",
    "could", "may", "might", "do", "does", "did", "have", "has",
    "had", "all", "any", "more", "most", "some", "such", "only",
    "own", "same", "too", "very", "just", "about", "into", "over",
    "after", "before", "up", "down", "out", "off", "how", "what",
    "when", "where", "who", "which", "why", "there", "here", "also",
    "one", "two", "use", "using", "used", "get", "got",
    // web boilerplate
    "home", "menu", "login", "search", "click", "page", "cookies",
    "cookie", "privacy", "terms", "accept", "subscribe", "share",
    "read", "learn", "see", "sign", "newsletter",
];

/// v0.1.438: the single source of truth for stopword checks
/// across the NLP modules (textrank, session_walk delegate here
/// rather than keeping their own near-identical lists). Public
/// so siblings can call it.
pub fn is_stopword(w: &str) -> bool {
    STOPWORDS.contains(&w)
}

/// Lowercase tokens of length >= 2, alphanumeric runs. Keeps
/// numbers (they can be meaningful tokens for vectors); callers
/// that don't want pure numbers can filter.
pub fn tokenize(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|t| t.chars().count() >= 2)
        .map(|t| t.to_lowercase())
        .collect()
}

// ── 1. RAKE keyphrase extraction ────────────────────────────

/// Extract up to `max` keyphrases via RAKE. Splits text into
/// candidate phrases at stopwords + punctuation, scores each
/// content word by deg(word)/freq(word) (degree = total length
/// of phrases it appears in), sums per phrase, and returns the
/// top phrases by score (ties → earlier appearance, for
/// determinism). Phrases are returned lowercased, space-joined.
pub fn rake_keyphrases(text: &str, max: usize) -> Vec<String> {
    use std::collections::HashMap;

    // 1. Build candidate phrases: maximal runs of non-stopword,
    //    length>=2 tokens, broken by stopwords and punctuation.
    let mut phrases: Vec<Vec<String>> = Vec::new();
    let mut cur: Vec<String> = Vec::new();
    // We need punctuation as a break, so iterate the raw text in
    // "token or break" fashion.
    let mut token = String::new();
    let flush_token = |token: &mut String, cur: &mut Vec<String>, phrases: &mut Vec<Vec<String>>| {
        if token.is_empty() {
            return;
        }
        let w = token.to_lowercase();
        token.clear();
        if w.chars().count() < 2 || is_stopword(&w) || w.chars().all(|c| c.is_ascii_digit()) {
            // Stopword / too short / pure number → phrase break.
            if !cur.is_empty() {
                phrases.push(std::mem::take(cur));
            }
        } else {
            cur.push(w);
        }
    };
    for c in text.chars() {
        if c.is_alphanumeric() {
            token.push(c);
        } else {
            flush_token(&mut token, &mut cur, &mut phrases);
            // Hard punctuation also breaks a phrase.
            if matches!(c, '.' | ',' | ';' | ':' | '!' | '?' | '\n' | '(' | ')' | '"') && !cur.is_empty() {
                phrases.push(std::mem::take(&mut cur));
            }
        }
    }
    flush_token(&mut token, &mut cur, &mut phrases);
    if !cur.is_empty() {
        phrases.push(cur);
    }
    if phrases.is_empty() {
        return Vec::new();
    }

    // 2. Word scores: freq + degree. degree(w) = Σ over phrases
    //    containing w of (phrase length). word score = deg/freq.
    let mut freq: HashMap<&str, f32> = HashMap::new();
    let mut degree: HashMap<&str, f32> = HashMap::new();
    for ph in &phrases {
        let len = ph.len() as f32;
        for w in ph {
            *freq.entry(w.as_str()).or_insert(0.0) += 1.0;
            *degree.entry(w.as_str()).or_insert(0.0) += len;
        }
    }
    let word_score = |w: &str| -> f32 {
        let f = freq.get(w).copied().unwrap_or(1.0);
        let d = degree.get(w).copied().unwrap_or(f);
        if f <= 0.0 { 0.0 } else { d / f }
    };

    // 3. Phrase score = Σ word scores. Dedup identical phrases,
    //    keeping first-seen order for tie determinism.
    let mut seen: HashMap<String, (f32, usize)> = HashMap::new();
    let mut order = 0usize;
    for ph in &phrases {
        let text = ph.join(" ");
        let score: f32 = ph.iter().map(|w| word_score(w)).sum();
        seen.entry(text).or_insert_with(|| {
            let o = order;
            order += 1;
            (score, o)
        });
    }
    let mut scored: Vec<(String, f32, usize)> =
        seen.into_iter().map(|(p, (s, o))| (p, s, o)).collect();
    scored.sort_by(|a, b| {
        b.1.partial_cmp(&a.1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.2.cmp(&b.2))
    });
    scored.into_iter().take(max).map(|(p, _, _)| p).collect()
}

// ── 2. TF-IDF keyword extraction ────────────────────────────

/// Background document-frequency table: how many documents in
/// the corpus contain each term, plus the total document count.
/// Built incrementally as pages are archived; persisted by the
/// caller. Pure data — no behavior beyond accumulation.
#[derive(Clone, Debug, Default)]
pub struct DocFreq {
    pub n_docs: u32,
    pub df: std::collections::HashMap<String, u32>,
}

impl DocFreq {
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a document's text to the DF table: each UNIQUE term in
    /// the doc increments its document frequency by 1.
    pub fn add_document(&mut self, text: &str) {
        let uniq: std::collections::HashSet<String> = tokenize(text)
            .into_iter()
            .filter(|w| !is_stopword(w))
            .collect();
        for w in uniq {
            *self.df.entry(w).or_insert(0) += 1;
        }
        self.n_docs += 1;
    }

    /// v0.1.431: build a DF table from a collection of document
    /// texts in one pass (e.g. all archived page snippets). The
    /// archive is the source of truth, so we rebuild from it
    /// rather than persisting a separate table that could drift.
    pub fn from_texts<'a, I: IntoIterator<Item = &'a str>>(texts: I) -> Self {
        let mut df = DocFreq::new();
        for t in texts {
            df.add_document(t);
        }
        df
    }

    /// Smoothed IDF for a term: ln((1 + N) / (1 + df)) + 1.
    /// Unseen terms get the max IDF (treated as df=0 → distinctive).
    fn idf(&self, term: &str) -> f32 {
        let df = self.df.get(term).copied().unwrap_or(0) as f32;
        let n = self.n_docs as f32;
        ((1.0 + n) / (1.0 + df)).ln() + 1.0
    }
}

/// Extract up to `max` TF-IDF keywords for `text` against the
/// background DF table. TF is raw count within the doc; IDF from
/// the corpus. When the DF table is empty (cold start), this
/// degrades to pure term frequency (every IDF is equal). Returns
/// (word, score) descending; ties → earlier first appearance.
pub fn tfidf_keywords(text: &str, df: &DocFreq, max: usize) -> Vec<(String, f32)> {
    use std::collections::HashMap;
    let mut tf: HashMap<String, f32> = HashMap::new();
    let mut order: HashMap<String, usize> = HashMap::new();
    let mut n = 0usize;
    for w in tokenize(text) {
        if is_stopword(&w) || w.chars().all(|c| c.is_ascii_digit()) {
            continue;
        }
        *tf.entry(w.clone()).or_insert(0.0) += 1.0;
        order.entry(w).or_insert_with(|| {
            let o = n;
            n += 1;
            o
        });
    }
    let mut scored: Vec<(String, f32, usize)> = tf
        .into_iter()
        .map(|(w, f)| {
            let s = f * df.idf(&w);
            let o = order.get(&w).copied().unwrap_or(0);
            (w, s, o)
        })
        .collect();
    scored.sort_by(|a, b| {
        b.1.partial_cmp(&a.1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.2.cmp(&b.2))
    });
    scored
        .into_iter()
        .take(max)
        .map(|(w, s, _)| (w, s))
        .collect()
}

/// v0.1.448: suggest tags for a piece of text. Combines TF-IDF
/// (corpus-relative salience) with RAKE (multi-word phrases),
/// normalizes, drops `exclude` (already-applied user tags, matched
/// case-insensitively), and returns up to `max` clean candidates.
///
/// Order: TF-IDF single terms first (they're the most reliably
/// salient against the corpus), then RAKE phrases that add new
/// information, deduped. Phrases longer than 3 words are dropped
/// (a tag should be short). Pure + instant.
pub fn suggest_tags(text: &str, df: &DocFreq, exclude: &[String], max: usize) -> Vec<String> {
    use std::collections::HashSet;
    let excl: HashSet<String> = exclude.iter().map(|t| t.to_lowercase()).collect();
    let mut seen: HashSet<String> = HashSet::new();
    let mut out: Vec<String> = Vec::new();

    let push = |cand: &str, out: &mut Vec<String>, seen: &mut HashSet<String>| {
        let norm = cand.trim().to_lowercase();
        if norm.is_empty() { return; }
        // A tag is short: <= 3 words.
        if norm.split_whitespace().count() > 3 { return; }
        if excl.contains(&norm) || seen.contains(&norm) { return; }
        seen.insert(norm.clone());
        out.push(norm);
    };

    // TF-IDF single terms first.
    for (w, _) in tfidf_keywords(text, df, max * 2) {
        push(&w, &mut out, &mut seen);
        if out.len() >= max { return out; }
    }
    // Then RAKE phrases for richer multi-word tags.
    for phrase in rake_keyphrases(text, max * 2) {
        push(&phrase, &mut out, &mut seen);
        if out.len() >= max { break; }
    }
    out
}

// ── 3. Hashing vectorizer + cosine similarity ───────────────

/// Fixed dimensionality for hashed vectors. 1<<14 = 16384 buckets
/// — low collision rate for page-sized vocab, small enough to be
/// cheap. Power of two for fast masking.
pub const VEC_DIM: usize = 1 << 14;

/// A sparse hashed bag-of-words vector: bucket index → weight.
/// Built with the hashing trick (no vocabulary). L2-normalizable
/// for cosine similarity.
#[derive(Clone, Debug, Default)]
pub struct SparseVec {
    pub dims: std::collections::HashMap<u32, f32>,
}

/// FNV-1a hash (deterministic, no_std-friendly, fast). We don't
/// use the default RandomState because vectors must be stable
/// across runs (for persistence + cross-document comparison).
fn fnv1a(s: &str) -> u64 {
    crate::util::fnv1a(s)
}

/// Hash a token to a bucket in [0, VEC_DIM).
fn bucket(token: &str) -> u32 {
    (fnv1a(token) as usize % VEC_DIM) as u32
}

impl SparseVec {
    /// Build from text: term frequency per hashed bucket
    /// (stopwords + pure numbers dropped). Not normalized.
    pub fn from_text(text: &str) -> Self {
        let mut dims = std::collections::HashMap::new();
        for w in tokenize(text) {
            if is_stopword(&w) || w.chars().all(|c| c.is_ascii_digit()) {
                continue;
            }
            *dims.entry(bucket(&w)).or_insert(0.0) += 1.0;
        }
        SparseVec { dims }
    }

    /// L2 norm.
    pub fn norm(&self) -> f32 {
        self.dims.values().map(|v| v * v).sum::<f32>().sqrt()
    }

    /// Cosine similarity in [0, 1] (vectors are non-negative).
    /// 0 when either is empty.
    pub fn cosine(&self, other: &SparseVec) -> f32 {
        let (small, large) = if self.dims.len() <= other.dims.len() {
            (&self.dims, &other.dims)
        } else {
            (&other.dims, &self.dims)
        };
        let mut dot = 0.0f32;
        for (k, v) in small {
            if let Some(w) = large.get(k) {
                dot += v * w;
            }
        }
        let denom = self.norm() * other.norm();
        if denom <= 0.0 {
            0.0
        } else {
            (dot / denom).clamp(0.0, 1.0)
        }
    }

    /// Raw dot product (no normalization).
    pub fn dot(&self, other: &SparseVec) -> f32 {
        let (small, large) = if self.dims.len() <= other.dims.len() {
            (&self.dims, &other.dims)
        } else {
            (&other.dims, &self.dims)
        };
        let mut d = 0.0f32;
        for (k, v) in small {
            if let Some(w) = large.get(k) {
                d += v * w;
            }
        }
        d
    }

    /// v0.1.437: cosine using PRECOMPUTED norms — for the O(n²)
    /// pairwise graph pass, where recomputing each vector's norm
    /// on every comparison (as plain `cosine` does) wastes ~O(n)
    /// redundant norm passes per vector. Caller computes each
    /// norm once. Result identical to `cosine`.
    pub fn cosine_with_norms(&self, self_norm: f32, other: &SparseVec, other_norm: f32) -> f32 {
        let denom = self_norm * other_norm;
        if denom <= 0.0 {
            0.0
        } else {
            (self.dot(other) / denom).clamp(0.0, 1.0)
        }
    }
}

// ── Multi-speaker comprehension aid (foundation only) ───────
//
// v0.1.430: per-speaker keyphrase digest for the "track each
// other in a crowded multi-voice space" idea. This is the PURE,
// NLP-facing core only — it is deliberately NOT wired to live
// audio in this ship. The feature touches human behavior (people
// being summarized in a shared space), so the social design —
// consent, who-sees-it, activation — is settled separately
// before any live capture path is added. See docs/todo.md
// "multi-speaker aid".
//
// Design constraints encoded here:
//   - Operates on text ALREADY attributed to a speaker by the
//     operator's own capture pipeline. It does not attribute or
//     identify anyone itself.
//   - Carries a CONFIDENCE signal so the UI can show uncertainty
//     (or withhold) rather than asserting "X said Y" when
//     attribution over noisy multi-speaker audio is shaky.
//   - Activation gating (3+ voices) is a caller policy; this
//     module just provides the per-speaker digest.

/// Activation threshold: the aid is only meaningful past this
/// many concurrently-active speakers (below it, you can just
/// listen). Caller policy uses this; the module doesn't enforce.
pub const MULTI_SPEAKER_MIN_VOICES: usize = 3;

/// A confidence-tagged digest of one speaker's recent speech.
#[derive(Clone, Debug, PartialEq)]
pub struct SpeakerDigest {
    /// Top keyphrases (RAKE) of what this speaker has been saying.
    pub keyphrases: Vec<String>,
    /// 0.0..=1.0 — how much to TRUST this digest, derived from how
    /// much attributed text we have. Low text → low confidence →
    /// the UI should show tentatively or not at all. This exists
    /// specifically to avoid misattribution harm (reading
    /// "Sarah: layoffs" when the attribution was a coin-flip).
    pub confidence: f32,
    /// Word count of the attributed text the digest is built from
    /// (lets the caller set its own display threshold too).
    pub word_count: usize,
}

/// Build a confidence-tagged digest from one speaker's attributed
/// text. Confidence ramps with the amount of text: a few words is
/// not enough to trust an attribution, ~40+ words is solid.
/// Returns an empty digest (confidence 0) for trivial input — the
/// caller should NOT display empty/low-confidence digests.
pub fn speaker_digest(attributed_text: &str, max_phrases: usize) -> SpeakerDigest {
    let wc = attributed_text.split_whitespace().count();
    // Confidence: linear ramp 0 at 0 words → 1.0 at 40 words.
    let confidence = ((wc as f32) / 40.0).clamp(0.0, 1.0);
    let keyphrases = if wc < 4 {
        Vec::new() // too little to say anything
    } else {
        rake_keyphrases(attributed_text, max_phrases)
    };
    SpeakerDigest { keyphrases, confidence, word_count: wc }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Multi-speaker digest ──
    #[test]
    fn speaker_digest_confidence_ramps_with_text() {
        let little = speaker_digest("just a few words here", 3);
        let lots = speaker_digest(
            &"machine learning safety research matters a great deal in modern systems ".repeat(5),
            3,
        );
        assert!(lots.confidence > little.confidence);
        assert!(lots.confidence <= 1.0);
        assert!(little.confidence < 1.0);
    }

    #[test]
    fn speaker_digest_trivial_is_empty_low_confidence() {
        let d = speaker_digest("hi", 3);
        assert!(d.keyphrases.is_empty(), "too little text → no phrases");
        assert!(d.confidence < 0.2, "trivial input → low confidence");
    }

    #[test]
    fn speaker_digest_extracts_phrases_with_enough_text() {
        let d = speaker_digest(
            "We need machine learning safety. Machine learning safety is the priority for our team this quarter.",
            5,
        );
        assert!(!d.keyphrases.is_empty());
        assert!(d.confidence > 0.0);
    }

    // ── RAKE ──
    #[test]
    fn rake_extracts_multiword_phrases() {
        let text = "Machine learning safety is important. \
                    Machine learning systems need careful evaluation.";
        let ph = rake_keyphrases(text, 5);
        // Should surface a multi-word phrase, not just single words.
        assert!(ph.iter().any(|p| p.split_whitespace().count() >= 2),
            "expected a multi-word phrase, got {:?}", ph);
        assert!(ph.iter().any(|p| p.contains("machine learning")),
            "expected 'machine learning' phrase: {:?}", ph);
    }

    #[test]
    fn rake_breaks_on_stopwords() {
        // "the" and "of" are delimiters; "borrow checker" is a phrase.
        let text = "the borrow checker of rust";
        let ph = rake_keyphrases(text, 5);
        assert!(ph.iter().any(|p| p == "borrow checker"), "got {:?}", ph);
        // "rust" alone is a 1-word phrase, fine.
    }

    #[test]
    fn rake_empty() {
        assert!(rake_keyphrases("", 5).is_empty());
        assert!(rake_keyphrases("the of and to", 5).is_empty(),
            "all-stopword input → no phrases");
    }

    // ── TF-IDF ──
    #[test]
    fn tfidf_surfaces_distinctive_terms() {
        let mut df = DocFreq::new();
        // "common" appears in every doc; "quantum" in only one.
        df.add_document("common topic common topic common");
        df.add_document("common topic common another");
        df.add_document("common quantum entanglement quantum physics");
        let kw = tfidf_keywords("common quantum entanglement quantum physics", &df, 3);
        // "quantum"/"entanglement"/"physics" should outrank "common".
        let top: Vec<&str> = kw.iter().map(|(w, _)| w.as_str()).collect();
        assert!(top.contains(&"quantum"), "distinctive term surfaces: {:?}", top);
        assert!(!top.contains(&"common") || top[0] != "common",
            "common term should not dominate: {:?}", top);
    }

    #[test]
    fn tfidf_cold_start_is_term_frequency() {
        let df = DocFreq::new(); // empty
        let kw = tfidf_keywords("apple apple apple banana banana cherry", &df, 3);
        // With equal IDF, ranking = frequency: apple > banana > cherry.
        assert_eq!(kw[0].0, "apple");
        assert_eq!(kw[1].0, "banana");
    }

    #[test]
    fn docfreq_counts_unique_per_doc() {
        let mut df = DocFreq::new();
        df.add_document("alpha alpha alpha"); // alpha in 1 doc
        df.add_document("alpha beta");          // alpha in 2, beta in 1
        assert_eq!(df.n_docs, 2);
        assert_eq!(df.df.get("alpha"), Some(&2));
        assert_eq!(df.df.get("beta"), Some(&1));
    }

    #[test]
    fn docfreq_from_texts_matches_incremental() {
        let texts = vec!["alpha alpha beta", "alpha gamma", "beta beta"];
        let batch = DocFreq::from_texts(texts.iter().copied());
        let mut incr = DocFreq::new();
        for t in &texts {
            incr.add_document(t);
        }
        assert_eq!(batch.n_docs, incr.n_docs);
        assert_eq!(batch.n_docs, 3);
        assert_eq!(batch.df.get("alpha"), Some(&2)); // docs 1 & 2
        assert_eq!(batch.df.get("beta"), Some(&2));  // docs 1 & 3
        assert_eq!(batch.df.get("gamma"), Some(&1));
    }

    // ── Hashing vectorizer ──
    #[test]
    fn cosine_self_is_one() {
        let v = SparseVec::from_text("rust memory safety borrow checker");
        assert!((v.cosine(&v) - 1.0).abs() < 1e-5, "self-similarity ≈ 1");
    }

    #[test]
    fn cosine_similar_higher_than_dissimilar() {
        let a = SparseVec::from_text("rust memory safety borrow checker ownership");
        let b = SparseVec::from_text("memory safety in rust via the borrow checker");
        let c = SparseVec::from_text("bananas potassium fruit smoothie recipe");
        let ab = a.cosine(&b);
        let ac = a.cosine(&c);
        assert!(ab > ac, "related ({}) should beat unrelated ({})", ab, ac);
        assert!(ab > 0.0);
    }

    #[test]
    fn cosine_empty_is_zero() {
        let empty = SparseVec::from_text("the of and");
        let v = SparseVec::from_text("rust memory safety");
        assert_eq!(empty.cosine(&v), 0.0);
    }

    #[test]
    fn cosine_with_norms_matches_cosine() {
        // v0.1.437: the precomputed-norm path must be identical.
        let a = SparseVec::from_text("rust memory safety borrow checker ownership");
        let b = SparseVec::from_text("memory safety in rust via the borrow checker");
        let plain = a.cosine(&b);
        let fast = a.cosine_with_norms(a.norm(), &b, b.norm());
        assert!((plain - fast).abs() < 1e-6, "plain {} vs fast {}", plain, fast);
        // Empty-vector guard still holds.
        let empty = SparseVec::from_text("the of and");
        assert_eq!(empty.cosine_with_norms(empty.norm(), &a, a.norm()), 0.0);
    }

    #[test]
    fn fnv_is_deterministic() {
        assert_eq!(fnv1a("hello"), fnv1a("hello"));
        assert_ne!(fnv1a("hello"), fnv1a("world"));
        assert!((bucket("token") as usize) < VEC_DIM);
    }

    // v0.1.448: smart-tag suggestions.
    #[test]
    fn suggest_tags_surfaces_salient_terms() {
        let corpus = vec![
            "Rust ownership and borrowing memory safety compiler",
            "Python data science pandas numpy notebook",
            "JavaScript async await promises event loop",
        ];
        let df = DocFreq::from_texts(corpus.iter().copied());
        let text = "Rust async runtime tokio futures executor await polling the future";
        let tags = suggest_tags(text, &df, &[], 6);
        assert!(!tags.is_empty());
        // Salient domain terms surface; stopwords don't.
        assert!(tags.iter().any(|t| t == "tokio" || t == "futures" || t == "runtime"));
        assert!(!tags.iter().any(|t| t == "the"));
    }

    #[test]
    fn suggest_tags_respects_exclusions() {
        let corpus = vec!["alpha beta gamma", "delta epsilon"];
        let df = DocFreq::from_texts(corpus.iter().copied());
        let text = "tokio futures runtime executor scheduler";
        let first = suggest_tags(text, &df, &[], 3);
        assert!(!first.is_empty());
        // Excluding the first result (case-insensitively) drops it.
        let excl = vec![first[0].to_uppercase()];
        let second = suggest_tags(text, &df, &excl, 3);
        assert!(!second.contains(&first[0]), "excluded tag must not reappear");
    }

    #[test]
    fn suggest_tags_caps_length_and_count() {
        let corpus = vec!["one two three four five six seven"];
        let df = DocFreq::from_texts(corpus.iter().copied());
        let text = "alpha beta gamma delta epsilon zeta eta theta iota kappa";
        let tags = suggest_tags(text, &df, &[], 4);
        assert!(tags.len() <= 4, "respects max count");
        assert!(tags.iter().all(|t| t.split_whitespace().count() <= 3),
            "no tag longer than 3 words");
    }
}
