//! voice_buttons: match a spoken phrase to an on-screen button so the
//! UI can HINT (animate the matched button) and optionally AUTO-FIRE
//! it (F9, see docs/design/voice_hinted_buttons.md).
//!
//! Mirrors the editor_voice/repl_voice matcher shape (normalize →
//! match phrase → action) but the output is "which visible button does
//! this target, with what confidence, and may it auto-fire." A central
//! registry keyed by a screen/context id lets one matcher serve every
//! button; QML just declares the context + renders the hint animation.
//! PURE: no UI — the backend emits (button_id, confidence) and QML
//! animates. First consumer: the new-tab confirm/deny dialog (F7).
//!
//! SAFETY: auto-fire is opt-in PER BUTTON and gated on a high
//! confidence threshold. Destructive buttons stay hint-only — a
//! misheard word must never auto-trigger something irreversible (the
//! same `confirm` instinct editor_voice has for quit-without-saving).

/// A button that responds to voice, in some screen context.
#[derive(Clone, Debug)]
pub struct VoiceButton {
    pub id: String,
    /// Phrases that target this button (lowercase, no filler).
    pub triggers: Vec<String>,
    /// May a high-confidence match fire the button automatically? Set
    /// false for destructive/irreversible actions (hint-only).
    pub auto_fire: bool,
}

impl VoiceButton {
    pub fn new(id: impl Into<String>, triggers: &[&str], auto_fire: bool) -> Self {
        Self {
            id: id.into(),
            triggers: triggers.iter().map(|s| s.to_string()).collect(),
            auto_fire,
        }
    }
}

/// The result of matching a phrase against a button set.
#[derive(Clone, Debug, PartialEq)]
pub struct ButtonMatch {
    pub button_id: String,
    /// 1.0 = exact phrase; 0.7 = trigger present as a whole word in a
    /// longer phrase.
    pub confidence: f32,
    /// True iff the matched button allows auto-fire AND confidence met
    /// the threshold. When false, the UI should HINT only.
    pub fire: bool,
}

/// Default auto-fire confidence threshold. Exact match (1.0) fires;
/// a word-in-phrase match (0.7) only hints.
pub const FIRE_THRESHOLD: f32 = 0.9;

/// Normalize a spoken phrase: trim, lowercase, strip POLITENESS filler
/// only. Affirmation words (yes/yeah/no/etc.) are NOT filler — they're
/// primary triggers for confirm/deny buttons, so stripping them would
/// make those buttons unmatchable.
fn normalize(phrase: &str) -> String {
    let mut s = phrase.trim().to_lowercase();
    loop {
        let mut changed = false;
        for lead in ["please ", "okay ", "ok ", "um ", "uh ", "like ", "just "] {
            if let Some(rest) = s.strip_prefix(lead) {
                s = rest.to_string();
                changed = true;
            }
        }
        if !changed { break; }
    }
    s.trim().to_string()
}

/// Is `needle` present in `haystack` as a whole word/phrase (not a
/// substring of a larger word)?
fn word_present(haystack: &str, needle: &str) -> bool {
    haystack == needle
        || haystack.starts_with(&format!("{} ", needle))
        || haystack.ends_with(&format!(" {}", needle))
        || haystack.contains(&format!(" {} ", needle))
}

/// Match `phrase` against `buttons`, returning the best match (highest
/// confidence) or None. `threshold` gates auto-fire. Pure.
pub fn match_button(phrase: &str, buttons: &[VoiceButton], threshold: f32) -> Option<ButtonMatch> {
    let p = normalize(phrase);
    if p.is_empty() { return None; }
    let mut best: Option<ButtonMatch> = None;
    for b in buttons {
        for t in &b.triggers {
            let conf = if p == *t {
                1.0
            } else if word_present(&p, t) {
                0.7
            } else {
                0.0
            };
            if conf > 0.0 {
                let better = best.as_ref().map(|m| conf > m.confidence).unwrap_or(true);
                if better {
                    best = Some(ButtonMatch {
                        button_id: b.id.clone(),
                        confidence: conf,
                        fire: b.auto_fire && conf >= threshold,
                    });
                }
            }
        }
    }
    best
}

/// A ready-made confirm/deny button pair (the common dialog, e.g. F7's
/// "open a new tab?"). Both auto-fire — a clear yes/no is safe to act
/// on. Callers needing a destructive variant build VoiceButtons with
/// auto_fire=false instead.
pub fn confirm_deny() -> Vec<VoiceButton> {
    vec![
        VoiceButton::new("confirm", &["yes", "yeah", "confirm", "do it", "open it", "sure", "go ahead"], true),
        VoiceButton::new("deny", &["no", "nope", "cancel", "never mind", "dismiss", "not now"], true),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn btns() -> Vec<VoiceButton> {
        vec![
            VoiceButton::new("confirm", &["yes", "confirm", "open it", "do it"], true),
            VoiceButton::new("cancel", &["no", "cancel", "never mind"], true),
            // destructive → hint only
            VoiceButton::new("delete", &["delete", "remove"], false),
        ]
    }

    #[test]
    fn exact_match_fires() {
        let m = match_button("yes", &btns(), FIRE_THRESHOLD).unwrap();
        assert_eq!(m.button_id, "confirm");
        assert_eq!(m.confidence, 1.0);
        assert!(m.fire);
    }

    #[test]
    fn politeness_filler_stripped_but_affirmation_kept() {
        // "okay confirm" → strip "okay " → "confirm" exact
        let m = match_button("okay confirm", &btns(), FIRE_THRESHOLD).unwrap();
        assert_eq!(m.button_id, "confirm");
        assert_eq!(m.confidence, 1.0);
        // but a bare affirmation is NOT stripped (it's the trigger)
        let m2 = match_button("yes", &btns(), FIRE_THRESHOLD).unwrap();
        assert_eq!(m2.button_id, "confirm");
    }

    #[test]
    fn destructive_button_hint_only() {
        // exact match but auto_fire=false → fire must be false
        let m = match_button("delete", &btns(), FIRE_THRESHOLD).unwrap();
        assert_eq!(m.button_id, "delete");
        assert_eq!(m.confidence, 1.0);
        assert!(!m.fire, "destructive action never auto-fires");
    }

    #[test]
    fn word_in_longer_phrase_hints_not_fires() {
        // trigger present as a word but not the whole phrase → 0.7,
        // below threshold → hint only even for an auto-fire button.
        let m = match_button("yes do it now", &btns(), FIRE_THRESHOLD).unwrap();
        assert_eq!(m.button_id, "confirm");
        assert_eq!(m.confidence, 0.7);
        assert!(!m.fire, "0.7 < 0.9 threshold → hint, don't fire");
    }

    #[test]
    fn no_match_returns_none() {
        assert!(match_button("scroll down please", &btns(), FIRE_THRESHOLD).is_none());
        assert!(match_button("", &btns(), FIRE_THRESHOLD).is_none());
        assert!(match_button("   ", &btns(), FIRE_THRESHOLD).is_none());
    }

    #[test]
    fn substring_not_mistaken_for_word() {
        // "yesterday" must NOT match the "yes" trigger.
        let m = match_button("yesterday", &btns(), FIRE_THRESHOLD);
        assert!(m.is_none(), "substring of a word is not a trigger match");
    }

    #[test]
    fn best_confidence_wins() {
        // a phrase that exact-matches one trigger and word-matches
        // another should pick the exact (1.0).
        let bs = vec![
            VoiceButton::new("a", &["go"], true),
            VoiceButton::new("b", &["go ahead"], true),
        ];
        let m = match_button("go ahead", &bs, FIRE_THRESHOLD).unwrap();
        assert_eq!(m.button_id, "b", "exact 'go ahead' beats word 'go'");
        assert_eq!(m.confidence, 1.0);
    }

    #[test]
    fn confirm_deny_helper_matches_both() {
        let cd = confirm_deny();
        assert_eq!(match_button("yes", &cd, FIRE_THRESHOLD).unwrap().button_id, "confirm");
        assert_eq!(match_button("no", &cd, FIRE_THRESHOLD).unwrap().button_id, "deny");
        assert_eq!(match_button("never mind", &cd, FIRE_THRESHOLD).unwrap().button_id, "deny");
        // both fire on a clear answer
        assert!(match_button("yes", &cd, FIRE_THRESHOLD).unwrap().fire);
        assert!(match_button("cancel", &cd, FIRE_THRESHOLD).unwrap().fire);
    }

    #[test]
    fn threshold_is_tunable() {
        // lower the threshold to 0.6 → a 0.7 word-match now fires.
        let m = match_button("yes do it now", &btns(), 0.6).unwrap();
        assert_eq!(m.confidence, 0.7);
        assert!(m.fire, "0.7 >= 0.6 → fires at the lower threshold");
    }
}
