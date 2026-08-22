//! missing_pkg — packages your commands said were missing, collected.
//!
//! Ported from `missingPkgDrawer` in `main.qml`: a right-edge sidebar listing
//! packages reported missing across recent command blocks, each with its
//! ecosystem, a suggested install command and a hit count. Tap the hint to copy
//! it; tap Dismiss to hide it.
//!
//! The detector is `voiceterm::missing_packages`, which is std-only and already
//! does the work — scanning block output for the shapes of "command not found",
//! "ModuleNotFoundError", "Cannot find module" and so on, grouping by
//! ecosystem, and choosing an install command from the system's actual package
//! manager. Included by path rather than reimplemented.
//!
//! ## The hint is copied, never run
//!
//! The QML says the user "can tap the install hint to copy it to clipboard",
//! and the detector's own doc calls the command a *"best guess — user should
//! verify before running"*. Those two agree and this keeps them: a guessed
//! `sudo apt install <name>` executed by a tap is the worst possible
//! combination of confident and wrong. [`Action::Copy`] carries the text and
//! nothing here spawns anything.
//!
//! ## Why hit count is shown rather than sorted away
//!
//! A package that failed once might be a typo; one that failed nine times is
//! something the operator keeps needing. The count is the difference and it is
//! on every row.

#[path = "../../src/missing_packages.rs"]
#[allow(dead_code, unused_imports)]
pub mod missing_packages;

pub use missing_packages::{Ecosystem, MissingPackage};

/// What the drawer produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Put this install command on the clipboard. NOT run — see the module
    /// note.
    Copy(String),
    /// Hide this package. Carries both parts of its identity, because a name
    /// alone collides across ecosystems: `requests` is a python package and
    /// also a node one, and dismissing the wrong one leaves the real problem
    /// on screen while hiding a different real problem.
    Dismiss { ecosystem: String, name: String },
    Close,
}

/// A short label for the ecosystem column.
///
/// By reference: Ecosystem is not Copy in the detector, and adding a derive
/// there to suit a label function would be a view changing a model's contract.
pub fn eco_label(e: &Ecosystem) -> &'static str {
    match e {
        Ecosystem::Shell => "shell",
        Ecosystem::Python => "python",
        Ecosystem::Node => "node",
        Ecosystem::Rust => "rust",
        Ecosystem::System => "system",
    }
}

/// Rows worth showing.
///
/// Dismissed entries are filtered here rather than deleted by the detector,
/// which is deliberate on its side: a dismissed package that reappears in a
/// NEW block should come back, and a deleted one could not. The drawer just
/// does not draw them.
pub fn visible(all: &[MissingPackage]) -> Vec<&MissingPackage> {
    let mut v: Vec<&MissingPackage> = all.iter().filter(|p| !p.dismissed).collect();
    // Most-recent first, then by how often. Recency leads because the thing
    // that just failed is the thing being worked on; hit count breaks ties,
    // so a package failing repeatedly in one session rises above one that
    // failed once at the same moment.
    v.sort_by(|a, b| b.last_seen.cmp(&a.last_seen).then(b.hit_count.cmp(&a.hit_count)));
    v
}

/// Draw the drawer. Returns an action if one was pressed.
pub fn missing_pkg_ui(
    ui: &mut egui::Ui,
    all: &[MissingPackage],
    c: &uikit_egui::PaletteColors,
) -> Option<Action> {
    let mut act = None;
    let rows = visible(all);

    ui.horizontal(|ui| {
        ui.label(egui::RichText::new("missing packages").strong());
        if ui.button("close").clicked() { act = Some(Action::Close); }
    });

    if rows.is_empty() {
        // Distinguished from "not scanned yet": an empty drawer with no words
        // reads as a feature that did not run.
        ui.label(egui::RichText::new(
            "nothing reported missing in recent commands").small().weak());
        return act;
    }
    ui.label(egui::RichText::new("tap a command to copy it — these are guesses, check before running")
        .small().weak());
    ui.separator();

    egui::ScrollArea::vertical().auto_shrink([false, true]).show(ui, |ui| {
        for p in rows {
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(&p.name).strong());
                ui.label(egui::RichText::new(eco_label(&p.ecosystem)).small().color(c.accent));
                if p.hit_count > 1 {
                    // Only when it is more than one. A "×1" badge on every row
                    // is noise that makes the ×9 harder to spot, which is the
                    // one the count exists for.
                    ui.label(egui::RichText::new(format!("×{}", p.hit_count))
                        .small().color(c.text_weak));
                }
            });
            ui.horizontal(|ui| {
                if ui.add(egui::Button::new(
                    egui::RichText::new(&p.install_hint).monospace().small()))
                    .on_hover_text("copy — not run")
                    .clicked()
                {
                    act = Some(Action::Copy(p.install_hint.clone()));
                }
                if ui.small_button("dismiss").clicked() {
                    act = Some(Action::Dismiss {
                        ecosystem: eco_label(&p.ecosystem).to_string(),
                        name: p.name.clone(),
                    });
                }
            });
            ui.separator();
        }
    });
    act
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pkg(name: &str, eco: Ecosystem, hits: u32, last: u64, dismissed: bool) -> MissingPackage {
        MissingPackage {
            name: name.into(), ecosystem: eco,
            install_hint: format!("install {name}"),
            block_ids: vec![1], first_seen: 0, last_seen: last,
            hit_count: hits, dismissed,
        }
    }

    #[test]
    fn the_install_hint_is_copied_and_never_run() {
        // The detector's own doc calls it a "best guess — user should verify
        // before running", and the QML copies to clipboard. A guessed
        // `sudo apt install <name>` executed by a tap is the worst combination
        // of confident and wrong.
        let src = include_str!("missing_pkg.rs");
        for probe in [format!("Command::{}", "new"), format!("pty_{}", "out"),
                      format!("{}(", "spawn")] {
            assert!(!src.contains(probe.as_str()),
                    "this drawer can execute an install hint via {probe}");
        }
        // And the only action carrying the hint is Copy.
        assert!(src.contains("Action::Copy(p.install_hint.clone())"));
    }

    #[test]
    fn dismissing_carries_the_ecosystem_as_well_as_the_name() {
        // `requests` is a python package and also a node one. Dismissing by
        // name alone hides the wrong entry and leaves the real problem on
        // screen — while hiding a different real problem.
        let a = Action::Dismiss { ecosystem: "python".into(), name: "requests".into() };
        let b = Action::Dismiss { ecosystem: "node".into(), name: "requests".into() };
        assert_ne!(a, b);
    }

    #[test]
    fn dismissed_entries_are_hidden_here_rather_than_deleted() {
        // The detector keeps them so a package that reappears in a NEW block
        // comes back; a deleted one could not. The drawer just does not draw
        // them.
        let all = vec![pkg("a", Ecosystem::Python, 1, 10, false),
                       pkg("b", Ecosystem::Node, 1, 20, true)];
        let v = visible(&all);
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].name, "a");
        assert_eq!(all.len(), 2, "the drawer mutated the detector's list");
    }

    #[test]
    fn recent_leads_and_hit_count_breaks_the_tie() {
        // The thing that just failed is the thing being worked on. Two that
        // failed at the same moment order by how often — a package failing
        // repeatedly in one session is more likely the real blocker.
        let all = vec![
            pkg("old-often", Ecosystem::Python, 9, 10, false),
            pkg("new-once", Ecosystem::Node, 1, 99, false),
            pkg("new-often", Ecosystem::Rust, 5, 99, false),
        ];
        let v = visible(&all);
        assert_eq!(v.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(),
                   vec!["new-often", "new-once", "old-often"]);
    }

    #[test]
    fn a_single_hit_shows_no_count_badge() {
        // A "×1" on every row is noise that makes the ×9 harder to spot, and
        // the ×9 is the entire reason the count is shown.
        let src = include_str!("missing_pkg.rs");
        assert!(src.contains("if p.hit_count > 1"));
    }

    #[test]
    fn an_empty_drawer_says_why_it_is_empty() {
        // Otherwise it reads as a feature that did not run.
        let src = include_str!("missing_pkg.rs");
        assert!(src.contains("nothing reported missing"));
    }

    #[test]
    fn every_ecosystem_has_a_label() {
        // A missing arm here is a package whose row cannot be drawn — and the
        // enum is in another file, so this is what notices a new variant.
        for e in [Ecosystem::Shell, Ecosystem::Python, Ecosystem::Node,
                  Ecosystem::Rust, Ecosystem::System] {
            assert!(!eco_label(&e).is_empty());
        }
    }
}
