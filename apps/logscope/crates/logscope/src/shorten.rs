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

use std::collections::HashMap;

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
    let mut out: Vec<String>  = Vec::with_capacity(lines.len());
    let mut map: Vec<usize>   = Vec::with_capacity(lines.len());
    let mut stats = ShortenStats { original_lines: lines.len(), ..Default::default() };

    // Track the last N prefixes we've seen, with how many times each appeared.
    // key: leading token(s), value: (last_output_idx, count)
    let mut prefix_table: HashMap<String, (usize, usize)> = HashMap::new();

    for (orig_idx, &line) in lines.iter().enumerate() {
        let leading = leading_tokens(line, 3);

        // Check if this leading token sequence was seen recently
        let shortened = if let Some(prefix) = leading {
            if let Some((last_idx, count)) = prefix_table.get(&prefix).copied() {
                // Only collapse if within the look-back window and repeated enough
                if orig_idx.saturating_sub(last_idx) <= cfg.window
                    && count >= cfg.min_repeat_count - 1
                    && prefix.len() >= cfg.min_prefix_len
                {
                    // Find the first token after the repeated prefix and use ^^ notation
                    let rest = line.trim_start()
                        .trim_start_matches(&prefix)
                        .trim_start();
                    // Extract next identifier (the type name that changes)
                    let indent = leading_whitespace(line);
                    let shortened_line = format!("{}^^{}", indent, rest);
                    stats.chars_removed += prefix.len().saturating_sub(2); // ^^ is 2 chars
                    Some(shortened_line)
                } else {
                    None
                }
            } else {
                None
            }
        } else {
            None
        };

        let output_line = shortened.unwrap_or_else(|| line.to_owned());
        out.push(output_line);
        map.push(orig_idx);

        // Update prefix table
        if let Some(prefix) = leading_tokens(line, 3) {
            let entry = prefix_table.entry(prefix).or_insert((orig_idx, 0));
            entry.0 = orig_idx;
            entry.1 += 1;
        }

        // Evict old entries beyond the window
        if orig_idx % 50 == 0 {
            prefix_table.retain(|_, (last, _)| orig_idx.saturating_sub(*last) <= cfg.window);
        }
    }

    stats.output_lines = out.len();
    ShortenResult { lines: out, line_map: map, stats }
}

/// Extract the first `n` whitespace-separated tokens from a line as a
/// canonical prefix string, or None if fewer than 2 tokens.
fn leading_tokens(line: &str, n: usize) -> Option<String> {
    let trimmed = line.trim();
    // Skip pure noise lines (empty, just punctuation, very short)
    if trimmed.len() < 8 { return None; }

    let tokens: Vec<&str> = trimmed.split_whitespace().take(n).collect();
    if tokens.len() < 2 { return None; }

    // Only consider lines that start with a keyword or type identifier
    let first = tokens[0];
    let is_type_keyword = matches!(first,
        "struct" | "enum" | "union" | "class" | "typedef" | "const" |
        "static" | "extern" | "inline" | "unsigned" | "signed" |
        "let" | "fn" | "pub" | "use" | "impl" | "type" | "trait"
    ) || (first.chars().next().map(|c| c.is_ascii_lowercase()).unwrap_or(false)
          && !first.starts_with("//")
          && !first.starts_with("/*")
          && !first.starts_with('*'));

    if !is_type_keyword { return None; }

    // Use first 2 tokens as the collapsible prefix
    Some(tokens[..2.min(tokens.len())].join(" "))
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
