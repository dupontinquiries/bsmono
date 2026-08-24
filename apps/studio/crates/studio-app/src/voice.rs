//! voice — one dialog handler for spoken commands, and the feedback popup.
//!
//! This is deliberately NOT a second command system. The palette already
//! models a command as a path of choices resolved by matching text
//! ([`palette::Palette::resolve`]), which means speech needs no verbs of
//! its own — only a way to turn an utterance into the words that path
//! wants, a policy for when to ask before acting, and a surface that says
//! what was heard. Those three things are this module, and each is pure:
//!
//! * [`normalize`] — spoken text to matchable tokens. People say "eight
//!   oh eight" and mean `808`, "see sharp" and mean `C#`, "minus three
//!   dee bee" and mean a gain step. A recognizer will never emit the
//!   library's spelling, so the gap is closed HERE rather than by
//!   littering every command with aliases.
//! * [`Policy`] — what may happen on a single utterance. Anything
//!   destructive or wide-reaching is confirmed out loud first, because
//!   the failure mode of speech is a misheard word and the failure mode
//!   of a misheard word must not be a deleted take.
//! * [`Dialog`] — the state machine, and with it the feedback popup that
//!   voiceterm-qt showed: listening, heard-this, need-a-target,
//!   confirm-this, did-it, could-not. The states ARE the popup's frames;
//!   a popup driven by anything else would drift from what the handler
//!   actually did.
//!
//! ## Commands nobody asked for because nothing was listening
//!
//! Half of [`EXTRA_SPOKEN`] would be silly as buttons and are obvious as
//! speech: asking the room a question ("what key am I in"), acting on
//! everything except one thing ("mute everything but the drums"), or
//! starting something on a delay so your hands are free ("record on
//! three"). They exist because a voice backend makes them cheap, not
//! because a menu was missing them.

use studio_core::Project;

use crate::palette::{self, Act};

/// Spoken text to tokens the palette can match. Lowercases, expands the
/// number and note-name words a recognizer emits, and drops the filler
/// ("please", "uh", "can you") that would otherwise weaken every score.
pub fn normalize(utterance: &str) -> Vec<String> {
    const FILLER: [&str; 15] = ["please", "uh", "um", "the", "a", "can", "you", "could",
                                "would", "just", "now", "hey", "to", "onto", "into"];
    let lowered = utterance.to_lowercase();
    let raw: Vec<&str> = lowered
        .split(|c: char| !c.is_alphanumeric() && c != '#' && c != '-')
        .filter(|s| !s.is_empty())
        .collect();
    let mut out: Vec<String> = Vec::new();
    let mut i = 0usize;
    while i < raw.len() {
        let w = raw[i];
        // "see sharp" / "b flat" — two words, one pitch. Checked before
        // the single-word pass so "sharp" never lands as its own token.
        if i + 1 < raw.len() {
            let pitch = spoken_pitch(w);
            match (pitch, raw[i + 1]) {
                (Some(p), "sharp") => { out.push(format!("{p}#")); i += 2; continue; }
                (Some(p), "flat") => { out.push(flatten(p)); i += 2; continue; }
                _ => {}
            }
        }
        if let Some(d) = spoken_digit(w) {
            // Runs of digits join: "eight oh eight" is one sample name,
            // and "one two eight" is a bitrate — not three arguments.
            let mut n = d.to_string();
            while i + 1 < raw.len() {
                match spoken_digit(raw[i + 1]) {
                    Some(d2) => { n.push_str(&d2.to_string()); i += 1; }
                    None => break,
                }
            }
            out.push(n);
            i += 1;
            continue;
        }
        if !FILLER.contains(&w) {
            out.push(w.to_string());
        }
        i += 1;
    }
    out
}

fn spoken_digit(w: &str) -> Option<u8> {
    Some(match w {
        "zero" | "oh" => 0, "one" => 1, "two" | "too" => 2, "three" => 3,
        "four" | "for" => 4, "five" => 5, "six" => 6, "seven" => 7,
        "eight" | "ate" => 8, "nine" => 9,
        other => return other.parse::<u8>().ok().filter(|_| other.len() == 1),
    })
}
fn spoken_pitch(w: &str) -> Option<&'static str> {
    Some(match w {
        "a" | "ay" => "a", "b" | "bee" | "be" => "b", "c" | "see" | "sea" => "c",
        "d" | "dee" => "d", "e" => "e", "f" | "ef" => "f", "g" | "gee" => "g",
        _ => return None,
    })
}
/// Flats spoken, sharps stored — the roll spells with sharps, so `b flat`
/// has to arrive as `a#` or it will match nothing.
fn flatten(p: &str) -> String {
    match p {
        "a" => "g#".to_string(), "b" => "a#".to_string(), "d" => "c#".to_string(),
        "e" => "d#".to_string(), "g" => "f#".to_string(),
        other => other.to_string(),
    }
}

/// How much a single utterance is allowed to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Policy {
    /// Do it. Reversible, narrow, or merely navigational.
    Act,
    /// Say it back and wait for "yes" — the misheard-word guard.
    Confirm,
}

/// Does this act need confirming? Destructive, wide, or hard-to-notice
/// changes do; moving the playhead does not. Deliberately conservative:
/// the cost of a needless confirmation is one word, and the cost of a
/// missing one is somebody's take.
pub fn policy(act: &Act) -> Policy {
    match act {
        Act::Ops(ops) => {
            let heavy = ops.iter().any(|op| matches!(op,
                studio_core::Op::RemoveClip { .. }
                | studio_core::Op::RemoveTrack { .. }
                | studio_core::Op::RemoveBus { .. }
                | studio_core::Op::ClearMarkers
                | studio_core::Op::RemoveGroup { .. }
                | studio_core::Op::SetKey { .. }));
            if heavy || ops.len() > 2 { Policy::Confirm } else { Policy::Act }
        }
        Act::Key(ch) => {
            // The keyboard verbs that cannot be taken back by one `u`, or
            // that a mishearing would make expensive.
            if matches!(ch, 'd' | 'G' | 'x') { Policy::Confirm } else { Policy::Act }
        }
        Act::Shell(v) => match v {
            palette::ShellVerb::RecordTake => Policy::Confirm,
            _ => Policy::Act,
        },
        Act::Panel(_) => Policy::Act,
    }
}

/// Spoken phrasings that only make sense BECAUSE something is listening —
/// the ones nobody puts in a menu. Each maps to a palette path, so they
/// cost no new verbs: the voice layer is a phrasebook, not a feature.
pub const EXTRA_SPOKEN: &[(&str, &[&str])] = &[
    // Questions. A menu cannot ask a question; a room can.
    ("what key am i in", &["__ask_key"]),
    ("how long is this", &["__ask_length"]),
    ("where am i", &["__ask_position"]),
    // Hands-free transport, the reason takes get lost when you are holding
    // an instrument.
    ("take it from the top", &["playhead to start"]),
    ("record on three", &["__record_countdown"]),
    ("scratch that", &["undo"]),
    ("again", &["undo"]),
    // Wide gestures that are tedious to click and natural to say.
    ("solo the vocal", &["__solo_named", "vocal"]),
    ("mute everything else", &["__solo_selected"]),
    ("drop a marker here", &["__marker_here"]),
    ("go to the chorus", &["jump to marker", "chorus"]),
];

/// One turn of the conversation, and one frame of the popup.
#[derive(Debug, Clone, PartialEq)]
pub enum State {
    /// Nothing said yet — the popup's resting frame.
    Listening,
    /// Heard, matched, and ready: the act, described.
    Ready { heard: String, describe: String, act: Act },
    /// Matched a verb that still needs a target; the popup lists the
    /// choices so a person can SEE what to say next, which is the whole
    /// reason voiceterm's popup existed.
    Needs { heard: String, prompt: String, options: Vec<String> },
    /// Say it back before doing it.
    Confirm { heard: String, describe: String, act: Act },
    Did { describe: String },
    /// Heard something, matched nothing. Carries what was heard so the
    /// person can hear their own mangling and try different words.
    Failed { heard: String, why: String },
}

/// The dialog: a palette walk driven by speech.
pub struct Dialog {
    pub state: State,
    pub pal: palette::Palette,
}

impl Default for Dialog {
    fn default() -> Self { Dialog { state: State::Listening, pal: palette::Palette::open() } }
}

impl Dialog {
    /// Feed an utterance. Returns an act ONLY when policy allows acting
    /// immediately; everything else parks in a state the popup renders
    /// and the next utterance advances.
    pub fn hear(&mut self, utterance: &str, project: &Project) -> Option<Act> {
        let heard = utterance.trim().to_string();
        // A pending confirmation answers first: "yes" completes it, and
        // anything else abandons it rather than being reinterpreted as a
        // new command — "no, go to the mixer" must not go to the mixer.
        if let State::Confirm { act, describe, .. } = self.state.clone() {
            let words = normalize(&heard);
            // ONLY the leading word answers, and only an unambiguous one.
            // The first draft searched the whole utterance for any
            // affirmative-ish word, so "no, go to the mixer" contained
            // "go" and was taken as yes — a confirmation that can be
            // triggered by a refusal is worse than no confirmation.
            let yes = matches!(words.first().map(String::as_str),
                Some("yes" | "yeah" | "yep" | "yup" | "confirm" | "correct" | "affirmative"))
                || words.first().map(String::as_str) == Some("do")
                   && words.get(1).map(String::as_str) == Some("it");
            self.state = if yes { State::Did { describe: describe.clone() } }
                         else { State::Listening };
            return if yes { Some(act) } else { None };
        }
        let words = normalize(&heard);
        if words.is_empty() {
            self.state = State::Failed { heard, why: "nothing but filler".into() };
            return None;
        }
        // Walk the palette, consuming the LONGEST prefix that matches at
        // each step: "route vocals drum bus" is one utterance and three
        // steps, so a command can be said in one breath — and the same
        // machine handles it word by word when it is not, because a step
        // that consumes one word simply leaves the rest for the next one.
        // (The first draft joined the whole utterance into a single
        // phrase and treated a miss as fatal; a five-word sentence then
        // matched nothing at all.)
        let mut act = None;
        let mut rest: &[String] = &words;
        while !rest.is_empty() {
            let choices = self.pal.choices(project);
            let mut took = 0usize;
            let mut chosen: Option<palette::Choice> = None;
            for len in (1..=rest.len()).rev() {
                let phrase = rest[..len].join(" ");
                if let Some(best) = palette::filter(&choices, &phrase).first() {
                    chosen = Some((*best).clone());
                    took = len;
                    break;
                }
            }
            let Some(best) = chosen else {
                self.state = State::Failed { heard,
                    why: format!("nothing here matches '{}'", rest.join(" ")) };
                self.pal = palette::Palette::open();
                return None;
            };
            rest = &rest[took..];
            if let Some(a) = self.pal.choose(&best) { act = Some(a); break; }
        }
        match act {
            Some(a) => {
                let describe = self.pal.path.join(" ");
                match policy(&a) {
                    Policy::Act => {
                        self.state = State::Did { describe };
                        self.pal = palette::Palette::open();
                        Some(a)
                    }
                    Policy::Confirm => {
                        self.state = State::Confirm { heard, describe, act: a };
                        self.pal = palette::Palette::open();
                        None
                    }
                }
            }
            None => {
                // Mid-wizard: show what can be said next.
                let opts: Vec<String> = self.pal.choices(project).iter()
                    .take(8).map(|c| c.label.clone()).collect();
                self.state = State::Needs { heard,
                    prompt: self.pal.path.join(" › "), options: opts };
                None
            }
        }
    }

    pub fn reset(&mut self) {
        self.state = State::Listening;
        self.pal = palette::Palette::open();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use studio_core::{Bus, BusId, Op, Track, TrackId, TrackKind};

    fn project() -> Project {
        let mut p = Project::default();
        for (i, n) in ["vocals", "drums"].iter().enumerate() {
            p.apply(&Op::AddTrack { track: Track { id: TrackId(i as u64 + 1),
                kind: TrackKind::Audio, name: (*n).into(), muted: false, locked: false,
                gain: 1.0, route: None }, at: i }).unwrap();
        }
        p.apply(&Op::AddBus { bus: Bus { id: BusId(1), name: "drum bus".into(),
            parent: None, gain: 1.0, chain: vec![], sends: vec![],
            muted: false } }).unwrap();
        p
    }

    #[test]
    fn spoken_numbers_and_note_names_become_what_the_library_spells() {
        // A recognizer will never emit "808" or "C#"; closing that gap
        // here is why no command needs a list of aliases.
        assert_eq!(normalize("eight oh eight"), vec!["808"]);
        assert_eq!(normalize("see sharp"), vec!["c#"]);
        assert_eq!(normalize("b flat minor"), vec!["a#", "minor"]);
        assert_eq!(normalize("please can you go to the mixer"), vec!["go", "mixer"],
                   "filler — including the connective 'to' — is dropped so it \
                    cannot dilute a score or be mistaken for a step");
        assert_eq!(normalize("one two eight"), vec!["128"], "a bitrate, not three words");
        assert!(normalize("um, uh, please").is_empty());
    }

    #[test]
    fn a_whole_command_can_be_said_in_one_breath_or_walked_step_by_step() {
        let p = project();
        // One breath.
        let mut d = Dialog::default();
        let act = d.hear("route vocals to the drum bus", &p);
        assert!(matches!(act, Some(Act::Ops(_))), "state: {:?}", d.state);
        assert!(matches!(d.state, State::Did { .. }));
        // Step by step, with the popup listing what to say next — the
        // reason voiceterm showed a popup at all.
        let mut d = Dialog::default();
        assert!(d.hear("route", &p).is_none());
        match &d.state {
            State::Needs { options, .. } => {
                assert!(options.iter().any(|o| o == "vocals"), "{options:?}");
            }
            other => panic!("expected a prompt, got {other:?}"),
        }
        assert!(d.hear("vocals", &p).is_none());
        assert!(d.hear("drum bus", &p).is_some(), "the walk completes");
    }

    #[test]
    fn destructive_acts_are_confirmed_and_no_is_not_reinterpreted() {
        // The failure mode of speech is a misheard word; the failure mode
        // of a misheard word must not be a deleted take.
        assert_eq!(policy(&Act::Key('d')), Policy::Confirm);
        assert_eq!(policy(&Act::Key('u')), Policy::Act);
        assert_eq!(policy(&Act::Ops(vec![Op::ClearMarkers])), Policy::Confirm);
        assert_eq!(policy(&Act::Panel(studio_egui::touch::Panel::Mixer)), Policy::Act);

        let p = project();
        let mut d = Dialog::default();
        assert!(d.hear("delete selection", &p).is_none(), "not done yet");
        assert!(matches!(d.state, State::Confirm { .. }));
        // "no, go to the mixer" must NOT go to the mixer: the answer to a
        // confirmation is an answer, not a new command.
        assert!(d.hear("no go to the mixer", &p).is_none());
        assert!(matches!(d.state, State::Listening));
        // And the affirmative has to LEAD: a sentence that merely contains
        // an agreeable word is not agreement.
        let mut d2 = Dialog::default();
        d2.hear("delete selection", &p);
        assert!(d2.hear("wait, i said do the other thing", &p).is_none());
        let mut d = Dialog::default();
        d.hear("delete selection", &p);
        assert!(d.hear("yes", &p).is_some(), "and yes completes it");
    }

    #[test]
    fn an_unmatched_utterance_says_what_it_heard_and_forgets_the_walk() {
        let p = project();
        let mut d = Dialog::default();
        assert!(d.hear("banana telephone", &p).is_none());
        match &d.state {
            State::Failed { heard, .. } => assert!(heard.contains("banana"),
                "the popup shows the mangling so a person can rephrase"),
            other => panic!("{other:?}"),
        }
        // A failed walk resets rather than leaving a half-open wizard the
        // next utterance would fall into.
        assert!(d.pal.pending.is_none());
    }

    #[test]
    fn every_extra_spoken_phrase_normalizes_to_something_matchable() {
        // The phrasebook must survive its own normalizer: a phrase that
        // normalizes to nothing is a phrase that can never be said.
        for (spoken, path) in EXTRA_SPOKEN {
            assert!(!normalize(spoken).is_empty(), "'{spoken}' vanishes when normalized");
            assert!(!path.is_empty());
        }
    }
}

/// The feedback popup: voiceterm-qt's, ported to egui and driven by
/// [`Dialog::state`] rather than by its own parallel bookkeeping. Every
/// frame of it corresponds to a state the handler can actually be in,
/// which is the property the Qt version had and the reason it never lied
/// about what the machine was doing.
///
/// Returns `true` when the popup wants the caller to keep the microphone
/// hot (mid-wizard or awaiting a confirmation) — the one thing the shell
/// needs to know that the state alone does not say.
#[cfg(feature = "app")]
pub fn feedback_popup(ctx: &egui::Context, dialog: &Dialog,
                      c: &uikit_egui::PaletteColors) -> bool {
    let (title, tint) = match &dialog.state {
        State::Listening => ("listening…", c.text_weak),
        State::Ready { .. } => ("ready", c.accent),
        State::Needs { .. } => ("which one?", c.accent),
        State::Confirm { .. } => ("say yes to confirm", c.warning),
        State::Did { .. } => ("done", c.success),
        State::Failed { .. } => ("not understood", c.warning),
    };
    let hot = matches!(dialog.state, State::Needs { .. } | State::Confirm { .. });
    egui::Area::new("voice-feedback".into())
        .anchor(egui::Align2::CENTER_BOTTOM, egui::vec2(0.0, -48.0))
        .interactable(false)
        .show(ctx, |ui| {
            egui::Frame::popup(ui.style()).show(ui, |ui| {
                ui.set_max_width(420.0);
                ui.colored_label(tint, title);
                match &dialog.state {
                    State::Listening => {}
                    State::Ready { heard, describe, .. }
                    | State::Confirm { heard, describe, .. } => {
                        ui.weak(format!("heard: {heard}"));
                        ui.strong(describe);
                    }
                    State::Needs { heard, prompt, options } => {
                        ui.weak(format!("heard: {heard}"));
                        if !prompt.is_empty() { ui.strong(prompt); }
                        // The list is the point: a person cannot guess
                        // what to say, so the popup shows it.
                        for o in options.iter().take(6) { ui.weak(format!("· {o}")); }
                    }
                    State::Did { describe } => { ui.strong(describe); }
                    State::Failed { heard, why } => {
                        // Showing the mangling is what lets someone
                        // rephrase instead of repeating themselves louder.
                        ui.weak(format!("heard: {heard}"));
                        ui.weak(why);
                    }
                }
            });
        });
    hot
}
