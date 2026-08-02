//! Artifacts page — reclaim disk from regeneratable build output.
//!
//! `cockpit::artifacts` was one of fourteen cockpit modules the egui port never
//! called. It does two jobs — reclaiming disk, and keeping a `target/` or
//! `node_modules` out of the store on import — and its own header states the
//! safety property: "only known, regeneratable build output is ever touched;
//! source is never removed."
//!
//! ## Why the page shows sizes before it offers to delete
//!
//! "Clean build artifacts" with no number is a button nobody presses, because
//! the user cannot tell whether it will reclaim 40 MB or 40 GB and cannot tell
//! what it will take. A scan that names each directory and its size makes the
//! decision, and the button merely carries it out.
//!
//! ## Why clean() re-validates rather than trusting the scan
//!
//! `clean` re-checks `dir_junk_label` for every entry at deletion time. A scan
//! result is a snapshot, and between scanning and pressing the button a build
//! can finish, a directory can be replaced, or a symlink can change where it
//! points. Deleting from a stale list is how a "clean" removes source — and the
//! page must not undo that by caching a decision either.

use crate::app::App;

#[derive(Default)]
pub struct State {
    /// (path, label, bytes) — the last scan.
    pub found: Vec<(String, String, u64)>,
    pub scanned: bool,
    pub note: String,
    /// Awaiting confirmation. Deleting build output is safe by construction but
    /// slow to undo: a `target/` is minutes of rebuild, and the user should be
    /// choosing that rather than discovering it.
    pub confirm: bool,
}

/// Bytes as something a person can compare. Whole units below 10 so "1.2 GB"
/// reads at a glance while "1234567890 bytes" does not.
pub fn human_bytes(b: u64) -> String {
    const UNITS: [(&str, u64); 4] = [("GB", 1 << 30), ("MB", 1 << 20), ("KB", 1 << 10), ("B", 1)];
    for (name, scale) in UNITS {
        if b >= scale {
            let v = b as f64 / scale as f64;
            return if v < 10.0 && name != "B" { format!("{v:.1} {name}") }
                   else { format!("{} {name}", v.round() as u64) };
        }
    }
    "0 B".into()
}

pub fn ui(app: &mut App, ui: &mut egui::Ui) {
    ui.heading("Artifacts");
    ui.label(egui::RichText::new("regeneratable build output — source is never touched")
        .weak().small());
    ui.separator();

    // cockpit::flow already wraps scan and clean with the build base resolved
    // — reaching past it to artifacts:: directly would be a second place that
    // has to know which directory is never junk.
    // The same source the Files page uses — one answer to "which repo" rather
    // than a second one that can disagree with it.
    let root = cockpit::ops::repo_root();

    ui.horizontal(|ui| {
        if ui.add_enabled(!app.task.is_running(), egui::Button::new("↻ scan")).clicked() {
            let scan = cockpit::flow::reclaim_scan(&root);
            app.artifacts.found = scan.items.into_iter()
                .map(|(path, bytes, label)| (path, label, bytes)).collect();
            // Largest first: the decision is about disk, and a 4 GB target/ is
            // the entry that answers the question a 12 KB .DS_Store does not.
            app.artifacts.found.sort_by(|a, b| b.2.cmp(&a.2));
            app.artifacts.scanned = true;
        }
        let total: u64 = app.artifacts.found.iter().map(|(_, _, b)| b).sum();
        if app.artifacts.scanned {
            ui.label(egui::RichText::new(format!("{} in {} places",
                human_bytes(total), app.artifacts.found.len())).strong());
        }
    });

    if app.artifacts.scanned && app.artifacts.found.is_empty() {
        ui.label(egui::RichText::new("nothing to reclaim").weak());
    }

    egui::ScrollArea::vertical().max_height(320.0).show(ui, |ui| {
        for (path, label, bytes) in &app.artifacts.found {
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(human_bytes(*bytes)).monospace());
                ui.label(egui::RichText::new(label).weak().small());
                // The path last and elided: the label and the size are what the
                // decision turns on, and a long path pushes them off screen.
                ui.label(egui::RichText::new(path).small()).on_hover_text(path);
            });
        }
    });

    if !app.artifacts.found.is_empty() {
        ui.add_space(6.0);
        if !app.artifacts.confirm {
            if ui.button("clean").clicked() { app.artifacts.confirm = true; }
        } else {
            let total: u64 = app.artifacts.found.iter().map(|(_, _, b)| b).sum();
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(format!("Delete {} of build output?",
                    human_bytes(total))).strong());
                if ui.button("delete").clicked() {
                    // clean() re-validates each entry at deletion time, so a
                    // stale scan cannot remove something that stopped being
                    // junk. The page passes the ROOT rather than its own list
                    // precisely so that check happens.
                    let (n, freed) = cockpit::flow::reclaim_apply(
                        &root);
                    app.artifacts.note =
                        format!("removed {n} items, reclaimed {}", human_bytes(freed));
                    app.artifacts.found.clear();
                    app.artifacts.scanned = false;
                    app.artifacts.confirm = false;
                }
                if ui.button("cancel").clicked() { app.artifacts.confirm = false; }
            });
        }
    }

    if !app.artifacts.note.is_empty() {
        ui.add_space(6.0);
        ui.separator();
        ui.label(egui::RichText::new(&app.artifacts.note).monospace().small());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_read_at_a_glance() {
        // "1234567890 bytes" does not answer "is this worth deleting"; "1.1 GB"
        // does, and that is the only question this page asks.
        assert_eq!(human_bytes(0), "0 B");
        assert_eq!(human_bytes(512), "512 B");
        assert_eq!(human_bytes(1536), "1.5 KB");
        assert_eq!(human_bytes(40 * (1 << 20)), "40 MB");
        assert_eq!(human_bytes(1 << 30), "1.0 GB");
    }

    #[test]
    fn large_values_lose_the_decimal_because_precision_stops_mattering() {
        // 40.3 MB and 40 MB are the same decision.
        assert_eq!(human_bytes(1 << 20), "1.0 MB");
        assert_eq!(human_bytes(12 * (1 << 20)), "12 MB");
    }
}
