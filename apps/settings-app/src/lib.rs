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

/// Which screen the editor is showing. `Home` is a list of sections, one big
/// row each: a phone screen shows one topic at a time, and a list you can
/// see all of beats a scroll you have to remember.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Page {
    Home,
    Display,
    Touch,
    Input,
    Theme,
    Access,
    Sleep,
    Voice,
    /// The phone shell's own settings (tideshell-settings' Display screen).
    Shell,
    Speaker,
    /// COSMIC's settings files (dark mode, screen off, suspend) via cosmic-levers.
    Cosmic,
    About,
}

impl Page {
    /// The sections, in list order, with a one-line summary each.
    pub const SECTIONS: [(Page, &'static str, &'static str); 11] = [
        (Page::Shell, "Phone shell", "text and control size, gloves, screen sleep, 3D clock"),
        (Page::Cosmic, "COSMIC", "dark mode, screen off, suspend"),
        (Page::Speaker, "Speaker", "the phone speaker's sound chain"),
        (Page::Display, "Display", "white point, colour filter, panel opacity"),
        (Page::Touch, "Touch & gestures", "sizes, swipe feel, detent animation"),
        (Page::Input, "Keyboard & input", "handedness, tilt, movement keys"),
        (Page::Theme, "Theme", "palette for every app"),
        (Page::Access, "Accessibility", "voice control, spoken feedback, filtering"),
        (Page::Sleep, "Sleeping hours", "bedside clock, OLED care"),
        (Page::Voice, "Voice mesh", "mic, offload, trusted partners"),
        (Page::About, "About", "where settings live, who reads them"),
    ];

    /// Deep link: `bsettings sleep` opens that section directly (a shell's
    /// palette or a notification can point straight at the setting).
    pub fn from_token(t: &str) -> Option<Page> {
        Some(match t.trim().to_lowercase().as_str() {
            "display" => Page::Display,
            "touch" | "gestures" => Page::Touch,
            "input" | "keyboard" => Page::Input,
            "theme" => Page::Theme,
            "access" | "accessibility" => Page::Access,
            "sleep" | "bedside" => Page::Sleep,
            "voice" => Page::Voice,
            "about" => Page::About,
            "shell" | "phone" => Page::Shell,
            "speaker" => Page::Speaker,
            "cosmic" => Page::Cosmic,
            _ => return None,
        })
    }

    pub fn title(self) -> &'static str {
        Page::SECTIONS.iter().find(|(p, _, _)| *p == self).map(|(_, t, _)| *t).unwrap_or("BigSilicon Settings")
    }
}

/// The settings editor: a working `Prefs` plus the last-published snapshot for diffing.
pub struct SettingsEditor {
    pub prefs: Prefs,
    published: Prefs,
    a11y: A11yScene,
    pub page: Page,
    new_partner: String,
    /// Shown on the About page: how edits are stored right now.
    pub store_note: String,
    /// The phone shell's settings screens, opened on first use (they read
    /// several files; no reason to on a desktop that never opens them).
    phone: Option<tideshell_settings::App>,
}

impl SettingsEditor {
    /// Start from the current central prefs (e.g. what `Settings::prefs()` reports on connect).
    pub fn new(initial: Prefs) -> SettingsEditor {
        SettingsEditor {
            prefs: initial.clone(),
            published: initial,
            a11y: A11yScene::new(),
            page: Page::Home,
            new_partner: String::new(),
            store_note: String::new(),
            phone: None,
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
        let colors = uikit_egui::palette_colors(&self.prefs.palette);
        self.a11y.begin_frame();
        // Big rows and buttons: this runs on a phone, often one-handed.
        let row_h = 48.0 * self.prefs.touch.button_scale.clamp(0.8, 1.8);

        if self.page == Page::Home {
            ui.heading("BigSilicon Settings");
            ui.colored_label(colors.text_weak, "Changes apply live to every app.");
            ui.separator();
            for (page, title, summary) in Page::SECTIONS {
                // A row, not a Button: a button centres one line of text, which
                // truncated the title and ran the summary through it.
                let (r, resp) = ui.allocate_exact_size(egui::vec2(ui.available_width(), row_h * 1.15), egui::Sense::click());
                let v = ui.style().interact(&resp);
                ui.painter().rect_filled(r, egui::CornerRadius::same(8), v.weak_bg_fill);
                ui.painter().text(r.left_top() + egui::vec2(14.0, r.height() * 0.30), egui::Align2::LEFT_CENTER, title, egui::FontId::proportional(17.0), v.text_color());
                ui.painter().text(r.left_top() + egui::vec2(14.0, r.height() * 0.70), egui::Align2::LEFT_CENTER, summary, egui::FontId::proportional(12.5), colors.text_weak);
                ui.painter().text(r.right_center() - egui::vec2(16.0, 0.0), egui::Align2::RIGHT_CENTER, "›", egui::FontId::proportional(22.0), colors.text_weak);
                // Registered so voice control and glove focus can reach each section.
                self.a11y.add(egui::Id::new(("bsettings_section", title)).value(), uikit_egui::a11y::Semantics::button(title), Some(r));
                if resp.clicked() {
                    self.page = page;
                }
                ui.add_space(4.0);
            }
            ui.add_space(10.0);
            if ui.add_sized([ui.available_width(), row_h * 0.8], egui::Button::new("reset everything to defaults")).clicked() {
                self.prefs = Prefs::default();
            }
            return self.pending();
        }

        if ui.add_sized([120.0, row_h * 0.8], egui::Button::new("←  back")).clicked() {
            self.page = Page::Home;
        }
        ui.heading(self.page.title());
        ui.separator();
        match self.page {
            Page::Display => self.display_page(ui, &colors),
            Page::Touch => self.touch_page(ui),
            Page::Input => self.input_page(ui, &colors),
            Page::Theme => self.theme_page(ui),
            Page::Access => self.access_page(ui),
            Page::Sleep => self.sleep_section(ui, &colors),
            Page::Voice => self.voice_page(ui, &colors),
            Page::About => self.about_page(ui, &colors),
            Page::Shell | Page::Speaker | Page::Cosmic => self.phone_page(ui, &colors),
            Page::Home => {}
        }
        self.pending()
    }

    fn display_page(&mut self, ui: &mut Ui, colors: &PaletteColors) {
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
        ui.add(Slider::new(&mut self.prefs.display.opacity, 0.6..=1.0).text("panel opacity"));
        ui.colored_label(colors.text_weak, "below 1.0, panels and cards let a live background show through");
    }

    fn touch_page(&mut self, ui: &mut Ui) {
        ui.label("Gesture feel");
        ui.add(Slider::new(&mut self.prefs.gesture.sensitivity, 0.25..=3.0).text("sensitivity"));
        ui.add(Slider::new(&mut self.prefs.gesture.commit_fraction, 0.1..=0.6).text("commit distance"));
        ui.add(Slider::new(&mut self.prefs.gesture.flick_velocity, 0.3..=2.0).text("flick velocity"));
        ui.horizontal_wrapped(|ui| {
            ui.label("detent animation:");
            for a in settings_kit::AnimationStyle::all() {
                let selected = self.prefs.gesture.animation == a;
                if ui.selectable_label(selected, a.label()).clicked() {
                    self.prefs.gesture.animation = a;
                }
            }
        });
        ui.add_space(6.0);
        ui.label("Sizes");
        // Text size re-scales the whole window, so applying it live makes the slider jump under the
        // cursor. Draft it during the drag and commit to prefs only on release. Reduced range with
        // quarter-step detents.
        {
            let id = egui::Id::new("bsettings_text_scale_draft");
            let mut draft: f32 = ui.data(|d| d.get_temp(id)).unwrap_or(self.prefs.touch.text_scale);
            let resp = ui.add(Slider::new(&mut draft, 0.75..=1.5).step_by(0.25).text("text size"));
            if resp.dragged() {
                ui.data_mut(|d| d.insert_temp(id, draft)); // hold in-progress; don't apply yet
            } else {
                if resp.drag_stopped() || resp.changed() {
                    self.prefs.touch.text_scale = draft; // commit on release / click / keyboard
                }
                ui.data_mut(|d| d.insert_temp(id, self.prefs.touch.text_scale));
            }
        }
        ui.add(Slider::new(&mut self.prefs.touch.button_scale, 0.8..=1.8).text("button size"));
        ui.add(Slider::new(&mut self.prefs.touch.min_target, 0.0..=72.0).text("min target (pt, 0 = auto)"));
        ui.add(Slider::new(&mut self.prefs.touch.scale, 0.8..=1.6).text("ui scale"));
    }

    fn input_page(&mut self, ui: &mut Ui, colors: &PaletteColors) {
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
        ui.horizontal_wrapped(|ui| {
            ui.label("movement keys:");
            for m in settings_kit::MovementLayout::all() {
                let selected = self.prefs.input.movement == m;
                if ui.selectable_label(selected, m.label()).clicked() {
                    self.prefs.input.movement = m;
                }
            }
        });
        {
            // show the live cluster so ".oeu" (Dvorak) etc. is legible at a glance
            let [f, l, b, r] = self.prefs.input.movement.keys();
            ui.colored_label(colors.text_weak, format!("forward {f} · left {l} · back {b} · right {r}"));
        }
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.label("model pool dir:");
            ui.add(egui::TextEdit::singleline(&mut self.prefs.pool_dir).id_salt("lib-self_prefs_pool_dir"));
        });
        ui.colored_label(colors.text_weak, "directory of *.gguf the fabric daemon shares to the LAN pool");
    }

    fn theme_page(&mut self, ui: &mut Ui) {
        ui.horizontal_wrapped(|ui| {
            for p in Self::palettes() {
                let selected = self.prefs.palette == p;
                if ui.selectable_label(selected, p).clicked() {
                    self.prefs.palette = p.to_string();
                }
            }
        });
    }

    fn access_page(&mut self, ui: &mut Ui) {
        // semantic toggles → this settings app is itself a11y-navigable
        semantic_toggle(ui, &mut self.a11y, 1, "voice control", &mut self.prefs.a11y.voice);
        semantic_toggle(ui, &mut self.a11y, 2, "spoken feedback (TTS)", &mut self.prefs.a11y.tts);
        ui.horizontal_wrapped(|ui| {
            ui.label("voice filtering:");
            for f in settings_kit::VoiceFilter::all() {
                let selected = self.prefs.a11y.voice_filter == f;
                if ui.selectable_label(selected, f.label()).clicked() {
                    self.prefs.a11y.voice_filter = f;
                }
            }
        });
        ui.small("filtering also applies to the keyboard's voice typing");
    }

    /// tideshell-settings' screens, embedded. They save to their own files
    /// (the shell's prefs, the speaker state, COSMIC's config) and tell the
    /// running shell, exactly as the standalone app did.
    fn phone_page(&mut self, ui: &mut Ui, colors: &PaletteColors) {
        use tideshell_settings::Screen;
        if self.phone.is_none() {
            match cosmic_levers::Store::from_env() {
                Some(store) => self.phone = Some(tideshell_settings::App::new(store)),
                None => {
                    ui.colored_label(colors.text_weak, "no $XDG_CONFIG_HOME or $HOME: nothing to edit");
                    return;
                }
            }
        }
        let screen = match self.page {
            Page::Speaker => Screen::Speaker,
            Page::Cosmic => Screen::Cosmic,
            _ => Screen::Display,
        };
        if let Some(app) = self.phone.as_mut() {
            app.screen_ui(ui, screen);
            // External edits (cosmic-settings, the shell's own sheet) show up.
            ui.ctx().request_repaint_after(tideshell_settings::REREAD);
        }
    }

    fn about_page(&mut self, ui: &mut Ui, colors: &PaletteColors) {
        ui.label(egui::RichText::new("Where these settings live").strong());
        ui.label(if self.store_note.is_empty() { "unknown" } else { &self.store_note });
        if let Some(p) = settings_file_path() {
            ui.monospace(p.display().to_string());
        }
        ui.add_space(6.0);
        ui.label(egui::RichText::new("Who reads them").strong());
        for line in [
            "every egui app: theme, sizes, gesture feel, colour filter (live, via settingsd)",
            "tideshell: sleeping hours (bedside clock), re-read within 10 s",
            "tideshell: Phone shell and Speaker sections (its own files; applied at once)",
            "COSMIC: the COSMIC section writes cosmic-config, which COSMIC applies live",
            "rusty-board keyboard: voice filtering, read when it starts",
            "voice mesh daemons: mic, offload, partners",
        ] {
            ui.colored_label(colors.text_weak, format!("· {line}"));
        }
        ui.add_space(6.0);
        ui.colored_label(colors.text_weak, format!("{} settings, bsettings {}", self.prefs.to_kv().len(), env!("CARGO_PKG_VERSION")));
    }

    /// Sleeping hours: the bedside window. Hour and minute as drag values
    /// (big targets, no keyboard needed on a phone); the window may wrap midnight.
    fn sleep_section(&mut self, ui: &mut Ui, colors: &PaletteColors) {
        let sp = &mut self.prefs.sleep;
        ui.checkbox(&mut sp.enabled, "use sleeping hours");
        ui.add_enabled_ui(sp.enabled, |ui| {
            // Steppers, not drag fields: a drag value is a precision gesture,
            // and this is set by thumb, often in bed. Hours step by 1, minutes
            // by 15 (a bedtime is not 23:07).
            let mut hm = |ui: &mut Ui, label: &str, mins: &mut u16| {
                ui.horizontal(|ui| {
                    ui.add_sized([56.0, 44.0], egui::Label::new(label));
                    let btn = |ui: &mut Ui, t: &str| ui.add_sized([44.0, 44.0], egui::Button::new(t)).clicked();
                    if btn(ui, "−") {
                        *mins = step_time(*mins, -60);
                    }
                    ui.add_sized([72.0, 44.0], egui::Label::new(egui::RichText::new(format!("{:02}:{:02}", *mins / 60, *mins % 60)).size(22.0).monospace()));
                    if btn(ui, "+") {
                        *mins = step_time(*mins, 60);
                    }
                    ui.add_space(8.0);
                    if btn(ui, "−15") {
                        *mins = step_time(*mins, -15);
                    }
                    if btn(ui, "+15") {
                        *mins = step_time(*mins, 15);
                    }
                });
            };
            hm(ui, "from", &mut sp.start_min);
            hm(ui, "until", &mut sp.end_min);
            ui.checkbox(&mut sp.pulse, "show a dim clock once a minute (screen off otherwise)");
            if sp.start_min == sp.end_min {
                ui.colored_label(colors.text_weak, "from and until are the same: the window is empty");
            }
        });
        ui.colored_label(colors.text_weak, "the phone shell keeps the OLED dark and the clock dim in this window");
    }

    /// The voice sub-page: mesh toggles + a reorderable trusted-partner tier list. Edits
    /// `self.prefs.voice.*`; the caller publishes the diff exactly as on the main page.
    fn voice_page(&mut self, ui: &mut Ui, colors: &PaletteColors) {
        ui.colored_label(colors.text_weak, "Speech across the fleet — offload heavy compute to trusted partners.");
        ui.separator();

        ui.checkbox(&mut self.prefs.voice.mic_enabled, "This device's mic joins the mesh");
        ui.checkbox(&mut self.prefs.voice.offload_enabled, "Offload heavy transcription to a trusted partner");
        ui.checkbox(&mut self.prefs.voice.speaker_id, "Recognize who is speaking (multi-user)");
        ui.horizontal(|ui| {
            ui.label("compute host:");
            ui.add(egui::TextEdit::singleline(&mut self.prefs.voice.compute_host).id_salt("lib-voice_compute_host").hint_text("host:port, empty = automatic"));
        });
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
            ui.add(egui::TextEdit::singleline(&mut self.new_partner).id_salt("lib-self_new_partner"));
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

/// A time of day moved by `delta` minutes, wrapping at midnight.
pub fn step_time(mins: u16, delta: i32) -> u16 {
    ((mins as i32 + delta).rem_euclid(1440)) as u16
}

/// settingsd's persistence file: `$XDG_CONFIG_HOME/gitmind/settings.conf`.
pub fn settings_file_path() -> Option<std::path::PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(std::path::PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".config")))?;
    Some(base.join("gitmind").join("settings.conf"))
}

/// Merge `changes` into settingsd's file text: existing keys are rewritten in
/// place, new ones appended, everything else (comments, keys this build does
/// not know) kept as it was. For when settingsd is not running: the edit still
/// lands where settingsd will load it from, and where readers that watch the
/// file (tideshell's sleeping hours) find it.
pub fn merge_settings_text(text: &str, changes: &[(String, String)]) -> String {
    let mut done = vec![false; changes.len()];
    let mut out = String::new();
    for line in text.lines() {
        let key = line.split_once('=').map(|(k, _)| k.trim());
        match key.and_then(|k| changes.iter().position(|(ck, _)| ck == k)) {
            Some(i) => {
                out.push_str(&format!("{} = {}\n", changes[i].0, changes[i].1));
                done[i] = true;
            }
            None => {
                out.push_str(line);
                out.push('\n');
            }
        }
    }
    for (i, (k, v)) in changes.iter().enumerate() {
        if !done[i] {
            out.push_str(&format!("{k} = {v}\n"));
        }
    }
    out
}

/// Read the file into Prefs (defaults for anything missing).
pub fn prefs_from_settings_text(text: &str) -> Prefs {
    let kv: Vec<(String, String)> = text
        .lines()
        .filter(|l| !l.trim_start().starts_with('#'))
        .filter_map(|l| l.split_once('=').map(|(k, v)| (k.trim().to_string(), v.trim().to_string())))
        .collect();
    Prefs::from_kv(&kv)
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
    fn offline_edits_merge_into_the_settingsd_file_without_losing_anything() {
        let text = "# settingsd\ntheme.palette = Dark\nsome.future.key = 7\n";
        let merged = merge_settings_text(text, &[("theme.palette".into(), "Nord".into()), ("sleep.hours".into(), "23:00-07:00".into())]);
        assert_eq!(merged, "# settingsd\ntheme.palette = Nord\nsome.future.key = 7\nsleep.hours = 23:00-07:00\n");
        let p = prefs_from_settings_text(&merged);
        assert_eq!(p.palette, "Nord");
        assert!(p.sleep.enabled && p.sleep.start_min == 23 * 60);
    }

    #[test]
    fn time_steppers_wrap_midnight() {
        assert_eq!(step_time(23 * 60 + 45, 15), 0);
        assert_eq!(step_time(0, -60), 23 * 60);
        assert_eq!(step_time(7 * 60, 15), 7 * 60 + 15);
    }

    #[test]
    fn every_section_has_a_title_and_home_is_the_start() {
        let e = SettingsEditor::new(Prefs::default());
        assert_eq!(e.page, Page::Home);
        for (p, t, _) in Page::SECTIONS {
            assert_eq!(p.title(), t);
        }
        assert_eq!(Page::from_token("Sleep"), Some(Page::Sleep));
        assert_eq!(Page::from_token("nope"), None);
    }

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
