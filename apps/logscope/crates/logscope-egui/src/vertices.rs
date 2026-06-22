//! vertices.rs — types a generated answer into the break points the tree can fork and merge on.
//!
//! Two families, exactly as the design calls for:
//!   - LITERAL breaks (string-exact, cheap, unambiguous): `<think>`/`</think>`, code-fence open/close.
//!     These are where two branches can merge on a matching string — and `ThinkStart` is the signal
//!     to let a fast model run ahead while a reasoning model is still inside its think block.
//!   - REASONING breaks (semantic): a reasoning Step, or a Code item (fn/struct/...) via tree-sitter
//!     (ast.rs). Merging here is by meaning (same step, same item name), not byte-equality.
//!
//! `segment` returns them in document order; `merge_points` aligns two segmentations to the vertices
//! they share — the candidate merge sites.

#![allow(dead_code)]
use crate::ast;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VertexKind {
    ThinkStart,
    ThinkEnd,
    FenceStart, // label = language (may be empty)
    FenceEnd,
    Step,       // label = the step's leading text
    Code,       // label = item name; detail carries the ast Kind
}

impl VertexKind {
    /// Literal vertices match byte-exactly; reasoning vertices match by label/meaning.
    pub fn is_literal(&self) -> bool {
        matches!(self, VertexKind::ThinkStart | VertexKind::ThinkEnd | VertexKind::FenceEnd)
    }
}

#[derive(Debug, Clone)]
pub struct Vertex {
    pub kind: VertexKind,
    pub byte: usize,
    pub line: usize,
    pub label: String,
}

/// Literal + step vertices — pure string work, no tree-sitter (so it's unit-testable on its own).
pub fn literal_step_vertices(text: &str) -> Vec<Vertex> {
    let mut v = Vec::new();
    // <think> / </think> anywhere (inline or own-line)
    for (tag, kind) in [("<think>", VertexKind::ThinkStart), ("</think>", VertexKind::ThinkEnd)] {
        let mut from = 0;
        while let Some(rel) = text[from..].find(tag) {
            let b = from + rel;
            v.push(Vertex { kind: kind.clone(), byte: b, line: line_at(text, b), label: tag.to_string() });
            from = b + tag.len();
        }
    }
    // fences + steps, line by line
    let (mut byte, mut in_fence) = (0usize, false);
    for (ln, raw) in text.lines().enumerate() {
        let trimmed = raw.trim_start();
        if trimmed.starts_with("```") {
            if in_fence {
                v.push(Vertex { kind: VertexKind::FenceEnd, byte, line: ln, label: "```".into() });
                in_fence = false;
            } else {
                let lang = trimmed.trim_start_matches('`').trim().to_string();
                v.push(Vertex { kind: VertexKind::FenceStart, byte, line: ln, label: lang });
                in_fence = true;
            }
        } else if !in_fence && is_step(trimmed) {
            let label: String = trimmed.chars().take(40).collect();
            v.push(Vertex { kind: VertexKind::Step, byte, line: ln, label });
        }
        byte += raw.len() + 1; // +1 for the '\n' lines() stripped
    }
    v.sort_by_key(|x| x.byte);
    v
}

/// Full segmentation: literal/step vertices plus Code items (tree-sitter) inside each ```rust fence.
pub fn segment(text: &str) -> Vec<Vertex> {
    let mut v = literal_step_vertices(text);
    // walk fences; for rust fences, extract ast items and add Code vertices at their offsets
    let bytes = text.as_bytes();
    let mut i = 0;
    while let Some(open) = find_from(text, "```", i) {
        let line_end = text[open..].find('\n').map(|n| open + n + 1).unwrap_or(text.len());
        let lang = text[open + 3..line_end].trim();
        let close = find_from(text, "```", line_end).unwrap_or(text.len());
        if lang.starts_with("rust") || lang.is_empty() {
            let body = &text[line_end..close];
            for it in ast::extract(body) {
                let b = line_end + it.start_byte;
                let _ = bytes; // byte index already in char-safe range (ast spans are byte offsets)
                v.push(Vertex { kind: VertexKind::Code, byte: b, line: line_at(text, b),
                    label: if it.name.is_empty() { format!("{:?}", it.kind) } else { it.name } });
            }
        }
        i = (close + 3).min(text.len());
    }
    v.sort_by_key(|x| x.byte);
    v
}

/// Candidate merge sites between two segmentations: vertices that match. Literal kinds match on
/// (kind, exact label); reasoning kinds match on (kind, label). Greedy in document order, so the
/// result is a monotone alignment (each side consumed once).
pub fn merge_points(a: &[Vertex], b: &[Vertex]) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut bj = 0;
    for (ai, va) in a.iter().enumerate() {
        for (off, vb) in b[bj..].iter().enumerate() {
            if va.kind == vb.kind && va.label == vb.label {
                out.push((ai, bj + off));
                bj += off + 1;
                break;
            }
        }
    }
    out
}

fn is_step(t: &str) -> bool {
    let lower = t.to_ascii_lowercase();
    // numbered ("1." "2)"), explicit ("Step 3"), or discourse markers that open a reasoning move
    let numbered = t.find(['.', ')']).map(|p| p > 0 && p <= 3 && t[..p].chars().all(|c| c.is_ascii_digit())).unwrap_or(false);
    numbered
        || lower.starts_with("step ")
        || ["first,", "then,", "next,", "finally,", "therefore,", "so,", "now,"].iter().any(|m| lower.starts_with(m))
}

fn line_at(text: &str, byte: usize) -> usize { text[..byte.min(text.len())].bytes().filter(|&c| c == b'\n').count() }
fn find_from(text: &str, pat: &str, from: usize) -> Option<usize> { text.get(from..).and_then(|s| s.find(pat)).map(|r| from + r) }
