//! Timeline page — checkpoints of this repo (gitmind's CheckpointEngine over the git history).
//!
//! Filled out from the first-port read-only list (oid + label only) to the surface the Qt page
//! and `gin` both imply the GUI should have:
//!   • relative age per checkpoint (the loader always had `when`; the first port dropped it),
//!   • **snapshot now** — the core gitmind ritual (".gitmind/README: checkpoint before every risky
//!     step") finally reachable without the CLI; reports "no changes" honestly when the engine
//!     returns None instead of pretending a checkpoint was made,
//!   • copy-oid per row (the thing you paste into `gin export-native` / `materialize`),
//!   • select a row → the file-level delta vs the worktree (CheckpointEngine::diff), capped for
//!     display — "what has changed since then", the question a timeline exists to answer.
//! Local git, so loads stay synchronous (fast); only snapshot uses the shared Task (it can hash a
//! large worktree).
use crate::app::App;

#[derive(Default)]
pub struct State {
    pub loaded: bool,
    pub checkpoints: Vec<(String, i64, String)>, // (oid, when, label)
    pub err: String,
    pub snap_label: String,
    pub note: String,
    /// Selected checkpoint oid + its file deltas vs the current worktree (path, +added, -removed).
    pub selected: Option<String>,
    pub deltas: Vec<(String, u64, u64)>,
}

/// Compact "how long ago" for a unix timestamp, next to each checkpoint. Pure for testing; `now`
/// is passed in. Sub-minute is "now" — checkpoint granularity, not a stopwatch.
pub fn ago(now: i64, when: i64) -> String {
    let d = (now - when).max(0);
    match d {
        0..=59 => "now".into(),
        60..=3599 => format!("{}m ago", d / 60),
        3600..=86_399 => format!("{}h ago", d / 3600),
        86_400..=2_591_999 => format!("{}d ago", d / 86_400),
        _ => format!("{}mo ago", d / 2_592_000),
    }
}

fn engine_at_root() -> Option<gitmind_core::backend::GitCli> {
    gitmind_core::backend::GitCli::discover(cockpit::ops::repo_root())
}

fn load(state: &mut State) {
    state.loaded = true;
    state.err.clear();
    match engine_at_root() {
        Some(git) => {
            let eng = gitmind_core::checkpoint::CheckpointEngine::new(&git, "main");
            match eng.list(40) {
                Ok(cps) => {
                    state.checkpoints = cps.into_iter().map(|c| (c.oid, c.when, c.label)).collect();
                }
                Err(e) => state.err = e,
            }
        }
        None => state.err = "not inside a git repo".to_string(),
    }
}

fn load_deltas(state: &mut State, oid: &str) {
    state.deltas.clear();
    if let Some(git) = engine_at_root() {
        let eng = gitmind_core::checkpoint::CheckpointEngine::new(&git, "main");
        match eng.diff(oid, None) {
            // numstat-style (+added / -removed per file) — what CheckpointEngine::diff measures
            Ok(ds) => {
                state.deltas = ds.into_iter().map(|d| (d.path, d.added, d.removed)).collect();
            }
            Err(e) => state.err = e,
        }
    }
}

pub fn ui(app: &mut App, ui: &mut egui::Ui) {
    ui.heading("Timeline");
    ui.label(egui::RichText::new("checkpoints of this repo — snapshot, inspect, copy").weak().small());
    ui.separator();

    // ── snapshot now ───────────────────────────────────────────────────────────────────────
    ui.horizontal(|ui| {
        let hint = egui::TextEdit::singleline(&mut app.timeline.snap_label)
            .hint_text("label (e.g. \"before merge\")")
            .desired_width(220.0);
        ui.add(hint);
        let can = !app.task.is_running();
        if ui.add_enabled(can, egui::Button::new("📌 snapshot now")).clicked() {
            let label = if app.timeline.snap_label.trim().is_empty() {
                "manual snapshot".to_string()
            } else {
                app.timeline.snap_label.trim().to_string()
            };
            app.task.run(move || match engine_at_root() {
                Some(git) => {
                    let eng = gitmind_core::checkpoint::CheckpointEngine::new(&git, "main");
                    match eng.snapshot(&label) {
                        Ok(Some(cp)) => format!("✓ checkpoint {} — {}", &cp.oid[..cp.oid.len().min(8)], cp.label),
                        Ok(None) => "nothing to snapshot — worktree matches the last checkpoint".into(),
                        Err(e) => format!("error: {e}"),
                    }
                }
                None => "not inside a git repo".into(),
            });
        }
        if ui.button("↻ refresh").clicked() || !app.timeline.loaded {
            load(&mut app.timeline);
        }
    });
    if let Some(r) = app.task.take_result() {
        app.timeline.note = r;
        app.timeline.loaded = false; // a snapshot changes the list — reload next frame
    }
    if !app.timeline.note.is_empty() {
        ui.label(egui::RichText::new(&app.timeline.note).monospace().small());
    }
    if !app.timeline.err.is_empty() {
        ui.colored_label(egui::Color32::from_rgb(0xf7, 0x76, 0x8e), &app.timeline.err);
    }
    ui.separator();

    if app.timeline.checkpoints.is_empty() && app.timeline.err.is_empty() {
        ui.label(egui::RichText::new("no checkpoints yet — 📌 snapshot now records the first one").weak());
    }

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);

    uikit_egui::scroll::bounded(ui, 260.0, |ui| {
        for (oid, when, label) in app.timeline.checkpoints.clone() {
            let short = &oid[..oid.len().min(8)];
            let selected = app.timeline.selected.as_deref() == Some(oid.as_str());
            ui.horizontal(|ui| {
                if ui.selectable_label(selected, egui::RichText::new(short).monospace()).clicked() {
                    if selected {
                        app.timeline.selected = None;
                        app.timeline.deltas.clear();
                    } else {
                        app.timeline.selected = Some(oid.clone());
                        load_deltas(&mut app.timeline, &oid);
                    }
                }
                ui.label(egui::RichText::new(ago(now, when)).weak().small());
                ui.label(&label);
                if ui.small_button("⧉").on_hover_text("copy full oid").clicked() {
                    ui.ctx().copy_text(oid.clone());
                }
            });
        }
    });

    // ── the selected checkpoint's delta vs the worktree ───────────────────────────────────
    if let Some(sel) = app.timeline.selected.clone() {
        ui.separator();
        let n = app.timeline.deltas.len();
        ui.label(
            egui::RichText::new(format!("changed since {} — {} file{}", &sel[..sel.len().min(8)], n, if n == 1 { "" } else { "s" }))
                .strong()
                .small(),
        );
        if n == 0 {
            ui.label(egui::RichText::new("worktree is identical to this checkpoint").weak().small());
        }
        const CAP: usize = 40;
        uikit_egui::scroll::bounded(ui, 160.0, |ui| {
            for (path, added, removed) in app.timeline.deltas.iter().take(CAP) {
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new(format!("+{added}")).monospace().small()
                        .color(egui::Color32::from_rgb(0x9e, 0xce, 0x6a)));
                    ui.label(egui::RichText::new(format!("-{removed}")).monospace().small()
                        .color(egui::Color32::from_rgb(0xf7, 0x76, 0x8e)));
                    ui.label(egui::RichText::new(path).monospace().small());
                });
            }
            if n > CAP {
                ui.label(egui::RichText::new(format!("… and {} more", n - CAP)).weak().small());
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ago_rounds_to_the_natural_unit() {
        assert_eq!(ago(1000, 990), "now");
        assert_eq!(ago(1000, 1000), "now");
        assert_eq!(ago(1000, 1100), "now", "clock skew must not print negative ages");
        assert_eq!(ago(10_000, 10_000 - 61), "1m ago");
        assert_eq!(ago(10_000, 10_000 - 59 * 60), "59m ago");
        assert_eq!(ago(100_000, 100_000 - 3600), "1h ago");
        assert_eq!(ago(1_000_000, 1_000_000 - 90_000), "1d ago");
        assert_eq!(ago(10_000_000, 10_000_000 - 40 * 86_400), "1mo ago");
    }
}
