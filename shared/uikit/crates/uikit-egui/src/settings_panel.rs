//! settings_panel — egui port of `main.qml`'s `settingsPopup` (L27729).
//!
//! The one screen with no egui counterpart at all. Every other audited
//! surface existed and was missing pieces; this one was absent, so
//! everything it reaches — model setup, chat, the recipe list, session
//! history, voice activation, theme — had no route from the port.
//!
//! ## Navigation, not configuration
//!
//! Four of the six entries are DOORS: they open another screen and
//! nothing more. Only voice activation and theme are settings in the
//! sense of a value the panel owns. That shape is why this returns a
//! [`SettingsEvent`] rather than mutating a config struct — a widget
//! that opened the chat panel itself would need a handle to it, and to
//! the model setup, and to the session store, which is most of the app.
//!
//! The QML draws the doors with a trailing `→` for exactly this reason:
//! it distinguishes "this goes somewhere" from "this changes something",
//! and losing that distinction is how a settings screen becomes a place
//! people are afraid to click.

use egui::{RichText, Ui};

/// What the panel asked the host to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingsEvent {
    /// "⚡ Set up models (LLM + voice)" — opens the setup wizard.
    OpenSetup,
    /// "💬 Chat with local model".
    OpenChat,
    /// "Recipes — what web2qml can do".
    OpenRecipes,
    /// "Sessions — what you've done".
    OpenSessions,
    /// Voice activation was toggled; the new value is in [`Settings`].
    VoiceActivationChanged,
    /// Theme index changed.
    ThemeChanged,
}

/// The values the panel owns. Everything else it only navigates to.
#[derive(Debug, Clone, PartialEq)]
pub struct Settings {
    /// Wake-word listening on.
    pub voice_activation: bool,
    /// Selected theme index into `themes`.
    pub theme: usize,
    /// Theme names, host-supplied — the panel shows what it is given
    /// rather than carrying a palette list that would drift from the
    /// real one.
    pub themes: Vec<String>,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            voice_activation: false,
            theme: 0,
            themes: vec!["dark".into(), "light".into()],
        }
    }
}

impl Settings {
    /// Clamp the theme index to the list. A stored index from a build
    /// with more themes must not panic or silently select nothing — it
    /// falls back to the first, which always exists when the list does.
    pub fn theme_name(&self) -> &str {
        self.themes
            .get(self.theme)
            .or_else(|| self.themes.first())
            .map(String::as_str)
            .unwrap_or("default")
    }

    /// Move to the next theme, wrapping. The QML cycles rather than
    /// offering a dropdown, which is right for two or three themes and
    /// is what the dot control does.
    pub fn cycle_theme(&mut self) {
        if self.themes.is_empty() {
            return;
        }
        self.theme = (self.theme + 1) % self.themes.len();
    }
}

/// Draw the panel. Returns at most one event per frame — the first
/// thing clicked, since these open exclusive screens and two at once is
/// not a state the host can honour.
pub fn settings_ui(ui: &mut Ui, s: &mut Settings) -> Option<SettingsEvent> {
    let mut event = None;

    ui.label(RichText::new("Settings").heading().strong());
    ui.add_space(8.0);

    // The doors. Each carries a trailing arrow, as the QML does, so a
    // navigation reads differently from a toggle at a glance.
    for (label, ev) in [
        ("⚡ Set up models (LLM + voice)", SettingsEvent::OpenSetup),
        ("💬 Chat with local model", SettingsEvent::OpenChat),
        ("✨ Recipes — what web2qml can do", SettingsEvent::OpenRecipes),
        ("⏱ Sessions — what you've done", SettingsEvent::OpenSessions),
    ] {
        ui.horizontal(|ui| {
            if ui.button(label).clicked() && event.is_none() {
                event = Some(ev);
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(RichText::new("→").weak());
            });
        });
    }

    ui.add_space(10.0);
    ui.separator();

    // The two actual settings.
    ui.label(RichText::new("🎙 Voice activation").strong().small());
    if ui.checkbox(&mut s.voice_activation, "listen for the wake word").changed()
        && event.is_none()
    {
        event = Some(SettingsEvent::VoiceActivationChanged);
    }

    ui.add_space(6.0);
    ui.horizontal(|ui| {
        ui.label(RichText::new("Theme").weak().small());
        if ui.button(s.theme_name().to_string()).clicked() && event.is_none() {
            s.cycle_theme();
            event = Some(SettingsEvent::ThemeChanged);
        }
    });

    event
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cycling_themes_wraps_and_never_leaves_the_list() {
        let mut s = Settings { themes: vec!["a".into(), "b".into(), "c".into()], ..Default::default() };
        assert_eq!(s.theme_name(), "a");
        s.cycle_theme();
        assert_eq!(s.theme_name(), "b");
        s.cycle_theme();
        s.cycle_theme();
        assert_eq!(s.theme_name(), "a", "wrapped back to the first");
    }

    #[test]
    fn a_stored_index_past_the_end_falls_back_rather_than_selecting_nothing() {
        // A config written by a build with more themes must not leave the
        // panel showing nothing, or panic on the index.
        let s = Settings { theme: 99, themes: vec!["only".into()], ..Default::default() };
        assert_eq!(s.theme_name(), "only");
    }

    #[test]
    fn an_empty_theme_list_is_not_a_panic() {
        let mut s = Settings { theme: 0, themes: vec![], ..Default::default() };
        assert_eq!(s.theme_name(), "default");
        s.cycle_theme(); // must not divide by zero
        assert_eq!(s.theme, 0);
    }

    #[test]
    fn the_doors_and_the_toggles_are_different_kinds_of_event() {
        // The distinction the QML draws with its trailing arrows, kept in
        // the type: four entries navigate, two change a value the panel
        // owns. A host routing these does not have to guess which is
        // which.
        let navigations = [
            SettingsEvent::OpenSetup,
            SettingsEvent::OpenChat,
            SettingsEvent::OpenRecipes,
            SettingsEvent::OpenSessions,
        ];
        let changes = [SettingsEvent::VoiceActivationChanged, SettingsEvent::ThemeChanged];
        for n in navigations {
            assert!(!changes.contains(&n), "{n:?} is both a door and a setting");
        }
    }
}
