//! action_compass — a compass action menu for the voiceterm toolbar, built on the shared
//! `uikit_egui::slide_menu::show_cardinal` and wired to real terminal [`crate::App`] actions.
//!
//! A single "⋯" host button that, on press, floats a ring of quick actions around it; the live swipe
//! highlights one and release runs it. Same gesture as rusty-board's cardinal keyboard — the QML
//! toolbar's GestureHost/SlidePopup pattern, egui-native. Actions operate on the whole `App` so the
//! compass can both mutate the terminal (clear/copy/interrupt/stage) and open the find bar (search).
//!
//! The direction→action map and id round-trip are pure and unit-tested; [`apply`] is exercised against
//! a headless `egui::Context`.

use crate::App;
use uikit_egui::slide_menu::{Dir8, SlideMenu, SlideOption, SlideOutcome};

/// A quick action reachable from the compass.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TermAction {
    /// Clear the visible screen.
    Clear,
    /// Copy the visible screen to the clipboard.
    Copy,
    /// Clear the input line (^C).
    Interrupt,
    /// Stage `clear` on the input line (not run).
    StageClear,
    /// Open the in-screen find bar.
    Search,
}

impl TermAction {
    pub fn id(self) -> &'static str {
        match self {
            TermAction::Clear => "clear",
            TermAction::Copy => "copy",
            TermAction::Interrupt => "interrupt",
            TermAction::StageClear => "stage-clear",
            TermAction::Search => "search",
        }
    }
    pub fn from_id(id: &str) -> Option<TermAction> {
        Some(match id {
            "clear" => TermAction::Clear,
            "copy" => TermAction::Copy,
            "interrupt" => TermAction::Interrupt,
            "stage-clear" => TermAction::StageClear,
            "search" => TermAction::Search,
            _ => return None,
        })
    }
    /// Short glyph/label shown in the compass cell.
    pub fn label(self) -> &'static str {
        match self {
            TermAction::Clear => "clr",
            TermAction::Copy => "copy",
            TermAction::Interrupt => "^C",
            TermAction::StageClear => "clear",
            TermAction::Search => "find",
        }
    }
}

/// Which action sits in each compass direction (tap = [`TermAction::Clear`]). Pure — the tested core.
pub fn compass_layout() -> [(Dir8, TermAction); 4] {
    [
        (Dir8::N, TermAction::Copy),
        (Dir8::E, TermAction::Search),
        (Dir8::S, TermAction::Interrupt),
        (Dir8::W, TermAction::StageClear),
    ]
}

/// Perform `action` against `app` (copying through `ctx` when needed).
pub fn apply(action: TermAction, app: &mut App, ctx: &egui::Context) {
    match action {
        TermAction::Clear => app.terminal.clear(),
        TermAction::Copy => ctx.copy_text(app.terminal.screen_text()),
        TermAction::Interrupt => app.terminal.interrupt(),
        TermAction::StageClear => app.terminal.stage("clear"),
        TermAction::Search => app.open_search(),
    }
}

/// A toolbar compass control. Hold one; call [`ActionCompass::ui`] where the toolbar button belongs; on
/// a chosen action it runs [`apply`] against `app`.
pub struct ActionCompass {
    menu: SlideMenu,
    /// The center-tap action (a plain press/release with no swipe).
    pub tap: TermAction,
}

impl Default for ActionCompass {
    fn default() -> Self {
        Self { menu: SlideMenu::new("voiceterm-action-compass").item_size(46.0), tap: TermAction::Clear }
    }
}

impl ActionCompass {
    /// Render the "⋯" host button and its compass; on release, run the chosen action against `app` and
    /// return it (so the caller can also react, e.g. flash a toast).
    pub fn ui(&mut self, ui: &mut egui::Ui, app: &mut App) -> Option<TermAction> {
        let resp = ui.add(egui::Button::new("⋯").min_size(egui::vec2(34.0, 28.0)));
        let id = resp.id;
        self.show_from_host(ui, id, &resp, app)
    }

    /// Same compass, anchored to a HOST RESPONSE THE CALLER ALREADY DREW (e.g. voiceterm's mic FAB —
    /// see shell.rs's `mic_fab_ui`) instead of drawing our own generic button. This is what the real
    /// app does: the compass opens from the mic button itself, not a separate "⋯".
    pub fn ui_from_host(&mut self, ui: &mut egui::Ui, host_resp: &egui::Response, app: &mut App) -> Option<TermAction> {
        let id = host_resp.id;
        self.show_from_host(ui, id, host_resp, app)
    }

    fn show_from_host(&mut self, ui: &mut egui::Ui, id: egui::Id, resp: &egui::Response, app: &mut App) -> Option<TermAction> {
        let tap = SlideOption::new(self.tap.id(), self.tap.label());
        let dirs: Vec<(Dir8, SlideOption)> = compass_layout()
            .into_iter()
            .map(|(d, a)| (d, SlideOption::new(a.id(), a.label())))
            .collect();
        if let SlideOutcome::Selected(chosen) = self.menu.show_cardinal(ui, id, resp, &tap, &dirs) {
            if let Some(action) = TermAction::from_id(&chosen) {
                apply(action, app, ui.ctx());
                return Some(action);
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn action_ids_round_trip() {
        for a in [TermAction::Clear, TermAction::Copy, TermAction::Interrupt, TermAction::StageClear, TermAction::Search] {
            assert_eq!(TermAction::from_id(a.id()), Some(a));
        }
        assert_eq!(TermAction::from_id("nope"), None);
    }

    #[test]
    fn compass_covers_four_distinct_directions_and_actions() {
        let layout = compass_layout();
        let dirs: std::collections::HashSet<_> = layout.iter().map(|(d, _)| format!("{d:?}")).collect();
        let acts: std::collections::HashSet<_> = layout.iter().map(|(_, a)| a.id()).collect();
        assert_eq!(dirs.len(), 4);
        assert_eq!(acts.len(), 4);
    }

    #[test]
    fn apply_runs_each_action_on_the_app() {
        let ctx = egui::Context::default();
        let mut app = App::new(20, 4);
        app.terminal.feed("output here");
        app.terminal.stage("half typed");

        apply(TermAction::Search, &mut app, &ctx);
        assert!(app.search_open, "search opens the find bar");

        apply(TermAction::Interrupt, &mut app, &ctx);
        assert_eq!(app.terminal.input, "", "^C clears the input line");

        apply(TermAction::StageClear, &mut app, &ctx);
        assert_eq!(app.terminal.input, "clear");

        apply(TermAction::Clear, &mut app, &ctx);
        assert_eq!(app.terminal.screen_text(), "", "clear wipes the screen");
    }
}
