//! forecast_card — the "🔮 Forecast" box from the GTK Live page.
//!
//! GTK shows two lines, "1 hour: 21.3°C (20.8-21.9°C)" and "24 hours: …",
//! refreshed once a minute from `forecast::ForecastModel`. Before the model
//! has any observations the lines read "--°C", and this card keeps that:
//! a forecast the model has not made is shown as absent, not as the current
//! temperature dressed up as a prediction.

use crate::ui::egui::chrome::card_heading;

/// One horizon's forecast: centre, low, high — in °C.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Band {
    pub temp: f32,
    pub low: f32,
    pub high: f32,
}

impl Band {
    /// "21.3°C (20.8–21.9°C)", the GTK line without its prefix.
    pub fn text(&self) -> String {
        format!("{:.1}°C  ({:.1}–{:.1}°C)", self.temp, self.low, self.high)
    }
}

/// The card's state.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ForecastCard {
    pub one_hour: Option<Band>,
    pub one_day: Option<Band>,
    /// How many observations the model is working from — the number that
    /// explains why the bands are wide, or absent.
    pub observations: usize,
}

impl ForecastCard {
    /// The two lines, prefix and value, "--°C" when there is nothing.
    pub fn lines(&self) -> [(&'static str, String); 2] {
        let f = |b: Option<Band>| b.map(|b| b.text()).unwrap_or_else(|| "--°C".into());
        [("1 hour", f(self.one_hour)), ("24 hours", f(self.one_day))]
    }

    /// What to say beneath the lines.
    pub fn note(&self) -> Option<String> {
        match self.observations {
            0 => Some("Waiting for the first reading.".into()),
            1 => Some("One observation so far — the forecast needs a trend.".into()),
            n if self.one_day.is_none() => Some(format!("{n} observations; not enough for a day ahead yet.")),
            _ => None,
        }
    }
}

/// Draw the card.
pub fn forecast_ui(ui: &mut egui::Ui, f: &ForecastCard, colors: &uikit_egui::PaletteColors) {
    card_heading(ui, "🔮 Forecast");
    egui::Grid::new("forecast_lines").num_columns(2).spacing([16.0, 2.0]).show(ui, |ui| {
        for (label, value) in f.lines() {
            ui.colored_label(colors.text_weak, label);
            ui.colored_label(colors.text, value);
            ui.end_row();
        }
    });
    if let Some(n) = f.note() {
        ui.colored_label(colors.text_weak, egui::RichText::new(n).small());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_absent_forecast_reads_as_dashes_not_as_a_number() {
        let f = ForecastCard::default();
        assert_eq!(f.lines()[0].1, "--°C");
        assert_eq!(f.lines()[1].1, "--°C");
        assert!(f.note().unwrap().contains("Waiting"));
    }

    #[test]
    fn a_band_prints_centre_and_range_to_one_decimal() {
        let b = Band { temp: 21.26, low: 20.8, high: 21.9 };
        assert_eq!(b.text(), "21.3°C  (20.8–21.9°C)");
    }

    #[test]
    fn the_note_explains_a_missing_day_forecast_by_count() {
        let f = ForecastCard { one_hour: Some(Band { temp: 21.0, low: 20.0, high: 22.0 }),
                               one_day: None, observations: 7 };
        assert!(f.note().unwrap().contains("7 observations"));
        let full = ForecastCard { one_day: f.one_hour, ..f.clone() };
        assert_eq!(full.note(), None, "nothing to explain once both are present");
    }
}
//! forecast_card — the "🔮 Forecast" box from the GTK Live page.
//!
//! GTK shows two lines, "1 hour: 21.3°C (20.8-21.9°C)" and "24 hours: …",
//! refreshed once a minute from `forecast::ForecastModel`. Before the model
//! has any observations the lines read "--°C", and this card keeps that:
//! a forecast the model has not made is shown as absent, not as the current
//! temperature dressed up as a prediction.

use crate::ui::egui::chrome::card_heading;

/// One horizon's forecast: centre, low, high — in °C.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Band {
    pub temp: f32,
    pub low: f32,
    pub high: f32,
}

impl Band {
    /// "21.3°C (20.8–21.9°C)", the GTK line without its prefix.
    pub fn text(&self) -> String {
        format!("{:.1}°C  ({:.1}–{:.1}°C)", self.temp, self.low, self.high)
    }
}

/// The card's state.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ForecastCard {
    pub one_hour: Option<Band>,
    pub one_day: Option<Band>,
    /// How many observations the model is working from — the number that
    /// explains why the bands are wide, or absent.
    pub observations: usize,
}

impl ForecastCard {
    /// The two lines, prefix and value, "--°C" when there is nothing.
    pub fn lines(&self) -> [(&'static str, String); 2] {
        let f = |b: Option<Band>| b.map(|b| b.text()).unwrap_or_else(|| "--°C".into());
        [("1 hour", f(self.one_hour)), ("24 hours", f(self.one_day))]
    }

    /// What to say beneath the lines.
    pub fn note(&self) -> Option<String> {
        match self.observations {
            0 => Some("Waiting for the first reading.".into()),
            1 => Some("One observation so far — the forecast needs a trend.".into()),
            n if self.one_day.is_none() => Some(format!("{n} observations; not enough for a day ahead yet.")),
            _ => None,
        }
    }
}

/// Draw the card.
pub fn forecast_ui(ui: &mut egui::Ui, f: &ForecastCard, colors: &uikit_egui::PaletteColors) {
    card_heading(ui, "🔮 Forecast");
    egui::Grid::new("forecast_lines").num_columns(2).spacing([16.0, 2.0]).show(ui, |ui| {
        for (label, value) in f.lines() {
            ui.colored_label(colors.text_weak, label);
            ui.colored_label(colors.text, value);
            ui.end_row();
        }
    });
    if let Some(n) = f.note() {
        ui.colored_label(colors.text_weak, egui::RichText::new(n).small());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_absent_forecast_reads_as_dashes_not_as_a_number() {
        let f = ForecastCard::default();
        assert_eq!(f.lines()[0].1, "--°C");
        assert_eq!(f.lines()[1].1, "--°C");
        assert!(f.note().unwrap().contains("Waiting"));
    }

    #[test]
    fn a_band_prints_centre_and_range_to_one_decimal() {
        let b = Band { temp: 21.26, low: 20.8, high: 21.9 };
        assert_eq!(b.text(), "21.3°C  (20.8–21.9°C)");
    }

    #[test]
    fn the_note_explains_a_missing_day_forecast_by_count() {
        let f = ForecastCard { one_hour: Some(Band { temp: 21.0, low: 20.0, high: 22.0 }),
                               one_day: None, observations: 7 };
        assert!(f.note().unwrap().contains("7 observations"));
        let full = ForecastCard { one_day: f.one_hour, ..f.clone() };
        assert_eq!(full.note(), None, "nothing to explain once both are present");
    }
}
