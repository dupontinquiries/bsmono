//! action_compass — a compass action menu for the voiceterm toolbar, built on the shared
//! `uikit_egui::slide_menu::show_cardinal`, wired to real [`crate::Terminal`] actions.
//!
//! A single "⋯" host button that, on press, floats a ring of quick terminal actions around it; the
//! live swipe highlights one and release runs it against the terminal. Same gesture as rusty-board's
//! cardinal keyboard — the QML toolbar's GestureHost/SlidePopup pattern, egui-native.
//!
//! The direction→action map and id round-trip are pure and unit-tested; [`apply`] performs the action
//! on a `Terminal` (whose `clear`/`screen_text`/`interrupt`/`stage` are themselves tested in uikit-term).

use crate::Terminal;
use uikit_egui::slide_menu::{Dir8, SlideMenu, SlideOption, SlideOutcome};

/// A quick terminal action reachable from the compass.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TermAction {
    /// Clear the visible screen.
    Clear,
    /// Copy the visible screen to the clipboard.
    Copy,
    /// Clear the input line (^C).
    Interrupt,
    /// Stage `ls` on the input line (not run).
    StageLs,
    /// Stage `clear` on the input line (not run).
    StageClear,
}

impl TermAction {
    pub fn id(self) -> &'static str {
        match self {
            TermAction::Clear => "clear",
            TermAction::Copy => "copy",
            TermAction::Interrupt => "interrupt",
            TermAction::StageLs => "stage-ls",
            TermAction::StageClear => "stage-clear",
        }
    }
    pub fn from_id(id: &str) -> Option<TermAction> {
        Some(match id {
            "clear" => TermAction::Clear,
            "copy" => TermAction::Copy,
            "interrupt" => TermAction::Interrupt,
            "stage-ls" => TermAction::StageLs,
            "stage-clear" => TermAction::StageClear,
            _ => return None,
        })
    }
    /// Short glyph/label shown in the compass cell.
    pub fn label(self) -> &'static str {
        match self {
            TermAction::Clear => "clr",
            TermAction::Copy => "copy",
            TermAction::Interrupt => "^C",
            TermAction::StageLs => "ls",
            TermAction::StageClear => "clear",
        }
    }
}

/// Which action sits in each compass direction (tap = [`TermAction::Clear`]). Pure — the tested core.
pub fn compass_layout() -> [(Dir8, TermAction); 4] {
    [
        (Dir8::N, TermAction::Copy),
        (Dir8::E, TermAction::StageLs),
        (Dir8::S, TermAction::Interrupt),
        (Dir8::W, TermAction::StageClear),
    ]
}

/// Perform `action` against `term` (copying to the clipboard through `ctx` when needed).
pub fn apply(action: TermAction, term: &mut Terminal, ctx: &egui::Context) {
    match action {
        TermAction::Clear => term.clear(),
        TermAction::Copy => ctx.copy_text(term.screen_text()),
        TermAction::Interrupt => term.interrupt(),
        TermAction::StageLs => term.stage("ls"),
        TermAction::StageClear => term.stage("clear"),
    }
}

/// A toolbar compass control. Hold one; call [`ActionCompass::ui`] where the toolbar button belongs;
/// on a chosen action it runs [`apply`] against `term`.
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
    /// Render the "⋯" host button and its compass; on release, run the chosen action against `term`
    /// and return it (so the caller can also react, e.g. flash a toast).
    pub fn ui(&mut self, ui: &mut egui::Ui, term: &mut Terminal) -> Option<TermAction> {
        let resp = ui.add(egui::Button::new("⋯").min_size(egui::vec2(34.0, 28.0)));
        let id = resp.id;
        let tap = SlideOption::new(self.tap.id(), self.tap.label());
        let dirs: Vec<(Dir8, SlideOption)> = compass_layout()
            .into_iter()
            .map(|(d, a)| (d, SlideOption::new(a.id(), a.label())))
            .collect();
        if let SlideOutcome::Selected(chosen) = self.menu.show_cardinal(ui, id, &resp, &tap, &dirs) {
            if let Some(action) = TermAction::from_id(&chosen) {
                apply(action, term, ui.ctx());
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
        for a in [TermAction::Clear, TermAction::Copy, TermAction::Interrupt, TermAction::StageLs, TermAction::StageClear] {
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
    fn clear_and_interrupt_and_stage_mutate_the_terminal() {
        // the clipboard (Copy) path needs an egui Context, but the terminal-mutating actions don't.
        let mut t = Terminal::new(20, 4);
        t.feed("output here");
        assert_eq!(t.screen_text(), "output here");

        // Clear is applied by calling the terminal method the dispatcher uses.
        t.clear();
        assert_eq!(t.screen_text(), "");

        t.stage("git status");
        assert_eq!(t.input, "git status");
        t.interrupt();
        assert_eq!(t.input, "");
    }
}
