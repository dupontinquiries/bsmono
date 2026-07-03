// src/table_ops.rs
//
// v0.1.195 (ship 2.4.1) — table data model and modification protocol.
// First piece of a multi-ship "dynamic tables for small screens" arc:
//
//   2.4.1: data model + protocol + markdown parser (THIS SHIP)
//   2.4.2: TableData → QML rendering with viewport-aware collapsing
//   2.4.3: NLP submodule parsing user requests into TableOps
//   2.4.4: in-app interaction surface (touch/voice input)
//   2.4.5: edit mode (add column, fill column with formula)
//
// Design philosophy:
//
//   - `TableData` is the bare data: headers + rows of cell values.
//     Cells are strings; numeric coercion happens at predicate/formula
//     evaluation time, not at storage time. This keeps the model simple
//     and round-trip-safe for non-numeric data (currency, percentages,
//     formatted dates).
//
//   - `TableOp` is a single modification. Operations compose: a view is
//     a base TableData plus an ordered Vec of operations applied in order.
//
//   - `TableView::apply()` is pure: it walks the ops, transforming the
//     base data, and returns the resulting TableData. This means the
//     original data is preserved (so the user can undo any op without
//     re-fetching the source) and views are cheap to recompute.
//
//   - The protocol is the contract between consumers (renderer) and
//     producers (NLP parser, edit-mode UI). Adding a new operation
//     means: extend the enum, handle it in apply(), and update the
//     parser to produce it. The renderer doesn't need to change unless
//     the new op changes the table shape in ways the renderer can't
//     already handle.
//
// Not in this ship:
//   - QML rendering (2.4.2)
//   - NLP / voice parsing (2.4.3)
//   - Interaction surface (2.4.4)
//   - Formula language beyond basic stubs (2.4.5)

use std::fmt::Write;

/// The bare table data. Headers and rows of string cells. Numeric
/// values are stored as their string representation; coercion to
/// f64 happens at evaluation time.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TableData {
    pub headers: Vec<String>,
    pub rows: Vec<Vec<String>>,
}

impl TableData {
    pub fn new(headers: Vec<String>, rows: Vec<Vec<String>>) -> Self {
        TableData { headers, rows }
    }

    pub fn n_cols(&self) -> usize {
        self.headers.len()
    }

    pub fn n_rows(&self) -> usize {
        self.rows.len()
    }

    /// Attempt to coerce a cell to f64. Tolerates surrounding
    /// whitespace, leading currency symbols, trailing % signs.
    /// Returns None when the string doesn't look numeric.
    fn cell_as_f64(s: &str) -> Option<f64> {
        let s = s.trim();
        if s.is_empty() {
            return None;
        }
        // Strip common currency prefixes and percent suffixes.
        let s = s.trim_start_matches(|c: char| matches!(c, '$' | '€' | '£' | '¥'));
        let (s, _is_percent) = if let Some(stripped) = s.strip_suffix('%') {
            (stripped, true)
        } else {
            (s, false)
        };
        // Strip thousands separators (commas in en-US, spaces in fr,
        // dots-as-thousands in some locales we ignore for now).
        let cleaned: String = s.chars().filter(|c| *c != ',').collect();
        cleaned.trim().parse::<f64>().ok()
    }
}

/// Row-level filter predicates. Applied to a specific column's cell
/// values; rows where the predicate returns false are dropped from
/// the view.
#[derive(Clone, Debug, PartialEq)]
pub enum Predicate {
    /// Case-insensitive exact match (after trim).
    Equals(String),
    /// Case-insensitive substring match (after trim).
    Contains(String),
    /// Numeric > value. Non-numeric cells fail the predicate.
    GreaterThan(f64),
    /// Numeric < value. Non-numeric cells fail the predicate.
    LessThan(f64),
    /// Cell is non-empty (after trim).
    NonEmpty,
}

impl Predicate {
    pub fn eval(&self, cell: &str) -> bool {
        match self {
            Predicate::Equals(v) => {
                cell.trim().eq_ignore_ascii_case(v.trim())
            }
            Predicate::Contains(v) => {
                cell.trim().to_lowercase().contains(&v.trim().to_lowercase())
            }
            Predicate::GreaterThan(t) => {
                TableData::cell_as_f64(cell).map(|n| n > *t).unwrap_or(false)
            }
            Predicate::LessThan(t) => {
                TableData::cell_as_f64(cell).map(|n| n < *t).unwrap_or(false)
            }
            Predicate::NonEmpty => !cell.trim().is_empty(),
        }
    }
}

/// Cell-fill formulas for the AddColumn / FillColumn operations.
/// 2.4.1 ships the minimum useful set; richer formulas (lookups,
/// regex, multi-column expressions) go in ship 2.4.5.
#[derive(Clone, Debug, PartialEq)]
pub enum Formula {
    /// Same value in every row.
    Constant(String),
    /// Numeric: multiply the previous column's value by `factor`.
    /// Non-numeric cells produce empty output.
    MultiplyPrev(f64),
    /// Numeric: add `delta` to the previous column's value.
    AddPrev(f64),
    /// Concatenate two columns with a separator.
    Concat { left: usize, sep: String, right: usize },
    /// Sum of all numeric cells in the row. Non-numeric cells are
    /// skipped. Useful for adding a "total" column to a money table.
    SumRow,
}

impl Formula {
    /// Evaluate this formula for one row, given the row's cell values
    /// and the index of the column being filled (so MultiplyPrev /
    /// AddPrev can find the "previous" cell).
    pub fn eval(&self, row: &[String], filled_col: usize) -> String {
        match self {
            Formula::Constant(s) => s.clone(),
            Formula::MultiplyPrev(factor) => {
                if filled_col == 0 {
                    return String::new();
                }
                let prev = &row[filled_col - 1];
                match TableData::cell_as_f64(prev) {
                    Some(n) => format_number(n * factor),
                    None => String::new(),
                }
            }
            Formula::AddPrev(delta) => {
                if filled_col == 0 {
                    return String::new();
                }
                let prev = &row[filled_col - 1];
                match TableData::cell_as_f64(prev) {
                    Some(n) => format_number(n + delta),
                    None => String::new(),
                }
            }
            Formula::Concat { left, sep, right } => {
                let l = row.get(*left).map(String::as_str).unwrap_or("");
                let r = row.get(*right).map(String::as_str).unwrap_or("");
                format!("{}{}{}", l, sep, r)
            }
            Formula::SumRow => {
                let mut sum = 0.0_f64;
                let mut any = false;
                for (i, cell) in row.iter().enumerate() {
                    if i == filled_col {
                        continue; // don't include ourselves
                    }
                    if let Some(n) = TableData::cell_as_f64(cell) {
                        sum += n;
                        any = true;
                    }
                }
                if any { format_number(sum) } else { String::new() }
            }
        }
    }
}

/// Format a number the way users expect: integer if no fractional
/// part, else up to 6 decimals trimmed of trailing zeros.
fn format_number(n: f64) -> String {
    if n.fract() == 0.0 && n.abs() < 1e16 {
        return format!("{}", n as i64);
    }
    let s = format!("{:.6}", n);
    // Trim trailing zeros and dangling decimal point.
    let s = s.trim_end_matches('0').trim_end_matches('.');
    s.to_string()
}

/// A single modification to a table.
///
/// The protocol is *append-only* from the producer's perspective:
/// pushing more ops onto a TableView refines the view. Removing
/// ops requires explicit pop or replace; consumers shouldn't
/// reach into the ops vec.
#[derive(Clone, Debug, PartialEq)]
pub enum TableOp {
    /// Keep ONLY these column indices (in given order). Columns
    /// not listed are hidden. Useful for "show me columns 1 4 7".
    Show(Vec<usize>),
    /// Hide these column indices. Other columns kept in original
    /// order. Useful for "hide column 3".
    Hide(Vec<usize>),
    /// Filter rows: keep only rows where the predicate holds on
    /// the given column's cell.
    Filter { column: usize, predicate: Predicate },
    /// Sort rows by the given column. Numeric sort if all cells
    /// in the column parse as numbers; lexicographic otherwise.
    Sort { column: usize, ascending: bool },
    /// Add a new column with the given header. If `after` is None,
    /// appends at the end. The new column is filled with the
    /// given formula, or empty strings if formula is None.
    AddColumn { name: String, after: Option<usize>, formula: Option<Formula> },
    /// Replace the contents of `column` by evaluating `formula`
    /// for each row.
    FillColumn { column: usize, formula: Formula },
}

/// A view over base data with an op stack. apply() walks the stack
/// in order, producing the rendered table.
#[derive(Clone, Debug, Default)]
pub struct TableView {
    pub base: TableData,
    pub ops: Vec<TableOp>,
}

impl TableView {
    pub fn new(base: TableData) -> Self {
        TableView { base, ops: Vec::new() }
    }

    pub fn push_op(&mut self, op: TableOp) {
        self.ops.push(op);
    }

    pub fn pop_op(&mut self) -> Option<TableOp> {
        self.ops.pop()
    }

    pub fn clear_ops(&mut self) {
        self.ops.clear();
    }

    /// Walk the ops in order and return the resulting TableData.
    /// Pure: doesn't mutate self. Original base is preserved.
    pub fn apply(&self) -> TableData {
        let mut t = self.base.clone();
        for op in &self.ops {
            t = apply_op(t, op);
        }
        t
    }
}

/// Apply a single op to a TableData, returning the result.
///
/// Out-of-range column indices are silently ignored (no panic).
/// The renderer can flag this; the protocol stays robust.
fn apply_op(mut t: TableData, op: &TableOp) -> TableData {
    match op {
        TableOp::Show(cols) => {
            // Build a new table containing only the specified
            // columns, in the order given. Invalid indices skip.
            let valid: Vec<usize> = cols.iter().copied()
                .filter(|&c| c < t.n_cols()).collect();
            let new_headers: Vec<String> = valid.iter()
                .map(|&c| t.headers[c].clone()).collect();
            let new_rows: Vec<Vec<String>> = t.rows.iter()
                .map(|row| valid.iter().map(|&c| {
                    row.get(c).cloned().unwrap_or_default()
                }).collect()).collect();
            TableData { headers: new_headers, rows: new_rows }
        }
        TableOp::Hide(cols) => {
            // Keep all columns NOT in `cols`.
            let hide: std::collections::HashSet<usize> = cols.iter().copied().collect();
            let keep: Vec<usize> = (0..t.n_cols()).filter(|c| !hide.contains(c)).collect();
            apply_op(t, &TableOp::Show(keep))
        }
        TableOp::Filter { column, predicate } => {
            if *column >= t.n_cols() {
                return t; // out of range, no-op
            }
            t.rows.retain(|row| {
                let cell = row.get(*column).map(String::as_str).unwrap_or("");
                predicate.eval(cell)
            });
            t
        }
        TableOp::Sort { column, ascending } => {
            if *column >= t.n_cols() {
                return t;
            }
            // Detect whether all non-empty cells in the column
            // parse as numbers; if so, numeric sort.
            let all_numeric = t.rows.iter().all(|row| {
                let cell = row.get(*column).map(String::as_str).unwrap_or("");
                cell.trim().is_empty() || TableData::cell_as_f64(cell).is_some()
            });
            let col = *column;
            let asc = *ascending;
            if all_numeric {
                t.rows.sort_by(|a, b| {
                    let av = a.get(col).and_then(|c| TableData::cell_as_f64(c)).unwrap_or(f64::NAN);
                    let bv = b.get(col).and_then(|c| TableData::cell_as_f64(c)).unwrap_or(f64::NAN);
                    // NaN sorts last in either direction.
                    let cmp = av.partial_cmp(&bv).unwrap_or(std::cmp::Ordering::Equal);
                    if asc { cmp } else { cmp.reverse() }
                });
            } else {
                t.rows.sort_by(|a, b| {
                    let av = a.get(col).map(String::as_str).unwrap_or("");
                    let bv = b.get(col).map(String::as_str).unwrap_or("");
                    let cmp = av.to_lowercase().cmp(&bv.to_lowercase());
                    if asc { cmp } else { cmp.reverse() }
                });
            }
            t
        }
        TableOp::AddColumn { name, after, formula } => {
            let insert_at = match after {
                None => t.n_cols(),
                Some(idx) => (*idx + 1).min(t.n_cols()),
            };
            t.headers.insert(insert_at, name.clone());
            for row in &mut t.rows {
                // Insert an empty cell first, then evaluate the
                // formula (if any) with the row in its post-insert
                // state. This lets formulas reference the new
                // column position correctly.
                row.insert(insert_at, String::new());
                if let Some(f) = formula {
                    let v = f.eval(row, insert_at);
                    row[insert_at] = v;
                }
            }
            t
        }
        TableOp::FillColumn { column, formula } => {
            if *column >= t.n_cols() {
                return t;
            }
            for row in &mut t.rows {
                // Pad row if shorter than headers (defensive).
                while row.len() < t.headers.len() {
                    row.push(String::new());
                }
                let v = formula.eval(row, *column);
                row[*column] = v;
            }
            t
        }
    }
}

// ────────────────────────────────────────────────────────────────
// Markdown table parsing.
// ────────────────────────────────────────────────────────────────

/// Parse a markdown-style table into TableData. Returns None if the
/// input isn't a recognizable table.
///
/// Accepts standard GFM table syntax:
///
///   | h1 | h2 | h3 |
///   |----|----|----|
///   | a  | b  | c  |
///   | d  | e  | f  |
///
/// Tolerates:
///   - Missing leading/trailing pipes
///   - Whitespace padding
///   - Alignment markers in separator row (|:-:|, |---:|, etc.) —
///     parsed but not currently preserved
///   - Empty cells (renders as empty strings)
///
/// Does not handle:
///   - Multi-line cells (markdown tables can't have them anyway)
///   - HTML-formatted cells — passed through verbatim, the renderer
///     decides whether to interpret them
///
/// Note: escaped pipes (`\|`) ARE handled — split_md_row un-escapes
/// them into literal pipes within a cell (see its docs). An earlier
/// version of this comment incorrectly listed them as unsupported.
pub fn parse_markdown_table(s: &str) -> Option<TableData> {
    let lines: Vec<&str> = s.lines()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty())
        .collect();

    if lines.len() < 2 {
        return None;
    }

    // Row 2 (after header) must be the separator: cells of only
    // dashes (with optional colons for alignment). That's how we
    // identify this as a table at all.
    if !is_separator_row(lines[1]) {
        return None;
    }

    let headers = split_md_row(lines[0]);
    if headers.is_empty() {
        return None;
    }
    let n_cols = headers.len();

    let mut rows = Vec::new();
    for line in &lines[2..] {
        let cells = split_md_row(line);
        if cells.is_empty() {
            continue;
        }
        // Pad short rows; truncate long rows to header count.
        let mut row: Vec<String> = cells.into_iter().take(n_cols).collect();
        while row.len() < n_cols {
            row.push(String::new());
        }
        rows.push(row);
    }

    Some(TableData { headers, rows })
}

/// Check whether a line is a markdown table separator row: cells
/// containing only `-`, `:`, and whitespace.
///
/// v0.1.452: also accept en-dash `–`/em-dash `—` (smart-dash
/// autocorrect artifacts) so a "valid but unformatted" table whose
/// separator got auto-converted still parses. Mirrors the relaxed
/// detection in manpages::looks_like_table_separator.
fn is_separator_row(line: &str) -> bool {
    let cells = split_md_row(line);
    if cells.is_empty() {
        return false;
    }
    cells.iter().all(|c| {
        !c.is_empty()
            && c.chars().all(|ch| matches!(ch,
                '-' | '\u{2013}' | '\u{2014}' | ':' | ' ' | '\t'))
            && c.chars().any(|ch| matches!(ch, '-' | '\u{2013}' | '\u{2014}'))
    })
}

/// Split a markdown table row on `|` characters, trimming each cell.
/// Handles missing leading/trailing pipes by treating the row as
/// the content between virtual pipes at the edges.
///
/// v0.1.229: honors `\|` as a literal pipe inside cell content
/// (GFM table escape). The escape backslash is consumed in the
/// output cell so `a\|b` produces a single cell with text `a|b`,
/// not `a\|b`. Leading and trailing pipes are still stripped as
/// table delimiters; if the line ends with `\|` (an escaped pipe
/// at the very end of the last cell, rare), the trailing-strip is
/// suppressed.
///
/// Other backslash escapes (e.g. `\n`, `\\`) inside table cells
/// are NOT processed here — they're passed through to the inline
/// markdown parser which handles its own escape rules (see
/// manpages.rs detect_escape). This function only cares about the
/// pipe-splitting concern.
fn split_md_row(line: &str) -> Vec<String> {
    let mut s = line;
    // Strip leading pipe (always a delimiter — nothing can precede
    // a position-0 char to make it an escape).
    if s.starts_with('|') {
        s = &s[1..];
    }
    // Strip trailing pipe, but only if it's NOT escaped. An escaped
    // trailing pipe means the last cell legitimately ends with `|`.
    // Test by counting consecutive backslashes before the trailing
    // `|` — odd count = escape consuming the pipe, even count = the
    // pipe is a delimiter (and the backslashes are literal pairs).
    if s.ends_with('|') {
        let bytes = s.as_bytes();
        let pipe_idx = bytes.len() - 1;
        let mut bs_count = 0;
        let mut i = pipe_idx;
        while i > 0 && bytes[i - 1] == b'\\' {
            bs_count += 1;
            i -= 1;
        }
        if bs_count % 2 == 0 {
            s = &s[..pipe_idx];
        }
    }

    // Walk the trimmed string emitting cells. The GFM rule for
    // escaping pipes inside table cells:
    //
    //   Count the consecutive backslashes immediately before a `|`.
    //   - Even count (including 0): `|` is an unescaped cell
    //     boundary. The backslashes pass through verbatim (the
    //     inline parser handles them as its own escapes if any).
    //   - Odd count: the last `\` escapes the `|` (output literal
    //     `|`). The remaining backslashes pass through verbatim.
    //
    // E.g.:
    //   `a\|b`     → 1 backslash before pipe → escape → cell `a|b`
    //   `a\\|b`    → 2 backslashes → pipe is delim → cells `a\\` `b`
    //   `a\\\|b`   → 3 backslashes → escape → cell `a\\|b`
    //   (The trailing `\\` in the escaped case stays in the cell;
    //   the inline parser later handles it as one literal `\`.)
    //
    // Iterate chars (not bytes) so multi-byte UTF-8 cells with
    // emoji, accented letters, CJK, etc. don't get garbled.
    let mut cells: Vec<String> = Vec::new();
    let mut current = String::with_capacity(32);
    let chars: Vec<char> = s.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == '|' {
            // Count consecutive `\` immediately before this pipe.
            // Look backward through `current` (the buffered chars
            // since the last cell break). Since `\` is ASCII, we
            // can count via str::ends_with-like inspection in
            // reverse.
            let bs_run = {
                let mut n = 0;
                for ch in current.chars().rev() {
                    if ch == '\\' { n += 1; } else { break; }
                }
                n
            };
            if bs_run % 2 == 1 {
                // Odd → escape consumes the last backslash and the
                // pipe becomes literal `|`. Pop the consumed `\`
                // from current and push `|`.
                current.pop();
                current.push('|');
            } else {
                // Even → delimiter. Flush current as a cell.
                cells.push(std::mem::take(&mut current).trim().to_string());
            }
            i += 1;
            continue;
        }
        // Any other char: pass through. Includes `\` — it stays in
        // the buffer to be counted later if a `|` follows.
        current.push(c);
        i += 1;
    }
    cells.push(current.trim().to_string());
    cells
}

/// Render a TableData back to a markdown string. Useful for
/// logging and for the future "save view" feature where a
/// transformed table is exported as markdown for the user.
pub fn to_markdown(t: &TableData) -> String {
    let mut o = String::with_capacity(t.n_cols() * 16 * (t.n_rows() + 2));
    // Compute column widths so the output is human-readable.
    let mut widths: Vec<usize> = t.headers.iter().map(|h| h.chars().count()).collect();
    for row in &t.rows {
        for (i, cell) in row.iter().enumerate() {
            if i < widths.len() {
                widths[i] = widths[i].max(cell.chars().count());
            }
        }
    }
    // Minimum width 3 so the separator has at least three dashes.
    for w in &mut widths {
        *w = (*w).max(3);
    }
    // Header row.
    o.push('|');
    for (i, h) in t.headers.iter().enumerate() {
        let _ = write!(o, " {:width$} |", h, width = widths[i]);
    }
    o.push('\n');
    // Separator row.
    o.push('|');
    for w in &widths {
        for _ in 0..(*w + 2) {
            o.push('-');
        }
        o.push('|');
    }
    o.push('\n');
    // Data rows.
    for row in &t.rows {
        o.push('|');
        for (i, cell) in row.iter().enumerate() {
            if i < widths.len() {
                let _ = write!(o, " {:width$} |", cell, width = widths[i]);
            }
        }
        o.push('\n');
    }
    o
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> TableData {
        TableData::new(
            vec!["Name".into(), "Age".into(), "Salary".into()],
            vec![
                vec!["Alice".into(), "30".into(), "60000".into()],
                vec!["Bob".into(), "25".into(), "45000".into()],
                vec!["Carol".into(), "40".into(), "80000".into()],
            ],
        )
    }

    #[test]
    fn parse_basic_markdown_table() {
        let md = "\
| Name | Age | Salary |
|------|-----|--------|
| Alice | 30 | 60000 |
| Bob   | 25 | 45000 |
";
        let t = parse_markdown_table(md).unwrap();
        assert_eq!(t.n_cols(), 3);
        assert_eq!(t.n_rows(), 2);
        assert_eq!(t.headers[0], "Name");
        assert_eq!(t.rows[1][2], "45000");
    }

    #[test]
    fn parse_table_without_outer_pipes() {
        let md = "\
Name | Age | Salary
-----|-----|-------
Alice | 30 | 60000
";
        let t = parse_markdown_table(md).unwrap();
        assert_eq!(t.n_cols(), 3);
        assert_eq!(t.n_rows(), 1);
    }

    #[test]
    fn parse_short_rows_get_padded() {
        let md = "\
| A | B | C |
|---|---|---|
| 1 | 2 |
";
        let t = parse_markdown_table(md).unwrap();
        assert_eq!(t.rows[0], vec!["1", "2", ""]);
    }

    #[test]
    fn parse_rejects_non_table() {
        assert!(parse_markdown_table("just some text").is_none());
        assert!(parse_markdown_table("| not a header |").is_none()); // no separator
    }

    #[test]
    fn parse_alignment_separators() {
        let md = "\
| L | C | R |
|:--|:-:|--:|
| 1 | 2 | 3 |
";
        let t = parse_markdown_table(md).unwrap();
        assert_eq!(t.n_cols(), 3);
    }

    // v0.1.452: separator with smart-dash autocorrect (em-dash)
    // still parses as a table.
    #[test]
    fn parse_emdash_separator() {
        let md = "\
| A | B |
| \u{2014} | \u{2014} |
| 1 | 2 |
";
        let t = parse_markdown_table(md).expect("em-dash separator should parse");
        assert_eq!(t.n_cols(), 2);
        assert_eq!(t.rows.len(), 1);
        // And a colon-only separator (no dash) is still rejected.
        assert!(parse_markdown_table("| A | B |\n| : | : |\n| 1 | 2 |\n").is_none(),
            "colon-only separator must not parse");
    }

    #[test]
    fn parse_single_column_table() {
        // v0.1.882: a one-column table (a hypothesized failure mode in the
        // "local tables not rendered" report). Must parse, 1 col, 2 rows.
        let md = "| Item |\n|------|\n| apple |\n| pear |\n";
        let t = parse_markdown_table(md).expect("single-column table should parse");
        assert_eq!(t.n_cols(), 1);
        assert_eq!(t.headers, vec!["Item"]);
        assert_eq!(t.rows.len(), 2);
        assert_eq!(t.rows[1], vec!["pear".to_string()]);
    }

    #[test]
    fn parse_escaped_pipe_in_cell() {
        // GFM: `\|` is a literal pipe inside a cell, not a column delimiter.
        let md = "| expr | meaning |\n|---|---|\n| a \\| b | a or b |\n";
        let t = parse_markdown_table(md).expect("escaped-pipe table should parse");
        assert_eq!(t.n_cols(), 2, "escaped pipe must not split into a 3rd column");
        assert_eq!(t.rows[0][0], "a | b", "the \\| un-escapes to a literal pipe");
        assert_eq!(t.rows[0][1], "a or b");
    }

    #[test]
    fn parse_inline_markup_cells_kept_verbatim_at_parse() {
        // The parser preserves cell text verbatim (incl. *, `, [](); the
        // inline_to_richtext pass at render time turns it into rich text).
        let md = "| col |\n|---|\n| *bold* `code` [x](http://e.com) |\n";
        let t = parse_markdown_table(md).expect("inline-markup cell should parse");
        assert_eq!(t.rows[0][0], "*bold* `code` [x](http://e.com)");
    }

    #[test]
    fn show_keeps_only_listed_columns() {
        let t = fixture();
        let view = TableView { base: t, ops: vec![TableOp::Show(vec![0, 2])] };
        let r = view.apply();
        assert_eq!(r.headers, vec!["Name", "Salary"]);
        assert_eq!(r.rows[0], vec!["Alice", "60000"]);
    }

    #[test]
    fn show_respects_order() {
        // "show me columns 2 0 1" means columns in that order.
        let t = fixture();
        let view = TableView { base: t, ops: vec![TableOp::Show(vec![2, 0, 1])] };
        let r = view.apply();
        assert_eq!(r.headers, vec!["Salary", "Name", "Age"]);
    }

    #[test]
    fn hide_keeps_others_in_original_order() {
        let t = fixture();
        let view = TableView { base: t, ops: vec![TableOp::Hide(vec![1])] };
        let r = view.apply();
        assert_eq!(r.headers, vec!["Name", "Salary"]);
    }

    #[test]
    fn filter_equals() {
        let t = fixture();
        let view = TableView {
            base: t,
            ops: vec![TableOp::Filter {
                column: 0,
                predicate: Predicate::Equals("alice".into()),
            }],
        };
        let r = view.apply();
        assert_eq!(r.n_rows(), 1);
        assert_eq!(r.rows[0][0], "Alice");
    }

    #[test]
    fn filter_greater_than_numeric() {
        let t = fixture();
        let view = TableView {
            base: t,
            ops: vec![TableOp::Filter {
                column: 2,
                predicate: Predicate::GreaterThan(50000.0),
            }],
        };
        let r = view.apply();
        assert_eq!(r.n_rows(), 2);
        // Bob's 45000 is filtered out.
        let names: Vec<&str> = r.rows.iter().map(|row| row[0].as_str()).collect();
        assert!(names.contains(&"Alice"));
        assert!(names.contains(&"Carol"));
        assert!(!names.contains(&"Bob"));
    }

    #[test]
    fn filter_contains_case_insensitive() {
        let t = fixture();
        let view = TableView {
            base: t,
            ops: vec![TableOp::Filter {
                column: 0,
                predicate: Predicate::Contains("ar".into()),
            }],
        };
        let r = view.apply();
        // "Carol" contains "ar"; Alice/Bob don't.
        assert_eq!(r.n_rows(), 1);
    }

    #[test]
    fn sort_numeric_ascending() {
        let t = fixture();
        let view = TableView {
            base: t,
            ops: vec![TableOp::Sort { column: 1, ascending: true }],
        };
        let r = view.apply();
        let ages: Vec<&str> = r.rows.iter().map(|row| row[1].as_str()).collect();
        assert_eq!(ages, vec!["25", "30", "40"]);
    }

    #[test]
    fn sort_numeric_descending() {
        let t = fixture();
        let view = TableView {
            base: t,
            ops: vec![TableOp::Sort { column: 2, ascending: false }],
        };
        let r = view.apply();
        let sals: Vec<&str> = r.rows.iter().map(|row| row[2].as_str()).collect();
        assert_eq!(sals, vec!["80000", "60000", "45000"]);
    }

    #[test]
    fn sort_lexicographic_when_non_numeric() {
        let t = fixture();
        let view = TableView {
            base: t,
            ops: vec![TableOp::Sort { column: 0, ascending: true }],
        };
        let r = view.apply();
        let names: Vec<&str> = r.rows.iter().map(|row| row[0].as_str()).collect();
        assert_eq!(names, vec!["Alice", "Bob", "Carol"]);
    }

    #[test]
    fn add_column_appended() {
        let t = fixture();
        let view = TableView {
            base: t,
            ops: vec![TableOp::AddColumn {
                name: "Bonus".into(),
                after: None,
                formula: None,
            }],
        };
        let r = view.apply();
        assert_eq!(r.n_cols(), 4);
        assert_eq!(r.headers[3], "Bonus");
        assert_eq!(r.rows[0][3], "");
    }

    #[test]
    fn add_column_with_2x_prev_formula() {
        // User's exact phrasing: "fill column with 2x before"
        let t = fixture();
        let view = TableView {
            base: t,
            ops: vec![TableOp::AddColumn {
                name: "Doubled Salary".into(),
                after: Some(2),
                formula: Some(Formula::MultiplyPrev(2.0)),
            }],
        };
        let r = view.apply();
        assert_eq!(r.n_cols(), 4);
        assert_eq!(r.headers[3], "Doubled Salary");
        // Alice's salary 60000 → 120000
        assert_eq!(r.rows[0][3], "120000");
        // Bob's 45000 → 90000
        assert_eq!(r.rows[1][3], "90000");
        // Carol's 80000 → 160000
        assert_eq!(r.rows[2][3], "160000");
    }

    #[test]
    fn fill_column_with_constant() {
        let t = fixture();
        let view = TableView {
            base: t,
            ops: vec![TableOp::FillColumn {
                column: 1,
                formula: Formula::Constant("REDACTED".into()),
            }],
        };
        let r = view.apply();
        for row in &r.rows {
            assert_eq!(row[1], "REDACTED");
        }
    }

    #[test]
    fn fill_column_concat() {
        let t = fixture();
        let view = TableView {
            base: t,
            ops: vec![TableOp::AddColumn {
                name: "Summary".into(),
                after: None,
                formula: Some(Formula::Concat {
                    left: 0,
                    sep: " - ".into(),
                    right: 1,
                }),
            }],
        };
        let r = view.apply();
        assert_eq!(r.rows[0][3], "Alice - 30");
    }

    #[test]
    fn ops_compose_in_order() {
        // First filter, then show specific columns. Verify result.
        let t = fixture();
        let view = TableView {
            base: t,
            ops: vec![
                TableOp::Filter {
                    column: 2,
                    predicate: Predicate::GreaterThan(50000.0),
                },
                TableOp::Show(vec![0, 2]),
            ],
        };
        let r = view.apply();
        assert_eq!(r.headers, vec!["Name", "Salary"]);
        assert_eq!(r.n_rows(), 2);
    }

    #[test]
    fn round_trip_markdown() {
        // Parse → render → parse should round-trip identical data.
        let md = "\
| Name | Age |
|------|-----|
| Alice | 30 |
| Bob | 25 |
";
        let t = parse_markdown_table(md).unwrap();
        let rendered = to_markdown(&t);
        let t2 = parse_markdown_table(&rendered).unwrap();
        assert_eq!(t, t2);
    }

    #[test]
    fn currency_coercion_in_predicate() {
        // GreaterThan should handle "$60,000" as 60000.
        let t = TableData::new(
            vec!["Item".into(), "Price".into()],
            vec![
                vec!["Widget".into(), "$1,200".into()],
                vec!["Gadget".into(), "$50".into()],
            ],
        );
        let view = TableView {
            base: t,
            ops: vec![TableOp::Filter {
                column: 1,
                predicate: Predicate::GreaterThan(100.0),
            }],
        };
        let r = view.apply();
        assert_eq!(r.n_rows(), 1);
        assert_eq!(r.rows[0][0], "Widget");
    }

    #[test]
    fn sum_row_formula() {
        let t = TableData::new(
            vec!["Q1".into(), "Q2".into(), "Q3".into()],
            vec![vec!["100".into(), "200".into(), "150".into()]],
        );
        let view = TableView {
            base: t,
            ops: vec![TableOp::AddColumn {
                name: "Total".into(),
                after: None,
                formula: Some(Formula::SumRow),
            }],
        };
        let r = view.apply();
        assert_eq!(r.rows[0][3], "450");
    }

    #[test]
    fn empty_ops_returns_base() {
        let t = fixture();
        let view = TableView::new(t.clone());
        let r = view.apply();
        assert_eq!(r, t);
    }

    #[test]
    fn invalid_column_indices_are_ignored() {
        let t = fixture();
        let view = TableView {
            base: t,
            ops: vec![TableOp::Show(vec![0, 99, 2, 100])],
        };
        let r = view.apply();
        // 99 and 100 silently dropped.
        assert_eq!(r.headers, vec!["Name", "Salary"]);
    }

    // v0.1.229: escape-aware split_md_row tests.

    #[test]
    fn escaped_pipe_inside_cell_is_literal() {
        let md = "\
| key | value |
|-----|-------|
| pipe-char | a\\|b |
";
        let t = parse_markdown_table(md).unwrap();
        assert_eq!(t.n_cols(), 2);
        assert_eq!(t.n_rows(), 1);
        // The escaped pipe should become literal `|` and the
        // backslash should be consumed.
        assert_eq!(t.rows[0][1], "a|b", "got: {:?}", t.rows[0]);
    }

    #[test]
    fn double_backslash_before_pipe_does_not_escape() {
        // 2 backslashes before `|` means literal `\\` + table-delim
        // pipe. The cell ends after `\\`, not consuming the pipe.
        let md = "\
| key | value | extra |
|-----|-------|-------|
| x | a\\\\ | b |
";
        let t = parse_markdown_table(md).unwrap();
        // Row should have 3 cells: x, a\\, b
        assert_eq!(t.n_cols(), 3);
        // Note: a\\ in the markdown is two literal backslashes;
        // they stay in the cell for the inline parser to handle.
        assert_eq!(t.rows[0], vec!["x", "a\\\\", "b"]);
    }

    #[test]
    fn triple_backslash_before_pipe_escapes() {
        // 3 backslashes before `|` = 1 backslash literal + 1
        // backslash escape of the pipe → cell has `\|`.
        let md = "\
| key | value |
|-----|-------|
| x | a\\\\\\|b |
";
        let t = parse_markdown_table(md).unwrap();
        assert_eq!(t.n_cols(), 2);
        // 3 backslashes split as: pair = literal `\\` passes
        // through, last `\` escapes `|` → cell contains `a\\|b`
        // (a + literal `\\` + literal `|` + `b`).
        // In Rust string repr: "a\\\\|b" = a + 2 backslashes + | + b
        assert_eq!(t.rows[0][1], "a\\\\|b", "got: {:?}", t.rows[0]);
    }

    #[test]
    fn escaped_trailing_pipe_preserved() {
        // Line ends with `\|` — the trailing pipe is escaped, not
        // a table delimiter. The cell contains the literal pipe.
        // To make this a valid table row, prepend a leading pipe.
        let md = "\
| key | value |
|-----|-------|
| x | abc\\|
";
        let t = parse_markdown_table(md).unwrap();
        // After stripping the leading pipe and NOT stripping the
        // (escaped) trailing pipe, we have: " x | abc\\|"
        // Split: "x", "abc|"
        assert_eq!(t.rows[0][1], "abc|", "got: {:?}", t.rows[0]);
    }

    #[test]
    fn pipe_in_cell_without_escape_truncates_to_header() {
        // v0.1.466: corrected expectation. The module's documented
        // contract is that the HEADER defines column count and data
        // rows are normalized to it (pad short / truncate long) —
        // this is standard GFM behavior. A bare unescaped `|` inside
        // a cell therefore does NOT add a third column; the row is
        // truncated to the header's 2 columns. (The prior test
        // asserted n_cols()==3, which would mean any cell containing
        // a stray pipe silently restructures the table — a bug, not
        // a feature. Users who want a literal pipe in a cell escape
        // it as `\|`, which split_md_row honors.)
        let md = "\
| key | value |
|-----|-------|
| x | a|b |
";
        let t = parse_markdown_table(md).unwrap();
        // Header has 2 columns → row normalized to 2; the split
        // produced [x, a, b] and the trailing `b` is truncated.
        assert_eq!(t.n_cols(), 2);
        assert_eq!(t.rows[0], vec!["x", "a"]);
    }

    #[test]
    fn escaped_pipe_keeps_cell_intact() {
        // The supported way to put a literal pipe in a cell: `\|`.
        // This keeps the row at 2 columns with the pipe preserved.
        let md = "\
| key | value |
|-----|-------|
| x | a\\|b |
";
        let t = parse_markdown_table(md).unwrap();
        assert_eq!(t.n_cols(), 2);
        assert_eq!(t.rows[0], vec!["x", "a|b"]);
    }

    #[test]
    fn multi_byte_chars_in_escaped_cell() {
        // UTF-8 sanity: emoji + escaped pipe in same cell.
        let md = "\
| a | b |
|---|---|
| 📷 | x\\|y |
";
        let t = parse_markdown_table(md).unwrap();
        assert_eq!(t.rows[0][0], "📷");
        assert_eq!(t.rows[0][1], "x|y");
    }
}
