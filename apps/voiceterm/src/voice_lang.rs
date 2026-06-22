// src/voice_lang.rs
//
// Language configuration for voice input. v0.1.110 groundwork for
// multilingual support.
//
// Design:
//
// - Single configurable language code at runtime (default "en").
//   Set via WEB2QML_VOICE_LANG env var, or future Settings UI.
//   ISO 639-1 codes: "en", "es", "fr", "de", "it", "pt", "ja", "zh",
//   "ko", "ru", "nl", "pl", "tr", "ar", "hi", etc. (anything whisper
//   supports).
//
// - Whisper handles transcription multilingually NATIVELY when a
//   multilingual model is loaded (ggml-base.bin, ggml-small.bin,
//   ggml-medium.bin, ggml-large-v3.bin — all multilingual).
//   ggml-tiny.en.bin and ggml-base.en.bin are English-only.
//
//   For non-English users to work well, they need:
//   1. A multilingual model file present in ~/.config/web2qml/models/
//   2. The WEB2QML_VOICE_LANG env var set to their language code
//
//   Without (1), whisper hallucinates English on non-English speech.
//   Without (2), whisper auto-detects (works ~85% of the time, but
//   short utterances like "abrir terminal" can be misdetected).
//
// - Intent parsing is currently English-only (voice_intent::parse).
//   To support other languages, parallel modules
//   (voice_intent_es.rs, voice_intent_fr.rs, ...) would replicate
//   the keyword tables for each language. This module provides the
//   dispatcher hook (`parse_for_lang`) that routes to the right
//   parser based on `current()`.
//
//   Currently only English is implemented; non-English languages
//   fall back to the English parser, which works only for utterances
//   the user happens to speak in English (e.g. mixed-language users
//   who say "open Google" but switch to French for content).
//
// - Phonetic fingerprinting (MFCC-based) is language-AGNOSTIC. The
//   user can say "abrir terminal" in Spanish, confirm the intent
//   (which adds a fingerprint), and future utterances of the same
//   Spanish phrase will match the fingerprint regardless of language
//   parser. This is a useful escape hatch for unsupported languages.
//
// Adding a new language:
//   1. Add an arm to `whisper_lang_code()` if needed (most ISO codes
//      pass through directly to whisper)
//   2. Create voice_intent_<code>.rs with a `parse_<code>(text: &str)
//      -> Intent` function mirroring voice_intent::parse
//   3. Add a dispatch arm in `parse_for_lang()` below
//   4. Add language-specific filler seeds if the language has
//      different disfluencies (e.g. Spanish "este", French "euh")
//
// Privacy: the language code is logged on transcription. No
// concerns since it's user-configured.

use std::sync::OnceLock;

/// Cached current language code. Set on first call to current(),
/// re-read on cache miss.
static CURRENT_LANG: OnceLock<String> = OnceLock::new();

/// Return the configured language code (ISO 639-1 lowercase, e.g.
/// "en", "es"). Defaults to "en" if WEB2QML_VOICE_LANG is unset
/// or empty.
pub fn current() -> &'static str {
    CURRENT_LANG.get_or_init(|| {
        let raw = std::env::var("WEB2QML_VOICE_LANG").unwrap_or_default();
        let normalized = raw.trim().to_lowercase();
        if normalized.is_empty() {
            "en".to_string()
        } else {
            // Take the first 2 chars (handle "en-US" → "en").
            normalized.chars().take(2).collect::<String>()
        }
    })
}

/// Whisper language code. Maps our ISO 639-1 to whisper's expected
/// format (which is also ISO 639-1 in most cases — they accept "en",
/// "es", "fr", etc directly). For "auto-detect" pass None to
/// whisper params.
///
/// Returns Some(code) for known languages, None for "auto" (whisper
/// auto-detect). When None, whisper guesses from the audio — useful
/// for users who routinely switch languages.
pub fn whisper_lang_code() -> Option<&'static str> {
    let lang = current();
    if lang == "auto" {
        return None;
    }
    // Whisper supports all ISO 639-1 codes for the languages in its
    // training set. Pass our code through directly for these.
    // We hardcode the list to defend against typo'd env values
    // (e.g. WEB2QML_VOICE_LANG=english instead of en) — those would
    // confuse whisper if passed verbatim.
    const SUPPORTED: &[&str] = &[
        "en", "es", "fr", "de", "it", "pt", "nl", "pl", "tr",
        "ru", "uk", "cs", "sv", "da", "no", "fi", "el",
        "ja", "zh", "ko", "vi", "th", "id", "ms",
        "ar", "he", "hi", "bn", "ta", "te", "ur", "fa",
        "ca", "ro", "hu", "bg", "sk", "sl", "hr", "sr",
        "et", "lv", "lt", "mt", "ga", "cy", "is", "mk",
        "sq", "az", "kk", "uz", "tg", "mn", "my", "km",
        "lo", "si", "gl", "eu", "be", "hy", "ka",
    ];
    if SUPPORTED.iter().any(|&s| s == lang) {
        // Match by static string so we can return &'static str.
        SUPPORTED.iter().find(|&&s| s == lang).copied()
    } else {
        // Unknown lang — fall back to None (auto-detect).
        crate::clog!("voice-lang", "unknown language code '{}', auto-detecting", lang);
        None
    }
}

/// Dispatch text → Intent to the right per-language parser.
///
/// Currently only "en" has a real implementation. Other languages
/// fall back to the English parser. Mixed-language users (who say
/// e.g. "abrir google" — Spanish "abrir" + English "google") get
/// reasonable results from the English parser if "abrir" is
/// transcribed correctly by whisper, since "google" carries enough
/// signal even if the verb is unrecognized. But the better path is
/// to add a dedicated voice_intent_es module.
pub fn parse_for_lang(text: &str) -> crate::voice_intent::Intent {
    match current() {
        // English: native parser
        "en" => crate::voice_intent::parse(text),
        // Spanish: dedicated parser, falls back to English internally
        // for unrecognized Spanish utterances.
        "es" => crate::voice_intent_es::parse_es(text),
        // French: dedicated parser, falls back to English internally.
        "fr" => crate::voice_intent_fr::parse_fr(text),
        // Italian: dedicated parser (v0.1.872), falls back to English.
        "it" => crate::voice_intent_it::parse_it(text),
        // Portuguese: dedicated parser (v0.1.872), falls back to English.
        "pt" => crate::voice_intent_pt::parse_pt(text),
        // German: dedicated parser (v0.1.873), falls back to English.
        "de" => crate::voice_intent_de::parse_de(text),
        // Other languages: fall back to English. Language-specific
        // parsers will be wired here as they're added.
        _ => crate::voice_intent::parse(text),
    }
}

/// True if a multilingual whisper model is available on disk.
///
/// Used by the diagnostic / settings UI to warn users who set
/// WEB2QML_VOICE_LANG to a non-English language but only have
/// English-only models installed (which would silently produce
/// English hallucinations).
pub fn multilingual_model_available() -> bool {
    let dir = match std::env::var("HOME") {
        Ok(h) => std::path::PathBuf::from(h).join(".config/web2qml/models"),
        Err(_) => return false,
    };
    // Multilingual model file names (from whisper.cpp distribution).
    // Any of these means we can transcribe non-English.
    const MULTILINGUAL: &[&str] = &[
        "ggml-base.bin",
        "ggml-base-q5_1.bin",
        "ggml-small.bin",
        "ggml-small-q5_1.bin",
        "ggml-medium.bin",
        "ggml-medium-q5_0.bin",
        "ggml-large-v3.bin",
        "ggml-large-v3-q5_0.bin",
        "ggml-large-v3-turbo.bin",
        "ggml-large-v3-turbo-q5_0.bin",
    ];
    MULTILINGUAL.iter().any(|f| dir.join(f).exists())
}
