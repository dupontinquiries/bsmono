//! status_rows — the changed-file list from `qml/FileRow.qml`, as a model.
//!
//! The QML row is a `SwipeDelegate`: swipe right to stage, swipe left to
//! unstage, long-press to peek. Those are three decisions and one gesture
//! vocabulary, and only the gesture part is toolkit-specific — so the
//! decisions live here, tested without a repository, and the page below draws
//! them.
//!
//! ## The two-letter status is the whole model
//!
//! `git status --porcelain` reports each path as `XY path`, where **X is the
//! index** and **Y is the working tree**. That pair is not decoration:
//!
//! * `M ` — modified and already staged. Unstaging is the only move.
//! * ` M` — modified, not staged. Staging is the only move.
//! * `MM` — staged, then modified again. BOTH moves are available, and a row
//!   that offered only one would silently discard the other half of the file's
//!   state. This is the case a single `staged: bool` cannot express, which is
//!   why the flag is derived from X and Y rather than stored.
//! * `??` — untracked. Stageable, never unstageable, and shown differently
//!   because "untracked" and "modified" are different problems.
//!
//! ## Renames carry two paths and the arrow is not part of either
//!
//! Porcelain writes a rename as `R  old -> new`. Splitting on whitespace and
//! taking the last field gives the new path and loses the old one; taking the
//! first gives a path that no longer exists. Both are kept, because the row
//! has to say what happened and the stage command needs the new one.

/// Which side of a porcelain pair a change sits on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    /// Staged only: the index differs from HEAD, the working tree matches.
    Indexed,
    /// Unstaged only: the working tree differs from the index.
    Worktree,
    /// Both — staged, then edited again.
    Both,
    /// Not tracked at all.
    Untracked,
}

impl Stage {
    /// Classify porcelain's two status letters.
    ///
    /// `None` means "not a change": both columns blank. Porcelain does not
    /// emit that line, and treating it as unstaged would put a clean file in
    /// the list — so the caller skips rather than inventing a stage.
    ///
    /// Extracted from inside `parse_porcelain` when `from_pair` needed the
    /// same rule. Two copies of an XY classifier is exactly the duplication
    /// that let bo-suite's UI and engine disagree about a threshold, and this
    /// one decides whether a button says "stage" or "unstage".
    pub fn from_xy(xy: &str) -> Option<Stage> {
        let b = xy.as_bytes();
        if b.len() < 2 {
            return None;
        }
        let (x, y) = (b[0] as char, b[1] as char);
        if x == '?' && y == '?' {
            return Some(Stage::Untracked);
        }
        match (x != ' ' && x != '?', y != ' ' && y != '?') {
            (true, true) => Some(Stage::Both),
            (true, false) => Some(Stage::Indexed),
            (false, true) => Some(Stage::Worktree),
            (false, false) => None,
        }
    }

    /// May this row be staged?
    ///
    /// Everything except a purely-indexed change: there is nothing left in the
    /// working tree to add.
    pub fn can_stage(self) -> bool {
        !matches!(self, Stage::Indexed)
    }

    /// May this row be unstaged?
    ///
    /// Only what is actually in the index. Offering unstage on an untracked
    /// file would run `git restore --staged` against a path git has never
    /// heard of — an error the user did nothing to deserve.
    pub fn can_unstage(self) -> bool {
        matches!(self, Stage::Indexed | Stage::Both)
    }

    /// The word under the filename.
    pub fn label(self) -> &'static str {
        match self {
            Stage::Indexed => "staged",
            Stage::Worktree => "modified",
            Stage::Both => "staged + modified",
            Stage::Untracked => "untracked",
        }
    }
}

/// One changed file.
#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    /// The two porcelain letters, verbatim, so the row can show what git said.
    pub xy: String,
    /// The path to act on. For a rename this is the NEW path.
    pub path: String,
    /// The previous path, for a rename only.
    pub from: Option<String>,
    pub stage: Stage,
}

impl Row {
    /// Build a row from cockpit's `(status, path)` pair.
    ///
    /// `parse_porcelain` takes whole `git status --porcelain` lines, which is
    /// right when this module runs git itself. cockpit's flow already ran it
    /// and split the result, so its callers had the two halves and no way in —
    /// they printed "{stat} {path}" instead, which shows a rename as one
    /// opaque "R  old -> new" string.
    ///
    /// The rename arrow is parsed HERE rather than by the caller, because the
    /// caller is a UI page and " -> " is porcelain's spelling, not a display
    /// choice. A page that split on it would be parsing git output in a
    /// render function.
    pub fn from_pair(status: &str, path: &str) -> Row {
        // NOT trimmed. Porcelain's XY is POSITIONAL: in "R " the X column is
        // R and the Y column is a space meaning "no worktree change". Trimming
        // leaves one character, from_xy rejects it as truncated, and a staged
        // rename silently classifies as Worktree — offering "stage" on a file
        // that is already staged. Padded instead, so a caller that did trim
        // still lands in the right column.
        let mut xy = status.to_string();
        while xy.len() < 2 {
            xy.push(' ');
        }
        let (from, path) = match path.split_once(" -> ") {
            Some((old, new)) => (Some(old.trim().to_string()), new.trim().to_string()),
            None => (None, path.trim().to_string()),
        };
        // A pair that classifies to nothing still becomes a row: cockpit
        // produced it from a real status run, so dropping it here would hide
        // a file the engine reported. Worktree is the conservative reading —
        // it offers "stage", which is reversible, rather than "unstage".
        let stage = Stage::from_xy(&xy).unwrap_or(Stage::Worktree);
        Row { xy, path, from, stage }
    }

    /// What the row shows on the right, trimmed the way the QML trims it.
    pub fn status_letter(&self) -> String {
        self.xy.trim().to_string()
    }

    /// The line describing the change, including where a rename came from.
    pub fn describe(&self) -> String {
        match &self.from {
            Some(old) => format!("{} → {}", old, self.path),
            None => self.path.clone(),
        }
    }
}

/// Parse `git status --porcelain` output.
///
/// Tolerant of the trailing newline and of blank lines; a line shorter than
/// the three-character `XY ` prefix is skipped rather than panicking on a
/// slice, because a truncated read is a real thing that happens when a
/// subprocess is killed mid-write.
pub fn parse_porcelain(out: &str) -> Vec<Row> {
    let mut rows = Vec::new();
    for line in out.lines() {
        if line.len() < 4 {
            continue;
        }
        let (xy, rest) = line.split_at(2);
        let rest = rest.trim_start();
        if rest.is_empty() {
            continue;
        }
        let Some(stage) = Stage::from_xy(&xy) else { continue };
        // A rename is `old -> new`. Quoted paths (git quotes anything with a
        // space or a non-ASCII byte) keep their quotes: stripping them here
        // would produce a path the caller then has to re-quote to use, and
        // getting that wrong is how a filename with a space deletes the wrong
        // file.
        let (from, path) = match rest.split_once(" -> ") {
            Some((old, new)) => (Some(old.to_string()), new.to_string()),
            None => (None, rest.to_string()),
        };
        rows.push(Row { xy: xy.to_string(), path, from, stage });
    }
    rows
}

/// What the page asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RowAction {
    Stage(String),
    Unstage(String),
    /// Long-press: show the file's contents.
    Peek(String),
}

/// The argv for an action, for the host to run.
///
/// Returned rather than executed: this crate spawns no processes, and the
/// separation is what lets the decision be tested. `--` terminates the
/// options so a path beginning with a dash is a path rather than a flag.
pub fn git_argv(action: &RowAction) -> Option<Vec<String>> {
    let s = |v: &str| v.to_string();
    match action {
        RowAction::Stage(p) => Some(vec![s("add"), s("--"), p.clone()]),
        // `restore --staged` rather than `reset HEAD`: it is the current
        // porcelain spelling and it does not touch the working tree, which
        // `reset --hard` adjacency makes worth being explicit about.
        RowAction::Unstage(p) => Some(vec![s("restore"), s("--staged"), s("--"), p.clone()]),
        // Peek is a read the page does itself.
        RowAction::Peek(_) => None,
    }
}

/// Counts for the page header: (staged, unstaged, untracked).
///
/// A `Both` row counts on BOTH sides, because it genuinely has staged and
/// unstaged content — reporting it once would make the numbers disagree with
/// what `git commit` is about to include.
pub fn counts(rows: &[Row]) -> (usize, usize, usize) {
    let mut staged = 0;
    let mut unstaged = 0;
    let mut untracked = 0;
    for r in rows {
        match r.stage {
            Stage::Indexed => staged += 1,
            Stage::Worktree => unstaged += 1,
            Stage::Both => {
                staged += 1;
                unstaged += 1;
            }
            Stage::Untracked => untracked += 1,
        }
    }
    (staged, unstaged, untracked)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Porcelain, with the leading column preserved.
    ///
    /// Written as `concat!` and not as a `"\` continuation literal: that form
    /// strips leading whitespace on each line, which silently turned
    /// " M main.rs" into "M main.rs" — an unstaged edit read as a staged one.
    /// The X column IS the leading space here, so the one thing this fixture
    /// exists to test was the thing the literal ate.
    const SAMPLE: &str = concat!(
        " M src/main.rs\n",
        "M  src/lib.rs\n",
        "MM src/both.rs\n",
        "?? notes.txt\n",
        "A  src/added.rs\n",
        "R  old/name.rs -> new/name.rs\n",
        "D  src/gone.rs\n",
    );

    #[test]
    fn the_two_letters_decide_what_the_row_can_do() {
        let rows = parse_porcelain(SAMPLE);
        let by = |p: &str| rows.iter().find(|r| r.path == p).unwrap().clone();

        let unstaged = by("src/main.rs");
        assert_eq!(unstaged.stage, Stage::Worktree);
        assert!(unstaged.stage.can_stage() && !unstaged.stage.can_unstage());

        let staged = by("src/lib.rs");
        assert_eq!(staged.stage, Stage::Indexed);
        assert!(!staged.stage.can_stage(), "nothing left in the worktree to add");
        assert!(staged.stage.can_unstage());

        // The case a single bool cannot express.
        let both = by("src/both.rs");
        assert_eq!(both.stage, Stage::Both);
        assert!(both.stage.can_stage() && both.stage.can_unstage());
        assert_eq!(both.stage.label(), "staged + modified");

        let untracked = by("notes.txt");
        assert_eq!(untracked.stage, Stage::Untracked);
        assert!(untracked.stage.can_stage());
        assert!(!untracked.stage.can_unstage(),
                "git has never heard of this path; unstaging it is an error");
    }

    #[test]
    fn a_rename_keeps_both_paths_and_acts_on_the_new_one() {
        let rows = parse_porcelain(SAMPLE);
        let r = rows.iter().find(|r| r.path == "new/name.rs").unwrap();
        assert_eq!(r.from.as_deref(), Some("old/name.rs"));
        assert_eq!(r.describe(), "old/name.rs → new/name.rs");
        // Staging acts on the path that exists.
        let argv = git_argv(&RowAction::Stage(r.path.clone())).unwrap();
        assert_eq!(argv, vec!["add", "--", "new/name.rs"]);
    }

    #[test]
    fn the_status_letters_survive_to_the_row() {
        let rows = parse_porcelain(SAMPLE);
        let added = rows.iter().find(|r| r.path == "src/added.rs").unwrap();
        assert_eq!(added.status_letter(), "A");
        let deleted = rows.iter().find(|r| r.path == "src/gone.rs").unwrap();
        assert_eq!(deleted.status_letter(), "D");
        assert_eq!(deleted.stage, Stage::Indexed, "a staged delete is still staged");
    }

    #[test]
    fn counts_report_a_both_row_on_both_sides() {
        let rows = parse_porcelain(SAMPLE);
        let (staged, unstaged, untracked) = counts(&rows);
        // staged: lib.rs, both.rs, added.rs, renamed, gone.rs
        assert_eq!(staged, 5);
        // unstaged: main.rs, both.rs
        assert_eq!(unstaged, 2);
        assert_eq!(untracked, 1);
    }

    #[test]
    fn unstage_uses_restore_and_never_touches_the_worktree() {
        let argv = git_argv(&RowAction::Unstage("src/lib.rs".into())).unwrap();
        assert_eq!(argv, vec!["restore", "--staged", "--", "src/lib.rs"]);
        // A path that starts with a dash is a PATH, which is what `--` is for.
        let argv = git_argv(&RowAction::Stage("--weird-name".into())).unwrap();
        assert_eq!(argv, vec!["add", "--", "--weird-name"]);
        // Peek is a read; there is no command to run.
        assert!(git_argv(&RowAction::Peek("x".into())).is_none());
    }

    #[test]
    fn malformed_output_is_skipped_rather_than_panicking() {
        // A truncated read is real: a subprocess killed mid-write leaves a
        // partial line, and slicing it blindly would panic in the UI thread.
        let rows = parse_porcelain("M\n\n   \n?? ok.txt\nXY\n");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].path, "ok.txt");
    }

    #[test]
    fn quoted_paths_keep_their_quotes() {
        // git quotes a path containing a space. Stripping the quotes here
        // hands the caller something it must re-quote to use, and a filename
        // with a space is exactly where that goes wrong.
        let rows = parse_porcelain("?? \"my notes.txt\"\n");
        assert_eq!(rows[0].path, "\"my notes.txt\"");
    }

    #[test]
    fn a_cockpit_pair_becomes_the_same_row_a_porcelain_line_would() {
        // cockpit already ran git and split the output, so its callers had
        // two halves and no way into this model — they printed "{stat} {path}"
        // and a rename showed as one opaque string.
        let a = Row::from_pair("R ", "old/name.rs -> new/name.rs");
        assert_eq!(a.path, "new/name.rs", "the path to act on is the NEW one");
        assert_eq!(a.from.as_deref(), Some("old/name.rs"));
        assert_eq!(a.describe(), "old/name.rs → new/name.rs");
        assert_eq!(a.stage, Stage::Indexed);

        // And it agrees with the whole-line parser on the same content, which
        // is the property that makes one classifier worth extracting.
        let b = &parse_porcelain("R  old/name.rs -> new/name.rs\n")[0];
        assert_eq!(a.stage, b.stage);
        assert_eq!(a.path, b.path);
        assert_eq!(a.from, b.from);
    }

    #[test]
    fn a_pair_that_classifies_to_nothing_is_still_shown() {
        // cockpit produced it from a real status run. Dropping it here would
        // hide a file the engine reported; Worktree is the conservative
        // reading because it offers "stage", which is reversible.
        let r = Row::from_pair("  ", "odd/case.rs");
        assert_eq!(r.stage, Stage::Worktree);
        assert_eq!(r.describe(), "odd/case.rs");
        assert!(r.stage.can_stage());
    }

    #[test]
    fn the_classifier_is_shared_not_copied() {
        // Both entry points route through Stage::from_xy. Two copies of an XY
        // classifier is the duplication that let bo-suite's UI and engine
        // disagree about a threshold, and this one decides whether a button
        // says stage or unstage.
        assert_eq!(Stage::from_xy("??"), Some(Stage::Untracked));
        assert_eq!(Stage::from_xy("MM"), Some(Stage::Both));
        assert_eq!(Stage::from_xy("M "), Some(Stage::Indexed));
        assert_eq!(Stage::from_xy(" M"), Some(Stage::Worktree));
        assert_eq!(Stage::from_xy("  "), None, "not a change");
        assert_eq!(Stage::from_xy("M"), None, "a truncated read is not a stage");
    }
}
