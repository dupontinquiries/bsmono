//! model — `ModelPage`, ported.
//!
//! What the model is, what it has to learn from, what the device is doing, and
//! who calibrated it.
//!
//! ## The neural count carries its own threshold
//!
//! `neural_calibrations` renders with `"  (need 5 for neural)"` appended while
//! it is under five. That suffix is the most useful thing on the page: without
//! it, "2" is a number with no meaning — the user cannot tell whether they are
//! nearly there or nowhere near, and a neural model quietly running on two
//! samples looks identical to one running on fifty. Telling them the target
//! next to the count is what turns a statistic into an instruction.
//!
//! It disappears once the threshold is met, rather than becoming a tick. A
//! permanent "5/5 ✓" is clutter on a screen the user only opens when something
//! is wrong.
//!
//! ## Percentages have different precisions on purpose
//!
//! Confidence shows one decimal; CPU load and battery show none. That is not
//! inconsistency — confidence is the number you watch move as the model
//! improves, and a whole percent hides a session's worth of progress. Battery
//! at 61.4% is false precision on a reading that is an estimate anyway.
//!
//! ## The calibration log names the UI that did it
//!
//! Entries carry a source, and the Qt page treats `qt-ui`, `gtk-ui` and
//! `iced-ui` specially. That mattered when three frontends wrote to one
//! database — an odd calibration could be traced to the UI that produced it.
//! It matters more now with a fourth: this port's own entries should be
//! distinguishable from the ones it inherited, or a bug introduced here would
//! be indistinguishable from one that arrived with the data.

/// Neural calibrations needed before the neural model is worth using.
pub const NEURAL_MIN: u32 = 5;

/// Which frontend recorded a calibration.
///
/// A closed set for the ones that exist plus a catch-all, rather than a raw
/// string, so a typo in one frontend cannot silently create a fifth "UI".
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    Qt,
    Gtk,
    Iced,
    Egui,
    /// Something else, or nothing. The Qt page shows "(unknown)".
    Unknown(String),
}

impl Source {
    pub fn parse(s: &str) -> Source {
        match s.trim() {
            "qt-ui" => Source::Qt,
            "gtk-ui" => Source::Gtk,
            "iced-ui" => Source::Iced,
            "egui-ui" => Source::Egui,
            other => Source::Unknown(other.to_string()),
        }
    }

    /// What the row shows.
    ///
    /// An empty source reads "(unknown)" rather than an empty cell: a blank
    /// there is indistinguishable from a rendering fault, and this log exists
    /// precisely to attribute entries.
    pub fn label(&self) -> String {
        match self {
            Source::Qt => "qt-ui".into(),
            Source::Gtk => "gtk-ui".into(),
            Source::Iced => "iced-ui".into(),
            Source::Egui => "egui-ui".into(),
            Source::Unknown(s) if s.is_empty() => "(unknown)".into(),
            Source::Unknown(s) => s.clone(),
        }
    }

    /// Is this one of the known frontends?
    pub fn is_known_ui(&self) -> bool { !matches!(self, Source::Unknown(_)) }
}

/// One calibration entry.
#[derive(Debug, Clone, PartialEq)]
pub struct CalEntry {
    pub source: Source,
    /// The temperature the user said it actually was.
    pub target_c: f32,
    pub at: i64,
}

impl CalEntry {
    pub fn target_text(&self) -> String { format!("{:.1}°C", self.target_c) }
}

/// Model statistics.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ModelStats {
    pub model_type: String,
    pub linear: u32,
    pub neural: u32,
    /// 0..=1.
    pub confidence: f32,
}

impl ModelStats {
    /// The neural count, with its threshold hint while short.
    ///
    /// The hint disappears once met rather than becoming a tick: a permanent
    /// "5/5 ✓" is clutter on a screen people only open when something is wrong.
    pub fn neural_text(&self) -> String {
        if self.neural < NEURAL_MIN {
            format!("{}  (need {NEURAL_MIN} for neural)", self.neural)
        } else {
            self.neural.to_string()
        }
    }

    /// True while the neural model does not have enough to work with.
    pub fn neural_short(&self) -> bool { self.neural < NEURAL_MIN }

    /// One decimal: this is the number you watch move as the model improves,
    /// and a whole percent hides a session's worth of progress.
    pub fn confidence_text(&self) -> String {
        format!("{:.1}%", (self.confidence.clamp(0.0, 1.0) * 100.0))
    }

    pub fn model_text(&self) -> String {
        if self.model_type.trim().is_empty() { "—".into() } else { self.model_type.clone() }
    }
}

/// Device state.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct DeviceState {
    /// 0..=1.
    pub cpu_load: f32,
    /// 0..=1.
    pub battery: f32,
    pub charging: bool,
}

impl DeviceState {
    /// No decimals: whole percents, because these are estimates and 61.4%
    /// claims a precision the reading does not have.
    pub fn cpu_text(&self) -> String {
        format!("{:.0}%", self.cpu_load.clamp(0.0, 1.0) * 100.0)
    }

    /// Battery, with the charging marker appended.
    ///
    /// Appended rather than replacing the number: "charging" alone loses the
    /// level, and the level is why the page is open.
    pub fn battery_text(&self) -> String {
        let pct = format!("{:.0}%", self.battery.clamp(0.0, 1.0) * 100.0);
        if self.charging { format!("{pct}  ⚡ charging") } else { pct }
    }
}

/// The page.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ModelPage {
    pub stats: ModelStats,
    pub device: DeviceState,
    log: Vec<CalEntry>,
}

impl ModelPage {
    pub fn set_log(&mut self, log: Vec<CalEntry>) { self.log = log; }
    pub fn log(&self) -> &[CalEntry] { &self.log }

    /// The header's count, phrased as the Qt page does.
    pub fn log_count_text(&self) -> String { format!("{} entries", self.log.len()) }

    /// Entries from a given frontend, for tracing an odd calibration back.
    pub fn from_source(&self, s: &Source) -> Vec<&CalEntry> {
        self.log.iter().filter(|e| &e.source == s).collect()
    }
}

/// Draw the page.
pub fn model_ui(ui: &mut egui::Ui, p: &ModelPage, c: &uikit_egui::PaletteColors) {
    ui.label(egui::RichText::new("📊 Model statistics").strong());
    egui::Grid::new("model_stats").num_columns(2).spacing([16.0, 4.0]).show(ui, |ui| {
        ui.colored_label(c.text_weak, "Active model:");
        ui.colored_label(c.text, p.stats.model_text());
        ui.end_row();
        ui.colored_label(c.text_weak, "Linear cal.:");
        ui.colored_label(c.text, p.stats.linear.to_string());
        ui.end_row();
        ui.colored_label(c.text_weak, "Neural cal.:");
        // The hint is dimmer than the count: it is guidance beside a number,
        // not part of it.
        let col = if p.stats.neural_short() { c.text_weak } else { c.text };
        ui.colored_label(col, p.stats.neural_text());
        ui.end_row();
        ui.colored_label(c.text_weak, "Confidence:");
        ui.colored_label(c.text, p.stats.confidence_text());
        ui.end_row();
    });

    ui.add_space(10.0);
    ui.label(egui::RichText::new("⚙️  Device state").strong());
    egui::Grid::new("model_device").num_columns(2).spacing([16.0, 4.0]).show(ui, |ui| {
        ui.colored_label(c.text_weak, "CPU load:");
        ui.colored_label(c.text, p.device.cpu_text());
        ui.end_row();
        ui.colored_label(c.text_weak, "Battery:");
        ui.colored_label(c.text, p.device.battery_text());
        ui.end_row();
    });

    ui.add_space(10.0);
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new("🧾  Recent calibrations").strong());
        ui.colored_label(c.text_weak, p.log_count_text());
    });
    if p.log().is_empty() {
        ui.colored_label(c.text_weak, "No calibrations recorded yet.");
        return;
    }
    egui::ScrollArea::vertical().id_salt("cal_log").max_height(200.0).show(ui, |ui| {
        for e in p.log() {
            ui.horizontal(|ui| {
                ui.colored_label(c.text_weak, e.source.label());
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.colored_label(c.text, e.target_text());
                });
            });
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stats(neural: u32, conf: f32) -> ModelStats {
        ModelStats { model_type: "neural".into(), linear: 46, neural, confidence: conf }
    }

    #[test]
    fn the_neural_count_carries_its_threshold_while_short() {
        // Without it "2" is a number with no meaning: the user cannot tell
        // whether they are nearly there or nowhere near, and a neural model
        // running on two samples looks identical to one running on fifty.
        assert_eq!(stats(2, 0.5).neural_text(), "2  (need 5 for neural)");
        assert!(stats(2, 0.5).neural_short());
    }

    #[test]
    fn the_hint_disappears_once_met_rather_than_becoming_a_tick() {
        // A permanent "5/5 ✓" is clutter on a screen people only open when
        // something is wrong.
        assert_eq!(stats(5, 0.5).neural_text(), "5");
        assert_eq!(stats(50, 0.5).neural_text(), "50");
        assert!(!stats(5, 0.5).neural_short());
    }

    #[test]
    fn confidence_keeps_a_decimal_and_device_percentages_do_not() {
        // Not inconsistency: confidence is the number you watch move as the
        // model improves, and a whole percent hides a session's progress.
        // Battery at 61.4% is false precision on an estimate.
        assert_eq!(stats(5, 0.8642).confidence_text(), "86.4%");
        let d = DeviceState { cpu_load: 0.614, battery: 0.614, charging: false };
        assert_eq!(d.cpu_text(), "61%");
        assert_eq!(d.battery_text(), "61%");
    }

    #[test]
    fn a_confidence_outside_the_range_is_clamped_rather_than_shown() {
        // A backend reporting 1.2 is a bug, but "120% confident" on screen is a
        // worse one.
        assert_eq!(stats(5, 1.4).confidence_text(), "100.0%");
        assert_eq!(stats(5, -0.2).confidence_text(), "0.0%");
    }

    #[test]
    fn charging_is_appended_to_the_level_not_substituted_for_it() {
        // "Charging" alone loses the level, and the level is why the page is
        // open.
        let d = DeviceState { cpu_load: 0.1, battery: 0.42, charging: true };
        assert!(d.battery_text().starts_with("42%"));
        assert!(d.battery_text().contains("charging"));
    }

    #[test]
    fn a_missing_model_type_shows_a_dash() {
        let mut s = stats(5, 0.5);
        s.model_type = "  ".into();
        assert_eq!(s.model_text(), "—");
    }

    #[test]
    fn the_log_names_the_frontend_that_recorded_each_entry() {
        // Three frontends wrote to one database and now there are four. An odd
        // calibration has to be traceable to the UI that produced it, or a bug
        // introduced by this port is indistinguishable from one that arrived
        // with the data.
        assert_eq!(Source::parse("qt-ui"), Source::Qt);
        assert_eq!(Source::parse("gtk-ui"), Source::Gtk);
        assert_eq!(Source::parse("iced-ui"), Source::Iced);
        assert_eq!(Source::parse("egui-ui"), Source::Egui);
        assert!(Source::parse("qt-ui").is_known_ui());
    }

    #[test]
    fn an_unrecognised_source_is_kept_rather_than_discarded() {
        // A future frontend, or a script, should show its name rather than be
        // flattened into "(unknown)" — that would hide the very thing the log
        // is for.
        let s = Source::parse("cli-tool");
        assert_eq!(s.label(), "cli-tool");
        assert!(!s.is_known_ui());
    }

    #[test]
    fn an_empty_source_reads_unknown_rather_than_blank() {
        // A blank cell is indistinguishable from a rendering fault, on a log
        // whose whole purpose is attribution.
        assert_eq!(Source::parse("").label(), "(unknown)");
        assert_eq!(Source::parse("   ").label(), "(unknown)");
    }

    #[test]
    fn entries_can_be_traced_back_to_one_frontend() {
        let mut p = ModelPage::default();
        p.set_log(vec![
            CalEntry { source: Source::Qt, target_c: 21.0, at: 0 },
            CalEntry { source: Source::Egui, target_c: 22.5, at: 1 },
            CalEntry { source: Source::Qt, target_c: 20.0, at: 2 }]);
        assert_eq!(p.from_source(&Source::Qt).len(), 2);
        assert_eq!(p.from_source(&Source::Egui).len(), 1);
        assert_eq!(p.from_source(&Source::Gtk).len(), 0);
    }

    #[test]
    fn the_log_header_counts_entries() {
        let mut p = ModelPage::default();
        assert_eq!(p.log_count_text(), "0 entries");
        p.set_log(vec![CalEntry { source: Source::Egui, target_c: 21.0, at: 0 }]);
        assert_eq!(p.log_count_text(), "1 entries");
    }

    #[test]
    fn a_calibration_target_shows_one_decimal() {
        // Matching the sensor's honest resolution, as the history page does.
        let e = CalEntry { source: Source::Egui, target_c: 21.44, at: 0 };
        assert_eq!(e.target_text(), "21.4°C");
    }
}
