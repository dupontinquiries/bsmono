//! What a press on a tile means — headless, so the rules are tested without a
//! frame.
//!
//! The gallery is used with a finger as often as a mouse (phones are a target),
//! and the two want the same model, the one every phone gallery has settled on:
//!
//! * a TAP opens the photo;
//! * a LONG-PRESS (or a right-click) starts selecting, with that photo;
//! * while anything is selected, a tap toggles instead of opening, so picking
//!   twenty photos is twenty taps, not twenty long-presses;
//! * on a keyboard, Ctrl-click toggles and Shift-click selects the run from the
//!   last photo touched, in the grid's order.
//!
//! It replaces click-to-select / double-click-to-open, which needed a double
//! tap to see a photo on a phone and left no gesture for "select".

use std::collections::BTreeSet;

/// A press on a tile, as the frame loop saw it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Press {
    /// A tap or a left click, with the modifiers held.
    Primary { ctrl: bool, shift: bool },
    /// A long-press on a touch screen, or a right click.
    Secondary,
}

impl Press {
    pub const TAP: Press = Press::Primary { ctrl: false, shift: false };
}

/// The selected photos and the one a range starts from.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Selection {
    paths: BTreeSet<String>,
    anchor: Option<String>,
}

impl Selection {
    pub fn is_empty(&self) -> bool {
        self.paths.is_empty()
    }
    pub fn len(&self) -> usize {
        self.paths.len()
    }
    pub fn contains(&self, path: &str) -> bool {
        self.paths.contains(path)
    }
    pub fn paths(&self) -> Vec<String> {
        self.paths.iter().cloned().collect()
    }
    pub fn clear(&mut self) {
        self.paths.clear();
        self.anchor = None;
    }
    /// Select exactly `paths` (Select all).
    pub fn set(&mut self, paths: impl IntoIterator<Item = String>) {
        self.paths = paths.into_iter().collect();
        self.anchor = None;
    }
    /// Drop paths that are no longer shown (an album's photo removed from it).
    pub fn retain(&mut self, keep: impl Fn(&str) -> bool) {
        self.paths.retain(|p| keep(p));
        if self.anchor.as_deref().is_some_and(|a| !keep(a)) {
            self.anchor = None;
        }
    }

    fn toggle(&mut self, path: &str) {
        if !self.paths.remove(path) {
            self.paths.insert(path.to_string());
        }
        self.anchor = Some(path.to_string());
    }

    /// Apply a press on `path`. `order` is the grid's order (for a Shift
    /// range). Returns the photo to open, when the press means "open".
    pub fn press(&mut self, order: &[String], path: &str, press: Press) -> Option<String> {
        match press {
            Press::Secondary | Press::Primary { ctrl: true, shift: false } => {
                self.toggle(path);
                None
            }
            Press::Primary { shift: true, .. } => {
                let range = self.anchor.as_deref().and_then(|a| {
                    let i = order.iter().position(|p| p == a)?;
                    let j = order.iter().position(|p| p == path)?;
                    Some(if i <= j { i..=j } else { j..=i })
                });
                match range {
                    Some(r) => self.paths.extend(order[r].iter().cloned()),
                    None => {
                        self.paths.insert(path.to_string());
                    }
                }
                self.anchor = Some(path.to_string());
                None
            }
            Press::Primary { .. } if !self.paths.is_empty() => {
                self.toggle(path);
                None
            }
            Press::Primary { .. } => Some(path.to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn order() -> Vec<String> {
        (0..6).map(|i| format!("p{i}")).collect()
    }

    #[test]
    fn a_tap_opens_and_a_long_press_starts_selecting() {
        let mut s = Selection::default();
        assert_eq!(s.press(&order(), "p2", Press::TAP), Some("p2".into()), "tap opens");
        assert!(s.is_empty(), "and selects nothing");
        assert_eq!(s.press(&order(), "p2", Press::Secondary), None);
        assert!(s.contains("p2"));
    }

    #[test]
    fn while_selecting_a_tap_toggles_instead_of_opening() {
        let mut s = Selection::default();
        s.press(&order(), "p1", Press::Secondary);
        assert_eq!(s.press(&order(), "p3", Press::TAP), None);
        assert_eq!(s.paths(), vec!["p1", "p3"]);
        s.press(&order(), "p1", Press::TAP);
        assert_eq!(s.paths(), vec!["p3"], "a second tap deselects");
        s.press(&order(), "p3", Press::TAP);
        assert!(s.is_empty());
        assert_eq!(s.press(&order(), "p4", Press::TAP), Some("p4".into()), "empty again: taps open");
    }

    #[test]
    fn shift_selects_the_run_from_the_last_touched_either_way() {
        let mut s = Selection::default();
        s.press(&order(), "p4", Press::Primary { ctrl: true, shift: false });
        s.press(&order(), "p1", Press::Primary { ctrl: false, shift: true });
        assert_eq!(s.paths(), vec!["p1", "p2", "p3", "p4"]);
        // A shift-click with no anchor just adds the one.
        let mut t = Selection::default();
        t.press(&order(), "p5", Press::Primary { ctrl: false, shift: true });
        assert_eq!(t.paths(), vec!["p5"]);
    }

    #[test]
    fn retain_drops_what_is_no_longer_shown_and_its_anchor() {
        let mut s = Selection::default();
        s.set(order());
        s.press(&order(), "p2", Press::Secondary); // anchor p2, deselected
        s.retain(|p| p != "p0" && p != "p2");
        assert_eq!(s.paths(), vec!["p1", "p3", "p4", "p5"]);
        s.press(&order(), "p0", Press::Primary { ctrl: false, shift: true });
        assert!(s.contains("p0") && s.len() == 5, "no stale anchor: shift adds just the one");
    }
}
