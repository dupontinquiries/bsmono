//! voice_intent.rs — map a finalized spoken utterance to a `VoiceIntent`. PURE + unit-tested; the app
//! (`Chat::on_voice_utterance`) dispatches the result. Most intents are just modal `Command`s, so they
//! reuse the existing `run_command` dispatch — adding a new voice command is one match arm here (+ one
//! dispatch arm if it's not already a `Command`). Anything unrecognized falls through to dictation.

use crate::vimmode::Command;

/// A continuous camera-travel direction (relative to where the camera is looking). Started by "move back"
/// etc., stopped by "stop"/"hold here".
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MoveDir { Forward, Back, Left, Right, Up, Down }

/// What a spoken utterance means. Non-command variants are handled directly by the app; `Command`
/// variants ride the existing modal dispatch (goto / branch / prune / overview / view switch / best / stop).
#[derive(Clone, Debug, PartialEq)]
pub enum VoiceIntent {
    Command(Command),
    Tour(bool),       // start (true) / stop (false) the strand fly-through
    Freecam(bool),    // enter (true) / exit (false) free-flight
    CodeMode(bool),   // coder dictation on (true) / prose dictation off (false)
    HoldCamera,       // "hold here" / "wait a minute" — freeze camera traversal (stops tour/freecam/glide/drift)
    Move(MoveDir),    // "move back" / "move forward" — start continuous travel in a direction
    Send,             // submit the composer as a turn
    Dictate(String),  // fallback: the words go into the composer
}

/// Parse a finalized utterance into an intent. Recognizes a small, natural command vocabulary; everything
/// else is dictation (returned untrimmed-of-case as the ORIGINAL text so casing/punctuation is preserved).
pub fn parse(utterance: &str) -> VoiceIntent {
    // Fix systematically-misheard command words BEFORE matching (e.g. "brunch" -> "branch") — a no-op
    // until the confusion table has seeds/learned corrections loaded (see voice_confusion::load_from_disk
    // + seed_universal_confusions, called once from voice::init).
    let corrected = crate::voice_confusion::apply_corrections(utterance);
    let original = corrected.trim();
    let t = original.trim_end_matches(['.', '!', '?', ',']).trim().to_ascii_lowercase();

    // camera / flythrough
    if matches!(t.as_str(), "stop tour" | "end tour" | "land" | "stop flying") { return VoiceIntent::Tour(false); }
    if matches!(t.as_str(), "tour" | "start tour" | "fly" | "fly through" | "flythrough" | "take the tour") { return VoiceIntent::Tour(true); }
    if matches!(t.as_str(), "exit freecam" | "stop freecam" | "exit free cam" | "exit free camera") { return VoiceIntent::Freecam(false); }
    if matches!(t.as_str(), "freecam" | "free cam" | "free camera" | "free flight" | "fly free") { return VoiceIntent::Freecam(true); }
    // freeze camera traversal in place — "hold here" / "wait a minute" / etc. Distinct from Command::Stop
    // (which cancels generation); this holds the CAMERA where it is (stops tour, freecam, and any glide).
    if matches!(t.as_str(), "hold" | "hold here" | "hold on" | "hold it" | "wait" | "wait a minute" | "wait a moment" | "freeze" | "stay here" | "pause") { return VoiceIntent::HoldCamera; }
    // continuous directional travel — "move back" starts drifting, "stop"/"hold" ends it
    match t.as_str() {
        "move forward" | "move ahead" | "go forward" | "forward" | "move in" | "pull in" => return VoiceIntent::Move(MoveDir::Forward),
        "move back" | "move backward" | "move backwards" | "back up" | "pull back" | "back away" | "move out" => return VoiceIntent::Move(MoveDir::Back),
        "move left" | "go left" | "pan left" | "strafe left" => return VoiceIntent::Move(MoveDir::Left),
        "move right" | "go right" | "pan right" | "strafe right" => return VoiceIntent::Move(MoveDir::Right),
        "move up" | "go up" | "rise" | "ascend" => return VoiceIntent::Move(MoveDir::Up),
        "move down" | "go down" | "descend" | "drop down" => return VoiceIntent::Move(MoveDir::Down),
        _ => {}
    }

    // submit the composer
    if matches!(t.as_str(), "send" | "send it" | "submit" | "go ahead" | "send message" | "send that") { return VoiceIntent::Send; }

    // dictation mode: coder (spoken symbols → punctuation) vs prose (verbatim)
    if matches!(t.as_str(), "code mode" | "coder mode" | "code dictation" | "symbols on") { return VoiceIntent::CodeMode(true); }
    if matches!(t.as_str(), "prose mode" | "text mode" | "plain mode" | "symbols off") { return VoiceIntent::CodeMode(false); }

    // graph commands → the modal Command dispatch
    let words: Vec<&str> = t.split_whitespace().collect();
    if let Some(n) = goto_target(&words) { return VoiceIntent::Command(Command::Goto(n)); }
    if matches!(t.as_str(), "branch" | "branch here" | "fork" | "fork here" | "new branch" | "branch from here") { return VoiceIntent::Command(Command::Branch); }
    if matches!(t.as_str(), "prune" | "prune this" | "prune here" | "prune it") { return VoiceIntent::Command(Command::Prune); }
    if matches!(t.as_str(), "unprune" | "restore" | "un prune" | "bring it back") { return VoiceIntent::Command(Command::Unprune); }
    if matches!(t.as_str(), "overview" | "fit" | "zoom out" | "show all" | "show everything" | "birds eye" | "bird's eye") { return VoiceIntent::Command(Command::Overview); }
    if matches!(t.as_str(), "spatial" | "palace" | "mind palace" | "scene" | "3d" | "three d" | "spatial view" | "show the palace") { return VoiceIntent::Command(Command::Spatial); }
    if matches!(t.as_str(), "linear" | "chat view" | "text view" | "linear view" | "back to chat") { return VoiceIntent::Command(Command::Linear); }
    if matches!(t.as_str(), "best" | "show best" | "go to best" | "which is best") { return VoiceIntent::Command(Command::Best); }
    if matches!(t.as_str(), "stop" | "cancel" | "halt" | "abort") { return VoiceIntent::Command(Command::Stop); }

    VoiceIntent::Dictate(original.to_string())
}

/// Extract a goto target from "go to 12" / "focus on three" / "jump to 5" / "node 7" / "turn 7". Returns
/// the 1-based turn number the modal `Goto` expects (matching the ids shown in the UI). `None` otherwise.
fn goto_target(words: &[&str]) -> Option<usize> {
    let joined = words.join(" ");
    let rest = ["go to ", "goto ", "focus on ", "focus ", "jump to ", "jump ", "node ", "turn ", "select ", "open "]
        .iter().find_map(|p| joined.strip_prefix(p))?;
    parse_number(rest.trim())
}

/// Parse a small natural number: digits ("12") or number words ("twelve", "twenty one", "twenty-one").
fn parse_number(s: &str) -> Option<usize> {
    let s = s.trim();
    if let Ok(n) = s.parse::<usize>() { return Some(n); }
    const UNITS: &[(&str, usize)] = &[("zero",0),("one",1),("two",2),("three",3),("four",4),("five",5),("six",6),
        ("seven",7),("eight",8),("nine",9),("ten",10),("eleven",11),("twelve",12),("thirteen",13),("fourteen",14),
        ("fifteen",15),("sixteen",16),("seventeen",17),("eighteen",18),("nineteen",19)];
    const TENS: &[(&str, usize)] = &[("twenty",20),("thirty",30),("forty",40),("fifty",50),("sixty",60),
        ("seventy",70),("eighty",80),("ninety",90)];
    let parts: Vec<&str> = s.split([' ', '-']).filter(|x| !x.is_empty()).collect();
    match parts.as_slice() {
        [w] => UNITS.iter().chain(TENS).find(|(k, _)| k == w).map(|&(_, n)| n),
        [tw, uw] => {
            let t = TENS.iter().find(|(k, _)| k == tw).map(|&(_, n)| n)?;
            let u = UNITS.iter().find(|(k, _)| k == uw).map(|&(_, n)| n)?;
            if u < 10 { Some(t + u) } else { None }
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn digits_and_words_goto() {
        assert!(matches!(parse("go to 12"), VoiceIntent::Command(Command::Goto(12))));
        assert!(matches!(parse("goto 3"), VoiceIntent::Command(Command::Goto(3))));
        assert!(matches!(parse("focus on three"), VoiceIntent::Command(Command::Goto(3))));
        assert!(matches!(parse("jump to twenty one"), VoiceIntent::Command(Command::Goto(21))));
        assert!(matches!(parse("node seven"), VoiceIntent::Command(Command::Goto(7))));
        assert!(matches!(parse("turn 5."), VoiceIntent::Command(Command::Goto(5))));
    }

    #[test]
    fn graph_and_view_commands() {
        assert!(matches!(parse("branch here"), VoiceIntent::Command(Command::Branch)));
        assert!(matches!(parse("prune"), VoiceIntent::Command(Command::Prune)));
        assert!(matches!(parse("overview"), VoiceIntent::Command(Command::Overview)));
        assert!(matches!(parse("show the palace"), VoiceIntent::Command(Command::Spatial)));
        assert!(matches!(parse("back to chat"), VoiceIntent::Command(Command::Linear)));
        assert!(matches!(parse("stop"), VoiceIntent::Command(Command::Stop)));
    }

    #[test]
    fn camera_and_send_and_dictation() {
        assert!(matches!(parse("start tour"), VoiceIntent::Tour(true)));
        assert!(matches!(parse("stop tour"), VoiceIntent::Tour(false)));
        assert!(matches!(parse("free cam"), VoiceIntent::Freecam(true)));
        assert!(matches!(parse("hold here"), VoiceIntent::HoldCamera));
        assert!(matches!(parse("wait a minute"), VoiceIntent::HoldCamera));
        assert!(matches!(parse("move back"), VoiceIntent::Move(crate::voice_intent::MoveDir::Back)));
        assert!(matches!(parse("move forward"), VoiceIntent::Move(crate::voice_intent::MoveDir::Forward)));
        assert!(matches!(parse("move up"), VoiceIntent::Move(crate::voice_intent::MoveDir::Up)));
        assert!(matches!(parse("send it"), VoiceIntent::Send));
        // ordinary speech dictates, preserving the original casing/text
        match parse("What is the fault in this log?") {
            VoiceIntent::Dictate(s) => assert_eq!(s, "What is the fault in this log?"),
            _ => panic!("ordinary speech should dictate"),
        }
    }
}
