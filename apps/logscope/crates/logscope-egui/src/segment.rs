//! segment.rs — detect the natural SEAMS in a model's streaming output: the moment it shifts between
//! explaining and coding, or forms a new thought. Pure + deterministic + unit-tested. No model, no GPU.
//!
//! Why seams matter (they're not just pause points):
//!   1. SELF-REVIEW — a code→explain seam is where a unit just finished and the model is about to reason
//!      about it: the natural beat to offer a "look back, still on task?" review (see tripwire.rs).
//!   2. FAN-OUT — an explain→code seam is where a plan turns into execution: branch here and you get K code
//!      attempts from ONE explanation (the coordinator fan-out), each a tree node off the shared prefix.
//!   3. TREE NODES — every seam is a cheap KV checkpoint (extend_prefix off the shared ancestor), so the
//!      conversation tree gains a node exactly where the reasoning changed mode.
//!   4. PREDICTION — expert routing differs between code tokens and prose tokens, so tagging each segment
//!      with its MODE lets the expert-prediction lexical map condition on it (a sharper prefetch signal).
//!
//! On "small model vs main prompt" (the open question): the CHEAP, RELIABLE seams — fence transitions,
//! paragraph shifts after a substantive block, sentence-initial discourse markers — are pure string scans
//! with high precision, so they need no model and belong here. The main prompt can ENCOURAGE the model to
//! self-segment ("finish a thought, then pause"), and a small model is only worth invoking for a genuinely
//! semantic "is this a new thought?" call the string scan can't make — the same division tripwire.rs draws
//! between mechanical (string) and semantic (needs judgment) checks.

/// The kind of seam detected at a point in the stream.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Seam {
    /// Prose → the start of a fenced code block. The plan-to-execution boundary (best fan-out point).
    ExplainToCode,
    /// A fenced code block just closed → prose. A unit finished; the model is about to reason (review beat).
    CodeToExplain,
    /// A new reasoning step within prose: a paragraph break followed by a discourse marker ("Now,",
    /// "Next,", "Looking at", "That suggests"). A new thought is forming.
    NewThought,
}

/// The current generation mode — what kind of tokens the model is emitting right now. Used to tag segments
/// for the expert-prediction map (routing differs by mode) and to know which seam a transition produces.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode {
    Prose,
    Code,
}

/// A detected seam and where it sits in the text (byte offset of the transition point).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Boundary {
    pub kind: Seam,
    pub at: usize,
}

/// The mode the text is in at its END — did it finish inside a code fence or in prose? (An odd number of
/// ``` fences means we're currently inside code.) Drives the mode tag handed to the predictor.
pub fn current_mode(text: &str) -> Mode {
    if text.matches("```").count() % 2 == 1 { Mode::Code } else { Mode::Prose }
}

/// Sentence-initial discourse markers that signal a NEW reasoning step when they open a fresh paragraph.
/// Kept tight for precision: these overwhelmingly introduce a new thought rather than continue one.
const THOUGHT_MARKERS: &[&str] = &[
    "Now", "Next", "Then", "First", "Second", "Third", "Finally", "So ", "Looking at", "Let me",
    "Let's", "That suggests", "That means", "This means", "This suggests", "Given ", "Here", "The issue",
    "The problem", "The root cause", "Notice", "Note that", "Now,", "Next,", "Actually", "Instead",
];

/// All seams in `text`, in order. A seam is emitted at each explain↔code fence transition, and at each
/// new-thought marker that opens a paragraph. Deterministic; safe to call on the growing stream.
pub fn boundaries(text: &str) -> Vec<Boundary> {
    let mut out = Vec::new();
    // 1) fence transitions: every ``` toggles mode. Odd-indexed fences open code, even-indexed close it.
    let mut idx = 0usize;
    let mut fence_no = 0u32;
    while let Some(p) = text[idx..].find("```") {
        let at = idx + p;
        fence_no += 1;
        // opening fence (1st, 3rd, …) = ExplainToCode; closing (2nd, 4th, …) = CodeToExplain
        let kind = if fence_no % 2 == 1 { Seam::ExplainToCode } else { Seam::CodeToExplain };
        out.push(Boundary { kind, at });
        idx = at + 3;
    }
    // 2) new-thought markers: a marker that begins a line AFTER a blank line, and is OUTSIDE any code block.
    for (line_start, line) in line_offsets(text) {
        let t = line.trim_start();
        if t.is_empty() { continue; }
        if in_code_at(text, line_start) { continue; } // markers inside code aren't thoughts
        if !preceded_by_blank(text, line_start) { continue; } // must open a paragraph
        if THOUGHT_MARKERS.iter().any(|m| t.starts_with(m)) {
            out.push(Boundary { kind: Seam::NewThought, at: line_start });
        }
    }
    out.sort_by_key(|b| b.at);
    out
}

/// Streaming helper: the newest seam strictly after byte offset `after` (what the harness watches to fire a
/// pause/branch/checkpoint exactly once per seam). None if no new seam has appeared.
pub fn latest_boundary(text: &str, after: usize) -> Option<Boundary> {
    boundaries(text).into_iter().filter(|b| b.at > after).next_back()
}

/// Whether the byte offset falls inside an open code fence (odd number of ``` before it).
fn in_code_at(text: &str, offset: usize) -> bool {
    text[..offset.min(text.len())].matches("```").count() % 2 == 1
}

/// True if the line starting at `line_start` is preceded by a blank line (paragraph opener).
fn preceded_by_blank(text: &str, line_start: usize) -> bool {
    if line_start == 0 { return true; }
    let before = &text[..line_start];
    // strip the single newline that ends the previous line, then check the line before is blank/absent
    let trimmed = before.strip_suffix('\n').unwrap_or(before);
    match trimmed.rfind('\n') {
        Some(p) => trimmed[p + 1..].trim().is_empty(),
        None => trimmed.trim().is_empty(),
    }
}

/// (byte offset of line start, line text) for each line.
fn line_offsets(text: &str) -> Vec<(usize, &str)> {
    let mut out = Vec::new();
    let mut start = 0usize;
    for (i, c) in text.char_indices() {
        if c == '\n' { out.push((start, &text[start..i])); start = i + 1; }
    }
    if start < text.len() { out.push((start, &text[start..])); }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_explain_to_code_and_back() {
        let text = "Here is the plan.\n\n```rust\nfn main() {}\n```\n\nThat compiles.";
        let bs = boundaries(text);
        assert!(bs.iter().any(|b| b.kind == Seam::ExplainToCode));
        assert!(bs.iter().any(|b| b.kind == Seam::CodeToExplain));
        // order: explain→code precedes code→explain
        let e = bs.iter().position(|b| b.kind == Seam::ExplainToCode).unwrap();
        let c = bs.iter().position(|b| b.kind == Seam::CodeToExplain).unwrap();
        assert!(e < c);
    }

    #[test]
    fn current_mode_tracks_open_fence() {
        assert_eq!(current_mode("prose ```rust\ncode"), Mode::Code); // one fence → inside code
        assert_eq!(current_mode("```\ncode\n```\nprose"), Mode::Prose); // balanced → prose
        assert_eq!(current_mode("just prose"), Mode::Prose);
    }

    #[test]
    fn new_thought_marker_after_blank_line() {
        let text = "The greeting is done.\n\nLooking at the log, there are repeated errors.";
        let bs = boundaries(text);
        assert!(bs.iter().any(|b| b.kind == Seam::NewThought), "‘Looking at’ opens a new thought: {bs:?}");
    }

    #[test]
    fn marker_mid_paragraph_is_not_a_new_thought() {
        // "So" appears but NOT opening a paragraph (no preceding blank line) → not a seam
        let text = "This works and so we can continue with the plan.";
        let bs = boundaries(text);
        assert!(!bs.iter().any(|b| b.kind == Seam::NewThought));
    }

    #[test]
    fn markers_inside_code_are_ignored() {
        // "Now" inside a code comment must not be a NewThought seam
        let text = "```rust\n// Now compute the sum\nlet x = 1;\n```";
        let bs = boundaries(text);
        assert!(!bs.iter().any(|b| b.kind == Seam::NewThought));
    }

    #[test]
    fn latest_boundary_is_the_freshest_past_cursor() {
        let text = "Plan.\n\n```rust\nfn f() {}\n```\n\nNow the next part.";
        let all = boundaries(text);
        let last_at = all.iter().map(|b| b.at).max().unwrap();
        let latest = latest_boundary(text, 0).unwrap();
        assert_eq!(latest.at, last_at);
        // with the cursor past everything, no new seam
        assert!(latest_boundary(text, text.len()).is_none());
    }

    #[test]
    fn the_greeting_then_analysis_example() {
        // the exact shape from the feedback: greeting, blank line, then the model shifts into analysis
        let text = "Hello! I'm Logscope. How can I assist?\n\nLooking at the log sample: there are repeated /health requests.";
        let bs = boundaries(text);
        // the shift into analysis is a NewThought seam — a good place to checkpoint / branch
        assert!(bs.iter().any(|b| b.kind == Seam::NewThought));
    }
}
