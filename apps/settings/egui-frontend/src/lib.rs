//! settings-egui — the central BigSilicon settings app.
//!
//! An egui UI (on uikit-egui) that edits the shared [`Prefs`] and returns the changed
//! key/values to publish over the sharemd bus (via `bigsilicon-client::Settings`), so every
//! egui-kit app updates live. This crate is the UI + the change-detection; the transport is
//! the Settings client. The editor logic (which controls, what ranges) is here; the pure
//! `changed_keys` helper is unit-tested without a live UI.

use egui::Ui;
use settings_kit::{diff, Prefs};

/// A live palette choice list (from snapclone-theme).
fn palette_ids() -> Vec<&'static str> {
    snapclone_theme::PaletteId::all().iter().map(|p| p.as_str()).collect()
}

/// Render the settings editor into `ui`, mutating `prefs` in place. Returns the list of
/// changed `(key, value)` pairs to publish (empty if nothing changed this frame). The caller
/// forwards them to `Settings::set`.
pub fn settings_editor_ui(ui: &mut Ui, prefs: &mut Prefs) -> Vec<(String, String)> {
    let before = prefs.clone();

    ui.heading("BigSilicon settings");
    ui.label("Changes apply live to every egui-kit app.");
    ui.separator();

    ui.label("Touch gesture feel");
    ui.add(egui::Slider::new(&mut prefs.gesture.sensitivity, 0.25..=3.0).text("sensitivity"));
    ui.add(egui::Slider::new(&mut prefs.gesture.commit_fraction, 0.1..=0.6).text("commit distance"));
    ui.add(egui::Slider::new(&mut prefs.gesture.flick_velocity, 0.3..=2.5).text("flick velocity"));
    ui.separator();

    ui.label("Touch targets");
    ui.add(egui::Slider::new(&mut prefs.touch.min_target, 0.0..=72.0).text("min size (0 = auto)"));
    ui.add(egui::Slider::new(&mut prefs.touch.scale, 0.75..=1.6).text("ui scale"));
    ui.separator();

    ui.label("Theme");
    egui::ComboBox::from_label("palette")
        .selected_text(prefs.palette.clone())
        .show_ui(ui, |ui| {
            for id in palette_ids() {
                ui.selectable_value(&mut prefs.palette, id.to_string(), id);
            }
        });
    ui.separator();

    ui.label("Accessibility");
    ui.checkbox(&mut prefs.a11y.voice, "voice control");
    ui.checkbox(&mut prefs.a11y.tts, "spoken feedback (TTS)");

    diff(&before, prefs)
}

/// The central settings app: holds the working Prefs and turns editor changes into bus
/// publishes. Transport-agnostic — the caller wires `publish` to `Settings::set` (so this
/// stays testable and doesn't force a sharemd connection).
pub struct SettingsApp<F: FnMut(&str, &str)> {
    pub prefs: Prefs,
    publish: F,
}

impl<F: FnMut(&str, &str)> SettingsApp<F> {
    pub fn new(prefs: Prefs, publish: F) -> Self {
        SettingsApp { prefs, publish }
    }

    /// Draw the editor and publish anything the user changed.
    pub fn ui(&mut self, ui: &mut Ui) {
        for (k, v) in settings_editor_ui(ui, &mut self.prefs) {
            (self.publish)(&k, &v);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn changed_prefs_produce_publishes() {
        // simulate the editor changing a couple of fields, then check what would publish
        let before = Prefs::default();
        let mut after = before.clone();
        after.gesture.sensitivity = 2.2;
        after.a11y.tts = true;
        let published = diff(&before, &after);
        assert!(published.contains(&("gesture.sensitivity".to_string(), "2.2".to_string())));
        assert!(published.contains(&("a11y.tts".to_string(), "true".to_string())));
        assert_eq!(published.len(), 2);
    }

    #[test]
    fn settings_app_forwards_changes_to_publish() {
        use std::cell::RefCell;
        use std::rc::Rc;
        let sent: Rc<RefCell<Vec<(String, String)>>> = Rc::new(RefCell::new(Vec::new()));
        let sink = sent.clone();
        let mut app = SettingsApp::new(Prefs::default(), move |k: &str, v: &str| sink.borrow_mut().push((k.into(), v.into())));
        // directly exercise the publish path (no egui context needed)
        app.prefs.palette = "Nord".into();
        let changes = diff(&Prefs::default(), &app.prefs);
        for (k, v) in changes {
            (app.publish)(&k, &v);
        }
        assert_eq!(sent.borrow().as_slice(), &[("theme.palette".to_string(), "Nord".to_string())]);
    }

    #[test]
    fn palette_list_is_nonempty() {
        assert!(!palette_ids().is_empty());
    }
}
