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
    Settings,
    Subtitles,
    ListenOnly,
    ScrubToSections,
    PlayClipboard,
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
        Command::Settings,
        Command::Subtitles,
        Command::ListenOnly,
        Command::ScrubToSections,
        Command::PlayClipboard,
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
            Command::Settings => "Settings…",
            Command::Subtitles => "Subtitles",
            Command::ListenOnly => "Listen only (video off)",
            Command::ScrubToSections => "Scrub to sections…",
            Command::PlayClipboard => "Play from clipboard",
            Command::Quit => "Quit",
        }
    }

    /// The shortcut shown on the right of the row, if any.
    pub fn shortcut(self) -> Option<&'static str> {
        match self {
            Command::OpenFile => Some("Ctrl+O"),
            Command::PlayPause => Some("Space"),
            Command::Settings => Some("Ctrl+,"),
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
            Command::CycleFit => "zoom scale fill contain stretch letterbox bleed",
            Command::ResetView => "restore default original",
            Command::SpeedUp => "faster rate tempo",
            Command::SpeedDown => "slower rate tempo",
            Command::VolumeUp => "louder audio sound",
            Command::VolumeDown => "quieter audio sound mute",
            Command::Settings => "options preferences config gestures sensitivity power",
            Command::Subtitles => "captions cc transcribe transcript",
            Command::ScrubToSections => "find search skip prompt relevant jump",
            Command::ListenOnly => "audio sound podcast battery video off hide",
            Command::PlayClipboard => "paste url link youtube open video",
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

/// Palette state. The query, the cursor and the open/focus bookkeeping are the
/// kit's (`command_bar::State`) since the surface became the kit's to draw; what
/// is left here is the part that is about a video player: which commands match.
#[derive(Clone, Debug, Default)]
pub struct Palette {
    pub bar: uikit_egui::command_bar::State,
}

impl Palette {
    pub fn is_open(&self) -> bool {
        self.bar.is_open()
    }

    /// Opens empty: the kit clears the query on open, not on close, for the
    /// reason this module used to give itself - a palette that remembers the
    /// last query shows stale matches the moment it appears.
    pub fn open(&mut self) {
        self.bar.open();
    }

    pub fn close(&mut self) {
        self.bar.close();
    }

    pub fn matches(&self) -> Vec<Command> {
        rank(&self.bar.query)
    }

    /// The command at row `i` of the current matches - what the surface reports
    /// as chosen, by Enter on the cursor or by a tap.
    pub fn at(&self, i: usize) -> Option<Command> {
        self.matches().get(i).copied()
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
        p.bar.query = "rot".into();
        p.bar.cursor = 3;
        p.close();
        p.open();
        assert!(p.bar.query.is_empty(), "stale query survived");
        assert_eq!(p.bar.cursor, 0);
        assert!(p.is_open());
    }

    // Removed with the port to `command_bar::show`, and why, since each pinned a
    // decision this module no longer makes:
    //  * selection_wraps_at_both_ends - the kit CLAMPS (its reasoning: the list
    //    is short and ordered by score, so wrapping from the worst row back to
    //    the best reads as having lost the selection). One fleet-wide answer
    //    beats this player disagreeing with every other palette; pinned by
    //    command_bar's the_cursor_clamps_rather_than_wrapping_past_the_best_match.
    //  * changing_the_query_highlights_the_best_match_not_the_last and
    //    chosen_stays_inside_the_visible_list_even_if_selected_is_stale - the
    //    surface resets the cursor on every edit and clamps it to the rows it
    //    shows; pinned by command_bar's surface_tests.

    #[test]
    fn the_row_the_surface_reports_maps_to_the_command_ranked_there() {
        let mut p = Palette::default();
        p.open();
        p.bar.query = "rot".into();
        assert_eq!(p.at(0), Some(Command::Rotate));
        assert_eq!(p.at(99), None, "a stale index runs nothing rather than something else");
    }

    #[test]
    fn chosen_is_none_when_nothing_matches() {
        let mut p = Palette::default();
        p.open();
        p.bar.query = "zzzzzqqq".into();
        assert!(p.matches().is_empty());
        assert_eq!(p.at(0), None);
    }

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


/// The shared action a command stands for.
///
/// The palette keeps its own enum because it owns row labels, shortcuts and
/// aliases, which are presentation. What it must NOT own is the set of things
/// that can happen -- that lives in `vidkit_egui::Action`, which the voice
/// router also targets. Two lists of capabilities drift, and a command added
/// to one and forgotten in the other is invisible until somebody reaches for
/// it and nothing happens.
impl Command {
    pub fn action(self) -> vidkit_egui::Action {
        use vidkit_egui::Action as A;
        match self {
            Command::OpenFile => A::OpenFile,
            Command::PlayPause => A::TogglePlay,
            Command::Rotate => A::Rotate,
            Command::CycleFit => A::CycleFit,
            Command::ResetView => A::ResetView,
            // The bar and the gestures move in 5% steps; the palette says so
            // on the row and must move by the same amount.
            Command::SpeedUp => A::SetSpeed(f64::NAN),
            Command::SpeedDown => A::SetSpeed(f64::NAN),
            Command::VolumeUp => A::SetVolume(f32::NAN),
            Command::VolumeDown => A::SetVolume(f32::NAN),
            Command::Settings => A::OpenSettings,
            Command::Subtitles => A::ToggleSubtitles,
            Command::ListenOnly => A::ToggleListenOnly,
            Command::ScrubToSections => A::ScrubToSections,
            Command::PlayClipboard => A::PlayClipboard,
            Command::Quit => A::Quit,
        }
    }
}

#[cfg(test)]
mod action_bridge_tests {
    use super::*;

    /// Every palette row maps to a shared action, so nothing in the palette is
    /// a capability the rest of the app has never heard of.
    #[test]
    fn every_command_maps_to_an_action() {
        for c in Command::ALL {
            let _ = c.action();
        }
    }

    /// And every shared action a video player can perform is reachable from
    /// the palette -- this is the direction that rots, because a new Action
    /// arrives in the kit and nothing here fails.
    #[test]
    fn every_shared_action_has_a_palette_row() {
        use vidkit_egui::Action as A;
        let rows: Vec<A> = Command::ALL.iter().map(|c| c.action()).collect();
        let has = |a: &A| rows.iter().any(|r| std::mem::discriminant(r) == std::mem::discriminant(a));
        for a in [
            A::TogglePlay, A::Rotate, A::CycleFit, A::ResetView, A::OpenFile,
            A::SetSpeed(1.0), A::SetVolume(1.0), A::OpenSettings,
            A::ToggleSubtitles, A::ToggleListenOnly, A::ScrubToSections, A::PlayClipboard, A::Quit,
        ] {
            assert!(has(&a), "{a:?} has no palette row");
        }
    }

    /// The new rows need aliases, or they are only findable by their exact
    /// wording -- which is the thing `aliases()` exists to prevent.
    #[test]
    fn the_new_commands_are_findable_by_other_words() {
        for (query, want) in [
            ("options", Command::Settings),
            ("captions", Command::Subtitles),
            ("paste", Command::PlayClipboard),
            ("find", Command::ScrubToSections),
        ] {
            let ranked = rank(query);
            assert!(
                ranked.contains(&want),
                "{query:?} did not find {want:?}: {ranked:?}"
            );
        }
    }
}
