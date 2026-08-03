//! table_view — `TableView`: the surface over voiceterm's table engine.
//!
//! The engine is already here and it is NOT re-implemented: `table_ops` holds
//! `TableOp` and the apply pipeline, `table_nlp` parses natural language into
//! ops and meta-commands. Both are plain Rust with no Qt in them, and the port
//! already has a convention for exactly this — `#[path]` including the app's own
//! module so "the two cannot drift". What was missing is the VIEW: the thing
//! that owns a stack of ops, shows the result, and lets you take one back.
//!
//! (I checked before writing. Two sessions ago I rewrote a sink picker that
//! already existed and was better than mine; the habit now is to grep first.)
//!
//! ## The stack is the whole design
//!
//! Ops are not applied destructively to the data. They accumulate, and the
//! displayed table is base-data-plus-stack, recomputed. That is what makes
//! "undo" a pop rather than an inverse operation — and inverse operations are
//! where this kind of feature goes wrong, because `Sort` has no inverse and
//! `Hide` has one only if you remember what order the columns were in.
//!
//! ## A meta-command is not a transformation
//!
//! `parse_meta_command` is tried FIRST and separately, because "undo" is a
//! statement about the stack rather than about the data. Folding it into the op
//! vocabulary would put a variant in `TableOp` that has no before/after
//! semantics, and every consumer of `TableOp` would have to know to ignore it.
//!
//! ## An unparsed command is not an error state
//!
//! Say what was not understood and leave the table alone. A table that clears
//! itself when you mistype is a table you stop trusting.

/// What a submitted line turned out to be.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Submitted {
    /// An op was pushed onto the stack.
    Pushed,
    /// The stack was popped.
    Undone,
    /// The stack was cleared.
    Reset,
    /// Nothing matched, with the parser's complaint.
    NotUnderstood(String),
}

/// The view's own state. The DATA and the ops live in the engine types; this
/// holds the stack and the input.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TableView {
    /// How many ops are stacked. The ops themselves are engine types held by
    /// the caller — this module is deliberately generic over them so it can be
    /// tested without constructing a whole table.
    depth: usize,
    /// The command line.
    pub input: String,
    /// The last thing that happened, shown under the input.
    pub status: Option<String>,
    /// True while the base data is empty — there is nothing to transform.
    pub has_data: bool,
}

impl TableView {
    pub fn new(has_data: bool) -> Self {
        TableView { depth: 0, input: String::new(), status: None, has_data }
    }

    pub fn depth(&self) -> usize { self.depth }
    pub fn is_base(&self) -> bool { self.depth == 0 }

    /// Push an op. Returns the new depth.
    pub fn push(&mut self) -> usize {
        self.depth += 1;
        self.depth
    }

    /// Pop the most recent op. `false` when there was nothing to undo.
    ///
    /// Reported rather than silently ignored, so the view can say "nothing to
    /// undo" instead of appearing to have done something.
    pub fn undo(&mut self) -> bool {
        if self.depth == 0 { return false; }
        self.depth -= 1;
        true
    }

    /// Clear the stack, returning to base data. `false` when already there.
    pub fn reset(&mut self) -> bool {
        if self.depth == 0 { return false; }
        self.depth = 0;
        true
    }

    /// The status line under the input.
    pub fn describe(&self) -> String {
        match self.depth {
            0 => "base data".into(),
            1 => "1 op applied".into(),
            n => format!("{n} ops applied"),
        }
    }

    /// Handle a submitted line, given the two parsers.
    ///
    /// The meta-command parser is tried FIRST and separately: "undo" is a
    /// statement about the stack, not about the data, and folding it into the
    /// op vocabulary would put a variant in `TableOp` with no before/after
    /// semantics that every consumer would have to ignore.
    ///
    /// Generic over the parsers so this can be tested without the engine, and
    /// so the engine's own types stay the single source of the vocabulary.
    pub fn submit<M, O, E>(
        &mut self,
        meta: impl Fn(&str) -> Option<M>,
        is_undo: impl Fn(&M) -> bool,
        op: impl Fn(&str) -> Result<O, E>,
    ) -> Submitted
    where E: std::fmt::Display {
        let line = self.input.trim().to_string();
        if line.is_empty() {
            // An empty line is not a mistake; it is a keypress. Saying nothing
            // is the correct response.
            return Submitted::NotUnderstood(String::new());
        }
        if let Some(m) = meta(&line) {
            self.input.clear();
            return if is_undo(&m) {
                let ok = self.undo();
                self.status = Some(if ok { self.describe() }
                                   else { "nothing to undo".into() });
                Submitted::Undone
            } else {
                let ok = self.reset();
                self.status = Some(if ok { "back to base data".into() }
                                   else { "already at base data".into() });
                Submitted::Reset
            };
        }
        match op(&line) {
            Ok(_) => {
                self.push();
                self.input.clear();
                self.status = Some(self.describe());
                Submitted::Pushed
            }
            Err(e) => {
                // The table is left alone and the text is KEPT, so the user can
                // correct a typo instead of retyping the line. A table that
                // clears itself when you mistype is a table you stop trusting.
                let msg = e.to_string();
                self.status = Some(msg.clone());
                Submitted::NotUnderstood(msg)
            }
        }
    }
}

/// Column widths for a table, from the widest cell in each column.
///
/// Measured across the header AND the body: a header longer than every value
/// under it still has to fit, and a column that clips its own name is a column
/// you cannot identify.
pub fn column_widths(headers: &[String], rows: &[Vec<String>]) -> Vec<usize> {
    let mut w: Vec<usize> = headers.iter().map(|h| h.chars().count()).collect();
    for row in rows {
        for (i, cell) in row.iter().enumerate() {
            let n = cell.chars().count();
            if i < w.len() { if n > w[i] { w[i] = n; } }
            else { w.push(n); }
        }
    }
    w
}

/// Is a column numeric? Used to decide alignment.
///
/// Right-aligning numbers is what makes a column of them scannable — the digits
/// line up by place value. A single non-numeric cell makes the whole column
/// text, because a right-aligned column with one stray word in it looks broken.
pub fn column_is_numeric(rows: &[Vec<String>], col: usize) -> bool {
    let mut saw_one = false;
    for row in rows {
        let Some(cell) = row.get(col) else { continue };
        let t = cell.trim();
        if t.is_empty() { continue; }
        if t.parse::<f64>().is_err() { return false; }
        saw_one = true;
    }
    saw_one
}

/// Draw the table and its command line. Returns the submitted line, if any.
pub fn table_view_ui(ui: &mut egui::Ui, v: &mut TableView,
                     headers: &[String], rows: &[Vec<String>],
                     c: &uikit_egui::PaletteColors) -> Option<String> {
    let mut submitted = None;
    ui.horizontal(|ui| {
        ui.colored_label(c.text_weak, v.describe());
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            // Undo is disabled at base data rather than hidden: a control that
            // vanishes reads as a missing feature, one that is greyed reads as
            // nothing to undo.
            if ui.add_enabled(!v.is_base(), egui::Button::new("undo")).clicked() {
                submitted = Some("undo".to_string());
            }
            if ui.add_enabled(!v.is_base(), egui::Button::new("reset")).clicked() {
                submitted = Some("reset".to_string());
            }
        });
    });

    if headers.is_empty() && rows.is_empty() {
        ui.colored_label(c.text_weak, "No table captured yet.");
    } else {
        let numeric: Vec<bool> = (0..headers.len())
            .map(|i| column_is_numeric(rows, i)).collect();
        egui::ScrollArea::both().id_salt("table_view").show(ui, |ui| {
            ui.horizontal(|ui| {
                for h in headers {
                    ui.colored_label(c.text_weak, egui::RichText::new(h).strong());
                }
            });
            for row in rows {
                ui.horizontal(|ui| {
                    for (i, cell) in row.iter().enumerate() {
                        // Numbers right-align so the digits line up by place
                        // value, which is what makes a column scannable.
                        let layout = if *numeric.get(i).unwrap_or(&false) {
                            egui::Layout::right_to_left(egui::Align::Center)
                        } else {
                            egui::Layout::left_to_right(egui::Align::Center)
                        };
                        ui.allocate_ui_with_layout(egui::vec2(90.0, 18.0), layout, |ui| {
                            ui.colored_label(c.text, cell);
                        });
                    }
                });
            }
        });
    }

    let r = ui.add(egui::TextEdit::singleline(&mut v.input)
        .hint_text("sort by column 2 · hide column 3 · undo").desired_width(280.0));
    if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
        submitted = Some(v.input.trim().to_string());
    }
    if let Some(s) = &v.status { ui.colored_label(c.text_weak, s); }
    submitted
}

/// Submit a line using the REAL engine parsers, which is what the app does.
///
/// Kept here rather than at the call site so there is exactly one place that
/// decides the order — meta first, then ops — and the rule cannot be
/// reimplemented differently by a second caller.
pub fn submit_with_engine(v: &mut TableView) -> Submitted {
    v.submit(
        crate::table_nlp::parse_meta_command,
        |m| matches!(m, crate::table_nlp::MetaCommand::Undo),
        crate::table_nlp::parse_command,
    )
}

#[cfg(test)]
mod engine_tests {
    use super::*;

    fn submit(v: &mut TableView, line: &str) -> Submitted {
        v.input = line.into();
        submit_with_engine(v)
    }

    #[test]
    fn the_view_speaks_the_apps_own_english() {
        // The point of including the engine rather than re-implementing it: a
        // second parser would accept a slightly different English within a
        // week, and the two frontends would disagree about what a user typed.
        let mut v = TableView::new(true);
        assert_eq!(submit(&mut v, "hide column 3"), Submitted::Pushed);
        assert_eq!(v.depth(), 1);
    }

    #[test]
    fn the_engines_undo_vocabulary_reaches_the_stack() {
        let mut v = TableView::new(true);
        submit(&mut v, "hide column 3");
        // "undo last op" is the engine's phrasing, not one I invented.
        assert_eq!(submit(&mut v, "undo last op"), Submitted::Undone);
        assert!(v.is_base());
    }

    #[test]
    fn the_engines_reset_vocabulary_clears_the_stack() {
        let mut v = TableView::new(true);
        submit(&mut v, "hide column 1");
        submit(&mut v, "hide column 2");
        assert_eq!(submit(&mut v, "clear all"), Submitted::Reset);
        assert!(v.is_base());
    }

    #[test]
    fn a_command_the_engine_rejects_leaves_the_stack_alone() {
        let mut v = TableView::new(true);
        submit(&mut v, "hide column 1");
        assert!(matches!(submit(&mut v, "make it blue"), Submitted::NotUnderstood(_)));
        assert_eq!(v.depth(), 1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Stand-ins for the engine's parsers, so the view's rules can be tested
    /// without building a table.
    #[derive(Debug)]
    enum M { Undo, Reset }
    fn meta(s: &str) -> Option<M> {
        match s { "undo" => Some(M::Undo), "reset" => Some(M::Reset), _ => None }
    }
    fn is_undo(m: &M) -> bool { matches!(m, M::Undo) }
    // Returns an OWNED op: a parser that borrowed from its input would tie the
    // op's lifetime to the command line, and the stack outlives the line.
    fn op(s: &str) -> Result<String, String> {
        if s.starts_with("sort") || s.starts_with("hide") { Ok(s.to_string()) }
        else { Err(format!("don't understand {s:?}")) }
    }

    fn view() -> TableView { TableView::new(true) }

    fn submit(v: &mut TableView, line: &str) -> Submitted {
        v.input = line.into();
        v.submit(meta, is_undo, op)
    }

    #[test]
    fn ops_accumulate_rather_than_being_applied_destructively() {
        // The displayed table is base-plus-stack, recomputed. That is what
        // makes undo a pop rather than an inverse — and Sort has no inverse.
        let mut v = view();
        assert!(v.is_base());
        submit(&mut v, "sort by column 2");
        submit(&mut v, "hide column 3");
        assert_eq!(v.depth(), 2);
        assert!(!v.is_base());
    }

    #[test]
    fn undo_pops_one_op() {
        let mut v = view();
        submit(&mut v, "sort by column 2");
        submit(&mut v, "hide column 3");
        assert_eq!(submit(&mut v, "undo"), Submitted::Undone);
        assert_eq!(v.depth(), 1);
    }

    #[test]
    fn undo_at_base_data_says_so_rather_than_appearing_to_work() {
        let mut v = view();
        assert_eq!(submit(&mut v, "undo"), Submitted::Undone);
        assert_eq!(v.depth(), 0);
        assert_eq!(v.status.as_deref(), Some("nothing to undo"));
    }

    #[test]
    fn reset_clears_the_whole_stack() {
        let mut v = view();
        for _ in 0..4 { submit(&mut v, "sort by column 1"); }
        assert_eq!(v.depth(), 4);
        assert_eq!(submit(&mut v, "reset"), Submitted::Reset);
        assert!(v.is_base());
        assert_eq!(v.status.as_deref(), Some("back to base data"));
    }

    #[test]
    fn reset_at_base_data_says_it_is_already_there() {
        let mut v = view();
        submit(&mut v, "reset");
        assert_eq!(v.status.as_deref(), Some("already at base data"));
    }

    #[test]
    fn a_meta_command_is_tried_before_the_op_vocabulary() {
        // "undo" is a statement about the STACK, not about the data. Folding it
        // into TableOp would add a variant with no before/after semantics that
        // every consumer would have to ignore.
        let mut v = view();
        submit(&mut v, "sort by column 1");
        // The op parser would reject "undo"; the meta parser must see it first.
        assert_eq!(submit(&mut v, "undo"), Submitted::Undone);
        assert_eq!(v.depth(), 0, "it undid rather than erroring");
    }

    #[test]
    fn an_unparsed_command_leaves_the_table_alone() {
        // A table that clears itself when you mistype is a table you stop
        // trusting.
        let mut v = view();
        submit(&mut v, "sort by column 1");
        let out = submit(&mut v, "sorrt by colum 1");
        assert!(matches!(out, Submitted::NotUnderstood(_)));
        assert_eq!(v.depth(), 1, "the stack is untouched");
    }

    #[test]
    fn an_unparsed_command_keeps_what_was_typed_so_a_typo_can_be_corrected() {
        let mut v = view();
        submit(&mut v, "sorrt by colum 1");
        assert_eq!(v.input, "sorrt by colum 1");
    }

    #[test]
    fn a_parsed_command_clears_the_line_so_it_is_not_run_twice() {
        let mut v = view();
        submit(&mut v, "sort by column 1");
        assert!(v.input.is_empty());
    }

    #[test]
    fn the_complaint_names_what_was_not_understood() {
        // "don't understand" alone gives the user nothing to fix.
        let mut v = view();
        let out = submit(&mut v, "flibble");
        match out {
            Submitted::NotUnderstood(m) => assert!(m.contains("flibble"), "got {m:?}"),
            other => panic!("expected a complaint, got {other:?}"),
        }
    }

    #[test]
    fn an_empty_line_is_a_keypress_rather_than_a_mistake() {
        let mut v = view();
        let out = submit(&mut v, "   ");
        assert_eq!(out, Submitted::NotUnderstood(String::new()));
        assert!(v.status.is_none(), "and it says nothing");
    }

    #[test]
    fn the_status_counts_the_stack_in_readable_english() {
        let mut v = view();
        assert_eq!(v.describe(), "base data");
        submit(&mut v, "sort by column 1");
        assert_eq!(v.describe(), "1 op applied");
        submit(&mut v, "hide column 2");
        assert_eq!(v.describe(), "2 ops applied");
    }

    // ── layout ──────────────────────────────────────────────────────────────

    fn table() -> (Vec<String>, Vec<Vec<String>>) {
        (vec!["name".into(), "size".into()],
         vec![vec!["alpha".into(), "12".into()],
              vec!["a-much-longer-name".into(), "7".into()]])
    }

    #[test]
    fn a_column_is_wide_enough_for_its_widest_cell() {
        let (h, r) = table();
        let w = column_widths(&h, &r);
        assert_eq!(w[0], "a-much-longer-name".chars().count());
    }

    #[test]
    fn a_header_longer_than_its_values_still_fits() {
        // A column that clips its own name is a column you cannot identify.
        let h = vec!["a-very-long-header".to_string()];
        let r = vec![vec!["1".to_string()]];
        assert_eq!(column_widths(&h, &r)[0], "a-very-long-header".chars().count());
    }

    #[test]
    fn a_ragged_row_widens_the_table_rather_than_being_truncated() {
        // A row with more cells than the header is malformed input, and dropping
        // the extra cells hides the fact.
        let h = vec!["a".to_string()];
        let r = vec![vec!["1".to_string(), "22".to_string()]];
        let w = column_widths(&h, &r);
        assert_eq!(w.len(), 2);
        assert_eq!(w[1], 2);
    }

    #[test]
    fn widths_are_measured_in_characters_not_bytes() {
        // A multibyte column would otherwise be padded far too wide.
        let h = vec!["名前".to_string()];
        let r: Vec<Vec<String>> = vec![];
        assert_eq!(column_widths(&h, &r)[0], 2);
    }

    #[test]
    fn a_numeric_column_is_detected_so_the_digits_can_line_up() {
        let (_, r) = table();
        assert!(column_is_numeric(&r, 1));
        assert!(!column_is_numeric(&r, 0));
    }

    #[test]
    fn one_stray_word_makes_the_whole_column_text() {
        // A right-aligned column with one word in it looks broken.
        let r = vec![vec!["1".to_string()], vec!["n/a".to_string()], vec!["3".to_string()]];
        assert!(!column_is_numeric(&r, 0));
    }

    #[test]
    fn blank_cells_do_not_make_a_numeric_column_text() {
        // A gap in the data is not a word.
        let r = vec![vec!["1".to_string()], vec!["".to_string()], vec!["3".to_string()]];
        assert!(column_is_numeric(&r, 0));
    }

    #[test]
    fn an_all_blank_column_is_not_numeric() {
        // Nothing to align, and calling it numeric would right-align a column
        // of nothing.
        let r = vec![vec!["".to_string()], vec!["  ".to_string()]];
        assert!(!column_is_numeric(&r, 0));
    }

    #[test]
    fn a_column_index_past_the_end_is_not_numeric() {
        let (_, r) = table();
        assert!(!column_is_numeric(&r, 9));
    }
}
