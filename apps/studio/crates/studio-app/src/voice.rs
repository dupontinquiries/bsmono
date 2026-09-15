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

// ── genre lexicons ──────────────────────────────────────────────────────

/// A vocabulary the fastpath can be taught. Genres name the same sounds
/// differently — a trap producer says "eight oh eight", a film composer
/// says "braams", a country session says "fiddle" — and none of those
/// words appear in a filename or a channel name. Each domain is a small
/// table of spoken phrases mapped to the words the LIBRARY actually uses,
/// so the alias closes a naming gap and never invents a concept.
///
/// These are deliberately STARTER dictionaries: a few dozen entries each,
/// chosen to be the terms a person says in the first minute, not an
/// attempt at a genre's whole language. They are data, so growing one is
/// a line, and the drift test below keeps every target honest.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Domain {
    /// Always on: the words every session uses regardless of genre.
    Common,
    Trap,
    HipHop,
    Classical,
    FilmScore,
    Country,
    /// Instrument names, which cut across genres and are the single
    /// biggest source of "it heard me and matched nothing".
    Instruments,
}

impl Domain {
    pub const ALL: [Domain; 7] = [Domain::Common, Domain::Trap, Domain::HipHop,
                                  Domain::Classical, Domain::FilmScore,
                                  Domain::Country, Domain::Instruments];
    pub fn token(self) -> &'static str {
        match self {
            Domain::Common => "common", Domain::Trap => "trap",
            Domain::HipHop => "hiphop", Domain::Classical => "classical",
            Domain::FilmScore => "film", Domain::Country => "country",
            Domain::Instruments => "instruments",
        }
    }
    pub fn from_token(t: &str) -> Option<Domain> {
        Domain::ALL.iter().copied().find(|d| d.token() == t)
    }
    pub fn label(self) -> &'static str {
        match self {
            Domain::Common => "common (always on)", Domain::Trap => "trap",
            Domain::HipHop => "hip hop", Domain::Classical => "classical",
            Domain::FilmScore => "film score", Domain::Country => "country",
            Domain::Instruments => "instrument names",
        }
    }

    /// `(spoken phrase, what the library calls it)`. Phrases may be
    /// several words; the longest match wins, so "hi hat" never lands as
    /// "hi" plus "hat".
    pub fn entries(self) -> &'static [(&'static str, &'static str)] {
        match self {
            Domain::Common => &[
                ("high hat", "hat"), ("hi hat", "hat"), ("hihat", "hat"),
                ("bass drum", "kick"), ("kick drum", "kick"),
                ("snare drum", "snare"), ("hand clap", "clap"),
                ("vocal", "vox"), ("vocals", "vox"), ("voice", "vox"),
                ("sound effect", "fx"), ("effect", "fx"),
                ("loop it", "loop"), ("beat", "drums"),
            ],
            Domain::Trap => &[
                // The genre's own names for its furniture.
                ("eight oh eight", "808"), ("eight o eight", "808"),
                ("sub bass", "808"), ("sub", "808"),
                ("hat roll", "hat"), ("rolling hats", "hat"),
                ("triplet hats", "hat"), ("open hat", "hat"),
                ("ad lib", "vox"), ("adlib", "vox"),
                ("riser", "fx"), ("stomp", "fx"),
                ("bell melody", "bell"), ("plucks", "melody"),
            ],
            Domain::HipHop => &[
                ("boom bap", "kick"), ("break beat", "loop"), ("breakbeat", "loop"),
                ("acapella", "vox"), ("a capella", "vox"),
                ("scratch", "fx"), ("chop", "loop"), ("chops", "loop"),
                ("rim shot", "rim"), ("dusty drums", "drums"),
                ("sample flip", "loop"),
            ],
            Domain::Classical => &[
                ("pizzicato", "pizz"), ("arco", "strings"),
                ("string section", "strings"), ("strings section", "strings"),
                ("timpani", "tom"), ("kettle drum", "tom"),
                ("choir", "vox"), ("chorale", "vox"),
                ("harpsichord", "keys"), ("piano forte", "piano"),
            ],
            Domain::FilmScore => &[
                // The words a composer says that no file is ever named.
                ("braams", "brass"), ("braam", "brass"), ("brass hit", "brass"),
                ("swell", "riser"), ("rise", "riser"), ("whoosh", "riser"),
                ("boom", "impact"), ("hit", "impact"), ("stinger", "impact"),
                ("drone", "pad"), ("bed", "pad"), ("texture", "pad"),
                ("ostinato", "loop"), ("pulse", "loop"),
            ],
            Domain::Country => &[
                ("fiddle", "violin"), ("pedal steel", "steel"),
                ("steel guitar", "steel"), ("dobro", "steel"),
                ("banjo roll", "banjo"), ("train beat", "drums"),
                ("brush kit", "drums"), ("brushes", "drums"),
                ("upright", "bass"), ("stand up bass", "bass"),
            ],
            Domain::Instruments => &[
                ("electric guitar", "guitar"), ("acoustic guitar", "guitar"),
                ("french horn", "horn"), ("double bass", "bass"),
                ("electric piano", "keys"), ("rhodes", "keys"),
                ("synthesizer", "synth"), ("organ", "keys"),
                ("cello", "cello"), ("viola", "viola"), ("violin", "violin"),
                ("trumpet", "trumpet"), ("sax", "sax"), ("saxophone", "sax"),
                ("flute", "flute"), ("clarinet", "clarinet"),
                ("mandolin", "mandolin"), ("harp", "harp"),
            ],
        }
    }
}

/// The active vocabularies. `Common` is always consulted; the rest are a
/// person's choice, kept per device in the workspace sidecar — a trap
/// session and a scoring session want different ears, and neither should
/// have to disable the other's words to be understood.
#[derive(Debug, Clone, PartialEq)]
pub struct Lexicon {
    pub domains: Vec<Domain>,
}

impl Default for Lexicon {
    fn default() -> Self { Lexicon { domains: vec![Domain::Common, Domain::Instruments] } }
}

impl Lexicon {
    pub fn enabled(&self, d: Domain) -> bool {
        d == Domain::Common || self.domains.contains(&d)
    }
    pub fn toggle(&mut self, d: Domain) {
        if d == Domain::Common { return; } // always on; toggling it is a no-op, said here
        if let Some(i) = self.domains.iter().position(|x| *x == d) {
            self.domains.remove(i);
        } else {
            self.domains.push(d);
            self.domains.sort();
        }
    }
    pub fn to_tokens(&self) -> String {
        self.domains.iter().map(|d| d.token()).collect::<Vec<_>>().join(",")
    }
    pub fn from_tokens(s: &str) -> Lexicon {
        let domains: Vec<Domain> = s.split(',').filter_map(|t| Domain::from_token(t.trim()))
            .collect();
        if domains.is_empty() { Lexicon::default() } else { Lexicon { domains } }
    }

    /// Rewrite spoken words into library words, longest phrase first so a
    /// two-word term is never split into two useless ones. Unknown words
    /// pass through untouched — a lexicon that swallowed what it did not
    /// know would make every unlisted sample unreachable by voice.
    ///
    /// The result is a SECOND READING, never a replacement: a caller must
    /// try the raw words too, because an alias can destroy a match that
    /// already worked (a track literally named "vocals" stops matching
    /// once "vocals" has become "vox"). [`Dialog::hear`] tries the
    /// aliased reading first and the literal one after, so the lexicon
    /// can only ever add understanding.
    pub fn apply(&self, words: &[String]) -> Vec<String> {
        let mut table: Vec<(Vec<String>, &'static str)> = Vec::new();
        for d in Domain::ALL {
            if !self.enabled(d) { continue; }
            for (spoken, canon) in d.entries() {
                table.push((spoken.split(' ').map(str::to_string).collect(), canon));
            }
        }
        // Longest first: "hi hat" must beat any single-word entry, and a
        // domain added later must not be able to shadow a longer phrase
        // just by being earlier in the list.
        table.sort_by_key(|(p, _)| std::cmp::Reverse(p.len()));
        let mut out: Vec<String> = Vec::with_capacity(words.len());
        let mut i = 0usize;
        'outer: while i < words.len() {
            for (phrase, canon) in &table {
                let n = phrase.len();
                if i + n <= words.len() && words[i..i + n] == phrase[..] {
                    // Don't repeat a word the previous alias already
                    // produced ("kick drum" then "drums" → one "kick").
                    if out.last().map(String::as_str) != Some(*canon) {
                        out.push((*canon).to_string());
                    }
                    i += n;
                    continue 'outer;
                }
            }
            out.push(words[i].clone());
            i += 1;
        }
        out
    }
}

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

/// The outcome of walking one reading of an utterance.
enum Walk {
    /// Reached a leaf: here is the act.
    Did(Act),
    /// Consumed everything and landed mid-wizard — the popup asks.
    Partial,
    /// Could not match the remaining words; carries them, so the popup
    /// can say WHICH part failed rather than "no".
    Stuck(String),
}

/// Consume the longest matching prefix at each step until the words run
/// out or nothing matches.
fn walk(pal: &mut palette::Palette, words: &[String], project: &Project) -> Walk {
    let mut rest = words;
    while !rest.is_empty() {
        let choices = pal.choices(project);
        let mut chosen: Option<(palette::Choice, usize)> = None;
        for len in (1..=rest.len()).rev() {
            let phrase = rest[..len].join(" ");
            if let Some(best) = palette::filter(&choices, &phrase).first() {
                chosen = Some(((*best).clone(), len));
                break;
            }
        }
        let Some((best, took)) = chosen else { return Walk::Stuck(rest.join(" ")) };
        rest = &rest[took..];
        if let Some(a) = pal.choose(&best) { return Walk::Did(a); }
    }
    Walk::Partial
}

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
    /// The active vocabularies — see [`Lexicon`]. Applied to every
    /// utterance before the palette sees it, which is why no command
    /// carries genre aliases of its own.
    pub lexicon: Lexicon,
}

impl Default for Dialog {
    fn default() -> Self {
        Dialog { state: State::Listening, pal: palette::Palette::open(),
                 lexicon: Lexicon::default() }
    }
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
        let raw = normalize(&heard);
        if raw.is_empty() {
            self.state = State::Failed { heard, why: "nothing but filler".into() };
            return None;
        }
        // Two readings, aliased first: the lexicon widens what can be
        // understood and must never narrow it.
        let aliased = self.lexicon.apply(&raw);
        let readings: Vec<Vec<String>> =
            if aliased == raw { vec![raw] } else { vec![aliased, raw] };
        // Walk the palette, consuming the LONGEST prefix that matches at
        // each step: "route vocals drum bus" is one utterance and three
        // steps, so a command can be said in one breath — and the same
        // machine handles it word by word when it is not, because a step
        // that consumes one word simply leaves the rest for the next one.
        // (The first draft joined the whole utterance into a single
        // phrase and treated a miss as fatal; a five-word sentence then
        // matched nothing at all.)
        let start = self.pal.clone();
        let mut act = None;
        let mut missed: Option<String> = None;
        let mut walked = false;
        for words in &readings {
            self.pal = start.clone();
            match walk(&mut self.pal, words, project) {
                Walk::Did(a) => { act = Some(a); walked = true; break; }
                Walk::Partial => { walked = true; break; }
                Walk::Stuck(what) => { missed.get_or_insert(what); }
            }
        }
        if !walked {
            self.state = State::Failed { heard,
                why: format!("nothing here matches '{}'",
                             missed.unwrap_or_else(|| "that".into())) };
            self.pal = palette::Palette::open();
            return None;
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
    fn a_genre_word_finds_the_file_a_genre_never_names() {
        // The gap these tables close: a recognizer emits what a person
        // says, and nobody names a file "braams" or "hi hat".
        let lex = Lexicon { domains: vec![Domain::Common, Domain::Trap,
                                          Domain::FilmScore] };
        let say = |s: &str| lex.apply(&normalize(s));
        assert_eq!(say("hi hat"), vec!["hat"], "two words, one sound");
        assert_eq!(say("hat roll"), vec!["hat"], "trap's own phrase, one token");
        // "high hat" consumes first (longest match at position 0), and the
        // leftover word passes through rather than being forced into a
        // second alias — harmless, and honest about what was matched.
        assert_eq!(say("high hat roll"), vec!["hat", "roll"]);
        assert_eq!(say("kick drum and the bass drum"), vec!["kick", "and", "kick"],
                   "each phrase resolves where it stands");
        assert_eq!(say("sub bass"), vec!["808"]);
        assert_eq!(say("braams"), vec!["brass"]);
        assert_eq!(say("add a whoosh"), vec!["add", "riser"],
                   "'a' is filler, 'add' is a word the person meant");
        // Unknown words survive: a lexicon that ate what it did not know
        // would make every unlisted sample unreachable by voice.
        assert_eq!(say("zaytoven"), vec!["zaytoven"]);
        // A domain that is off does not fire; Common always does.
        let off = Lexicon { domains: vec![] };
        assert_eq!(off.apply(&normalize("braams")), vec!["braams"]);
        assert_eq!(off.apply(&normalize("hi hat")), vec!["hat"], "common is always on");
        assert!(!off.enabled(Domain::Trap));
        assert!(off.enabled(Domain::Common));
    }

    #[test]
    fn an_alias_can_only_widen_what_is_understood_never_narrow_it() {
        // THE bug this design exists to prevent: 'vocals' aliases to
        // 'vox', which stops matching a track literally named "vocals".
        // The aliased reading is tried first and the literal one after,
        // so both spellings work.
        let p = project();
        let mut d = Dialog::default();
        assert!(d.hear("route vocals to the drum bus", &p).is_some(),
                "the literal name still resolves: {:?}", d.state);
        let mut d = Dialog::default();
        d.lexicon = Lexicon { domains: vec![Domain::Common, Domain::Trap] };
        assert!(d.hear("route vocals to the drum bus", &p).is_some(),
                "and it still resolves with more vocabularies loaded");
    }

    #[test]
    fn every_alias_points_at_a_word_the_libraries_actually_use() {
        // The drift guard: an alias whose target appears in no filename
        // and no channel name is a rewrite into nothing — it would make
        // the spoken word WORSE than useless, since the raw word at least
        // had a chance. Targets must be a classifier token, an instrument
        // name, or a plain lowercase word with no spaces.
        for d in Domain::ALL {
            for (spoken, canon) in d.entries() {
                assert!(!canon.is_empty(), "{d:?}: '{spoken}' maps to nothing");
                assert_eq!(*canon, canon.to_lowercase(),
                           "{d:?}: '{canon}' must be lowercase to match");
                assert!(!canon.contains(' '),
                        "{d:?}: '{canon}' is a phrase, not a searchable token");
                assert!(!spoken.is_empty() && *spoken == spoken.to_lowercase(),
                        "{d:?}: '{spoken}' must be lowercase to ever match");
                // And the spoken side must survive normalization, or it
                // can never be said.
                assert!(!normalize(spoken).is_empty(),
                        "{d:?}: '{spoken}' vanishes when normalized");
            }
        }
        // Tokens round-trip through the sidecar.
        let lex = Lexicon { domains: vec![Domain::Trap, Domain::Country] };
        assert_eq!(Lexicon::from_tokens(&lex.to_tokens()), lex);
        assert_eq!(Lexicon::from_tokens("nonsense"), Lexicon::default(),
                   "garbage means the default ears, not deaf ones");
        // Common cannot be switched off — it is the shared vocabulary.
        let mut l = Lexicon::default();
        l.toggle(Domain::Common);
        assert!(l.enabled(Domain::Common));
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

// ── the missing seam: real capture, feeding `App::voice_utterance` ──────
//
// Everything above this line was already complete and tested headless
// before this addition -- `hear`/`Dialog`/`State`/`feedback_popup` never
// needed to change. What was missing was anything calling
// `App::voice_utterance(text)` with a REAL transcription: Ctrl+Space
// opened a `Dialog` sitting at `State::Listening` with nothing behind it
// listening. This is that missing half, built on `voice-engine-kit` the
// same way `voice_popup.rs` (voiceterm-egui/uikit-egui) is -- spawned on
// its own thread on open, polled once per frame, fed in once ready.

/// One pending capture: `running` distinguishes "nothing started" from
/// "started, no result yet"; `result` is drained by [`poll_capture`] the
/// instant it lands.
#[derive(Default)]
pub struct VoiceCapturePending {
    running: bool,
    result: Option<String>,
}

/// Start listening on a background thread. Safe to call repeatedly while
/// already running -- it is a no-op in that case, matching
/// `voice_popup::spawn_listen`'s own guard for the same reason (Ctrl+Space
/// re-triggering mid-capture should not spawn a second worker).
pub fn spawn_listen(pending: std::sync::Arc<std::sync::Mutex<VoiceCapturePending>>) {
    // Matches voice_popup's own reasoning for calling this explicitly and
    // early: capture starts the instant this returns, not after whatever
    // latency exists before the spawned thread's first instruction runs.
    voice_engine_kit::begin_capture();
    {
        let mut p = pending.lock().unwrap_or_else(|e| e.into_inner());
        if p.running {
            return;
        }
        p.running = true;
        p.result = None;
    }
    std::thread::Builder::new()
        .name("studio-voice-listen".to_string())
        .spawn(move || {
            let (text, _audio) = voice_engine_kit::listen_with_audio();
            let mut p = pending.lock().unwrap_or_else(|e| e.into_inner());
            p.result = Some(text);
            p.running = false;
        })
        .ok();
}

/// Call once per frame. Returns `Some(text)` exactly once, the frame a
/// result lands -- `None` otherwise, including while still running.
///
/// Returns the RAW text, unclassified -- checked against `Dialog::hear`'s
/// real behavior before assuming it was safe not to: an empty string
/// (a cancelled recording, per `voice_engine_kit::listen_with_audio`'s
/// own convention) fed straight into `hear` sets `State::Failed { why:
/// "nothing but filler" }`, which misrepresents a deliberate cancel as a
/// misheard command. That classification belongs to the CALLER, which
/// already has to decide what "cancelled" vs "real error" vs "a genuine
/// utterance" means for this specific UI moment -- not duplicated here as
/// a second opinion `poll_capture` would then need to keep in sync with
/// whatever the caller does.
pub fn poll_capture(pending: &std::sync::Arc<std::sync::Mutex<VoiceCapturePending>>) -> Option<String> {
    let mut p = pending.lock().unwrap_or_else(|e| e.into_inner());
    if p.running {
        return None;
    }
    p.result.take()
}
