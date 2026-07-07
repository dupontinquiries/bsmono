//! bigsilicon-settings — the central settings app.
//!
//! An egui UI (on uikit-egui) that edits the shared [`Prefs`](settings_kit::Prefs) and
//! publishes each change to the sharemd `settings` namespace so every egui-kit app updates
//! live. The editor keeps a working copy and a "last published" copy; [`SettingsEditor::ui`]
//! renders the controls and returns exactly the key/values that changed, which the on-device
//! `main` hands to `bigsilicon_client::Settings::set`.
//!
//! The diff + the schema mapping are pure and tested; the on-device `main` is a thin loop:
//! `Settings::connect()` → each frame `editor.ui(...)` → publish the returned changes.

use egui::{Slider, Ui};
use settings_kit::Prefs;
use uikit_egui::{a11y::semantic_toggle, A11yScene, Adaptive, PaletteColors};

/// Which screen the editor is showing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Page {
    Main,
    Voice,
}

/// The settings editor: a working `Prefs` plus the last-published snapshot for diffing.
pub struct SettingsEditor {
    pub prefs: Prefs,
    published: Prefs,
    a11y: A11yScene,
    page: Page,
    new_partner: String,
}

impl SettingsEditor {
    /// Start from the current central prefs (e.g. what `Settings::prefs()` reports on connect).
    pub fn new(initial: Prefs) -> SettingsEditor {
        SettingsEditor {
            prefs: initial.clone(),
            published: initial,
            a11y: A11yScene::new(),
            page: Page::Main,
            new_partner: String::new(),
        }
    }

    /// The changes (key, value) since the last time they were published. The caller publishes
    /// these and then calls [`mark_published`].
    pub fn pending(&self) -> Vec<(String, String)> {
        diff(&self.published, &self.prefs)
    }

    /// Record that the current prefs were published.
    pub fn mark_published(&mut self) {
        self.published = self.prefs.clone();
    }

    /// The list of palette ids for the theme picker.
    pub fn palettes() -> Vec<&'static str> {
        snapclone_theme::PaletteId::all().iter().map(|p| p.as_str()).collect()
    }

    /// Render the settings UI. Returns the changes to publish this frame (empty if nothing
    /// changed). The caller publishes them via `bigsilicon_client::Settings::set` and calls
    /// `mark_published`.
    pub fn ui(&mut self, ui: &mut Ui, _adaptive: &Adaptive) -> Vec<(String, String)> {
        let colors = PaletteColors::from_palette(snapclone_theme::palette_for(snapclone_theme::PaletteId::all()[0]));
        self.a11y.begin_frame();

        if self.page == Page::Voice {
            self.voice_page(ui, &colors);
            return self.pending();
        }

        ui.heading("BigSilicon Settings");
        ui.colored_label(colors.text_weak, "Changes apply live to every app.");
        ui.separator();

        ui.label("Gesture feel");
        ui.add(Slider::new(&mut self.prefs.gesture.sensitivity, 0.25..=3.0).text("sensitivity"));
        ui.add(Slider::new(&mut self.prefs.gesture.commit_fraction, 0.1..=0.6).text("commit distance"));
        ui.add(Slider::new(&mut self.prefs.gesture.flick_velocity, 0.3..=2.0).text("flick velocity"));
        ui.add_space(6.0);

        ui.label("Swipe & interaction");
        ui.horizontal_wrapped(|ui| {
            ui.label("detent animation:");
            for a in settings_kit::AnimationStyle::all() {
                let selected = self.prefs.gesture.animation == a;
                if ui.selectable_label(selected, a.label()).clicked() {
                    self.prefs.gesture.animation = a;
                }
            }
        });
        ui.add(Slider::new(&mut self.prefs.touch.text_scale, 0.8..=1.8).text("text size"));
        ui.add(Slider::new(&mut self.prefs.touch.button_scale, 0.8..=1.8).text("button size"));
        ui.horizontal_wrapped(|ui| {
            ui.label("handedness:");
            for h in settings_kit::Handedness::all() {
                let selected = self.prefs.input.handedness == h;
                if ui.selectable_label(selected, h.label()).clicked() {
                    self.prefs.input.handedness = h;
                }
            }
        });
        semantic_toggle(ui, &mut self.a11y, 3, "double-tilt device to flip handedness", &mut self.prefs.input.double_tilt);
        ui.add_space(6.0);

        ui.label("Touch");
        ui.add(Slider::new(&mut self.prefs.touch.min_target, 0.0..=72.0).text("min target (pt, 0 = auto)"));
        ui.add(Slider::new(&mut self.prefs.touch.scale, 0.8..=1.6).text("ui scale"));
        ui.add_space(6.0);

        ui.label("Theme");
        ui.horizontal_wrapped(|ui| {
            for p in Self::palettes() {
                let selected = self.prefs.palette == p;
                if ui.selectable_label(selected, p).clicked() {
                    self.prefs.palette = p.to_string();
                }
            }
        });
        ui.add_space(6.0);

        ui.label("Accessibility");
        // semantic toggles → this settings app is itself a11y-navigable
        semantic_toggle(ui, &mut self.a11y, 1, "voice control", &mut self.prefs.a11y.voice);
        semantic_toggle(ui, &mut self.a11y, 2, "spoken feedback (TTS)", &mut self.prefs.a11y.tts);
        if self.prefs.a11y.voice {
            ui.horizontal_wrapped(|ui| {
                ui.label("voice filtering:");
                for f in settings_kit::VoiceFilter::all() {
                    let selected = self.prefs.a11y.voice_filter == f;
                    if ui.selectable_label(selected, f.label()).clicked() {
                        self.prefs.a11y.voice_filter = f;
                    }
                }
            });
        }
        ui.add_space(6.0);

        ui.label("Display");
        ui.add(Slider::new(&mut self.prefs.display.white_point, 0.4..=1.0).text("white point (lower dims bright whites)"));
        ui.horizontal_wrapped(|ui| {
            ui.label("color filter:");
            for f in settings_kit::ColorFilter::all() {
                let selected = self.prefs.display.color_filter == f;
                if ui.selectable_label(selected, f.label()).clicked() {
                    self.prefs.display.color_filter = f;
                }
            }
        });
        ui.add_space(8.0);

        if ui.button("reset to defaults").clicked() {
            self.prefs = Prefs::default();
        }

        ui.separator();
        if ui.button("🎙  Voice partners  →").clicked() {
            self.page = Page::Voice;
        }

        self.pending()
    }

    /// The voice sub-page: mesh toggles + a reorderable trusted-partner tier list. Edits
    /// `self.prefs.voice.*`; the caller publishes the diff exactly as on the main page.
    fn voice_page(&mut self, ui: &mut Ui, colors: &PaletteColors) {
        if ui.button("←  back").clicked() {
            self.page = Page::Main;
        }
        ui.heading("Voice mesh");
        ui.colored_label(colors.text_weak, "Speech across the fleet — offload heavy compute to trusted partners.");
        ui.separator();

        ui.checkbox(&mut self.prefs.voice.mic_enabled, "This device's mic joins the mesh");
        ui.checkbox(&mut self.prefs.voice.offload_enabled, "Offload heavy transcription to a trusted partner");
        ui.checkbox(&mut self.prefs.voice.speaker_id, "Recognize who is speaking (multi-user)");
        ui.add_space(8.0);

        ui.label("Trusted voice partners (tier list — best first)");
        ui.colored_label(colors.text_weak, "The mesh prefers the highest reachable partner for heavy speech compute.");

        let partners = self.prefs.voice.partners.clone();
        let n = partners.len();
        let mut swap: Option<(usize, usize)> = None;
        let mut remove: Option<usize> = None;
        for (i, p) in partners.iter().enumerate() {
            ui.horizontal(|ui| {
                ui.label(format!("{}.", i + 1));
                ui.label(p);
                if i > 0 && ui.small_button("↑").clicked() {
                    swap = Some((i, i - 1));
                }
                if i + 1 < n && ui.small_button("↓").clicked() {
                    swap = Some((i, i + 1));
                }
                if ui.small_button("✕").clicked() {
                    remove = Some(i);
                }
            });
        }
        if let Some((a, b)) = swap {
            self.prefs.voice.partners.swap(a, b);
        }
        if let Some(i) = remove {
            self.prefs.voice.partners.remove(i);
        }
        if partners.is_empty() {
            ui.colored_label(colors.text_weak, "No partners yet — add a trusted device id below.");
        }

        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.text_edit_singleline(&mut self.new_partner);
            if ui.button("add partner").clicked() {
                let id = self.new_partner.trim().to_string();
                if !id.is_empty() && !self.prefs.voice.partners.contains(&id) {
                    self.prefs.voice.partners.push(id);
                }
                self.new_partner.clear();
            }
        });
    }
}

/// The changed (key, value) pairs between two prefs — what to publish. Because `to_kv`
/// emits a stable key order, we can zip the two flat forms and keep the ones that differ.
pub fn diff(old: &Prefs, new: &Prefs) -> Vec<(String, String)> {
    old.to_kv()
        .into_iter()
        .zip(new.to_kv())
        .filter(|((_, ov), (_, nv))| ov != nv)
        .map(|((_, _), (k, v))| (k, v))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diff_reports_only_changed_keys() {
        let old = Prefs::default();
        let mut new = old.clone();
        new.gesture.sensitivity = 2.0;
        new.palette = "Nord".into();
        let d = diff(&old, &new);
        // exactly the two changed keys, with new values
        assert!(d.contains(&("gesture.sensitivity".to_string(), "2".to_string())));
        assert!(d.contains(&("theme.palette".to_string(), "Nord".to_string())));
        assert_eq!(d.len(), 2);
    }

    #[test]
    fn diff_reports_display_changes() {
        let old = Prefs::default();
        let mut new = old.clone();
        new.display.white_point = 0.6;
        new.display.color_filter = settings_kit::ColorFilter::Tritanopia;
        let d = diff(&old, &new);
        assert!(d.iter().any(|(k, v)| k == "display.white_point" && v == "0.6"));
        assert!(d.iter().any(|(k, v)| k == "display.color_filter" && v == "tritanopia"));
    }

    #[test]
    fn diff_reports_swipe_interaction_changes() {
        let old = Prefs::default();
        let mut new = old.clone();
        new.gesture.animation = settings_kit::AnimationStyle::Snappy;
        new.touch.text_scale = 1.4;
        new.input.handedness = settings_kit::Handedness::Left;
        new.input.double_tilt = true;
        let d = diff(&old, &new);
        assert!(d.iter().any(|(k, v)| k == "gesture.animation" && v == "snappy"));
        assert!(d.iter().any(|(k, v)| k == "touch.text_scale" && v == "1.4"));
        assert!(d.iter().any(|(k, v)| k == "input.handedness" && v == "left"));
        assert!(d.iter().any(|(k, v)| k == "input.double_tilt" && v == "true"));
    }

    #[test]
    fn no_change_no_publish() {
        let p = Prefs::default();
        assert!(diff(&p, &p).is_empty());
    }

    #[test]
    fn editor_tracks_pending_until_published() {
        let mut ed = SettingsEditor::new(Prefs::default());
        assert!(ed.pending().is_empty());
        ed.prefs.gesture.commit_fraction = 0.5;
        assert_eq!(ed.pending(), vec![("gesture.commit_fraction".to_string(), "0.5".to_string())]);
        ed.mark_published();
        assert!(ed.pending().is_empty(), "nothing pending once published");
    }

    #[test]
    fn palettes_are_available() {
        assert!(!SettingsEditor::palettes().is_empty());
    }
}

#[cfg(test)]
mod voice_page_tests {
    use super::*;
    #[test]
    fn voice_edits_are_published_as_diff() {
        let mut ed = SettingsEditor::new(Prefs::default());
        ed.prefs.voice.partners = vec!["desktop".into(), "laptop".into()];
        ed.prefs.voice.offload_enabled = false;
        let changes = ed.pending();
        assert!(changes.iter().any(|(k, v)| k == "voice.partners" && v == "desktop,laptop"));
        assert!(changes.iter().any(|(k, v)| k == "voice.offload_enabled" && v == "false"));
    }
}
