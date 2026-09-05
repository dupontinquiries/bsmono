//! What a spoken (or typed) sentence asks for.
//!
//! This is the TEXT half of the voice fast path. The acoustic half —
//! matching an utterance's 39-dim fingerprint against enrolled exemplars
//! before any recogniser runs — is `voiceprint-kit`, lifted from
//! voiceterm-qt, and [`crate::fastpath`] composes the two: a fingerprint
//! hit yields the transcript it was enrolled with, and that transcript
//! comes here. So this module never sees audio, and everything below is a
//! string test.
//!
//! # Grammar, deliberately small
//!
//! The common cases the operator named: turn lights and music on/off, drive
//! media, make a schedule. A sentence is lower-cased, stripped of filler
//! ("please", "the", "my", "can you"), then matched by leading pattern:
//!
//! ```text
//!   turn/switch/put  <target>  on/off        turn the kitchen light off
//!   <target>  on/off                          lights off
//!   turn on/off  <target>                     turn off everything
//!   play / pause / stop / next / previous / skip / volume up|down / mute
//!   [make sure] <power phrase>  by <time>     make sure the lamp is off by 9pm
//!   <power phrase>  at <time>
//!   <power phrase>  when I get home / when I leave / when (my) wifi gets weak|far
//! ```
//!
//! Anything else is [`Intent::Unknown`] with the cleaned text, and the UI
//! shows that rather than guessing.

use crate::schedule::{Action, Trigger, DEFAULT_WEAK_DBM};

/// Which device(s) a sentence means. Resolved against the hub by the shell
/// via [`Target::matches`]; kept symbolic here so a rule saved as
/// "lights" still works after the lamp is renamed or a second one appears.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    /// A device by (part of) its name: "kitchen light", "porch".
    Named(String),
    /// Every device whose name says light/lamp/bulb, or whose service is a
    /// lightbulb.
    Lights,
    /// The media player, not a BLE device at all.
    Music,
    /// Every controllable device.
    All,
}

impl Target {
    pub fn describe(&self) -> String {
        match self {
            Target::Named(n) => n.clone(),
            Target::Lights => "the lights".into(),
            Target::Music => "the music".into(),
            Target::All => "everything".into(),
        }
    }

    pub fn to_token(&self) -> String {
        match self {
            Target::Named(n) => format!("name:{n}"),
            Target::Lights => "lights".into(),
            Target::Music => "music".into(),
            Target::All => "all".into(),
        }
    }

    pub fn from_token(t: &str) -> Option<Target> {
        match t {
            "lights" => Some(Target::Lights),
            "music" => Some(Target::Music),
            "all" => Some(Target::All),
            _ => t.strip_prefix("name:").map(|n| Target::Named(n.to_string())),
        }
    }

    /// Does a device with this name match? Word-based, case-insensitive:
    /// "lamp" matches "Living Room Lamp", "living lamp" does not match
    /// "Kitchen Lamp" (every word of the target must appear).
    pub fn matches_name(&self, device_name: &str) -> bool {
        let name = device_name.to_ascii_lowercase();
        let words: Vec<&str> = name.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).collect();
        match self {
            Target::All => true,
            Target::Music => false,
            Target::Lights => words.iter().any(|w| is_light_word(w)),
            Target::Named(n) => {
                let want: Vec<String> = n.to_ascii_lowercase().split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).map(|w| singular(w)).collect();
                !want.is_empty() && want.iter().all(|w| words.iter().any(|d| singular(d) == *w))
            }
        }
    }
}

fn is_light_word(w: &str) -> bool {
    matches!(singular(w).as_str(), "light" | "lamp" | "bulb" | "led" | "strip")
}

fn singular(w: &str) -> String {
    let w = w.to_ascii_lowercase();
    if w.len() > 3 && w.ends_with('s') && !w.ends_with("ss") { w[..w.len() - 1].to_string() } else { w }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaCommand {
    PlayPause,
    Play,
    Pause,
    Stop,
    Next,
    Previous,
    VolumeUp,
    VolumeDown,
    Mute,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Intent {
    Power { target: Target, on: bool },
    Media(MediaCommand),
    Schedule { trigger: Trigger, action: Action },
    Unknown(String),
}

const FILLER: &[&str] = &["please", "the", "a", "an", "my", "our", "can", "you", "could", "would", "hey", "ok", "okay", "just", "go", "ahead", "and"];

fn clean(text: &str) -> Vec<String> {
    text.to_ascii_lowercase()
        .split(|c: char| !(c.is_alphanumeric() || c == ':'))
        .filter(|w| !w.is_empty() && !FILLER.contains(w))
        .map(|w| w.to_string())
        .collect()
}

/// "9pm", "9", "21", "9:30pm", "21:15", "noon", "midnight" → minute of day.
/// A bare hour 1–11 is read as PM: this is a lights app, "by 9" is bedtime
/// and "at 6" is dinner far more often than dawn. Mornings say "am". The
/// UI echoes the rule back as a sentence, so a wrong guess is visible.
pub fn parse_time(words: &[String], i: usize) -> Option<(u16, usize)> {
    let w = words.get(i)?;
    if w == "noon" { return Some((12 * 60, i + 1)); }
    if w == "midnight" { return Some((0, i + 1)); }
    let digits_end = w.trim_end_matches(|c: char| c.is_ascii_alphabetic()).len();
    let (num, suffix) = w.split_at(digits_end);
    let (h, m) = match num.split_once(':') {
        Some((h, m)) => (h.parse::<u16>().ok()?, m.parse::<u16>().ok()?),
        None => (num.parse::<u16>().ok()?, 0),
    };
    if h > 23 || m > 59 { return None; }
    let mut next = i + 1;
    let ampm = if !suffix.is_empty() { Some(suffix.to_string()) } else if let Some(n) = words.get(i + 1) {
        if n == "am" || n == "pm" { next += 1; Some(n.clone()) } else { None }
    } else { None };
    let h24 = match (ampm.as_deref(), h) {
        (Some("am"), 12) => 0,
        (Some("am"), h) => h,
        (Some("pm"), 12) => 12,
        (Some("pm"), h) if h < 12 => h + 12,
        (Some(_), h) => h,
        (None, h) if (1..=11).contains(&h) => h + 12,
        (None, h) => h,
    };
    Some((h24 * 60 + m, next))
}

fn power_word(w: &str) -> Option<bool> {
    match w {
        "on" | "up" => Some(true),
        "off" | "down" | "out" => Some(false),
        _ => None,
    }
}

fn target_from(words: &[String]) -> Option<Target> {
    let joined: Vec<String> = words.iter().filter(|w| !matches!(w.as_str(), "is" | "are" | "be" | "stays" | "stay")).cloned().collect();
    if joined.is_empty() { return None; }
    let s = joined.join(" ");
    match s.as_str() {
        "everything" | "all" | "all devices" | "all of them" => return Some(Target::All),
        "music" | "media" | "song" | "songs" | "player" | "speaker" | "speakers" | "tunes" | "radio" => return Some(Target::Music),
        // Plurals are the category; a singular "light"/"lamp" is a specific
        // device and goes through name matching (which still finds every
        // device with that word if there is only one).
        "lights" | "lamps" | "all lights" | "all the lights" | "every light" => return Some(Target::Lights),
        _ => {}
    }
    Some(Target::Named(s))
}

/// The power phrase inside a sentence: `(target, on)` if it has one.
/// Handles "turn X on", "turn on X", "X on", "make sure X is off",
/// "switch off X", "X stays off".
fn parse_power(words: &[String]) -> Option<(Target, bool)> {
    let w: Vec<String> = words.iter().filter(|x| !matches!(x.as_str(), "make" | "sure" | "that" | "is" | "are" | "be" | "stays" | "stay" | "should")).cloned().collect();
    if w.is_empty() { return None; }
    let verb = matches!(w[0].as_str(), "turn" | "switch" | "put" | "set" | "shut" | "power" | "kill");
    let rest: &[String] = if verb { &w[1..] } else { &w[..] };
    if rest.is_empty() { return None; }
    // "turn on X"
    if let Some(on) = power_word(&rest[0]) {
        if rest.len() > 1 {
            return target_from(&rest[1..]).map(|t| (t, on));
        }
        return None;
    }
    // "X on"
    if let Some(on) = power_word(rest.last()?) {
        return target_from(&rest[..rest.len() - 1]).map(|t| (t, on));
    }
    // "shut X" / "kill X" imply off
    if w[0] == "shut" || w[0] == "kill" {
        return target_from(rest).map(|t| (t, false));
    }
    None
}

fn parse_media(words: &[String]) -> Option<MediaCommand> {
    let s = words.join(" ");
    let c = match s.as_str() {
        "play" | "resume" | "play music" | "resume music" | "start music" | "music on" | "play something" => MediaCommand::Play,
        "pause" | "pause music" | "pause it" | "hold on" => MediaCommand::Pause,
        "stop" | "stop music" | "stop it" | "music off" | "quiet" | "silence" => MediaCommand::Stop,
        "next" | "skip" | "next song" | "next track" | "skip this" | "skip song" => MediaCommand::Next,
        "previous" | "back" | "go back" | "last song" | "previous song" | "previous track" => MediaCommand::Previous,
        "volume up" | "louder" | "turn it up" | "up" => MediaCommand::VolumeUp,
        "volume down" | "quieter" | "softer" | "turn it down" => MediaCommand::VolumeDown,
        "mute" | "shush" => MediaCommand::Mute,
        "toggle music" | "play pause" | "play or pause" => MediaCommand::PlayPause,
        _ => return None,
    };
    Some(c)
}

/// The whole sentence.
pub fn parse(text: &str) -> Intent {
    let words = clean(text);
    if words.is_empty() { return Intent::Unknown(String::new()); }

    // Schedules: split on the trigger phrase, whichever side it is on.
    if let Some((trigger, rest)) = split_trigger(&words) {
        if let Some((target, on)) = parse_power(&rest) {
            return Intent::Schedule { trigger, action: Action { target, on } };
        }
        return Intent::Unknown(words.join(" "));
    }

    if let Some(m) = parse_media(&words) {
        return Intent::Media(m);
    }
    if let Some((target, on)) = parse_power(&words) {
        // "turn the music on" is a media command, not a device.
        if target == Target::Music {
            return Intent::Media(if on { MediaCommand::Play } else { MediaCommand::Stop });
        }
        return Intent::Power { target, on };
    }
    Intent::Unknown(words.join(" "))
}

/// Find a trigger phrase; return it and the remaining words (the power
/// phrase). Trigger phrases: `by <time>`, `at <time>`, `when i get home`,
/// `when i come home`, `when i arrive`, `when i leave`, `when i go out`,
/// `when wifi gets weak|far|low`, `once wifi starts getting far`.
fn split_trigger(words: &[String]) -> Option<(Trigger, Vec<String>)> {
    let n = words.len();
    for i in 0..n {
        let w = words[i].as_str();
        if (w == "by" || w == "at") && i + 1 < n {
            if let Some((minute, end)) = parse_time(words, i + 1) {
                let mut rest = words[..i].to_vec();
                rest.extend_from_slice(&words[end..]);
                let t = if w == "by" { Trigger::By { minute_of_day: minute } } else { Trigger::At { minute_of_day: minute } };
                return Some((t, rest));
            }
        }
        if w == "when" || w == "once" || w == "whenever" || w == "if" {
            let tail: Vec<&str> = words[i + 1..].iter().map(|s| s.as_str()).collect();
            let joined = tail.join(" ");
            let (trig, consumed) = if joined.starts_with("i get home") || joined.starts_with("i come home") || joined.starts_with("i m home") || joined.starts_with("i arrive") || joined.starts_with("i am home") {
                (Trigger::ArriveHome, if joined.starts_with("i am home") || joined.starts_with("i get home") || joined.starts_with("i come home") { 3 } else if joined.starts_with("i arrive") { 2 } else { 3 })
            } else if joined.starts_with("i leave") || joined.starts_with("i go out") || joined.starts_with("i head out") || joined.starts_with("i m out") {
                (Trigger::LeaveHome, if joined.starts_with("i leave") { 2 } else { 3 })
            } else if joined.starts_with("wifi") || joined.starts_with("wi fi") || joined.starts_with("signal") {
                // "wifi gets weak", "wifi starts getting far", "wifi signal is low", "signal drops"
                let far = tail.iter().take(6).any(|t| matches!(*t, "weak" | "far" | "low" | "bad" | "drops" | "dropping" | "fades" | "fading" | "poor"));
                if !far { continue; }
                // consume through the qualifier word
                let upto = tail.iter().position(|t| matches!(*t, "weak" | "far" | "low" | "bad" | "drops" | "dropping" | "fades" | "fading" | "poor")).unwrap_or(0) + 1;
                // optional "but not disconnected" / "but still connected" — skip it
                let mut upto = upto;
                if tail.get(upto) == Some(&"but") {
                    while upto < tail.len() && !matches!(tail[upto], "turn" | "switch" | "put" | "set" | "make") { upto += 1; }
                }
                (Trigger::WifiWeak { below_dbm: DEFAULT_WEAK_DBM }, upto)
            } else {
                continue;
            };
            let mut rest = words[..i].to_vec();
            rest.extend_from_slice(&words[i + 1 + consumed..]);
            return Some((trig, rest));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn power(text: &str) -> (Target, bool) {
        match parse(text) {
            Intent::Power { target, on } => (target, on),
            other => panic!("{text:?} → {other:?}"),
        }
    }

    #[test]
    fn the_common_power_phrasings() {
        assert_eq!(power("turn the kitchen light off"), (Target::Named("kitchen light".into()), false));
        assert_eq!(power("turn off the kitchen light"), (Target::Named("kitchen light".into()), false));
        assert_eq!(power("lights on"), (Target::Lights, true));
        assert_eq!(power("turn on the lights"), (Target::Lights, true));
        assert_eq!(power("please switch the lamp on"), (Target::Named("lamp".into()), true));
        assert_eq!(power("turn everything off"), (Target::All, false));
        assert_eq!(power("shut the porch light"), (Target::Named("porch light".into()), false));
        assert_eq!(power("kill the lights"), (Target::Lights, false));
    }

    #[test]
    fn music_is_a_media_command_whichever_way_it_is_phrased() {
        assert_eq!(parse("turn the music on"), Intent::Media(MediaCommand::Play));
        assert_eq!(parse("turn off the music"), Intent::Media(MediaCommand::Stop));
        assert_eq!(parse("music off"), Intent::Media(MediaCommand::Stop));
        assert_eq!(parse("play"), Intent::Media(MediaCommand::Play));
        assert_eq!(parse("pause"), Intent::Media(MediaCommand::Pause));
        assert_eq!(parse("next song"), Intent::Media(MediaCommand::Next));
        assert_eq!(parse("skip"), Intent::Media(MediaCommand::Next));
        assert_eq!(parse("go back"), Intent::Media(MediaCommand::Previous));
        assert_eq!(parse("volume up"), Intent::Media(MediaCommand::VolumeUp));
        assert_eq!(parse("quieter"), Intent::Media(MediaCommand::VolumeDown));
        assert_eq!(parse("mute"), Intent::Media(MediaCommand::Mute));
    }

    #[test]
    fn deadlines_by_a_time() {
        match parse("make sure my light turns off by 9pm") {
            Intent::Schedule { trigger: Trigger::By { minute_of_day }, action } => {
                assert_eq!(minute_of_day, 21 * 60);
                assert_eq!(action, Action { target: Target::Named("light turns".into()), on: false });
            }
            other => panic!("{other:?}"),
        }
        match parse("turn the lamp off by 9:30 pm") {
            Intent::Schedule { trigger: Trigger::By { minute_of_day }, action } => {
                assert_eq!(minute_of_day, 21 * 60 + 30);
                assert_eq!(action.target, Target::Named("lamp".into()));
            }
            other => panic!("{other:?}"),
        }
        match parse("at 7am turn the lights on") {
            Intent::Schedule { trigger: Trigger::At { minute_of_day }, action } => {
                assert_eq!(minute_of_day, 7 * 60);
                assert_eq!(action, Action { target: Target::Lights, on: true });
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_bare_small_hour_reads_as_evening() {
        match parse("lights off by 9") {
            Intent::Schedule { trigger: Trigger::By { minute_of_day }, .. } => assert_eq!(minute_of_day, 21 * 60),
            other => panic!("{other:?}"),
        }
        match parse("lights on at 6") {
            Intent::Schedule { trigger: Trigger::At { minute_of_day }, .. } => assert_eq!(minute_of_day, 18 * 60),
            other => panic!("{other:?}"),
        }
        match parse("lights on at 21:15") {
            Intent::Schedule { trigger: Trigger::At { minute_of_day }, .. } => assert_eq!(minute_of_day, 21 * 60 + 15),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn home_and_wifi_triggers() {
        assert_eq!(parse("turn this light on when I get home"),
            Intent::Schedule { trigger: Trigger::ArriveHome, action: Action { target: Target::Named("this light".into()), on: true } });
        assert_eq!(parse("when I leave turn everything off"),
            Intent::Schedule { trigger: Trigger::LeaveHome, action: Action { target: Target::All, on: false } });
        assert_eq!(parse("once my wifi signal starts to get far but not disconnected yet turn the hall light on"),
            Intent::Schedule { trigger: Trigger::WifiWeak { below_dbm: DEFAULT_WEAK_DBM }, action: Action { target: Target::Named("hall light".into()), on: true } });
        assert_eq!(parse("turn on the porch light when the wifi gets weak"),
            Intent::Schedule { trigger: Trigger::WifiWeak { below_dbm: DEFAULT_WEAK_DBM }, action: Action { target: Target::Named("porch light".into()), on: true } });
    }

    #[test]
    fn nonsense_is_unknown_not_a_guess() {
        assert_eq!(parse("what is the weather"), Intent::Unknown("what is weather".into()));
        assert_eq!(parse("turn"), Intent::Unknown("turn".into()));
        assert_eq!(parse(""), Intent::Unknown(String::new()));
    }

    #[test]
    fn targets_match_device_names_by_words() {
        assert!(Target::Named("kitchen light".into()).matches_name("Kitchen Light"));
        assert!(Target::Named("lamp".into()).matches_name("Living Room Lamp"));
        assert!(!Target::Named("living lamp".into()).matches_name("Kitchen Lamp"));
        assert!(Target::Named("lights".into()).matches_name("Hall Light"), "plural/singular");
        assert!(Target::Lights.matches_name("Wemo Mini Lamp"));
        assert!(!Target::Lights.matches_name("Kettle"));
        assert!(Target::All.matches_name("anything"));
        assert!(!Target::Music.matches_name("Speaker Plug"), "music is never a BLE device");
    }

    #[test]
    fn target_tokens_round_trip() {
        for t in [Target::Named("a b".into()), Target::Lights, Target::Music, Target::All] {
            assert_eq!(Target::from_token(&t.to_token()), Some(t));
        }
        assert_eq!(Target::from_token("bogus"), None);
    }
}
