//! voice_control — pause, play and search by voice, if the person asks for it.
//!
//! Ported from voiceterm's fast path (`src/voice_intent.rs`) rather than
//! mounted from it: that module reaches `token_stats`, `confusion_table`
//! and `system_info`, so mounting it would drag a learned-filler store and
//! a Whisper correction table into a music app. What travels is the SHAPE
//! — normalise, then match exactly, then gate on confidence — and the
//! normalisation rules, which are the part that took real transcripts to
//! get right.
//!
//! ## Off by default, and that is not a formality
//!
//! A music player that listens is a microphone in a living room. It stays
//! off until someone turns it on in the frontend, the setting persists,
//! and nothing here opens an audio device — this module is a parser. What
//! feeds it is the host's business, which is also what makes it testable.
//!
//! ## Why exact matching rather than a model
//!
//! The vocabulary is a dozen phrases and the cost of a wrong guess is
//! high: "play" starting the wrong album is annoying, but a
//! misheard "next" during someone's favourite track is the app taking
//! something away. Everything that is not clearly a command comes back
//! `None`, and the caller does nothing.
//!
//! ## Search is deliberately weaker
//!
//! `search X` and `play X` both carry free text, which cannot be verified
//! against anything. They are marked low confidence so a caller that
//! wants confirmation before acting can ask for it, exactly as voiceterm's
//! `is_high_confidence` does for `SearchWeb`.

/// What a phrase asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    Play,
    Pause,
    /// "play/pause" as one control — what a headset button sends.
    Toggle,
    Next,
    Previous,
    Stop,
    /// Find something. Free text, so low confidence.
    Search(String),
    /// Play something by name. Free text, so low confidence.
    PlayNamed(String),
    VolumeUp,
    VolumeDown,
    /// 0–100.
    VolumeSet(u8),
    Mute,
    Unmute,
    ShuffleOn,
    ShuffleOff,
    RepeatOne,
    RepeatAll,
    RepeatOff,
}

impl Command {
    /// Can this be acted on without asking?
    ///
    /// Free text cannot be checked against anything, so a mishearing
    /// becomes a search for nonsense or the wrong album starting. The
    /// distinction is voiceterm's, for the same reason.
    pub fn is_high_confidence(&self) -> bool {
        !matches!(self, Command::Search(_) | Command::PlayNamed(_))
    }

    /// What a confirmation prompt would say.
    pub fn describe(&self) -> String {
        match self {
            Command::Play => "play".into(),
            Command::Pause => "pause".into(),
            Command::Toggle => "play or pause".into(),
            Command::Next => "next track".into(),
            Command::Previous => "previous track".into(),
            Command::Stop => "stop".into(),
            Command::Search(q) => format!("search for {q}"),
            Command::PlayNamed(q) => format!("play {q}"),
            Command::VolumeUp => "turn it up".into(),
            Command::VolumeDown => "turn it down".into(),
            Command::VolumeSet(n) => format!("set the volume to {n}"),
            Command::Mute => "mute".into(),
            Command::Unmute => "unmute".into(),
            Command::ShuffleOn => "shuffle on".into(),
            Command::ShuffleOff => "shuffle off".into(),
            Command::RepeatOne => "repeat this track".into(),
            Command::RepeatAll => "repeat everything".into(),
            Command::RepeatOff => "stop repeating".into(),
        }
    }
}

/// Words people say that carry no instruction.
///
/// The list is short on purpose. Stripping aggressively turns "play the
/// smiths" into "play smiths" and then a search misses; these are only
/// the ones that never appear inside a title.
const FILLERS: &[&str] = &["um", "uh", "erm", "like", "please", "just"];

/// Politeness and conversational scaffolding, stripped from the front.
///
/// Real people say "could you please play" far more than "play". The
/// exact-match layer below would miss every one of those without this.
const PREFIXES: &[&str] = &[
    "hey", "ok", "okay", "computer", "could you", "can you", "would you",
    "i want to", "i'd like to", "i would like to", "please",
];

/// Softeners people put on the end.
const SUFFIXES: &[&str] = &["for me", "right now", "real quick", "thanks", "thank you"];

/// Punctuation and whitespace off the end, in a LOOP.
///
/// Whisper leaves combinations — "next?!", "pause..." — and a single
/// `trim_end_matches` pass takes one character class, not all of them.
fn trim_tail(t: &str) -> String {
    let mut t = t.to_string();
    loop {
        let trimmed = t.trim_end_matches(|c: char| {
            matches!(c, '.' | '!' | '?' | ',' | ';') || c.is_whitespace()
        });
        if trimmed.len() == t.len() { break; }
        t = trimmed.to_string();
    }
    t
}

/// Normalise a transcript into bare-command form.
///
/// Lower-cased, punctuation off the end, politeness off the front,
/// softeners off the back, fillers removed. Ported from voiceterm's
/// `normalize_phrasing` and `strip_fillers` — the rules, not the code,
/// because its versions read a learned per-user store this app has no
/// business carrying.
pub fn normalize(text: &str) -> String {
    let mut t = text.trim().to_lowercase();
    t = trim_tail(&t);
    // Prefixes, repeatedly: "hey could you please play" is three of them
    // stacked, and one pass would leave two.
    loop {
        let before = t.len();
        for p in PREFIXES {
            if let Some(rest) = t.strip_prefix(p) {
                // Only on a WORD boundary, or "okay" would eat the start
                // of "okay computer" wrongly and "can you" would match
                // inside "canyon".
                if rest.is_empty() || rest.starts_with(' ') {
                    t = rest.trim_start().to_string();
                    break;
                }
            }
        }
        if t.len() == before { break; }
    }
    for s in SUFFIXES {
        if let Some(rest) = t.strip_suffix(s) {
            if rest.is_empty() || rest.ends_with(' ') {
                t = rest.trim_end().to_string();
            }
        }
    }
    // AGAIN, because removing a softener exposes the punctuation that
    // preceded it: "okay skip this, thanks" loses "thanks" and is left
    // with "skip this," which matches nothing. My first version trimmed
    // once, at the top, and the test found it.
    t = trim_tail(&t);
    // Fillers, from the FRONT only, and stopping at the first word that is
    // not one.
    //
    // My first version filtered every word in the phrase, and the test
    // caught it: "play like a prayer" became "play a prayer". Word-wise
    // matching does not save you — "like" is a filler AND the first word
    // of a great many songs, and the same is true of "just". Hesitation
    // happens at the start of an utterance; a filler in the middle of one
    // is usually part of a title.
    let mut words: Vec<&str> = t.split_whitespace().collect();
    while let Some(first) = words.first() {
        // Punctuation off the WORD before testing it: a hesitation is
        // transcribed "um," far more often than "um", and comparing the
        // raw token means the filler list never matches the thing it was
        // written for. The debug print that showed me "um, next track"
        // coming out of the normaliser is why this is here.
        let bare = first.trim_matches(|c: char| matches!(c, '.' | ',' | '!' | '?' | ';'));
        if FILLERS.contains(&bare) { words.remove(0); } else { break; }
    }
    words.join(" ")
}

/// Parse a transcript. `None` when it is not clearly a command.
///
/// Everything unrecognised does NOTHING, rather than falling back to a
/// search: a room where half of what is said becomes a search query is a
/// room where the music keeps changing on its own.
pub fn parse(text: &str) -> Option<Command> {
    let t = normalize(text);
    if t.is_empty() {
        return None;
    }
    // Exact phrases first. A `starts_with` layer below would otherwise
    // claim "play" out of "play the smiths".
    let exact = match t.as_str() {
        "play" | "resume" | "unpause" | "keep playing" => Some(Command::Play),
        "pause" | "hold on" | "wait" => Some(Command::Pause),
        "play pause" | "play or pause" | "toggle" => Some(Command::Toggle),
        "next" | "next track" | "next song" | "skip" | "skip this" =>
            Some(Command::Next),
        "previous" | "previous track" | "previous song" | "back" | "go back"
            | "start over" => Some(Command::Previous),
        "stop" | "stop the music" | "silence" => Some(Command::Stop),
        "louder" | "turn it up" | "volume up" | "turn up the volume" =>
            Some(Command::VolumeUp),
        "quieter" | "turn it down" | "volume down" | "turn down the volume" =>
            Some(Command::VolumeDown),
        "mute" | "be quiet" => Some(Command::Mute),
        "unmute" | "sound on" => Some(Command::Unmute),
        "shuffle" | "shuffle on" | "turn on shuffle" => Some(Command::ShuffleOn),
        "shuffle off" | "turn off shuffle" | "stop shuffling" =>
            Some(Command::ShuffleOff),
        "repeat" | "repeat this" | "repeat this track" | "loop this" =>
            Some(Command::RepeatOne),
        "repeat all" | "repeat everything" => Some(Command::RepeatAll),
        "repeat off" | "stop repeating" => Some(Command::RepeatOff),
        _ => None,
    };
    if exact.is_some() {
        return exact;
    }

    // "set the volume to 40", "volume 40"
    if let Some(n) = volume_number(&t) {
        return Some(Command::VolumeSet(n));
    }

    // LONGEST PREFIX FIRST, and a bare verb is not a query.
    //
    // Both parts were wrong first time. "search for" fell past
    // "search for " (no trailing space to match) into "search " and
    // produced a search for the word "for" — so the bare forms are
    // checked, and the list is ordered longest-first so "search for x"
    // cannot be claimed by "search ".
    if let Some(q) = after_verb(&t, &["search for", "look for", "search", "find"]) {
        return Some(Command::Search(q));
    }
    if let Some(q) = after_verb(&t, &["listen to", "put on", "play"]) {
        let q = q.trim_start_matches("some ").trim().to_string();
        if q.is_empty() { return None; }
        return Some(Command::PlayNamed(q));
    }
    None
}

/// The text after one of these verbs, when there is any.
///
/// `None` both when no verb matched and when the phrase was JUST the verb
/// — "search for" on its own is someone who stopped mid-sentence, and
/// opening an empty result list is a worse answer than doing nothing.
fn after_verb(t: &str, verbs: &[&str]) -> Option<String> {
    for v in verbs {
        if t == *v {
            return None;
        }
        if let Some(rest) = t.strip_prefix(v) {
            // Word boundary: "played" must not match "play".
            if let Some(q) = rest.strip_prefix(' ') {
                let q = q.trim();
                return if q.is_empty() { None } else { Some(q.to_string()) };
            }
        }
    }
    None
}

/// "volume 40", "set the volume to 40", "volume to 40 percent".
fn volume_number(t: &str) -> Option<u8> {
    let rest = t.strip_prefix("set the volume to ")
        .or_else(|| t.strip_prefix("set volume to "))
        .or_else(|| t.strip_prefix("volume to "))
        .or_else(|| t.strip_prefix("volume "))?;
    let n: u32 = rest.trim().trim_end_matches(" percent").trim().parse().ok()?;
    // Clamped, not refused: "volume 200" means as loud as it goes, and
    // refusing it makes the person say it again to no effect.
    Some(n.min(100) as u8)
}

/// Whether voice control is on, and how careful it should be.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VoicePrefs {
    /// OFF until asked for. A music player that listens is a microphone
    /// in a living room.
    pub enabled: bool,
    /// Act on low-confidence commands (free text) without confirming.
    ///
    /// Separate from `enabled` because they are different worries:
    /// whether to listen at all, and whether to trust what was heard.
    pub act_on_uncertain: bool,
    /// Require the phrase to start with this word. Empty means none.
    ///
    /// Not a wake WORD in the always-listening sense — this module never
    /// listens. It is a filter on transcripts a host already has, for a
    /// room where the television talks.
    pub prefix: String,
    /// The command that prints phrases, one per line. Empty means none,
    /// and none means nothing is spawned — see `voice_source`.
    ///
    /// A command rather than a built-in recogniser: voiceterm already owns
    /// the whisper models, the mic pipeline and the wake gate, and a
    /// second copy here would be a second thing to keep in step. What this
    /// app needs is the output.
    pub recognizer: String,
}

impl Default for VoicePrefs {
    fn default() -> Self {
        VoicePrefs { enabled: false, act_on_uncertain: false,
                     prefix: String::new(), recognizer: String::new() }
    }
}

/// The file, under the app's config dir.
pub const FILE: &str = "voice.conf";

pub fn render(p: &VoicePrefs) -> String {
    format!("# bo-suite voice control\n\
             # enabled: listen for music commands at all\n\
             # act_on_uncertain: act on \"play <something>\" and \"search …\" \
             without confirming\n\
             # prefix: require phrases to start with this word (blank for none)\n\
             #\n\
             # RECOGNISER CONTRACT: a command that prints ONE PHRASE PER LINE\n\
             # on stdout, unbuffered, for as long as it runs. Nothing else is\n\
             # assumed — no exit code, no protocol, no handshake. Blank means\n\
             # none, and none means nothing is spawned.\n\
             #\n\
             # An example needing no extra software:\n\
             #   mkfifo -m 600 /run/user/$UID/bo-voice.fifo\n\
             #   recognizer = cat /run/user/1000/bo-voice.fifo\n\
             # then have anything write a line into that fifo.\n\
             #\n\
             # Phrases over 120 characters are ignored: a command is a few\n\
             # words, and a sentence means something is transcribing the room\n\
             # rather than talking to the speaker.\n\
             enabled = {}\nact_on_uncertain = {}\nprefix = {}\nrecognizer = {}\n",
            p.enabled, p.act_on_uncertain, p.prefix, p.recognizer)
}

pub fn parse_prefs(text: &str) -> VoicePrefs {
    let mut p = VoicePrefs::default();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') { continue; }
        let Some((k, v)) = line.split_once('=') else { continue };
        let (k, v) = (k.trim(), v.trim());
        match k {
            "enabled" => p.enabled = matches!(v, "true" | "yes" | "on" | "1"),
            "act_on_uncertain" =>
                p.act_on_uncertain = matches!(v, "true" | "yes" | "on" | "1"),
            "prefix" => p.prefix = v.to_string(),
            "recognizer" => p.recognizer = v.to_string(),
            // Unknown keys ignored: the file outlives the build that wrote
            // it, and a setting removed later must not stop the rest
            // loading.
            _ => {}
        }
    }
    p
}

pub fn load() -> VoicePrefs {
    config_kit::load("bo-suite", FILE).map(|t| parse_prefs(&t)).unwrap_or_default()
}

pub fn store(p: &VoicePrefs) -> std::io::Result<()> {
    config_kit::save("bo-suite", FILE, &render(p))
}

/// What the app should do with a heard phrase.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// Voice is off, or the phrase was not a command.
    Ignored,
    /// Do it.
    Run(Command),
    /// Ask first — free text, and the person has not opted into acting on
    /// it unconfirmed.
    Confirm(Command),
}

/// Decide what a phrase means, under the current settings.
///
/// The gate lives HERE rather than at the call site so there is one place
/// that can be wrong about whether voice is on — a check duplicated across
/// three handlers is one that gets missed in the fourth.
pub fn decide(prefs: &VoicePrefs, text: &str) -> Outcome {
    if !prefs.enabled {
        return Outcome::Ignored;
    }
    let mut phrase = text.trim().to_lowercase();
    if !prefs.prefix.trim().is_empty() {
        let want = prefs.prefix.trim().to_lowercase();
        match phrase.strip_prefix(&want) {
            // Word boundary, or a prefix of "bo" would fire on "bought".
            Some(rest) if rest.is_empty() || rest.starts_with(' ') => {
                phrase = rest.trim_start().to_string();
            }
            _ => return Outcome::Ignored,
        }
    }
    match parse(&phrase) {
        None => Outcome::Ignored,
        Some(c) if c.is_high_confidence() || prefs.act_on_uncertain => Outcome::Run(c),
        Some(c) => Outcome::Confirm(c),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn on() -> VoicePrefs {
        VoicePrefs { enabled: true, ..Default::default() }
    }

    #[test]
    fn the_plain_transport_words_work() {
        assert_eq!(parse("pause"), Some(Command::Pause));
        assert_eq!(parse("Next!"), Some(Command::Next));
        assert_eq!(parse("previous track"), Some(Command::Previous));
        assert_eq!(parse("turn it up"), Some(Command::VolumeUp));
    }

    #[test]
    fn politeness_and_fillers_do_not_stop_a_command_matching() {
        // Real people say "could you please pause" far more than "pause",
        // and the exact-match layer would miss every one without the
        // normaliser.
        assert_eq!(parse("hey could you please pause"), Some(Command::Pause));
        assert_eq!(parse("um, next track for me"), Some(Command::Next));
        assert_eq!(parse("okay skip this, thanks"), Some(Command::Next));
    }

    #[test]
    fn a_filler_word_inside_a_title_survives() {
        // "like" is a filler and also the first word of a great many
        // songs. Stripping by substring would search for "a prayer".
        assert_eq!(parse("play like a prayer"),
                   Some(Command::PlayNamed("like a prayer".into())),
                   "a filler was cut out of a title");
    }

    #[test]
    fn play_alone_resumes_and_play_something_does_not() {
        // The distinction the exact-match-first ordering exists for: a
        // `starts_with` layer would claim "play" out of "play the smiths"
        // and resume whatever was paused instead.
        assert_eq!(parse("play"), Some(Command::Play));
        assert_eq!(parse("play the smiths"),
                   Some(Command::PlayNamed("the smiths".into())));
    }

    #[test]
    fn free_text_is_low_confidence_and_asks_before_acting() {
        // A misheard search is a query for nonsense; a misheard "play X"
        // starts the wrong album. Neither can be checked against anything.
        let p = on();
        assert_eq!(decide(&p, "search for miles davis"),
                   Outcome::Confirm(Command::Search("miles davis".into())));
        assert_eq!(decide(&p, "pause"), Outcome::Run(Command::Pause));

        let trusting = VoicePrefs { act_on_uncertain: true, ..on() };
        assert_eq!(decide(&trusting, "search for miles davis"),
                   Outcome::Run(Command::Search("miles davis".into())));
    }

    #[test]
    fn nothing_happens_when_voice_is_off() {
        // Off by default: a music player that listens is a microphone in a
        // living room.
        let p = VoicePrefs::default();
        assert!(!p.enabled);
        assert_eq!(decide(&p, "pause"), Outcome::Ignored);
    }

    #[test]
    fn an_unrecognised_phrase_does_nothing_rather_than_searching() {
        // A room where half of what is said becomes a search query is a
        // room where the music keeps changing on its own.
        let p = on();
        assert_eq!(decide(&p, "did you see the match last night"), Outcome::Ignored);
        assert_eq!(decide(&p, ""), Outcome::Ignored);
        assert_eq!(decide(&p, "search for"), Outcome::Ignored,
                   "an empty query opened a search");
    }

    #[test]
    fn a_prefix_filters_the_room_and_matches_on_a_word_boundary() {
        // For a room where the television talks. A bare `starts_with`
        // would fire "bo" on "bought".
        let p = VoicePrefs { prefix: "bo".into(), ..on() };
        assert_eq!(decide(&p, "bo pause"), Outcome::Run(Command::Pause));
        assert_eq!(decide(&p, "pause"), Outcome::Ignored);
        assert_eq!(decide(&p, "bought a record"), Outcome::Ignored);
    }

    #[test]
    fn a_volume_number_is_clamped_rather_than_refused() {
        // "volume 200" means as loud as it goes; refusing makes the person
        // repeat themselves to no effect.
        assert_eq!(parse("volume 40"), Some(Command::VolumeSet(40)));
        assert_eq!(parse("set the volume to 200"), Some(Command::VolumeSet(100)));
        assert_eq!(parse("volume to 0"), Some(Command::VolumeSet(0)));
        assert_eq!(parse("volume banana"), None);
    }

    #[test]
    fn the_settings_file_round_trips_and_survives_an_unknown_key() {
        let p = VoicePrefs { enabled: true, act_on_uncertain: true, prefix: "bo".into(),
                             recognizer: "voiceterm --print-phrases".into() };
        assert_eq!(parse_prefs(&render(&p)), p);
        let old = parse_prefs("enabled = true\nfuture_thing = 3\nprefix = bo\n");
        assert!(old.enabled);
        assert_eq!(old.prefix, "bo");
    }

    #[test]
    fn the_config_file_states_the_contract_and_an_example_that_works() {
        // The first version of this pointed at `voiceterm --print-phrases`,
        // a flag that does not exist. A hint that sends someone hunting is
        // worse than no hint, so the file carries the contract and an
        // example needing nothing but `mkfifo` — and this test keeps the
        // two from drifting apart.
        let text = render(&VoicePrefs::default());
        assert!(text.contains("ONE PHRASE PER"), "the contract is not stated");
        assert!(text.contains("mkfifo"), "no example that works today");
        assert!(!text.contains("--print-phrases"), "the invented flag came back");
        // The length rule is enforced in `voice_source`; saying it here is
        // how someone debugging a silent room finds out why.
        assert!(text.contains("120"), "the length limit is undocumented");
    }

    #[test]
    fn every_command_can_describe_itself_for_a_prompt() {
        // A confirmation that says "run command?" is one nobody can answer.
        for c in [Command::Pause, Command::Next, Command::VolumeSet(30),
                  Command::Search("x".into()), Command::PlayNamed("y".into())] {
            let d = c.describe();
            assert!(!d.is_empty() && d.len() < 40, "{d:?}");
        }
    }
}

/// The settings panel. Off, and what turning it on means.
///
/// Returns true when something changed, so the caller writes the file once
/// rather than every frame the panel is open.
pub fn voice_settings_ui(ui: &mut egui::Ui, p: &mut VoicePrefs,
                         c: &uikit_egui::PaletteColors) -> bool {
    let mut changed = false;
    changed |= ui.checkbox(&mut p.enabled, "voice control").changed();
    // Said plainly, next to the switch. Someone turning this on is
    // deciding whether to let an application act on speech in their home,
    // and the consequence should not be a paragraph away in a manual.
    ui.colored_label(c.text_weak,
        "Acts on phrases the app is given. It does not open a microphone \
         itself — whatever feeds it decides that.");
    if !p.enabled {
        return changed;
    }
    ui.separator();
    changed |= ui.checkbox(&mut p.act_on_uncertain,
                           "act on \"play …\" and \"search …\" without asking").changed();
    ui.colored_label(c.text_weak,
        "Those carry free text, so a mishearing starts the wrong thing. \
         Off, they ask first.");
    ui.horizontal(|ui| {
        ui.label("only after");
        changed |= ui.add(egui::TextEdit::singleline(&mut p.prefix)
                          .hint_text("nothing").desired_width(90.0)).changed();
    });
    ui.colored_label(c.text_weak,
        "A word every command must start with, for a room where the \
         television talks.");
    ui.horizontal(|ui| {
        ui.label("listen with");
        changed |= ui.add(egui::TextEdit::singleline(&mut p.recognizer)
                          .hint_text("nothing — no phrases will arrive")
                          .desired_width(230.0)).changed();
    });
    // Named examples, because "a command that prints phrases" is an
    // interface nobody can guess the shape of, and a blank box with no
    // hint is a feature switched on that never does anything.
    ui.colored_label(c.text_weak,
        "A command printing one phrase per line — voiceterm's recogniser, \
         or a script reading a fifo. Nothing runs until you name one.");
    changed
}

#[cfg(test)]
mod ui_tests {
    use super::*;

    fn draw(p: &mut VoicePrefs) -> Vec<String> {
        let ctx = egui::Context::default();
        let c = uikit_egui::palette_colors("amber");
        let out = ctx.run(egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO,
                                                        egui::vec2(420.0, 600.0))),
            ..Default::default()
        }, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| { voice_settings_ui(ui, p, &c); });
        });
        out.shapes.iter().filter_map(|s| match &s.shape {
            egui::epaint::Shape::Text(t) => Some(t.galley.text().to_string()),
            _ => None,
        }).collect()
    }

    #[test]
    fn turning_it_on_is_a_decision_the_panel_explains() {
        // Someone enabling this is deciding whether an app may act on
        // speech in their home. The consequence should not be a paragraph
        // away in a manual.
        let mut p = VoicePrefs::default();
        let joined = draw(&mut p).join(" ");
        assert!(joined.contains("voice control"), "{joined}");
        assert!(joined.contains("microphone"), "the panel does not say what it does: {joined}");
    }

    #[test]
    fn the_finer_settings_only_appear_once_it_is_on() {
        // Three switches for a feature that is off is three questions
        // nobody has to answer yet.
        let mut off = VoicePrefs::default();
        let a = draw(&mut off).join(" ");
        assert!(!a.contains("without asking"), "{a}");

        let mut on = VoicePrefs { enabled: true, ..Default::default() };
        let b = draw(&mut on).join(" ");
        assert!(b.contains("without asking"), "{b}");
        assert!(b.contains("television"), "the prefix is unexplained: {b}");
    }
}
