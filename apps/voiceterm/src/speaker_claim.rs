// src/speaker_claim.rs
//
// v0.1.260: natural-language speaker disambiguation. Lets users
// say things like "Hey Kessler here, open terminal tab" to
// explicitly declare which registered speaker they are, helping
// the acoustic identifier when it's uncertain.
//
// Filed from the v0.1.251 hardware test: user said "Hey Kessler
// here, open terminal tab" attempting to override the
// acoustic-identifier's wrong guess of "Allison Martin". The
// idea was sound but no parsing existed for it.
//
// SECURITY: the claim does NOT bypass the acoustic check when
// acoustic is confident. Threat model: someone walks up to an
// unlocked device, says "Hey Owner here, delete everything."
// The system must NOT accept that. So the claim only matters
// when acoustic identification is uncertain or absent. When
// acoustic is confident and disagrees with claim, acoustic
// wins and the disagreement is logged.

/// Patterns matched at the start of an utterance to extract a
/// claimed speaker name. Case-insensitive. Order matters: the
/// first matching pattern wins, so more specific patterns must
/// come first.
///
/// Each entry is (prefix_pattern, name_position) where
/// name_position indicates whether the name appears between two
/// fixed strings.
const CLAIM_PATTERNS: &[ClaimPattern] = &[
    // "hey <name> here" / "hi <name> here" / "yo <name> here"
    ClaimPattern { prefix: "hey ",      suffix: " here" },
    ClaimPattern { prefix: "hi ",       suffix: " here" },
    ClaimPattern { prefix: "yo ",       suffix: " here" },
    // "okay <name> here" / "alright <name> here"
    ClaimPattern { prefix: "okay ",     suffix: " here" },
    ClaimPattern { prefix: "ok ",       suffix: " here" },
    ClaimPattern { prefix: "alright ",  suffix: " here" },
    // bare "<name> here"
    ClaimPattern { prefix: "",          suffix: " here" },
    // "i'm <name>" / "im <name>"
    ClaimPattern { prefix: "i'm ",      suffix: "" },
    ClaimPattern { prefix: "im ",       suffix: "" },
    // "this is <name>" / "it's <name>"
    ClaimPattern { prefix: "this is ",  suffix: "" },
    ClaimPattern { prefix: "it's ",     suffix: "" },
    ClaimPattern { prefix: "its ",      suffix: "" },
    // "<name> speaking"
    ClaimPattern { prefix: "",          suffix: " speaking" },
];

#[derive(Debug)]
struct ClaimPattern {
    prefix: &'static str,
    suffix: &'static str,
}

/// Parsed claim result.
#[derive(Debug, Clone, PartialEq)]
pub struct Claim {
    /// The claimed speaker name (raw text, before fuzzy-matching
    /// against the registry).
    pub claimed_name: String,
    /// The rest of the utterance after stripping the claim
    /// prefix and the suffix (if applicable). This is what gets
    /// fed to the intent parser.
    pub remainder: String,
}

/// Parse a Whisper transcription for a speaker claim. Returns
/// `Some` if a claim pattern matched, with the extracted name
/// and remainder. Returns `None` if no pattern matched (the
/// utterance is a normal command with no speaker claim).
///
/// Examples:
///   "hey kessler here open terminal" → claimed="kessler",
///       remainder="open terminal"
///   "i'm allison please install firefox" → claimed="allison",
///       remainder="please install firefox"
///   "open terminal tab" → None
pub fn parse_claim(utterance: &str) -> Option<Claim> {
    let lower = utterance.trim().to_lowercase();
    if lower.is_empty() { return None; }

    for pat in CLAIM_PATTERNS {
        if let Some(claim) = try_pattern(&lower, pat, utterance) {
            return Some(claim);
        }
    }
    None
}

fn try_pattern(lower: &str, pat: &ClaimPattern, original: &str) -> Option<Claim> {
    // Lower-case match for matching positions; remainder text
    // pulled from `original` to preserve original casing for
    // intent parsing downstream.
    if !lower.starts_with(pat.prefix) { return None; }
    let after_prefix = &lower[pat.prefix.len()..];

    // Now we need to find the name. Two cases:
    // 1. suffix non-empty: name is between prefix and suffix.
    //    Find the suffix in after_prefix.
    // 2. suffix empty: name is the first 1-2 words after prefix.
    //    "i'm Kessler open terminal" → name = "kessler",
    //    remainder = "open terminal".
    let (name, remainder_start) = if !pat.suffix.is_empty() {
        // Find the suffix — must be followed by whitespace or end.
        // E.g. "kessler here open" — suffix " here" matches at
        // position 7, then remainder starts at 12.
        let idx = match find_word_boundary(after_prefix, pat.suffix) {
            Some(i) => i,
            None => return None,
        };
        let name_str = after_prefix[..idx].trim().to_string();
        let after_suffix_start = pat.prefix.len() + idx + pat.suffix.len();
        // Skip any leading punctuation like comma after the suffix.
        let mut start = after_suffix_start;
        let bytes = original.as_bytes();
        while start < bytes.len() && (bytes[start] == b',' || bytes[start] == b' '
            || bytes[start] == b';' || bytes[start] == b'.' || bytes[start] == b':') {
            start += 1;
        }
        (name_str, start)
    } else {
        // No suffix: take first 1-2 words as the name, rest is
        // remainder. We allow 1 word for "im Kessler" or 2 words
        // for compound names like "i'm John Smith".
        let mut words = after_prefix.split_whitespace();
        let first = match words.next() { Some(w) => w, None => return None };
        // v0.1.598: the first word MUST be plausibly a name. Without this
        // guard, "i'm done", "this is great", "i'm sorry running tests"
        // capture "done"/"great"/"sorry" as the claimed name — which then
        // FUZZY-MATCHES short enrolled names (done~Dan 0.68, great~Greg
        // 0.71, sorry~Cory 0.71) in match_claim_to_registry, hijacking the
        // utterance as a speaker claim and stealing the turn for the wrong
        // person. A clearly-non-name first word means this isn't a claim.
        if is_common_command_word(first) || is_non_name_filler(first) {
            return None;
        }
        let second_opt = words.clone().next();

        // Decide whether the second word is also part of the
        // name. Heuristic: it is if it's "capitalized" (in the
        // original casing) — looking at the original utterance.
        // E.g. "I'm John Smith open terminal" — "Smith" starts
        // with capital → part of name.
        //
        // But for lowercased Whisper output ("i'm john smith"),
        // we lose case info. So secondary heuristic: stop at
        // common-words (the, a, please, can, etc.). If the
        // second word is a "stop word," it's part of the
        // command, not the name.
        let is_two_word_name = match second_opt {
            Some(s2) => {
                !is_common_command_word(s2) && {
                    // Also check capitalization in the original.
                    let original_words: Vec<&str> = original.split_whitespace().collect();
                    // Find the second word's position in original.
                    // Crude: count whitespace tokens past the
                    // prefix length.
                    let prefix_words = pat.prefix.split_whitespace().count();
                    if original_words.len() > prefix_words + 1 {
                        let candidate = original_words[prefix_words + 1];
                        candidate.chars().next()
                            .map(|c| c.is_uppercase()).unwrap_or(false)
                    } else { false }
                }
            }
            None => false,
        };

        let (name_str, used_words) = if is_two_word_name {
            (format!("{} {}", first, second_opt.unwrap()), 2usize)
        } else {
            (first.to_string(), 1usize)
        };

        // Compute remainder start position in original. Skip
        // prefix + name + intervening punctuation.
        let mut byte_pos = pat.prefix.len();
        // Skip whitespace
        let bytes = original.as_bytes();
        while byte_pos < bytes.len() && bytes[byte_pos] == b' ' { byte_pos += 1; }
        // Skip used_words worth of words.
        for _ in 0..used_words {
            // Walk past current word
            while byte_pos < bytes.len() && bytes[byte_pos] != b' '
                && bytes[byte_pos] != b',' && bytes[byte_pos] != b'.'
                && bytes[byte_pos] != b';' && bytes[byte_pos] != b':' { byte_pos += 1; }
            // Walk past separator(s)
            while byte_pos < bytes.len() && (bytes[byte_pos] == b' '
                || bytes[byte_pos] == b',' || bytes[byte_pos] == b'.'
                || bytes[byte_pos] == b';' || bytes[byte_pos] == b':') { byte_pos += 1; }
        }
        (name_str, byte_pos)
    };

    if name.is_empty() { return None; }

    let remainder = if remainder_start < original.len() {
        original[remainder_start..].trim().to_string()
    } else {
        String::new()
    };

    Some(Claim { claimed_name: name, remainder })
}

/// Find the byte index of `needle` in `haystack` such that the
/// match is at a word boundary (preceded by whitespace and
/// followed by whitespace or end-of-string). Otherwise
/// `haystack.contains("here")` would match "where" too.
///
/// `needle` is expected to start with a space (we use this only
/// for our pattern suffixes like " here" and " speaking").
fn find_word_boundary(haystack: &str, needle: &str) -> Option<usize> {
    let bytes = haystack.as_bytes();
    let nb = needle.as_bytes();
    if nb.is_empty() { return None; }
    if bytes.len() < nb.len() { return None; }
    let mut i = 0;
    while i + nb.len() <= bytes.len() {
        if &bytes[i..i + nb.len()] == nb {
            // Suffix is " here" (leading space) so left boundary
            // is satisfied by the space itself. For right
            // boundary: position after must be end-of-string,
            // space, or punctuation.
            let after = i + nb.len();
            if after == bytes.len()
                || matches!(bytes[after], b' ' | b',' | b'.' | b';' | b':' | b'!' | b'?') {
                return Some(i);
            }
        }
        i += 1;
    }
    None
}

/// Stop-words: if seen as the second token after "i'm <word>",
/// indicate that <word> is the full name and the rest is the
/// command. E.g. "i'm Allison please install" — "please" is a
/// command word, so the name is just "Allison."
/// v0.1.598: words that commonly follow "i'm"/"this is"/"it's" but are
/// predicates/states/adjectives, NOT speaker names. Without rejecting
/// these, short ones fuzzy-match short enrolled names and hijack the
/// turn (done~Dan, great~Greg, sorry~Cory). Kept separate from
/// is_common_command_word (verbs/articles) for clarity; this list is
/// states/feelings/acknowledgements.
fn is_non_name_filler(w: &str) -> bool {
    matches!(w,
        "done" | "ready" | "good" | "great" | "fine" | "okay" | "ok"
        | "sorry" | "sure" | "right" | "wrong" | "back" | "here"
        | "trying" | "running" | "looking" | "working" | "thinking"
        | "confused" | "lost" | "stuck" | "happy" | "glad" | "afraid"
        | "not" | "just" | "still" | "already" | "almost" | "about"
        | "gonna" | "now"
    )
}

fn is_common_command_word(w: &str) -> bool {
    matches!(w,
        "the" | "a" | "an" | "please" | "can" | "could" | "would"
        | "open" | "close" | "show" | "hide" | "find" | "search"
        | "go" | "stop" | "start" | "run" | "play" | "pause"
        | "tell" | "ask" | "send" | "save" | "delete" | "remove"
        | "create" | "make" | "do" | "give" | "get" | "let"
        | "you" | "me" | "we" | "they" | "he" | "she" | "it"
        | "is" | "are" | "was" | "were" | "be" | "been"
        | "and" | "or" | "but" | "if" | "when" | "what" | "where"
        | "how" | "why" | "going" | "want" | "need" | "have"
        | "i" | "ill" | "i'll"
    )
}

/// Match a claimed name against registered speaker names.
/// Returns the best match with confidence in [0.0, 1.0]. Uses
/// case-insensitive comparison and Levenshtein-distance-based
/// fuzzy matching for Whisper mistranscriptions (e.g. "castle"
/// for "Kessler").
///
/// Confidence levels:
///   1.0  = exact case-insensitive match
///   0.9  = the claim matches the first word of a multi-word
///          registered name (e.g. "Kessler" matches "Kessler
///          DuPont-Teevin")
///   0.5..0.85 = fuzzy match by Levenshtein, scaled by edit
///          distance vs name length
///   <0.5 = no good match (caller treats as no match)
pub fn match_claim_to_registry(
    claimed: &str,
    registered_names: &[String],
) -> Option<(String, f32)> {
    if claimed.trim().is_empty() || registered_names.is_empty() {
        return None;
    }
    let claimed_lower = claimed.trim().to_lowercase();

    // 1. Exact case-insensitive
    for name in registered_names {
        if name.to_lowercase() == claimed_lower {
            return Some((name.clone(), 1.0));
        }
    }
    // 2. First-word match (e.g. "Kessler" claim matches
    //    "Kessler DuPont-Teevin" registered)
    for name in registered_names {
        let first = name.split_whitespace().next().unwrap_or("").to_lowercase();
        if !first.is_empty() && first == claimed_lower {
            return Some((name.clone(), 0.9));
        }
    }
    // 3. Whole-name fuzzy via Levenshtein
    let mut best: Option<(String, f32)> = None;
    for name in registered_names {
        let name_lower = name.to_lowercase();
        let d = levenshtein(&claimed_lower, &name_lower);
        let max_len = claimed_lower.len().max(name_lower.len()).max(1);
        let raw = 1.0 - (d as f32 / max_len as f32);
        // Scale into the 0.5..0.85 range so it sits below the
        // first-word-match's 0.9 ceiling.
        let scaled = 0.5 + raw * 0.35;
        if raw < 0.5 { continue; }  // too noisy to count
        match &best {
            None => best = Some((name.clone(), scaled)),
            Some((_, b)) if scaled > *b => best = Some((name.clone(), scaled)),
            _ => {}
        }
    }
    // 4. ALSO try fuzzy against the first word only, useful for
    //    Whisper transcribing "castle" instead of "Kessler"
    //    (one-letter edit distance against the first word, not
    //    the full "Kessler DuPont-Teevin").
    for name in registered_names {
        let first = name.split_whitespace().next().unwrap_or("").to_lowercase();
        if first.is_empty() || first.len() < 3 { continue; }
        let d = levenshtein(&claimed_lower, &first);
        let max_len = claimed_lower.len().max(first.len()).max(1);
        let raw = 1.0 - (d as f32 / max_len as f32);
        let scaled = 0.5 + raw * 0.35;
        // v0.1.466: the first-word fuzzy path is specifically the
        // Whisper-mishearing catcher (the module's motivating case
        // from the v0.1.251 hardware log: "Kessler" → "castle").
        // The prior 0.6 gate was too strict to catch its OWN
        // example: castle vs kessler is dist 4 / 7 = raw 0.429,
        // rejected at 0.6. Lowered to 0.4, which admits that case
        // (scaled→0.65, a deliberately LOW-confidence match) while
        // still firmly rejecting noise (e.g. "zebrafish" vs any
        // name scores raw≈0.11). Confidence stays in the 0.5..0.85
        // band so a true first-word EXACT match (0.9) always wins.
        if raw < 0.4 { continue; }
        match &best {
            None => best = Some((name.clone(), scaled)),
            Some((_, b)) if scaled > *b => best = Some((name.clone(), scaled)),
            _ => {}
        }
    }
    best
}

/// Levenshtein edit distance (insertions, deletions,
/// substitutions cost 1). Iterative DP, two-row optimization.
/// O(m*n) time, O(min(m,n)) memory.
fn levenshtein(a: &str, b: &str) -> usize {
    let a_chars: Vec<char> = a.chars().collect();
    let b_chars: Vec<char> = b.chars().collect();
    let (a_chars, b_chars) = if a_chars.len() < b_chars.len() {
        (a_chars, b_chars)
    } else {
        (b_chars, a_chars)
    };
    if a_chars.is_empty() { return b_chars.len(); }
    let mut prev: Vec<usize> = (0..=a_chars.len()).collect();
    let mut curr = vec![0usize; a_chars.len() + 1];
    for (j, &bc) in b_chars.iter().enumerate() {
        curr[0] = j + 1;
        for (i, &ac) in a_chars.iter().enumerate() {
            let cost = if ac == bc { 0 } else { 1 };
            curr[i + 1] = (curr[i] + 1)
                .min(prev[i + 1] + 1)
                .min(prev[i] + cost);
        }
        std::mem::swap(&mut prev, &mut curr);
    }
    prev[a_chars.len()]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_claim_returns_none() {
        assert_eq!(parse_claim(""), None);
        assert_eq!(parse_claim("open terminal tab"), None);
        assert_eq!(parse_claim("show me the latest news"), None);
    }

    #[test]
    fn hey_name_here_pattern() {
        let c = parse_claim("hey Kessler here open terminal tab")
            .expect("should parse");
        assert_eq!(c.claimed_name, "kessler");
        assert_eq!(c.remainder, "open terminal tab");
    }

    #[test]
    fn hey_name_here_with_comma() {
        let c = parse_claim("hey Kessler here, open terminal tab")
            .expect("should parse");
        assert_eq!(c.claimed_name, "kessler");
        assert_eq!(c.remainder, "open terminal tab");
    }

    #[test]
    fn bare_name_here() {
        let c = parse_claim("Kessler here open terminal")
            .expect("should parse");
        assert_eq!(c.claimed_name, "kessler");
        assert_eq!(c.remainder, "open terminal");
    }

    #[test]
    fn im_name_pattern() {
        let c = parse_claim("I'm Allison please install firefox")
            .expect("should parse");
        assert_eq!(c.claimed_name, "allison");
        assert_eq!(c.remainder, "please install firefox");
    }

    #[test]
    fn im_compound_name() {
        // Two-word name detection: capitalization in original.
        let c = parse_claim("I'm John Smith open the terminal")
            .expect("should parse");
        assert_eq!(c.claimed_name, "john smith");
        assert_eq!(c.remainder, "open the terminal");
    }

    #[test]
    fn this_is_pattern() {
        let c = parse_claim("this is Kessler open terminal")
            .expect("should parse");
        assert_eq!(c.claimed_name, "kessler");
        assert_eq!(c.remainder, "open terminal");
    }

    #[test]
    fn name_speaking_pattern() {
        let c = parse_claim("Allison speaking, install firefox")
            .expect("should parse");
        assert_eq!(c.claimed_name, "allison");
        assert_eq!(c.remainder, "install firefox");
    }

    #[test]
    fn match_exact_case_insensitive() {
        let names = vec!["Kessler DuPont-Teevin".to_string(),
                         "Allison Martin".to_string()];
        let (n, c) = match_claim_to_registry("kessler dupont-teevin", &names)
            .expect("should match");
        assert_eq!(n, "Kessler DuPont-Teevin");
        assert!((c - 1.0).abs() < 0.001);
    }

    #[test]
    fn match_first_word() {
        let names = vec!["Kessler DuPont-Teevin".to_string(),
                         "Allison Martin".to_string()];
        let (n, c) = match_claim_to_registry("kessler", &names)
            .expect("should match by first word");
        assert_eq!(n, "Kessler DuPont-Teevin");
        assert!((c - 0.9).abs() < 0.001);
    }

    #[test]
    fn match_fuzzy_whisper_error() {
        // Whisper transcribed "Kessler" as "castle" in the
        // hardware-bug log. Should still match Kessler.
        let names = vec!["Kessler DuPont-Teevin".to_string(),
                         "Allison Martin".to_string()];
        let result = match_claim_to_registry("castle", &names);
        // "castle" vs "kessler" — distance 4 over 7 chars =
        // raw 0.429, scaled to ~0.65 via the first-word fuzzy
        // path. A deliberately low-confidence match (this is a
        // best-effort Whisper-error recovery, not a sure thing).
        if let Some((n, c)) = result {
            assert_eq!(n, "Kessler DuPont-Teevin");
            assert!(c > 0.5 && c < 0.9);
        } else {
            panic!("fuzzy match should succeed for 'castle' → 'Kessler'");
        }
    }

    #[test]
    fn match_no_match() {
        let names = vec!["Kessler".to_string(), "Allison".to_string()];
        // Completely unrelated claim.
        assert!(match_claim_to_registry("zebrafish", &names).is_none());
    }

    #[test]
    fn match_empty_registry() {
        let names: Vec<String> = vec![];
        assert!(match_claim_to_registry("anyone", &names).is_none());
    }

    #[test]
    fn parse_claim_lowercase_preserves_original_case_in_remainder() {
        let c = parse_claim("hey Kessler here Open Terminal Tab")
            .expect("should parse");
        assert_eq!(c.remainder, "Open Terminal Tab",
            "remainder should preserve original casing");
    }

    #[test]
    fn levenshtein_known_distances() {
        assert_eq!(levenshtein("kitten", "sitting"), 3);
        assert_eq!(levenshtein("", ""), 0);
        assert_eq!(levenshtein("abc", ""), 3);
        assert_eq!(levenshtein("castle", "kessler"), 4);
    }

    #[test]
    fn does_not_match_where_for_here() {
        // " here" should match at word boundary, not inside
        // "where" or "there".
        let c = parse_claim("hey where is the terminal");
        // "where" contains "here" but we shouldn't extract
        // a name. The pattern "hey <name> here" requires the
        // 'here' word standalone.
        assert!(c.is_none() ||
            c.as_ref().map(|x| !x.claimed_name.contains("where"))
                .unwrap_or(true));
    }

    #[test]
    fn empty_remainder_when_claim_is_whole_utterance() {
        let c = parse_claim("Kessler here").expect("should parse");
        assert_eq!(c.claimed_name, "kessler");
        assert_eq!(c.remainder, "");
    }

    #[test]
    fn filler_first_word_is_not_a_claim() {
        // v0.1.598: "i'm done" etc. must NOT parse as a speaker claim —
        // otherwise "done"/"great"/"sorry" fuzzy-match short enrolled
        // names (Dan/Greg/Cory) and hijack the turn.
        assert_eq!(parse_claim("i'm done"), None);
        assert_eq!(parse_claim("im done with that"), None);
        assert_eq!(parse_claim("this is great"), None);
        assert_eq!(parse_claim("i'm sorry run the tests"), None);
        assert_eq!(parse_claim("i'm running cargo test"), None);
        assert_eq!(parse_claim("it's okay"), None);
        assert_eq!(parse_claim("i'm not sure"), None);
    }

    #[test]
    fn real_names_still_parse_after_filler_guard() {
        // The guard must NOT reject legitimate enrollment phrases.
        let c = parse_claim("i'm Kessler").expect("real name should parse");
        assert_eq!(c.claimed_name, "kessler");
        let c = parse_claim("this is Allison open terminal")
            .expect("real name should parse");
        assert_eq!(c.claimed_name, "allison");
        assert_eq!(c.remainder.to_lowercase(), "open terminal");
        // a name that happens to start like nothing in the filler list
        let c = parse_claim("im Dan").expect("Dan should still parse");
        assert_eq!(c.claimed_name, "dan");
    }
}
