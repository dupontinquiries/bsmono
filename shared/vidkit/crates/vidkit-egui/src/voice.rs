//! Voice control routing.
//!
//! No new matcher. `shared/voiced-core`'s `grammar` already does the work
//! voiceterm needed — clean the transcription with voice-kit, gate out
//! hallucinations, try system phrases, then the focused app's rules, where a
//! rule is leading literal words plus an optional trailing `{slot}`. This
//! module is the two pieces that are vidkit's own: the rules to register, and
//! the map from a matched event back to a [`crate::Action`].
//!
//! # There is no transcriber here, deliberately
//!
//! `voiced-core` abstracts the mic and whisper behind `Transcriber` for the
//! same reason `route_ai` has no worker thread: standing one up needs a model
//! and a device this crate cannot supply or test. So everything below takes
//! text and returns an action, and the whole layer is testable by feeding it
//! the strings whisper would have produced.
//!
//! # Why the actions are the palette's
//!
//! Voice and the palette want the same set of things done, and two lists would
//! drift — a command added to one and forgotten in the other is invisible until
//! someone says it out loud and nothing happens. Routing to [`crate::Action`]
//! means the palette's entries and the voice grammar cannot disagree about what
//! exists, and `every_action_is_reachable_by_voice` fails when a new action
//! arrives without a phrase.

use std::time::Duration;

use crate::palette::Action;

/// The app name vidkit registers its grammar under.
pub const APP: &str = "vidkit";

/// Event names, kept as constants because they are written twice — once when
/// registering the rule and once when routing the match — and a typo in either
/// is a phrase that is heard and silently does nothing.
mod ev {
    pub const PLAY: &str = "play";
    pub const PAUSE: &str = "pause";
    pub const ROTATE: &str = "rotate";
    pub const FIT: &str = "cycle_fit";
    pub const RESET: &str = "reset_view";
    pub const SUBTITLES: &str = "subtitles";
    pub const LISTEN: &str = "listen_only";
    pub const QUALITY_FULL: &str = "quality_full";
    pub const SAVER: &str = "battery_saver";
    pub const SETTINGS: &str = "settings";
    pub const QUIT: &str = "quit";
    pub const BACK: &str = "back";
    pub const FORWARD: &str = "forward";
    pub const SPEED: &str = "speed";
    pub const VOLUME: &str = "volume";
    pub const FIND: &str = "find";
    pub const OPEN_CLIP: &str = "open_clipboard";
}

/// Phrases, and the event each fires. Several spellings per event on purpose:
/// people do not say the command name, they say what they want.
pub fn phrases() -> Vec<(&'static str, &'static str)> {
    vec![
        (ev::PLAY, "play"),
        (ev::PLAY, "resume"),
        (ev::PAUSE, "pause"),
        (ev::PAUSE, "stop"),
        (ev::ROTATE, "rotate"),
        (ev::ROTATE, "turn it"),
        (ev::FIT, "fill the screen"),
        (ev::FIT, "change the fit"),
        (ev::RESET, "reset the view"),
        (ev::SUBTITLES, "subtitles"),
        (ev::LISTEN, "listen only"),
        (ev::LISTEN, "audio only"),
        (ev::LISTEN, "turn off the video"),
        (ev::QUALITY_FULL, "full quality"),
        (ev::SAVER, "battery saver"),
        (ev::SAVER, "save battery"),
        (ev::SUBTITLES, "captions"),
        (ev::SETTINGS, "settings"),
        (ev::QUIT, "quit"),
        (ev::BACK, "go back {amount}"),
        (ev::BACK, "back {amount}"),
        (ev::FORWARD, "skip ahead {amount}"),
        (ev::FORWARD, "forward {amount}"),
        (ev::SPEED, "speed {value}"),
        (ev::SPEED, "play at {value}"),
        (ev::VOLUME, "volume {value}"),
        (ev::FIND, "find {prompt}"),
        (ev::FIND, "skip to {prompt}"),
        (ev::FIND, "show me {prompt}"),
        (ev::OPEN_CLIP, "play the link"),
        (ev::OPEN_CLIP, "open the link"),
    ]
}

/// Register vidkit's grammar on a shared matcher.
pub fn register(matcher: &mut voiced_core::grammar::Matcher) {
    for (event, phrase) in phrases() {
        let toks: Vec<String> = phrase.split_whitespace().map(str::to_string).collect();
        matcher.register_rule(APP, event, &toks);
    }
}

/// What a matched utterance asked for.
#[derive(Clone, PartialEq, Debug)]
pub enum Command {
    /// Do this now.
    Do(Action),
    /// Scrub to the sections matching a prompt. Carries the spoken text, which
    /// the relevance analyzer takes as-is.
    Find(String),
    /// Heard, understood, and not something this app does.
    Unhandled(String),
}

/// Small spoken numbers. Whisper writes digits most of the time, but "two" and
/// "ten seconds" turn up often enough that refusing them makes the feature feel
/// broken for no good reason.
fn spoken_number(word: &str) -> Option<f64> {
    Some(match word {
        "zero" => 0.0,
        "one" => 1.0,
        "two" => 2.0,
        "three" => 3.0,
        "four" => 4.0,
        "five" => 5.0,
        "ten" => 10.0,
        "fifteen" => 15.0,
        "twenty" => 20.0,
        "thirty" => 30.0,
        "sixty" | "a minute" => 60.0,
        "half" => 0.5,
        _ => return None,
    })
}

/// First number in a slot, spoken or written. `"1.5 times"` and `"ten
/// seconds"` both work.
pub fn parse_number(slot: &str) -> Option<f64> {
    let cleaned = slot.to_lowercase().replace(['x', ','], " ");
    for word in cleaned.split_whitespace() {
        let trimmed = word.trim_matches(|c: char| !c.is_ascii_digit() && c != '.');
        if let Ok(v) = trimmed.parse::<f64>() {
            return Some(v);
        }
        if let Some(v) = spoken_number(word) {
            return Some(v);
        }
    }
    // "back a minute" is two words and neither parses alone.
    spoken_number(cleaned.trim())
}

/// Route a matched app event to a command.
pub fn route(event_type: &str, slots: &[String]) -> Command {
    let slot = slots.first().map(String::as_str).unwrap_or("");
    match event_type {
        ev::PLAY | ev::PAUSE => Command::Do(Action::TogglePlay),
        ev::ROTATE => Command::Do(Action::Rotate),
        ev::FIT => Command::Do(Action::CycleFit),
        ev::RESET => Command::Do(Action::ResetView),
        ev::SUBTITLES => Command::Do(Action::ToggleSubtitles),
        ev::LISTEN => Command::Do(Action::ToggleListenOnly),
        ev::QUALITY_FULL => Command::Do(Action::SetSwBudget(1.0)),
        ev::SAVER => Command::Do(Action::SetSwBudget(0.25)),
        ev::SETTINGS => Command::Do(Action::OpenSettings),
        ev::QUIT => Command::Do(Action::Quit),
        ev::OPEN_CLIP => Command::Do(Action::PlayClipboard),
        // A bare "go back" with no amount is a sensible request, so an
        // unparseable slot falls back rather than doing nothing.
        ev::BACK => Command::Do(Action::SeekBy(-(parse_number(slot).unwrap_or(10.0) as i64))),
        ev::FORWARD => Command::Do(Action::SeekBy(parse_number(slot).unwrap_or(30.0) as i64)),
        ev::SPEED => match parse_number(slot) {
            // Clamped to the same range the gestures use, so "speed one
            // hundred" is not a way to reach a state the UI cannot leave.
            Some(v) => Command::Do(Action::SetSpeed(v.clamp(0.25, 4.0))),
            None => Command::Unhandled(format!("speed {slot}")),
        },
        ev::VOLUME => match parse_number(slot) {
            // Spoken as a percentage when it is one: "volume fifty" means 50%,
            // "volume 0.5" means the same thing.
            Some(v) => {
                let f = if v > 1.0 { v / 100.0 } else { v };
                Command::Do(Action::SetVolume(f.clamp(0.0, 1.0) as f32))
            }
            None => Command::Unhandled(format!("volume {slot}")),
        },
        ev::FIND => {
            let p = slot.trim();
            if p.is_empty() {
                Command::Unhandled("find with nothing to find".into())
            } else {
                Command::Find(p.to_string())
            }
        }
        other => Command::Unhandled(other.to_string()),
    }
}

/// Convenience: text in, command out, using a matcher this app is active on.
pub fn handle(matcher: &voiced_core::grammar::Matcher, utterance: &str) -> Option<Command> {
    match matcher.match_utterance(utterance) {
        voiced_core::grammar::Match::App { event_type, slots } => Some(route(&event_type, &slots)),
        // System intents (switch app, open X in Y) are the shell's business,
        // not the player's.
        voiced_core::grammar::Match::System(_) => None,
        voiced_core::grammar::Match::None => None,
    }
}

/// Seconds as a `Duration`, for a caller applying `SeekBy`.
pub fn seek_delta(secs: i64) -> Duration {
    Duration::from_secs(secs.unsigned_abs())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn matcher() -> voiced_core::grammar::Matcher {
        let mut m = voiced_core::grammar::Matcher::new();
        register(&mut m);
        m.set_active(APP);
        m
    }

    #[test]
    fn plain_commands_route_to_actions() {
        let m = matcher();
        for (said, want) in [
            ("play", Action::TogglePlay),
            ("pause", Action::TogglePlay),
            ("rotate", Action::Rotate),
            ("subtitles", Action::ToggleSubtitles),
            ("audio only", Action::ToggleListenOnly),
            ("battery saver", Action::SetSwBudget(0.25)),
            ("full quality", Action::SetSwBudget(1.0)),
            ("settings", Action::OpenSettings),
            ("reset the view", Action::ResetView),
        ] {
            assert_eq!(
                handle(&m, said),
                Some(Command::Do(want.clone())),
                "{said:?} did not route to {want:?}"
            );
        }
    }

    /// People do not say the command name, they say what they want, so several
    /// spellings map to one event.
    #[test]
    fn alternate_phrasings_reach_the_same_action() {
        let m = matcher();
        assert_eq!(handle(&m, "resume"), handle(&m, "play"));
        assert_eq!(handle(&m, "stop"), handle(&m, "pause"));
        assert_eq!(handle(&m, "captions"), handle(&m, "subtitles"));
    }

    #[test]
    fn a_prompt_is_carried_through_verbatim() {
        let m = matcher();
        assert_eq!(
            handle(&m, "skip to the part about the battery"),
            Some(Command::Find("the part about the battery".into()))
        );
        assert_eq!(
            handle(&m, "find the ending"),
            Some(Command::Find("the ending".into()))
        );
    }

    #[test]
    fn written_and_spoken_numbers_both_parse() {
        assert_eq!(parse_number("10 seconds"), Some(10.0));
        assert_eq!(parse_number("1.5"), Some(1.5));
        assert_eq!(parse_number("1.5x"), Some(1.5));
        assert_eq!(parse_number("ten seconds"), Some(10.0));
        assert_eq!(parse_number("two"), Some(2.0));
        assert_eq!(parse_number("a minute"), Some(60.0));
        assert_eq!(parse_number("banana"), None);
    }

    #[test]
    fn seeking_takes_a_direction_from_the_phrase() {
        let m = matcher();
        assert_eq!(
            handle(&m, "go back 20 seconds"),
            Some(Command::Do(Action::SeekBy(-20)))
        );
        assert_eq!(
            handle(&m, "skip ahead 30 seconds"),
            Some(Command::Do(Action::SeekBy(30)))
        );
    }

    /// "Go back" with no amount is a sensible thing to say; doing nothing
    /// because a slot was empty makes the feature feel broken.
    #[test]
    fn a_seek_with_no_amount_falls_back_rather_than_doing_nothing() {
        assert_eq!(route(ev::BACK, &[]), Command::Do(Action::SeekBy(-10)));
        assert_eq!(route(ev::FORWARD, &[]), Command::Do(Action::SeekBy(30)));
    }

    /// Voice must not reach a state the UI cannot leave.
    #[test]
    fn spoken_speed_is_clamped_to_the_same_range_as_the_gestures() {
        assert_eq!(route(ev::SPEED, &["100".into()]), Command::Do(Action::SetSpeed(4.0)));
        assert_eq!(route(ev::SPEED, &["0".into()]), Command::Do(Action::SetSpeed(0.25)));
        assert_eq!(route(ev::SPEED, &["1.5".into()]), Command::Do(Action::SetSpeed(1.5)));
    }

    /// "volume fifty" means 50%, and so does "volume 0.5".
    #[test]
    fn volume_accepts_a_percentage_or_a_fraction() {
        assert_eq!(route(ev::VOLUME, &["50".into()]), Command::Do(Action::SetVolume(0.5)));
        assert_eq!(route(ev::VOLUME, &["0.5".into()]), Command::Do(Action::SetVolume(0.5)));
        assert_eq!(route(ev::VOLUME, &["200".into()]), Command::Do(Action::SetVolume(1.0)));
    }

    /// A number that will not parse is reported, not silently swallowed --
    /// otherwise the viewer says something and nothing at all happens.
    #[test]
    fn an_unparseable_amount_is_reported() {
        assert!(matches!(
            route(ev::SPEED, &["banana".into()]),
            Command::Unhandled(_)
        ));
        assert!(matches!(route(ev::FIND, &["".into()]), Command::Unhandled(_)));
    }

    /// voiced-core gates hallucinations and non-speech before any rule runs;
    /// this checks vidkit inherits that rather than reacting to whisper's
    /// silence artifacts.
    #[test]
    fn silence_artifacts_do_not_trigger_commands() {
        let m = matcher();
        for noise in ["", "   ", "[BLANK_AUDIO]", "Thanks for watching!"] {
            assert_eq!(handle(&m, noise), None, "{noise:?} triggered a command");
        }
    }

    #[test]
    fn unrelated_speech_is_ignored() {
        let m = matcher();
        assert_eq!(handle(&m, "what time is dinner"), None);
    }

    /// Two lists drift. A command added to the palette and forgotten in the
    /// grammar is invisible until somebody says it out loud and nothing
    /// happens, so this fails when a new action arrives without a phrase.
    #[test]
    fn every_action_is_reachable_by_voice() {
        let routed: Vec<Action> = phrases()
            .iter()
            .filter_map(|(ev, _)| match route(ev, &["1".into()]) {
                Command::Do(a) => Some(a),
                _ => None,
            })
            .collect();

        for c in crate::palette::commands() {
            let covered = match &c.action {
                // Find is routed as Command::Find, not Command::Do.
                Action::ScrubToSections => phrases().iter().any(|(e, _)| *e == ev::FIND),
                // These need a file dialog or a channel listing; there is no
                // sensible utterance for them yet and pretending otherwise
                // would be a phrase that does nothing.
                Action::OpenFile | Action::BrowseClipboard | Action::SeekTo(_) => true,
                a => routed.iter().any(|r| std::mem::discriminant(r) == std::mem::discriminant(a)),
            };
            assert!(covered, "{} has no voice phrase", c.name);
        }
    }
}
