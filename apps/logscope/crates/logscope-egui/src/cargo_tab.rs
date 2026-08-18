//! cargo_tab — `cargo check` diagnostics, parsed and filtered.
//!
//! Ported from `crates/logscope/src/ui/cargo_tab.rs`, the GTK "Rust project
//! diagnostics viewer with cancel support". This is the half worth porting
//! carefully: reading `cargo check --message-format=json`, deciding which
//! messages become cards, and pulling the source context around each span.
//! The GTK version interleaves that with widget construction across 680 lines;
//! separated, the rules fit in a page and can be tested against real cargo
//! output with no compiler and no screen.
//!
//! ## The two filters, both inherited deliberately
//!
//! A `note` with no error code is dropped. Those are the "note: expected
//! struct X, found struct Y" continuation lines that belong to the error above
//! them — as cards they are duplicates of context the reader already has, and
//! the GTK version drops them for that reason.
//!
//! A message with no span is dropped. `cargo` emits summary lines ("aborting
//! due to 3 previous errors") with an empty `spans` array, and a card with no
//! file and no line has nothing to click through to.
//!
//! ## Rendered text is preferred over the raw message
//!
//! `rendered` is what rustc prints to a terminal, arrows and underlines
//! included, and it is the thing a person recognises. `message` is the bare
//! sentence. The GTK version falls back one to the other and so does this,
//! because `rendered` is absent on some message kinds and an empty card is
//! worse than a terse one.

use std::path::Path;

/// One diagnostic worth showing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    pub file: String,
    pub line: u32,
    /// `error`, `warning`, `note`, … verbatim from cargo.
    pub level: String,
    /// Rendered form when cargo supplied one, else the bare message.
    pub message: String,
    /// `E0599` and friends, when the message carries one.
    pub code: Option<String>,
    /// Source lines around the span, filled by [`with_context`].
    pub source_lines: Vec<String>,
}

impl Diagnostic {
    pub fn is_error(&self) -> bool {
        self.level == "error"
    }
    pub fn is_warning(&self) -> bool {
        self.level == "warning"
    }
    /// The `file:line` a reader clicks.
    pub fn location(&self) -> String {
        format!("{}:{}", self.file, self.line)
    }
}

/// Parse one line of `cargo check --message-format=json`.
///
/// Returns `None` for anything that is not a showable diagnostic: non-JSON
/// noise, a non-`compiler-message` reason, a bare note, or a message with no
/// span. Each of those is a real thing cargo emits, so the caller filters by
/// taking `Some` rather than by inspecting reasons.
pub fn parse_line(line: &str) -> Option<Diagnostic> {
    let v: serde_json::Value = serde_json::from_str(line).ok()?;
    if v["reason"] != "compiler-message" {
        return None;
    }
    let msg = &v["message"];
    let level = msg["level"].as_str().unwrap_or("note").to_owned();
    let code = msg["code"]["code"].as_str().map(str::to_owned);

    // A note with no code is a continuation of the message above it.
    if level == "note" && code.is_none() {
        return None;
    }

    let message = msg["rendered"]
        .as_str()
        .or_else(|| msg["message"].as_str())
        .unwrap_or("")
        .to_owned();

    // The primary span, or the first if none is marked — matching the GTK
    // version. A summary line has neither and is dropped.
    let spans = msg["spans"].as_array()?;
    let span = spans
        .iter()
        .find(|s| s["is_primary"] == true)
        .or_else(|| spans.first())?;

    Some(Diagnostic {
        file: span["file_name"].as_str().unwrap_or("?").to_owned(),
        line: span["line_start"].as_u64().unwrap_or(1) as u32,
        level,
        message,
        code,
        source_lines: Vec::new(),
    })
}

/// Parse a whole `cargo check` stdout.
pub fn parse_output(stdout: &str) -> Vec<Diagnostic> {
    stdout.lines().filter_map(parse_line).collect()
}

/// How many lines of source to show either side of the span.
pub const CONTEXT: u32 = 2;

/// Attach source context, reading from `workspace`.
///
/// Missing files are left with no context rather than failing the
/// diagnostic: cargo reports paths relative to a workspace member, and a
/// diagnostic whose source cannot be found is still worth showing — the
/// message and the location are the useful part.
pub fn with_context(mut d: Diagnostic, workspace: &Path) -> Diagnostic {
    let path = workspace.join(&d.file);
    if let Ok(text) = std::fs::read_to_string(&path) {
        let lines: Vec<&str> = text.lines().collect();
        let start = d.line.saturating_sub(CONTEXT).max(1) as usize;
        let end = ((d.line + CONTEXT) as usize).min(lines.len());
        if start <= end {
            d.source_lines = lines[start - 1..end]
                .iter()
                .enumerate()
                .map(|(i, l)| format!("{:>6}  {}", start + i, l))
                .collect();
        }
    }
    d
}

/// What the viewer holds.
#[derive(Debug, Clone, Default)]
pub struct CargoTab {
    pub diagnostics: Vec<Diagnostic>,
    /// Hide warning-level cards. The GTK version's `hide_warnings`.
    pub hide_warnings: bool,
    /// True while a check is running.
    pub running: bool,
    /// Set to stop the run. The GTK version threads an AtomicBool through the
    /// worker; the flag lives here and the host observes it, so the model
    /// carries no threading.
    pub cancel_requested: bool,
    /// Result of the last completed run.
    pub last_success: Option<bool>,
}

impl CargoTab {
    /// Is this a Rust project? The GTK version's `is_cargo_project`.
    pub fn is_cargo_project(path: &Path) -> bool {
        path.join("Cargo.toml").is_file()
    }

    pub fn start(&mut self) {
        self.running = true;
        self.cancel_requested = false;
        self.diagnostics.clear();
        self.last_success = None;
    }

    /// Cancel the run. Named for what it does — the host is what actually
    /// stops the subprocess, and pretending otherwise here would suggest the
    /// model can kill a process it never spawned.
    pub fn request_cancel(&mut self) {
        self.cancel_requested = true;
    }

    pub fn finished(&mut self, success: bool) {
        self.running = false;
        self.last_success = Some(success);
    }

    /// The cards to draw, after the warning filter.
    pub fn visible(&self) -> Vec<&Diagnostic> {
        self.diagnostics
            .iter()
            .filter(|d| !(self.hide_warnings && d.is_warning()))
            .collect()
    }

    pub fn error_count(&self) -> usize {
        self.diagnostics.iter().filter(|d| d.is_error()).count()
    }
    pub fn warning_count(&self) -> usize {
        self.diagnostics.iter().filter(|d| d.is_warning()).count()
    }

    /// The line above the cards.
    pub fn summary(&self) -> String {
        if self.running {
            return "running cargo check…".into();
        }
        match self.last_success {
            None => "no check run yet".into(),
            Some(_) => {
                let (e, w) = (self.error_count(), self.warning_count());
                if e == 0 && w == 0 {
                    // Success with no diagnostics is worth saying plainly; an
                    // empty card list otherwise looks like a check that failed
                    // to run.
                    "clean — no diagnostics".into()
                } else {
                    let hidden = if self.hide_warnings && w > 0 {
                        format!(", {w} warning(s) hidden")
                    } else {
                        String::new()
                    };
                    format!("{e} error(s), {w} warning(s){hidden}")
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ONE LINE EACH, because that is how cargo emits them. The first version
    // of these fixtures was pretty-printed across several lines and every
    // single-line test still passed — parse_line received the whole string
    // and serde does not care about whitespace. parse_output split on
    // newlines and got zero diagnostics from both. A fixture that is not
    // shaped like the real input tests the parser against a format nothing
    // produces.
    const ERR: &str = r#"{"reason":"compiler-message","message":{"level":"error","message":"cannot find value `x` in this scope","rendered":"error[E0425]: cannot find value `x`\n  --> src/a.rs:3:5\n","code":{"code":"E0425"},"spans":[{"file_name":"src/a.rs","line_start":3,"line_end":3,"is_primary":true}]}}"#;

    const WARN: &str = r#"{"reason":"compiler-message","message":{"level":"warning","message":"unused variable: `y`","rendered":"warning: unused variable: `y`\n","code":{"code":"unused_variables"},"spans":[{"file_name":"src/b.rs","line_start":9,"line_end":9,"is_primary":true}]}}"#;

    #[test]
    fn a_rendered_error_is_preferred_over_the_bare_message() {
        // `rendered` is what rustc prints to a terminal, arrows included, and
        // it is what a person recognises.
        let d = parse_line(ERR).expect("an error with a span is a card");
        assert!(d.message.starts_with("error[E0425]"), "{}", d.message);
        assert_eq!(d.code.as_deref(), Some("E0425"));
        assert_eq!(d.location(), "src/a.rs:3");
        assert!(d.is_error());
    }

    #[test]
    fn the_bare_message_is_used_when_there_is_no_rendered_form() {
        // Absent on some message kinds, and an empty card is worse than a
        // terse one.
        let line = r#"{"reason":"compiler-message","message":{"level":"error","message":"a terse one","code":null,"spans":[{"file_name":"src/c.rs","line_start":1,"is_primary":true}]}}"#;
        let d = parse_line(line).unwrap();
        assert_eq!(d.message, "a terse one");
    }

    #[test]
    fn a_note_without_a_code_is_not_a_card() {
        // These are the "note: expected struct X, found struct Y"
        // continuation lines belonging to the error above. As cards they
        // duplicate context the reader already has.
        let note = r#"{"reason":"compiler-message","message":{"level":"note","message":"expected struct `A`, found `B`","code":null,"spans":[{"file_name":"src/a.rs","line_start":3,"is_primary":true}]}}"#;
        assert!(parse_line(note).is_none());

        // A note that DOES carry a code is a lint and stands alone.
        let lint = note.replace(r#""code":null"#, r#""code":{"code":"unused_mut"}"#);
        assert!(parse_line(&lint).is_some());
    }

    #[test]
    fn a_message_with_no_span_is_dropped() {
        // "aborting due to 3 previous errors" has an empty spans array, and a
        // card with no file and no line has nothing to click through to.
        let summary = r#"{"reason":"compiler-message","message":{"level":"error","message":"aborting due to 3 previous errors","code":null,"spans":[]}}"#;
        assert!(parse_line(summary).is_none());
    }

    #[test]
    fn non_diagnostic_lines_are_skipped_without_erroring() {
        // cargo interleaves build-script and artifact lines with the
        // diagnostics, and stdout is not guaranteed to be pure JSON.
        assert!(parse_line(r#"{"reason":"compiler-artifact","target":{}}"#).is_none());
        assert!(parse_line("Compiling logscope v0.1.0").is_none());
        assert!(parse_line("").is_none());
    }

    #[test]
    fn a_whole_output_parses_to_only_the_showable_ones() {
        let out = format!("{ERR}\n{}\n{WARN}\nCompiling foo v0.1.0\n",
                          r#"{"reason":"compiler-artifact"}"#);
        let ds = parse_output(&out);
        assert_eq!(ds.len(), 2, "one error and one warning survive");
        assert_eq!(ds[0].level, "error");
        assert_eq!(ds[1].level, "warning");
    }

    #[test]
    fn hiding_warnings_hides_only_warnings_and_says_how_many() {
        let mut t = CargoTab::default();
        t.diagnostics = parse_output(&format!("{ERR}\n{WARN}\n"));
        t.finished(false);
        assert_eq!(t.visible().len(), 2);
        assert_eq!(t.summary(), "1 error(s), 1 warning(s)");

        t.hide_warnings = true;
        assert_eq!(t.visible().len(), 1);
        assert!(t.visible()[0].is_error());
        // The count still reports the hidden ones — a filter that made them
        // vanish from the summary too would look like they were fixed.
        assert!(t.summary().contains("1 warning(s) hidden"), "{}", t.summary());
    }

    #[test]
    fn a_clean_run_says_so_rather_than_showing_an_empty_list() {
        // An empty card list otherwise looks like a check that failed to run.
        let mut t = CargoTab::default();
        assert_eq!(t.summary(), "no check run yet");
        t.start();
        assert!(t.running);
        assert_eq!(t.summary(), "running cargo check…");
        t.finished(true);
        assert_eq!(t.summary(), "clean — no diagnostics");
    }

    #[test]
    fn starting_clears_the_previous_run() {
        // Otherwise a re-check appends to the last run's cards and an error
        // fixed five minutes ago is still on screen.
        let mut t = CargoTab::default();
        t.diagnostics = parse_output(ERR);
        t.finished(false);
        t.start();
        assert!(t.diagnostics.is_empty());
        assert_eq!(t.last_success, None);
        assert!(!t.cancel_requested, "a fresh run is not pre-cancelled");
    }

    #[test]
    fn source_context_survives_a_file_it_cannot_read() {
        // cargo reports paths relative to a workspace member, so a path that
        // does not resolve is normal. The message and location are still
        // worth showing.
        let d = parse_line(ERR).unwrap();
        let out = with_context(d, Path::new("/definitely/not/here"));
        assert!(out.source_lines.is_empty());
        assert_eq!(out.location(), "src/a.rs:3");
    }

    #[test]
    fn source_context_is_numbered_and_bounded_at_the_file_edges() {
        let dir = std::env::temp_dir().join(format!("cargotab-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::write(dir.join("src/a.rs"), "one\ntwo\nthree\nfour\nfive\n").unwrap();
        let d = with_context(parse_line(ERR).unwrap(), &dir);
        // line 3 with CONTEXT 2 covers 1..=5, and each line carries its number
        // so a reader can match the card to the file.
        assert_eq!(d.source_lines.len(), 5);
        assert!(d.source_lines[0].contains("1  one"), "{:?}", d.source_lines[0]);
        assert!(d.source_lines[4].contains("5  five"));

        // A span near the top must not underflow past line 1.
        let mut top = parse_line(ERR).unwrap();
        top.line = 1;
        let top = with_context(top, &dir);
        assert!(top.source_lines[0].contains("1  one"));
        let _ = std::fs::remove_dir_all(dir);
    }
}
