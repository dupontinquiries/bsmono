//! egui — a port of ambient-learner's Live page to egui, bringing over the iced/GTK feel:
//! the temperature reading is the focal point through size, the confidence reads at a glance, and a
//! sparkline shows the recent ambient trend. It adds the smoothness the earlier ports lacked (the
//! reading eases to new values instead of jumping, the confidence bar glides colour) via the shared
//! `uikit_egui::motion` kit, and a "Share with…" row that exposes the current reading to snapclone
//! contacts through the `weather-share` kit.
//!
//! The display maths are pure and unit-tested; only the drawing needs egui.

use uikit_egui::motion;
use weather_share::{WeatherRecord, WeatherShare};

/// The data the Live view renders — a decoupled snapshot of the forecast model's output, so the view
/// doesn't reach into the app's locks.
#[derive(Clone, Debug, Default)]
pub struct LiveSnapshot {
    pub location: String,
    pub temperature: f32,
    pub confidence: f32,
    /// Forecast confidence band.
    pub min_temp: f32,
    pub max_temp: f32,
    /// Recent ambient readings for the sparkline (oldest → newest).
    pub series: Vec<f32>,
    pub condition: String,
    pub updated: u64,
}

impl LiveSnapshot {
    /// Build the shareable [`WeatherRecord`] for the current reading.
    pub fn record(&self) -> WeatherRecord {
        WeatherRecord::new(
            self.location.clone(),
            self.temperature,
            if self.condition.is_empty() { "—" } else { &self.condition },
            self.updated,
        )
    }
}

/// Format a temperature the way the focal reading shows it (one decimal, degree sign). Pure.
pub fn format_temp(c: f32) -> String {
    format!("{c:.1}°")
}

/// Confidence (0..1) → a label + a colour that goes red (low) → amber → green (high). Pure.
pub fn confidence_tint(conf: f32) -> (&'static str, egui::Color32) {
    let c = conf.clamp(0.0, 1.0);
    if c >= 0.75 {
        ("high", egui::Color32::from_rgb(86, 211, 100))
    } else if c >= 0.4 {
        ("fair", egui::Color32::from_rgb(230, 180, 60))
    } else {
        ("low", egui::Color32::from_rgb(214, 110, 90))
    }
}

/// Map a value in `[min,max]` to `[0,1]` for the sparkline; a flat/degenerate range maps to the middle.
pub fn norm(v: f32, min: f32, max: f32) -> f32 {
    if (max - min).abs() < 1e-4 {
        0.5
    } else {
        ((v - min) / (max - min)).clamp(0.0, 1.0)
    }
}

/// The Live view. Holds the eased display value so the reading animates between updates.
#[derive(Default)]
pub struct LiveView {
    /// A contact id that "share" is armed for (set by the picker row); cleared after sharing.
    shared_note: Option<String>,
}

impl LiveView {
    /// Render the Live page. `contacts` are `(sc_id, display_name)`; sharing goes through `share`.
    pub fn ui(
        &mut self,
        ui: &mut egui::Ui,
        snap: &LiveSnapshot,
        contacts: &[(String, String)],
        share: &mut WeatherShare,
    ) {
        // Focal temperature — eased toward the latest reading so it glides rather than snaps.
        let shown = ui.ctx().animate_value_with_time(
            egui::Id::new("ambient_live_temp"),
            snap.temperature,
            0.4,
        );
        ui.vertical_centered(|ui| {
            if !snap.location.is_empty() {
                ui.label(egui::RichText::new(&snap.location).weak());
            }
            ui.label(egui::RichText::new(format_temp(shown)).size(72.0).strong());

            let (clabel, ctarget) = confidence_tint(snap.confidence);
            let ccol = motion::animate_color(ui.ctx(), egui::Id::new("ambient_conf_col"), ctarget, 0.2);
            ui.horizontal(|ui| {
                ui.add_space(ui.available_width() * 0.25);
                ui.colored_label(ccol, format!("confidence: {clabel}"));
            });
            // Confidence bar, eased width.
            let frac = motion::animate_eased(
                ui.ctx(),
                egui::Id::new("ambient_conf_bar"),
                true,
                0.3,
                motion::ease::out_cubic,
            ) * snap.confidence.clamp(0.0, 1.0);
            let (rect, _) = ui.allocate_exact_size(egui::vec2(220.0, 6.0), egui::Sense::hover());
            ui.painter().rect_filled(rect, egui::CornerRadius::same(3), ui.visuals().extreme_bg_color);
            let mut fill = rect;
            fill.set_width(rect.width() * frac);
            ui.painter().rect_filled(fill, egui::CornerRadius::same(3), ccol);

            ui.add_space(4.0);
            ui.label(
                egui::RichText::new(format!("band {:.1}° – {:.1}°", snap.min_temp, snap.max_temp)).weak().small(),
            );
        });

        ui.add_space(10.0);
        self.sparkline(ui, &snap.series);

        ui.add_space(12.0);
        ui.separator();
        // Share-with-contact row — the weather-share integration.
        ui.label(egui::RichText::new("Share this reading").strong());
        if contacts.is_empty() {
            ui.label(egui::RichText::new("no contacts yet").weak().small());
        }
        let rec = snap.record();
        for (id, name) in contacts {
            ui.horizontal(|ui| {
                ui.label(name);
                let already = share.shared_with("live").contains(id);
                if already {
                    if ui.button("Stop sharing").clicked() {
                        share.revoke(id, "live");
                        self.shared_note = Some(format!("stopped sharing with {name}"));
                    }
                    ui.label(egui::RichText::new("· sharing").small().color(egui::Color32::from_rgb(86, 211, 100)));
                } else if ui.button("Share").clicked() {
                    share.share_with(id, "live", &rec);
                    self.shared_note = Some(format!("shared with {name}"));
                }
            });
        }
        if let Some(note) = &self.shared_note {
            ui.label(egui::RichText::new(note).weak().small());
        }
    }

    /// A minimal sparkline of the recent ambient series.
    fn sparkline(&self, ui: &mut egui::Ui, series: &[f32]) {
        let (rect, _) = ui.allocate_exact_size(egui::vec2(ui.available_width().min(280.0), 44.0), egui::Sense::hover());
        ui.painter().rect_filled(rect, egui::CornerRadius::same(6), ui.visuals().extreme_bg_color);
        if series.len() < 2 {
            return;
        }
        let (min, max) = series.iter().fold((f32::MAX, f32::MIN), |(lo, hi), &v| (lo.min(v), hi.max(v)));
        let pts: Vec<egui::Pos2> = series
            .iter()
            .enumerate()
            .map(|(i, &v)| {
                let x = rect.left() + rect.width() * (i as f32 / (series.len() - 1) as f32);
                let y = rect.bottom() - rect.height() * norm(v, min, max);
                egui::pos2(x, y)
            })
            .collect();
        ui.painter().add(egui::Shape::line(pts, egui::Stroke::new(1.5, ui.visuals().selection.bg_fill)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn temp_formats_with_one_decimal_and_degree() {
        assert_eq!(format_temp(21.04), "21.0°");
        assert_eq!(format_temp(-4.55), "-4.6°"); // {:.1} rounds half away from zero
    }

    #[test]
    fn confidence_tiers_and_clamping() {
        assert_eq!(confidence_tint(0.9).0, "high");
        assert_eq!(confidence_tint(0.5).0, "fair");
        assert_eq!(confidence_tint(0.1).0, "low");
        assert_eq!(confidence_tint(2.0).0, "high"); // clamped
    }

    #[test]
    fn norm_maps_range_and_handles_flat() {
        assert_eq!(norm(5.0, 0.0, 10.0), 0.5);
        assert_eq!(norm(0.0, 0.0, 10.0), 0.0);
        assert_eq!(norm(10.0, 0.0, 10.0), 1.0);
        assert_eq!(norm(3.0, 7.0, 7.0), 0.5); // degenerate range → middle
    }

    #[test]
    fn snapshot_builds_a_shareable_record() {
        let s = LiveSnapshot {
            location: "Montréal".into(),
            temperature: 19.5,
            condition: "cloudy".into(),
            updated: 1_700_000_000,
            ..Default::default()
        };
        let r = s.record();
        assert_eq!(r.location, "Montréal");
        assert_eq!(r.temp_c, 19.5);
        assert_eq!(r.condition, "cloudy");
    }
}
