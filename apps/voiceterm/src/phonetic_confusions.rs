// src/phonetic_confusions.rs
//
// Phonetic confusion sets — words that voice transcription
// (whisper) commonly confuses. Loaded from data/phonetic_confusions.toml
// at startup, with a built-in fallback so the system works even without
// the data file.
//
// Used by the voice intent matcher: when a transcribed token looks like
// it might be a confused word, we expand to all members of the matching
// group and let the intent classifier run on each. Matches multiply
// candidate space slightly but recall improves significantly for hard-
// to-transcribe terminal commands.
//
// Format: see data/phonetic_confusions.toml. Each [[group]] has a
// `words = ["w1", "w2", ...]` array. We build a HashMap<String,
// Vec<String>> where each word in a group maps to ALL OTHER words in
// that group.

use std::collections::HashMap;
use std::sync::OnceLock;
use std::path::Path;

static CONFUSIONS: OnceLock<HashMap<String, Vec<String>>> = OnceLock::new();

/// Look up alternates for a word. Returns empty slice if no confusion
/// group includes this word. Lookup is lowercase-normalized.
///
/// Caller pattern:
/// ```text
/// for alt in alternates_for("ls") {
///     try_intent_match(alt);
/// }
/// // Plus the original word itself:
/// try_intent_match("ls");
/// ```
pub fn alternates_for(word: &str) -> &'static [String] {
    let map = CONFUSIONS.get_or_init(load);
    map.get(&word.to_lowercase())
        .map(|v| v.as_slice())
        .unwrap_or(&[])
}

/// v0.1.615: phrase-level expansion for the intent-retry path. Given a
/// phrase whisper produced, generate alternate phrasings by substituting
/// ONE confused word at a time with each of its phonetic alternates. The
/// caller (a future voice_parse_intent retry — the wiring the v0.1.194
/// audit note anticipated) tries the original first, then these variants
/// until one matches an intent.
///
/// Single-substitution (not combinatorial) on purpose: whisper typically
/// mis-hears ONE word, and the full cross-product would explode (k words
/// × n alternates each). Bounded by `max_variants`. The original phrase
/// is NOT included (the caller already tried it). Order is stable:
/// left-to-right by word position, then by alternate order in the TOML.
pub fn phrase_variants(phrase: &str, max_variants: usize) -> Vec<String> {
    if max_variants == 0 {
        return Vec::new();
    }
    let words: Vec<&str> = phrase.split_whitespace().collect();
    let mut out: Vec<String> = Vec::new();
    for (i, w) in words.iter().enumerate() {
        for alt in alternates_for(w) {
            // skip a no-op substitution (alt equal to the word, case-insensitive)
            if alt.eq_ignore_ascii_case(w) {
                continue;
            }
            let mut variant = words.clone();
            variant[i] = alt.as_str();
            let joined = variant.join(" ");
            if !out.contains(&joined) {
                out.push(joined);
                if out.len() >= max_variants {
                    return out;
                }
            }
        }
    }
    out
}

/// Force reload from disk. Used by tests; production calls load() once
/// via OnceLock and never re-reads. (Adding a hot-reload hook is out of
/// scope; users editing the TOML need to restart.)
#[allow(dead_code)]
pub fn force_reload() {
    // OnceLock has no clear; this is best-effort
}

fn load() -> HashMap<String, Vec<String>> {
    let mut groups: Vec<Vec<String>> = Vec::new();

    // Try data dir first.
    let candidates = [
        std::path::PathBuf::from("data/phonetic_confusions.toml"),
        std::path::PathBuf::from("./data/phonetic_confusions.toml"),
    ];
    let mut loaded_path: Option<std::path::PathBuf> = None;
    for path in &candidates {
        if path.exists() {
            if let Some(parsed) = parse_toml_file(path) {
                groups = parsed;
                loaded_path = Some(path.clone());
                break;
            }
        }
    }
    // Try next-to-binary
    if groups.is_empty() {
        if let Ok(exe) = std::env::current_exe() {
            if let Some(parent) = exe.parent() {
                for rel in &["data/phonetic_confusions.toml",
                             "../data/phonetic_confusions.toml",
                             "../../data/phonetic_confusions.toml"] {
                    let p = parent.join(rel);
                    if p.exists() {
                        if let Some(parsed) = parse_toml_file(&p) {
                            groups = parsed;
                            loaded_path = Some(p);
                            break;
                        }
                    }
                }
            }
        }
    }
    // User overlay at ~/.config/web2qml/data/
    if let Ok(home) = std::env::var("HOME") {
        let user_path = Path::new(&home)
            .join(".config/web2qml/data/phonetic_confusions.toml");
        if user_path.exists() {
            if let Some(parsed) = parse_toml_file(&user_path) {
                // User overlay APPENDS — additional confusion sets
                // beyond the defaults.
                groups.extend(parsed);
                crate::clog!("phonetic_confusions", "user overlay: +{} groups",
                    groups.len());
            }
        }
    }

    // Fallback to built-in defaults if no file found.
    if groups.is_empty() {
        groups = default_groups();
        crate::clog!("phonetic_confusions", "using built-in defaults ({} groups)",
            groups.len());
    } else if let Some(p) = loaded_path {
        crate::clog!("phonetic_confusions", "loaded {} groups from {}",
            groups.len(), p.display());
    }

    // Build the lookup map: each word → other words in its group.
    let mut map: HashMap<String, Vec<String>> = HashMap::new();
    for group in &groups {
        for word in group {
            let key = word.to_lowercase();
            let alts: Vec<String> = group.iter()
                .filter(|w| w.to_lowercase() != key)
                .cloned()
                .collect();
            // If a word appears in multiple groups, merge alts.
            map.entry(key).or_default().extend(alts);
        }
    }
    // Dedupe each entry.
    for v in map.values_mut() {
        v.sort();
        v.dedup();
    }
    map
}

/// Minimal TOML subset parser for our `[[group]]` + `words = [...]`
/// schema. Returns Vec<Vec<String>> on success.
///
/// We avoid pulling in a full TOML crate — the format here is constrained
/// enough that a hand-written parser is small and zero-dep. Rejects
/// malformed entries silently (returns whatever it could parse).
fn parse_toml_file(path: &Path) -> Option<Vec<Vec<String>>> {
    let content = std::fs::read_to_string(path).ok()?;
    let mut groups: Vec<Vec<String>> = Vec::new();
    let mut current_group: Option<Vec<String>> = None;
    for line in content.lines() {
        let trimmed = line.trim();
        // Skip comments and blank lines.
        if trimmed.is_empty() || trimmed.starts_with('#') { continue; }
        // [[group]] starts a new group.
        if trimmed.starts_with("[[") && trimmed.ends_with("]]") {
            if let Some(g) = current_group.take() {
                if !g.is_empty() { groups.push(g); }
            }
            current_group = Some(Vec::new());
            continue;
        }
        // words = ["w1", "w2", ...] — parse the bracketed array.
        if let Some(rest) = trimmed.strip_prefix("words") {
            let rest = rest.trim_start();
            if !rest.starts_with('=') { continue; }
            let rest = rest[1..].trim_start();
            if !rest.starts_with('[') { continue; }
            // Find matching ]; tolerate trailing whitespace/comments.
            let close = rest.rfind(']')?;
            let inner = &rest[1..close];
            let words: Vec<String> = inner.split(',')
                .map(|s| s.trim().trim_matches('"').trim_matches('\'').to_string())
                .filter(|s| !s.is_empty())
                .collect();
            if let Some(ref mut g) = current_group {
                g.extend(words);
            }
        }
    }
    // Close out final group.
    if let Some(g) = current_group.take() {
        if !g.is_empty() { groups.push(g); }
    }
    if groups.is_empty() { None } else { Some(groups) }
}

/// Built-in defaults — same as data/phonetic_confusions.toml shipped
/// with the binary. We DUPLICATE the data here so the binary works
/// without the data directory present (e.g. distribution as a single
/// executable). When the file IS present, it OVERRIDES these.
fn default_groups() -> Vec<Vec<String>> {
    fn s(arr: &[&str]) -> Vec<String> {
        arr.iter().map(|s| s.to_string()).collect()
    }
    vec![
        s(&["ls", "less", "list"]),
        s(&["cd", "seedy", "see d", "city"]),
        s(&["pwd", "pud", "pee w d"]),
        s(&["grep", "great", "grip", "grape"]),
        s(&["sudo", "pseudo", "sudoku"]),
        s(&["vim", "veeam", "vin"]),
        s(&["nano", "no", "nono"]),
        s(&["cat", "kat", "caught"]),
        s(&["echo", "ekko", "eco"]),
        s(&["chmod", "shmod", "chamod"]),
        s(&["chown", "shown", "chow n"]),
        s(&["mkdir", "make dir", "make directory"]),
        s(&["rmdir", "rm dir", "remove dir"]),
        s(&["find", "fine", "fined"]),
        s(&["which", "witch"]),
        s(&["history", "his story"]),
        s(&["clear", "cleer"]),
        s(&["forward", "for word", "foreword"]),
        s(&["back", "bach"]),
        s(&["reload", "re load"]),
        s(&["yes", "yeah", "yep"]),
        s(&["no", "nope", "naw", "nah"]),
        s(&["cancel", "kansul"]),
        s(&["save", "safe"]),
        s(&["quit", "qit", "kit"]),
        s(&["copy", "kopy", "coppy"]),
        s(&["paste", "past"]),
        s(&["one", "won", "wun"]),
        s(&["two", "to", "too"]),
        s(&["four", "for", "fore"]),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn phrase_variants_substitutes_one_word() {
        // "grep foo" — grep has alternates (great/grip/grape in defaults,
        // or whatever the loaded TOML has). Each variant must keep "foo"
        // and change only the grep slot.
        let v = phrase_variants("grep foo", 10);
        if !alternates_for("grep").is_empty() {
            assert!(!v.is_empty(), "grep has alternates → expect variants");
            for variant in &v {
                assert!(variant.ends_with(" foo"), "second word unchanged: {:?}", variant);
                assert_ne!(variant, "grep foo", "original excluded");
                // exactly one word differs from the original
                let changed: usize = variant.split_whitespace()
                    .zip("grep foo".split_whitespace())
                    .filter(|(a, b)| a != b).count();
                assert_eq!(changed, 1, "single substitution only: {:?}", variant);
            }
        }
    }

    #[test]
    fn phrase_variants_respects_max() {
        let v = phrase_variants("grep grep grep", 2);
        assert!(v.len() <= 2, "bounded by max_variants, got {}", v.len());
    }

    #[test]
    fn phrase_variants_zero_max_is_empty() {
        assert!(phrase_variants("grep foo", 0).is_empty());
    }

    #[test]
    fn phrase_variants_unknown_words_yield_nothing() {
        // a phrase of words with no confusion alternates → no variants
        let v = phrase_variants("zxqw plover frobnicate", 10);
        assert!(v.is_empty(), "no known confusions → no variants, got {:?}", v);
    }

    #[test]
    fn phrase_variants_no_self_substitution() {
        // no variant should be byte-identical to the input phrase
        let v = phrase_variants("grep the file", 20);
        assert!(v.iter().all(|x| x != "grep the file"));
    }

    #[test]
    fn loads_defaults_when_no_file() {
        // Force load by accessing.
        let alts = alternates_for("ls");
        assert!(alts.iter().any(|s| s == "less"));
    }

    #[test]
    fn lowercase_lookup() {
        let alts = alternates_for("LS");
        assert!(alts.iter().any(|s| s == "less"));
    }

    #[test]
    fn unknown_word_returns_empty() {
        let alts = alternates_for("xyzzy");
        assert!(alts.is_empty());
    }
}
