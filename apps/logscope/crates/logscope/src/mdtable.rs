//! mdtable.rs — recovering a table from output that nearly is one.
//!
//! A small model asked for a table produces something table-shaped and not quite valid. From a real
//! Qwen transcript:
//!
//! ```text
//! | Feature | Status |
//! |-------|--------|
//! ✅ Two players control turtles via arrow keys | ✔️ |
//! ```
//!
//! **The rows have lost their leading pipe.** A strict parser renders that as three lines of prose,
//! which is worse than useless — the model conveyed a structure and the reader is shown its debris.
//!
//! ## Leniency is the requirement, not a nicety
//!
//! Every relaxation here corresponds to something an actual model actually emitted:
//!
//! * a missing leading or trailing pipe
//! * a separator row whose dashes do not match the column count
//! * a row with fewer cells than the header
//! * a row with MORE cells, from an unescaped pipe inside a cell
//!
//! A strict parser is correct and useless; the interesting question is how far to bend before a
//! paragraph containing one pipe gets mangled into a table.

/// A recovered table.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Table {
    pub header: Vec<String>,
    pub rows: Vec<Vec<String>>,
    /// What had to be repaired, so a renderer can show the table and still be honest that the source
    /// was malformed.
    pub repairs: Vec<String>,
}

/// Is this line the `|---|---|` separator?
///
/// Accepts colons for alignment and tolerates a wrong dash count, because the dash count carries no
/// information a reader can use and models get it wrong constantly.
fn is_separator(line: &str) -> bool {
    let t = line.trim().trim_matches('|');
    !t.is_empty() && t.split('|').all(|c| {
        let c = c.trim();
        !c.is_empty() && c.chars().all(|ch| ch == '-' || ch == ':')
    })
}

/// Split a row into cells, tolerating a missing pipe at either end.
fn cells(line: &str) -> Vec<String> {
    let t = line.trim();
    let t = t.strip_prefix('|').unwrap_or(t);
    let t = t.strip_suffix('|').unwrap_or(t);
    t.split('|').map(|c| c.trim().to_string()).collect()
}

/// Below this many pipes a line is prose that happens to contain one.
///
/// **The guard against over-eagerness.** A leniency that turns "use a | b" into a two-column table
/// is worse than the strictness it replaced, because it damages text that was fine.
const MIN_CELLS: usize = 2;

/// Try to read a table starting at `lines[0]`.
///
/// Returns None if this is not table-shaped — which must stay the common answer, since most text is
/// not a table.
pub fn parse(lines: &[&str]) -> Option<Table> {
    if lines.len() < 3 { return None }
    let header = cells(lines[0]);
    if header.len() < MIN_CELLS { return None }
    // The separator is what distinguishes a table from a line with pipes in it. Without one this is
    // prose, however many pipes it has.
    if !is_separator(lines[1]) { return None }

    let mut repairs = Vec::new();
    if !lines[0].trim().starts_with('|') {
        repairs.push("header was missing its leading pipe".into());
    }
    let sep_cols = cells(lines[1]).len();
    if sep_cols != header.len() {
        repairs.push(format!("separator declared {sep_cols} columns, header has {}", header.len()));
    }

    let mut rows = Vec::new();
    for (i, l) in lines[2..].iter().enumerate() {
        let t = l.trim();
        // A blank line or a line with no pipe at all ends the table. Models routinely follow a table
        // with prose and no separating blank.
        if t.is_empty() || !t.contains('|') { break }
        let mut c = cells(l);
        if !t.starts_with('|') {
            repairs.push(format!("row {} was missing its leading pipe", i + 1));
        }
        // PAD rather than drop. A short row means the model omitted a cell, and showing the row with
        // a gap conveys more than hiding it — the reader can see what is missing.
        if c.len() < header.len() {
            repairs.push(format!("row {} had {} cells, padded to {}", i + 1, c.len(), header.len()));
            c.resize(header.len(), String::new());
        }
        // MERGE the overflow into the last cell rather than truncating. Extra cells nearly always
        // come from an unescaped pipe inside a cell, so the text belongs together and dropping it
        // loses content the model meant to convey.
        if c.len() > header.len() {
            repairs.push(format!("row {} had {} cells; the surplus was an unescaped pipe and was \
merged back into the last cell", i + 1, c.len()));
            let tail = c.split_off(header.len() - 1).join(" | ");
            c.push(tail);
        }
        rows.push(c);
    }
    if rows.is_empty() { return None }
    Some(Table { header, rows, repairs })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_REAL_QWEN_OUTPUT_is_recovered() {
        // Rows missing their leading pipe — a strict parser shows three lines of debris where the
        // model conveyed a structure.
        let t = parse(&[
            "| Feature | Status |",
            "|-------|--------|",
            "✅ Two players control turtles via arrow keys | ✔️ |",
            "✅ Turtles move toward center | ✔️ |",
        ]).unwrap();
        assert_eq!(t.header, vec!["Feature", "Status"]);
        assert_eq!(t.rows.len(), 2);
        assert_eq!(t.rows[0][1], "✔️");
        assert!(t.repairs.iter().any(|r| r.contains("missing its leading pipe")));
    }

    #[test]
    fn PROSE_with_a_pipe_is_not_turned_into_a_table() {
        // The guard against over-eagerness: a leniency that mangles fine text is worse than the
        // strictness it replaced.
        assert!(parse(&["use a | b", "then c", "and d"]).is_none());
    }

    #[test]
    fn the_SEPARATOR_is_what_makes_it_a_table() {
        // Without one this is prose, however many pipes it has.
        assert!(parse(&["| a | b |", "| c | d |", "| e | f |"]).is_none());
    }

    #[test]
    fn a_MISMATCHED_separator_is_tolerated_and_noted() {
        // The dash count carries no information a reader can use and models get it wrong constantly.
        let t = parse(&["| a | b | c |", "|---|---|", "| 1 | 2 | 3 |"]).unwrap();
        assert_eq!(t.header.len(), 3);
        assert!(t.repairs.iter().any(|r| r.contains("separator declared 2")));
    }

    #[test]
    fn a_SHORT_row_is_PADDED_not_dropped() {
        // The model omitted a cell; showing the row with a gap conveys more than hiding it, because
        // the reader can see what is missing.
        let t = parse(&["| a | b | c |", "|---|---|---|", "| 1 | 2 |"]).unwrap();
        assert_eq!(t.rows[0], vec!["1", "2", ""]);
        assert!(t.repairs.iter().any(|r| r.contains("padded")));
    }

    #[test]
    fn an_OVERLONG_row_MERGES_rather_than_truncating() {
        // Extra cells nearly always come from an unescaped pipe inside a cell, so the text belongs
        // together and dropping it loses content the model meant to convey.
        let t = parse(&["| a | b |", "|---|---|", "| 1 | x | y |"]).unwrap();
        assert_eq!(t.rows[0], vec!["1", "x | y"]);
        assert!(t.repairs.iter().any(|r| r.contains("unescaped pipe")));
    }

    #[test]
    fn the_table_ENDS_at_the_first_non_row() {
        // Models routinely follow a table with prose and no separating blank line.
        let t = parse(&["| a | b |", "|---|---|", "| 1 | 2 |", "Next steps:"]).unwrap();
        assert_eq!(t.rows.len(), 1);
    }

    #[test]
    fn a_WELL_FORMED_table_needs_no_repairs() {
        // The repairs list must stay empty for valid input, or a renderer showing it cries wolf.
        let t = parse(&["| a | b |", "|---|---|", "| 1 | 2 |"]).unwrap();
        assert!(t.repairs.is_empty(), "{:?}", t.repairs);
    }
}
