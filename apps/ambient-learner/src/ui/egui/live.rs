//! live — `LivePage`, `BreathingDot`, and the theme ramps they read from.
//!
//! The app's main screen: what the sensors say right now, how much of it is
//! trustworthy, and whether the sensors are actually there.
//!
//! ## Sensor health is a first-class row, not an error state
//!
//! `Theme.healthLabel` has four values and only one of them is a warning:
//! locked, degraded, unavailable, initializing. That is the right shape for an
//! app whose entire subject is sensors that may or may not exist on a given
//! phone — "unavailable" is a normal Tuesday for a device with no barometer,
//! not a fault to be reported in red and dismissed.
//!
//! The glyphs carry the same idea. Three of the four are dots and only the
//! genuinely bad one is a warning triangle, so the row reads as status rather
//! than as a recurring alarm.
//!
//! ## Missing and zero are different, everywhere
//!
//! Every reading here is optional, because every sensor is. A humidity of 0%
//! and a humidity we could not read are completely different facts, and a page
//! about sensors that conflated them would be lying about its own subject.
//! `Reading::display` shows an em dash for absence and never a zero.

/// Sensor health, as the app models it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Health {
    /// Everything we expect is reporting.
    Ok,
    /// Some sensors are answering, some are not.
    Degraded,
    /// Nothing is answering.
    Unavailable,
    /// We have not finished asking yet.
    #[default]
    Initializing,
}

impl Health {
    /// `Theme.healthLabel`.
    ///
    /// Only the genuinely bad state gets a warning glyph. The others are dots,
    /// because on a phone with no barometer "unavailable" is a normal Tuesday
    /// and a red alarm every launch teaches the user to ignore the row.
    pub fn label(self) -> &'static str {
        match self {
            Health::Ok => "● Sensors locked",
            Health::Degraded => "● Sensors degraded",
            Health::Unavailable => "⚠ Sensors unavailable",
            Health::Initializing => "○ Sensors initializing",
        }
    }

    /// Which palette role the row takes.
    pub fn role(self) -> HealthRole {
        match self {
            Health::Ok => HealthRole::Success,
            Health::Degraded => HealthRole::Warning,
            Health::Unavailable => HealthRole::Danger,
            // Not dim-as-in-broken: dim as in "no news yet".
            Health::Initializing => HealthRole::Dim,
        }
    }

    /// Derive health from what actually answered.
    ///
    /// Taken from the readings rather than stored, so the row cannot claim
    /// "locked" while every card shows a dash — the failure this whole page is
    /// supposed to make visible.
    pub fn from_counts(available: usize, expected: usize) -> Health {
        if expected == 0 { return Health::Initializing; }
        match available {
            0 => Health::Unavailable,
            n if n == expected => Health::Ok,
            _ => Health::Degraded,
        }
    }
}

/// Palette role for the health row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HealthRole { Success, Warning, Danger, Dim }

/// `Theme.tempColor` — a five-band ramp, cold blue to hot red.
///
/// Bands rather than a gradient because the point is to be readable at a
/// glance from across a room, and a continuous ramp makes 21° and 23° the same
/// colour to the eye. The boundaries are the ones the QML uses.
pub fn temp_color(t: f32) -> [u8; 3] {
    if t < 10.0 { [0x64, 0xb5, 0xf6] }
    else if t < 18.0 { [0x81, 0xc7, 0x84] }
    else if t < 24.0 { [0xff, 0xf1, 0x76] }
    else if t < 30.0 { [0xff, 0xb7, 0x4d] }
    else { [0xef, 0x53, 0x50] }
}

/// One optional sensor reading.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Reading {
    pub value: Option<f32>,
    /// Decimal places to show.
    pub places: usize,
}

impl Reading {
    pub fn new(value: Option<f32>, places: usize) -> Self { Reading { value, places } }
    pub fn missing() -> Self { Reading { value: None, places: 0 } }
    pub fn available(&self) -> bool { self.value.is_some() }

    /// The number, or an em dash.
    ///
    /// Never a zero for a missing reading: 0% humidity and humidity we could
    /// not read are different facts, and an app about sensors that conflated
    /// them would be lying about its own subject.
    pub fn display(&self, unit: &str) -> String {
        match self.value {
            Some(v) => format!("{v:.*}{unit}", self.places),
            None => "—".into(),
        }
    }
}

/// Everything the live page shows.
#[derive(Debug, Clone, PartialEq)]
pub struct Live {
    pub temp_c: Reading,
    pub humidity_pct: Reading,
    pub pressure_hpa: Reading,
    pub light: Reading,
    /// Model confidence, 0..=1.
    pub confidence: Option<f32>,
    /// How many of the four readings this device can produce at all.
    ///
    /// `None` means "assume all four", the QML's behaviour. The host sets it
    /// from what has ever reported, so a phone with no barometer reads
    /// "locked" with its one thermal sensor answering rather than
    /// "degraded" forever — the difference between a sensor that stopped
    /// and a sensor that was never there, which `Reading` alone cannot
    /// carry.
    pub expected: Option<usize>,
}

impl Default for Live {
    fn default() -> Self {
        Live {
            temp_c: Reading::missing(),
            humidity_pct: Reading::missing(),
            pressure_hpa: Reading::missing(),
            light: Reading::missing(),
            confidence: None,
            expected: None,
        }
    }
}

impl Live {
    /// The four sensor readings, for counting and for drawing.
    pub fn readings(&self) -> [(&'static str, &'static str, Reading); 4] {
        [("Temperature", "°C", self.temp_c),
         ("Humidity", "%", self.humidity_pct),
         ("Pressure", " hPa", self.pressure_hpa),
         ("Light", "", self.light)]
    }

    /// Health, derived from what answered rather than reported separately.
    pub fn health(&self) -> Health {
        let all = self.readings();
        let available = all.iter().filter(|(_, _, r)| r.available()).count();
        // Never fewer expected than answered: a host that undercounts must
        // not turn a full set of readings into a "degraded" row.
        let expected = self.expected.unwrap_or(all.len()).clamp(available, all.len());
        Health::from_counts(available, expected)
    }

    /// The page's accent, from temperature.
    ///
    /// A missing temperature has no accent rather than a default one — the
    /// page should look uncommitted when it knows nothing, not confidently
    /// cold.
    pub fn accent(&self) -> Option<[u8; 3]> { self.temp_c.value.map(temp_color) }

    /// Confidence as a percentage string, or absent.
    pub fn confidence_text(&self) -> Option<String> {
        self.confidence.map(|c| format!("{:.0}% confident", (c.clamp(0.0, 1.0)) * 100.0))
    }
}

/// `BreathingDot` — a slow pulse that says the app is still reading.
///
/// Uses the shared `Breath` rather than another sine: this is the sixth
/// oscillator in the fleet and the first one that did not have to be written.
#[derive(Debug, Clone)]
pub struct BreathingDot {
    breath: uikit_egui::motion::Breath,
    /// Stops when nothing is reporting. A dot that keeps breathing while the
    /// sensors are dead says the app is fine when it is not.
    pub live: bool,
}

/// One breath, matching the QML's slow cycle.
pub const DOT_PERIOD_MS: f32 = 2_600.0;

impl Default for BreathingDot {
    fn default() -> Self {
        BreathingDot {
            breath: uikit_egui::motion::Breath::new(DOT_PERIOD_MS, 0.35, 1.0),
            live: false,
        }
    }
}

impl BreathingDot {
    pub fn tick(&mut self, dt_ms: f32) { if self.live { self.breath.tick(dt_ms); } }

    /// Opacity, 0 when nothing is reporting.
    pub fn alpha(&self) -> f32 { if self.live { self.breath.value() } else { 0.0 } }
}

/// Draw the live page.
pub fn live_ui(ui: &mut egui::Ui, live: &Live, dot: &BreathingDot,
               c: &uikit_egui::PaletteColors) {
    let health = live.health();
    ui.horizontal(|ui| {
        let col = match health.role() {
            HealthRole::Success => c.success,
            HealthRole::Warning => c.warning,
            HealthRole::Danger => c.danger,
            HealthRole::Dim => c.text_weak,
        };
        // The breathing dot sits with the health row, because they say the same
        // thing at different speeds.
        let (rect, _) = ui.allocate_exact_size(egui::vec2(10.0, 10.0), egui::Sense::hover());
        let a = (dot.alpha() * 255.0) as u8;
        ui.painter().circle_filled(rect.center(), 4.0,
            egui::Color32::from_rgba_unmultiplied(col.r(), col.g(), col.b(), a));
        ui.colored_label(col, health.label());
    });

    if let Some(t) = live.confidence_text() {
        ui.colored_label(c.text_weak, t);
    }
    ui.separator();

    for (name, unit, r) in live.readings() {
        ui.horizontal(|ui| {
            ui.colored_label(c.text_weak, name);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                // Only temperature is coloured by its own ramp; the others take
                // the ordinary ink, or the page becomes a rainbow and the ramp
                // stops meaning anything.
                let col = if name == "Temperature" {
                    match r.value.map(temp_color) {
                        Some([rr, gg, bb]) => egui::Color32::from_rgb(rr, gg, bb),
                        None => c.text_weak,
                    }
                } else if r.available() { c.text } else { c.text_weak };
                ui.colored_label(col, r.display(unit));
            });
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn full() -> Live {
        Live {
            temp_c: Reading::new(Some(21.0), 1),
            humidity_pct: Reading::new(Some(48.0), 0),
            pressure_hpa: Reading::new(Some(1013.0), 0),
            light: Reading::new(Some(120.0), 0),
            confidence: Some(0.82),
            expected: None,
        }
    }

    #[test]
    fn a_missing_reading_shows_a_dash_and_never_a_zero() {
        // 0% humidity and humidity we could not read are different facts, and
        // an app about sensors that conflated them would be lying about its own
        // subject.
        assert_eq!(Reading::missing().display("%"), "—");
        assert_eq!(Reading::new(Some(0.0), 0).display("%"), "0%");
    }

    #[test]
    fn readings_keep_the_precision_they_were_given() {
        assert_eq!(Reading::new(Some(21.34), 1).display("°C"), "21.3°C");
        assert_eq!(Reading::new(Some(1013.2), 0).display(" hPa"), "1013 hPa");
    }

    #[test]
    fn health_is_derived_from_what_answered_not_reported_separately() {
        // Otherwise the row can claim "locked" while every card shows a dash —
        // the exact failure this page exists to make visible.
        assert_eq!(full().health(), Health::Ok);
        assert_eq!(Live::default().health(), Health::Unavailable);
        let mut partial = full();
        partial.light = Reading::missing();
        assert_eq!(partial.health(), Health::Degraded);
    }

    #[test]
    fn initializing_is_distinct_from_unavailable() {
        // "We have not asked yet" and "nothing is there" are different, and
        // showing a warning during startup trains the user to ignore it.
        assert_eq!(Health::from_counts(0, 0), Health::Initializing);
        assert_eq!(Health::from_counts(0, 4), Health::Unavailable);
    }

    #[test]
    fn only_the_genuinely_bad_state_gets_a_warning_glyph() {
        // On a phone with no barometer, "unavailable" is a normal Tuesday. A
        // red alarm every launch teaches the user to ignore the row.
        assert!(Health::Ok.label().starts_with('●'));
        assert!(Health::Degraded.label().starts_with('●'));
        assert!(Health::Initializing.label().starts_with('○'));
        assert!(Health::Unavailable.label().starts_with('⚠'));
    }

    #[test]
    fn initializing_is_dim_rather_than_alarming() {
        assert_eq!(Health::Initializing.role(), HealthRole::Dim);
        assert_eq!(Health::Unavailable.role(), HealthRole::Danger);
    }

    #[test]
    fn the_temperature_ramp_bands_where_the_qml_bands() {
        assert_eq!(temp_color(-5.0), [0x64, 0xb5, 0xf6]);
        assert_eq!(temp_color(9.9), [0x64, 0xb5, 0xf6]);
        assert_eq!(temp_color(10.0), [0x81, 0xc7, 0x84]);
        assert_eq!(temp_color(23.9), [0xff, 0xf1, 0x76]);
        assert_eq!(temp_color(24.0), [0xff, 0xb7, 0x4d]);
        assert_eq!(temp_color(35.0), [0xef, 0x53, 0x50]);
    }

    #[test]
    fn the_ramp_is_banded_rather_than_continuous() {
        // Readable at a glance from across a room. A continuous gradient makes
        // 21° and 23° the same colour to the eye.
        assert_eq!(temp_color(19.0), temp_color(23.0));
        assert_ne!(temp_color(23.9), temp_color(24.1));
    }

    #[test]
    fn a_missing_temperature_leaves_the_page_without_an_accent() {
        // Uncommitted when it knows nothing, rather than confidently cold.
        assert!(Live::default().accent().is_none());
        assert_eq!(full().accent(), Some(temp_color(21.0)));
    }

    #[test]
    fn confidence_is_absent_rather_than_zero_when_unknown() {
        assert_eq!(Live::default().confidence_text(), None);
        assert_eq!(full().confidence_text().as_deref(), Some("82% confident"));
    }

    #[test]
    fn confidence_is_clamped_so_a_model_cannot_report_more_than_certainty() {
        let mut l = full();
        l.confidence = Some(1.5);
        assert_eq!(l.confidence_text().as_deref(), Some("100% confident"));
        l.confidence = Some(-0.2);
        assert_eq!(l.confidence_text().as_deref(), Some("0% confident"));
    }

    #[test]
    fn the_dot_stops_breathing_when_nothing_is_reporting() {
        // A dot that kept breathing while the sensors are dead says the app is
        // fine when it is not.
        let mut d = BreathingDot::default();
        d.live = false;
        d.tick(DOT_PERIOD_MS / 2.0);
        assert_eq!(d.alpha(), 0.0);

        d.live = true;
        d.tick(DOT_PERIOD_MS / 4.0);
        assert!(d.alpha() > 0.0);
    }

    #[test]
    fn the_dot_reuses_the_shared_oscillator() {
        // Sixth breather in the fleet, and the first that did not have to be
        // written. Its band is the kit's, not another private sine.
        let mut d = BreathingDot::default();
        d.live = true;
        for _ in 0..200 {
            d.tick(DOT_PERIOD_MS / 50.0);
            assert!((0.0..=1.0).contains(&d.alpha()));
        }
    }

    #[test]
    fn a_device_without_optional_sensors_is_locked_not_degraded() {
        // One thermal sensor answering on a phone that has no barometer,
        // hygrometer or light sensor is the whole set answering.
        let mut l = Live { temp_c: Reading::new(Some(21.0), 1), ..Default::default() };
        assert_eq!(l.health(), Health::Degraded, "without the count, absence reads as failure");
        l.expected = Some(1);
        assert_eq!(l.health(), Health::Ok);
        // A humidity sensor that once reported and stopped IS degradation.
        l.expected = Some(2);
        assert_eq!(l.health(), Health::Degraded);
        // And an undercount cannot manufacture one.
        l.humidity_pct = Reading::new(Some(40.0), 0);
        l.expected = Some(1);
        assert_eq!(l.health(), Health::Ok);
    }
}
