//! dictation — the voice pipeline, with the double-transcription designed out.
//!
//! ## What was actually going wrong
//!
//! The streaming worker snapshots the WHOLE ring buffer each interval and runs
//! whisper on it, so every partial is a **complete re-transcription of the
//! utterance so far** — not an increment. Partial 3 already contains everything
//! partial 2 said.
//!
//! That has one correct consumption rule and one tempting wrong one:
//!
//! * **Replace.** Partial N+1 supersedes partial N. This is right.
//! * **Append.** Looks reasonable if you think of partials as a stream of new
//!   words, and produces "hello hello world hello world there" — the doubling.
//!
//! And the same mistake at a larger scale: running a final pass over the same
//! audio and *appending* its result to what the partials already showed
//! transcribes the whole utterance twice and prints it twice.
//!
//! ## Partials are good enough to keep
//!
//! On real use the partials are accurate and fast enough to be the actual
//! output, which makes the final pass redundant work that produces a second
//! copy of text you already have. So [`Mode::PartialsOnly`] is the default:
//! commit the last partial and skip the final entirely. It is faster (no
//! several-hundred-millisecond wait on release), it cannot double, and it is
//! what the user already reads on screen — so what they see committed is
//! exactly what they saw while speaking, which matters more than a marginal
//! accuracy gain nobody can perceive.
//!
//! [`Mode::FinalPass`] remains for the cases where the extra accuracy is worth
//! the wait, and it **replaces** rather than appends.

/// How an utterance is finished.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Mode {
    /// Commit the last partial; run no final pass. Fast, and cannot double.
    #[default]
    PartialsOnly,
    /// Run a final pass and replace the partial with its result.
    FinalPass,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Idle,
    Listening,
    /// Only reachable in [`Mode::FinalPass`].
    Transcribing,
}

/// One dictation session.
/// Map a settings-kit filter level to voice-kit's three flags.
///
/// voice-kit's own doc says "the flags come from a settings-kit `VoiceFilter`
/// level, mapped by voiced so this crate stays UI-agnostic" — the mapping was
/// always meant to live at the call site, and I had hardcoded `(true, true, _)`
/// instead, which silently overrode anyone who had chosen Off.
///
/// `partial` matters because a partial is mid-word by nature: requiring it to
/// look like a complete sentence would blank the preview every other frame, so
/// Strict only tightens the FINAL.
pub fn filter_flags(level: settings_kit::VoiceFilter, partial: bool) -> (bool, bool, bool) {
    use settings_kit::VoiceFilter as V;
    match level {
        V::Off => (false, false, false),
        V::Light => (true, false, false),
        V::Standard => (true, true, false),
        V::Strict => (true, true, !partial),
    }
}

#[derive(Debug, Clone)]
pub struct Dictation {
    /// How hard to filter. From settings-kit's A11yPrefs, so a user who chose
    /// Off is not silently overridden by a hardcoded Standard.
    pub filter_level: settings_kit::VoiceFilter,
    pub mode: Mode,
    state: State,
    /// The live, uncommitted text — a complete transcription of the utterance
    /// so far, replaced wholesale by each new partial.
    partial: String,
    /// What has actually been handed to the text field.
    committed: String,
    /// Guards against a second finish for the same utterance, which is how a
    /// double tap or a late worker result would otherwise emit the text twice.
    finished: bool,
}

impl Default for Dictation {
    fn default() -> Self {
        Dictation {
            filter_level: settings_kit::VoiceFilter::Standard,
            mode: Mode::default(), state: State::Idle,
            partial: String::new(), committed: String::new(), finished: false,
        }
    }
}

impl Dictation {
    pub fn state(&self) -> State { self.state }
    /// What to show live, greyed, while speaking.
    pub fn partial(&self) -> &str { &self.partial }
    /// What has been committed this session.
    pub fn committed(&self) -> &str { &self.committed }
    pub fn is_active(&self) -> bool { self.state != State::Idle }

    pub fn start(&mut self) {
        self.state = State::Listening;
        self.partial.clear();
        self.committed.clear();
        self.finished = false;
    }

    /// A partial arrived.
    ///
    /// REPLACES, never appends — each partial already contains everything the
    /// previous one said. Ignored once the utterance is finished, so a worker
    /// that was mid-inference when the user released cannot resurrect stale
    /// text over the committed result.
    pub fn partial_arrived(&mut self, text: &str) {
        if self.state != State::Listening || self.finished { return; }
        // Whisper invents text from silence — "thank you", "you", "[BLANK_AUDIO]"
        // — and in PartialsOnly mode the last partial IS what gets committed, so
        // an unfiltered hallucination is typed into the user's document. The
        // filter is deliberately CONSERVATIVE: it matches the canonical list
        // exactly, so "thank you very much for the help" survives.
        // filter_transcript composes the three filters in the right order and
        // returns None when nothing survives — using it rather than chaining
        // the parts by hand, since the ordering (de-loop AFTER artifact strip,
        // because artifacts sit past the loop) is a decision the kit already
        // made and getting it wrong here would be invisible.
        //
        // require_speech is FALSE on partials: a partial is mid-word by nature
        // and rejecting it for not looking like a sentence would blank the
        // preview on every other frame.
        let (a, h, sp) = filter_flags(self.filter_level, true);
        match voice_kit::hallucination::filter_transcript(text, a, h, sp) {
            Some(cleaned) => self.partial = cleaned,
            // Keep the previous partial rather than blanking: a moment of
            // silence mid-sentence should not erase what was already heard.
            None => {}
        }
    }

    /// The user released. Returns the text to insert, or `None` when there is
    /// nothing to say.
    ///
    /// In `PartialsOnly` this is the whole story: the last partial is the
    /// result, committed immediately with no second inference and therefore no
    /// possibility of a second copy.
    pub fn finish(&mut self) -> Option<String> {
        if self.finished || self.state == State::Idle { return None; }
        match self.mode {
            Mode::PartialsOnly => {
                self.finished = true;
                self.state = State::Idle;
                let text = std::mem::take(&mut self.partial);
                if text.is_empty() { return None; }
                self.committed = text.clone();
                Some(text)
            }
            Mode::FinalPass => {
                self.state = State::Transcribing;
                None // the caller runs the final pass and calls final_arrived
            }
        }
    }

    /// The final pass came back.
    ///
    /// REPLACES the partial rather than appending to it — both describe the
    /// same audio. Returns the text to display, and whether the caller must
    /// **replace** what it already inserted rather than insert again, which is
    /// the distinction that stops the doubling at the text-field boundary.
    pub fn final_arrived(&mut self, text: &str) -> Option<Commit> {
        if self.finished || self.state != State::Transcribing { return None; }
        self.finished = true;
        self.state = State::Idle;
        // Now actually hallucinated-away, rather than only described as such.
        // require_speech is TRUE here: a final IS a complete utterance, so one
        // that does not look like speech is whisper filling silence.
        let (a, h, sp) = filter_flags(self.filter_level, false);
        let text = voice_kit::hallucination::filter_transcript(text, a, h, sp)
            .unwrap_or_default();
        let previous = std::mem::take(&mut self.partial);
        // An empty or hallucinated-away final must not wipe a good partial: the
        // partial was accurate enough to read, so keep it.
        let text = if text.is_empty() { previous.clone() } else { text };
        if text.is_empty() { return None; }
        self.committed = text.clone();
        Some(Commit { text, replaces: previous })
    }

    /// Abandon without committing.
    pub fn cancel(&mut self) {
        self.state = State::Idle;
        self.partial.clear();
        self.finished = true;
    }
}

/// The result of a final pass.
#[derive(Debug, Clone, PartialEq)]
pub struct Commit {
    pub text: String,
    /// What was already shown and must be REMOVED first. Empty when nothing was
    /// displayed live. Handing the caller both halves is what makes "replace"
    /// enforceable rather than a convention it can forget.
    pub replaces: String,
}

/// How much audio a streaming partial should re-examine.
///
/// The GTK worker re-transcribes the entire ring every interval, so a
/// thirty-second dictation runs whisper over ~30 s of audio on the last pass
/// and has re-read the opening fifty times. Capping the window bounds the work
/// per partial and, on a phone, is the difference between partials that keep up
/// and partials that fall behind the speaker.
///
/// The cost is that text older than the window stops being revised — which is
/// fine, because it has already been read and accepted by the person speaking.
pub const PARTIAL_WINDOW_MS: u32 = 12_000;

/// Samples to feed the next partial, given everything captured so far.
pub fn partial_window(total_samples: usize, rate: u32) -> std::ops::Range<usize> {
    let cap = (PARTIAL_WINDOW_MS as usize * rate as usize) / 1000;
    let start = total_samples.saturating_sub(cap);
    start..total_samples
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What the streaming worker actually produces: each partial is a COMPLETE
    /// re-transcription of the utterance so far, not an increment.
    const PARTIALS: [&str; 4] = [
        "hello",
        "hello world",
        "hello world how",
        "hello world how are you",
    ];

    #[test]
    fn partials_replace_rather_than_accumulate() {
        // THE bug. Treating partials as a stream of new words and appending
        // them produces "hello hello world hello world how ..." — which is
        // exactly what double-transcription looks like from the outside.
        let mut d = Dictation::default();
        d.start();
        for p in PARTIALS {
            d.partial_arrived(p);
        }
        assert_eq!(d.partial(), "hello world how are you", "the last partial IS the utterance");
        assert!(!d.partial().contains("hello world hello"), "no accumulation");
    }

    #[test]
    fn partials_only_commits_once_and_never_transcribes_twice() {
        // If the partials are good enough to read, a final pass is redundant
        // work over the same audio — and that redundancy is where the second
        // copy comes from.
        let mut d = Dictation::default();
        assert_eq!(d.mode, Mode::PartialsOnly, "the default avoids the second pass");
        d.start();
        for p in PARTIALS { d.partial_arrived(p); }
        assert_eq!(d.finish().as_deref(), Some("hello world how are you"));
        assert_eq!(d.state(), State::Idle, "no transcribing state at all");
        // and a second finish emits nothing — a double tap or a late worker
        // result cannot produce a second copy
        assert_eq!(d.finish(), None);
        assert_eq!(d.finish(), None);
    }

    #[test]
    fn a_late_partial_cannot_resurrect_stale_text_over_the_committed_result() {
        // The worker can be mid-inference when the user releases; its result
        // arrives after the commit and must be dropped.
        let mut d = Dictation::default();
        d.start();
        d.partial_arrived("hello world");
        let committed = d.finish().unwrap();
        d.partial_arrived("hello wor");     // stale, from a slower inference
        assert_eq!(d.partial(), "", "ignored after finishing");
        assert_eq!(d.committed(), committed);
    }

    #[test]
    fn a_final_pass_replaces_the_partial_and_says_what_to_remove() {
        // The other half of the fix, at the text-field boundary: the caller is
        // handed both the new text AND what it must remove, so "replace" is
        // enforceable rather than a convention it can forget.
        let mut d = Dictation { mode: Mode::FinalPass, ..Default::default() };
        d.start();
        for p in PARTIALS { d.partial_arrived(p); }
        assert_eq!(d.finish(), None, "a final pass defers the commit");
        assert_eq!(d.state(), State::Transcribing);

        let c = d.final_arrived("Hello, world. How are you?").expect("commits");
        assert_eq!(c.text, "Hello, world. How are you?");
        assert_eq!(c.replaces, "hello world how are you", "remove what was shown live");
        // the committed text is the final, not both concatenated
        assert!(!d.committed().contains("hello world how are you hello"));
        assert_eq!(d.committed(), "Hello, world. How are you?");
        // and it cannot commit twice
        assert_eq!(d.final_arrived("something else"), None);
    }

    #[test]
    fn an_empty_final_does_not_wipe_a_good_partial() {
        // Whisper sometimes returns nothing on a final pass over audio it
        // already transcribed happily. The partial was accurate enough to read,
        // so losing it to an empty result would be a regression the user sees
        // as "it deleted what I said".
        let mut d = Dictation { mode: Mode::FinalPass, ..Default::default() };
        d.start();
        d.partial_arrived("turn the lights on");
        d.finish();
        let c = d.final_arrived("   ").expect("keeps the partial");
        assert_eq!(c.text, "turn the lights on");
    }

    #[test]
    fn silence_commits_nothing() {
        let mut d = Dictation::default();
        d.start();
        assert_eq!(d.finish(), None, "nothing was said");
        assert_eq!(d.committed(), "");
        // and a finish with no start does nothing at all
        let mut d = Dictation::default();
        assert_eq!(d.finish(), None);
    }

    #[test]
    fn cancelling_commits_nothing_even_if_a_partial_was_showing() {
        let mut d = Dictation::default();
        d.start();
        d.partial_arrived("delete everything");
        d.cancel();
        assert_eq!(d.committed(), "");
        assert_eq!(d.finish(), None, "cancelled means cancelled");
        assert!(!d.is_active());
    }

    #[test]
    fn starting_again_clears_the_previous_utterance() {
        let mut d = Dictation::default();
        d.start();
        d.partial_arrived("first thing");
        d.finish();
        d.start();
        assert_eq!(d.partial(), "");
        assert_eq!(d.committed(), "", "a new utterance does not inherit the old one");
        d.partial_arrived("second thing");
        assert_eq!(d.finish().as_deref(), Some("second thing"));
    }

    #[test]
    fn the_partial_window_bounds_the_work_per_inference() {
        // The GTK worker re-transcribes the entire ring every interval, so a
        // 30-second dictation re-reads its opening fifty times and the partials
        // fall behind the speaker on a phone.
        let rate = 16_000;
        // short utterance: the whole thing
        assert_eq!(partial_window(8_000, rate), 0..8_000);
        // long one: only the tail
        let long = 30 * rate as usize;
        let w = partial_window(long, rate);
        assert_eq!(w.end, long);
        assert_eq!(w.len(), (PARTIAL_WINDOW_MS as usize * rate as usize) / 1000);
        assert!(w.start > 0, "the opening is no longer re-read every time");
        assert!(w.len() < long, "and the work per partial is bounded");
    }

#[cfg(test)]
mod filter_tests {
    use super::*;

    fn listening() -> Dictation {
        let mut d = Dictation::default();
        d.start();
        d
    }

    #[test]
    fn a_user_who_chose_off_is_not_silently_overridden() {
        // The bug this mapping fixes: the flags were hardcoded to (true, true, _),
        // so a person who turned filtering OFF still had their transcripts
        // rewritten — and could never work out why.
        use settings_kit::VoiceFilter as V;
        assert_eq!(filter_flags(V::Off, true), (false, false, false));
        assert_eq!(filter_flags(V::Off, false), (false, false, false));
        let mut d = listening();
        d.filter_level = V::Off;
        d.partial_arrived("thank you");
        assert_eq!(d.partial(), "thank you", "off means off");
    }

    #[test]
    fn each_level_adds_exactly_one_filter() {
        // Light trims artifacts, Standard also drops hallucinations, Strict also
        // requires speech. A level that skipped one would be indistinguishable
        // from its neighbour.
        use settings_kit::VoiceFilter as V;
        assert_eq!(filter_flags(V::Light, false),    (true, false, false));
        assert_eq!(filter_flags(V::Standard, false), (true, true,  false));
        assert_eq!(filter_flags(V::Strict, false),   (true, true,  true));
    }

    #[test]
    fn strict_tightens_the_final_but_not_the_partial() {
        // A partial is mid-word by nature; requiring it to look like a complete
        // sentence would blank the preview every other frame.
        use settings_kit::VoiceFilter as V;
        assert_eq!(filter_flags(V::Strict, true).2, false, "partial is not required to be speech");
        assert_eq!(filter_flags(V::Strict, false).2, true, "the final is");
    }

    #[test]
    fn a_hallucinated_partial_does_not_erase_a_good_one() {
        // Whisper invents "thank you" from silence, and in PartialsOnly the last
        // partial IS what gets committed — so an unfiltered one is typed into
        // the user's document. A pause mid-sentence must keep what was heard.
        let mut d = listening();
        d.partial_arrived("turn left at the");
        d.partial_arrived("thank you");
        assert_eq!(d.partial(), "turn left at the", "the silence was dropped");
        d.partial_arrived("turn left at the lights");
        assert_eq!(d.partial(), "turn left at the lights");
    }

    #[test]
    fn a_real_phrase_containing_a_filtered_word_survives() {
        // The filter matches the canonical list EXACTLY, so this must not be
        // mistaken for the hallucination it starts with.
        let mut d = listening();
        d.partial_arrived("thank you very much for the help");
        assert_eq!(d.partial(), "thank you very much for the help");
    }

    #[test]
    fn trailing_artifacts_are_stripped_from_a_partial() {
        let mut d = listening();
        d.partial_arrived("meet me at six [BLANK_AUDIO]");
        assert_eq!(d.partial(), "meet me at six");
    }

    #[test]
    fn an_artifact_in_the_middle_is_left_alone() {
        // Only the END is trimmed — text after an artifact is real speech, and
        // cutting there would lose it.
        let mut d = listening();
        d.partial_arrived("hello [BLANK_AUDIO] world");
        assert!(d.partial().contains("world"), "{}", d.partial());
    }

    #[test]
    fn a_partial_is_not_required_to_look_like_a_sentence() {
        // A partial is mid-word by nature; rejecting it for not looking like
        // speech would blank the preview every other frame.
        let mut d = listening();
        d.partial_arrived("navig");
        assert_eq!(d.partial(), "navig");
    }
}
}
