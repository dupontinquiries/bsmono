//! v0.1.608: structured-output detection — the pure foundation for the
//! experimental "terminal as a Jupyter-style notebook" view. Given the
//! raw text output of a command block (a notebook "cell" — see
//! term_blocks::CommandBlock, which already segments a session into
//! command+output blocks), decide whether the output is STRUCTURED and,
//! if so, parse it into something Qt can render richly inline:
//!   - a TABLE (reusing table_ops::TableData), or
//!   - a KEY/VALUE MAP ("expression mapping": env vars, config dumps).
//!
//! This module is parse-only and fully testable. The QML side (the
//! toggleable experimental notebook view: per-cell rich rendering, touch
//! gestures to collapse/expand/pin cells) consumes the Structured result
//! and is build-gated — see TODO_notebook_view.md.
//!
//! Detection is intentionally CONSERVATIVE: when in doubt it returns
//! `Plain`, so the normal terminal rendering is used. A false "this is a
//! table" is worse than a missed one — it would mangle ordinary output.

use crate::table_ops::{parse_markdown_table, TableData};

/// The classification of a cell's output.
#[derive(Clone, Debug, PartialEq)]
pub enum Structured {
    /// Not structured — render as normal terminal text.
    Plain,
    /// A table (delimited or whitespace-aligned).
    Table(TableData),
    /// A key/value mapping (env-style KEY=value or config-style key: value).
    Map(Vec<(String, String)>),
}

/// Minimum data rows (excluding header) for something to count as a
/// table. One row of columns is too ambiguous (could be a sentence with
/// spaces); require at least 2 consistent rows.
const MIN_TABLE_ROWS: usize = 2;

/// Classify a block of command output. Tries, in order: markdown table,
/// CSV, TSV, whitespace-aligned table, then key/value map. Returns
/// `Plain` if none fit confidently.
pub fn classify(output: &str) -> Structured {
    let trimmed = output.trim_matches('\n');
    if trimmed.trim().is_empty() {
        return Structured::Plain;
    }

    // 1. Markdown/pipe table — reuse the existing parser.
    if looks_like_markdown_table(trimmed) {
        if let Some(t) = parse_markdown_table(trimmed) {
            if t.n_rows() >= 1 {
                return Structured::Table(t);
            }
        }
    }

    // 2/3. Single-delimiter tables: CSV then TSV.
    for delim in [',', '\t'] {
        if let Some(t) = parse_delimited(trimmed, delim) {
            return Structured::Table(t);
        }
    }

    // 4. Whitespace-aligned columns (ps, df, headerless listings whose
    //    columns don't fight the padding convention below). Most common
    //    shell output, also the trickiest to get right.
    if let Some(t) = parse_whitespace_aligned(trimmed) {
        return Structured::Table(t);
    }

    // 5. Key/value map ("expression mapping").
    if let Some(m) = parse_kv_map(trimmed) {
        return Structured::Map(m);
    }

    Structured::Plain
}

fn looks_like_markdown_table(s: &str) -> bool {
    let mut lines = s.lines();
    let first = match lines.next() {
        Some(l) => l,
        None => return false,
    };
    // A pipe in the header and a separator-ish second line (---|---).
    if !first.contains('|') {
        return false;
    }
    match lines.next() {
        Some(sep) => sep.contains('-') && sep.contains('|'),
        None => false,
    }
}

/// Parse a single-delimiter table (CSV/TSV). Requires: ≥ (1 header +
/// MIN_TABLE_ROWS) lines, every line has the SAME column count, and that
/// count is ≥ 2 (a single column isn't a table). Returns None otherwise.
fn parse_delimited(s: &str, delim: char) -> Option<TableData> {
    let lines: Vec<&str> = s.lines().filter(|l| !l.trim().is_empty()).collect();
    if lines.len() < 1 + MIN_TABLE_ROWS {
        return None;
    }
    let split = |l: &str| -> Vec<String> {
        l.split(delim).map(|c| c.trim().to_string()).collect()
    };
    let header = split(lines[0]);
    let ncol = header.len();
    if ncol < 2 {
        return None;
    }
    let mut rows = Vec::new();
    for l in &lines[1..] {
        let cells = split(l);
        if cells.len() != ncol {
            return None; // inconsistent column count → not a clean table
        }
        rows.push(cells);
    }
    Some(TableData::new(header, rows))
}

/// Parse whitespace-aligned columns: split each line on runs of 2+
/// spaces. Requires consistent column count (≥2) across ≥ 1+MIN_TABLE_ROWS
/// lines (the first line's own split length is the count every other line
/// must match — which also means there's no meaningful difference between
/// "the first row is a header" and "the first row is data that happens to
/// have the same shape as the rest"; a caller can't tell headered output
/// like `df`/`ps` from a headerless listing whose rows are internally
/// consistent, and doesn't need to — see `classify`'s own callers, which
/// treat `headers` as display labels either way).
///
/// NOT `ls -l`, despite this crate's own earlier comments having claimed
/// it since v0.1.608: GNU `ls` right-pads its size and day-of-month
/// fields to a fixed VISUAL width by varying the separator's space count
/// inversely with the value's own width (a 1-digit day gets 2 padding
/// spaces, a 2-digit day gets 1, so both occupy the same column width).
/// That is exactly backwards from what a "split on 2+ spaces" heuristic
/// needs — the real column boundary sometimes has enough spaces to split
/// on and sometimes doesn't, for the SAME logical column, depending on
/// that row's own content. A genuine `ls -l` listing with varying file
/// sizes typically fails this function's consistency check outright
/// (different rows produce different cell counts) and correctly falls
/// through to `Plain`. A prior version of this comment tried to "fix"
/// this with a second, headerless-specific pass — reverted on review: its
/// trigger condition (`all lines agree on a cell count`) was provably
/// identical to the existing check below, so it could never run
/// differently and was dead code the moment it was written. The real
/// fix needs a character-POSITION column detector across all lines, not
/// a bigger version of this per-line splitter — a different algorithm,
/// filed as a real gap rather than quietly left as a false claim OR
/// "fixed" by code that doesn't change anything.
fn parse_whitespace_aligned(s: &str) -> Option<TableData> {
    let lines: Vec<&str> = s.lines().filter(|l| !l.trim().is_empty()).collect();
    if lines.len() < 1 + MIN_TABLE_ROWS {
        return None;
    }
    let split = |l: &str| -> Vec<String> {
        // Split on runs of 2+ whitespace.
        let mut cells = Vec::new();
        let mut cur = String::new();
        let mut space_run = 0;
        for ch in l.chars() {
            if ch == ' ' || ch == '\t' {
                space_run += 1;
            } else {
                if space_run >= 2 && !cur.is_empty() {
                    cells.push(std::mem::take(&mut cur));
                } else if space_run >= 1 {
                    cur.push(' ');
                }
                space_run = 0;
                cur.push(ch);
            }
        }
        if !cur.is_empty() {
            cells.push(cur.trim().to_string());
        }
        cells.into_iter().map(|c| c.trim().to_string()).filter(|c| !c.is_empty()).collect()
    };
    let header = split(lines[0]);
    let ncol = header.len();
    if ncol < 2 {
        return None;
    }
    let mut rows = Vec::new();
    for l in &lines[1..] {
        let cells = split(l);
        if cells.len() != ncol {
            return None;
        }
        rows.push(cells);
    }
    Some(TableData::new(header, rows))
}

/// Parse a key/value map: lines of `KEY=value` (env) or `key: value`
/// (config). Requires ≥2 lines that ALL match the same separator style,
/// so a stray "foo: bar" sentence in prose doesn't trigger it.
fn parse_kv_map(s: &str) -> Option<Vec<(String, String)>> {
    let lines: Vec<&str> = s.lines().filter(|l| !l.trim().is_empty()).collect();
    if lines.len() < 2 {
        return None;
    }
    // Try '=' first (env-style: no spaces around =, key is identifier-ish),
    // then ': ' (config-style).
    for (sep, require_space) in [('=', false), (':', true)] {
        let mut pairs = Vec::new();
        let mut all_ok = true;
        for l in &lines {
            match split_kv(l, sep, require_space) {
                Some((k, v)) => pairs.push((k, v)),
                None => { all_ok = false; break; }
            }
        }
        if all_ok && pairs.len() == lines.len() {
            // v0.1.621: guard against prose/log false-positives. Lines
            // like "Warning: disk low" / "Error: cannot write to /tmp"
            // each split as key:value, so two such lines were wrongly
            // classified as a Map. Real kv-map VALUES are overwhelmingly
            // compact single tokens (localhost, 5432, admin); prose
            // values are multi-word sentences. Require a majority of
            // values to be single-token. (This correctly KEEPS maps with
            // multi-word KEYS like "Last Modified: 2pm" — we constrain
            // the value column, not the key column.) Only applied to the
            // ':' config style; '=' env style is already tightly
            // constrained (identifier key, no spaces).
            if sep == ':' {
                let single_tok = pairs.iter()
                    .filter(|(_, v)| !v.trim().contains(char::is_whitespace))
                    .count();
                if single_tok * 2 < pairs.len() {
                    continue; // too many prose-like values — not a map
                }
            }
            return Some(pairs);
        }
    }
    None
}

fn split_kv(line: &str, sep: char, require_space_after: bool) -> Option<(String, String)> {
    let idx = line.find(sep)?;
    let key = line[..idx].trim();
    let rest = &line[idx + 1..];
    if require_space_after {
        // config-style "key: value" — require the separator to be
        // followed by a space (or end), to avoid matching "http://x".
        if !(rest.is_empty() || rest.starts_with(' ')) {
            return None;
        }
    }
    let val = rest.trim();
    // Key must be non-empty and look like a key (no internal whitespace
    // for env-style; config keys can have spaces but stay short).
    if key.is_empty() {
        return None;
    }
    if sep == '=' && key.contains(char::is_whitespace) {
        return None; // "a = b c" isn't an env assignment
    }
    Some((key.to_string(), val.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_text_stays_plain() {
        assert_eq!(classify("just a normal line of output"), Structured::Plain);
        assert_eq!(classify("hello world\nthis is prose\nnothing structured here"),
            Structured::Plain);
        assert_eq!(classify(""), Structured::Plain);
        assert_eq!(classify("   \n  \n"), Structured::Plain);
    }

    #[test]
    fn parses_csv() {
        let out = classify("name,age,city\nAlice,30,NYC\nBob,25,LA");
        match out {
            Structured::Table(t) => {
                assert_eq!(t.headers, vec!["name", "age", "city"]);
                assert_eq!(t.n_rows(), 2);
                assert_eq!(t.rows[1], vec!["Bob", "25", "LA"]);
            }
            other => panic!("expected Table, got {:?}", other),
        }
    }

    #[test]
    fn parses_tsv() {
        let out = classify("a\tb\n1\t2\n3\t4");
        assert!(matches!(out, Structured::Table(_)));
    }

    #[test]
    fn parses_whitespace_aligned_like_df() {
        // df-style output: columns separated by 2+ spaces
        let df = "Filesystem     Size  Used\n/dev/sda1      100G   40G\ntmpfs          8G     1G";
        match classify(df) {
            Structured::Table(t) => {
                assert_eq!(t.n_cols(), 3);
                assert_eq!(t.n_rows(), 2);
                assert_eq!(t.headers[0], "Filesystem");
            }
            other => panic!("expected Table, got {:?}", other),
        }
    }

    /// This crate's own comments claimed "ls -l" as a supported case since
    /// v0.1.608. It isn't, and can't be with this splitting strategy — see
    /// the long comment on `parse_whitespace_aligned` for the mechanism
    /// (GNU ls's inverse-width padding on size/day fields defeats a
    /// split-on-2+-spaces heuristic). A genuine `ls -l` listing with
    /// varying file sizes produces a different cell count per row and
    /// correctly falls through to Plain — pinned here so the false claim
    /// doesn't quietly reappear in a future comment.
    #[test]
    fn genuine_ls_minus_l_output_is_not_detected_and_that_is_documented() {
        let out = "-rw-r--r-- 1 root root    3 Sep  4 16:00 hostname\n\
                   -rw-r--r-- 1 root root   81 Sep  4 16:00 hosts\n\
                   -rw-r--r-- 1 root root 1111 Apr 18 18:13 passwd";
        assert_eq!(classify(out), Structured::Plain);
    }

    /// What DOES work without a header row: every line — including the
    /// first — has to agree on a cell count already, per
    /// `parse_whitespace_aligned`'s existing check (there's no separate
    /// "headerless" code path; a first row that's data-shaped-like-the-
    /// rest passes the same test a real header would). `ps` with its
    /// header line manually stripped is a real example of that shape.
    #[test]
    fn a_headerless_listing_whose_rows_are_internally_consistent_still_works() {
        let out = "1234 pts/0    00:00:01 bash\n5678 pts/0    00:00:00 vim\n9012 pts/1    00:00:03 cargo";
        match classify(out) {
            Structured::Table(t) => {
                assert_eq!(t.n_rows(), 2, "the first row is read as headers, per the doc above");
                assert_eq!(t.n_cols(), 2);
            }
            other => panic!("expected Table, got {other:?}"),
        }
    }

    #[test]
    fn single_space_sentence_is_not_a_table() {
        // prose with single spaces must NOT parse as a whitespace table
        let prose = "the quick brown fox\njumps over the lazy dog\nand keeps on running";
        assert_eq!(classify(prose), Structured::Plain);
    }

    #[test]
    fn inconsistent_columns_reject() {
        // 3 cols then 2 cols → not a clean table
        let bad = "a,b,c\n1,2,3\n4,5";
        // CSV parse rejects (inconsistent); falls through. Not a kv map
        // either → Plain.
        assert_eq!(classify(bad), Structured::Plain);
    }

    #[test]
    fn one_data_row_is_not_a_table() {
        // header + a single row is too ambiguous
        assert_eq!(classify("name,age\nAlice,30"), Structured::Plain);
    }

    #[test]
    fn parses_env_style_map() {
        let env = "PATH=/usr/bin\nHOME=/home/k\nSHELL=/bin/zsh";
        match classify(env) {
            Structured::Map(m) => {
                assert_eq!(m.len(), 3);
                assert_eq!(m[0], ("PATH".to_string(), "/usr/bin".to_string()));
                assert_eq!(m[2], ("SHELL".to_string(), "/bin/zsh".to_string()));
            }
            other => panic!("expected Map, got {:?}", other),
        }
    }

    #[test]
    fn parses_config_style_map() {
        let cfg = "host: localhost\nport: 8080\ndebug: true";
        match classify(cfg) {
            Structured::Map(m) => {
                assert_eq!(m.len(), 3);
                assert_eq!(m[1], ("port".to_string(), "8080".to_string()));
            }
            other => panic!("expected Map, got {:?}", other),
        }
    }

    #[test]
    fn url_in_config_does_not_break_kv() {
        // "url: http://x" — the ':' in http:// must not split there.
        // require_space_after means the FIRST ': ' (after url) wins.
        let cfg = "url: http://example.com\nname: test";
        match classify(cfg) {
            Structured::Map(m) => {
                assert_eq!(m[0], ("url".to_string(), "http://example.com".to_string()));
            }
            other => panic!("expected Map, got {:?}", other),
        }
    }

    #[test]
    fn single_kv_line_is_not_a_map() {
        // one "key: value" line could be prose → need 2+
        assert_eq!(classify("note: this is just a sentence"), Structured::Plain);
    }

    #[test]
    fn markdown_table_via_existing_parser() {
        let md = "| a | b |\n|---|---|\n| 1 | 2 |\n| 3 | 4 |";
        assert!(matches!(classify(md), Structured::Table(_)));
    }

    #[test]
    fn prose_log_lines_are_not_a_map() {
        // v0.1.621 regression: "Warning: disk low" / "Error: ..." each
        // split as key:value, so two prose log lines were wrongly a Map.
        // The value-compactness guard (most values single-token) rejects
        // them — prose values are multi-word.
        let log = "Warning: disk space low\nError: cannot write to disk";
        assert_eq!(classify(log), Structured::Plain);
    }

    #[test]
    fn map_with_multiword_key_still_detected() {
        // the guard constrains VALUES, not keys — a multi-word KEY with a
        // compact value is still a valid map.
        let m = "Last Modified: 2pm\nSize: 4kb\nOwner: root";
        assert!(matches!(classify(m), Structured::Map(_)));
    }

    #[test]
    fn map_with_mostly_compact_values_survives_one_prose_value() {
        // majority rule: 2 of 3 compact → still a map (tolerates one
        // longer value).
        let m = "host: localhost\nport: 5432\nnote: primary db server";
        assert!(matches!(classify(m), Structured::Map(_)));
    }
}
