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
    /// Sound only: the video decoder stops until it is turned back on.
    ToggleListenOnly,
    /// Software presenter conversion budget, 0..1 of a frame
    /// (vidkit_core::damage); NaN = follow the power policy.
    SetSwBudget(f32),
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
        Command { name: "software quality: full", aliases: &["budget", "damage", "lossless"], hint: "convert every changed tile at full detail", action: Action::SetSwBudget(1.0) },
        Command { name: "software quality: battery saver", aliases: &["budget", "damage", "save", "energy"], hint: "a quarter of a frame's conversions; motion goes soft", action: Action::SetSwBudget(0.25) },
        Command { name: "listen only", aliases: &["audio only", "sound only", "video off", "battery", "podcast"], hint: "stop decoding video; the sound carries on", action: Action::ToggleListenOnly },
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

/// What the clipboard currently offers, so callers do not each have to know
/// how to read it.
#[derive(Copy, Clone, Default, Debug)]
pub struct ClipboardState {
    pub playable: bool,
    /// A channel or playlist, which can be listed rather than played.
    pub browsable: bool,
}

/// Keep only the commands that make sense right now.
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
            | Action::SetVolume(_)
            | Action::Rotate
            | Action::CycleFit
            | Action::ResetView
            | Action::ScrubToSections
            | Action::ToggleSubtitles
            | Action::ToggleListenOnly
            | Action::SetSwBudget(_) => has_video,
            Action::PlayClipboard => clipboard.playable,
            Action::BrowseClipboard => clipboard.browsable,
            _ => true,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(list: &[Command]) -> Vec<&str> {
        list.iter().map(|c| c.name).collect()
    }










    // ---- availability ----------------------------------------------------

    #[test]
    fn playback_commands_are_hidden_with_nothing_loaded() {
        let none = ClipboardState::default();
        let list = available(commands(), false, &none);
        let n = names(&list);
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
        let n = names(&list);
        assert!(n.contains(&"play from clipboard"));
        assert!(
            !n.contains(&"browse channel from clipboard"),
            "offered browse for a plain video"
        );

        let chan = ClipboardState { playable: true, browsable: true };
        let list = available(commands(), true, &chan);
        let n = names(&list);
        assert!(n.contains(&"browse channel from clipboard"));

        let empty = ClipboardState::default();
        let list = available(commands(), true, &empty);
        let n = names(&list);
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
        Action::ToggleListenOnly => "🎧",
        Action::SetSwBudget(_) => "▦",
        Action::OpenSettings => "⚙",
        Action::Quit => "✕",
    }
}

#[cfg(test)]
mod icon_tests {
    use super::*;

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
