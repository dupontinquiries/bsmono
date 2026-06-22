//! v0.1.611: pure selection-range extraction. The row/column math that
//! decides WHICH text a (r1,c1)-(r2,c2) selection covers used to live
//! inline in Backend::term_selection_text, tangled with pty.render_row,
//! so it couldn't be unit-tested — and the selection off-by-one arc
//! (TODO_selection_off_by_one) is exactly a range-math question. This
//! extracts that logic to operate on already-rendered row strings, so
//! the normalize/clamp/first-last-middle-row/inclusive-end behavior is
//! testable in isolation. term_selection_text now delegates here.
//!
//! Coordinates: rows and cols are 0-based screen positions. The end
//! column is INCLUSIVE (matches how a drag-selection reports the cell
//! under the cursor as part of the selection).

/// Extract the selected text from the given rendered rows. `rows[i]` is
/// the already-trimmed text of screen row i (as pty.render_row returns).
/// Mirrors the original term_selection_text semantics exactly:
///   - normalize so (r1,c1) <= (r2,c2)
///   - clamp negatives to 0
///   - first row of a multi-row sel: c_start..end-of-row
///   - last row: 0..=c_end
///   - middle rows: whole row
///   - single-row sel: c_start..=c_end
///   - rows joined with '\n'
pub fn extract_selection(rows: &[String], r1: i32, c1: i32, r2: i32, c2: i32) -> String {
    let (mut sr, mut sc, mut er, mut ec) = (r1, c1, r2, c2);
    if sr > er || (sr == er && sc > ec) {
        std::mem::swap(&mut sr, &mut er);
        std::mem::swap(&mut sc, &mut ec);
    }
    if sr < 0 { sr = 0; }
    if sc < 0 { sc = 0; }
    if er < 0 { er = 0; }
    if ec < 0 { ec = 0; }

    // v0.1.631: clamp the row range to the rows that actually exist.
    // Without this, a selection that overshoots the bottom of the grid
    // (e.g. a drag past the last line, or an end row past the slice)
    // iterates sr..=er over nonexistent rows and appends an empty string
    // + '\n' for each, producing a pile of trailing newlines in the
    // copied text. highlight_row_span already clamps the HIGHLIGHT to the
    // last real row (line ~85); this makes the copied TEXT match. Probe-
    // found: extract_selection(&3rows, 0,0, 9,5) was returning 6 stray
    // trailing newlines; an empty rows slice returned "\n".
    if rows.is_empty() {
        return String::new();
    }
    let last_row = (rows.len() - 1) as i32;
    if sr > last_row {
        return String::new(); // selection starts past the grid
    }
    if er > last_row {
        er = last_row;
    }

    let mut out = String::new();
    for row_idx in sr..=er {
        let row_text = rows.get(row_idx as usize).map(|s| s.as_str()).unwrap_or("");
        let (col_start, col_end_inclusive) = if sr == er {
            (sc as usize, ec as usize)
        } else if row_idx == sr {
            (sc as usize, usize::MAX)
        } else if row_idx == er {
            (0, ec as usize)
        } else {
            (0, usize::MAX)
        };
        let chars: Vec<char> = row_text.chars().collect();
        let len = chars.len();
        let cs = col_start.min(len);
        let ce = (col_end_inclusive.saturating_add(1)).min(len);
        if cs < ce {
            let slice: String = chars[cs..ce].iter().collect();
            out.push_str(&slice);
        }
        if row_idx < er {
            out.push('\n');
        }
    }
    out
}

/// v0.1.611: the row range a selection should HIGHLIGHT, as an inclusive
/// (first_row, last_row). This is the geometry the off-by-one arc is
/// about: the highlight must cover exactly the rows the selection spans,
/// not one extra. Separated out so the highlight renderer and the text
/// extractor agree on the SAME range (the arc's evidence was that the
/// highlight and the extracted text disagreed by one row).
///
/// Returns None if, after normalization+clamp, there's nothing to
/// highlight (shouldn't happen for a real selection, but keeps callers
/// from rendering a bogus row).
pub fn highlight_row_span(r1: i32, r2: i32, total_rows: usize) -> Option<(usize, usize)> {
    if total_rows == 0 {
        return None;
    }
    let mut a = r1.max(0) as usize;
    let mut b = r2.max(0) as usize;
    if a > b {
        std::mem::swap(&mut a, &mut b);
    }
    let last = total_rows - 1;
    if a > last {
        return None; // selection starts past the screen → nothing visible
    }
    let b = b.min(last); // clamp the end to the last real row — NOT last+1
    Some((a, b))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn single_row_inclusive() {
        let r = rows(&["hello world"]);
        // cols 0..=4 inclusive → "hello"
        assert_eq!(extract_selection(&r, 0, 0, 0, 4), "hello");
    }

    #[test]
    fn single_char_selection() {
        let r = rows(&["abc"]);
        // col 1..=1 → "b" (the inclusive-end must include exactly one char)
        assert_eq!(extract_selection(&r, 0, 1, 0, 1), "b");
    }

    #[test]
    fn multi_row_first_to_last() {
        let r = rows(&["aaaa", "bbbb", "cccc"]);
        // row0 from col2 to end, row1 whole, row2 col0..=1
        assert_eq!(extract_selection(&r, 0, 2, 2, 1), "aa\nbbbb\ncc");
    }

    #[test]
    fn normalize_reversed_selection() {
        let r = rows(&["hello world"]);
        // reversed coords should give the same as forward
        assert_eq!(extract_selection(&r, 0, 4, 0, 0), "hello");
    }

    #[test]
    fn full_three_lines_no_extra() {
        // The arc's core complaint: selecting 3 lines must yield exactly
        // 3 lines of text, not 4. A whole-line V-select is r=0..2.
        let r = rows(&["line one", "line two", "line three", "line four"]);
        let out = extract_selection(&r, 0, 0, 2, 1000);
        assert_eq!(out.lines().count(), 3, "exactly 3 lines, got {:?}", out);
        assert!(out.contains("line three"));
        assert!(!out.contains("line four"), "must NOT include the 4th line");
    }

    #[test]
    fn out_of_range_rows_dont_panic() {
        let r = rows(&["only row"]);
        // selecting rows 5..=7 (past the end) → no panic. Faithful to
        // the original: empty rows still emit their '\n' separators, so
        // the result is newlines (not a panic, not real text).
        let out = extract_selection(&r, 5, 0, 7, 3);
        assert!(out.chars().all(|c| c == '\n'), "no real text, got {:?}", out);
    }

    #[test]
    fn col_past_row_length_clamps() {
        let r = rows(&["hi"]);
        // col_end way past the 2-char row → just "hi", no panic
        assert_eq!(extract_selection(&r, 0, 0, 0, 99), "hi");
    }

    // ── highlight_row_span: the geometry the off-by-one is about ──

    #[test]
    fn highlight_span_inclusive() {
        // selecting rows 2..=4 highlights exactly rows 2,3,4
        assert_eq!(highlight_row_span(2, 4, 24), Some((2, 4)));
    }

    #[test]
    fn highlight_span_single_row() {
        assert_eq!(highlight_row_span(3, 3, 24), Some((3, 3)));
    }

    #[test]
    fn highlight_span_clamps_end_to_last_row_not_beyond() {
        // r2 past the screen must clamp to the LAST real row (23), not
        // 24 — emitting row 24 is exactly the "one extra line" bug.
        assert_eq!(highlight_row_span(20, 99, 24), Some((20, 23)));
    }

    #[test]
    fn highlight_span_start_past_screen_is_none() {
        assert_eq!(highlight_row_span(30, 35, 24), None);
    }

    #[test]
    fn highlight_span_reversed_normalizes() {
        assert_eq!(highlight_row_span(5, 1, 24), Some((1, 5)));
    }

    #[test]
    fn highlight_span_empty_screen_is_none() {
        assert_eq!(highlight_row_span(0, 3, 0), None);
    }

    #[test]
    fn extract_clamps_end_row_past_grid_no_trailing_newlines() {
        // v0.1.631 probe fix: end row past the actual rows must NOT append
        // a blank line + '\n' per nonexistent row.
        let r = rows(&["hello world", "second line", "third"]);
        let got = extract_selection(&r, 0, 0, 9, 5);
        assert_eq!(got, "hello world\nsecond line\nthird");
        assert!(!got.ends_with('\n'), "must not have trailing newlines");
    }

    #[test]
    fn extract_empty_rows_slice_is_empty_string() {
        // was returning "\n" before the clamp.
        assert_eq!(extract_selection(&[], 0, 0, 1, 1), "");
    }

    #[test]
    fn extract_start_past_grid_is_empty() {
        let r = rows(&["only line"]);
        assert_eq!(extract_selection(&r, 5, 0, 9, 0), "");
    }
}
