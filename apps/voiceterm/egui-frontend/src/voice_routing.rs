//! voice_routing — who gets to act on the microphone, and what stops when.
//!
//! voiceterm is a voice-driven app, so glove mode's dictation does not arrive
//! at an idle microphone. It arrives at one that is already listening for
//! commands, already identifying speakers, and already deciding whether to
//! offer enrolment. Something has to arbitrate, and none of the existing mic
//! modules does: `mic_topology`, `mic_scheduler` and `mic_pipeline_fsm` are all
//! about HARDWARE — how many whisper instances, which mic feeds which, when to
//! reshape. They answer "how do we hear", not "who gets to act".
//!
//! ## One capture, several consumers, one interpreter
//!
//! The mic is opened ONCE and fanned out. Two consumers opening the device
//! independently is how you get a recogniser that works until the moment the
//! other feature starts, on some devices and not others.
//!
//! Consumers split into two kinds, and the split is the whole design:
//!
//! * **Passive taps** — speaker fingerprinting, VAD, new-speaker detection.
//!   They observe and never act. They keep receiving audio in EVERY state,
//!   including dictation, which is the point: a long dictated message is the
//!   best fingerprint training data the app will ever get, and cutting the tap
//!   to "focus on dictation" would throw away exactly the audio worth keeping.
//! * **Interpreters** — the command NLU and the dictation sink. Exactly ONE is
//!   live at a time, because a transcript cannot be both a command and content.
//!
//! ## What shuts down, and why it is not just tidiness
//!
//! While dictating, the command NLU is SUSPENDED. Not filtered afterwards —
//! suspended. Two reasons, and the second is the one that matters:
//!
//! 1. It is the expensive stage. On a phone in the cold, with the screen dimmed
//!    by the posture engine and a recogniser already running, not running the
//!    intent matcher over every utterance is real battery.
//! 2. **A filtered command is still a command that nearly fired.** If the NLU
//!    runs and its output is discarded, every near-miss is one bug away from
//!    executing. Suspending it means the path does not exist while dictating.
//!
//! ## The always-live set has to be prefixed, or it is not always-live
//!
//! Some commands must survive dictation — you have to be able to stop. But
//! "stop" is a word people dictate. The escape from glove mode solved this
//! out-of-band, with a held pose; the equivalent in speech is a WAKE PREFIX.
//! `terminal, stop` cannot be produced by dictating a sentence, because the
//! prefix is addressed to the machine and people do not address machines
//! mid-sentence by accident.
//!
//! Which is also why the always-live set is tiny. Every word in it is a word
//! that can be triggered while the user is speaking to someone else.

/// The wake prefix that reaches the always-live commands during dictation.
///
/// Lowercased and compared after trimming, because the recogniser's
/// capitalisation is not something to build a safety property on.
pub const WAKE_PREFIX: &str = "terminal";

/// What the microphone is being used for right now.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Listening {
    /// Nothing is interpreting. Passive taps still run.
    #[default]
    Idle,
    /// The command vocabulary is live.
    Commands,
    /// A field is armed; transcripts are content.
    Dictation { field: String },
}

/// A consumer of the audio stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Consumer {
    /// Speaker identification and enrolment. Passive.
    Fingerprint,
    /// Voice activity detection. Passive, and cheap enough to always run.
    Vad,
    /// Intent matching. Suspended during dictation.
    CommandNlu,
    /// Where dictated text goes.
    DictationSink,
}

impl Consumer {
    /// Passive consumers observe and never act on the app.
    pub fn passive(self) -> bool {
        matches!(self, Consumer::Fingerprint | Consumer::Vad)
    }
}

/// What a transcript turned out to be.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Routed {
    /// Run it as a command.
    Command(String),
    /// Put it in the armed field.
    Text(String),
    /// An always-live command, reached through the wake prefix during
    /// dictation.
    Interrupt(String),
    /// Heard, and deliberately not acted on.
    Ignored,
}

/// The router.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct VoiceRouter {
    state: Listening,
    /// Commands that survive dictation, behind the wake prefix.
    always_live: Vec<String>,
}

impl VoiceRouter {
    /// The default always-live set: stop, and nothing else that acts on data.
    ///
    /// Kept tiny on purpose. Every word here is a word that can fire while the
    /// user is talking to a person rather than the phone, so the bar is "would
    /// I rather this fired by accident than be unable to say it".
    pub fn new() -> Self {
        VoiceRouter {
            state: Listening::Idle,
            always_live: vec!["stop".into(), "cancel".into(), "wake".into()],
        }
    }

    pub fn state(&self) -> &Listening { &self.state }
    pub fn dictating(&self) -> bool { matches!(self.state, Listening::Dictation { .. }) }
    pub fn always_live(&self) -> &[String] { &self.always_live }

    pub fn listen_for_commands(&mut self) { self.state = Listening::Commands; }
    pub fn idle(&mut self) { self.state = Listening::Idle; }

    /// Arm a field. The command NLU stops here, not later.
    pub fn dictate_into(&mut self, field: &str) {
        self.state = Listening::Dictation { field: field.to_string() };
    }

    /// Is a consumer receiving audio in the current state?
    ///
    /// Passive taps are always true — including during dictation, which is the
    /// point: a long dictated message is the best fingerprint training data the
    /// app will ever get.
    pub fn feeds(&self, c: Consumer) -> bool {
        if c.passive() { return true; }
        match (&self.state, c) {
            (Listening::Commands, Consumer::CommandNlu) => true,
            (Listening::Dictation { .. }, Consumer::DictationSink) => true,
            _ => false,
        }
    }

    /// Route a transcript.
    pub fn route(&self, transcript: &str) -> Routed {
        let t = transcript.trim();
        if t.is_empty() { return Routed::Ignored; }
        match &self.state {
            Listening::Idle => Routed::Ignored,
            Listening::Commands => Routed::Command(t.to_string()),
            Listening::Dictation { .. } => {
                // The wake prefix is the ONLY way through. Without it every
                // word is content, including "stop".
                match self.strip_wake(t) {
                    Some(rest) if self.is_always_live(&rest) => Routed::Interrupt(rest),
                    // Addressed to the machine but not an always-live command:
                    // ignored rather than dictated. Someone who said "terminal,
                    // list files" was talking to the app, and typing it into
                    // their message would be the worst of both.
                    Some(_) => Routed::Ignored,
                    None => Routed::Text(t.to_string()),
                }
            }
        }
    }

    /// Strip the wake prefix, if present.
    fn strip_wake(&self, t: &str) -> Option<String> {
        let lower = t.to_lowercase();
        let rest = lower.strip_prefix(WAKE_PREFIX)?;
        // A separator is required: "terminals are useful" is not addressed to
        // anything.
        let rest = rest.strip_prefix(',').or_else(|| rest.strip_prefix(' '))?;
        Some(rest.trim().to_string())
    }

    fn is_always_live(&self, phrase: &str) -> bool {
        self.always_live.iter().any(|w| w == phrase)
    }

    /// Which pipeline stages should be running.
    ///
    /// Returned as a list so a host can diff it against what it has spawned,
    /// rather than each caller deciding for itself what "dictating" implies.
    pub fn active_consumers(&self) -> Vec<Consumer> {
        let all = [Consumer::Fingerprint, Consumer::Vad,
                   Consumer::CommandNlu, Consumer::DictationSink];
        all.into_iter().filter(|c| self.feeds(*c)).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dictating() -> VoiceRouter {
        let mut r = VoiceRouter::new();
        r.dictate_into("body");
        r
    }

    #[test]
    fn fingerprinting_keeps_training_through_dictation() {
        // THE requirement. A long dictated message is the best training data
        // the app will ever get; cutting the tap to "focus on dictation" throws
        // away exactly the audio worth keeping.
        let r = dictating();
        assert!(r.feeds(Consumer::Fingerprint));
        assert!(r.feeds(Consumer::Vad));
    }

    #[test]
    fn passive_taps_run_in_every_state_including_idle() {
        for r in [VoiceRouter::new(), dictating(), {
            let mut c = VoiceRouter::new(); c.listen_for_commands(); c }] {
            assert!(r.feeds(Consumer::Fingerprint), "{:?}", r.state());
            assert!(r.feeds(Consumer::Vad));
        }
    }

    #[test]
    fn the_command_nlu_is_suspended_during_dictation_not_filtered_after() {
        // A filtered command is still a command that nearly fired: if the NLU
        // runs and its output is discarded, every near-miss is one bug away
        // from executing.
        let r = dictating();
        assert!(!r.feeds(Consumer::CommandNlu));
        assert!(r.feeds(Consumer::DictationSink));
    }

    #[test]
    fn exactly_one_interpreter_is_live_at_a_time() {
        // A transcript cannot be both a command and content.
        for r in [VoiceRouter::new(), dictating(), {
            let mut c = VoiceRouter::new(); c.listen_for_commands(); c }] {
            let live = [Consumer::CommandNlu, Consumer::DictationSink]
                .into_iter().filter(|c| r.feeds(*c)).count();
            assert!(live <= 1, "{:?} had {live} interpreters", r.state());
        }
    }

    #[test]
    fn an_idle_router_interprets_nothing_but_still_listens() {
        let r = VoiceRouter::new();
        assert_eq!(r.route("list files"), Routed::Ignored);
        assert!(r.feeds(Consumer::Fingerprint), "the taps do not stop");
    }

    #[test]
    fn commands_route_as_commands_when_that_is_what_is_live() {
        let mut r = VoiceRouter::new();
        r.listen_for_commands();
        assert_eq!(r.route("list files"), Routed::Command("list files".into()));
    }

    #[test]
    fn every_word_is_content_while_dictating_including_stop() {
        // Which is exactly why the always-live set needs a prefix.
        let r = dictating();
        assert_eq!(r.route("stop"), Routed::Text("stop".into()));
        assert_eq!(r.route("list files"), Routed::Text("list files".into()));
    }

    #[test]
    fn the_wake_prefix_reaches_an_always_live_command_mid_dictation() {
        let r = dictating();
        assert_eq!(r.route("terminal, stop"), Routed::Interrupt("stop".into()));
        assert_eq!(r.route("terminal stop"), Routed::Interrupt("stop".into()));
    }

    #[test]
    fn the_prefix_needs_a_separator_so_ordinary_words_do_not_trigger_it() {
        // "terminals are useful" is not addressed to anything.
        let r = dictating();
        assert_eq!(r.route("terminals are useful"),
                   Routed::Text("terminals are useful".into()));
    }

    #[test]
    fn addressing_the_machine_with_a_non_live_command_is_ignored_not_dictated() {
        // Someone who said "terminal, list files" was talking to the app.
        // Typing it into their message would be the worst of both outcomes.
        let r = dictating();
        assert_eq!(r.route("terminal, list files"), Routed::Ignored);
    }

    #[test]
    fn the_prefix_is_case_insensitive() {
        // The recogniser's capitalisation is not a thing to build a safety
        // property on.
        let r = dictating();
        assert_eq!(r.route("Terminal, stop"), Routed::Interrupt("stop".into()));
    }

    #[test]
    fn the_always_live_set_is_tiny() {
        // Every word here can fire while the user is talking to a person. The
        // bar is "would I rather this fired by accident than be unable to say
        // it", and almost nothing clears it.
        let r = VoiceRouter::new();
        assert!(r.always_live().len() <= 4, "{:?}", r.always_live());
        // Nothing in it acts on data.
        for w in r.always_live() {
            assert!(!w.contains("delete") && !w.contains("send"), "{w}");
        }
    }

    #[test]
    fn an_empty_transcript_is_ignored_in_every_state() {
        for r in [VoiceRouter::new(), dictating()] {
            assert_eq!(r.route("   "), Routed::Ignored);
        }
    }

    #[test]
    fn the_active_consumer_list_is_what_a_host_should_have_running() {
        // Returned as a list so a host can diff it against what it spawned,
        // rather than each caller deciding what "dictating" implies.
        let r = dictating();
        let active = r.active_consumers();
        assert!(active.contains(&Consumer::Fingerprint));
        assert!(active.contains(&Consumer::DictationSink));
        assert!(!active.contains(&Consumer::CommandNlu));
    }

    #[test]
    fn leaving_dictation_brings_the_command_path_back() {
        let mut r = dictating();
        assert!(!r.feeds(Consumer::CommandNlu));
        r.listen_for_commands();
        assert!(r.feeds(Consumer::CommandNlu));
        assert!(!r.feeds(Consumer::DictationSink));
    }
}
