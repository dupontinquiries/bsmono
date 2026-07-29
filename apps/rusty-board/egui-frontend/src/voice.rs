//! voice — the port's dictation service.
//!
//! rusty-board's voice stack (`src/voice/`: recorder, transcriber, service,
//! model discovery, hallucination filtering) is GTK-free, so the port mounts it
//! by `#[path]` rather than reimplementing it — the same decision as `config`,
//! and for the same reason: one implementation, no drift, and a fix to the
//! recorder helps both front ends.
//!
//! What this module adds is the layer the GTK build does *not* have: driving
//! [`Dictation`] so partials replace rather than accumulate, and so the
//! utterance is committed exactly once. See `dictation.rs` for why that is the
//! whole of the double-transcription fix.

#[cfg(feature = "voice")]
#[path = "../../src/voice/mod.rs"]
pub mod engine;

use crate::dictation::{partial_window, Dictation, Mode, State};

/// Drives the engine's voice service through the dictation state machine.
///
/// The GTK front end polls partials on a glib timer and runs a separate final
/// pass on release; this polls the same partials but treats the last one as the
/// result, which is both faster and incapable of producing the second copy.
pub struct DictationService {
    pub dictation: Dictation,
    /// Samples captured this utterance, for the bounded partial window.
    captured: usize,
    rate: u32,
}

impl DictationService {
    pub fn new(rate: u32) -> DictationService {
        DictationService { dictation: Dictation::default(), captured: 0, rate }
    }

    /// Choose whether to run the extra accuracy pass. Defaults to off, because
    /// on real use the partials are accurate and fast enough to be the output.
    pub fn set_mode(&mut self, mode: Mode) { self.dictation.mode = mode; }

    pub fn start(&mut self) {
        self.captured = 0;
        self.dictation.start();
    }

    /// Note how much audio exists so far, so the next partial can be windowed.
    pub fn captured(&mut self, samples: usize) { self.captured = samples; }

    /// The range of captured audio the next partial should transcribe.
    ///
    /// Bounded rather than "everything so far": re-reading the whole ring every
    /// interval is what makes partials fall behind the speaker on a phone, and
    /// on the FP5 in particular that is the difference between usable and not.
    pub fn next_window(&self) -> std::ops::Range<usize> {
        partial_window(self.captured, self.rate)
    }

    /// A partial came back from the transcriber.
    pub fn partial(&mut self, text: &str) { self.dictation.partial_arrived(text); }

    /// What to show live, greyed, under the caret.
    pub fn preview(&self) -> &str { self.dictation.partial() }

    /// The user released the key.
    ///
    /// `Some(text)` in the default mode — commit it and you are done, with no
    /// second inference and no wait. `None` in `FinalPass`, where the caller
    /// runs the extra pass and feeds `final_text`.
    pub fn release(&mut self) -> Option<String> { self.dictation.finish() }

    /// The final pass came back (only in `FinalPass`). The returned `Commit`
    /// carries what was already displayed so the caller can REPLACE it rather
    /// than inserting a second copy.
    pub fn final_text(&mut self, text: &str) -> Option<crate::dictation::Commit> {
        self.dictation.final_arrived(text)
    }

    pub fn cancel(&mut self) { self.dictation.cancel(); }
    pub fn state(&self) -> State { self.dictation.state() }
    pub fn is_active(&self) -> bool { self.dictation.is_active() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_whole_utterance_commits_once_with_no_second_inference() {
        let mut s = DictationService::new(16_000);
        s.start();
        for (n, p) in ["turn", "turn the", "turn the lights on"].iter().enumerate() {
            s.captured((n + 1) * 8_000);
            s.partial(p);
        }
        assert_eq!(s.preview(), "turn the lights on");
        assert_eq!(s.release().as_deref(), Some("turn the lights on"));
        assert_eq!(s.state(), State::Idle, "no transcribing wait at all");
        assert_eq!(s.release(), None, "and it cannot commit a second time");
    }

    #[test]
    fn the_window_stops_growing_once_the_utterance_is_long() {
        let mut s = DictationService::new(16_000);
        s.start();
        s.captured(4 * 16_000); // 4 s
        assert_eq!(s.next_window(), 0..64_000, "short: read all of it");
        s.captured(40 * 16_000); // 40 s
        let w = s.next_window();
        assert!(w.start > 0, "long: only the tail is re-read");
        assert_eq!(w.end, 40 * 16_000);
        assert!(w.len() < 20 * 16_000, "and the work per partial is bounded");
    }

    #[test]
    fn the_final_pass_path_still_replaces() {
        let mut s = DictationService::new(16_000);
        s.set_mode(Mode::FinalPass);
        s.start();
        s.partial("turn the lights on");
        assert_eq!(s.release(), None, "deferred to the final pass");
        let c = s.final_text("Turn the lights on.").expect("commits");
        assert_eq!(c.text, "Turn the lights on.");
        assert_eq!(c.replaces, "turn the lights on", "remove the preview, do not append");
    }
}
