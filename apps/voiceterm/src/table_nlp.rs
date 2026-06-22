// src/table_nlp.rs
//
// v0.1.195 (ship 2.4.3, paired with 2.4.1) — natural-language command
// parsing for the table operations protocol. Translates user phrases
// like "show me columns 1 4 7" into TableOp variants.
//
// Scope: a *controlled command vocabulary* parser, not an NLP-from-
// scratch system. Vocab is small (show/hide/filter/sort/add/fill/keep)
// and grammar is regular. Tractable with hand-rolled regex + token
// walking. Future ships can extend the vocabulary; the protocol
// underneath doesn't change.
//
// What this DOESN'T try to handle:
//   - "show same as before" — no temporal references
//   - "make the X column smaller" — no formatting commands
//   - "highlight rows over 50" — no styling commands; data only
//   - Fuzzy column-name matching ("salary" → col index) — punt to
//     ship 2.4.4 once we wire it into the live table renderer where
//     header strings are visible
//
// What it DOES handle (initial scope):
//
//   show {me|}? {column|columns|cols} 1 4 7    → Show([0,3,6])
//   hide {column|col} 3                         → Hide([2])
//   hide columns 2 3 4                          → Hide([1,2,3])
//   keep only columns 1 2                       → Show([0,1])
//   sort by column 2 {ascending|descending}?    → Sort{column:1, ...}
//   sort by 3 desc                              → Sort{column:2, false}
//   filter column 1 = "active"                  → Filter{Equals}
//   filter column 2 > 50                        → Filter{GreaterThan}
//   filter column 1 contains foo                → Filter{Contains}
//   add column "Bonus"                          → AddColumn
//   add column "Doubled" with 2x prev           → AddColumn + MultiplyPrev
//   fill column 3 with 2x prev                  → FillColumn{MultiplyPrev}
//   fill column 4 with sum                      → FillColumn{SumRow}
//
// Column numbers in user input are ALWAYS 1-indexed (user-friendly).
// The parser converts to 0-indexed before producing the TableOp.

use crate::table_ops::{Formula, Predicate, TableOp};

/// v0.1.200: meta-commands that manipulate the op stack rather
/// than transform the table. Distinct from TableOp because they
/// don't have a "before/after data" semantics — they affect the
/// view's history.
///
/// Separating these from parse_command keeps the TableOp protocol
/// focused on transformations and avoids polluting it with stack-
/// manipulation variants.
#[derive(Clone, Debug, PartialEq)]
pub enum MetaCommand {
    /// Pop the most recent op. "undo" / "undo last".
    Undo,
    /// Clear all ops, returning to base data. "reset" / "clear".
    Reset,
}

/// Try to parse a meta-command (undo / reset). Returns None if the
/// input doesn't match a meta vocabulary. Caller should fall
/// through to parse_command for transformation commands.
///
/// Returned vocabulary:
///   "undo"            → Undo
///   "undo last"       → Undo
///   "undo last op"    → Undo
///   "reset"           → Reset
///   "clear"           → Reset
///   "clear all"       → Reset
pub fn parse_meta_command(input: &str) -> Option<MetaCommand> {
    let normalized = normalize(input);
    let tokens: Vec<&str> = normalized.split_whitespace().collect();
    if tokens.is_empty() {
        return None;
    }
    match tokens[0] {
        "undo" => {
            // Accept optional trailing "last", "last op", etc.
            // We don't error on unrecognized trailing words because
            // "undo" by itself is unambiguous.
            Some(MetaCommand::Undo)
        }
        "reset" | "clear" => Some(MetaCommand::Reset),
        _ => None,
    }
}

/// Public entry point: parse a single command string into a TableOp.
/// Returns `Err` with a human-readable message if the command isn't
/// recognized. The error is designed to be shown back to the user
/// ("I didn't understand 'show fish 7'") not just logged.
pub fn parse_command(input: &str) -> Result<TableOp, String> {
    let normalized = normalize(input);
    let tokens: Vec<&str> = normalized.split_whitespace().collect();
    if tokens.is_empty() {
        return Err("Empty command".into());
    }

    // Dispatch on the first verb. Some verbs accept synonyms; we
    // canonicalize them here so the rest of the parser sees one
    // form per concept.
    match tokens[0] {
        "show" | "keep" | "only" => parse_show(&tokens),
        "hide" | "drop" | "remove" => parse_hide(&tokens),
        "filter" | "where" => parse_filter(&tokens),
        "sort" | "order" => parse_sort(&tokens),
        "add" => parse_add(&tokens, input),
        "fill" => parse_fill(&tokens, input),
        _ => Err(format!("I don't recognize the command \"{}\"", tokens[0])),
    }
}

/// Lowercase + strip surrounding punctuation. Keeps content tokens
/// intact so column numbers and quoted strings still parse cleanly.
fn normalize(s: &str) -> String {
    s.trim().to_lowercase()
}

/// Parse a sequence of 1-indexed column numbers from the token list,
/// starting at `start`. Returns the converted 0-indexed list plus
/// the index of the first non-number token (so callers can keep
/// parsing the rest of the command).
fn parse_column_numbers(tokens: &[&str], start: usize) -> (Vec<usize>, usize) {
    let mut out = Vec::new();
    let mut i = start;
    while i < tokens.len() {
        // Skip joiners (comma-stripped, "and") that users often use.
        let cleaned: String = tokens[i].chars()
            .filter(|c| !matches!(c, ',' | ';'))
            .collect();
        if cleaned.is_empty() || cleaned == "and" {
            i += 1;
            continue;
        }
        match cleaned.parse::<usize>() {
            Ok(n) if n >= 1 => {
                out.push(n - 1); // convert to 0-indexed
                i += 1;
            }
            Ok(_) => {
                // 0 — out-of-range; stop here.
                break;
            }
            Err(_) => break,
        }
    }
    (out, i)
}

/// "show [me] [columns] 1 4 7" / "keep only [columns] 1 2"
fn parse_show(tokens: &[&str]) -> Result<TableOp, String> {
    let mut i = 1;
    // Skip filler words: me, only, the, columns, column, cols
    while i < tokens.len()
        && matches!(tokens[i], "me" | "only" | "the" | "columns" | "column" | "cols" | "col")
    {
        i += 1;
    }
    let (cols, _) = parse_column_numbers(tokens, i);
    if cols.is_empty() {
        return Err("Expected column numbers after \"show\"".into());
    }
    Ok(TableOp::Show(cols))
}

/// "hide [columns] 2 3 4"
fn parse_hide(tokens: &[&str]) -> Result<TableOp, String> {
    let mut i = 1;
    while i < tokens.len()
        && matches!(tokens[i], "the" | "columns" | "column" | "cols" | "col")
    {
        i += 1;
    }
    let (cols, _) = parse_column_numbers(tokens, i);
    if cols.is_empty() {
        return Err("Expected column numbers after \"hide\"".into());
    }
    Ok(TableOp::Hide(cols))
}

/// "filter column 2 > 50"
/// "filter column 1 = active" / "filter column 1 contains foo"
/// "where column 3 is non-empty"
fn parse_filter(tokens: &[&str]) -> Result<TableOp, String> {
    let mut i = 1;
    // Optional "column"/"col" keyword.
    while i < tokens.len()
        && matches!(tokens[i], "the" | "column" | "col" | "cols")
    {
        i += 1;
    }
    // Column number.
    if i >= tokens.len() {
        return Err("Expected column number after \"filter\"".into());
    }
    let col_str: String = tokens[i].chars().filter(|c| c.is_ascii_digit()).collect();
    let col_1 = col_str.parse::<usize>()
        .map_err(|_| format!("Expected a column number, got \"{}\"", tokens[i]))?;
    if col_1 == 0 {
        return Err("Column numbers start at 1".into());
    }
    let col = col_1 - 1;
    i += 1;

    // Operator + value. Glue operator words like "is", "equals" all
    // to the equals predicate.
    if i >= tokens.len() {
        // Bare "filter column 3" → presence filter (NonEmpty).
        return Ok(TableOp::Filter { column: col, predicate: Predicate::NonEmpty });
    }
    let op = tokens[i];
    i += 1;
    let predicate = match op {
        "=" | "is" | "equals" | "==" => {
            let value = collect_value(&tokens[i..]);
            Predicate::Equals(value)
        }
        ">" | "gt" | "above" => {
            let value = parse_value_as_f64(&tokens[i..])?;
            Predicate::GreaterThan(value)
        }
        "<" | "lt" | "below" | "under" => {
            let value = parse_value_as_f64(&tokens[i..])?;
            Predicate::LessThan(value)
        }
        "contains" | "has" => {
            let value = collect_value(&tokens[i..]);
            Predicate::Contains(value)
        }
        "non-empty" | "nonempty" | "set" | "filled" => {
            Predicate::NonEmpty
        }
        _ => return Err(format!("Unknown filter operator \"{}\"", op)),
    };
    Ok(TableOp::Filter { column: col, predicate })
}

/// "sort by column 2 [ascending|descending|asc|desc]"
/// "order by 3 desc"
fn parse_sort(tokens: &[&str]) -> Result<TableOp, String> {
    let mut i = 1;
    while i < tokens.len()
        && matches!(tokens[i], "by" | "the" | "column" | "col")
    {
        i += 1;
    }
    if i >= tokens.len() {
        return Err("Expected column number after \"sort\"".into());
    }
    let col_str: String = tokens[i].chars().filter(|c| c.is_ascii_digit()).collect();
    let col_1 = col_str.parse::<usize>()
        .map_err(|_| format!("Expected column number, got \"{}\"", tokens[i]))?;
    if col_1 == 0 {
        return Err("Column numbers start at 1".into());
    }
    let col = col_1 - 1;
    i += 1;

    // Default ascending. Look for direction keyword.
    let ascending = if i < tokens.len() {
        match tokens[i] {
            "desc" | "descending" | "down" | "reverse" => false,
            "asc" | "ascending" | "up" => true,
            _ => true, // unknown trailing word — default ascending
        }
    } else {
        true
    };
    Ok(TableOp::Sort { column: col, ascending })
}

/// "add column [name]"
/// "add column [name] with 2x prev"
/// "add column [name] with sum"
fn parse_add(tokens: &[&str], raw_input: &str) -> Result<TableOp, String> {
    let mut i = 1;
    while i < tokens.len()
        && matches!(tokens[i], "a" | "the" | "new" | "column" | "col")
    {
        i += 1;
    }
    // Name: either a quoted string (from raw_input) or remaining
    // non-keyword words until "with" / end.
    let name = extract_name(tokens, i, raw_input);
    // Find "with" to parse a formula.
    let formula = tokens.iter().position(|&t| t == "with").map(|w_idx| {
        parse_formula(&tokens[w_idx + 1..])
    }).transpose()?;
    Ok(TableOp::AddColumn { name, after: None, formula })
}

/// "fill column 3 with 2x prev"
/// "fill column 4 with sum"
/// "fill column 2 with REDACTED"
fn parse_fill(tokens: &[&str], _raw_input: &str) -> Result<TableOp, String> {
    let mut i = 1;
    while i < tokens.len()
        && matches!(tokens[i], "the" | "column" | "col")
    {
        i += 1;
    }
    if i >= tokens.len() {
        return Err("Expected column number after \"fill\"".into());
    }
    let col_str: String = tokens[i].chars().filter(|c| c.is_ascii_digit()).collect();
    let col_1 = col_str.parse::<usize>()
        .map_err(|_| format!("Expected column number, got \"{}\"", tokens[i]))?;
    if col_1 == 0 {
        return Err("Column numbers start at 1".into());
    }
    let column = col_1 - 1;
    i += 1;
    // Expect "with"
    if i >= tokens.len() || tokens[i] != "with" {
        return Err("Expected \"with\" followed by a formula".into());
    }
    i += 1;
    let formula = parse_formula(&tokens[i..])?;
    Ok(TableOp::FillColumn { column, formula })
}

/// Parse the formula portion (everything after "with").
///
///   "2x prev" / "2x previous" / "2x before"  → MultiplyPrev(2.0)
///   "3x prev"                                → MultiplyPrev(3.0)
///   "plus 5"                                 → AddPrev(5.0)
///   "minus 2"                                → AddPrev(-2.0)
///   "sum"                                    → SumRow
///   "REDACTED" / quoted text                 → Constant(...)
fn parse_formula(tokens: &[&str]) -> Result<Formula, String> {
    if tokens.is_empty() {
        return Err("Expected a formula after \"with\"".into());
    }
    let head = tokens[0];

    // "2x prev" pattern
    if let Some(num_str) = head.strip_suffix('x') {
        if let Ok(factor) = num_str.parse::<f64>() {
            // Look for "prev" / "before" / "previous"
            if tokens.iter().any(|t| matches!(*t, "prev" | "previous" | "before")) {
                return Ok(Formula::MultiplyPrev(factor));
            }
            // Just "2x" with no anchor — assume prev.
            return Ok(Formula::MultiplyPrev(factor));
        }
    }

    // "plus N" / "minus N"
    if head == "plus" || head == "minus" {
        if tokens.len() < 2 {
            return Err(format!("Expected a number after \"{}\"", head));
        }
        let n: f64 = tokens[1].parse().map_err(|_|
            format!("Expected a number, got \"{}\"", tokens[1])
        )?;
        let delta = if head == "minus" { -n } else { n };
        return Ok(Formula::AddPrev(delta));
    }

    // "sum"
    if head == "sum" || head == "total" {
        return Ok(Formula::SumRow);
    }

    // Otherwise treat the whole remainder as a constant string.
    let joined = tokens.join(" ");
    let trimmed = joined.trim_matches(|c: char| matches!(c, '"' | '\''));
    Ok(Formula::Constant(trimmed.to_string()))
}

/// Extract a column name from add-column tokens. Prefer quoted
/// strings in the raw input; fall back to the first non-keyword
/// token sequence.
fn extract_name(tokens: &[&str], start: usize, raw_input: &str) -> String {
    // Look for a quoted name in the raw input first.
    if let Some(name) = extract_quoted(raw_input) {
        return name;
    }
    // Otherwise gather words until "with" or end-of-tokens.
    let mut parts = Vec::new();
    for tok in &tokens[start..] {
        if *tok == "with" {
            break;
        }
        parts.push(*tok);
    }
    if parts.is_empty() {
        "New".to_string()
    } else {
        // Capitalize the first letter of each word for a more
        // header-shaped result.
        parts.iter().map(|w| {
            let mut c = w.chars();
            match c.next() {
                Some(first) => first.to_uppercase().chain(c).collect::<String>(),
                None => String::new(),
            }
        }).collect::<Vec<_>>().join(" ")
    }
}

/// Find the first quoted substring in the input.
fn extract_quoted(s: &str) -> Option<String> {
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '"' || c == '\'' {
            let quote = c;
            let mut out = String::new();
            for c2 in chars.by_ref() {
                if c2 == quote {
                    return Some(out);
                }
                out.push(c2);
            }
            return None;
        }
    }
    None
}

/// Collect a value string from remaining tokens (after operator).
/// Joins words with spaces; strips surrounding quotes if present.
fn collect_value(rest: &[&str]) -> String {
    let joined = rest.join(" ");
    let trimmed = joined.trim_matches(|c: char| matches!(c, '"' | '\''));
    trimmed.to_string()
}

/// Parse the first remaining token as a number (with currency
/// tolerance), for numeric predicates.
fn parse_value_as_f64(rest: &[&str]) -> Result<f64, String> {
    if rest.is_empty() {
        return Err("Expected a number".into());
    }
    let cleaned: String = rest[0].chars()
        .filter(|c| c.is_ascii_digit() || *c == '.' || *c == '-')
        .collect();
    cleaned.parse::<f64>().map_err(|_|
        format!("Expected a number, got \"{}\"", rest[0])
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn show_columns_1_4_7() {
        // The user's exact example.
        let op = parse_command("show me columns 1 4 7").unwrap();
        assert_eq!(op, TableOp::Show(vec![0, 3, 6]));
    }

    #[test]
    fn show_handles_commas() {
        let op = parse_command("show columns 1, 4, 7").unwrap();
        assert_eq!(op, TableOp::Show(vec![0, 3, 6]));
    }

    #[test]
    fn show_handles_and() {
        let op = parse_command("show columns 1 and 4 and 7").unwrap();
        assert_eq!(op, TableOp::Show(vec![0, 3, 6]));
    }

    #[test]
    fn keep_only_synonym() {
        let op = parse_command("keep only columns 2 3").unwrap();
        assert_eq!(op, TableOp::Show(vec![1, 2]));
    }

    #[test]
    fn hide_column() {
        let op = parse_command("hide column 3").unwrap();
        assert_eq!(op, TableOp::Hide(vec![2]));
    }

    #[test]
    fn hide_multiple() {
        let op = parse_command("hide columns 2 4 6").unwrap();
        assert_eq!(op, TableOp::Hide(vec![1, 3, 5]));
    }

    #[test]
    fn drop_synonym() {
        let op = parse_command("drop column 1").unwrap();
        assert_eq!(op, TableOp::Hide(vec![0]));
    }

    #[test]
    fn filter_equals() {
        let op = parse_command("filter column 1 = active").unwrap();
        assert_eq!(op, TableOp::Filter {
            column: 0,
            predicate: Predicate::Equals("active".into()),
        });
    }

    #[test]
    fn filter_is_synonym() {
        let op = parse_command("filter column 1 is active").unwrap();
        if let TableOp::Filter { predicate: Predicate::Equals(v), .. } = op {
            assert_eq!(v, "active");
        } else {
            panic!("expected Filter+Equals");
        }
    }

    #[test]
    fn filter_greater_than() {
        let op = parse_command("filter column 2 > 50").unwrap();
        assert_eq!(op, TableOp::Filter {
            column: 1,
            predicate: Predicate::GreaterThan(50.0),
        });
    }

    #[test]
    fn filter_contains() {
        let op = parse_command("filter column 1 contains widget").unwrap();
        assert_eq!(op, TableOp::Filter {
            column: 0,
            predicate: Predicate::Contains("widget".into()),
        });
    }

    #[test]
    fn filter_bare_column_means_nonempty() {
        let op = parse_command("filter column 3").unwrap();
        assert_eq!(op, TableOp::Filter {
            column: 2,
            predicate: Predicate::NonEmpty,
        });
    }

    #[test]
    fn sort_ascending_default() {
        let op = parse_command("sort by column 2").unwrap();
        assert_eq!(op, TableOp::Sort { column: 1, ascending: true });
    }

    #[test]
    fn sort_descending() {
        let op = parse_command("sort by column 3 descending").unwrap();
        assert_eq!(op, TableOp::Sort { column: 2, ascending: false });
    }

    #[test]
    fn sort_desc_abbrev() {
        let op = parse_command("sort by 3 desc").unwrap();
        assert_eq!(op, TableOp::Sort { column: 2, ascending: false });
    }

    #[test]
    fn add_column_no_formula() {
        let op = parse_command("add column").unwrap();
        if let TableOp::AddColumn { name, formula, .. } = op {
            // No name given — defaults to "New" (capitalized).
            assert_eq!(name, "New");
            assert!(formula.is_none());
        } else {
            panic!("expected AddColumn");
        }
    }

    #[test]
    fn add_column_with_quoted_name() {
        let op = parse_command("add column \"Bonus\"").unwrap();
        if let TableOp::AddColumn { name, formula, .. } = op {
            assert_eq!(name, "Bonus");
            assert!(formula.is_none());
        } else {
            panic!("expected AddColumn");
        }
    }

    #[test]
    fn fill_column_with_2x_prev() {
        // User's exact phrasing: "fill column with 2x before"
        let op = parse_command("fill column 4 with 2x before").unwrap();
        assert_eq!(op, TableOp::FillColumn {
            column: 3,
            formula: Formula::MultiplyPrev(2.0),
        });
    }

    #[test]
    fn fill_with_2x_prev_synonyms() {
        let op = parse_command("fill column 4 with 2x prev").unwrap();
        assert!(matches!(op, TableOp::FillColumn {
            formula: Formula::MultiplyPrev(f), ..
        } if (f - 2.0).abs() < 1e-9));
        let op = parse_command("fill column 4 with 2x previous").unwrap();
        assert!(matches!(op, TableOp::FillColumn {
            formula: Formula::MultiplyPrev(f), ..
        } if (f - 2.0).abs() < 1e-9));
    }

    #[test]
    fn fill_with_3x_prev() {
        let op = parse_command("fill column 2 with 3x prev").unwrap();
        assert!(matches!(op, TableOp::FillColumn {
            formula: Formula::MultiplyPrev(f), ..
        } if (f - 3.0).abs() < 1e-9));
    }

    #[test]
    fn fill_with_sum() {
        let op = parse_command("fill column 5 with sum").unwrap();
        assert_eq!(op, TableOp::FillColumn {
            column: 4,
            formula: Formula::SumRow,
        });
    }

    #[test]
    fn fill_with_constant() {
        let op = parse_command("fill column 2 with REDACTED").unwrap();
        if let TableOp::FillColumn { formula: Formula::Constant(s), .. } = op {
            // Normalized to lowercase since we lowercase the whole
            // input. Acceptable; future ship can preserve case in
            // values by tracking raw spans.
            assert_eq!(s, "redacted");
        } else {
            panic!("expected FillColumn+Constant");
        }
    }

    #[test]
    fn add_with_2x_prev_formula() {
        let op = parse_command("add column doubled with 2x prev").unwrap();
        if let TableOp::AddColumn { name, formula, .. } = op {
            assert_eq!(name, "Doubled");
            assert!(matches!(formula, Some(Formula::MultiplyPrev(f)) if (f - 2.0).abs() < 1e-9));
        } else {
            panic!("expected AddColumn");
        }
    }

    #[test]
    fn unknown_command_errs() {
        let r = parse_command("frobnicate the widget");
        assert!(r.is_err());
        assert!(r.unwrap_err().contains("frobnicate"));
    }

    #[test]
    fn empty_command_errs() {
        let r = parse_command("");
        assert!(r.is_err());
    }

    #[test]
    fn show_no_columns_errs() {
        let r = parse_command("show me");
        assert!(r.is_err());
    }

    #[test]
    fn end_to_end_show_apply() {
        use crate::table_ops::{TableData, TableView};
        let t = TableData::new(
            vec!["A".into(), "B".into(), "C".into(), "D".into()],
            vec![vec!["1".into(), "2".into(), "3".into(), "4".into()]],
        );
        let op = parse_command("show columns 1 3").unwrap();
        let view = TableView { base: t, ops: vec![op] };
        let r = view.apply();
        assert_eq!(r.headers, vec!["A", "C"]);
        assert_eq!(r.rows[0], vec!["1", "3"]);
    }

    #[test]
    fn end_to_end_fill_2x_apply() {
        use crate::table_ops::{TableData, TableView};
        let t = TableData::new(
            vec!["Salary".into(), "Doubled".into()],
            vec![
                vec!["100".into(), "".into()],
                vec!["200".into(), "".into()],
            ],
        );
        let op = parse_command("fill column 2 with 2x before").unwrap();
        let view = TableView { base: t, ops: vec![op] };
        let r = view.apply();
        assert_eq!(r.rows[0][1], "200");
        assert_eq!(r.rows[1][1], "400");
    }

    // ─── parse_meta_command tests (v0.1.200) ───

    #[test]
    fn undo_recognized() {
        assert_eq!(parse_meta_command("undo"), Some(MetaCommand::Undo));
    }

    #[test]
    fn undo_last_recognized() {
        assert_eq!(parse_meta_command("undo last"), Some(MetaCommand::Undo));
    }

    #[test]
    fn undo_last_op_recognized() {
        assert_eq!(parse_meta_command("undo last op"), Some(MetaCommand::Undo));
    }

    #[test]
    fn undo_capital_letters() {
        assert_eq!(parse_meta_command("Undo"), Some(MetaCommand::Undo));
        assert_eq!(parse_meta_command("UNDO"), Some(MetaCommand::Undo));
    }

    #[test]
    fn reset_recognized() {
        assert_eq!(parse_meta_command("reset"), Some(MetaCommand::Reset));
    }

    #[test]
    fn clear_recognized() {
        assert_eq!(parse_meta_command("clear"), Some(MetaCommand::Reset));
        assert_eq!(parse_meta_command("clear all"), Some(MetaCommand::Reset));
    }

    #[test]
    fn show_is_not_meta() {
        // A transformation command shouldn't be parsed as meta.
        assert!(parse_meta_command("show columns 1 4 7").is_none());
    }

    #[test]
    fn empty_is_not_meta() {
        assert!(parse_meta_command("").is_none());
        assert!(parse_meta_command("   ").is_none());
    }

    #[test]
    fn unknown_verb_is_not_meta() {
        assert!(parse_meta_command("frobnicate").is_none());
    }
}
