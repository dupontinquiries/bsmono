// src/web_summary.rs
//
// v0.1.414 (AI summaries → web): summarize the CURRENT WEB PAGE
// with the LLM, parallel to the terminal-block summaries in
// log_summary.rs.
//
// Also home to the unified summary-SCOPE setting the user
// requested: Off / Terminal / Web / Full, a single segmented
// control deciding which summary paths run.
//   - Off:      no auto-summaries anywhere
//   - Terminal: summarize command output only (legacy behavior)
//   - Web:      summarize web pages only
//   - Full:     both
//
// The terminal path (log_summary) consults `terminal_enabled()`;
// this module's page path consults `web_enabled()`. Both derive
// from the single SCOPE atomic so the control is the one source
// of truth.
//
// Design choice: this is a FOCUSED path, not a reuse of
// log_summary's block queue. That queue is terminal-specific
// (diffing against prior runs, dumb-summary fallback, email
// reshaping, per-block_id keying) — none of which fits a page
// summary. We share the LLM call (llm_provider::provider_query)
// and nothing else.

#![allow(dead_code)]

use std::sync::atomic::{AtomicI32, AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

/// Which summary paths are active. Persisted as an i32.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SummaryScope {
    Off,
    Terminal,
    Web,
    Full,
}

impl SummaryScope {
    pub fn from_i32(v: i32) -> SummaryScope {
        match v {
            1 => SummaryScope::Terminal,
            2 => SummaryScope::Web,
            3 => SummaryScope::Full,
            _ => SummaryScope::Off,
        }
    }
    pub fn to_i32(self) -> i32 {
        match self {
            SummaryScope::Off => 0,
            SummaryScope::Terminal => 1,
            SummaryScope::Web => 2,
            SummaryScope::Full => 3,
        }
    }
    /// Short label for the UI segmented control.
    pub fn label(self) -> &'static str {
        match self {
            SummaryScope::Off => "Off",
            SummaryScope::Terminal => "Terminal",
            SummaryScope::Web => "Web",
            SummaryScope::Full => "Full",
        }
    }
    pub fn terminal_active(self) -> bool {
        matches!(self, SummaryScope::Terminal | SummaryScope::Full)
    }
    pub fn web_active(self) -> bool {
        matches!(self, SummaryScope::Web | SummaryScope::Full)
    }
}

// Default Off — auto-summaries are opt-in (they ship page/term
// text to an LLM, so the user must choose to enable).
static SCOPE: AtomicI32 = AtomicI32::new(0);

pub fn set_scope(scope: SummaryScope) {
    SCOPE.store(scope.to_i32(), Ordering::Relaxed);
    crate::clog!("web-summary", "scope={}", scope.label());
}

pub fn scope() -> SummaryScope {
    SummaryScope::from_i32(SCOPE.load(Ordering::Relaxed))
}

pub fn terminal_enabled() -> bool {
    scope().terminal_active()
}

pub fn web_enabled() -> bool {
    scope().web_active()
}

// ── Page summary state ───────────────────────────────────────
//
// The most recent page summary, keyed by URL. We keep only the
// latest (a page summary isn't worth a multi-entry cache the way
// terminal blocks are — the user is on one page at a time). QML
// polls `revision()` to detect a new summary, then reads
// `current_summary()`.

#[derive(Clone, Debug)]
pub struct PageSummary {
    pub url: String,
    pub text: String,
    pub model: String,
    pub completed_at: u64,
    /// v0.1.424: true if this is the instant rule-based
    /// (extractive) summary, false once an LLM has produced a
    /// better one. The staged pathway shows the extractive
    /// summary immediately, then the LLM upgrades it — and if the
    /// LLM times out (common on slow local models), the
    /// extractive summary remains as a useful fallback rather
    /// than a blank banner.
    pub is_extractive: bool,
}

static CURRENT: OnceLock<Mutex<Option<PageSummary>>> = OnceLock::new();
static REVISION: AtomicU64 = AtomicU64::new(0);
// Set true while a request is in flight so QML can show a spinner
// and we don't double-fire for the same page.
static IN_FLIGHT: AtomicI32 = AtomicI32::new(0);

fn current_cell() -> &'static Mutex<Option<PageSummary>> {
    CURRENT.get_or_init(|| Mutex::new(None))
}

pub fn revision() -> u64 {
    REVISION.load(Ordering::Relaxed)
}

pub fn in_flight() -> bool {
    IN_FLIGHT.load(Ordering::Relaxed) != 0
}

/// The most recent page summary, if any. QML reads this after
/// `revision()` changes.
pub fn current_summary() -> Option<PageSummary> {
    current_cell().lock().ok()?.clone()
}

/// Clear the stored summary (e.g. on navigation to a new page so
/// a stale summary isn't shown for the wrong page).
pub fn clear() {
    if let Ok(mut g) = current_cell().lock() {
        if g.is_some() {
            *g = None;
            REVISION.fetch_add(1, Ordering::Relaxed);
        }
    }
}

fn store(summary: PageSummary) {
    if let Ok(mut g) = current_cell().lock() {
        *g = Some(summary);
    }
    REVISION.fetch_add(1, Ordering::Relaxed);
}

/// Extract readable text from a page's HTML for summarization.
/// Prefers the <body>, collapses whitespace, truncates to
/// `max_chars`. Returns empty when there's no usable text.
///
/// Strips script/style content by relying on scraper's text()
/// which only yields text nodes — but script/style text nodes
/// ARE included by scraper, so we additionally drop the obvious
/// ones by selecting body and skipping known-noise via a simple
/// post-filter is overkill; instead we cap length and let the
/// prompt instruct the model to ignore boilerplate.
pub fn extract_page_text(html: &str, max_chars: usize) -> String {
    let doc = scraper::Html::parse_document(html);
    // Try <body>; fall back to whole-doc text.
    let body_sel = scraper::Selector::parse("body").ok();
    let raw: String = if let Some(sel) = body_sel.as_ref() {
        if let Some(body) = doc.select(sel).next() {
            body.text().collect()
        } else {
            doc.root_element().text().collect()
        }
    } else {
        doc.root_element().text().collect()
    };
    let clean: String = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    truncate_chars(&clean, max_chars)
}

/// Char-boundary-safe truncation.
fn truncate_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    s.chars().take(max).collect()
}

/// Build the page-summary prompt. Asks for a tight, useful
/// summary and explicitly tells the model to ignore nav/boilerplate.
pub fn build_page_prompt(url: &str, title: &str, text: &str) -> String {
    let mut p = String::with_capacity(text.len() + 500);
    // v0.1.445: knowledge-extraction framing (was a generic
    // "summarize for a reader" book-report prompt). The user works
    // in a terminal-first tool: what they need from a page is the
    // EXTRACTABLE knowledge — facts, definitions, commands,
    // parameters, decisions, gotchas — not prose. This prompt asks
    // for that, so the result helps them act / jump back to the
    // terminal with context, rather than just "what the page is
    // about".
    p.push_str("Extract the actionable knowledge from this web page for someone who will act on it. ");
    p.push_str("Focus on: concrete facts, key definitions, commands or code, parameters/values, ");
    p.push_str("steps, decisions, and gotchas. Skip framing and opinion. ");
    p.push_str("Output 3-5 short bullet-style lines, each a single concrete takeaway. ");
    p.push_str("No preamble, no \"this page\"; lead each line with the fact itself.\n\n");
    if !title.trim().is_empty() {
        p.push_str("Title: ");
        p.push_str(title.trim());
        p.push('\n');
    }
    if !url.trim().is_empty() {
        p.push_str("URL: ");
        p.push_str(url.trim());
        p.push('\n');
    }
    p.push_str("\nPage text:\n");
    p.push_str(text);
    p
}

/// v0.1.445: directed/"take action" prompt. When the user clicks
/// "Take action" and (optionally) speaks a directive, the page is
/// summarized THROUGH the lens of that directive — e.g. "find the
/// install command", "what's the config flag for X", "give me the
/// curl example". Empty directive falls back to general knowledge
/// extraction. This is what lets the user point the read at a
/// goal and jump back to the terminal with exactly what they need.
pub fn build_directed_prompt(directive: &str, title: &str, text: &str) -> String {
    let d = directive.trim();
    if d.is_empty() {
        return build_page_prompt("", title, text);
    }
    let mut p = String::with_capacity(text.len() + 500);
    p.push_str("The reader has a specific goal for this page. Goal: \"");
    p.push_str(d);
    p.push_str("\".\n");
    p.push_str("Answer the goal using ONLY information present on the page. ");
    p.push_str("If the page contains a command, code, value, or exact steps relevant ");
    p.push_str("to the goal, quote them verbatim. Be concise and concrete — the reader ");
    p.push_str("will act on this immediately. If the page does not address the goal, say so ");
    p.push_str("in one line. No preamble.\n\n");
    if !title.trim().is_empty() {
        p.push_str("Title: ");
        p.push_str(title.trim());
        p.push('\n');
    }
    p.push_str("\nPage text:\n");
    p.push_str(text);
    p
}

// ── Chunked summaries (v0.1.425) ─────────────────────────────
//
// Splitting the page into chunks and summarizing each with a
// SHORT prompt lets a slow local model complete each piece
// within its per-call timeout, where one big call over the
// whole page would time out (observed on the user's hardware:
// tinyllama / gemma:2b / phi3:mini all timed out at 8s on a
// full-page prompt). The chunk summaries are combined
// progressively so the banner builds up rather than waiting for
// everything.

/// Target characters per chunk. Small enough that a single chunk
/// is a quick LLM call; large enough that we don't make dozens
/// of calls. ~1500 chars ≈ a few paragraphs.
pub const CHUNK_CHARS: usize = 1500;
/// Cap the number of chunks so a huge page doesn't fan out into
/// an unbounded number of LLM calls.
pub const MAX_CHUNKS: usize = 6;

/// Split page text into chunks of ~CHUNK_CHARS, breaking at
/// sentence boundaries so a chunk doesn't cut mid-thought.
/// Returns at most MAX_CHUNKS chunks (later text is dropped —
/// web ledes front-load the important content, same rationale
/// as the row cap / extractive lede).
pub fn chunk_text(text: &str) -> Vec<String> {
    let mut chunks = Vec::new();
    let mut cur = String::new();
    for sentence in split_sentences(text) {
        // If adding this sentence would push the chunk well past
        // the target AND we already have content, close the chunk.
        if !cur.is_empty() && cur.chars().count() + sentence.chars().count() > CHUNK_CHARS {
            chunks.push(std::mem::take(&mut cur));
            if chunks.len() >= MAX_CHUNKS {
                return chunks;
            }
        }
        cur.push_str(&sentence);
    }
    if !cur.trim().is_empty() && chunks.len() < MAX_CHUNKS {
        chunks.push(cur);
    }
    chunks
}

/// Prompt for extracting the key knowledge from ONE chunk into a
/// single tight line. Deliberately terse — a short ask completes
/// faster and is less likely to time out on a small local model.
/// v0.1.445: reframed from "key point" to extractable knowledge
/// (fact / command / value / decision) for the terminal-first
/// reader.
pub fn build_chunk_prompt(chunk: &str) -> String {
    let mut p = String::with_capacity(chunk.len() + 200);
    p.push_str("State the single most useful fact, command, value, or step in this passage, ");
    p.push_str("in ONE concise line. If it's a command or value, quote it exactly. ");
    p.push_str("No preamble, no \"this passage\".\n\n");
    p.push_str(chunk);
    p
}

/// v0.1.442: minimum content-word ratio for a chunk to be worth
/// an LLM call. Below this the chunk is mostly stopwords / nav
/// boilerplate / link text — summarizing it spends a call (and on
/// local models, possibly an 8s timeout) to produce noise. 0.18
/// is conservative: ordinary prose runs 0.4-0.6 content-word
/// ratio, so this only drops the genuinely thin chunks.
const MIN_CONTENT_RATIO: f32 = 0.18;
/// A chunk also needs at least this many DISTINCT content words —
/// guards against a short chunk with one repeated keyword passing
/// the ratio test.
const MIN_DISTINCT_CONTENT: usize = 6;

/// Is this chunk informative enough to be worth an LLM call?
/// Content-word ratio (non-stopword, length>=3, non-numeric
/// tokens / total tokens) above MIN_CONTENT_RATIO AND at least
/// MIN_DISTINCT_CONTENT distinct content words. Pure + instant.
pub fn chunk_is_informative(chunk: &str) -> bool {
    let mut total = 0usize;
    let mut content = 0usize;
    let mut distinct = std::collections::HashSet::new();
    for raw in chunk.split(|c: char| !c.is_alphanumeric()) {
        if raw.is_empty() {
            continue;
        }
        total += 1;
        let w = raw.to_lowercase();
        if w.chars().count() < 3 {
            continue;
        }
        if w.chars().all(|c| c.is_ascii_digit()) {
            continue;
        }
        if crate::nlp::is_stopword(&w) {
            continue;
        }
        content += 1;
        distinct.insert(w);
    }
    if total == 0 {
        return false;
    }
    let ratio = content as f32 / total as f32;
    ratio >= MIN_CONTENT_RATIO && distinct.len() >= MIN_DISTINCT_CONTENT
}

/// Filter chunks to those worth an LLM call. If filtering would
/// drop EVERYTHING (e.g. a short page that's all borderline), we
/// keep the original list — better to summarize thin content than
/// to summarize nothing. Order preserved.
pub fn select_informative_chunks(chunks: Vec<String>) -> Vec<String> {
    let kept: Vec<String> = chunks
        .iter()
        .filter(|c| chunk_is_informative(c))
        .cloned()
        .collect();
    if kept.is_empty() {
        chunks
    } else {
        kept
    }
}

/// Prompt for combining several per-chunk sentence-summaries
/// into a final scannable knowledge digest. Only used when there
/// are multiple chunks (one chunk needs no combine pass).
pub fn build_combine_prompt(chunk_summaries: &[String]) -> String {
    let mut p = String::with_capacity(512);
    // v0.1.445: combine into an actionable knowledge digest, not a
    // prose paragraph. Keep concrete items (facts/commands/values)
    // as distinct lines so the reader can scan and act.
    p.push_str("Combine these notes into a tight, scannable knowledge digest of the page. ");
    p.push_str("Keep concrete facts, commands, and values as distinct short lines; ");
    p.push_str("merge duplicates; drop filler. 3-5 lines max. No preamble.\n\n");
    for (i, cs) in chunk_summaries.iter().enumerate() {
        let _ = std::fmt::Write::write_fmt(&mut p, format_args!("{}. {}\n", i + 1, cs.trim()));
    }
    p
}

/// Join partial chunk summaries into a progressive combined text
/// to show in the banner while chunks are still being processed.
/// Just concatenates with spaces — the real combine pass (if any)
/// happens via the LLM at the end.
pub fn join_partials(chunk_summaries: &[String]) -> String {
    chunk_summaries
        .iter()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

/// v0.1.433: collapse near-duplicate chunk summaries via content
/// cosine similarity before the combine pass. Long pages often
/// repeat the same point across sections; without dedup the
/// combine prompt (and the join_partials fallback) carries
/// redundant notes. Keeps the FIRST occurrence of each
/// near-duplicate (earlier chunk = earlier in the page = usually
/// the cleaner statement). Two summaries with cosine >=
/// DEDUP_THRESHOLD are considered duplicates. Order preserved.
const DEDUP_THRESHOLD: f32 = 0.80;

pub fn dedup_chunk_summaries(summaries: &[String]) -> Vec<String> {
    let mut kept: Vec<String> = Vec::new();
    let mut kept_vecs: Vec<crate::nlp::SparseVec> = Vec::new();
    for s in summaries {
        let st = s.trim();
        if st.is_empty() {
            continue;
        }
        let v = crate::nlp::SparseVec::from_text(st);
        let is_dup = kept_vecs.iter().any(|kv| kv.cosine(&v) >= DEDUP_THRESHOLD);
        if !is_dup {
            kept.push(st.to_string());
            kept_vecs.push(v);
        }
    }
    kept
}

/// Store a progressive (chunked) partial summary. Like
/// stage_extractive it keeps is_extractive=true (it's still a
/// "building" state, not the final combined LLM answer) and does
/// NOT end the in-flight request. Won't downgrade a final
/// (non-extractive) summary.
pub fn stage_partial(url: &str, text: &str) {
    if let Ok(g) = current_cell().lock() {
        if let Some(existing) = g.as_ref() {
            if existing.url == url && !existing.is_extractive {
                return;
            }
        }
    }
    store(PageSummary {
        url: url.to_string(),
        text: text.to_string(),
        model: "chunked".to_string(),
        completed_at: epoch_secs(),
        is_extractive: true,
    });
}

/// Decide whether a page is worth summarizing. Skips trivially
/// short pages (the user can just read those) and our own
/// internal web2qml:// pages.
pub fn worth_summarizing(url: &str, text: &str) -> bool {
    if url.starts_with("web2qml://") {
        return false;
    }
    // Need a reasonable amount of prose. ~200 chars ≈ a few
    // sentences; below that, summarizing is pointless.
    text.chars().count() >= 200
}

/// Mark a request as started (returns false if one's already in
/// flight — caller should skip to avoid stacking requests).
pub fn begin_request() -> bool {
    // compare_exchange: 0 → 1. If it was already 1, someone else
    // is running; return false.
    IN_FLIGHT
        .compare_exchange(0, 1, Ordering::SeqCst, Ordering::SeqCst)
        .is_ok()
}

pub fn end_request() {
    IN_FLIGHT.store(0, Ordering::Relaxed);
}

/// Store a completed LLM summary (called by the worker thread on
/// the backend side). Marks is_extractive=false (this is the
/// upgraded version) and ends the in-flight state.
pub fn finish(url: &str, text: &str, model: &str) {
    store(PageSummary {
        url: url.to_string(),
        text: text.to_string(),
        model: model.to_string(),
        completed_at: epoch_secs(),
        is_extractive: false,
    });
    end_request();
}

/// v0.1.424: stage the instant extractive summary BEFORE the LLM
/// runs. Stores it (is_extractive=true) so the banner shows
/// something immediately, but does NOT end the in-flight state —
/// the LLM upgrade is still coming. If the LLM later times out,
/// end_request() is called without a finish(), leaving this
/// extractive summary in place as the fallback.
///
/// Only stages if there isn't already a NON-extractive summary
/// for this url (don't downgrade a good LLM summary to extractive
/// on a re-trigger).
pub fn stage_extractive(url: &str, text: &str) {
    if let Ok(g) = current_cell().lock() {
        if let Some(existing) = g.as_ref() {
            if existing.url == url && !existing.is_extractive {
                return; // already have a better summary; don't downgrade
            }
        }
    }
    store(PageSummary {
        url: url.to_string(),
        text: text.to_string(),
        model: "extractive".to_string(),
        completed_at: epoch_secs(),
        is_extractive: true,
    });
}

/// Build an instant, no-LLM extractive summary from page text:
/// the most CENTRAL sentences (v0.1.429: via TextRank — PageRank
/// over a sentence-similarity graph), capped to ~max_chars.
/// Gives the reader a real gist immediately while the LLM (maybe)
/// produces something better. TextRank picks by importance, not
/// position — a strict upgrade over the old first-N-sentences
/// approach, and still instant (no model, no timeout). Falls back
/// to leading sentences only if TextRank yields nothing.
pub fn extractive_summary(text: &str, max_chars: usize) -> String {
    // TextRank: up to 3 most-central sentences in document order.
    let ranked = crate::textrank::summarize(text, 3);
    if !ranked.is_empty() {
        return truncate_chars(&ranked, max_chars);
    }
    // Fallback (e.g. TextRank filtered everything as fragments):
    // the old leading-sentences lede.
    let mut out = String::new();
    let mut count = 0usize;
    for sentence in split_sentences(text) {
        let s = sentence.trim();
        if s.split_whitespace().count() < 4 {
            continue;
        }
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(s);
        count += 1;
        if count >= 3 || out.chars().count() >= max_chars {
            break;
        }
    }
    truncate_chars(&out, max_chars)
}

/// Split text into sentence-like spans on . ! ? boundaries.
/// Keeps the terminator with the sentence. Not linguistically
/// perfect (won't handle "Dr." etc.) but fine for a lede preview.
/// v0.1.438: delegate to textrank::split_sentences (was a
/// byte-identical local copy). Single implementation now.
fn split_sentences(text: &str) -> Vec<String> {
    crate::textrank::split_sentences(text)
}

fn epoch_secs() -> u64 {
    crate::util::epoch_secs()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scope_roundtrip() {
        for s in [SummaryScope::Off, SummaryScope::Terminal,
                  SummaryScope::Web, SummaryScope::Full] {
            assert_eq!(SummaryScope::from_i32(s.to_i32()), s);
        }
    }

    #[test]
    fn scope_unknown_is_off() {
        assert_eq!(SummaryScope::from_i32(99), SummaryScope::Off);
        assert_eq!(SummaryScope::from_i32(-1), SummaryScope::Off);
    }

    #[test]
    fn scope_active_flags() {
        assert!(!SummaryScope::Off.terminal_active());
        assert!(!SummaryScope::Off.web_active());
        assert!(SummaryScope::Terminal.terminal_active());
        assert!(!SummaryScope::Terminal.web_active());
        assert!(!SummaryScope::Web.terminal_active());
        assert!(SummaryScope::Web.web_active());
        assert!(SummaryScope::Full.terminal_active());
        assert!(SummaryScope::Full.web_active());
    }

    #[test]
    fn extract_strips_tags_collapses_ws() {
        let html = "<html><body><h1>Title</h1>  <p>Hello   world.</p>\n\n<p>More.</p></body></html>";
        let t = extract_page_text(html, 1000);
        assert!(t.contains("Title"));
        assert!(t.contains("Hello world."));
        // Whitespace collapsed — no double spaces.
        assert!(!t.contains("  "));
    }

    #[test]
    fn extract_truncates() {
        let body = "word ".repeat(500); // 2500 chars
        let html = format!("<html><body><p>{}</p></body></html>", body);
        let t = extract_page_text(&html, 100);
        assert!(t.chars().count() <= 100);
    }

    #[test]
    fn worth_summarizing_rules() {
        let long = "word ".repeat(100); // > 200 chars
        assert!(worth_summarizing("https://x.com/a", &long));
        // Internal pages skipped.
        assert!(!worth_summarizing("web2qml://home", &long));
        // Too short skipped.
        assert!(!worth_summarizing("https://x.com/a", "tiny"));
    }

    #[test]
    fn prompt_includes_title_and_text() {
        let p = build_page_prompt("https://x.com", "My Title", "Some content here.");
        assert!(p.contains("My Title"));
        assert!(p.contains("Some content here."));
        assert!(p.contains("https://x.com"));
    }

    #[test]
    fn prompt_omits_empty_title() {
        let p = build_page_prompt("https://x.com", "", "Body text.");
        assert!(!p.contains("Title:"));
        assert!(p.contains("Body text."));
    }

    #[test]
    fn truncate_chars_respects_boundary() {
        // Multi-byte chars shouldn't panic.
        let s = "café ".repeat(50);
        let t = truncate_chars(&s, 10);
        assert_eq!(t.chars().count(), 10);
    }

    // v0.1.424: staged extractive summary.
    #[test]
    fn extractive_picks_central_not_just_leading() {
        // v0.1.429: extractive_summary now uses TextRank, which
        // picks CENTRAL sentences, not the first N. The three
        // topically-linked sentences should win over the unrelated
        // trailing one.
        let text = "Rust guarantees memory safety without a garbage collector. \
                    Memory safety in Rust comes from the borrow checker. \
                    The borrow checker enforces memory ownership at compile time. \
                    Bananas are an excellent source of dietary potassium today.";
        let s = extractive_summary(text, 400);
        assert!(!s.contains("Bananas"),
            "unrelated outlier should be dropped by centrality: {:?}", s);
        assert!(s.to_lowercase().contains("memory") || s.to_lowercase().contains("borrow"),
            "central topic present");
    }

    #[test]
    fn extractive_skips_short_fragments() {
        // Nav-like short fragments (<4 words) are skipped.
        let text = "Home. Login. About. \
                    This is a real sentence with enough words to count.";
        let s = extractive_summary(text, 400);
        assert!(s.contains("real sentence"), "real prose kept");
        assert!(!s.starts_with("Home"), "short fragment skipped");
    }

    #[test]
    fn extractive_respects_char_cap() {
        let text = "Word ".repeat(200) + "and more words here for sure.";
        let s = extractive_summary(&text, 50);
        assert!(s.chars().count() <= 50);
    }

    #[test]
    fn extractive_empty_for_blank() {
        assert_eq!(extractive_summary("", 400), "");
        // Only short fragments → nothing usable.
        assert_eq!(extractive_summary("Hi. Yo. Ok.", 400), "");
    }

    // v0.1.425: chunked summaries.
    #[test]
    fn chunk_text_splits_long_text() {
        // Build text well over CHUNK_CHARS with clear sentences.
        let sentence = "This is a reasonably long sentence used to pad the chunk. ";
        let text = sentence.repeat(120); // ~120 * 58 ≈ 6960 chars
        let chunks = chunk_text(&text);
        assert!(chunks.len() > 1, "long text should split: got {}", chunks.len());
        assert!(chunks.len() <= MAX_CHUNKS, "respects MAX_CHUNKS");
    }

    #[test]
    fn chunk_text_short_is_one_chunk() {
        let chunks = chunk_text("Just one short sentence here.");
        assert_eq!(chunks.len(), 1);
    }

    #[test]
    fn chunk_text_caps_at_max() {
        // Many sentences, each near CHUNK_CHARS, would exceed
        // MAX_CHUNKS if uncapped.
        let big_sentence = "x ".repeat(800) + "end. "; // ~1605 chars
        let text = big_sentence.repeat(20);
        let chunks = chunk_text(&text);
        assert!(chunks.len() <= MAX_CHUNKS, "got {}", chunks.len());
    }

    #[test]
    fn chunk_prompt_is_terse_and_includes_chunk() {
        let p = build_chunk_prompt("Some passage content.");
        // v0.1.445: knowledge-extraction framing.
        assert!(p.contains("ONE concise line"), "asks for one concise line");
        assert!(p.to_lowercase().contains("command") || p.to_lowercase().contains("fact"),
            "asks for extractable knowledge");
        assert!(p.contains("Some passage content."), "includes the chunk");
    }

    #[test]
    fn combine_prompt_numbers_the_notes() {
        let notes = vec!["First point.".to_string(), "Second point.".to_string()];
        let p = build_combine_prompt(&notes);
        assert!(p.contains("1. First point."));
        assert!(p.contains("2. Second point."));
        assert!(p.to_lowercase().contains("digest"), "asks for a knowledge digest");
    }

    // v0.1.445: directed / "take action" prompt.
    #[test]
    fn directed_prompt_embeds_goal() {
        let p = build_directed_prompt("find the install command", "Setup", "run: cargo install foo");
        assert!(p.contains("find the install command"), "embeds the directive");
        assert!(p.contains("run: cargo install foo"), "includes page text");
        assert!(p.contains("Setup"), "includes title");
    }

    #[test]
    fn directed_prompt_empty_falls_back() {
        // Empty directive → general knowledge extraction (same as
        // build_page_prompt).
        let p = build_directed_prompt("", "T", "body text here");
        let g = build_page_prompt("", "T", "body text here");
        assert_eq!(p, g, "empty directive falls back to page prompt");
    }

    #[test]
    fn join_partials_spaces_and_skips_empty() {
        let parts = vec!["A.".to_string(), "  ".to_string(), "B.".to_string()];
        assert_eq!(join_partials(&parts), "A. B.");
        assert_eq!(join_partials(&[]), "");
    }

    // v0.1.433: cross-chunk dedup.
    #[test]
    fn dedup_collapses_near_duplicates() {
        let s = vec![
            "The borrow checker enforces memory ownership rules at compile time.".to_string(),
            "The borrow checker enforces memory ownership rules during compile time.".to_string(),
            "The async runtime uses a work-stealing scheduler for tasks.".to_string(),
        ];
        let out = dedup_chunk_summaries(&s);
        // The first two are near-identical (cosine ~0.94 > 0.80);
        // one should be dropped. The third is distinct and kept.
        assert!(out.len() < s.len(), "a near-duplicate should be collapsed: {:?}", out);
        assert!(out.iter().any(|x| x.contains("work-stealing")), "distinct kept");
        assert!(out.iter().any(|x| x.to_lowercase().contains("borrow")), "first occurrence kept");
    }

    #[test]
    fn dedup_keeps_distinct_and_order() {
        let s = vec![
            "Apples are red fruit grown in orchards.".to_string(),
            "Quantum computers use qubits for superposition states.".to_string(),
            "Volcanoes erupt molten lava from the mantle.".to_string(),
        ];
        let out = dedup_chunk_summaries(&s);
        assert_eq!(out.len(), 3, "all distinct → none dropped");
        // Order preserved.
        assert!(out[0].contains("Apples"));
        assert!(out[2].contains("Volcanoes"));
    }

    #[test]
    fn dedup_skips_empty() {
        let s = vec!["".to_string(), "  ".to_string(), "Real content sentence here.".to_string()];
        let out = dedup_chunk_summaries(&s);
        assert_eq!(out.len(), 1);
    }

    // v0.1.442: chunk informativeness filter.
    #[test]
    fn informative_keeps_prose() {
        let prose = "Rust guarantees memory safety without a garbage collector by \
                     enforcing ownership rules at compile time through the borrow \
                     checker which tracks references.";
        assert!(chunk_is_informative(prose), "real prose is informative");
    }

    #[test]
    fn informative_drops_nav_boilerplate() {
        let nav = "Home About Contact Login Sign up Privacy Terms Cookies Menu \
                   Search Share Subscribe Read more Click here Learn more See all";
        assert!(!chunk_is_informative(nav), "all-boilerplate chunk dropped");
    }

    #[test]
    fn informative_drops_thin_linkdump() {
        // Passes the ratio test but fails the distinct-content
        // guard (few unique content words).
        let dump = "Home | Page 1 | Page 2 | Page 3 | Next | Prev | Top | Back";
        assert!(!chunk_is_informative(dump), "low-distinct link dump dropped");
    }

    #[test]
    fn informative_empty_is_false() {
        assert!(!chunk_is_informative(""));
        assert!(!chunk_is_informative("   "));
    }

    #[test]
    fn select_keeps_order_and_filters() {
        let chunks = vec![
            "Home Menu Login Search Share".to_string(), // boilerplate
            "The mitochondria is the powerhouse of the cell producing ATP through respiration.".to_string(),
            "Cookies Privacy Terms Accept".to_string(), // boilerplate
        ];
        let kept = select_informative_chunks(chunks);
        assert_eq!(kept.len(), 1, "only the prose chunk kept");
        assert!(kept[0].contains("mitochondria"));
    }

    #[test]
    fn select_keeps_all_if_all_thin() {
        // If everything is borderline, keep the original (better
        // to summarize thin content than nothing).
        let chunks = vec!["Home Menu Login".to_string(), "Search Share".to_string()];
        let kept = select_informative_chunks(chunks.clone());
        assert_eq!(kept.len(), chunks.len(), "fallback keeps all when all thin");
    }
}
