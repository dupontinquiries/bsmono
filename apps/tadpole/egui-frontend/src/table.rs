//! table — OCR regions that are a table, and the three ways to render them.
//!
//! A detector returns boxes with positions. Whether those boxes are prose or a
//! table is a **geometric** question, and answering it is what lets the app
//! offer a clean markdown table instead of a wall of words in reading order.
//!
//! ## Raw spacing is not a fallback, it is a first-class mode
//!
//! Converting a table to markdown loses information: column widths, alignment,
//! and any cell whose meaning was "blank here, deliberately". For a receipt, a
//! log, or fixed-width source code, **the spacing IS the data** — reflowing it
//! is corruption dressed as tidiness. So `Render::Raw` reconstructs the
//! original column positions with padding and is always available, and the
//! toggle is a first-class control rather than a preference buried in settings.
//!
//! ## Rows and columns come from clustering, not from lines
//!
//! Ruled lines are the obvious signal and the one that fails most: plenty of
//! tables have no rules at all, and a photographed page's rules are often
//! fainter than its text. Clustering the box centres works on both, and it
//! degrades honestly — a layout that is not a table produces one column, which
//! `looks_like_table` then rejects.

use crate::Region;

/// How to present a detected table.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Render {
    /// Pipe-delimited markdown with a header separator.
    Markdown,
    /// Original column positions preserved with spaces.
    Raw,
    /// Reading order, one region per line — what the app does for prose.
    Lines,
}

/// A table recovered from regions.
#[derive(Clone, Debug, PartialEq)]
pub struct Table {
    /// Row-major cells. Missing cells are empty strings, never dropped: a gap
    /// in a table is information, and shifting later cells left to close it
    /// silently moves data into the wrong column.
    pub rows: Vec<Vec<String>>,
    /// Left edge of each column, in image pixels — what `Raw` rebuilds from.
    pub column_x: Vec<f32>,
}

impl Table {
    pub fn n_cols(&self) -> usize { self.column_x.len() }
    pub fn n_rows(&self) -> usize { self.rows.len() }

    /// Is this actually tabular?
    ///
    /// Two columns and two rows minimum. One column is a list, and one row is a
    /// heading — calling either a table would offer a markdown conversion that
    /// makes them worse.
    pub fn looks_like_table(&self) -> bool {
        self.n_cols() >= 2 && self.n_rows() >= 2
    }
}

/// Group regions into rows and columns.
///
/// `tolerance_scale` multiplies the median text height to decide what counts as
/// "the same line" and "the same column". Larger is more forgiving of skew.
pub fn detect(regions: &[Region], tolerance_scale: f32) -> Table {
    if regions.is_empty() {
        return Table { rows: Vec::new(), column_x: Vec::new() };
    }
    let mut heights: Vec<f32> = regions.iter()
        .map(|r| { let (_, y0, _, y1) = r.bounds(); (y1 - y0).max(1.0) })
        .collect();
    heights.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    // Median rather than mean: one enormous heading would drag a mean up and
    // merge every real row into one.
    let unit = heights[heights.len() / 2];
    let row_tol = unit * tolerance_scale;
    let col_tol = unit * tolerance_scale * 1.5;

    let mut idx: Vec<usize> = (0..regions.len()).collect();
    let centre_y = |i: usize| { let (_, y0, _, y1) = regions[i].bounds(); (y0 + y1) * 0.5 };
    let left_x = |i: usize| regions[i].bounds().0;

    idx.sort_by(|&a, &b| centre_y(a).partial_cmp(&centre_y(b)).unwrap());

    // Rows: consecutive regions whose centres are within row_tol.
    let mut row_groups: Vec<Vec<usize>> = Vec::new();
    for i in idx {
        match row_groups.last_mut() {
            Some(g) if (centre_y(i) - centre_y(g[0])).abs() <= row_tol => g.push(i),
            _ => row_groups.push(vec![i]),
        }
    }

    // Columns: cluster every region's left edge across the whole page, so a row
    // missing a cell still lines up with the rows that have one. Clustering per
    // row would put the second cell of a short row in column two regardless.
    let mut lefts: Vec<f32> = row_groups.iter().flatten().map(|&i| left_x(i)).collect();
    lefts.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let mut column_x: Vec<f32> = Vec::new();
    for x in lefts {
        match column_x.last() {
            Some(&c) if (x - c).abs() <= col_tol => {}
            _ => column_x.push(x),
        }
    }

    let col_of = |x: f32| -> usize {
        column_x.iter().enumerate()
            .min_by(|a, b| (a.1 - x).abs().partial_cmp(&(b.1 - x).abs()).unwrap())
            .map(|(i, _)| i).unwrap_or(0)
    };

    let mut rows = Vec::new();
    for g in &row_groups {
        let mut cells = vec![String::new(); column_x.len()];
        let mut ordered = g.clone();
        ordered.sort_by(|&a, &b| left_x(a).partial_cmp(&left_x(b)).unwrap());
        for i in ordered {
            let c = col_of(left_x(i));
            // Two regions landing in one cell are joined with a space rather
            // than one overwriting the other — a wrapped cell is common and
            // losing half of it is worse than a slightly long cell.
            if cells[c].is_empty() { cells[c] = regions[i].text.clone(); }
            else { cells[c].push(' '); cells[c].push_str(&regions[i].text); }
        }
        rows.push(cells);
    }
    Table { rows, column_x }
}

/// Render a table as markdown.
///
/// The first row becomes the header, because that is what markdown requires and
/// what an OCR'd table almost always has. When it does not, the user has `Raw`.
pub fn to_markdown(t: &Table) -> String {
    if t.rows.is_empty() { return String::new(); }
    let mut out = String::new();
    let width = t.n_cols();
    let esc = |s: &str| s.replace('|', "\\|");
    for (n, row) in t.rows.iter().enumerate() {
        out.push('|');
        for c in 0..width {
            out.push(' ');
            out.push_str(&esc(row.get(c).map(String::as_str).unwrap_or("")));
            out.push_str(" |");
        }
        out.push('\n');
        if n == 0 {
            out.push('|');
            for _ in 0..width { out.push_str(" --- |"); }
            out.push('\n');
        }
    }
    out
}

/// Render with the original spacing reconstructed.
///
/// Columns are placed at their measured pixel offsets scaled to characters, so
/// a receipt still lines up and a fixed-width block still reads as one. A cell
/// that would overflow its column pushes the rest right rather than being
/// truncated — losing characters to preserve a grid is the wrong trade when the
/// text is the point.
pub fn to_raw(t: &Table, px_per_char: f32) -> String {
    let mut out = String::new();
    let starts: Vec<usize> = t.column_x.iter()
        .map(|x| ((x - t.column_x.first().copied().unwrap_or(0.0)) / px_per_char.max(1.0))
            .round().max(0.0) as usize)
        .collect();
    for row in &t.rows {
        let mut line = String::new();
        for (c, cell) in row.iter().enumerate() {
            if cell.is_empty() { continue; }
            let want = starts.get(c).copied().unwrap_or(0);
            while line.chars().count() < want { line.push(' '); }
            if !line.is_empty() && line.chars().count() >= want { line.push(' '); }
            line.push_str(cell);
        }
        out.push_str(line.trim_end());
        out.push('\n');
    }
    out
}

/// Is this page a diagram rather than prose or a table?
///
/// Deliberately conservative — it looks for the arrow tokens a niftyplot or
/// mermaid source uses. Guessing "diagram" from geometry alone would turn every
/// scattered layout into a broken graph, and a wrong diagram is far more
/// confusing than plain text.
pub fn looks_like_diagram(text: &str) -> bool {
    let arrows = ["-->", "->", "==>", "---"];
    let hits = text.lines()
        .filter(|l| arrows.iter().any(|a| l.contains(a)))
        .count();
    hits >= 2
}

/// Parse OCR'd text as a niftyplot diagram.
///
/// Returns the document only when it actually found nodes: the parser is
/// permissive by design — unknown directives become warnings rather than errors
/// — so it will happily return an empty document for prose, and offering a
/// blank canvas would be worse than offering nothing.
pub fn as_diagram(text: &str) -> Option<nifty_core::niftyplot::GraphDoc> {
    if !looks_like_diagram(text) { return None; }
    let doc = nifty_core::niftyplot::parse(text);
    if doc.nodes.is_empty() { None } else { Some(doc) }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(text: &str, x: f32, y: f32, w: f32) -> Region {
        Region { text: text.into(), score: 0.9,
                 corners: [(x, y), (x + w, y), (x + w, y + 20.0), (x, y + 20.0)] }
    }

    /// A three-column receipt with a wobbly baseline, which is what a photo of
    /// one actually produces.
    fn receipt() -> Vec<Region> {
        vec![
            r("Item", 10.0, 10.0, 40.0), r("Qty", 200.0, 12.0, 30.0), r("Price", 300.0, 11.0, 45.0),
            r("Coffee", 10.0, 50.0, 55.0), r("2", 200.0, 51.0, 12.0), r("4.00", 300.0, 49.0, 38.0),
            r("Tea", 10.0, 90.0, 30.0), r("1", 200.0, 91.0, 12.0), r("2.50", 300.0, 90.0, 38.0),
        ]
    }

    #[test]
    fn a_wobbly_baseline_still_groups_into_rows() {
        // Photographed rows are never level. Row grouping by centre with a
        // tolerance is what makes this work where a strict y-equality does not.
        let t = detect(&receipt(), 0.6);
        assert_eq!(t.n_rows(), 3, "three rows: {:?}", t.rows);
        assert_eq!(t.n_cols(), 3);
        assert!(t.looks_like_table());
    }

    #[test]
    fn a_missing_cell_leaves_a_gap_rather_than_shifting_the_row() {
        // The failure that silently corrupts data: closing a gap moves every
        // later cell into the wrong column, so a price lands under Qty.
        let mut regs = receipt();
        regs.retain(|x| x.text != "1");                 // Tea has no quantity
        let t = detect(&regs, 0.6);
        let tea = t.rows.iter().find(|row| row[0] == "Tea").expect("Tea row");
        assert_eq!(tea[1], "", "the gap stays a gap");
        assert_eq!(tea[2], "2.50", "and the price stays in the price column");
    }

    #[test]
    fn a_paragraph_is_not_a_table() {
        // One column is a list and one row is a heading. Offering a markdown
        // conversion for either makes it worse.
        let prose = vec![r("The quick brown fox", 10.0, 10.0, 200.0),
                         r("jumps over the lazy dog", 10.0, 40.0, 220.0)];
        assert!(!detect(&prose, 0.6).looks_like_table());
        let heading = vec![r("A", 10.0, 10.0, 20.0), r("B", 200.0, 10.0, 20.0)];
        assert!(!detect(&heading, 0.6).looks_like_table(), "one row is a heading");
    }

    #[test]
    fn markdown_has_a_header_separator_and_escapes_pipes() {
        // A cell containing a pipe would otherwise split into two columns and
        // shift the entire rest of the row.
        let mut regs = receipt();
        regs[3] = r("Coffee | large", 10.0, 50.0, 55.0);
        let md = to_markdown(&detect(&regs, 0.6));
        assert!(md.lines().nth(1).unwrap().contains("---"), "header separator");
        assert!(md.contains("Coffee \\| large"), "pipe escaped: {md}");
    }

    #[test]
    fn raw_keeps_the_columns_where_they_were() {
        // The mode that exists because reflowing a receipt is corruption
        // dressed as tidiness.
        let raw = to_raw(&detect(&receipt(), 0.6), 9.0);
        let lines: Vec<&str> = raw.lines().collect();
        assert_eq!(lines.len(), 3);
        let qty_col = lines[1].find('2').expect("quantity on the Coffee line");
        assert!(qty_col > 10, "the second column is indented, not reflowed: {:?}", lines[1]);
        assert!(lines[1].contains("Coffee") && lines[1].contains("4.00"));
    }

    #[test]
    fn raw_does_not_truncate_a_cell_that_overflows_its_column() {
        // Losing characters to preserve a grid is the wrong trade when the text
        // is the point.
        let regs = vec![
            r("an extremely long first cell", 10.0, 10.0, 260.0), r("x", 300.0, 10.0, 12.0),
            r("b", 10.0, 50.0, 12.0), r("y", 300.0, 50.0, 12.0),
        ];
        let raw = to_raw(&detect(&regs, 0.6), 9.0);
        assert!(raw.contains("an extremely long first cell"));
        assert!(raw.contains('x'), "the overflowed neighbour survives too");
    }

    #[test]
    fn two_words_inside_one_cell_are_joined_rather_than_half_lost() {
        // A detector splitting "very hot" into two boxes a few pixels apart is
        // ordinary; keeping one and dropping the other loses half the cell with
        // no sign it happened.
        //
        // My first version of this test put the second word 40px right, which
        // is a genuinely separate COLUMN — the detector was right and the test
        // was wrong. The distinction is the column tolerance, and it is the
        // thing that makes the join safe: words share a cell, columns do not.
        let regs = vec![
            r("Item", 10.0, 10.0, 40.0), r("Note", 200.0, 10.0, 40.0),
            r("Coffee", 10.0, 50.0, 55.0), r("very", 200.0, 50.0, 35.0),
            r("hot", 208.0, 51.0, 25.0),
        ];
        let t = detect(&regs, 0.6);
        let row = t.rows.iter().find(|r| r[0] == "Coffee").expect("row");
        assert_eq!(row[1], "very hot");
    }

    #[test]
    fn a_word_far_enough_right_is_a_column_not_a_continuation() {
        // The other side of the same rule, so a change to the tolerance cannot
        // quietly turn every table into two columns of run-on text.
        let regs = vec![
            r("A", 10.0, 10.0, 20.0), r("B", 200.0, 10.0, 20.0), r("C", 300.0, 10.0, 20.0),
            r("a", 10.0, 50.0, 20.0), r("b", 200.0, 50.0, 20.0), r("c", 300.0, 50.0, 20.0),
        ];
        assert_eq!(detect(&regs, 0.6).n_cols(), 3);
    }

    #[test]
    fn an_empty_page_produces_an_empty_table_rather_than_panicking() {
        let t = detect(&[], 0.6);
        assert_eq!(t.n_rows(), 0);
        assert!(!t.looks_like_table());
        assert_eq!(to_markdown(&t), "");
        assert_eq!(to_raw(&t, 9.0), "");
    }

    #[test]
    fn a_giant_heading_does_not_merge_every_row() {
        // The reason the unit is a MEDIAN height rather than a mean: one large
        // heading would drag a mean up until the whole page is one row.
        let mut regs = receipt();
        regs.push(Region { text: "RECEIPT".into(), score: 0.9,
                           corners: [(10.0, -80.0), (300.0, -80.0), (300.0, 0.0), (10.0, 0.0)] });
        assert!(detect(&regs, 0.6).n_rows() >= 3, "rows survived the heading");
    }

    #[test]
    fn a_diagram_is_recognised_only_from_explicit_arrows() {
        // Guessing "diagram" from geometry would turn every scattered layout
        // into a broken graph, and a wrong diagram is more confusing than text.
        assert!(looks_like_diagram("a --> b\nb --> c"));
        assert!(!looks_like_diagram("The quick brown fox\njumps over the dog"));
        assert!(!looks_like_diagram("one --> two"), "a single arrow is not enough");
    }

    #[test]
    fn prose_never_parses_as_a_diagram() {
        // The parser is permissive by design and returns an empty document for
        // anything; offering a blank canvas is worse than offering nothing.
        assert!(as_diagram("just some words\nand more words").is_none());
    }
}
