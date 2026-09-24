//! tideshell-settings: COSMIC's settings with fingers.
//!
//! cosmic-settings is not usable by touch on the phone, so this app covers
//! the settings a phone needs, reading and writing the same files COSMIC
//! does (shared/cosmic-levers). Changes apply live: COSMIC watches them.
//!
//! Layers: [`cosmic_levers`] is the engine (files, types, defaults); this
//! crate is [`App`] (state + [`Action`]s, testable without a window) and
//! [`App::ui`] (egui). The UI only produces actions; [`App::apply`] performs
//! them. Every row can show where its value came from (user file, system
//! default, built-in default) and the file's text, so what the app did is
//! always checkable on disk.

pub mod speaker;

use cosmic_levers::{Control, Lever, Reading, Source, Store, Value};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    Set(usize, Value),
    Reset(usize),
    ToggleDetails(usize),
}

/// Which screen is showing. Two, because they edit different things: the
/// levers are COSMIC's settings files, and the speaker is tideshell's own
/// live audio chain. Mixing them in one list would suggest they save to the
/// same place and behave the same way, and they do neither.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    Cosmic,
    Speaker,
}

pub struct App {
    pub screen: Screen,
    pub speaker: speaker::Speaker,
    pub store: Store,
    pub levers: Vec<Lever>,
    pub readings: Vec<Reading>,
    pub details: Option<usize>,
    /// The last thing done, shown at the top: "saved <path>" or an error.
    pub status: String,
    last_read: Instant,
}

/// External changes (cosmic-settings, a shell) show up within this.
pub const REREAD: Duration = Duration::from_secs(2);
/// Rows and buttons: a finger, not a cursor.
pub const TOUCH_H: f32 = 48.0;

impl App {
    pub fn new(store: Store) -> App {
        let levers = cosmic_levers::levers();
        let readings = levers.iter().map(|l| store.read(l)).collect();
        App { screen: Screen::Cosmic, speaker: speaker::Speaker::default(), store, levers, readings, details: None, status: String::new(), last_read: Instant::now() }
    }

    pub fn reread(&mut self) {
        self.readings = self.levers.iter().map(|l| self.store.read(l)).collect();
        self.last_read = Instant::now();
    }

    pub fn apply(&mut self, a: Action) {
        match a {
            Action::Set(i, v) => {
                let l = self.levers[i];
                self.status = match self.store.write(&l, v) {
                    Ok(p) => format!("{}: {} (saved {}; COSMIC applies it live)", l.label, v.describe(), p.display()),
                    Err(e) => format!("{}: not saved: {e}", l.label),
                };
                self.reread();
            }
            Action::Reset(i) => {
                let l = self.levers[i];
                self.status = match self.store.reset(&l) {
                    Ok(true) => format!("{}: back to the default", l.label),
                    Ok(false) => format!("{}: already at the default", l.label),
                    Err(e) => format!("{}: not reset: {e}", l.label),
                };
                self.reread();
            }
            Action::ToggleDetails(i) => self.details = if self.details == Some(i) { None } else { Some(i) },
        }
    }

    /// One frame, inside the window's root `Ui` (eframe 0.34's `App::ui`).
    /// Returns the actions it applied (for tests and traces).
    pub fn ui(&mut self, root: &mut egui::Ui) -> Vec<Action> {
        if self.last_read.elapsed() >= REREAD {
            self.reread();
        }
        root.ctx().request_repaint_after(REREAD);
        let mut actions = Vec::new();
        let screen = self.screen;
        let mut want_screen = screen;
        egui::CentralPanel::default().show_inside(root, |ui| {
            ui.heading("Settings");
            // Two finger-high tabs rather than a menu: there are two screens
            // and there will not be many more, and a menu costs a tap to
            // discover what a tab shows at rest.
            ui.horizontal(|ui| {
                if big(ui, "COSMIC", screen == Screen::Cosmic) {
                    want_screen = Screen::Cosmic;
                }
                if big(ui, "Speaker", screen == Screen::Speaker) {
                    want_screen = Screen::Speaker;
                }
            });
            if !self.status.is_empty() {
                ui.label(egui::RichText::new(&self.status).small());
            }
            ui.add_space(8.0);
            if screen == Screen::Speaker {
                egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                    speaker::ui(&mut self.speaker, ui, TOUCH_H);
                });
                return;
            }
            egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                let mut section = "";
                for i in 0..self.levers.len() {
                    let l = self.levers[i];
                    if l.section != section {
                        section = l.section;
                        ui.add_space(12.0);
                        ui.label(egui::RichText::new(section).strong().size(15.0));
                        ui.separator();
                    }
                    row(ui, i, &l, &self.readings[i], self.details == Some(i), &mut actions);
                }
            });
        });
        self.screen = want_screen;
        for a in actions.clone() {
            self.apply(a);
        }
        actions
    }
}

fn big(ui: &mut egui::Ui, text: &str, selected: bool) -> bool {
    let b = egui::Button::new(egui::RichText::new(text).size(16.0)).selected(selected).min_size(egui::vec2(TOUCH_H * 1.4, TOUCH_H));
    ui.add(b).clicked()
}

fn row(ui: &mut egui::Ui, i: usize, l: &Lever, r: &Reading, open: bool, out: &mut Vec<Action>) {
    ui.add_space(6.0);
    // The label is the details toggle: a whole-width, finger-high target.
    let head = format!("{}   {}{}", l.label, r.value.describe(), if r.problem.is_some() { "  (!)" } else { "" });
    let resp = ui.add(egui::Button::new(egui::RichText::new(head).size(17.0)).frame(false).min_size(egui::vec2(ui.available_width(), TOUCH_H)));
    if resp.clicked() {
        out.push(Action::ToggleDetails(i));
    }
    ui.horizontal_wrapped(|ui| match l.control {
        Control::Toggle => {
            let on = r.value == Value::Bool(true);
            if big(ui, "Off", !on) && on {
                out.push(Action::Set(i, Value::Bool(false)));
            }
            if big(ui, "On", on) && !on {
                out.push(Action::Set(i, Value::Bool(true)));
            }
        }
        Control::Choices(cs) => {
            for (label, v) in cs {
                let val = Value::OptMillis(*v);
                if big(ui, label, r.value == val) && r.value != val {
                    out.push(Action::Set(i, val));
                }
            }
        }
    });
    if open {
        ui.add_space(4.0);
        ui.label(l.help);
        let from = match &r.source {
            Source::User(p) => format!("your setting: {}", p.display()),
            Source::System(p) => format!("system default: {}", p.display()),
            Source::Default => format!("built-in default ({}); no file yet", l.default.describe()),
        };
        ui.label(egui::RichText::new(from).small().monospace());
        if let Some(raw) = &r.raw {
            ui.label(egui::RichText::new(format!("file says: {}", raw.trim())).small().monospace());
        }
        if let Some(p) = &r.problem {
            ui.colored_label(ui.visuals().warn_fg_color, p);
        }
        if matches!(r.source, Source::User(_)) && big(ui, "Reset to default", false) {
            out.push(Action::Reset(i));
        }
    }
    ui.add_space(6.0);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app(tag: &str) -> App {
        let base = std::env::temp_dir().join(format!("tideshell-settings-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        App::new(Store { config_home: base.join("home"), data_dirs: vec![base.join("share")] })
    }
    fn idx(a: &App, key: &str) -> usize {
        a.levers.iter().position(|l| l.entry.key == key).unwrap()
    }

    #[test]
    fn actions_write_the_files_cosmic_reads_and_reset_removes_them() {
        let mut a = app("act");
        let i = idx(&a, "is_dark");
        assert_eq!(a.readings[i].source, Source::Default);
        a.apply(Action::Set(i, Value::Bool(false)));
        assert_eq!(a.readings[i].value, Value::Bool(false));
        let p = a.store.config_home.join("cosmic/com.system76.CosmicTheme.Mode/v1/is_dark");
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "false");
        assert!(a.status.contains("saved") && a.status.contains("is_dark"), "{}", a.status);
        let s = idx(&a, "screen_off_time");
        a.apply(Action::Set(s, Value::OptMillis(Some(60_000))));
        assert_eq!(std::fs::read_to_string(a.store.config_home.join("cosmic/com.system76.CosmicIdle/v1/screen_off_time")).unwrap(), "Some(60000)");
        a.apply(Action::Reset(i));
        assert_eq!(a.readings[i].source, Source::Default);
        assert!(!p.exists());
        a.apply(Action::ToggleDetails(s));
        assert_eq!(a.details, Some(s));
        a.apply(Action::ToggleDetails(s));
        assert_eq!(a.details, None);
    }

    #[test]
    #[allow(deprecated)]
    fn a_frame_renders_every_setting_and_does_nothing_by_itself() {
        let mut a = app("frame");
        a.details = Some(0);
        let ctx = egui::Context::default();
        let mut acted = vec![];
        // The test drives egui without eframe, so it needs a root Ui of its
        // own; the deprecated calls are confined to this harness.
        let _ = ctx.run(egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(400.0, 900.0))), ..Default::default() }, |ctx| {
            egui::CentralPanel::default().frame(egui::Frame::NONE).show(ctx, |ui| acted = a.ui(ui));
        });
        assert!(acted.is_empty(), "no input, no action: {acted:?}");
        assert!(!a.store.config_home.exists(), "rendering writes nothing");
    }

    #[test]
    fn an_external_change_shows_up_on_the_next_read() {
        let mut a = app("ext");
        let i = idx(&a, "suspend_on_ac_time");
        let p = a.store.config_home.join("cosmic/com.system76.CosmicIdle/v1/suspend_on_ac_time");
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, "None").unwrap();
        a.reread();
        assert_eq!(a.readings[i].value, Value::OptMillis(None));
        assert_eq!(a.readings[i].source, Source::User(p));
    }
}

#[cfg(test)]
mod screen_tests {
    use super::*;

    fn app() -> App {
        // A store pointed at a scratch directory: the test must not read or
        // write the machine's real COSMIC config.
        let dir = std::env::temp_dir().join(format!("ts-settings-{}", std::process::id()));
        App::new(Store { config_home: dir, data_dirs: vec![] })
    }

    #[test]
    fn the_app_opens_on_the_cosmic_screen() {
        // The settings people came for are COSMIC's; the speaker chain is
        // the specialist screen and should not be what greets them.
        let a = app();
        assert_eq!(a.screen, Screen::Cosmic);
        assert!(!a.levers.is_empty());
    }

    #[test]
    fn the_two_screens_keep_their_own_state() {
        // They edit different things in different places: COSMIC's files and
        // tideshell's live chain. Switching must not disturb either.
        let mut a = app();
        a.details = Some(0);
        a.speaker.apply(speaker::SpeakerAction::Set(tideshell_core::chaingraph::Stage::BassL, "amt", 0.6));
        a.screen = Screen::Speaker;
        assert_eq!(a.details, Some(0), "the lever row stays open underneath");
        let amt = a.speaker.resolve(0.0, 0.0).into_iter().find(|(k, _)| k == "bassL:amt").unwrap().1;
        assert_eq!(amt, 0.6);
        a.screen = Screen::Cosmic;
        assert_eq!(a.speaker.values.iter().find(|(_, c, _)| *c == "amt").unwrap().2, 0.6);
    }
}
