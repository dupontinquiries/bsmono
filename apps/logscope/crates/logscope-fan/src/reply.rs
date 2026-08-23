//! reply — one correct scan of a model reply's fenced blocks.
//!
//! ## Why a shared primitive
//!
//! Seven files in this workspace look for triple backticks, using three
//! different strategies, and three bugs were found in one afternoon by reading
//! them against each other:
//!
//! * `verdict::extract_code` took the NEXT fence and joined every block, so a
//!   reply that showed its own output failed to compile.
//! * `workspace_exec::parse_plan` took the FIRST fence and lost every step
//!   after a quoted snippet.
//! * `stepwise::extract_files` and `workspace_exec::parse_tools` are correct,
//!   by bounding the region first and by balancing braces — two right answers
//!   that neither of the other sites could reuse.
//!
//! Every one is a parser meeting content that contains its own delimiter.
//!
//! ## The rule all four miss
//!
//! CommonMark closes a fence with a run of backticks **at least as long as the
//! opener**. So a four-backtick block legitimately contains three-backtick
//! blocks, which is precisely how a model quotes markdown that itself contains
//! code — and it is the shape all four sites get wrong, because they search for
//! the literal three.
//!
//! [`fences`] implements the run-length rule. It does not decide what any
//! consumer wants: `verdict` wants source blocks, `stepwise` wants blocks under
//! a header, `parse_tools` wants brace-balanced JSON. Those are policies over a
//! correct scan, and mixing the scan into each policy is how they drifted apart.
//!
//! Pure, std-only, no allocation beyond the returned spans.

/// One fenced block, as it appears in the reply.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fence<'a> {
    /// The info string after the opening run — `rust`, `toml`, or empty.
    pub tag: &'a str,
    /// The content between the fences, without either fence line.
    pub body: &'a str,
    /// How many backticks opened it. A consumer that re-emits a block needs
    /// this: closing a 4-run block with 3 backticks produces different markup.
    pub ticks: usize,
    /// Byte offset of the opening backtick run in the original text.
    pub start: usize,
    /// Byte offset one past the closing run, or the end of input for an
    /// unterminated block.
    pub end: usize,
}

impl Fence<'_> {
    /// Whether the tag names something that is source rather than output.
    ///
    /// An empty tag is NOT source. That is the call that fixed the compile
    /// oracle: the commonest reply shape is code in a tagged fence followed by
    /// an untagged fence showing what it prints, and treating the second as
    /// code is why a correct answer was graded a failure.
    pub fn is_source(&self) -> bool {
        let t = self.tag.trim().to_ascii_lowercase();
        if t.is_empty() { return false }
        !matches!(t.as_str(),
            "text" | "output" | "console" | "sh" | "bash" | "shell" | "log" | "json"
            | "toml" | "yaml" | "yml" | "diff" | "md" | "markdown" | "plain" | "none")
    }
}

/// Scan the TOP-LEVEL fenced blocks of `text`.
///
/// A block opened with N backticks ends at the next line whose first
/// non-whitespace is a run of N or more backticks. Blocks nested inside are
/// part of the body and are not returned separately — which is the whole point:
/// a consumer asking for "the blocks in this reply" means the ones the author
/// delimited, not every backtick run in their content.
///
/// An unterminated block runs to the end of input rather than being dropped. A
/// truncated reply is common and its partial block is usually the thing the
/// caller wants; dropping it silently loses the answer.
pub fn fences(text: &str) -> Vec<Fence<'_>> {
    let mut out = Vec::new();
    let mut pos = 0usize;
    let bytes = text.as_bytes();

    while pos < text.len() {
        // Find a line-initial backtick run.
        let Some(open) = find_fence_line(text, pos) else { break };
        let ticks = run_len(bytes, open);
        // Info string: rest of the opening line.
        let after = open + ticks;
        let line_end = text[after..].find('\n').map(|i| after + i).unwrap_or(text.len());
        let tag = &text[after..line_end];
        let body_start = (line_end + 1).min(text.len());

        // Closing run of >= ticks, at the start of a line.
        let mut scan = body_start;
        let mut close: Option<usize> = None;
        while scan < text.len() {
            let Some(c) = find_fence_line(text, scan) else { break };
            if run_len(bytes, c) >= ticks { close = Some(c); break }
            // A shorter run is content — skip past its line.
            scan = text[c..].find('\n').map(|i| c + i + 1).unwrap_or(text.len());
        }

        match close {
            Some(c) => {
                let end = c + run_len(bytes, c);
                out.push(Fence { tag, body: &text[body_start..c], ticks, start: open, end });
                pos = text[end..].find('\n').map(|i| end + i + 1).unwrap_or(text.len());
            }
            None => {
                out.push(Fence { tag, body: &text[body_start..], ticks, start: open, end: text.len() });
                break;
            }
        }
    }
    out
}

/// Offset of the next line-initial backtick run at or after `from`.
fn find_fence_line(text: &str, from: usize) -> Option<usize> {
    let b = text.as_bytes();
    let mut i = from;
    // Only positions that begin a line qualify; leading spaces are allowed.
    while i < b.len() {
        let line_start = i;
        let mut j = line_start;
        while j < b.len() && (b[j] == b' ' || b[j] == b'\t') { j += 1 }
        if j < b.len() && b[j] == b'`' { return Some(j) }
        match text[line_start..].find('\n') {
            Some(k) => i = line_start + k + 1,
            None => return None,
        }
    }
    None
}

fn run_len(b: &[u8], at: usize) -> usize {
    let mut n = 0;
    while at + n < b.len() && b[at + n] == b'`' { n += 1 }
    n
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_plain_block_yields_its_tag_and_body() {
        let f = fences("```rust\nfn a() {}\n```\n");
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].tag, "rust");
        assert_eq!(f[0].body, "fn a() {}\n");
        assert_eq!(f[0].ticks, 3);
    }

    /// THE RULE ALL FOUR EXISTING PARSERS MISS.
    #[test]
    fn a_four_tick_block_contains_three_tick_blocks() {
        let t = "````md\nHere is code:\n```rust\nfn a() {}\n```\ndone\n````\n";
        let f = fences(t);
        assert_eq!(f.len(), 1, "the inner block is CONTENT, not a sibling: {f:#?}");
        assert_eq!(f[0].tag, "md");
        assert!(f[0].body.contains("```rust"), "the inner fence must survive in the body");
        assert!(f[0].body.contains("done"), "content after the inner block was lost");
    }

    #[test]
    fn two_sibling_blocks_are_two_fences() {
        let f = fences("```rust\nfn a() {}\n```\nprose\n```\noutput\n```\n");
        assert_eq!(f.len(), 2);
        assert_eq!(f[0].tag, "rust");
        assert_eq!(f[1].tag, "", "an untagged output block keeps its empty tag");
        assert!(f[0].is_source() && !f[1].is_source());
    }

    /// The bug that started this: the untagged block is not source.
    #[test]
    fn an_untagged_block_is_not_source() {
        let f = fences("```\n1\n```\n");
        assert!(!f[0].is_source(),
                "an untagged fence is what the model is SHOWING, not what it wrote");
    }

    #[test]
    fn an_unterminated_block_runs_to_the_end_rather_than_vanishing() {
        let f = fences("```rust\nfn a() {}\n");
        assert_eq!(f.len(), 1, "a truncated reply's partial block is usually the answer");
        assert_eq!(f[0].body, "fn a() {}\n");
        assert_eq!(f[0].end, "```rust\nfn a() {}\n".len());
    }

    #[test]
    fn an_indented_fence_is_still_a_fence() {
        let f = fences("  ```rust\n  fn a() {}\n  ```\n");
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].tag, "rust");
    }

    #[test]
    fn backticks_inside_a_line_are_not_a_fence() {
        // Inline code must not open a block, and a run that is not line-initial
        // is inline however long it is.
        //
        // My first version of this test asserted the trailing "```rust" opened
        // a rust block. It does not, and should not: it sits mid-line after
        // prose, so CommonMark reads it as inline code. What DOES open a block
        // is the standalone "```" on the following line, with an empty tag —
        // the test expectation was wrong, not the scanner.
        let t = "call `foo()` and then ```rust\nfn a() {}\n```\n";
        let f = fences(t);
        assert_eq!(f.len(), 1, "only line-initial runs open a block: {f:#?}");
        assert_eq!(f[0].tag, "", "the line-initial ``` opens an untagged block");
        assert!(!f[0].is_source(), "and an untagged block is not source");

        // The unambiguous form, for contrast.
        let g = fences("prose\n```rust\nfn a() {}\n```\n");
        assert_eq!(g[0].tag, "rust");
    }

    #[test]
    fn spans_point_back_into_the_original_text() {
        let t = "before\n```rust\nx\n```\nafter\n";
        let f = fences(t);
        assert_eq!(&t[f[0].start..f[0].end], "```rust\nx\n```");
    }

    #[test]
    fn no_fences_yields_nothing_rather_than_one_empty_block() {
        assert!(fences("just prose\n").is_empty());
        assert!(fences("").is_empty());
    }

    /// The three real-world shapes that produced three separate bugs, all
    /// scanned correctly by one function.
    #[test]
    fn the_three_shapes_that_each_broke_a_different_parser() {
        // 1. verdict: code then output — two blocks, one source.
        let a = fences("```rust\nfn f() {}\n```\nprints:\n```\n1\n```\n");
        assert_eq!(a.iter().filter(|f| f.is_source()).count(), 1);

        // 2. parse_plan: a plan quoting code — one block, snippet inside.
        let b = fences("````plan\n1. add:\n```rust\nfn h() {}\n```\n2. wire\n````\n");
        assert_eq!(b.len(), 1);
        assert!(b[0].body.contains("2. wire"), "steps after the snippet survive");

        // 3. parse_tools: JSON whose string contains a fence.
        let c = fences("````tool\n{ \"contents\": \"```rust\\nx\\n```\" }\n````\n");
        assert_eq!(c.len(), 1);
        assert!(c[0].body.contains("contents"));
    }
}
