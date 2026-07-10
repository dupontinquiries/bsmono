//! log_view — a logscope-styled log list for logs/analysis panes: per-level coloured badges
//! (DEBUG/INFO/WARN/ERROR), roomy padding, and a fade-in for freshly-arrived entries via the shared
//! `motion` kit. Errors also tint their message so they read at a glance.
//!
//! The level → badge/colour mapping is pure and unit-tested; only rendering needs egui.

use crate::motion;
use egui::{Color32, Ui};

/// Severity of a log line, in logscope's palette.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum LogLevel {
    Debug,
    Info,
    Warn,
    Error,
}

impl LogLevel {
    /// The badge text + colour.
    pub fn badge(self) -> (&'static str, Color32) {
        match self {
            LogLevel::Debug => ("DEBUG", Color32::from_rgb(120, 130, 150)),
            LogLevel::Info => ("INFO", Color32::from_rgb(90, 175, 120)),
            LogLevel::Warn => ("WARN", Color32::from_rgb(230, 180, 60)),
            LogLevel::Error => ("ERROR", Color32::from_rgb(214, 90, 70)),
        }
    }
    /// Parse a level from a common textual form (case-insensitive); unknown → `Info`.
    pub fn parse(s: &str) -> LogLevel {
        match s.trim().to_ascii_lowercase().as_str() {
            "debug" | "dbg" | "trace" => LogLevel::Debug,
            "warn" | "warning" => LogLevel::Warn,
            "error" | "err" | "fatal" => LogLevel::Error,
            _ => LogLevel::Info,
        }
    }
}

/// One log line. `id` is a stable per-entry key (e.g. a monotonic sequence) so its fade-in plays once.
#[derive(Clone, Debug)]
pub struct LogEntry {
    pub level: LogLevel,
    pub text: String,
    pub id: u64,
}

impl LogEntry {
    pub fn new(level: LogLevel, text: impl Into<String>, id: u64) -> Self {
        Self { level, text: text.into(), id }
    }
}

/// Render `entries` newest-last with coloured level badges, padding, and a fade-in per new entry.
/// Auto-scrolls to the bottom. Drop into any logs/analysis pane.
pub fn log_view(ui: &mut Ui, entries: &[LogEntry]) {
    egui::ScrollArea::vertical().auto_shrink([false, false]).stick_to_bottom(true).show(ui, |ui| {
        for e in entries {
            log_row(ui, e);
        }
    });
}

/// A single log row (also usable standalone).
pub fn log_row(ui: &mut Ui, e: &LogEntry) {
    // fade the row in on first sight; keyed by id so it only plays once.
    let fade = motion::animate_eased(ui.ctx(), egui::Id::new(("logrow", e.id)), true, 0.35, motion::ease::out_cubic);
    let (label, col) = e.level.badge();
    egui::Frame::new().inner_margin(egui::Margin::symmetric(10, 5)).show(ui, |ui| {
        ui.set_opacity(fade);
        ui.horizontal(|ui| {
            // level chip
            egui::Frame::new()
                .fill(col)
                .inner_margin(egui::Margin::symmetric(6, 1))
                .corner_radius(egui::CornerRadius::same(4))
                .show(ui, |ui| {
                    ui.label(egui::RichText::new(label).monospace().small().strong().color(Color32::WHITE));
                });
            ui.add_space(8.0);
            // errors tint their message; warnings stay default but the chip carries the signal
            let text_col = if e.level == LogLevel::Error { col } else { ui.visuals().text_color() };
            ui.label(egui::RichText::new(&e.text).color(text_col));
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn badges_are_distinct_and_labelled() {
        let levels = [LogLevel::Debug, LogLevel::Info, LogLevel::Warn, LogLevel::Error];
        let labels: Vec<&str> = levels.iter().map(|l| l.badge().0).collect();
        assert_eq!(labels, ["DEBUG", "INFO", "WARN", "ERROR"]);
        // every level has a distinct colour
        let cols: std::collections::HashSet<_> = levels.iter().map(|l| format!("{:?}", l.badge().1)).collect();
        assert_eq!(cols.len(), 4);
    }

    #[test]
    fn parse_maps_common_forms_and_defaults_to_info() {
        assert_eq!(LogLevel::parse("WARNING"), LogLevel::Warn);
        assert_eq!(LogLevel::parse(" err "), LogLevel::Error);
        assert_eq!(LogLevel::parse("trace"), LogLevel::Debug);
        assert_eq!(LogLevel::parse("chatter"), LogLevel::Info); // unknown → info
    }
}
