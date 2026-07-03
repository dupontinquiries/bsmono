//! grammar — the brain between whisper and routing: transcribed text → structured
//! [`Intent`].
//!
//! The raw mic + whisper transcription (device + model I/O) is abstracted away behind
//! [`Transcriber`]; everything here is pure and testable. The pipeline for one utterance:
//!
//! 1. **clean** it (voice-kit: strip trailing artifacts, trim runaway repeat-loops),
//! 2. **gate** it (voice-kit: drop hallucinations / non-speech),
//! 3. **system phrases** first ("switch to X", "open Y in X") — universal, focus-independent,
//! 4. **app grammars** — the focused app's rules win over background apps'.
//!
//! An app registers rules like `play {track}` → event `play_track`; the leading literal
//! words must match and a trailing `{slot}` captures the rest.

use crate::Intent;

/// One grammar rule: leading literal words, an optional trailing capture slot, and the
/// event it fires.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rule {
    words: Vec<String>,
    slot: Option<String>,
    pub event_type: String,
}

impl Rule {
    /// Parse a rule from wire tokens: the phrase tokens (e.g. `["play", "{track}"]`) plus
    /// the event type. A single `{name}` token (must be last) becomes the capture slot.
    pub fn parse(event_type: &str, phrase_tokens: &[String]) -> Rule {
        let mut words = Vec::new();
        let mut slot = None;
        for (i, tok) in phrase_tokens.iter().enumerate() {
            if let Some(name) = tok.strip_prefix('{').and_then(|t| t.strip_suffix('}')) {
                slot = Some(name.to_string());
                // anything after a slot is ignored (slot captures the tail)
                let _ = i;
                break;
            }
            words.push(normalize(tok));
        }
        Rule { words, slot, event_type: event_type.to_string() }
    }

    /// Try to match a normalized word list; on success returns the captured slots (0 or 1).
    fn match_words(&self, toks: &[String]) -> Option<Vec<String>> {
        if toks.len() < self.words.len() {
            return None;
        }
        if self.words.iter().zip(toks).any(|(a, b)| a != b) {
            return None;
        }
        let rest = &toks[self.words.len()..];
        match &self.slot {
            None => {
                if rest.is_empty() {
                    Some(vec![])
                } else {
                    None // extra words with no slot → not a match
                }
            }
            Some(_) => {
                if rest.is_empty() {
                    None // slot present but nothing to capture
                } else {
                    Some(vec![rest.join(" ")])
                }
            }
        }
    }
}

/// One app's grammar (its rules).
#[derive(Debug, Clone, Default)]
pub struct Grammar {
    pub rules: Vec<Rule>,
}

impl Grammar {
    pub fn add(&mut self, rule: Rule) {
        self.rules.push(rule);
    }
    fn match_toks(&self, toks: &[String]) -> Option<(String, Vec<String>)> {
        for r in &self.rules {
            if let Some(slots) = r.match_words(toks) {
                return Some((r.event_type.clone(), slots));
            }
        }
        None
    }
}

/// The outcome of matching an utterance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Match {
    /// A system intent (switch/open) — route directly.
    System(Intent),
    /// An app-domain intent discovered from `app`'s grammar.
    App { event_type: String, slots: Vec<String> },
    /// Cleaned to nothing / hallucination / no rule matched.
    None,
}

/// Holds per-app grammars + which app is active, and turns text into a [`Match`].
#[derive(Default)]
pub struct Matcher {
    grammars: std::collections::HashMap<String, Grammar>,
    active: Option<String>,
}

impl Matcher {
    pub fn new() -> Self {
        Matcher::default()
    }
    pub fn set_active(&mut self, app: &str) {
        self.active = Some(app.to_string());
    }
    /// Register a rule for an app (`play {track}` → `play_track`).
    pub fn register_rule(&mut self, app: &str, event_type: &str, phrase_tokens: &[String]) {
        self.grammars.entry(app.to_string()).or_default().add(Rule::parse(event_type, phrase_tokens));
    }
    pub fn remove_app(&mut self, app: &str) {
        self.grammars.remove(app);
    }

    /// Full pipeline: clean → gate → system → active app → other apps.
    pub fn match_utterance(&self, raw: &str) -> Match {
        // 1. clean (voice-kit)
        let cleaned = voice_kit::hallucination::trim_hallucination_loop(
            &voice_kit::hallucination::strip_trailing_artifacts(raw),
        );
        // 2. gate
        if voice_kit::hallucination::is_hallucination(&cleaned) || !voice_kit::hallucination::looks_like_speech(&cleaned) {
            return Match::None;
        }
        // 3. system phrases (focus-independent)
        if let Some(intent) = Intent::parse_system(&cleaned) {
            return Match::System(intent);
        }
        // 4. app grammars — active app first, then the rest (deterministic)
        let toks: Vec<String> = cleaned.split_whitespace().map(normalize).collect();
        if let Some(a) = &self.active {
            if let Some(g) = self.grammars.get(a) {
                if let Some((et, slots)) = g.match_toks(&toks) {
                    return Match::App { event_type: et, slots };
                }
            }
        }
        let mut others: Vec<&String> = self.grammars.keys().filter(|k| Some(*k) != self.active.as_ref()).collect();
        others.sort();
        for app in others {
            if let Some((et, slots)) = self.grammars[app].match_toks(&toks) {
                return Match::App { event_type: et, slots };
            }
        }
        Match::None
    }
}

/// The raw audio→text seam. The real implementation wraps voice-kit model resolution and a
/// whisper backend (voiceterm's whisper pool); tests and headless use supply a mock. Kept
/// out of the pure logic so the daemon can be built and tested without an audio device.
pub trait Transcriber {
    /// Block for the next finalized transcript, or `None` when the source ends.
    fn next_transcript(&mut self) -> Option<String>;
}

fn normalize(s: &str) -> String {
    s.trim().to_lowercase().chars().filter(|c| c.is_alphanumeric() || *c == ' ').collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn matcher() -> Matcher {
        let mut m = Matcher::new();
        m.register_rule("logscope", "next_page", &["next".into(), "page".into()]);
        m.register_rule("beatspace", "play_track", &["play".into(), "{track}".into()]);
        m
    }

    #[test]
    fn literal_and_slot_rules() {
        let m = matcher();
        m_assert(m.match_utterance("next page"), Match::App { event_type: "next_page".into(), slots: vec![] });
        m_assert(
            m.match_utterance("play koyopper drums"),
            Match::App { event_type: "play_track".into(), slots: vec!["koyopper drums".into()] },
        );
    }

    #[test]
    fn system_phrases_beat_app_grammars() {
        let m = matcher();
        m_assert(m.match_utterance("switch to beatspace"), Match::System(Intent::SwitchApp { app: "beatspace".into() }));
        m_assert(
            m.match_utterance("open run.log in logscope"),
            Match::System(Intent::OpenDoc { app: "logscope".into(), document: "run.log".into() }),
        );
    }

    #[test]
    fn hallucinations_and_noise_drop_to_none() {
        let m = matcher();
        assert_eq!(m.match_utterance("  "), Match::None);
        // a classic whisper silence-hallucination is filtered by voice-kit
        assert_eq!(m.match_utterance("thanks for watching!"), Match::None);
        assert_eq!(m.match_utterance("qqqq"), Match::None); // no rule
    }

    #[test]
    fn active_app_wins_on_ambiguity() {
        let mut m = Matcher::new();
        m.register_rule("logscope", "scroll", &["down".into()]);
        m.register_rule("unendlich", "move_down", &["down".into()]);
        m.set_active("unendlich");
        m_assert(m.match_utterance("down"), Match::App { event_type: "move_down".into(), slots: vec![] });
        m.set_active("logscope");
        m_assert(m.match_utterance("down"), Match::App { event_type: "scroll".into(), slots: vec![] });
    }

    fn m_assert(got: Match, want: Match) {
        assert_eq!(got, want);
    }
}
