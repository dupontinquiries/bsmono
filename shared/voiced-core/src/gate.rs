//! gate — the "quick voice DB first" decision policy for the voice pipeline.
//!
//! The phonetic fingerprint DB (voiceterm's `phonetic`, rusty-board's equivalent) matches an utterance
//! in well under 10ms. Historically it was only a *preview*: whisper always ran and overwrote it. This
//! module makes the DB the DEFAULT:
//!
//!   * a **confident** match IS the transcript and RETIRES the whisper call for that utterance — the
//!     expensive read is skipped (dedup: the same known phrase is never re-transcribed);
//!   * a **weak** match stays a provisional preview while whisper confirms;
//!   * a **miss** falls through to whisper, whose confident result is stored back as a new fingerprint
//!     — self-supervised "magic profile" training, run via the voiced daemon's whisper pool.
//!
//! Pure and std-only: it takes the DB's `(label, similarity)` and whisper's `(text, confident)` and
//! returns a decision, so it's unit-testable without audio, whisper, or a GPU, and shared by every
//! front-end (voiceterm, rusty-board) and the daemon.

/// Similarity thresholds on the phonetic match score (higher = more similar, normalized to `[0, 1]`).
#[derive(Clone, Copy, Debug)]
pub struct Thresholds {
    /// At or above this, the DB match is authoritative and whisper is skipped (the dedup path).
    pub dedup: f32,
    /// At or above this (but below `dedup`), show a provisional preview; whisper still runs.
    pub preview: f32,
}

impl Default for Thresholds {
    /// DB-first defaults. `dedup` is deliberately high so only strong matches retire whisper; `preview`
    /// matches voiceterm's historical 0.55 preview cutoff.
    fn default() -> Self {
        Self { dedup: 0.80, preview: 0.55 }
    }
}

/// What the pipeline should do with an utterance, given its best phonetic-DB match.
#[derive(Clone, Debug, PartialEq)]
pub enum Decision {
    /// Confident DB hit: emit `label` as the final transcript and SKIP whisper (the dedup path).
    Dedup(String),
    /// Weak DB hit: show `label` as a provisional preview; whisper still runs and may replace it.
    Preview(String),
    /// No usable DB match: run whisper.
    Whisper,
}

impl Decision {
    /// True when this decision retires the whisper call for the utterance.
    pub fn skips_whisper(&self) -> bool {
        matches!(self, Decision::Dedup(_))
    }
    /// The label to surface now, if any (final for `Dedup`, provisional for `Preview`).
    pub fn label(&self) -> Option<&str> {
        match self {
            Decision::Dedup(l) | Decision::Preview(l) => Some(l),
            Decision::Whisper => None,
        }
    }
}

/// Classify the best phonetic-DB match into a pipeline decision. `best` is the DB's top
/// `(label, similarity)`, or `None` if the DB had nothing.
pub fn decide(best: Option<(&str, f32)>, t: &Thresholds) -> Decision {
    match best {
        Some((label, score)) if !label.is_empty() && score >= t.dedup => Decision::Dedup(label.to_string()),
        Some((label, score)) if !label.is_empty() && score >= t.preview => Decision::Preview(label.to_string()),
        _ => Decision::Whisper,
    }
}

/// After whisper transcribes an utterance the DB did NOT already know confidently, decide whether to
/// store that text as a new fingerprint — the self-supervised training step. Kept conservative so the
/// DB is never poisoned: only clean, confident, command-length text enriches it.
pub fn should_enrich(whisper_text: &str, whisper_confident: bool, db: &Decision) -> bool {
    if !whisper_confident || db.skips_whisper() {
        return false; // low confidence, or the DB already owned this utterance
    }
    let t = whisper_text.trim();
    !t.is_empty()
        && t.split_whitespace().count() <= 8 // a command/phrase, not a paragraph
        && !t.chars().any(|c| c.is_control()) // no garbage control chars
        && t.chars().any(|c| c.is_alphabetic()) // has real words, not just punctuation/numbers
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn confident_match_dedups_and_skips_whisper() {
        let d = decide(Some(("open mail", 0.86)), &Thresholds::default());
        assert_eq!(d, Decision::Dedup("open mail".into()));
        assert!(d.skips_whisper());
        assert_eq!(d.label(), Some("open mail"));
    }

    #[test]
    fn weak_match_is_a_preview_and_still_whispers() {
        let d = decide(Some(("open mail", 0.6)), &Thresholds::default());
        assert_eq!(d, Decision::Preview("open mail".into()));
        assert!(!d.skips_whisper());
    }

    #[test]
    fn miss_empty_and_none_all_fall_through_to_whisper() {
        let t = Thresholds::default();
        assert_eq!(decide(Some(("x", 0.4)), &t), Decision::Whisper);
        assert_eq!(decide(Some(("", 0.99)), &t), Decision::Whisper); // empty label never dedups
        assert_eq!(decide(None, &t), Decision::Whisper);
    }

    #[test]
    fn thresholds_are_configurable() {
        let strict = Thresholds { dedup: 0.95, preview: 0.7 };
        assert_eq!(decide(Some(("cmd", 0.86)), &strict), Decision::Preview("cmd".into()));
    }

    #[test]
    fn enrich_stores_confident_command_length_text_on_a_miss() {
        let miss = Decision::Whisper;
        assert!(should_enrich("git status", true, &miss));
        assert!(should_enrich("  open the file  ", true, &miss)); // trimmed, still short
    }

    #[test]
    fn enrich_rejects_low_confidence_dedup_garbage_and_prose() {
        assert!(!should_enrich("git status", false, &Decision::Whisper)); // not confident
        assert!(!should_enrich("open mail", true, &Decision::Dedup("open mail".into()))); // DB knew it
        assert!(!should_enrich("", true, &Decision::Whisper)); // empty
        assert!(!should_enrich("\x07\x00", true, &Decision::Whisper)); // control-char garbage
        assert!(!should_enrich("12345", true, &Decision::Whisper)); // no real words
        let prose = "this is a long sentence that is clearly not a short voice command at all";
        assert!(!should_enrich(prose, true, &Decision::Whisper)); // too long for a fingerprint label
    }
}
