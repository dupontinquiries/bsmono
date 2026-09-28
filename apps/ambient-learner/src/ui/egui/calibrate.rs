//! calibrate — the "🎯 Calibrate" card from the GTK Live page.
//!
//! The GTK build opens a dialog: "Enter the known ambient temperature", a
//! numeric entry defaulting to 20, Cancel / Calibrate. The calibration then
//! runs on a thread (a burst of readings, averaged with the last fifteen
//! seconds of history) and the status label flashes when the count goes up.
//!
//! The egui page keeps the same contract with less ceremony: the entry sits
//! on the page, and the card reports what the host did with the request —
//! the host's log line, not a guess. Every decision here (is the text a
//! temperature? is it in range? may the button be pressed?) is on the pure
//! struct so a test can hold it to the same bounds the engine enforces.

use crate::ui::egui::chrome::card_heading;

/// The engine's accepted range, in °C. Mirrors `engine::config::temperature`
/// rather than importing it, because this module is also compiled where the
/// engine is not mounted (the view-only test build) — and the test below
/// pins the two together.
pub const MIN_C: f32 = -40.0;
pub const MAX_C: f32 = 40.0;

/// The card's state.
#[derive(Debug, Clone, PartialEq)]
pub struct Calibrate {
    /// The entry text. Starts at "20" like the GTK dialog.
    pub text: String,
    /// True from the tap until the host reports the calibration applied or
    /// refused. While busy the button is disabled: a second tap during a
    /// burst would queue a second calibration to the same target, which
    /// the GTK dialog prevented by being modal.
    pub busy: bool,
    /// What the host said last — "calibrated to 21.0°C (3 sensors)", or
    /// why it could not.
    pub status: Option<String>,
    /// Calibrations this session, newest last: (target, when as unix secs).
    pub session: Vec<(f32, i64)>,
}

impl Default for Calibrate {
    fn default() -> Self {
        Calibrate { text: "20".into(), busy: false, status: None, session: Vec::new() }
    }
}

/// What the card asks the host to do.
#[derive(Debug, Clone, PartialEq)]
pub enum CalibrateAction {
    /// Calibrate the models to this ambient temperature, in °C.
    Calibrate(f32),
}

impl Calibrate {
    /// The entry parsed as a temperature, if it is one in range.
    ///
    /// `Err` carries the reason, for the hint under the entry: a number
    /// out of range and a word are refused differently, because one is a
    /// typo and the other is a misunderstanding of what the box is for.
    pub fn target(&self) -> Result<f32, &'static str> {
        let t = self.text.trim().replace(',', ".");
        let v: f32 = t.parse().map_err(|_| "enter a temperature in °C")?;
        if !v.is_finite() { return Err("enter a temperature in °C"); }
        if !(MIN_C..=MAX_C).contains(&v) {
            return Err("outside the accepted range (-40 to 40 °C)");
        }
        Ok(v)
    }

    /// May the button be pressed?
    pub fn can_submit(&self) -> bool { !self.busy && self.target().is_ok() }

    /// The host has answered. Records a success in the session list.
    pub fn settle(&mut self, target: Option<f32>, at: i64, status: String) {
        self.busy = false;
        if let Some(t) = target { self.session.push((t, at)); }
        self.status = Some(status);
    }
}

/// Draw the card. Returns the action when the button is pressed.
pub fn calibrate_ui(ui: &mut egui::Ui, c: &mut Calibrate,
                    colors: &uikit_egui::PaletteColors) -> Option<CalibrateAction> {
    card_heading(ui, "🎯 Calibrate");
    ui.colored_label(colors.text_weak,
        egui::RichText::new("Enter the known ambient temperature").small());
    let mut out = None;
    ui.horizontal(|ui| {
        let edit = egui::TextEdit::singleline(&mut c.text)
            .desired_width(72.0)
            .hint_text("20");
        let resp = ui.add(edit);
        ui.label("°C");
        let can = c.can_submit();
        let label = if c.busy { "Calibrating…" } else { "Calibrate" };
        let submit_key = resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
        if ui.add_enabled(can, egui::Button::new(label)).clicked() || (can && submit_key) {
            if let Ok(t) = c.target() {
                c.busy = true;
                c.status = Some(format!("calibrating to {t:.1}°C…"));
                out = Some(CalibrateAction::Calibrate(t));
            }
        }
    });
    if let Err(why) = c.target() {
        if !c.text.trim().is_empty() {
            ui.colored_label(colors.warning, egui::RichText::new(why).small());
        }
    }
    if let Some(s) = &c.status {
        ui.colored_label(colors.text_weak, egui::RichText::new(s).small());
    }
    if !c.session.is_empty() {
        ui.colored_label(colors.text_weak, egui::RichText::new(
            format!("{} this session", c.session.len())).small());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_entry_is_the_gtk_dialogs_twenty_degrees() {
        let c = Calibrate::default();
        assert_eq!(c.target(), Ok(20.0));
        assert!(c.can_submit());
    }

    #[test]
    fn a_word_and_an_out_of_range_number_are_refused_for_different_reasons() {
        let mut c = Calibrate::default();
        c.text = "warm".into();
        assert_eq!(c.target(), Err("enter a temperature in °C"));
        c.text = "45".into();
        assert!(c.target().unwrap_err().contains("range"));
        c.text = "-40".into();
        assert_eq!(c.target(), Ok(-40.0), "the bound itself is allowed");
    }

    #[test]
    fn a_decimal_comma_is_a_decimal_point() {
        // A German-locale keyboard produces "21,5"; refusing it would read
        // as the app being broken on that phone.
        let mut c = Calibrate::default();
        c.text = "21,5".into();
        assert_eq!(c.target(), Ok(21.5));
    }

    #[test]
    fn busy_blocks_a_second_submission_until_the_host_answers() {
        let mut c = Calibrate::default();
        c.busy = true;
        assert!(!c.can_submit());
        c.settle(Some(20.0), 1_700_000_000, "calibrated".into());
        assert!(c.can_submit());
        assert_eq!(c.session.len(), 1);
        assert_eq!(c.status.as_deref(), Some("calibrated"));
    }

    #[test]
    fn a_refusal_settles_without_recording_a_calibration() {
        let mut c = Calibrate::default();
        c.busy = true;
        c.settle(None, 0, "no sensor readings yet".into());
        assert!(!c.busy);
        assert!(c.session.is_empty(), "a refused calibration is not a calibration");
    }

    #[test]
    fn the_bounds_match_the_engines() {
        // The engine refuses targets outside config::temperature; a card
        // that accepted a wider range would show "calibrating…" for a
        // request the engine then drops on the floor.
        let src = include_str!("../../engine.rs");
        assert!(src.contains("pub const MIN: f32 = -40.0;"), "engine MIN moved");
        assert!(src.contains("pub const MAX: f32 = 40.0;"), "engine MAX moved");
    }
}
