//! The command palette.
//!
//! Everything the player can do, findable by typing. The matching and ranking
//! are pure so they can be tested without laying out a frame; the widget below
//! them only draws.
//!
//! # Ranking is the whole feature
//!
//! A palette that returns the right command third is a palette people stop
//! using, because the value is pressing Enter without reading. So the order is
//! explicit and tested: an exact name, then a prefix, then a word boundary,
//! then a subsequence, and only then anything else. Ties break on the shorter
//! name — a short command matching as well as a long one is almost always the
//! one meant.

use std::time::Duration;

/// Something the palette can do. Data, not a closure, so the list can be
/// filtered, ranked and tested without any of the actions being callable.
#[derive(Clone, PartialEq, Debug)]
pub enum Action {
    TogglePlay,
    SeekTo(Duration),
    SeekBy(i64),
    SetSpeed(f64),
    SetVolume(f32),
    Rotate,
    CycleFit,
    ResetView,
    OpenFile,
    /// Play whatever is on the clipboard; the payload is what was recognised.
    PlayClipboard,
    /// Browse a channel or playlist found on the clipboard.
    BrowseClipboard,
    /// Ask for a prompt, then scrub to the sections that match it.
    ScrubToSections,
    ToggleSubtitles,
    OpenSettings,
    Quit,
}

#[derive(Clone, Debug)]
pub struct Command {
    /// What the user types and reads.
    pub name: &'static str,
    /// Extra words that should find it. "paste" finding "play from clipboard"
    /// matters more than any ranking subtlety.
    pub aliases: &'static [&'static str],
    pub hint: &'static str,
    pub action: Action,
}

/// The fixed set. Entries whose action is unavailable are filtered by the
/// caller, which knows what is loaded.
pub fn commands() -> Vec<Command> {
    vec![
        Command { name: "play / pause", aliases: &["toggle", "space"], hint: "or tap the video", action: Action::TogglePlay },
        Command { name: "open file", aliases: &["load", "browse"], hint: "a local mp4 or webm", action: Action::OpenFile },
        Command { name: "play from clipboard", aliases: &["paste", "url", "youtube", "link"], hint: "a URL or video id", action: Action::PlayClipboard },
        Command { name: "browse channel from clipboard", aliases: &["paste", "channel", "playlist"], hint: "list the uploader's videos", action: Action::BrowseClipboard },
        Command { name: "scrub to sections", aliases: &["find", "prompt", "skip", "search"], hint: "describe what you want to see", action: Action::ScrubToSections },
        Command { name: "subtitles", aliases: &["captions", "cc", "transcribe"], hint: "transcribe as it plays", action: Action::ToggleSubtitles },
        Command { name: "rotate", aliases: &["turn", "orientation"], hint: "90 degrees, or swipe the right edge", action: Action::Rotate },
        Command { name: "cycle fit", aliases: &["zoom", "fill", "letterbox", "bleed"], hint: "fit, fill, stretch", action: Action::CycleFit },
        Command { name: "reset view", aliases: &["unzoom", "restore"], hint: "clear rotation and zoom", action: Action::ResetView },
        Command { name: "speed 1x", aliases: &["normal"], hint: "", action: Action::SetSpeed(1.0) },
        Command { name: "speed 1.5x", aliases: &["faster"], hint: "", action: Action::SetSpeed(1.5) },
        Command { name: "speed 2x", aliases: &["double"], hint: "", action: Action::SetSpeed(2.0) },
        Command { name: "back 10s", aliases: &["rewind"], hint: "", action: Action::SeekBy(-10) },
        Command { name: "forward 30s", aliases: &["skip ahead"], hint: "", action: Action::SeekBy(30) },
        Command { name: "settings", aliases: &["options", "preferences", "config"], hint: "rotation, letterbox, gestures", action: Action::OpenSettings },
        Command { name: "quit", aliases: &["exit", "close"], hint: "", action: Action::Quit },
    ]
}

/// How well a query matched, best first.
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum MatchKind {
    /// Exact name.
    Exact,
    /// Name starts with the query.
    Prefix,
    /// A word in the name starts with the query.
    WordStart,
    /// An alias matched.
    Alias,
    /// Letters appear in order, not adjacent.
    Subsequence,
}

#[derive(Clone, Debug)]
pub struct Hit {
    pub command: Command,
    pub kind: MatchKind,
}

fn subsequence(needle: &str, hay: &str) -> bool {
    let mut it = hay.chars();
    needle.chars().all(|c| it.any(|h| h == c))
}

fn classify_one(query: &str, c: &Command) -> Option<MatchKind> {
    let name = c.name.to_lowercase();
    if name == query {
        return Some(MatchKind::Exact);
    }
    if name.starts_with(query) {
        return Some(MatchKind::Prefix);
    }
    if name.split_whitespace().any(|w| w.starts_with(query)) {
        return Some(MatchKind::WordStart);
    }
    if c.aliases
        .iter()
        .any(|a| a.starts_with(query) || *a == query)
    {
        return Some(MatchKind::Alias);
    }
    if subsequence(query, &name) {
        return Some(MatchKind::Subsequence);
    }
    None
}

/// Filter and rank. An empty query returns everything in declaration order,
/// which is the natural "what can this do" list.
pub fn search(query: &str, commands: &[Command]) -> Vec<Hit> {
    let q = query.trim().to_lowercase();
    if q.is_empty() {
        return commands
            .iter()
            .cloned()
            .map(|command| Hit {
                command,
                kind: MatchKind::Prefix,
            })
            .collect();
    }

    let mut hits: Vec<Hit> = commands
        .iter()
        .filter_map(|c| {
            classify_one(&q, c).map(|kind| Hit {
                command: c.clone(),
                kind,
            })
        })
        .collect();

    // Stable sort, so equal-ranked commands keep declaration order rather than
    // shuffling as the user types -- a list that reorders under the cursor is
    // how you press the wrong thing.
    hits.sort_by(|a, b| {
        a.kind
            .cmp(&b.kind)
            .then(a.command.name.len().cmp(&b.command.name.len()))
    });
    hits
}

/// Keep only commands that make sense right now.
///
/// A palette offering "rotate" with nothing loaded, or "browse channel" when
/// the clipboard holds a plain video, teaches people that half the entries do
/// nothing.
pub fn available(commands: Vec<Command>, has_video: bool, clipboard: &ClipboardState) -> Vec<Command> {
    commands
        .into_iter()
        .filter(|c| match c.action {
            Action::TogglePlay
            | Action::SeekTo(_)
            | Action::SeekBy(_)
            | Action::SetSpeed(_)
            | Action::Rotate
            | Action::CycleFit
            | Action::ResetView
            | Action::ScrubToSections
            | Action::ToggleSubtitles => has_video,
            Action::PlayClipboard => clipboard.playable,
            Action::BrowseClipboard => clipboard.browsable,
            _ => true,
        })
        .collect()
}

/// What the clipboard currently offers, so the palette does not have to know
/// how to read it.
#[derive(Copy, Clone, Default, Debug)]
pub struct ClipboardState {
    pub playable: bool,
    /// A channel or playlist, which can be listed rather than played.
    pub browsable: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(hits: &[Hit]) -> Vec<&str> {
        hits.iter().map(|h| h.command.name).collect()
    }

    #[test]
    fn an_empty_query_lists_everything_in_order() {
        let all = commands();
        let hits = search("", &all);
        assert_eq!(hits.len(), all.len());
        assert_eq!(hits[0].command.name, all[0].name);
    }

    /// The whole feature: the right command must be FIRST, because the point is
    /// pressing Enter without reading.
    #[test]
    fn the_obvious_query_puts_the_obvious_command_first() {
        for (query, want) in [
            ("quit", "quit"),
            ("settings", "settings"),
            ("rotate", "rotate"),
            ("open", "open file"),
            ("subtit", "subtitles"),
        ] {
            let hits = search(query, &commands());
            assert_eq!(
                hits.first().map(|h| h.command.name),
                Some(want),
                "{query:?} ranked {:?}",
                names(&hits)
            );
        }
    }

    /// "paste" finding "play from clipboard" matters more than any ranking
    /// subtlety: nobody types the name of that command.
    #[test]
    fn aliases_find_commands_nobody_types_the_name_of() {
        for (query, want) in [
            ("paste", "play from clipboard"),
            ("url", "play from clipboard"),
            ("cc", "subtitles"),
            ("letterbox", "cycle fit"),
            ("options", "settings"),
            ("find", "scrub to sections"),
        ] {
            let hits = search(query, &commands());
            assert!(
                names(&hits).contains(&want),
                "{query:?} did not find {want:?}: {:?}",
                names(&hits)
            );
        }
    }

    #[test]
    fn ranking_is_ordered_exact_prefix_wordstart_alias_subsequence() {
        assert!(MatchKind::Exact < MatchKind::Prefix);
        assert!(MatchKind::Prefix < MatchKind::WordStart);
        assert!(MatchKind::WordStart < MatchKind::Alias);
        assert!(MatchKind::Alias < MatchKind::Subsequence);
    }

    /// A word-boundary match must beat a subsequence one, or "fit" surfaces
    /// something that merely contains f, i and t in order.
    /// A word-boundary match must be CLASSIFIED as one, not merely end up
    /// first. "cycle fit" is the only thing matching "fit" either way, so the
    /// order alone cannot tell a word match from a scattered one -- asserting
    /// position here passes even with WordStart deleted.
    #[test]
    fn a_word_match_is_ranked_as_a_word_match() {
        let all = commands();
        let hits = search("fit", &all);
        assert_eq!(hits[0].command.name, "cycle fit", "{:?}", names(&hits));
        assert_eq!(
            hits[0].kind,
            MatchKind::WordStart,
            "a mid-name word match was demoted to {:?}",
            hits[0].kind
        );
    }

    /// Ties on kind break on the shorter name: a short command matching as
    /// well as a long one is almost always the one meant. Asserted by length,
    /// because several commands tie here and which short one wins is
    /// declaration order, not a claim worth pinning.
    #[test]
    fn equal_kinds_prefer_the_shorter_name() {
        let all = commands();
        let hits = search("s", &all);
        assert!(hits.len() > 3, "not enough matches to tie: {:?}", names(&hits));
        assert!(
            hits[0].command.name.len() <= 9,
            "a long command outranked the short ones: {:?}",
            names(&hits)
        );
        // ...and the list really is sorted by length within a kind.
        let first_kind = hits[0].kind;
        let lens: Vec<usize> = hits
            .iter()
            .filter(|h| h.kind == first_kind)
            .map(|h| h.command.name.len())
            .collect();
        let mut sorted = lens.clone();
        sorted.sort();
        assert_eq!(lens, sorted, "same-kind hits are not length ordered");
    }

    #[test]
    fn a_query_matching_nothing_returns_nothing() {
        assert!(search("zzzzqqq", &commands()).is_empty());
    }

    #[test]
    fn matching_ignores_case_and_surrounding_space() {
        assert_eq!(
            search("  QUIT ", &commands()).first().map(|h| h.command.name),
            Some("quit")
        );
    }

    /// Equal-ranked entries must not shuffle as the user types; a list that
    /// reorders under the cursor is how you press the wrong thing.
    #[test]
    fn equal_ranks_keep_a_stable_order() {
        let (c1, c2) = (commands(), commands());
        let (h1, h2) = (search("s", &c1), search("s", &c2));
        assert_eq!(names(&h1), names(&h2));
    }

    // ---- availability ----------------------------------------------------

    #[test]
    fn playback_commands_are_hidden_with_nothing_loaded() {
        let none = ClipboardState::default();
        let list = available(commands(), false, &none);
        let hits = search("", &list);
        let n = names(&hits);
        assert!(!n.contains(&"rotate"), "offered rotate with no video");
        assert!(!n.contains(&"subtitles"));
        // ...but the ones that always work stay.
        assert!(n.contains(&"open file"));
        assert!(n.contains(&"quit"));
        assert!(n.contains(&"settings"));
    }

    #[test]
    fn clipboard_commands_track_what_is_actually_on_it() {
        let plain = ClipboardState { playable: true, browsable: false };
        let list = available(commands(), true, &plain);
        let hits = search("", &list);
        let n = names(&hits);
        assert!(n.contains(&"play from clipboard"));
        assert!(
            !n.contains(&"browse channel from clipboard"),
            "offered browse for a plain video"
        );

        let chan = ClipboardState { playable: true, browsable: true };
        let list = available(commands(), true, &chan);
        let hits = search("", &list);
        let n = names(&hits);
        assert!(n.contains(&"browse channel from clipboard"));

        let empty = ClipboardState::default();
        let list = available(commands(), true, &empty);
        let hits = search("", &list);
        let n = names(&hits);
        assert!(!n.contains(&"play from clipboard"), "offered an empty clipboard");
    }

    /// Every command needs a name and a distinct action, or two entries do the
    /// same thing and one of them is dead.
    #[test]
    fn the_command_set_is_well_formed() {
        let all = commands();
        assert!(all.len() > 8);
        for c in &all {
            assert!(!c.name.is_empty());
            assert_eq!(c.name, c.name.to_lowercase(), "{} is not lowercase", c.name);
        }
        let mut seen: Vec<&Action> = Vec::new();
        for c in &all {
            assert!(
                !seen.contains(&&c.action),
                "{} duplicates an earlier action",
                c.name
            );
            seen.push(&c.action);
        }
    }
}

// ---------------------------------------------------------------------------
// the widget
// ---------------------------------------------------------------------------

/// Open/closed state plus what has been typed. The caller owns it, so the
/// palette can be opened from a key, a button, or a gesture without the widget
/// needing to know which.
#[derive(Default)]
pub struct PaletteState {
    pub open: bool,
    pub query: String,
    /// Index into the filtered list, not the full one — the highlight has to
    /// follow what is on screen as the query narrows it.
    pub selected: usize,
    /// The last text pasted into the window, which is how the clipboard is
    /// read without a clipboard dependency: egui delivers `Event::Paste` on
    /// Ctrl+V and this keeps the most recent one.
    pub last_paste: Option<String>,
}

impl PaletteState {
    pub fn open(&mut self) {
        self.open = true;
        self.query.clear();
        self.selected = 0;
    }

    pub fn close(&mut self) {
        self.open = false;
    }

    pub fn toggle(&mut self) {
        if self.open {
            self.close()
        } else {
            self.open()
        }
    }

    /// Move the highlight, wrapping. Wrapping matters more than it looks: the
    /// list is short and reversing direction to reach the last entry is a worse
    /// interaction than one more press of the same key.
    pub fn step(&mut self, delta: i32, len: usize) {
        if len == 0 {
            self.selected = 0;
            return;
        }
        let n = len as i32;
        self.selected = (((self.selected as i32 + delta) % n) + n) as usize % len;
    }
}

/// Draw the palette. Returns the action the user chose, if any.
///
/// A modal window rather than an inline panel: it is a mode, and a mode that
/// does not look like one gets typed into by accident.
pub fn show(
    ctx: &egui::Context,
    state: &mut PaletteState,
    has_video: bool,
    colors: &uikit_egui::PaletteColors,
) -> Option<Action> {
    if !state.open {
        return None;
    }

    // Capture pastes whenever the palette is open, so "play from clipboard"
    // has something to offer. egui hands these over as events; reading the
    // system clipboard directly would mean another dependency and, on Wayland,
    // a protocol this crate should not know about.
    ctx.input(|i| {
        for e in &i.events {
            if let egui::Event::Paste(text) = e {
                state.last_paste = Some(text.clone());
            }
        }
    });

    let clip = match &state.last_paste {
        Some(t) => {
            let p = vidkit_ytdlp::classify(t);
            ClipboardState {
                playable: p.is_playable(),
                browsable: matches!(
                    p,
                    vidkit_ytdlp::Paste::Channel { .. } | vidkit_ytdlp::Paste::Playlist { .. }
                ),
            }
        }
        None => ClipboardState::default(),
    };

    let list = available(commands(), has_video, &clip);
    let hits = search(&state.query, &list);
    if state.selected >= hits.len() {
        state.selected = 0;
    }

    let mut chosen = None;
    let mut close = false;

    // Keys are read before the window, so Escape closes even when the text
    // field has focus and would otherwise swallow it.
    ctx.input_mut(|i| {
        if i.consume_key(egui::Modifiers::NONE, egui::Key::Escape) {
            close = true;
        }
        if i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown) {
            state.step(1, hits.len());
        }
        if i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp) {
            state.step(-1, hits.len());
        }
        if i.consume_key(egui::Modifiers::NONE, egui::Key::Enter) {
            chosen = hits.get(state.selected).map(|h| h.command.action.clone());
            close = true;
        }
    });

    egui::Window::new("command palette")
        .collapsible(false)
        .resizable(false)
        .title_bar(false)
        .anchor(egui::Align2::CENTER_TOP, [0.0, 60.0])
        .default_width(420.0)
        .show(ctx, |ui| {
            let field = ui.add(
                egui::TextEdit::singleline(&mut state.query)
                    .hint_text("type a command, or paste a link")
                    .desired_width(f32::INFINITY),
            );
            // Focus once on open, not every frame: re-requesting it every frame
            // fights the arrow keys and makes the list unnavigable.
            if field.changed() {
                state.selected = 0;
            }
            ui.memory_mut(|m| {
                if m.focused().is_none() {
                    m.request_focus(field.id);
                }
            });

            ui.separator();
            if hits.is_empty() {
                ui.weak("nothing matches");
            }
            egui::ScrollArea::vertical().max_height(280.0).show(ui, |ui| {
                for (i, hit) in hits.iter().enumerate() {
                    let selected = i == state.selected;
                    let resp = ui.selectable_label(
                        selected,
                        format!("{}  {}", icon_for(&hit.command.action), hit.command.name),
                    );
                    if selected && !hit.command.hint.is_empty() {
                        ui.indent("hint", |ui| {
                            ui.colored_label(colors.text_weak, hit.command.hint);
                        });
                    }
                    if resp.clicked() {
                        chosen = Some(hit.command.action.clone());
                        close = true;
                    }
                }
            });
        });

    if close {
        state.close();
    }
    chosen
}

/// A glyph per action, so the list scans without reading.
///
/// These render as glyphs rather than boxes only because `install()` reaches
/// `uikit_egui::fonts::install`; switcheroo is the cautionary case.
pub fn icon_for(action: &Action) -> &'static str {
    match action {
        Action::TogglePlay => "▶",
        Action::SeekTo(_) | Action::SeekBy(_) => "⇥",
        Action::SetSpeed(_) => "⏩",
        Action::SetVolume(_) => "🔊",
        Action::Rotate => "↻",
        Action::CycleFit | Action::ResetView => "⛶",
        Action::OpenFile => "📂",
        Action::PlayClipboard | Action::BrowseClipboard => "🔗",
        Action::ScrubToSections => "🔍",
        Action::ToggleSubtitles => "💬",
        Action::OpenSettings => "⚙",
        Action::Quit => "✕",
    }
}

#[cfg(test)]
mod widget_tests {
    use super::*;

    #[test]
    fn stepping_wraps_in_both_directions() {
        let mut s = PaletteState::default();
        s.step(-1, 3);
        assert_eq!(s.selected, 2, "up from the top should wrap to the bottom");
        s.step(1, 3);
        assert_eq!(s.selected, 0, "down from the bottom should wrap to the top");
    }

    /// An empty list must not panic or leave a stale index behind.
    #[test]
    fn stepping_an_empty_list_is_safe() {
        let mut s = PaletteState {
            selected: 5,
            ..Default::default()
        };
        s.step(1, 0);
        assert_eq!(s.selected, 0);
    }

    #[test]
    fn opening_clears_the_previous_query() {
        let mut s = PaletteState {
            query: "old".into(),
            selected: 4,
            ..Default::default()
        };
        s.open();
        assert!(s.open && s.query.is_empty() && s.selected == 0);
        s.toggle();
        assert!(!s.open);
    }

    /// Every action needs a glyph, or the list has a ragged left edge where
    /// one entry is missing its icon.
    #[test]
    fn every_action_has_an_icon() {
        let all = commands();
        for c in &all {
            assert!(!icon_for(&c.action).is_empty(), "{} has no icon", c.name);
        }
    }
}
