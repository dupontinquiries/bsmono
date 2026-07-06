#![allow(dead_code)]
//! shorten.rs — log line compressor
//!
//! Detects repetitive structural patterns across consecutive lines and
//! replaces redundant prefixes with `^^type` notation.
//!
//! Example input:
//!   struct drm_info_node *node = (struct drm_info_node *) m->private;
//!   struct drm_minor *minor = node->minor;
//!   struct drm_device *dev = minor->dev;
//!   struct drm_master *master;
//!
//! Example output:
//!   struct drm_info_node *node = (struct drm_info_node *) m->private;
//!   ^^minor *minor = node->minor;
//!   ^^device *dev = minor->dev;
//!   ^^master *master;


/// Configuration for the shortener.
#[derive(Clone)]
pub struct ShortenConfig {
    /// Minimum number of chars a shared prefix must have to be collapsed.
    pub min_prefix_len: usize,
    /// Only collapse prefixes that appear this many times in a row.
    pub min_repeat_count: usize,
    /// Maximum look-back window for finding repeated prefixes.
    pub window: usize,
}

impl Default for ShortenConfig {
    fn default() -> Self {
        ShortenConfig { min_prefix_len: 6, min_repeat_count: 2, window: 20 }
    }
}

/// Result of shortening a set of lines.
pub struct ShortenResult {
    pub lines:       Vec<String>,
    /// Map from output line index → original line index
    pub line_map:    Vec<usize>,
    pub stats:       ShortenStats,
}

#[derive(Default, Clone)]
pub struct ShortenStats {
    pub original_lines:  usize,
    pub output_lines:    usize,
    pub chars_removed:   usize,
}

/// Shorten a slice of log lines.
pub fn shorten(lines: &[&str], cfg: &ShortenConfig) -> ShortenResult {
    let mut out: Vec<String> = Vec::with_capacity(lines.len());
    let mut map: Vec<usize>  = Vec::with_capacity(lines.len());
    let mut stats = ShortenStats { original_lines: lines.len(), ..Default::default() };

    // Collapse the *longest prefix a line shares with the previous line* (snapped to a word
    // boundary): the shared part is the repeated structure, and whatever follows is the part that
    // *changed* — which is what you're actually scanning a log for. No prefix table / hashing:
    // just a byte-level common-prefix check against the previous line (cheaper than the old
    // per-line String key, and correct for arbitrary repeated structure, not just 2 tokens).
    let mut prev: Option<&str> = None;
    let mut run_len = 0usize;

    for (orig_idx, &line) in lines.iter().enumerate() {
        let shared = prev.map(|p| shared_prefix_len(p, line)).unwrap_or(0);

        // a run is >= 2 consecutive lines sharing a long-enough prefix; the first line of a run is
        // kept in full as its "header", the rest collapse.
        if shared >= cfg.min_prefix_len {
            run_len += 1;
        } else {
            run_len = 1;
        }
        let collapse = shared >= cfg.min_prefix_len && run_len >= cfg.min_repeat_count;

        let output_line = if collapse {
            let indent = leading_whitespace(line);
            stats.chars_removed += shared.saturating_sub(indent.len() + 2); // replaced by indent + ^^
            format!("{}^^{}", indent, &line[shared..])
        } else {
            line.to_owned()
        };
        out.push(output_line);
        map.push(orig_idx);
        prev = Some(line);
    }

    stats.output_lines = out.len();
    ShortenResult { lines: out, line_map: map, stats }
}

/// Longest common prefix of `a` and `b`, in bytes, snapped back to the last word boundary
/// (whitespace) so the collapse cuts *between whole tokens* rather than mid-word — that keeps the
/// changing values intact and readable. Returns 0 when no whole leading word is shared.
fn shared_prefix_len(a: &str, b: &str) -> usize {
    let raw = a.bytes().zip(b.bytes()).take_while(|(x, y)| x == y).count();
    // `raw` is a valid byte index in `a` (it's a common-prefix length), and rfind on the ASCII
    // whitespace keeps us on a char boundary.
    match a[..raw].rfind(|c: char| c.is_whitespace()) {
        Some(pos) => pos + 1, // include the boundary whitespace in the shared prefix
        None => 0,
    }
}

fn leading_whitespace(line: &str) -> &str {
    let trimmed = line.trim_start();
    &line[..line.len() - trimmed.len()]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_drm_example() {
        let lines = vec![
            "\tstruct drm_info_node *node = (struct drm_info_node *) m->private;",
            "\tstruct drm_minor *minor = node->minor;",
            "\tstruct drm_device *dev = minor->dev;",
            "\tstruct drm_master *master;",
        ];
        let cfg = ShortenConfig { min_repeat_count: 2, ..Default::default() };
        let result = shorten(&lines, &cfg);
        assert_eq!(result.lines[0], lines[0]); // first not shortened
        assert!(result.lines[1].contains("^^"), "line 1 should be shortened: {}", result.lines[1]);
        assert!(result.lines[2].contains("^^"), "line 2 should be shortened: {}", result.lines[2]);
    }

    #[test]
    fn collapses_shared_structure_and_surfaces_the_change() {
        // the whole point: fold the repeated structure, leave the changing value whole + visible
        let lines = vec![
            "INFO worker started id=1",
            "INFO worker started id=2",
            "INFO worker started id=3",
        ];
        let cfg = ShortenConfig { min_repeat_count: 2, ..Default::default() };
        let r = shorten(&lines, &cfg);
        assert_eq!(r.lines[0], lines[0], "run header shown in full");
        assert!(r.lines[1].contains("^^"));
        assert!(r.lines[1].ends_with("id=2"), "changing value stays whole: {}", r.lines[1]);
        assert!(!r.lines[1].contains("worker"), "repeated structure collapsed away");
    }

    #[test]
    fn unrelated_lines_are_left_alone() {
        let lines = vec!["alpha one two three", "beta four five six"];
        let cfg = ShortenConfig { min_repeat_count: 2, ..Default::default() };
        let r = shorten(&lines, &cfg);
        assert_eq!(r.lines[0], lines[0]);
        assert_eq!(r.lines[1], lines[1], "no shared prefix, nothing collapsed");
    }
}

// ── blockout ──────────────────────────────────────────────────────────────────
//
// Compresses repetitive line *structure* by replacing the repeated non-unique
// prefix tokens with "XX", keeping only the changing part visible.
//
// Example:
//   if lower.contains("tiny") ...     → kept
//   if lower.contains("mini") ...     → XX("mini") ...
//   if lower.contains("phi3") ...     → XX("phi3") ...
//
// Unlike `shorten` (which uses `^^`), blockout replaces within the line so
// the line count stays the same. It's better for code blocks where structure
// matters more than type names.

/// Blockout a slice of lines. Returns the same number of lines with
/// repeated leading structure replaced by "XX".
pub fn blockout(lines: &[&str]) -> Vec<String> {
    if lines.len() < 2 { return lines.iter().map(|s| s.to_string()).collect(); }

    let mut out: Vec<String> = Vec::with_capacity(lines.len());
    let mut prev_prefix: Option<String> = None;
    let mut prefix_count = 0usize;

    for &line in lines {
        let trimmed = line.trim_start();
        let indent  = &line[..line.len() - trimmed.len()];

        // Extract the first N structural tokens (keywords + operators, not literals)
        let structural = structural_prefix(trimmed, 3);

        let same = prev_prefix.as_deref() == Some(&structural)
            && !structural.is_empty()
            && structural.len() >= 4;

        if same && prefix_count >= 1 {
            // Replace the repeated structural prefix with XX
            let rest = trimmed[structural.len()..].trim_start();
            out.push(format!("{}XX{}", indent, rest));
        } else {
            out.push(line.to_string());
            prefix_count = 0;
        }

        if same { prefix_count += 1; } else { prefix_count = 1; }
        prev_prefix = Some(structural);
    }
    out
}

/// Extract the structural (non-literal) prefix: keywords and operators,
/// stopping at the first string/number literal or identifier that changes.
fn structural_prefix(line: &str, max_tokens: usize) -> String {
    let mut out = String::new();
    let mut count = 0;
    let chars: Vec<char> = line.chars().collect();
    let n = chars.len();
    let mut i = 0;

    while i < n && count < max_tokens {
        let ch = chars[i];

        // Skip whitespace (preserve in output as-is)
        if ch.is_whitespace() {
            out.push(ch);
            i += 1;
            continue;
        }

        // String literal → stop here (this is the changing part)
        if ch == '"' || ch == '\'' {
            break;
        }

        // Number literal → stop
        if ch.is_ascii_digit() { break; }

        // Word token
        if ch.is_alphabetic() || ch == '_' {
            let start = i;
            while i < n && (chars[i].is_alphanumeric() || chars[i] == '_') { i += 1; }
            let word: String = chars[start..i].iter().collect();
            out.push_str(&word);
            count += 1;
            continue;
        }

        // Operator / punctuation — include
        out.push(ch);
        if !ch.is_ascii_punctuation() || matches!(ch, '('|')'|'{'|'}'|';') {
            count += 1;
        }
        i += 1;
    }

    out.trim_end().to_string()
}

#[cfg(test)]
mod blockout_tests {
    use super::*;
    #[test]
    fn test_blockout_if_chain() {
        let lines = vec![
            r#"    if lower.contains("tiny") { return 500; }"#,
            r#"    if lower.contains("mini") { return 1100; }"#,
            r#"    if lower.contains("phi3") { return 3800; }"#,
        ];
        let result = blockout(&lines);
        assert_eq!(result[0], lines[0], "first line unchanged");
        assert!(result[1].contains("XX"), "line 2 should be blocked: {}", result[1]);
        assert!(result[2].contains("XX"), "line 3 should be blocked: {}", result[2]);
    }
}
