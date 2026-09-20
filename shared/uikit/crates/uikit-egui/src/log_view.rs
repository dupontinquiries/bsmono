//! log_view — a logscope-styled log list for logs/analysis panes: per-level coloured badges
//! (DEBUG/INFO/WARN/ERROR), roomy padding, and a fade-in for freshly-arrived entries via the shared
//! `motion` kit. Errors also tint their message so they read at a glance.
//!
//! The level → badge/colour mapping is pure and unit-tested; only rendering needs egui.

use crate::motion;
use egui::{Color32, Ui};

/// Severity of a log line, in logscope's palette.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum LogLevel {
    Debug,
    Info,
    Warn,
    Error,
}

impl LogLevel {
    /// The badge text + colour.
    pub fn badge(self) -> (&'static str, Color32) {
        match self {
            LogLevel::Debug => ("DEBUG", Color32::from_rgb(120, 130, 150)),
            LogLevel::Info => ("INFO", Color32::from_rgb(90, 175, 120)),
            LogLevel::Warn => ("WARN", Color32::from_rgb(230, 180, 60)),
            LogLevel::Error => ("ERROR", Color32::from_rgb(214, 90, 70)),
        }
    }
    /// Parse a level from a common textual form (case-insensitive); unknown → `Info`.
    pub fn parse(s: &str) -> LogLevel {
        match s.trim().to_ascii_lowercase().as_str() {
            "debug" | "dbg" | "trace" => LogLevel::Debug,
            "warn" | "warning" => LogLevel::Warn,
            "error" | "err" | "fatal" => LogLevel::Error,
            _ => LogLevel::Info,
        }
    }
}

/// One log line. `id` is a stable per-entry key (e.g. a monotonic sequence) so its fade-in plays once.
#[derive(Clone, Debug)]
pub struct LogEntry {
    pub level: LogLevel,
    pub text: String,
    pub id: u64,
}

impl LogEntry {
    pub fn new(level: LogLevel, text: impl Into<String>, id: u64) -> Self {
        Self { level, text: text.into(), id }
    }
}

/// Render `entries` newest-last with coloured level badges, padding, and a fade-in per new entry.
/// Auto-scrolls to the bottom. Drop into any logs/analysis pane.
pub fn log_view(ui: &mut Ui, entries: &[LogEntry]) {
    egui::ScrollArea::vertical().auto_shrink([false, false]).stick_to_bottom(true).show(ui, |ui| {
        for e in entries {
            log_row(ui, e);
        }
    });
}

/// A single log row (also usable standalone).
pub fn log_row(ui: &mut Ui, e: &LogEntry) {
    // fade the row in on first sight; keyed by id so it only plays once.
    let fade = motion::animate_eased(ui.ctx(), egui::Id::new(("logrow", e.id)), true, 0.35, motion::ease::out_cubic);
    let (label, col) = e.level.badge();
    egui::Frame::new().inner_margin(egui::Margin::symmetric(10, 5)).show(ui, |ui| {
        ui.set_opacity(fade);
        // Top-aligned: once the message can wrap, a centred chip would float
        // beside the middle of a five-line message.
        ui.horizontal_top(|ui| {
            // level chip
            egui::Frame::new()
                .fill(col)
                .inner_margin(egui::Margin::symmetric(6, 1))
                .corner_radius(egui::CornerRadius::same(4))
                .show(ui, |ui| {
                    ui.label(egui::RichText::new(label).monospace().small().strong().color(Color32::WHITE));
                });
            ui.add_space(8.0);
            // errors tint their message; warnings stay default but the chip carries the signal
            let text_col = if e.level == LogLevel::Error { col } else { ui.visuals().text_color() };
            // `.wrap()`: a plain label in a horizontal row never wraps, so a long
            // line ran under the right edge — clipped, and not scrollable to. With
            // it the label wraps at the width left after the chip.
            ui.add(egui::Label::new(egui::RichText::new(&e.text).color(text_col)).wrap());
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn badges_are_distinct_and_labelled() {
        let levels = [LogLevel::Debug, LogLevel::Info, LogLevel::Warn, LogLevel::Error];
        let labels: Vec<&str> = levels.iter().map(|l| l.badge().0).collect();
        assert_eq!(labels, ["DEBUG", "INFO", "WARN", "ERROR"]);
        // every level has a distinct colour
        let cols: std::collections::HashSet<_> = levels.iter().map(|l| format!("{:?}", l.badge().1)).collect();
        assert_eq!(cols.len(), 4);
    }

    /// A log line is whatever the tool printed, and tools print long lines: a
    /// command echo, a path, an MI record. At phone width the row must wrap it,
    /// not run it under the right edge where it cannot be read or scrolled to.
    #[test]
    fn a_long_line_wraps_inside_a_phone_width_instead_of_being_clipped() {
        let long = "smartctl -a --json=c -d sntjmicron /dev/disk/by-id/usb-Samsung_SSD_980_PRO_2TB_0123456789AB-0:0 exited 4 after 12.5 s";
        let entries = [LogEntry::new(LogLevel::Warn, long, 1)];
        let report = crate::layout_probe::probe(egui::vec2(360.0, 400.0), 3, true, |ctx| {
            #[allow(deprecated)]
            egui::CentralPanel::default().show(ctx, |ui| log_view(ui, &entries));
        });
        assert!(report.texts >= 2, "chip and message were laid out: {}", report.describe());
        assert!(report.clipped.is_empty() && report.offscreen.is_empty(), "{}", report.describe());
    }

    #[test]
    fn parse_maps_common_forms_and_defaults_to_info() {
        assert_eq!(LogLevel::parse("WARNING"), LogLevel::Warn);
        assert_eq!(LogLevel::parse(" err "), LogLevel::Error);
        assert_eq!(LogLevel::parse("trace"), LogLevel::Debug);
        assert_eq!(LogLevel::parse("chatter"), LogLevel::Info); // unknown → info
    }
}

/// Classify a line of build/tool output.
///
/// Exists because the obvious version is wrong in a way that shows up
/// constantly and reads as a broken build:
///
/// ```text
///     Compiling thiserror v2.0.12
/// ```
///
/// `line.contains("error")` matches that, so a successful compile of a
/// perfectly ordinary dependency renders in red. The same trap catches
/// `anyhow`'s `Error` re-exports, any crate with `fail` in its name, and
/// `--warnings` in a command echo.
///
/// The fix is to classify on the SHAPE of a diagnostic rather than the presence
/// of a scary word. cargo and rustc emit diagnostics at the start of a line as
/// `error:`, `error[E0308]:`, `warning:` — a prefix, with punctuation. Progress
/// lines are indented and start with a verb: `Compiling`, `Finished`, `Running`.
///
/// Being wrong in the safe direction matters here too: a real error shown as
/// Debug is missed, so anything that genuinely looks like a diagnostic prefix is
/// taken at face value even in an unexpected position.
pub fn classify_line(line: &str) -> LogLevel {
    let t = line.trim_start();

    // Progress lines first. cargo indents these and leads with a capitalised
    // verb, and none of them is a diagnostic however the crate is named.
    for verb in ["Compiling", "Checking", "Downloading", "Downloaded",
                 "Installing", "Updating", "Fresh", "Building", "Running",
                 "Documenting", "Packaging", "Uploading", "Blocking"] {
        if t.starts_with(verb) { return LogLevel::Debug; }
    }
    if t.starts_with("Finished") { return LogLevel::Info; }

    // A diagnostic is a prefix with punctuation: `error:`, `error[E0308]:`,
    // `warning:`. The bracket form has to be handled explicitly or every
    // rustc error code line falls through.
    let lower = t.to_ascii_lowercase();
    let diag = |kind: &str| {
        lower.starts_with(&format!("{kind}:"))
            || lower.starts_with(&format!("{kind}[")) && lower.contains("]:")
    };
    if diag("error") { return LogLevel::Error; }
    if diag("warning") { return LogLevel::Warn; }

    // Test harness and script conventions, anchored rather than searched.
    if t.starts_with('✗') || lower.starts_with("failed") || lower.starts_with("fatal") {
        return LogLevel::Error;
    }
    if lower.starts_with("test result: failed") { return LogLevel::Error; }
    if t.starts_with('✓') || lower.starts_with("test result: ok")
        || lower.starts_with("success") {
        return LogLevel::Info;
    }
    LogLevel::Debug
}

#[cfg(test)]
mod classify_tests {
    use super::*;

    #[test]
    fn compiling_thiserror_is_not_an_error() {
        // THE reported bug. `contains("error")` matches this line, so a
        // successful compile of an ordinary dependency rendered in red.
        assert_eq!(classify_line("   Compiling thiserror v2.0.12"), LogLevel::Debug);
        assert_eq!(classify_line("   Compiling anyhow v1.0.86"), LogLevel::Debug);
    }

    #[test]
    fn crates_with_scary_names_are_still_progress_lines() {
        // The same trap catches anything named for what it handles: error
        // types, failure types, warning lints, debug helpers.
        for line in ["   Compiling error-chain v0.12.4",
                     "   Compiling failure v0.1.8",
                     "   Compiling debug-ignore v1.0.5",
                     "   Compiling warnings-are-errors v0.1.0",
                     "   Checking thiserror-impl v2.0.12",
                     "   Downloading fail v0.5.1"] {
            assert_eq!(classify_line(line), LogLevel::Debug, "{line}");
        }
    }

    #[test]
    fn a_real_diagnostic_is_still_an_error() {
        // Fixing the false positive must not create a false negative — a
        // missed error is worse than a mislabelled progress line.
        assert_eq!(classify_line("error: could not compile `foo`"), LogLevel::Error);
        assert_eq!(classify_line("error[E0308]: mismatched types"), LogLevel::Error);
        assert_eq!(classify_line("warning: unused variable: `x`"), LogLevel::Warn);
    }

    #[test]
    fn the_rustc_error_code_form_is_handled_explicitly() {
        // Without the bracket case every coded error falls through to Debug,
        // which is precisely the errors that matter most.
        assert_eq!(classify_line("error[E0433]: failed to resolve"), LogLevel::Error);
        assert_eq!(classify_line("warning[unused_imports]: unused import"),
                   LogLevel::Warn);
    }

    #[test]
    fn indentation_does_not_hide_a_diagnostic() {
        // cargo indents nested output, and an error inside a sub-build is still
        // an error.
        assert_eq!(classify_line("    error: linking with `cc` failed"),
                   LogLevel::Error);
    }

    #[test]
    fn finished_is_success_and_compiling_is_progress() {
        // Distinguished because one is worth colouring and the other is noise
        // — a log where every line is Info tells you nothing.
        assert_eq!(classify_line("    Finished `dev` profile [unoptimized]"),
                   LogLevel::Info);
        assert_eq!(classify_line("   Compiling serde v1.0.203"), LogLevel::Debug);
    }

    #[test]
    fn a_command_echo_mentioning_warnings_is_not_a_warning() {
        // `cargo build -- -D warnings` in a log header would otherwise paint
        // the whole run yellow before it started.
        assert_eq!(classify_line("$ cargo build -- -D warnings"), LogLevel::Debug);
        assert_eq!(classify_line("RUSTFLAGS='-D warnings' cargo test"), LogLevel::Debug);
    }

    #[test]
    fn a_path_containing_error_is_not_an_error() {
        // Paths are the other place these words appear constantly.
        assert_eq!(classify_line("     Running `target/debug/error_demo`"),
                   LogLevel::Debug);
        assert_eq!(classify_line("   Compiling foo (/src/errors)"), LogLevel::Debug);
    }

    #[test]
    fn test_results_are_classified_by_outcome() {
        assert_eq!(classify_line("test result: ok. 42 passed; 0 failed"), LogLevel::Info);
        assert_eq!(classify_line("test result: FAILED. 1 passed; 2 failed"),
                   LogLevel::Error);
    }

    #[test]
    fn a_passing_test_line_mentioning_failed_is_not_an_error() {
        // "0 failed" appears in every successful run, and matching on the word
        // alone turns every green suite red.
        assert_eq!(classify_line("test result: ok. 676 passed; 0 failed; 0 ignored"),
                   LogLevel::Info);
    }

    #[test]
    fn tick_and_cross_conventions_are_anchored_to_the_start() {
        // A cross INSIDE a message is content, not a verdict.
        assert_eq!(classify_line("✗ push rejected"), LogLevel::Error);
        assert_eq!(classify_line("✓ pushed 3 refs"), LogLevel::Info);
        assert_eq!(classify_line("renamed a✗b to c"), LogLevel::Debug);
    }

    #[test]
    fn an_empty_line_is_harmless() {
        assert_eq!(classify_line(""), LogLevel::Debug);
        assert_eq!(classify_line("    "), LogLevel::Debug);
    }
}
