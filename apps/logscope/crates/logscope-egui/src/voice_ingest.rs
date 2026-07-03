//! voice_ingest.rs — turn streaming whisper passes (attributed to a speaker) into clean, finalized
//! [`voicefan::Utterance`]s ready to fan out.
//!
//! whisper-streaming hands back the FULL re-transcription on every pass (the tail is volatile until an
//! endpoint), each pass tagged with whoever `speakers::identify()` matched. [`Transcriber`] holds the
//! latest full text for the current speaker-turn and finalizes one utterance when the turn ends — a
//! speaker change, a `stable` endpoint pass, or an explicit `flush` (silence timeout). On finalize it
//! applies a conservative speaker *claim* ("kessler here, open the report" → name `kessler`, text
//! `open the report`) and drops sub-threshold noise. [`fan_budget`] sizes the fanout cap from spare fleet
//! capacity and speaker count.
//!
//! Pure + unit-tested. The seams are upstream: mic capture, the whisper FFI, and `speakers::identify`
//! voiceprint attribution (adapt voiceterm's `web2qml` for those). Claim detection here is deliberately
//! minimal; production should resolve names against the registry (`speaker_claim::match_claim_to_registry`).
#![allow(dead_code)]

use crate::voicefan::{FanBudget, SpeakerId, Utterance};

/// One streaming transcription pass from whisper + speaker attribution. `full_text` is the whole current
/// re-transcription (not a delta); `stable` is true when whisper hit an endpoint / the text settled.
#[derive(Clone, Debug)]
pub struct Pass {
    pub speaker: SpeakerId,
    pub name: String,       // display name from attribution (a claim may override it)
    pub full_text: String,  // whisper's full re-transcription this pass
    pub stable: bool,       // endpoint reached → finalize the turn
    pub t_ms: u64,
}

/// Coalesces passes into finalized utterances, one per speaker-turn.
#[derive(Default)]
pub struct Transcriber {
    speaker: Option<SpeakerId>,
    name: String,
    text: String,
    t_ms: u64,
    min_chars: usize,
}

impl Transcriber {
    pub fn new(min_chars: usize) -> Self { Self { min_chars, ..Default::default() } }

    /// Feed one pass. Returns any utterances finalized by it (0, 1, or — speaker change *and* endpoint in
    /// one pass — 2: the completed prior turn, then the new one).
    pub fn push(&mut self, pass: Pass) -> Vec<Utterance> {
        let mut out = Vec::new();
        if self.speaker.as_ref().is_some_and(|s| *s != pass.speaker) {
            if let Some(u) = self.flush() { out.push(u); } // turn handed off to a new speaker
        }
        if self.speaker.is_none() {
            self.speaker = Some(pass.speaker.clone());
            self.name = pass.name.clone();
            self.t_ms = pass.t_ms;
        }
        self.text = pass.full_text;
        if pass.stable {
            if let Some(u) = self.flush() { out.push(u); }
        }
        out
    }

    /// Force-finalize the current turn (e.g. on a silence timeout). Returns it if it clears the threshold.
    pub fn flush(&mut self) -> Option<Utterance> {
        let text = std::mem::take(&mut self.text);
        let speaker = self.speaker.take()?;
        let name = std::mem::take(&mut self.name);
        let t = self.t_ms;
        let trimmed = text.trim();
        let (final_name, final_text) = match parse_claim(trimmed) {
            Some((claimed, remainder)) => (claimed, remainder),
            None => (name, trimmed.to_string()),
        };
        if final_text.chars().count() < self.min_chars { return None; }
        Some(Utterance { speaker, name: final_name, text: final_text, t_ms: t })
    }
}

/// Conservative speaker-claim parse: only the low-false-positive anchors `"<name> here …"` and
/// `"my name is <name> …"`. Returns `(name, remainder)`. Production should fuzzy-match the name against
/// the speaker registry rather than trust the raw token.
fn parse_claim(u: &str) -> Option<(String, String)> {
    let words: Vec<&str> = u.split_whitespace().collect();
    if words.len() < 2 { return None; }
    let lw: Vec<String> = words.iter().map(|w| w.to_lowercase()).collect();
    const STOP: &[&str] = &["over", "right", "come", "look", "wait", "stop", "not", "get",
                            "go", "in", "out", "up", "down", "back", "is", "are", "the", "a"];
    // "<name> here ..."
    if lw[1] == "here" {
        let name = clean(words[0]);
        if !name.is_empty() && !STOP.contains(&lw[0].as_str()) {
            return Some((name, words.get(2..).map(|w| w.join(" ")).unwrap_or_default()));
        }
    }
    // "my name is <name> ..."
    if lw.len() >= 4 && lw[0] == "my" && lw[1] == "name" && lw[2] == "is" {
        let name = clean(words[3]);
        if !name.is_empty() { return Some((name, words.get(4..).map(|w| w.join(" ")).unwrap_or_default())); }
    }
    None
}
fn clean(w: &str) -> String { w.chars().filter(|c| c.is_alphanumeric()).collect() }

/// Size the fanout cap: never plan more lanes than the fleet can run soon (`spare_slots`), and never more
/// than `per_speaker_cap` per active speaker.
pub fn fan_budget(spare_slots: usize, n_speakers: usize, per_speaker_cap: usize) -> FanBudget {
    FanBudget { max_total_lanes: spare_slots.min(n_speakers.max(1) * per_speaker_cap) }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pass(spk: &str, text: &str, stable: bool) -> Pass {
        Pass { speaker: spk.into(), name: spk.into(), full_text: text.into(), stable, t_ms: 0 }
    }

    #[test]
    fn partials_coalesce_and_finalize_once_on_endpoint() {
        let mut t = Transcriber::new(3);
        assert!(t.push(pass("spk_a", "parse", false)).is_empty());
        assert!(t.push(pass("spk_a", "parse the", false)).is_empty());
        let out = t.push(pass("spk_a", "parse the csv file", true));
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].text, "parse the csv file");
        assert_eq!(out[0].speaker, "spk_a");
    }

    #[test]
    fn speaker_change_finalizes_the_prior_turn() {
        let mut t = Transcriber::new(3);
        assert!(t.push(pass("spk_a", "open the terminal", false)).is_empty()); // not yet stable
        let out = t.push(pass("spk_b", "no wait", false));                     // b takes over → a flushes
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].speaker, "spk_a");
        assert_eq!(out[0].text, "open the terminal");
        // b's turn is still open
        let fin = t.flush().unwrap();
        assert_eq!(fin.speaker, "spk_b");
    }

    #[test]
    fn claim_renames_speaker_and_strips_prefix() {
        let mut t = Transcriber::new(3);
        let out = t.push(pass("spk_1", "kessler here open the report", true));
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].name, "kessler");
        assert_eq!(out[0].text, "open the report");
        assert_eq!(out[0].speaker, "spk_1", "durable id is untouched; only the display name changes");
    }

    #[test]
    fn claim_does_not_misfire_on_ordinary_speech() {
        // "over here ..." starts with a stopword, "i'm going ..." isn't an anchored pattern
        assert_eq!(parse_claim("over here is the file"), None);
        assert_eq!(parse_claim("i'm going to parse the csv"), None);
        assert_eq!(parse_claim("parse the csv per region"), None);
    }

    #[test]
    fn short_or_empty_turns_are_dropped() {
        let mut t = Transcriber::new(4);
        assert!(t.push(pass("spk_a", "ok", true)).is_empty(), "too short");
        // a pure introduction with no command (empty remainder after the claim) is dropped too
        assert!(t.push(pass("spk_a", "my name is alice", true)).is_empty());
    }

    #[test]
    fn fan_budget_caps_by_slots_and_speakers() {
        assert_eq!(fan_budget(8, 3, 2).max_total_lanes, 6);  // 3×2 < 8
        assert_eq!(fan_budget(4, 3, 2).max_total_lanes, 4);  // slots bind
        assert_eq!(fan_budget(10, 1, 4).max_total_lanes, 4); // one speaker, capped per-speaker
        assert_eq!(fan_budget(0, 5, 4).max_total_lanes, 0);  // no spare capacity → no fanout
    }
}
