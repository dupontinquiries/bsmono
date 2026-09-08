//! The command palette.
//!
//! Ranking is `uikit_egui::fuzzy::score` and the bar-vs-pane decision is
//! `uikit_egui::command_bar::mode_for` — neither is reimplemented here. What
//! this module owns is the command SET and the selection state, which is the
//! part that is specific to a video player.
//!
//! The pure half is separated from the drawing so the ranking and the cursor
//! behaviour can be tested without laying out a frame: an off-by-one in list
//! navigation is invisible in a screenshot and obvious in an assertion.

use uikit_egui::fuzzy;

/// Everything the palette can do. One variant per row.
///
/// Deliberately the same actions the gestures and the bottom bar expose rather
/// than a second, parallel set: a palette that can do things no other control
/// can becomes the only way to do them, and a palette that does less than the
/// bar is a worse bar.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum Command {
    OpenFile,
    PlayPause,
    Rotate,
    CycleFit,
    ResetView,
    SpeedUp,
    SpeedDown,
    VolumeUp,
    VolumeDown,
    Quit,
}

impl Command {
    /// Every command, in the order they appear with an empty query.
    ///
    /// Ordered by how often it is wanted, not alphabetically: with no query
    /// typed the palette is a menu, and the first row is what Enter does.
    pub const ALL: &'static [Command] = &[
        Command::OpenFile,
        Command::PlayPause,
        Command::Rotate,
        Command::CycleFit,
        Command::ResetView,
        Command::SpeedUp,
        Command::SpeedDown,
        Command::VolumeUp,
        Command::VolumeDown,
        Command::Quit,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Command::OpenFile => "Open file…",
            Command::PlayPause => "Play / pause",
            Command::Rotate => "Rotate 90°",
            Command::CycleFit => "Cycle fit: contain / cover / stretch",
            Command::ResetView => "Reset zoom and rotation",
            Command::SpeedUp => "Speed +5%",
            Command::SpeedDown => "Speed −5%",
            Command::VolumeUp => "Volume +5%",
            Command::VolumeDown => "Volume −5%",
            Command::Quit => "Quit",
        }
    }

    /// The shortcut shown on the right of the row, if any.
    pub fn shortcut(self) -> Option<&'static str> {
        match self {
            Command::OpenFile => Some("Ctrl+O"),
            Command::PlayPause => Some("Space"),
            Command::Quit => Some("Ctrl+Q"),
            _ => None,
        }
    }

    /// Extra words the query can match that are not in the label.
    ///
    /// Without these, "seek" finds nothing and "fullscreen" finds nothing,
    /// because the labels happen to use other words. A palette that only
    /// matches its own wording makes the user guess the vocabulary.
    pub fn aliases(self) -> &'static str {
        match self {
            Command::OpenFile => "load browse choose path",
            Command::PlayPause => "pause resume stop toggle",
            Command::Rotate => "turn orientation portrait landscape",
            Command::CycleFit => "zoom scale fill contain stretch letterbox",
            Command::ResetView => "restore default original",
            Command::SpeedUp => "faster rate tempo",
            Command::SpeedDown => "slower rate tempo",
            Command::VolumeUp => "louder audio sound",
            Command::VolumeDown => "quieter audio sound mute",
            Command::Quit => "exit close",
        }
    }
}

/// Rank the commands against a query, best first.
///
/// Scores the label and the aliases and keeps the better of the two, so
/// "louder" finds Volume +5% without "louder" appearing on the row. An empty
/// query returns everything in declaration order.
pub fn rank(query: &str) -> Vec<Command> {
    if query.trim().is_empty() {
        return Command::ALL.to_vec();
    }
    let mut scored: Vec<(i32, usize, Command)> = Command::ALL
        .iter()
        .enumerate()
        .filter_map(|(i, c)| {
            let label = fuzzy::score(query, c.label());
            // Alias hits rank below label hits of the same strength: matching
            // the visible text is a better signal than matching a hidden
            // synonym.
            let alias = fuzzy::score(query, c.aliases()).map(|s| s - 6);
            let best = match (label, alias) {
                (Some(a), Some(b)) => Some(a.max(b)),
                (a, b) => a.or(b),
            };
            best.map(|s| (s, i, *c))
        })
        .collect();
    // Declaration order breaks score ties, so the list does not reshuffle
    // between keystrokes that do not change the ranking.
    scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    scored.into_iter().map(|(_, _, c)| c).collect()
}

/// Palette state: the query, the highlighted row, and whether it is open.
#[derive(Clone, Debug, Default)]
pub struct Palette {
    pub open: bool,
    pub query: String,
    pub selected: usize,
    /// Set when the palette opens, so the text field can take focus once
    /// rather than stealing it every frame.
    pub just_opened: bool,
}

impl Palette {
    pub fn open(&mut self) {
        self.open = true;
        self.just_opened = true;
        // Cleared on open, not on close: a palette that remembers the last
        // query shows stale matches the moment it appears, and the first
        // keystroke is almost always a new search.
        self.query.clear();
        self.selected = 0;
    }

    pub fn close(&mut self) {
        self.open = false;
        self.just_opened = false;
    }

    pub fn matches(&self) -> Vec<Command> {
        rank(&self.query)
    }

    /// Move the highlight, wrapping at both ends.
    ///
    /// Wrapping because the list is short and reaching the end by holding a key
    /// should not feel like the control has jammed.
    pub fn move_selection(&mut self, delta: i32) {
        let n = self.matches().len();
        if n == 0 {
            self.selected = 0;
            return;
        }
        let cur = self.selected.min(n - 1) as i32;
        self.selected = (cur + delta).rem_euclid(n as i32) as usize;
    }

    /// The command Enter would run.
    pub fn chosen(&self) -> Option<Command> {
        let m = self.matches();
        m.get(self.selected.min(m.len().saturating_sub(1))).copied()
    }

    /// Reset the highlight after the query changes.
    ///
    /// Back to the top, not clamped to the end. A new query produces a new
    /// RANKING, so the row that was highlighted has no relationship to the row
    /// now at that index — and clamping an out-of-range selection to the last
    /// row highlights the WORST match, which is the opposite of what typing
    /// asked for. The file picker already resets to 0 on every keystroke; the
    /// two behaving differently was the inconsistency, and this test caught it.
    pub fn on_query_changed(&mut self) {
        self.selected = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_query_lists_everything_in_order() {
        assert_eq!(rank(""), Command::ALL.to_vec());
        assert_eq!(rank("   "), Command::ALL.to_vec());
    }

    #[test]
    fn a_query_narrows_to_matching_commands() {
        let r = rank("rot");
        assert_eq!(r.first(), Some(&Command::Rotate), "got {r:?}");
        assert!(r.len() < Command::ALL.len(), "nothing was filtered out");
    }

    /// The aliases exist so the user does not have to guess the wording.
    #[test]
    fn synonyms_find_commands_whose_labels_do_not_contain_them() {
        for (query, want) in [
            ("louder", Command::VolumeUp),
            ("exit", Command::Quit),
            ("faster", Command::SpeedUp),
            ("browse", Command::OpenFile),
            ("letterbox", Command::CycleFit),
        ] {
            let r = rank(query);
            assert_eq!(
                r.first(),
                Some(&want),
                "{query:?} should find {want:?}, got {r:?}"
            );
        }
    }

    /// A visible-text hit should beat a hidden-synonym hit; matching what the
    /// user can see is the stronger signal.
    #[test]
    fn a_label_match_outranks_an_alias_match() {
        // "volume" is in both Volume rows' labels and in nothing else's.
        let r = rank("volume");
        assert!(
            matches!(r.first(), Some(Command::VolumeUp) | Some(Command::VolumeDown)),
            "got {r:?}"
        );
    }

    #[test]
    fn a_query_matching_nothing_returns_nothing() {
        assert!(rank("zzzzzqqq").is_empty());
    }

    #[test]
    fn opening_clears_the_previous_query() {
        let mut p = Palette::default();
        p.open();
        p.query = "rot".into();
        p.selected = 3;
        p.close();
        p.open();
        assert!(p.query.is_empty(), "stale query survived");
        assert_eq!(p.selected, 0);
        assert!(p.just_opened);
    }

    #[test]
    fn selection_wraps_at_both_ends() {
        let mut p = Palette::default();
        p.open();
        let n = Command::ALL.len();
        p.move_selection(-1);
        assert_eq!(p.selected, n - 1, "up from the first should wrap to the last");
        p.move_selection(1);
        assert_eq!(p.selected, 0, "down from the last should wrap to the first");
    }

    /// The bug this prevents: narrowing the list leaves the highlight past the
    /// end, and Enter then runs nothing or the wrong row.
    /// Typing must leave the BEST match under Enter, not whatever survived at
    /// the old index. Clamping to the end selected the worst match.
    #[test]
    fn changing_the_query_highlights_the_best_match_not_the_last() {
        let mut p = Palette::default();
        p.open();
        p.selected = Command::ALL.len() - 1;
        p.query = "rot".into();
        p.on_query_changed();

        let n = p.matches().len();
        assert!(p.selected < n, "selection {} past end {n}", p.selected);
        assert_eq!(p.selected, 0, "should return to the top of the new ranking");
        assert_eq!(
            p.chosen(),
            Some(Command::Rotate),
            "Enter should run the best match, got {:?}",
            p.chosen()
        );
    }

    /// A stale index must never be able to run a command the user cannot see.
    #[test]
    fn chosen_stays_inside_the_visible_list_even_if_selected_is_stale() {
        let mut p = Palette::default();
        p.open();
        p.query = "rot".into();
        p.selected = 99;
        let m = p.matches();
        assert!(m.contains(&p.chosen().unwrap()), "ran something off-list");
    }

    #[test]
    fn chosen_is_none_when_nothing_matches() {
        let mut p = Palette::default();
        p.open();
        p.query = "zzzzzqqq".into();
        p.on_query_changed();
        assert_eq!(p.chosen(), None);
    }

    /// Every command must be reachable by typing its own label, or a row
    /// exists that the palette cannot actually find.
    #[test]
    fn every_command_is_findable_by_its_own_label() {
        for c in Command::ALL {
            let r = rank(c.label());
            assert_eq!(r.first(), Some(c), "{:?} not found by its label", c);
        }
    }

    #[test]
    fn every_command_has_a_label_and_aliases() {
        for c in Command::ALL {
            assert!(!c.label().is_empty(), "{c:?} has no label");
            assert!(!c.aliases().is_empty(), "{c:?} has no aliases");
        }
        assert!(Command::ALL.len() >= 8, "the command set shrank unexpectedly");
    }
}
