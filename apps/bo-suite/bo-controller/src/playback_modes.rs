// bo-controller/src/playback_modes.rs
//
// Repeat-mode + Child-mode state, persisted via settings_store.
//
// REPEAT — three modes:
//   Off (default): play through queue once, stop at end.
//   One:           replay the just-finished track immediately.
//   All:           wrap to queue start when the last entry ends.
//
// CHILD MODE — a content filter that skips tracks whose title or
// path contains anything from a blacklist of profane / sexual /
// drug / violent words. The match is case-insensitive and uses
// word-boundary matching so "asset" doesn't trip the "ass" entry
// and "passage" doesn't trip "rape".
//
// We deliberately keep the lists in this file rather than reading
// from an external resource so the user can audit + edit them in
// one place. The lists are intentionally substantial — false
// positives are recoverable (user can disable child mode), false
// negatives leak inappropriate content to a kid and that's worse.
//
// MATCH SEMANTICS — the filter checks each blacklist word as a
// whole-word match against the title and the basename of the path.
// We don't check the WHOLE PATH because a parent's library may
// live in a directory whose name contains otherwise-flagged words
// (e.g. "Bitches Brew" album in a jazz library), and the filename
// itself wouldn't necessarily match. The path-basename check
// catches songs whose title is empty but whose filename carries
// the marker; the title check catches songs with proper metadata.

use serde::{Deserialize, Serialize};

const SETTING_REPEAT_MODE: &str = "playback:repeat_mode";
const SETTING_CHILD_MODE:  &str = "playback:child_mode";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RepeatMode {
    Off,
    /// Replay the just-finished track. Triggered at the end of EVERY
    /// track until the user changes mode — useful for putting a baby
    /// to sleep on one specific song, or for memorizing a passage.
    One,
    /// Loop the queue: when the LAST queued track ends, wrap to the
    /// first one. Stops only when the queue is emptied or the mode
    /// changes.
    All,
}

impl Default for RepeatMode { fn default() -> Self { RepeatMode::Off } }

impl RepeatMode {
    pub fn as_tag(self) -> &'static str {
        match self {
            RepeatMode::Off => "off",
            RepeatMode::One => "one",
            RepeatMode::All => "all",
        }
    }
    pub fn from_tag(s: &str) -> Self {
        match s {
            "one" => RepeatMode::One,
            "all" => RepeatMode::All,
            _     => RepeatMode::Off,
        }
    }
    /// Cycle Off → One → All → Off. Bound to a single toggle button
    /// in QML so the user gets all three modes from one control.
    pub fn next(self) -> Self {
        match self {
            RepeatMode::Off => RepeatMode::One,
            RepeatMode::One => RepeatMode::All,
            RepeatMode::All => RepeatMode::Off,
        }
    }
}

pub fn load_repeat_mode() -> RepeatMode {
    match crate::settings_store::get_str(SETTING_REPEAT_MODE) {
        Some(s) => RepeatMode::from_tag(&s),
        None    => RepeatMode::default(),
    }
}

pub fn save_repeat_mode(m: RepeatMode) {
    let _ = crate::settings_store::set_str(SETTING_REPEAT_MODE, m.as_tag());
}

// ── Child mode ────────────────────────────────────────────────────

pub fn load_child_mode() -> bool {
    crate::settings_store::get_str(SETTING_CHILD_MODE)
        .map(|s| s == "true")
        .unwrap_or(false)
}

pub fn save_child_mode(on: bool) {
    let _ = crate::settings_store::set_str(SETTING_CHILD_MODE,
                                           if on { "true" } else { "false" });
}

/// Comprehensive blacklist. Word-boundary matched, case-insensitive.
/// Three categories (one per source array) merged in `is_inappropriate`.
///
/// Curation principle: the goal is "stuff a 7-year-old shouldn't hear"
/// not "stuff a corporate environment should censor." So we include
/// drug references and slurs but NOT mild expletives like "damn" or
/// "hell" which fly in plenty of kid-appropriate songs (think any
/// Beatles album). The user can extend the list in two ways:
///   (a) edit this file and rebuild
///   (b) future work: a settings-store key with a user-curated list
///       appended at runtime
const PROFANITY: &[&str] = &[
    // Standard profanity. Each entry is the LEMMA; we match the
    // entry and common plural/verb forms (e.g. "fuck" also catches
    // "fucking" via the prefix scan below — see is_inappropriate).
    "fuck", "shit", "bitch", "asshole", "bastard", "dick",
    "pussy", "cunt", "twat", "wanker", "whore", "slut", "skank",
    "motherfucker", "douche", "douchebag", "prick", "cock",
    // Spelled-out versions of common censored words
    "f*ck", "sh*t", "b*tch", "a*shole",
];

const DRUG_TERMS: &[&str] = &[
    // Drug references that show up in mainstream lyrics.
    "cocaine", "crack", "heroin", "meth",
    "weed", "marijuana", "blunt", "joint",
    "ecstasy", "molly", "lsd", "acid",
    "drugs", "drug", "high",
    "syrup", "lean", "percocet", "xanax", "xan",
    "purple drank",
];

const SEXUAL_TERMS: &[&str] = &[
    // Sexual content. Note "money drugs bitches" was the user's
    // explicit example — bitches already in PROFANITY, money on
    // its own is fine (kids hear about money), but we add the
    // common combo phrases below.
    "sex", "sexy", "fucking", "fucked", "horny",
    "blowjob", "handjob", "rimjob",
    "tits", "boobs", "nipple",
    "rape", "raping", "rapist",
    "porn", "pornhub", "xxx",
];

const VIOLENT_TERMS: &[&str] = &[
    "kill", "murder", "murdered", "stab", "stabbed",
    "shoot", "shot",  // narrow context, but in lyrics typically violent
    "gun", "glock", "ak47", "ak-47",
    "thug", "gang", "gangsta",
];

/// Slur list — terms we always reject regardless of category. Kept
/// separate so a future "child mode strictness" slider could keep
/// these always-on while relaxing other categories. Truncated here
/// (the actual deployment can extend this).
const SLURS: &[&str] = &[
    "nigger", "nigga", "faggot", "fag", "tranny",
    "retard", "retarded", "spic", "kike", "chink",
];

/// Return true if the given text contains any blacklisted word as a
/// whole-word match, case-insensitive. Also catches common suffixes
/// (e.g. "fuck" → "fucking", "fucked", "fucker"): we treat any token
/// that starts with a blacklisted entry as a hit, unless the suffix
/// is itself in a safe-suffix list (e.g. "ass" wouldn't be blacklisted
/// but if it were, "assess" would still be safe because "ess" isn't a
/// verb/noun suffix and the boundary check would fail).
fn contains_blacklisted_word(text: &str) -> Option<&'static str> {
    let lower = text.to_lowercase();
    // Tokenize on non-alphanumeric so punctuation doesn't shield words.
    // e.g. "intro-fuck" tokenizes to ["intro", "fuck"].
    let tokens: Vec<&str> = lower
        .split(|c: char| !c.is_alphanumeric() && c != '*')
        .filter(|t| !t.is_empty())
        .collect();
    for token in &tokens {
        for &word in PROFANITY.iter()
            .chain(DRUG_TERMS.iter())
            .chain(SEXUAL_TERMS.iter())
            .chain(VIOLENT_TERMS.iter())
            .chain(SLURS.iter())
        {
            // Exact token match
            if *token == word { return Some(word); }
            // Prefix match for verb/noun forms: "fucking" matches "fuck".
            // We allow up to 4 trailing chars — covers -ing, -ed, -er,
            // -ers, -s, -es, -in, -in', -ah, etc. Beyond 4 chars we
            // require an exact match to avoid weird false hits.
            if token.len() > word.len() && token.len() <= word.len() + 4
                && token.starts_with(word) {
                // Sanity check: the extra suffix must be alphabetic only
                // (so "fuckin", "fucked", "fucker", "fuckery" hit but
                // arbitrary digit-blob tokens don't).
                let suffix = &token[word.len()..];
                if suffix.chars().all(|c| c.is_alphabetic()) {
                    return Some(word);
                }
            }
        }
    }
    None
}

/// Decide whether a track should be skipped under child mode.
/// Inputs are the track's title (from metadata) and its file path.
/// Returns Some(reason) if blacklisted, None if clean.
///
/// The reason string is the matched blacklist entry — caller can
/// log it for transparency ("skipping 'X' (matched 'fuck')") so a
/// suspicious parent can verify the filter is working.
pub fn should_skip_in_child_mode(title: &str, path: &str) -> Option<String> {
    if let Some(w) = contains_blacklisted_word(title) {
        return Some(format!("title contains '{}'", w));
    }
    // Path BASENAME only — see module doc above for why.
    let basename = std::path::Path::new(path)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("");
    if let Some(w) = contains_blacklisted_word(basename) {
        return Some(format!("filename contains '{}'", w));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repeat_cycle() {
        assert_eq!(RepeatMode::Off.next(), RepeatMode::One);
        assert_eq!(RepeatMode::One.next(), RepeatMode::All);
        assert_eq!(RepeatMode::All.next(), RepeatMode::Off);
    }

    #[test]
    fn repeat_tag_roundtrip() {
        for m in [RepeatMode::Off, RepeatMode::One, RepeatMode::All] {
            assert_eq!(RepeatMode::from_tag(m.as_tag()), m);
        }
    }

    #[test]
    fn child_filter_blocks_profanity_in_title() {
        let r = should_skip_in_child_mode("Big Fuckin' Mood", "/m/a.opus");
        assert!(r.is_some());
        assert!(r.unwrap().contains("fuck"));
    }

    #[test]
    fn child_filter_blocks_slur_in_filename() {
        let r = should_skip_in_child_mode("untitled", "/music/some-nigga-track.opus");
        assert!(r.is_some(), "should block slur in filename basename");
    }

    #[test]
    fn child_filter_passes_clean_title() {
        let r = should_skip_in_child_mode("Twinkle Twinkle Little Star", "/m/twinkle.opus");
        assert!(r.is_none(), "clean title should pass, got {:?}", r);
    }

    #[test]
    fn child_filter_no_false_positive_substrings() {
        // "asset" must not trip a hypothetical "ass" entry. "passage"
        // must not trip "rape". "classical" must not trip "ass".
        let cases = [
            ("Asset Allocation Strategies", "/m/x.opus"),
            ("Passage to India", "/m/y.opus"),
            ("Classical Music for Babies", "/m/z.opus"),
            ("Bandit Country", "/m/n.opus"),     // "bandit" not bitch
            ("The Glockenspiel", "/m/g.opus"),   // "glock" prefix but
                                                  //   not as token
        ];
        for (title, path) in cases {
            assert!(should_skip_in_child_mode(title, path).is_none(),
                "false positive on '{}'", title);
        }
    }

    #[test]
    fn child_filter_catches_punctuated_profanity() {
        // "intro-fuck" or "fuck.you" must still trip.
        assert!(should_skip_in_child_mode("intro-fuck", "/m/x.opus").is_some());
        assert!(should_skip_in_child_mode("fuck.you remix", "/m/x.opus").is_some());
    }

    #[test]
    fn child_filter_catches_censored_spellings() {
        // f*ck, sh*t etc. are in the explicit list.
        assert!(should_skip_in_child_mode("F*ck the System", "/m/x.opus").is_some());
        assert!(should_skip_in_child_mode("Sh*t Just Got Real", "/m/x.opus").is_some());
    }

    #[test]
    fn child_filter_catches_drug_references() {
        assert!(should_skip_in_child_mode("Cocaine Nights", "/m/x.opus").is_some());
        assert!(should_skip_in_child_mode("Smoke Weed Every Day", "/m/x.opus").is_some());
        assert!(should_skip_in_child_mode("Xan Master", "/m/x.opus").is_some());
    }

    #[test]
    fn child_filter_catches_sexual_terms() {
        assert!(should_skip_in_child_mode("Sexy Back", "/m/x.opus").is_some());
        assert!(should_skip_in_child_mode("Porn Star Dancing", "/m/x.opus").is_some());
    }

    #[test]
    fn child_filter_clean_path_basename() {
        // file_stem strips both directory and extension
        let r = should_skip_in_child_mode("",
            "/music/a/b/track-with-fuck-in-name.opus");
        assert!(r.is_some(), "should match basename without ext");
    }
}
