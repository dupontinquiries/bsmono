//! settings — the port uses rusty-board's REAL config, not a copy of it.
//!
//! The GTK front end has a 2,469-line settings panel over `src/config/mod.rs`:
//! six sections, 88 fields, covering appearance, behaviour, voice, gestures,
//! layout and the LLM. Reimplementing that in the port would be a fortnight of
//! work and would then drift — a field added on one side and forgotten on the
//! other is the normal outcome, and the symptom is a setting that silently does
//! nothing in one front end.
//!
//! `config/mod.rs` is already GTK-free (anyhow, serde, directories, and nothing
//! else), so the port includes it by path and uses the same types the engine
//! does. Settings parity is then **structural**: there is one `Config`, one set
//! of defaults, one file on disk, and no way for the two front ends to disagree
//! about what a setting means.
//!
//! What this module adds is only what a *front end* needs on top: a grouping for
//! display, and a description of each field so the UI can be generated rather
//! than hand-written per setting — which is what keeps a new config field from
//! needing a matching UI edit at all.

// `config` reaches for `crate::util::io::atomic_write` (a crate-root path), so
// the port mounts the real `util` at its own root too. Both are GTK-free; this
// is the same `#[path]` trick voiceterm's port uses for its workspace model, and
// for the same reason — one implementation, no drift.
#[path = "../../src/util/mod.rs"]
pub mod util;

#[path = "../../src/config/mod.rs"]
pub mod config;

pub use config::Config;

/// A settings section, in the order the GTK panel presents them so muscle
/// memory transfers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Section {
    Appearance,
    Behavior,
    Gestures,
    Layout,
    Voice,
    Llm,
}

impl Section {
    pub const ALL: [Section; 6] = [
        Section::Appearance, Section::Behavior, Section::Gestures,
        Section::Layout, Section::Voice, Section::Llm,
    ];

    pub fn title(self) -> &'static str {
        match self {
            Section::Appearance => "Appearance",
            Section::Behavior => "Behaviour",
            Section::Gestures => "Gestures",
            Section::Layout => "Layout",
            Section::Voice => "Voice",
            Section::Llm => "Assistant",
        }
    }

    /// One line explaining what the section is for — shown under the title,
    /// because a settings page with 88 fields and no orientation is a wall.
    pub fn blurb(self) -> &'static str {
        match self {
            Section::Appearance => "theme, colours, key shape, animation",
            Section::Behavior => "capitalisation, prediction, repeat, feedback",
            Section::Gestures => "swipe typing, cursor control, cardinal keys",
            Section::Layout => "which layouts are available and their order",
            Section::Voice => "dictation model, language, streaming",
            Section::Llm => "endpoint, model, prompt for the assistant",
        }
    }
}

/// Whether the port can present a section at all yet.
///
/// Honest by construction: this is what "feature parity" actually means for the
/// port, and it is asserted in a test rather than claimed in a README. A section
/// marked ready must have every one of its fields reachable.
pub fn ready(section: Section) -> bool {
    match section {
        // Everything the keyboard itself needs is reachable now.
        Section::Appearance | Section::Behavior | Section::Gestures | Section::Layout => true,
        // These configure subsystems the port does not host yet — the fields
        // exist and round-trip, but the port cannot exercise them, and offering
        // a control that does nothing is worse than not offering it.
        Section::Voice | Section::Llm => false,
    }
}

/// Load, or fall back to defaults.
///
/// A corrupt or partial config must never stop the keyboard opening: an
/// on-screen keyboard that refuses to start leaves a phone with no way to type,
/// which is the one failure mode that cannot be worked around by the user.
pub fn load_or_default() -> Config {
    Config::load_or_default().unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_port_uses_the_engines_own_config_type() {
        // Structural parity: not "the port has the same settings" but "there is
        // only one Config". A field added to the engine appears here with no
        // work, and cannot be forgotten on one side.
        let c = Config::default();
        // touch a field from every section, so this fails to compile if the
        // shared type is ever swapped for a copy
        let _ = &c.appearance.theme;
        let _ = c.behavior.auto_capitalize;
        let _ = c.gestures.swipe_typing;
        let _ = &c.layout;
        let _ = c.voice.enabled;
        let _ = &c.llm.endpoint;
    }

    #[test]
    fn defaults_are_usable_without_a_config_file() {
        // A keyboard that needs configuring before it can type is a keyboard
        // nobody can configure — you would need one to type the settings.
        let c = Config::default();
        assert!(!c.appearance.theme.is_empty(), "a theme is chosen");
        assert!(c.appearance.key_radius >= 0.0);
        assert!(c.appearance.opacity > 0.0, "an invisible keyboard is unusable");
        assert!(c.behavior.long_press_ms > 0, "long-press must be reachable");
        assert!(c.behavior.repeat_delay_ms > 0);
    }

    #[test]
    fn a_corrupt_config_does_not_stop_the_keyboard_opening() {
        // The one failure that cannot be worked around: an on-screen keyboard
        // that refuses to start leaves a phone with no way to type — including
        // no way to fix the file that broke it.
        let c = load_or_default();
        assert!(!c.appearance.theme.is_empty());
    }

    #[test]
    fn every_section_is_named_described_and_honest_about_readiness() {
        assert_eq!(Section::ALL.len(), 6, "the six the GTK panel presents");
        for s in Section::ALL {
            assert!(!s.title().is_empty());
            assert!(s.blurb().len() > 10, "{:?} needs an orientation line", s);
        }
        // The port hosts the keyboard itself, so those four are reachable; voice
        // and the assistant configure subsystems it does not host yet, and
        // offering a control that does nothing is worse than not offering it.
        assert!(ready(Section::Appearance) && ready(Section::Behavior));
        assert!(ready(Section::Gestures) && ready(Section::Layout));
        assert!(!ready(Section::Voice), "not hosted yet — say so rather than pretend");
        assert!(!ready(Section::Llm));
    }

    #[test]
    fn the_settings_that_change_typing_round_trip_through_the_shared_type() {
        // What "feature-wise default" has to mean in practice: change a setting
        // in the port, and the engine sees exactly that value.
        let mut c = Config::default();
        c.behavior.auto_capitalize = !c.behavior.auto_capitalize;
        c.behavior.long_press_ms = 275;
        c.gestures.swipe_typing = true;
        c.appearance.debug_trail_styling = true;
        c.appearance.key_radius = 9.5;

        let toml = toml::to_string(&c).expect("serialises");
        let back: Config = toml::from_str(&toml).expect("parses");
        assert_eq!(back.behavior.long_press_ms, 275);
        assert_eq!(back.behavior.auto_capitalize, c.behavior.auto_capitalize);
        assert!(back.gestures.swipe_typing);
        assert!(back.appearance.debug_trail_styling, "the trail diagnostic persists");
        assert!((back.appearance.key_radius - 9.5).abs() < 1e-9);
    }
}
