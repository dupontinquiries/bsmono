//! Voice: speak, and the world changes in front of you.
//!
//! No audio code lives here. The monorepo already has `voiced`, a
//! socket-activated daemon that runs mic -> whisper -> intent routing
//! and delivers `event <intent> <slot text>` to whichever app has
//! focus. The game's job is to register a grammar and act on events.
//! This module is the grammar and the mapping; the socket plumbing is
//! `ipc_kit::Client`, already used by every other app in the tree.
//!
//! Protocol, from `voiced/src/main.rs`'s own tests:
//!
//! ```text
//! hello <app>                                 register the app
//! register <intent> <state> <word> ... {slot} a slot grammar
//! focus <app>                                 (sent by the audio side)
//! event <intent> <slot text>                  delivered when matched
//! ```
//!
//! `/build enableVoice` sends the grammar and turns on LIVE mode: a
//! spoken build request previews immediately, and "build it" commits.
//! The stale-focus guard still applies -- you can look around while
//! deciding, but the commit only happens if you are looking back at
//! the spot the preview was made for.

/// What a recognised utterance means to the build session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VoiceIntent {
    /// "build a stone tower", "make a garden here"
    Build(String),
    /// "build it", "go ahead", "do it"
    Commit,
    /// "undo", "take it back"
    Undo,
    /// "bigger", "zoom out" / "smaller", "zoom in"
    Bigger,
    Smaller,
    /// "never mind", "cancel"
    Cancel,
    /// "show the plan"
    ShowPlan,
}

/// Intent names, as registered with voiced and as they come back in
/// `event`. Kept in one place so the two cannot drift.
pub const INTENT_BUILD: &str = "vox_build";
pub const INTENT_COMMIT: &str = "vox_commit";
pub const INTENT_UNDO: &str = "vox_undo";
pub const INTENT_BIGGER: &str = "vox_bigger";
pub const INTENT_SMALLER: &str = "vox_smaller";
pub const INTENT_CANCEL: &str = "vox_cancel";
pub const INTENT_PLAN: &str = "vox_plan";

/// The `register` messages to send after `hello`. Each is the args
/// list of one `Message::new("register", args)`. Several phrasings
/// per intent, because "build it" and "go ahead" should both commit,
/// and a grammar that only accepts one phrasing is a grammar the
/// player has to memorise.
pub fn grammar(app: &str) -> Vec<Vec<String>> {
    let _ = app;
    let reg = |intent: &str, words: &str| -> Vec<String> {
        let mut v = vec![intent.to_string(), "active".to_string()];
        v.extend(words.split_whitespace().map(String::from));
        v
    };
    vec![
        reg(INTENT_BUILD, "build {request}"),
        reg(INTENT_BUILD, "make {request}"),
        reg(INTENT_BUILD, "put {request}"),
        reg(INTENT_COMMIT, "build it"),
        reg(INTENT_COMMIT, "go ahead"),
        reg(INTENT_COMMIT, "do it"),
        reg(INTENT_COMMIT, "yes"),
        reg(INTENT_UNDO, "undo"),
        reg(INTENT_UNDO, "take it back"),
        reg(INTENT_UNDO, "no"),
        reg(INTENT_BIGGER, "bigger"),
        reg(INTENT_BIGGER, "zoom out"),
        reg(INTENT_SMALLER, "smaller"),
        reg(INTENT_SMALLER, "zoom in"),
        reg(INTENT_CANCEL, "never mind"),
        reg(INTENT_CANCEL, "cancel"),
        reg(INTENT_PLAN, "show the plan"),
    ]
}

/// Map a voiced `event` (verb, args) onto an intent. Returns None for
/// anything that is not ours, so the game's existing event handling
/// keeps seeing the rest.
pub fn parse_event(verb: &str, args: &[String]) -> Option<VoiceIntent> {
    if verb != "event" { return None; }
    let intent = args.first()?.as_str();
    let slot = args.get(1).map(|s| s.trim()).unwrap_or("");
    Some(match intent {
        INTENT_BUILD => {
            if slot.is_empty() { return None; }   // "build" with no request: not actionable
            VoiceIntent::Build(slot.to_string())
        }
        INTENT_COMMIT => VoiceIntent::Commit,
        INTENT_UNDO => VoiceIntent::Undo,
        INTENT_BIGGER => VoiceIntent::Bigger,
        INTENT_SMALLER => VoiceIntent::Smaller,
        INTENT_CANCEL => VoiceIntent::Cancel,
        INTENT_PLAN => VoiceIntent::ShowPlan,
        _ => return None,
    })
}

/// Turn an intent into the chat line the rest of the pipeline already
/// understands. Voice is a second keyboard: it produces the same
/// commands, so nothing downstream knows or cares which one spoke.
pub fn to_command_line(i: &VoiceIntent) -> Option<String> {
    Some(match i {
        VoiceIntent::Build(req) => format!("/build {req}"),
        VoiceIntent::Commit => "/commit".into(),
        VoiceIntent::Undo => "/undo".into(),
        VoiceIntent::Bigger => "/focus bigger".into(),
        VoiceIntent::Smaller => "/focus smaller".into(),
        VoiceIntent::ShowPlan => "/plan".into(),
        VoiceIntent::Cancel => return None,   // handled by the session directly
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> { v.iter().map(|x| x.to_string()).collect() }

    #[test]
    fn build_event_carries_the_request() {
        let i = parse_event("event", &s(&[INTENT_BUILD, "a stone tower on the hill"]));
        assert_eq!(i, Some(VoiceIntent::Build("a stone tower on the hill".into())));
        assert_eq!(to_command_line(&i.unwrap()), Some("/build a stone tower on the hill".into()));
    }

    #[test]
    fn empty_build_is_not_actionable() {
        assert_eq!(parse_event("event", &s(&[INTENT_BUILD, "  "])), None);
    }

    #[test]
    fn control_intents_map_to_the_same_chat_commands() {
        assert_eq!(to_command_line(&parse_event("event", &s(&[INTENT_COMMIT])).unwrap()), Some("/commit".into()));
        assert_eq!(to_command_line(&parse_event("event", &s(&[INTENT_UNDO])).unwrap()), Some("/undo".into()));
        assert_eq!(to_command_line(&parse_event("event", &s(&[INTENT_BIGGER])).unwrap()), Some("/focus bigger".into()));
        assert_eq!(to_command_line(&VoiceIntent::Cancel), None, "cancel is not a chat command");
    }

    #[test]
    fn foreign_events_are_ignored() {
        assert_eq!(parse_event("event", &s(&["play_track", "koyopper bass"])), None);
        assert_eq!(parse_event("focus", &s(&["unendlich"])), None);
    }

    #[test]
    fn grammar_registers_every_intent_with_a_slot_where_needed() {
        let g = grammar("unendlich");
        let builds: Vec<_> = g.iter().filter(|r| r[0] == INTENT_BUILD).collect();
        assert!(builds.len() >= 2, "several phrasings for build");
        for r in &builds { assert!(r.iter().any(|w| w == "{request}"), "build needs a slot: {r:?}"); }
        let commits = g.iter().filter(|r| r[0] == INTENT_COMMIT).count();
        assert!(commits >= 3, "several ways to say yes");
        for r in &g { assert_eq!(r[1], "active", "registration state"); }
        // every intent constant appears at least once
        for intent in [INTENT_BUILD, INTENT_COMMIT, INTENT_UNDO, INTENT_BIGGER,
                       INTENT_SMALLER, INTENT_CANCEL, INTENT_PLAN] {
            assert!(g.iter().any(|r| r[0] == intent), "{intent} not registered");
        }
    }
}
