//! fleet_actions — the Fleet page's press-hold action matrix, on top of
//! `uikit_egui::slide_menu` the same way voiceterm-egui's `gesture_menus.rs`
//! uses it: hold a "⋯" host button, a grid of options opens, drag/release
//! picks one. What lives here is the *data + id round-trip* (pure,
//! unit-tested); `pages/fleet.rs` does the actual `cockpit`/clipboard I/O
//! each action means.
//!
//! Two grids, not one, because they answer different layout questions:
//! - [`utility_menu`] — copy log / copy errors / copy build times / export
//!   report. Added regardless of screen width, next to whatever build/test
//!   controls are already on screen — these four never had ANY UI before
//!   this (the ask this module exists to answer), unlike build/test/build
//!   all/test all, which already have dedicated buttons.
//! - [`full_menu`] — the same four PLUS build all / test all, for narrow
//!   screens where a whole row of individual buttons doesn't fit
//!   (`uikit_egui::adaptive::Adaptive::is_compact()`, the same breakpoint
//!   `app.rs` already uses for the nav sidebar-vs-bottom-bar switch) —
//!   "unifying the current options on non-wide screens."
//!
//! Per-app "build + install locally" / "test" stay as their own buttons on
//! every width; they take a selected app as a parameter and don't fit a
//! static id-based grid without a bigger refactor of that closure capture.
//! The utility actions apply to whatever the shared `Task` last captured
//! (a per-app build, a per-app test, build all, or test all alike), so they
//! aren't tied to the whole-catalog buttons specifically — just easiest to
//! reach from next to them.

use uikit_egui::slide_menu::SlideOption;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FleetAction {
    BuildAll,
    TestAll,
    CopyLog,
    CopyErrors,
    CopyTimes,
    Export,
}

impl FleetAction {
    pub fn id(self) -> &'static str {
        match self {
            FleetAction::BuildAll => "build-all",
            FleetAction::TestAll => "test-all",
            FleetAction::CopyLog => "copy-log",
            FleetAction::CopyErrors => "copy-errors",
            FleetAction::CopyTimes => "copy-times",
            FleetAction::Export => "export",
        }
    }
    pub fn from_id(id: &str) -> Option<FleetAction> {
        Some(match id {
            "build-all" => FleetAction::BuildAll,
            "test-all" => FleetAction::TestAll,
            "copy-log" => FleetAction::CopyLog,
            "copy-errors" => FleetAction::CopyErrors,
            "copy-times" => FleetAction::CopyTimes,
            "export" => FleetAction::Export,
            _ => return None,
        })
    }
    pub fn label(self) -> &'static str {
        match self {
            FleetAction::BuildAll => "build all",
            FleetAction::TestAll => "test all",
            FleetAction::CopyLog => "\u{1F4CB} copy log",
            FleetAction::CopyErrors => "\u{26A0} errors only",
            FleetAction::CopyTimes => "\u{23F1} times by crate",
            FleetAction::Export => "\u{1F4E5} export report",
        }
    }
}

/// Copy/export only — added next to the existing build/test/build-all/test-all
/// buttons regardless of screen width.
pub fn utility_menu() -> Vec<SlideOption> {
    [FleetAction::CopyLog, FleetAction::CopyErrors, FleetAction::CopyTimes, FleetAction::Export]
        .into_iter()
        .map(|a| SlideOption::new(a.id(), a.label()))
        .collect()
}

/// Everything the whole-catalog row + the utility menu cover, unified into
/// one grid for narrow screens.
pub fn full_menu() -> Vec<SlideOption> {
    [
        FleetAction::BuildAll,
        FleetAction::TestAll,
        FleetAction::CopyLog,
        FleetAction::CopyErrors,
        FleetAction::CopyTimes,
        FleetAction::Export,
    ]
    .into_iter()
    .map(|a| SlideOption::new(a.id(), a.label()))
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL: [FleetAction; 6] = [
        FleetAction::BuildAll,
        FleetAction::TestAll,
        FleetAction::CopyLog,
        FleetAction::CopyErrors,
        FleetAction::CopyTimes,
        FleetAction::Export,
    ];

    #[test]
    fn ids_round_trip() {
        for a in ALL {
            assert_eq!(FleetAction::from_id(a.id()), Some(a));
        }
        assert_eq!(FleetAction::from_id("nope"), None);
    }

    #[test]
    fn full_menu_covers_all_six_with_no_duplicate_ids() {
        let m = full_menu();
        assert_eq!(m.len(), 6);
        let mut ids: Vec<&str> = m.iter().map(|o| o.id.as_str()).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), 6);
    }

    #[test]
    fn utility_menu_is_the_four_copy_export_actions_only() {
        let m = utility_menu();
        assert_eq!(m.len(), 4);
        for o in &m {
            let a = FleetAction::from_id(&o.id).expect("known id");
            assert!(
                matches!(a, FleetAction::CopyLog | FleetAction::CopyErrors | FleetAction::CopyTimes | FleetAction::Export),
                "utility menu must not include build-all/test-all — those already have dedicated buttons"
            );
        }
    }
}
