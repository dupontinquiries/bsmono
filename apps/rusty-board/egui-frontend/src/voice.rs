//! voice — dictation for the port, with double-transcription designed out.
//!
//! The GTK pipeline works well and has one reported fault: it double
//! transcribes. Reading it, there are two distinct mechanisms that produce that
//! symptom, and they need different fixes:
//!
//! **1. The streaming worker re-transcribes everything, every time.** It
//! snapshots the *whole* ring buffer each interval, so at 600 ms cadence a
//! ten-second utterance is transcribed from the start on every tick — sixteen
//! inferences over the same audio, the last one covering all of it. That is
//! literally transcribing the same speech repeatedly, and on a phone it is also
//! where the battery goes. [`Utterance::pending_window`] returns only the audio
//! not yet covered by a committed partial, so each inference does new work.
//!
//! **2. A partial and the final can both reach the text field.** The partial is
//! meant to be display-only, but any path that commits one and then commits the
//! final emits the same words twice. Here that is impossible by construction:
//! [`Dictation::partial`] cannot produce text to insert — only
//! [`Dictation::finish`] returns a [`Commit`], and it returns one exactly once
//! per utterance.
//!
//! The third protection is dedup at the seam. Even with the above, a final
//! transcription of the whole utterance will usually *restate* what the last
//! partial already showed. If the UI has been inserting as it goes (some hosts
//! want that), [`dedup_overlap`] trims the repeated prefix rather than pasting
//! it again — the same "hello worldhello world" the user would otherwise see.

/// Where a dictation session is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Phase {
    #[default]
    Idle,
    Recording,
    /// Audio captured, final inference running.
    Transcribing,
}

/// Text that should actually be inserted. Only `finish` produces one.
#[derive(Debug, Clone, PartialEq)]
pub struct Commit {
    pub text: String,
}

/// The dictation state machine.
#[derive(Debug, Clone, Default)]
pub struct Dictation {
    phase: Phase,
    /// Last partial, for display only.
    display: String,
    /// Whether this utterance has already produced its commit. The guard against
    /// a late worker result landing after the final and emitting again.
    committed: bool,
    /// Utterance counter, so a result from a previous utterance arriving late is
    /// recognisable as stale rather than applied to the current one.
    utterance: u64,
}

impl Dictation {
    pub fn phase(&self) -> Phase { self.phase }
    pub fn display_text(&self) -> &str { &self.display }
    pub fn utterance_id(&self) -> u64 { self.utterance }
    pub fn is_recording(&self) -> bool { self.phase == Phase::Recording }

    /// Begin. Returns the id this utterance's results must carry.
    pub fn start(&mut self) -> u64 {
        self.utterance += 1;
        self.phase = Phase::Recording;
        self.display.clear();
        self.committed = false;
        self.utterance
    }

    /// A streaming partial. Display only — deliberately returns nothing, so
    /// there is no way for a caller to insert it and then insert the final too.
    pub fn partial(&mut self, utterance: u64, text: &str) {
        if utterance != self.utterance || self.phase != Phase::Recording {
            return; // stale, or we already stopped listening
        }
        self.display = text.to_string();
    }

    /// The user released the key. Audio stops; inference is still to come.
    pub fn stop(&mut self) {
        if self.phase == Phase::Recording {
            self.phase = Phase::Transcribing;
        }
    }

    /// The final result. Produces the one and only commit for this utterance.
    ///
    /// A second call, or a result carrying a stale utterance id, returns `None`
    /// — which is the guard against the streaming worker finishing an inference
    /// *after* the final one has landed and emitting the same words again.
    pub fn finish(&mut self, utterance: u64, text: &str) -> Option<Commit> {
        if utterance != self.utterance || self.committed {
            return None;
        }
        self.committed = true;
        self.phase = Phase::Idle;
        self.display.clear();
        let t = text.trim();
        if t.is_empty() { return None; }
        Some(Commit { text: t.to_string() })
    }

    /// Abandon without inserting anything.
    pub fn cancel(&mut self) {
        self.phase = Phase::Idle;
        self.display.clear();
        self.committed = true; // nothing further from this utterance may commit
    }
}

/// Which slice of captured audio still needs transcribing.
///
/// The GTK worker snapshots the whole ring each tick, so a long utterance is
/// re-transcribed from the beginning every interval. This returns the range not
/// yet covered, plus a small overlap so a word straddling the boundary is not
/// cut in half — the reason naive chunking produces "hel" "lo".
pub struct Utterance {
    /// Samples already covered by a completed partial.
    covered: usize,
    /// Overlap kept before the boundary, in samples.
    overlap: usize,
}

impl Utterance {
    pub fn new(overlap_samples: usize) -> Utterance {
        Utterance { covered: 0, overlap: overlap_samples }
    }

    /// Range of `total` samples to feed the next inference, or `None` when
    /// there is not enough new audio to be worth an inference.
    pub fn pending_window(&self, total: usize, min_new: usize) -> Option<(usize, usize)> {
        if total <= self.covered || total - self.covered < min_new {
            return None;
        }
        Some((self.covered.saturating_sub(self.overlap), total))
    }

    /// Mark audio as covered once its inference lands.
    pub fn advance(&mut self, to: usize) { self.covered = self.covered.max(to); }

    pub fn covered(&self) -> usize { self.covered }
    pub fn reset(&mut self) { self.covered = 0; }
}

/// Trim from `next` whatever `already` ends with, so appending does not repeat.
///
/// The seam fix: a final transcription usually restates what the last partial
/// showed, and a host that inserts as it goes would otherwise paste the overlap
/// twice. Compares on normalised words rather than raw characters, because
/// whisper varies punctuation and capitalisation between passes over the same
/// audio — "hello world" and "Hello world." must be recognised as the same
/// words or the dedup never fires.
pub fn dedup_overlap(already: &str, next: &str) -> String {
    let norm = |s: &str| -> Vec<String> {
        s.split_whitespace()
            .map(|w| w.chars().filter(|c| c.is_alphanumeric()).flat_map(|c| c.to_lowercase()).collect())
            .filter(|w: &String| !w.is_empty())
            .collect()
    };
    let a = norm(already);
    let b_words: Vec<&str> = next.split_whitespace().collect();
    let b = norm(next);
    if a.is_empty() || b.is_empty() { return next.trim().to_string(); }

    // longest suffix of `a` that is a prefix of `b`
    let max = a.len().min(b.len());
    let mut overlap = 0;
    for k in (1..=max).rev() {
        if a[a.len() - k..] == b[..k] {
            overlap = k;
            break;
        }
    }
    if overlap == 0 { return next.trim().to_string(); }
    b_words[overlap..].join(" ")
}
