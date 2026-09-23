//! show — how gitmind draws what went wrong, borrowed from stoat-egui.
//!
//! Two things stoat does that made its debugging readable and gitmind's not:
//!
//! 1. A diagnosis is data (`cockpit::finding::Finding`) and is drawn as the
//!    kit's CALLOUT: severity chip, code, a title in plain words, notes that
//!    explain, help lines in the success colour so the way out is findable.
//!    gitmind printed verdicts with `{:?}` and failures as one-line strings.
//! 2. Logs are TYPED rows (`uikit_egui::log_view`): a level chip, wrapped
//!    text, levels from the kit's `classify_line`, which classifies build
//!    output by the SHAPE of a diagnostic. gitmind's push page used
//!    `contains("error")`, which paints `Compiling thiserror` red.

use cockpit::finding::{Finding, Severity};
use uikit_egui::callout;
use uikit_egui::log_view::{classify_line, log_row, LogEntry};
use uikit_egui::toast::Severity as KitSeverity;
use uikit_egui::PaletteColors;

/// Rows kept on screen: log_view lays out every row it is given, every
/// frame, and a cargo build is thousands of lines (stoat keeps 600).
pub const LOG_WINDOW: usize = 400;

fn kit(sev: Severity) -> KitSeverity {
    match sev {
        Severity::Error => KitSeverity::Error,
        Severity::Warning => KitSeverity::Warning,
        Severity::Info => KitSeverity::Info,
        Severity::Ok => KitSeverity::Success,
    }
}

/// One finding as a collapsible callout (open state remembered per code),
/// with the fix, if any, as copyable monospace text below it.
pub fn finding(ui: &mut egui::Ui, c: &PaletteColors, f: &Finding) {
    let id = egui::Id::new(("finding-open", &f.code));
    let mut open = ui.data(|d| d.get_temp::<bool>(id)).unwrap_or(true);
    let lines: Vec<callout::Line> = f.notes.iter().map(callout::Line::note).chain(f.help.iter().map(callout::Line::help)).collect();
    // Scoped by code: the callout's own widgets are not keyed by content, so
    // two callouts on one page clashed (egui's red "second use of widget ID",
    // seen in a rendered screenshot with two findings).
    ui.push_id(("finding", &f.code), |ui| {
        callout::show_collapsible(ui, c, kit(f.severity), &f.code, &f.title, &lines, &mut open);
    });
    ui.data_mut(|d| d.insert_temp(id, open));
    if let (true, Some(fix)) = (open, &f.fix) {
        // The button on its own line, the fix full-width below: side by side
        // in a wrapped row, the long rule wrapped over the button (the phone
        // layout test caught it; the old Diagnostics page had the same bug).
        if ui.small_button("copy fix").clicked() {
            ui.ctx().copy_text(fix.clone());
        }
        ui.add(egui::Label::new(egui::RichText::new(fix).monospace().small()).wrap());
    }
}

pub fn findings(ui: &mut egui::Ui, c: &PaletteColors, list: &[Finding]) {
    for f in list {
        finding(ui, c, f);
        ui.add_space(4.0);
    }
}

/// The last [`LOG_WINDOW`] lines as typed rows, newest last, pinned to the
/// bottom while it grows. `salt` keeps two logs on one page apart.
pub fn task_log(ui: &mut egui::Ui, salt: &str, lines: &[String], max_height: f32) {
    let start = lines.len().saturating_sub(LOG_WINDOW);
    egui::ScrollArea::vertical()
        .id_salt(("task-log", salt))
        .max_height(max_height)
        .stick_to_bottom(true)
        .auto_shrink([false, true])
        .show(ui, |ui| {
            if start > 0 {
                ui.label(egui::RichText::new(format!("… {start} earlier lines not shown")).weak().small());
            }
            for (i, line) in lines[start..].iter().enumerate() {
                log_row(ui, &LogEntry::new(classify_line(line), line.clone(), (start + i) as u64));
            }
        });
}

#[cfg(test)]
mod tests {
    use super::*;
    use uikit_egui::log_view::LogLevel;

    #[test]
    fn a_crate_named_error_is_not_an_error() {
        // the trap the push page fell into with contains("error")
        assert_eq!(classify_line("   Compiling thiserror v2.0.12"), LogLevel::Debug);
        assert_eq!(classify_line("error[E0308]: mismatched types"), LogLevel::Error);
    }

    #[test]
    fn every_severity_has_a_kit_colour() {
        for s in [Severity::Error, Severity::Warning, Severity::Info, Severity::Ok] {
            let _ = kit(s);
        }
    }
}
