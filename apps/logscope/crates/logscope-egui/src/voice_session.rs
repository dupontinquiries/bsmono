//! voice_session.rs — the single voice API surface the app holds and pumps.
//!
//! One `VoiceSession` owns the whole live voice pipeline: mic capture (`voice::VoiceCapture`) → the
//! streaming `Transcriber` (Pass → finalized `Utterance`) → the barge-in `Reactor` (interrupt/steer while
//! the model is speaking). The app calls `poll()` once per frame and gets a `VoiceTick`. That's the entire
//! contract — every future voice feature (graph navigation, branching, multimodal, commands) is built on
//! `VoiceTick`, never on the capture/whisper plumbing underneath. Keep this surface small and stable.
//!
//! Stub-safe: with no `voice` feature the underlying `VoiceCapture`/`poll_passes` are inert, so this
//! compiles and links in every build; `start()` simply errors when voice isn't compiled in.

use crate::reaction::{CancelFlag, Reaction, Reactor};
use crate::voice_ingest::Transcriber;
use crate::voicefan::Utterance;

/// The result of one `poll()`. Everything the app needs to react to speech this frame.
#[derive(Default)]
pub struct VoiceTick {
    /// Utterances that reached an endpoint this frame — turn these into chat turns or commands.
    pub utterances: Vec<Utterance>,
    /// Latest in-progress transcription (volatile tail) for live UI, if it changed this frame.
    pub partial: Option<String>,
    /// A barge-in verdict (Interrupt / Steer / …) — only while reactions are armed (model generating).
    pub reaction: Option<Reaction>,
}

/// The live voice session. Held as `Option<VoiceSession>` by the app; `None` = mic off. Normally a single
/// anonymous capture; set LOGSCOPE_SPEAKERS="Alice:USB Headset,Bob:Built-in Mic" (name:mic-substring pairs,
/// comma-separated) to run MULTIPLE mics concurrently, each tagged to its own speaker — e.g. two people on
/// separate headsets. Utterances from every capture feed the same shared whisper pipeline and downstream
/// Transcriber/Reactor, so multi-speaker branching (`voicefan`) sees a proper mix without special-casing.
pub struct VoiceSession {
    captures: Vec<crate::voice::VoiceCapture>,
    transcriber: Transcriber,
    reactor: Reactor,
    partial: String,
    armed: bool,
}

impl VoiceSession {
    /// Load whisper (once) and open mic(s). Single anonymous mic by default; LOGSCOPE_SPEAKERS opens one
    /// capture per named speaker instead. Errors with a human message if there's no model/mic or the build
    /// has no `voice` feature.
    pub fn start() -> Result<Self, String> {
        let model = crate::voice::model_path();
        crate::voice::init(&model)?;
        let captures = match parse_speakers_env() {
            Some(specs) => {
                let mut caps = Vec::with_capacity(specs.len());
                let mut errs = Vec::new();
                for (name, mic) in specs {
                    match crate::voice::VoiceCapture::start_with_mic(name.clone(), Some(&mic)) {
                        Ok(c) => { crate::clog!("voice", "speaker '{name}' -> mic '{mic}'"); caps.push(c); }
                        Err(e) => errs.push(format!("{name} ({mic}): {e}")),
                    }
                }
                if caps.is_empty() { return Err(format!("no LOGSCOPE_SPEAKERS mic opened: {}", errs.join("; "))); }
                if !errs.is_empty() { crate::clog!("voice", "some speakers failed to open: {}", errs.join("; ")); }
                caps
            }
            None => vec![crate::voice::VoiceCapture::start("")?],
        };
        Ok(Self {
            captures,
            transcriber: Transcriber::new(2), // finalize utterances of ≥2 chars
            reactor: Reactor::new(3),         // commit a barge-in after a few consistent fragments
            partial: String::new(),
            armed: false,
        })
    }

    /// Arm/disarm barge-in detection. Call `arm(true)` when the model starts generating and `arm(false)`
    /// when it stops; while armed, `poll()` may return a `reaction` (Interrupt/Steer) and the cancel flag
    /// below flips so the generation loop can abort.
    pub fn arm(&mut self, on: bool) {
        if on == self.armed { return; }
        self.armed = on;
        if on { self.reactor.begin(); } else { self.reactor.end(); }
    }

    /// A cheap-to-clone flag the generation loop polls to abort on an interrupt/steer barge-in.
    pub fn cancel_flag(&self) -> CancelFlag { self.reactor.cancel_flag() }

    /// The current in-progress transcription (for a live "listening…" readout). With multiple speakers
    /// this is whichever produced the most recent partial — good enough for one-speaker-at-a-time turns.
    pub fn partial(&self) -> &str { &self.partial }

    /// How many mics are currently active (1 in the common case; >1 under LOGSCOPE_SPEAKERS).
    pub fn speaker_count(&self) -> usize { self.captures.len() }

    /// Pump one frame: mic(s) → whisper → passes → {finalized utterances, partial preview, barge-in
    /// reaction}. Call once per UI frame while the mic is on. All captures share one whisper pipeline, so
    /// draining passes once per frame (not per capture) picks up whichever speaker(s) produced results.
    pub fn poll(&mut self) -> VoiceTick {
        for c in &mut self.captures { c.poll(); } // drain each mic buffer → VAD → submit to the whisper pool
        let mut tick = VoiceTick::default();
        for pass in crate::voice::poll_passes() {
            if pass.full_text != self.partial {
                self.partial = pass.full_text.clone();
                tick.partial = Some(self.partial.clone());
            }
            // Barge-in first (returns None unless armed), then fold the pass into finalized utterances.
            if let Some(r) = self.reactor.observe(&pass.full_text, pass.stable) {
                tick.reaction = Some(r);
            }
            tick.utterances.extend(self.transcriber.push(pass));
        }
        if !tick.utterances.is_empty() { self.partial.clear(); } // an endpoint clears the volatile tail
        tick
    }
}

/// Parse LOGSCOPE_SPEAKERS="Name1:mic-substring1,Name2:mic-substring2". Returns None if unset/empty (the
/// common single-mic case); a malformed entry (missing ':') is skipped with a log line, not a hard error.
fn parse_speakers_env() -> Option<Vec<(String, String)>> {
    let raw = std::env::var("LOGSCOPE_SPEAKERS").ok()?;
    let mut out = Vec::new();
    for part in raw.split(',') {
        let part = part.trim();
        if part.is_empty() { continue; }
        match part.split_once(':') {
            Some((name, mic)) if !name.trim().is_empty() && !mic.trim().is_empty() =>
                out.push((name.trim().to_string(), mic.trim().to_string())),
            _ => crate::clog!("voice", "LOGSCOPE_SPEAKERS: skipping malformed entry {part:?} (want Name:mic-substring)"),
        }
    }
    if out.is_empty() { None } else { Some(out) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn speakers_env_parses_pairs() {
        std::env::set_var("LOGSCOPE_SPEAKERS", "Alice:USB Headset,Bob:Built-in Mic");
        let specs = parse_speakers_env().unwrap();
        assert_eq!(specs, vec![("Alice".to_string(), "USB Headset".to_string()), ("Bob".to_string(), "Built-in Mic".to_string())]);
        std::env::remove_var("LOGSCOPE_SPEAKERS");
    }

    #[test]
    fn speakers_env_absent_or_malformed() {
        std::env::remove_var("LOGSCOPE_SPEAKERS");
        assert!(parse_speakers_env().is_none(), "unset should be None (single-mic default)");
        std::env::set_var("LOGSCOPE_SPEAKERS", "not-a-pair, , Alice:Headset");
        assert_eq!(parse_speakers_env().unwrap(), vec![("Alice".to_string(), "Headset".to_string())], "malformed entries are skipped, valid ones kept");
        std::env::remove_var("LOGSCOPE_SPEAKERS");
    }
}
