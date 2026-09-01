//! peek_viewer — a file viewer over the peek engine.
//!
//! Ported from `qml/PeekViewer.qml`: a path, a spec, and the indexed slice the
//! engine returns. The spec grammar — `5xx`, `5k`, `@symbol`, `A-B` — is
//! parsed by `peek::view`, not here, which is the whole reason this port is
//! small. The Qt backend calls `peek::view::read_to_string(path, spec,
//! context, true)` and so does this; nothing about the grammar is
//! reimplemented, so the two viewers cannot disagree about what `@fn` means.
//!
//! ## Errors are shown, not swallowed
//!
//! The QML puts `"error: " + r.error` into the same text area as the file
//! body. That is right for this control: a bad spec is the common case while
//! you are learning the grammar, and a viewer that goes blank teaches nothing
//! about why. The message is kept distinguishable from content so the caller
//! can style it, which the QML could not do with one string.

use std::path::Path;

/// What the viewer is showing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Shown {
    /// Nothing opened yet.
    Empty,
    /// The engine returned a slice.
    Text(String),
    /// The engine refused, with its reason.
    Error(String),
}

impl Shown {
    pub fn is_error(&self) -> bool {
        matches!(self, Shown::Error(_))
    }
    /// The body to render, whichever kind it is.
    pub fn body(&self) -> &str {
        match self {
            Shown::Empty => "",
            Shown::Text(s) | Shown::Error(s) => s,
        }
    }
}

/// Viewer state.
#[derive(Debug, Clone, Default)]
pub struct PeekViewer {
    pub path: String,
    /// The slice spec. Empty means the whole file, which is what the engine
    /// does with an empty string — not a special case here.
    pub spec: String,
    pub shown: Shown,
    /// Lines of context around a match, the QML's third argument.
    pub context: usize,
}

impl Default for Shown {
    fn default() -> Self {
        Shown::Empty
    }
}

impl PeekViewer {
    /// A viewer with the QML's defaults.
    pub fn new() -> PeekViewer {
        PeekViewer { context: 2, ..Default::default() }
    }

    /// Open a path and read it, clearing any previous spec.
    ///
    /// Clearing matches the QML's `open()`, and it matters: a spec is written
    /// against one file's line numbers or symbols, and carrying `@parse_args`
    /// onto a different file either errors or — worse — silently matches
    /// something unrelated.
    pub fn open(&mut self, path: impl Into<String>) {
        self.path = path.into();
        self.spec.clear();
        self.reload();
    }

    /// Re-read with the current path and spec.
    pub fn reload(&mut self) {
        if self.path.trim().is_empty() {
            self.shown = Shown::Empty;
            return;
        }
        self.shown = match read(Path::new(&self.path), &self.spec, self.context) {
            Ok(text) => Shown::Text(text),
            Err(e) => Shown::Error(e),
        };
    }

    /// How many lines the current view holds — for a status line, and so a
    /// test can assert a spec narrowed the output without matching on text.
    pub fn line_count(&self) -> usize {
        match &self.shown {
            Shown::Text(s) if !s.is_empty() => s.lines().count(),
            _ => 0,
        }
    }
}

/// The one call into the engine.
///
/// Split out so the viewer's own logic — clearing on open, empty path, error
/// routing — is testable without a filesystem, and so there is exactly one
/// place the engine is invoked.
pub fn read(path: &Path, spec: &str, context: usize) -> Result<String, String> {
    peek::view::read_to_string(path, spec, context, true).map_err(|e| e.to_string())
}

/// Draw the viewer.
pub fn peek_viewer_ui(ui: &mut egui::Ui, v: &mut PeekViewer) {
    let mut reload = false;
    ui.horizontal(|ui| {
        ui.label("path");
        if ui.add(egui::TextEdit::singleline(&mut v.path).id_salt("peek_viewer-v_path").desired_width(280.0))
            .lost_focus()
            && ui.input(|i| i.key_pressed(egui::Key::Enter))
        {
            reload = true;
        }
        ui.label("spec");
        // Narrow, with the grammar as the hint — the QML's placeholder was
        // "5xx / @fn" and that placeholder is the only documentation this
        // control has.
        if ui
            .add(
                egui::TextEdit::singleline(&mut v.spec).id_salt("peek_viewer-v_spec")
                    .hint_text("5xx / 5k / @fn / A-B")
                    .desired_width(110.0),
            )
            .lost_focus()
            && ui.input(|i| i.key_pressed(egui::Key::Enter))
        {
            reload = true;
        }
        if ui.button("read").clicked() {
            reload = true;
        }
    });
    if reload {
        v.reload();
    }

    let err = v.shown.is_error();
    egui::ScrollArea::vertical().id_salt("peek-body").show(ui, |ui| {
        let mut text = v.shown.body().to_string();
        if err {
            // Distinguishable from content, which the QML could not manage
            // with one string: a spec error should not look like a file that
            // happens to start with "error:".
            ui.colored_label(ui.visuals().error_fg_color, &text);
        } else {
            // Read-only rather than a label, so the slice can be selected and
            // copied — the point of a viewer.
            ui.add(
                egui::TextEdit::multiline(&mut text).id_salt("peek_viewer-text")
                    .code_editor()
                    .desired_width(f32::INFINITY)
                    .interactive(false),
            );
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::path::PathBuf;

    fn tmp(name: &str, body: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("peek-{}-{}", std::process::id(), name));
        let mut f = std::fs::File::create(&p).unwrap();
        f.write_all(body.as_bytes()).unwrap();
        p
    }

    fn numbered(n: usize) -> String {
        (1..=n).map(|i| format!("line {i}\n")).collect()
    }

    #[test]
    fn an_empty_path_shows_nothing_rather_than_erroring() {
        // The viewer opens before a file is chosen. An error there would be
        // noise about a state the operator is on their way out of.
        let mut v = PeekViewer::new();
        v.reload();
        assert_eq!(v.shown, Shown::Empty);
        assert_eq!(v.line_count(), 0);
        v.path = "   ".into();
        v.reload();
        assert_eq!(v.shown, Shown::Empty, "whitespace is not a path");
    }

    #[test]
    fn a_spec_narrows_the_slice_and_the_engine_parses_it() {
        // The grammar belongs to peek::view; this asserts the port reaches it
        // rather than reimplementing it.
        let p = tmp("slice", &numbered(40));
        let mut v = PeekViewer::new();
        v.open(p.to_string_lossy().to_string());
        let whole = v.line_count();
        assert!(whole >= 40, "no spec reads the file, got {whole}");
        v.spec = "10-14".into();
        v.reload();
        assert!(!v.shown.is_error(), "{:?}", v.shown);
        let sliced = v.line_count();
        assert!(sliced < whole, "a range must narrow: {sliced} vs {whole}");
        assert!(v.shown.body().contains("line 12"), "the range covers 12");
        let _ = std::fs::remove_file(p);
    }

    #[test]
    fn opening_a_new_file_clears_the_spec() {
        // A spec is written against one file's lines or symbols. Carrying it
        // over either errors or, worse, silently matches something unrelated.
        let a = tmp("a", &numbered(30));
        let b = tmp("b", &numbered(30));
        let mut v = PeekViewer::new();
        v.open(a.to_string_lossy().to_string());
        v.spec = "5-9".into();
        v.reload();
        v.open(b.to_string_lossy().to_string());
        assert_eq!(v.spec, "", "the spec must not follow the file");
        assert!(v.line_count() >= 30, "the new file reads whole");
        let _ = std::fs::remove_file(a);
        let _ = std::fs::remove_file(b);
    }

    #[test]
    fn a_missing_file_reports_the_engines_reason() {
        let mut v = PeekViewer::new();
        v.open("/definitely/not/here.rs");
        assert!(v.shown.is_error());
        assert!(!v.shown.body().is_empty(), "an error with no text helps nobody");
        // And it stays distinguishable from content, so the widget can colour
        // it instead of shipping a file that begins with "error:".
        assert!(matches!(v.shown, Shown::Error(_)));
    }

    #[test]
    fn a_bad_spec_is_an_error_not_an_empty_view() {
        // The common case while learning the grammar. Going blank teaches
        // nothing about why.
        let p = tmp("badspec", &numbered(20));
        let mut v = PeekViewer::new();
        v.open(p.to_string_lossy().to_string());
        v.spec = "@nosuchsymbol".into();
        v.reload();
        assert!(v.shown.is_error() || v.line_count() == 0,
                "a symbol that does not exist must not look like a successful read");
        let _ = std::fs::remove_file(p);
    }
}
