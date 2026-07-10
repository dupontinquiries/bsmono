//! action_compass — a compass action menu for the voiceterm toolbar, built on the shared
//! `uikit_egui::slide_menu::show_cardinal`.
//!
//! This is the terminal-side counterpart to rusty-board's cardinal keyboard: a single "⋯" host button
//! that, on press, floats a ring of quick terminal actions around it (clear, copy, paste, search,
//! interrupt); the live swipe highlights one and release runs it. Same gesture, same widget — the QML
//! toolbar's GestureHost/SlidePopup pattern, egui-native.
//!
//! The direction→action map and id round-trip are pure and unit-tested.

use uikit_egui::slide_menu::{Dir8, SlideMenu, SlideOption, SlideOutcome};

/// A quick terminal action reachable from the compass.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TermAction {
    Clear,
    Copy,
    Paste,
    Search,
    Interrupt,
}

impl TermAction {
    /// Stable id used as the compass option id (and round-tripped back via [`TermAction::from_id`]).
    pub fn id(self) -> &'static str {
        match self {
            TermAction::Clear => "clear",
            TermAction::Copy => "copy",
            TermAction::Paste => "paste",
            TermAction::Search => "search",
            TermAction::Interrupt => "interrupt",
        }
    }
    pub fn from_id(id: &str) -> Option<TermAction> {
        Some(match id {
            "clear" => TermAction::Clear,
            "copy" => TermAction::Copy,
            "paste" => TermAction::Paste,
            "search" => TermAction::Search,
            "interrupt" => TermAction::Interrupt,
            _ => return None,
        })
    }
    /// Short glyph/label shown in the compass cell.
    pub fn label(self) -> &'static str {
        match self {
            TermAction::Clear => "clr",
            TermAction::Copy => "copy",
            TermAction::Paste => "paste",
            TermAction::Search => "find",
            TermAction::Interrupt => "^C",
        }
    }
}

/// The compass layout: which action sits in each direction. Pure — the tested core.
pub fn compass_layout() -> [(Dir8, TermAction); 4] {
    [
        (Dir8::N, TermAction::Copy),
        (Dir8::E, TermAction::Search),
        (Dir8::S, TermAction::Paste),
        (Dir8::W, TermAction::Interrupt),
    ]
}

/// A toolbar compass control. Hold one; call [`ActionCompass::ui`] where the toolbar button belongs.
pub struct ActionCompass {
    menu: SlideMenu,
    /// The center-tap action (a plain click on the button), by default `Clear`.
    pub tap: TermAction,
}

impl Default for ActionCompass {
    fn default() -> Self {
        Self { menu: SlideMenu::new("voiceterm-action-compass").item_size(46.0), tap: TermAction::Clear }
    }
}

impl ActionCompass {
    /// Render the "⋯" host button and its compass; returns the chosen [`TermAction`] on release.
    pub fn ui(&mut self, ui: &mut egui::Ui) -> Option<TermAction> {
        let resp = ui.add(egui::Button::new("⋯").min_size(egui::vec2(34.0, 28.0)));
        let id = resp.id;
        let tap = SlideOption::new(self.tap.id(), self.tap.label());
        let dirs: Vec<(Dir8, SlideOption)> = compass_layout()
            .into_iter()
            .map(|(d, a)| (d, SlideOption::new(a.id(), a.label())))
            .collect();
        match self.menu.show_cardinal(ui, id, &resp, &tap, &dirs) {
            SlideOutcome::Selected(chosen) => TermAction::from_id(&chosen),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn action_ids_round_trip() {
        for a in [TermAction::Clear, TermAction::Copy, TermAction::Paste, TermAction::Search, TermAction::Interrupt] {
            assert_eq!(TermAction::from_id(a.id()), Some(a));
        }
        assert_eq!(TermAction::from_id("nope"), None);
    }

    #[test]
    fn compass_covers_four_distinct_directions_and_actions() {
        let layout = compass_layout();
        let dirs: std::collections::HashSet<_> = layout.iter().map(|(d, _)| format!("{d:?}")).collect();
        let acts: std::collections::HashSet<_> = layout.iter().map(|(_, a)| a.id()).collect();
        assert_eq!(dirs.len(), 4); // no direction reused
        assert_eq!(acts.len(), 4); // no action reused
    }
}
