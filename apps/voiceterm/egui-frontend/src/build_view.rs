//! build_view — the compiler-output menu, over the app's own extractor.
//!
//! `src/build_diagnostics.rs` says what it is: cargo and CMake "dump a wall of
//! text — most of it noise wrapping a few diagnostics that actually matter",
//! parsed into a categorised summary. It is explicitly "pure: text in, structs +
//! wire out. No I/O, no QML" — written to be mounted, and never was.
//!
//! What the port owes is the surface the doc describes: "a fuzzy,
//! voice-addressable menu (jump to error 1, show warnings)".
//!
//! ## Errors before warnings, always
//!
//! A build with 3 errors and 200 warnings is a build with 3 problems. Ordering
//! by source position — the obvious choice, and what the raw output does —
//! buries them, which is the exact failure the extractor exists to undo. Sorting
//! by severity first is the whole point of having parsed it.
//!
//! ## Why "jump to error 1" and not "jump to line 412"
//!
//! The menu is voice-addressable, and a line number is a terrible thing to say
//! out loud: four digits, easily misheard, and wrong the moment you edit. A
//! small ordinal is speakable and stable for the length of the build.

#[path = "../../src/build_diagnostics.rs"]
#[allow(dead_code)]
pub mod diagnostics;

pub use diagnostics::{BuildSummary, Category, Diagnostic, Severity};

/// Which diagnostics are showing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Show {
    /// Errors only — the default, because that is what blocks the build.
    #[default]
    Errors,
    All,
    /// One category, from the rolled-up counts.
    Category(Category),
}

#[derive(Debug, Clone, Default)]
pub struct BuildView {
    /// `None` until a build has been parsed. BuildSummary has no Default, and
    /// that is right: an empty summary and "no build yet" are different states,
    /// and a view that conflates them reports "build ok" before anything ran.
    pub summary: Option<BuildSummary>,
    pub show: Show,
    /// Which diagnostic is selected, as the ordinal the user can say.
    pub selected: Option<usize>,
    pub query: String,
}

impl BuildView {
    pub fn set_output(&mut self, cargo_output: &str) {
        let summary = diagnostics::parse_cargo(cargo_output);
        // A new build invalidates the selection: ordinal 3 in the previous build
        // is a different diagnostic now, and jumping to it would send the user
        // to the wrong file.
        self.selected = None;
        // Fall back to All when a build succeeded, so a warnings-only build does
        // not show an empty list under the default Errors filter.
        if summary.ok() && summary.warning_count > 0 { self.show = Show::All; }
        self.summary = Some(summary);
    }

    /// The visible diagnostics, errors first, each with the ordinal to say.
    pub fn rows(&self) -> Vec<(usize, &Diagnostic)> {
        let q = self.query.trim().to_lowercase();
        let Some(summary) = self.summary.as_ref() else { return Vec::new() };
        let mut v: Vec<&Diagnostic> = summary.diagnostics.iter()
            .filter(|d| match self.show {
                Show::Errors => d.severity == Severity::Error,
                Show::All => true,
                Show::Category(c) => d.category == c,
            })
            .filter(|d| q.is_empty() || d.one_line().to_lowercase().contains(&q))
            .collect();
        // Errors before warnings, then source order within each. The raw output
        // is in source order alone, which buries three errors under two hundred
        // warnings — the exact thing the extractor exists to undo.
        v.sort_by_key(|d| match d.severity { Severity::Error => 0, Severity::Warning => 1 });
        // Ordinals are 1-based and assigned AFTER sorting, so "error 1" is the
        // first thing on screen rather than the first thing in the file.
        v.into_iter().enumerate().map(|(i, d)| (i + 1, d)).collect()
    }

    /// Select by the spoken ordinal.
    pub fn jump(&mut self, ordinal: usize) -> Option<Diagnostic> {
        // Cloned rather than borrowed: storing the selection needs &mut self
        // while a borrow of the row is live, and the diagnostic is small.
        let hit = self.rows().into_iter().find(|(n, _)| *n == ordinal).map(|(_, d)| d.clone())?;
        // The ordinal, not an index into the unsorted vector — the sort order is
        // what the user is looking at.
        self.selected = Some(ordinal);
        Some(hit)
    }

    /// One line for a status bar. Reports the shape of the build, not its text.
    pub fn status(&self) -> String {
        let Some(summary) = self.summary.as_ref() else { return "no build yet".into() };
        if summary.ok() && summary.warning_count == 0 {
            return "build ok".into();
        }
        let mut s = String::new();
        if summary.error_count > 0 {
            s.push_str(&format!("{} error{}", summary.error_count,
                                if summary.error_count == 1 { "" } else { "s" }));
        }
        if summary.warning_count > 0 {
            if !s.is_empty() { s.push_str(", "); }
            s.push_str(&format!("{} warning{}", summary.warning_count,
                                if summary.warning_count == 1 { "" } else { "s" }));
        }
        // The failed target is the single most useful word when a workspace has
        // thirty crates and one of them broke.
        if let Some(t) = &summary.failed_target { s.push_str(&format!(" in {t}")); }
        s
    }

    /// Categories with counts, for the filter row.
    pub fn categories(&self) -> Vec<(Category, usize)> {
        self.summary.as_ref().map(|s| s.category_counts()).unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const OUTPUT: &str = "\
warning: unused variable: `x`
 --> src/a.rs:3:9
error[E0308]: mismatched types
 --> src/b.rs:12:5
warning: unused import: `std::fmt`
 --> src/c.rs:1:5
error[E0425]: cannot find value `y` in this scope
 --> src/d.rs:40:13
error: could not compile `mycrate` (lib) due to 2 previous errors
";

    fn view() -> BuildView {
        let mut v = BuildView::default();
        v.set_output(OUTPUT);
        v
    }

    #[test]
    fn errors_come_before_warnings_whatever_the_source_order() {
        // Three errors under two hundred warnings is the failure the extractor
        // exists to undo, and source order is what the raw output already gives.
        let mut v = view();
        v.show = Show::All;
        let rows = v.rows();
        assert!(!rows.is_empty());
        let first_warning = rows.iter().position(|(_, d)| d.severity == Severity::Warning);
        let last_error = rows.iter().rposition(|(_, d)| d.severity == Severity::Error);
        if let (Some(w), Some(e)) = (first_warning, last_error) {
            assert!(e < w, "a warning came before an error");
        }
    }

    #[test]
    fn ordinals_follow_the_screen_not_the_file() {
        // "Jump to error 1" has to mean the first thing on screen; a line number
        // is a terrible thing to say out loud and wrong the moment you edit.
        let mut v = view();
        let first = v.jump(1);
        assert!(first.is_some());
        assert_eq!(v.rows()[0].0, 1, "ordinals are 1-based");
        assert_eq!(v.selected, Some(1));
    }

    #[test]
    fn the_default_filter_shows_what_blocks_the_build() {
        let v = view();
        assert_eq!(v.show, Show::Errors);
        assert!(v.rows().iter().all(|(_, d)| d.severity == Severity::Error));
    }

    #[test]
    fn a_warnings_only_build_does_not_show_an_empty_list() {
        // The default filter is Errors, and a build that succeeded with warnings
        // would otherwise look like it produced nothing at all.
        let mut v = BuildView::default();
        v.set_output("warning: unused variable: `x`\n --> src/a.rs:3:9\n");
        assert_eq!(v.show, Show::All, "fell back so the warnings are visible");
        assert!(!v.rows().is_empty());
    }

    #[test]
    fn a_new_build_drops_the_previous_selection() {
        // Ordinal 3 in the last build is a different diagnostic now, and jumping
        // to it sends the user to the wrong file.
        let mut v = view();
        v.jump(1);
        assert!(v.selected.is_some());
        v.set_output(OUTPUT);
        assert_eq!(v.selected, None);
    }

    #[test]
    fn jumping_past_the_end_selects_nothing() {
        let mut v = view();
        assert!(v.jump(999).is_none());
        assert_eq!(v.selected, None, "and does not leave a stale selection");
    }

    #[test]
    fn the_status_line_names_the_failed_target() {
        // The single most useful word when a workspace has thirty crates and
        // one of them broke.
        let v = view();
        let s = v.status();
        assert!(s.contains("error"), "{s}");
        assert!(s.contains("mycrate"), "{s}");
    }

    #[test]
    fn a_clean_build_says_so_rather_than_showing_zeroes() {
        let mut v = BuildView::default();
        v.set_output("   Compiling mycrate v0.1.0\n    Finished dev profile\n");
        assert_eq!(v.status(), "build ok");
        assert!(v.rows().is_empty());
    }

    #[test]
    fn counts_are_singular_when_there_is_one() {
        // "1 errors" is the kind of detail that makes a tool feel unfinished.
        let mut v = BuildView::default();
        v.set_output("error[E0308]: mismatched types\n --> src/b.rs:12:5\n");
        assert!(v.status().contains("1 error"), "{}", v.status());
        assert!(!v.status().contains("1 errors"), "{}", v.status());
    }
}
