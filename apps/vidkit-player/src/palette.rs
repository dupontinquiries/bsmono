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
    SwFull,
    SwBalanced,
    SwSaver,
    SwMinimum,
    SwFollowPower,
    TranscribeQuick,
    TranscribeBetter,
    TranscribeBest,
    LangEnglish,
    LangDetectOnce,
    LangEveryChunk,
    Help,
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
        Command::SwFull,
        Command::SwBalanced,
        Command::SwSaver,
        Command::SwMinimum,
        Command::SwFollowPower,
        Command::TranscribeQuick,
        Command::TranscribeBetter,
        Command::TranscribeBest,
        Command::LangEnglish,
        Command::LangDetectOnce,
        Command::LangEveryChunk,
        Command::Help,
        Command::ScrubToSections,
        Command::PlayClipboard,
        Command::Quit,
    ];

    /// What the command does, in words, for the right-hand column. A
    /// newcomer reads this, not the label, to decide.
    pub fn hint(self) -> &'static str {
        match self {
            Command::OpenFile => "pick a video file",
            Command::PlayPause => "play or pause",
            Command::Rotate => "turn the picture a quarter",
            Command::CycleFit => "fit, fill, or stretch",
            Command::ResetView => "undo zoom and pan",
            Command::SpeedUp => "play faster",
            Command::SpeedDown => "play slower",
            Command::VolumeUp => "louder",
            Command::VolumeDown => "quieter",
            Command::Settings => "all the options",
            Command::Subtitles => "show what is being said",
            Command::ListenOnly => "sound only; saves battery",
            Command::SwFull | Command::SwBalanced | Command::SwSaver | Command::SwMinimum => "software rendering: detail vs battery",
            Command::SwFollowPower => "let the battery decide",
            Command::TranscribeQuick => "fast; whole video; the default",
            Command::TranscribeBetter => "slower; catches more words",
            Command::TranscribeBest => "slowest; for when base misses",
            Command::LangEnglish => "English videos: fastest",
            Command::LangDetectOnce => "one other language",
            Command::LangEveryChunk => "videos that switch language",
            Command::Help => "watch, find, budget, ? -- examples",
            Command::ScrubToSections => "chapters and search",
            Command::PlayClipboard => "play the link you copied",
            Command::Quit => "close the player",
        }
    }

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
            Command::SwFull => "Software quality: full",
            Command::SwBalanced => "Software quality: balanced (½ budget)",
            Command::SwSaver => "Software quality: battery saver (¼)",
            Command::SwMinimum => "Software quality: minimum (1/16)",
            Command::SwFollowPower => "Software quality: follow power",
            Command::TranscribeQuick => "Transcription model: quick (tiny)",
            Command::TranscribeBetter => "Transcription model: better (base)",
            Command::TranscribeBest => "Transcription model: best (small)",
            Command::LangEnglish => "Transcription language: English",
            Command::LangDetectOnce => "Transcription language: detect once",
            Command::LangEveryChunk => "Transcription language: every chunk (multilingual)",
            Command::Help => "What can I type here?",
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
            Command::SwFull | Command::SwBalanced | Command::SwSaver | Command::SwMinimum | Command::SwFollowPower => {
                "budget damage energy battery quality lod blur cpu software"
            }
            Command::TranscribeQuick | Command::TranscribeBetter | Command::TranscribeBest => "whisper model transcript transcription accuracy tiny base small better",
            Command::LangEnglish | Command::LangDetectOnce | Command::LangEveryChunk => "language languages multilingual whisper english detect transcript",
            Command::Help => "help examples how type syntax commands ?",
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
/// What a typed query asks for, beyond picking a command. Pure.
#[derive(Clone, Debug, PartialEq)]
pub enum Typed {
    /// `watch goes down`: play only the moments that mention it.
    Watch(String),
    /// `find goes down`: jump to a moment that mentions it.
    Find(String),
    /// `budget 30%`.
    Budget(f32),
}

pub fn parse_typed(query: &str) -> Option<Typed> {
    let q = query.trim_start();
    let lower = q.to_ascii_lowercase();
    for (k, mk) in [("watch ", Typed::Watch as fn(String) -> Typed), ("find ", Typed::Find), ("search ", Typed::Find)] {
        if lower.starts_with(k) {
            let rest = q[k.len()..].trim();
            // Quotes are fine and not needed: watch "goes down" == watch goes down.
            let rest = rest.trim_matches(|c| c == '"' || c == '\u{201c}' || c == '\u{201d}').trim();
            return (!rest.is_empty()).then(|| mk(rest.to_string()));
        }
    }
    parse_budget(q).map(Typed::Budget)
}

/// Worked examples for "What can I type?": (what to type, what it does).
/// Choosing one puts it in the box to edit rather than running it.
pub const EXAMPLES: [(&str, &str); 6] = [
    ("watch goes down", "play only the moments that say it, back to back"),
    ("find goes down", "list the moments that say it; pick one to jump"),
    ("?goes down", "search every video you have watched"),
    ("budget 30%", "software rendering: convert 30% of a frame"),
    ("listen", "any word finds commands: listen, quality, language"),
    ("rotate", "commands run straight away when picked"),
];

/// A typed budget: "budget 30", "budget 30%", "budget 0.3", "sw 25%",
/// "quality 0.5". Percent when > 1 or with %, a fraction otherwise; clamped
/// to 1..100%. Pure.
pub fn parse_budget(query: &str) -> Option<f32> {
    let q = query.trim().to_ascii_lowercase();
    let rest = ["budget", "sw", "quality", "damage"].iter().find_map(|k| q.strip_prefix(k))?;
    let rest = rest.trim().trim_start_matches(['=', ':']).trim();
    let (num, pct) = match rest.strip_suffix('%') {
        Some(n) => (n.trim(), true),
        None => (rest, false),
    };
    let v: f32 = num.parse().ok().filter(|v: &f32| v.is_finite() && *v > 0.0)?;
    let frac = if pct || v > 1.0 { v / 100.0 } else { v };
    Some(frac.clamp(0.01, 1.0))
}

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
            Command::SwFull => A::SetSwBudget(1.0),
            Command::SwBalanced => A::SetSwBudget(0.5),
            Command::SwSaver => A::SetSwBudget(0.25),
            Command::SwMinimum => A::SetSwBudget(1.0 / 16.0),
            Command::SwFollowPower => A::SetSwBudget(f32::NAN),
            Command::TranscribeQuick => A::TranscribeModel(0),
            Command::TranscribeBetter => A::TranscribeModel(1),
            Command::TranscribeBest => A::TranscribeModel(2),
            Command::LangEnglish => A::TranscribeLanguage(0),
            Command::LangDetectOnce => A::TranscribeLanguage(1),
            Command::LangEveryChunk => A::TranscribeLanguage(2),
            Command::Help => A::PaletteHelp,
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
            A::ToggleSubtitles, A::ToggleListenOnly, A::SetSwBudget(1.0), A::TranscribeModel(0), A::TranscribeLanguage(0), A::PaletteHelp, A::ScrubToSections, A::PlayClipboard, A::Quit,
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

#[cfg(test)]
mod budget_tests {
    use super::*;

    #[test]
    fn typed_budgets_parse_as_percent_or_fraction() {
        assert_eq!(parse_budget("budget 30"), Some(0.3));
        assert_eq!(parse_budget("budget 30%"), Some(0.3));
        assert_eq!(parse_budget("Budget 0.3"), Some(0.3));
        assert_eq!(parse_budget("sw 25%"), Some(0.25));
        assert_eq!(parse_budget("quality: 1"), Some(1.0), "1 alone is the whole frame");
        assert_eq!(parse_budget("budget 250"), Some(1.0), "clamped");
        assert_eq!(parse_budget("budget 0.001"), Some(0.01));
        assert_eq!(parse_budget("budget"), None, "nothing typed yet");
        assert_eq!(parse_budget("budget -5"), None);
        assert_eq!(parse_budget("budget abc"), None);
        assert_eq!(parse_budget("rotate"), None);
    }

    #[test]
    fn the_tiers_are_found_by_what_people_type() {
        for q in ["battery", "quality", "budget", "saver"] {
            assert!(rank(q).iter().any(|c| matches!(c, Command::SwSaver)), "{q} did not find the saver tier");
        }
    }
}

#[cfg(test)]
mod typed_tests {
    use super::*;

    #[test]
    fn typed_commands_say_what_they_are() {
        assert_eq!(parse_typed("watch goes down"), Some(Typed::Watch("goes down".into())));
        assert_eq!(parse_typed("Watch \"goes down\""), Some(Typed::Watch("goes down".into())), "quotes optional");
        assert_eq!(parse_typed("find laplace"), Some(Typed::Find("laplace".into())));
        assert_eq!(parse_typed("search laplace"), Some(Typed::Find("laplace".into())));
        assert_eq!(parse_typed("budget 30%"), Some(Typed::Budget(0.3)));
        assert_eq!(parse_typed("watch "), None, "nothing to watch for yet");
        assert_eq!(parse_typed("watchlist"), None, "a word, not the command");
        assert_eq!(parse_typed("rotate"), None);
    }

    #[test]
    fn every_command_explains_itself() {
        for c in Command::ALL {
            assert!(!c.hint().is_empty(), "{c:?}");
            assert!(c.hint().len() <= 40, "keep it glanceable: {c:?} {}", c.hint());
        }
        for (typed, _) in EXAMPLES.iter().filter(|(t, _)| t.starts_with("watch") || t.starts_with("find") || t.starts_with("budget")) {
            assert!(parse_typed(typed).is_some(), "an example that does not parse: {typed}");
        }
    }
}
}
