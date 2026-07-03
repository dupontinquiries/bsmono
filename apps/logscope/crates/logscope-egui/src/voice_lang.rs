//! voice_lang.rs — language dispatch for voice-intent parsing, ported from voiceterm's voice_lang. One env
//! var (LOGSCOPE_STT_LANG — the same one whisper's transcription language uses) picks both what whisper
//! transcribes AND which command-intent parser handles the result, so setting a language once covers both.
//!
//! Only a few languages have a dedicated parser (see `parse_for_lang`); everything else — including
//! "auto" — falls back to the English parser. This mirrors voiceterm's own honest scope: real dedicated
//! parsers for a handful of languages, not a mechanical stub for every ISO code.

use crate::voice_intent::VoiceIntent;
use std::sync::OnceLock;

/// The configured language (ISO 639-1, lowercase, e.g. "en", "es"). Reads LOGSCOPE_STT_LANG; "auto" or
/// unset defaults to "en" for intent parsing (whisper itself may still auto-detect the audio language
/// independently — this only decides which COMMAND VOCABULARY to try first).
pub fn current() -> &'static str {
    static LANG: OnceLock<String> = OnceLock::new();
    LANG.get_or_init(|| {
        let raw = std::env::var("LOGSCOPE_STT_LANG").unwrap_or_default().trim().to_lowercase();
        if raw.is_empty() || raw == "auto" { "en".to_string() } else { raw.chars().take(2).collect() }
    }).as_str()
}

/// Dispatch text to the right per-language parser. Dedicated parsers fall back to the English parser
/// internally for anything they don't recognize (so a stray English word or an unmatched phrase still
/// resolves), matching voiceterm's mixed-language-friendly design.
pub fn parse_for_lang(text: &str) -> VoiceIntent {
    match current() {
        "es" => crate::voice_intent_es::parse(text),
        "fr" => crate::voice_intent_fr::parse(text),
        "de" => crate::voice_intent_de::parse(text),
        _ => crate::voice_intent::parse(text), // English, and the honest fallback for every other language
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dispatch_falls_back_to_english_by_default() {
        // LOGSCOPE_STT_LANG is unset in the test environment, so current() is "en" and English commands
        // resolve normally through the dispatcher (not just the underlying parser).
        assert!(matches!(parse_for_lang("go to twelve"), VoiceIntent::Command(_)));
        assert!(matches!(parse_for_lang("branch here"), VoiceIntent::Command(_)));
    }
}
