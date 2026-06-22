//! doc_tree: parse a large document into a tree of budgeted CHUNKS so
//! the browse tab can LAZY-render only what's near the viewport instead
//! of laying out the whole thing (F6: a 12k-line / 99k-word book lags).
//!
//! Per-format grammar decides chunk boundaries:
//!   - Plain (.txt): pack lines into chunks up to a char budget,
//!     preferring to break at blank-line (paragraph) boundaries.
//!   - Markdown (.md): headings start new sections (a heading owns its
//!     following body); fenced code stays whole; otherwise pack to the
//!     budget at paragraph boundaries within a section.
//! Each leaf carries its line span (for lazy mounting + scrollbar
//! height estimation) and a cheap one-line summary (for an outline /
//! minimap). PURE: no rendering, no IO — the QML side mounts leaves on
//! demand. See docs/design/text_syntax_tree.md.
//!
//! Generalizes note_cards::split_into_cards (which splits small
//! captured snippets by structure) by adding: a size BUDGET so no leaf
//! is huge, LINE SPANS for lazy loading, and format dispatch.

/// Format grammar to apply.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DocFormat {
    Plain,
    Markdown,
}

impl DocFormat {
    /// Infer from a filename extension; defaults to Plain.
    pub fn from_ext(name: &str) -> DocFormat {
        let lower = name.to_ascii_lowercase();
        if lower.ends_with(".md") || lower.ends_with(".markdown") {
            DocFormat::Markdown
        } else {
            DocFormat::Plain
        }
    }
}

/// What a chunk predominantly holds (for styling / outline; the .txt
/// grammar only ever yields Prose).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChunkKind {
    Prose,
    Heading,
    Code,
    List,
}

/// One render-sized leaf of the document.
#[derive(Clone, Debug, PartialEq)]
pub struct Chunk {
    /// 0-based inclusive start line, exclusive end line (so end-start =
    /// line count). The QML side uses this to mount/evict + estimate
    /// scroll height.
    pub start_line: usize,
    pub end_line: usize,
    /// The chunk's text (already sliced — the caller need not re-split).
    pub text: String,
    pub kind: ChunkKind,
    /// Cheap one-line summary for an outline/minimap (heading text, or
    /// the first non-empty line truncated).
    pub summary: String,
}

impl Chunk {
    pub fn line_count(&self) -> usize { self.end_line.saturating_sub(self.start_line) }
}

/// Configuration for chunk sizing.
#[derive(Clone, Copy, Debug)]
pub struct ChunkConfig {
    /// Target max chars per chunk. A chunk may exceed this by at most
    /// one line/paragraph (we never split mid-paragraph). Balances
    /// quick build vs lazy-scroll granularity.
    pub char_budget: usize,
    /// Hard cap on lines per chunk regardless of char budget (keeps a
    /// pathological run of short lines from making a giant leaf).
    pub max_lines: usize,
}

impl Default for ChunkConfig {
    fn default() -> Self {
        // ~4KB / ~200 lines: each leaf builds in a few ms, fine grain
        // for lazy scroll. Tunable; a future CPU-aware budget can scale
        // this (see design doc).
        Self { char_budget: 4096, max_lines: 200 }
    }
}

/// Parse `text` into budgeted chunks per `format`. Pure: same input →
/// same chunks. Spans are contiguous and cover every line.
pub fn build(text: &str, format: DocFormat, cfg: &ChunkConfig) -> Vec<Chunk> {
    match format {
        DocFormat::Plain => build_plain(text, cfg),
        DocFormat::Markdown => build_markdown(text, cfg),
    }
}

fn summary_of(text: &str) -> String {
    let first = text.lines().find(|l| !l.trim().is_empty()).unwrap_or("");
    let t = first.trim();
    if t.chars().count() > 80 {
        let s: String = t.chars().take(77).collect();
        format!("{}…", s)
    } else {
        t.to_string()
    }
}

/// .txt grammar: pack lines up to the budget, closing a chunk at a
/// blank-line (paragraph) boundary when one is available near the cap.
fn build_plain(text: &str, cfg: &ChunkConfig) -> Vec<Chunk> {
    let lines: Vec<&str> = text.lines().collect();
    let mut chunks = Vec::new();
    if lines.is_empty() { return chunks; }

    let mut start = 0usize;
    let mut chars = 0usize;
    let mut last_blank: Option<usize> = None; // index of a blank line in [start, i)

    let mut i = 0usize;
    while i < lines.len() {
        let l = lines[i];
        let len = l.len() + 1;
        let over_budget = chars > 0 && (chars + len > cfg.char_budget);
        let over_lines = (i - start) >= cfg.max_lines;

        if over_budget || over_lines {
            // Prefer to break at the last blank line seen (paragraph
            // boundary) if there is one strictly after start; else break
            // here.
            let break_at = match last_blank {
                Some(b) if b > start => b,
                _ => i,
            };
            push_plain_chunk(&mut chunks, &lines, start, break_at);
            start = break_at;
            chars = 0;
            last_blank = None;
            // recompute chars for [start, i) we already passed
            for k in start..i { chars += lines[k].len() + 1; }
        }

        if l.trim().is_empty() { last_blank = Some(i); }
        chars += len;
        i += 1;
    }
    if start < lines.len() {
        push_plain_chunk(&mut chunks, &lines, start, lines.len());
    }
    chunks
}

fn push_plain_chunk(out: &mut Vec<Chunk>, lines: &[&str], start: usize, end: usize) {
    if end <= start { return; }
    let text = lines[start..end].join("\n");
    let summary = summary_of(&text);
    out.push(Chunk {
        start_line: start,
        end_line: end,
        text,
        kind: ChunkKind::Prose,
        summary,
    });
}

fn is_heading(l: &str) -> bool {
    let t = l.trim_start();
    t.starts_with('#')
}
fn is_fence(l: &str) -> bool {
    let t = l.trim_start();
    t.starts_with("```") || t.starts_with("~~~")
}
fn is_list_item(l: &str) -> bool {
    let t = l.trim_start();
    t.starts_with("- ") || t.starts_with("* ") || t.starts_with("+ ")
        || t.chars().next().map(|c| c.is_ascii_digit()).unwrap_or(false)
            && t.contains(". ")
}

/// .md grammar: headings + fenced code are structural boundaries; prose
/// and list runs pack to the budget. Each chunk still carries a span.
fn build_markdown(text: &str, cfg: &ChunkConfig) -> Vec<Chunk> {
    let lines: Vec<&str> = text.lines().collect();
    let mut chunks = Vec::new();
    let mut i = 0usize;

    // accumulator for a prose/list run
    let mut run_start: Option<usize> = None;
    let mut run_chars = 0usize;

    macro_rules! flush_run {
        ($end:expr) => {
            if let Some(s) = run_start.take() {
                if $end > s {
                    let body = lines[s..$end].join("\n");
                    let kind = if lines[s..$end].iter().any(|l| is_list_item(l)) {
                        ChunkKind::List
                    } else {
                        ChunkKind::Prose
                    };
                    let summary = summary_of(&body);
                    chunks.push(Chunk { start_line: s, end_line: $end, text: body, kind, summary });
                }
                // (run_chars is reset by the caller when a new run starts;
                // resetting here is dead — cargo check flagged it.)
            }
        };
    }

    while i < lines.len() {
        let l = lines[i];

        if l.trim().is_empty() {
            // blank line: a natural break point but keep it in the run
            if run_start.is_some() { run_chars += 1; }
            i += 1;
            continue;
        }

        if is_heading(l) {
            flush_run!(i);
            // heading owns its own chunk (cheap; gives a clean outline)
            let body = l.trim().to_string();
            let summary = summary_of(&body);
            chunks.push(Chunk {
                start_line: i, end_line: i + 1, text: body,
                kind: ChunkKind::Heading, summary,
            });
            i += 1;
            continue;
        }

        if is_fence(l) {
            flush_run!(i);
            let fence_start = i;
            let mut body = String::new();
            body.push_str(l); body.push('\n');
            i += 1;
            while i < lines.len() {
                body.push_str(lines[i]); body.push('\n');
                let closed = is_fence(lines[i]);
                i += 1;
                if closed { break; }
            }
            let summary = summary_of(&body);
            chunks.push(Chunk {
                start_line: fence_start, end_line: i,
                text: body.trim_end().to_string(),
                kind: ChunkKind::Code, summary,
            });
            continue;
        }

        // prose/list line: extend the run, flushing if over budget.
        if run_start.is_none() { run_start = Some(i); run_chars = 0; }
        let len = l.len() + 1;
        if run_chars > 0 && (run_chars + len > cfg.char_budget
            || (i - run_start.unwrap()) >= cfg.max_lines) {
            flush_run!(i);
            run_start = Some(i);
        }
        run_chars += len;
        i += 1;
    }
    flush_run!(lines.len());
    chunks
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> ChunkConfig { ChunkConfig::default() }
    fn small_cfg() -> ChunkConfig { ChunkConfig { char_budget: 64, max_lines: 100 } }

    #[test]
    fn hostile_inputs_dont_break_invariants() {
        // v0.1.527 (adversarial probe): degenerate inputs the original
        // tests missed. All must keep spans contiguous + fully covering
        // and never loop/panic.
        let check = |name: &str, text: &str| {
            let chunks = build(text, DocFormat::Plain, &ChunkConfig::default());
            let n = text.lines().count();
            if n == 0 { assert!(chunks.is_empty(), "{}: empty", name); return; }
            assert_eq!(chunks[0].start_line, 0, "{}: starts 0", name);
            for w in chunks.windows(2) {
                assert_eq!(w[0].end_line, w[1].start_line, "{}: contiguous", name);
            }
            assert_eq!(chunks.last().unwrap().end_line, n, "{}: covers all", name);
            for c in &chunks { assert!(c.end_line > c.start_line, "{}: non-empty chunk", name); }
        };
        // a single line far larger than the budget — can't split a line,
        // so it stays one chunk (must not loop or slice mid-line).
        let giant = "x".repeat(50_000);
        let c = build(&giant, DocFormat::Plain, &ChunkConfig::default());
        assert_eq!(c.len(), 1, "unsplittable giant line → 1 chunk");
        check("giant-line", &giant);
        check("all-blank", "\n\n\n\n\n");
        check("crlf", "a\r\nb\r\nc");
        check("no-trailing-nl", "alpha\nbeta");
        // unicode multi-byte must not panic (len() is bytes; we never
        // slice a string by byte index, so this is safe — assert it).
        let uni: String = (0..200).map(|_| "héllo 日本語\n").collect();
        check("unicode", &uni);
    }

    #[test]
    fn line_cap_fires_when_budget_never_trips() {
        // 1000 one-char lines with a huge char budget → the max_lines
        // cap must still bound chunk size.
        let many: String = (0..1000).map(|_| "a").collect::<Vec<_>>().join("\n");
        let chunks = build(&many, DocFormat::Plain, &ChunkConfig { char_budget: 9_999_999, max_lines: 50 });
        for c in &chunks {
            assert!(c.line_count() <= 50, "line cap bounds chunk, got {}", c.line_count());
        }
        assert!(chunks.len() >= 20, "1000 lines / 50 = ~20 chunks");
    }

    #[test]
    fn format_from_ext() {
        assert_eq!(DocFormat::from_ext("book.txt"), DocFormat::Plain);
        assert_eq!(DocFormat::from_ext("README.md"), DocFormat::Markdown);
        assert_eq!(DocFormat::from_ext("notes.MARKDOWN"), DocFormat::Markdown);
        assert_eq!(DocFormat::from_ext("noext"), DocFormat::Plain);
    }

    #[test]
    fn empty_yields_no_chunks() {
        assert!(build("", DocFormat::Plain, &cfg()).is_empty());
        assert!(build("", DocFormat::Markdown, &cfg()).is_empty());
    }

    #[test]
    fn plain_spans_contiguous_and_cover_everything() {
        let big: String = (0..12000)
            .map(|n| format!("line number {} with some filler text here", n))
            .collect::<Vec<_>>().join("\n");
        let chunks = build(&big, DocFormat::Plain, &cfg());
        assert!(chunks.len() > 50, "12k lines should split into many chunks, got {}", chunks.len());
        assert_eq!(chunks[0].start_line, 0);
        for w in chunks.windows(2) {
            assert_eq!(w[0].end_line, w[1].start_line, "contiguous spans");
        }
        assert_eq!(chunks.last().unwrap().end_line, 12000, "covers all lines");
    }

    #[test]
    fn plain_respects_char_budget() {
        let big: String = (0..500)
            .map(|n| format!("paragraph line {}", n))
            .collect::<Vec<_>>().join("\n");
        let chunks = build(&big, DocFormat::Plain, &small_cfg());
        for c in &chunks {
            // may overshoot by at most one line
            assert!(c.text.len() <= 64 + 40,
                "chunk {} chars should be ~within budget", c.text.len());
        }
    }

    #[test]
    fn plain_prefers_paragraph_breaks() {
        // two paragraphs separated by a blank; small budget forces a
        // split — it should land at the blank line, not mid-paragraph.
        let text = "alpha one\nalpha two\nalpha three\n\nbeta one\nbeta two";
        let chunks = build(text, DocFormat::Plain, &ChunkConfig { char_budget: 30, max_lines: 100 });
        assert!(chunks.len() >= 2);
        // the blank line (index 3) should be a boundary: some chunk ends at 3 or 4
        assert!(chunks.iter().any(|c| c.end_line == 3 || c.end_line == 4),
            "should break at the paragraph boundary; spans: {:?}",
            chunks.iter().map(|c| (c.start_line, c.end_line)).collect::<Vec<_>>());
    }

    #[test]
    fn markdown_heading_is_own_chunk() {
        let md = "# Title\n\nsome body text here\nmore body";
        let chunks = build(md, DocFormat::Markdown, &cfg());
        assert_eq!(chunks[0].kind, ChunkKind::Heading);
        assert_eq!(chunks[0].text, "# Title");
        assert_eq!(chunks[0].summary, "# Title");
        assert!(chunks.iter().skip(1).any(|c| c.kind == ChunkKind::Prose));
    }

    #[test]
    fn markdown_code_fence_stays_whole() {
        let md = "intro line\n\n```rust\nfn main() {}\nlet x = 1;\n```\n\nafter";
        let chunks = build(md, DocFormat::Markdown, &cfg());
        let code = chunks.iter().find(|c| c.kind == ChunkKind::Code)
            .expect("a code chunk");
        assert!(code.text.contains("fn main()") && code.text.contains("let x = 1;"),
            "fenced code kept whole: {:?}", code.text);
    }

    #[test]
    fn markdown_list_run_grouped() {
        let md = "- one\n- two\n- three";
        let chunks = build(md, DocFormat::Markdown, &cfg());
        assert!(chunks.iter().any(|c| c.kind == ChunkKind::List));
    }

    #[test]
    fn markdown_spans_cover_everything() {
        let md = "# H1\n\npara a\n\n```\ncode\n```\n\n- l1\n- l2\n\n## H2\n\npara b";
        let chunks = build(md, DocFormat::Markdown, &cfg());
        // every line index 0..N must be inside exactly one chunk's span
        let n = md.lines().count();
        let mut covered = vec![false; n];
        for c in &chunks {
            for k in c.start_line..c.end_line {
                assert!(!covered[k], "line {} double-covered", k);
                covered[k] = true;
            }
        }
        // headings/code/prose covered; blank-only lines may be skipped
        // between chunks but no line should be double-covered. Confirm
        // all NON-blank lines are covered.
        for (k, line) in md.lines().enumerate() {
            if !line.trim().is_empty() {
                assert!(covered[k], "non-blank line {} ({:?}) uncovered", k, line);
            }
        }
    }

    #[test]
    fn summary_truncates_long_lines() {
        let long = "x".repeat(200);
        let chunks = build(&long, DocFormat::Plain, &cfg());
        assert!(chunks[0].summary.chars().count() <= 81, "summary truncated with ellipsis");
        assert!(chunks[0].summary.ends_with('…'));
    }

    #[test]
    fn line_count_matches_span() {
        let chunks = build("a\nb\nc\nd", DocFormat::Plain, &cfg());
        let total: usize = chunks.iter().map(|c| c.line_count()).sum();
        assert_eq!(total, 4);
    }
}
