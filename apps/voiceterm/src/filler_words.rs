// src/filler_words.rs
//
// v0.1.264: corpus-learned filler-word filter. Replaces the
// previous hardcoded filler prefixes (strip "so ", "and ", etc.)
// with a learnable list of utterances the user has rejected
// enough times that we should drop them silently.
//
// Filed from v0.1.249 hardware test: user said "Mh mhmmm" — a
// thinking-sound — and it got treated as a parseable command.
// User direction:
//   "any way we can let the corpus decide this and not a default?"
//
// Mechanism:
//   1. When a voice popup closes without executing (user
//      cancelled or X-tapped), the recognized text is registered
//      as a candidate filler via `register_rejection(text)`.
//   2. The store maintains a count of rejections per
//      normalized text.
//   3. When count >= REJECTION_THRESHOLD (3), the text is
//      considered an "active filler." Future utterances matching
//      it get silently discarded by `is_filler(text)`.
//   4. Only short utterances (< 5 words) ever become fillers —
//      a 20-word utterance is something specific the user said,
//      not a filler, even if they cancelled it a few times.
//
// Storage: `~/.config/web2qml/filler_words.toml`.
//
// Format (manually editable):
//   [fillers]
//   "mh mhmmm" = 4
//   "so um" = 2
//
// Best-effort persistence: file I/O errors are logged but don't
// block the voice pipeline. State is kept in-memory between
// saves so brief unavailability of disk doesn't lose learning.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;

/// Number of rejections of the same normalized utterance before
/// it's promoted to "active filler" status. 3 chosen because:
///   - 1 is too few; users cancel single utterances for many
///     reasons (changed mind, picked wrong app, etc.)
///   - 5+ is too patient; a true filler will get hit fast
///   - 3 hits a balance: meaningful pattern but quick to learn
const REJECTION_THRESHOLD: u32 = 3;

/// Maximum word count for an utterance to be eligible as a
/// filler. Above this, the text is too specific to be a generic
/// filler — even if rejected many times, it's something the
/// user is actually trying to say.
const MAX_FILLER_WORDS: usize = 4;

/// In-memory rejection counts. Loaded from disk on first access.
/// Mutex-protected for thread safety; the voice path may be
/// invoked from multiple threads.
static REJECTIONS: Mutex<Option<HashMap<String, u32>>> = Mutex::new(None);

/// Returns the storage path. None when $HOME isn't set (build
/// environments).
fn store_path() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    let mut p = PathBuf::from(home);
    p.push(".config");
    p.push("web2qml");
    let _ = std::fs::create_dir_all(&p);
    p.push("filler_words.toml");
    Some(p)
}

/// Normalize an utterance for matching. Lowercase, collapse
/// runs of whitespace, strip leading/trailing whitespace. Strip
/// terminal punctuation since "mh mhmm" vs "mh mhmm." should
/// match.
pub fn normalize(text: &str) -> String {
    let lower = text.to_lowercase();
    let mut out = String::with_capacity(lower.len());
    let mut prev_space = true;  // suppress leading whitespace
    for c in lower.chars() {
        if c.is_whitespace() {
            if !prev_space {
                out.push(' ');
                prev_space = true;
            }
        } else if matches!(c, '.' | ',' | '!' | '?' | ';' | ':' | '"' | '\'') {
            // Skip terminal/intra-word punctuation. "mh mhmm" =
            // "mh mhmm." for filler-matching purposes.
            continue;
        } else {
            out.push(c);
            prev_space = false;
        }
    }
    out.trim().to_string()
}

/// Load rejection counts from disk. Idempotent — does nothing if
/// already loaded. Best-effort: I/O errors leave the in-memory
/// state empty (no fillers learned yet).
fn ensure_loaded() {
    let mut guard = match REJECTIONS.lock() {
        Ok(g) => g,
        Err(_) => return,
    };
    if guard.is_some() { return; }
    *guard = Some(load_from_disk());
}

fn load_from_disk() -> HashMap<String, u32> {
    let path = match store_path() { Some(p) => p, None => return HashMap::new() };
    let content = match std::fs::read_to_string(&path) {
        Ok(s) => s,
        Err(_) => return HashMap::new(),
    };
    parse_toml(&content)
}

/// Minimal TOML parser for our two-line format. We don't pull
/// in the `toml` crate because (a) it's heavy for this single
/// use, (b) our format is trivially regular and parseable with
/// 30 lines of string-splitting.
///
/// Expected format:
///   [fillers]
///   "mh mhmmm" = 4
///   "so um" = 2
///
/// Anything outside [fillers] is ignored. Lines without `=` are
/// ignored. Values must parse as u32, else line is skipped.
fn parse_toml(content: &str) -> HashMap<String, u32> {
    let mut out = HashMap::new();
    let mut in_fillers = false;
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('#') || trimmed.is_empty() { continue; }
        if trimmed.starts_with('[') {
            in_fillers = trimmed == "[fillers]";
            continue;
        }
        if !in_fillers { continue; }
        let eq = match trimmed.find('=') { Some(i) => i, None => continue };
        let key_part = trimmed[..eq].trim();
        let val_part = trimmed[eq+1..].trim();
        // Strip quotes from key.
        let key = key_part.trim_matches('"').trim_matches('\'').to_string();
        if key.is_empty() { continue; }
        let count: u32 = match val_part.parse() {
            Ok(n) => n,
            Err(_) => continue,
        };
        out.insert(key, count);
    }
    out
}

/// Persist the in-memory rejection counts to disk. Best-effort.
fn save_to_disk(state: &HashMap<String, u32>) {
    let path = match store_path() { Some(p) => p, None => return };
    let mut content = String::from("# web2qml filler-word rejections. Auto-learned from\n");
    content.push_str("# voice popups the user cancelled without executing.\n");
    content.push_str("# Entries with count >= ");
    content.push_str(&REJECTION_THRESHOLD.to_string());
    content.push_str(" are silently dropped from\n# voice pipeline. Edit by hand to add/remove fillers.\n\n");
    content.push_str("[fillers]\n");
    let mut keys: Vec<&String> = state.keys().collect();
    keys.sort();
    for k in keys {
        let escaped = k.replace('"', "");
        content.push('"');
        content.push_str(&escaped);
        content.push('"');
        content.push_str(" = ");
        content.push_str(&state[k].to_string());
        content.push('\n');
    }
    if let Err(e) = std::fs::write(&path, content) {
        crate::clog!("filler_words", "save failed: {}", e);
    }
}

/// Register a rejection. The text gets normalized and its
/// rejection count incremented. If the count reaches the
/// threshold, future calls to `is_filler` will return true for
/// equivalent normalized text.
///
/// Eligibility check: text must be short (<= MAX_FILLER_WORDS).
/// Long utterances are intentionally never promoted to fillers.
pub fn register_rejection(text: &str) {
    let normalized = normalize(text);
    if normalized.is_empty() { return; }
    let word_count = normalized.split_whitespace().count();
    if word_count > MAX_FILLER_WORDS {
        return;
    }
    ensure_loaded();
    let mut guard = match REJECTIONS.lock() {
        Ok(g) => g,
        Err(_) => return,
    };
    let map = match guard.as_mut() {
        Some(m) => m,
        None => return,
    };
    let entry = map.entry(normalized.clone()).or_insert(0);
    *entry += 1;
    let new_count = *entry;
    if new_count == REJECTION_THRESHOLD {
        // Just promoted to filler. Log so user can see what's
        // being silenced.
        crate::clog!("filler_words", "promoted to filler: {:?} (count={})",
            normalized, new_count);
    }
    let snapshot = map.clone();
    drop(guard);
    save_to_disk(&snapshot);
}

/// Returns true if this text matches a known filler with count
/// at or above the rejection threshold. Voice pipeline calls
/// this after transcription to decide whether to surface the
/// popup or silently drop.
///
/// Always returns false for utterances longer than
/// MAX_FILLER_WORDS — they can't be fillers by design, so we
/// short-circuit before even consulting the store.
pub fn is_filler(text: &str) -> bool {
    let normalized = normalize(text);
    if normalized.is_empty() { return false; }
    let word_count = normalized.split_whitespace().count();
    if word_count > MAX_FILLER_WORDS { return false; }
    ensure_loaded();
    let guard = match REJECTIONS.lock() {
        Ok(g) => g,
        Err(_) => return false,
    };
    let map = match guard.as_ref() {
        Some(m) => m,
        None => return false,
    };
    map.get(&normalized).map(|c| *c >= REJECTION_THRESHOLD).unwrap_or(false)
}

/// v0.1.264: list all filler entries (for UI inspection, debug,
/// or future "manage fillers" page). Returns (text, count)
/// pairs sorted by count descending then alphabetically.
pub fn list_all() -> Vec<(String, u32)> {
    ensure_loaded();
    let guard = match REJECTIONS.lock() {
        Ok(g) => g,
        Err(_) => return Vec::new(),
    };
    let map = match guard.as_ref() {
        Some(m) => m,
        None => return Vec::new(),
    };
    let mut entries: Vec<(String, u32)> =
        map.iter().map(|(k, v)| (k.clone(), *v)).collect();
    entries.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    entries
}

/// Remove a filler entry. Returns true if it existed.
pub fn remove(text: &str) -> bool {
    let normalized = normalize(text);
    ensure_loaded();
    let mut guard = match REJECTIONS.lock() {
        Ok(g) => g,
        Err(_) => return false,
    };
    let map = match guard.as_mut() {
        Some(m) => m,
        None => return false,
    };
    let removed = map.remove(&normalized).is_some();
    if removed {
        let snapshot = map.clone();
        drop(guard);
        save_to_disk(&snapshot);
    }
    removed
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex as StdMutex;

    // Tests touch the global state, so serialize them.
    static TEST_LOCK: StdMutex<()> = StdMutex::new(());

    fn reset_state() {
        if let Ok(mut g) = REJECTIONS.lock() {
            *g = Some(HashMap::new());
        }
    }

    #[test]
    fn normalize_strips_case_and_punctuation() {
        assert_eq!(normalize("Mh Mhmmm."), "mh mhmmm");
        assert_eq!(normalize("So Um..."), "so um");
        assert_eq!(normalize("  Hello,   world!  "), "hello world");
    }

    #[test]
    fn normalize_empty_input() {
        assert_eq!(normalize(""), "");
        assert_eq!(normalize("   "), "");
    }

    #[test]
    fn register_increments_count() {
        let _g = TEST_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        reset_state();
        register_rejection("Mh");
        register_rejection("Mh");
        let map = REJECTIONS.lock().unwrap();
        let m = map.as_ref().unwrap();
        assert_eq!(*m.get("mh").unwrap(), 2);
    }

    #[test]
    fn is_filler_only_after_threshold() {
        let _g = TEST_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        reset_state();
        register_rejection("uh");
        assert!(!is_filler("uh"), "1 rejection shouldn't make it filler");
        register_rejection("uh");
        assert!(!is_filler("uh"), "2 rejections shouldn't make it filler");
        register_rejection("uh");
        assert!(is_filler("uh"), "3 rejections should promote to filler");
        // Different case + punctuation should still match.
        assert!(is_filler("Uh."), "case/punct should normalize");
    }

    #[test]
    fn long_utterances_never_promoted() {
        let _g = TEST_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        reset_state();
        let long = "this is a long utterance that should not become filler";
        register_rejection(long);
        register_rejection(long);
        register_rejection(long);
        register_rejection(long);
        register_rejection(long);
        assert!(!is_filler(long),
            "long utterances should never be promoted to filler");
    }

    #[test]
    fn at_word_count_boundary() {
        let _g = TEST_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        reset_state();
        // Exactly MAX_FILLER_WORDS (4) is allowed.
        let four = "um like you know";
        register_rejection(four);
        register_rejection(four);
        register_rejection(four);
        assert!(is_filler(four),
            "4-word utterance should be eligible (= MAX_FILLER_WORDS)");
        // 5+ is not.
        let five = "um like you know what";
        register_rejection(five);
        register_rejection(five);
        register_rejection(five);
        register_rejection(five);
        assert!(!is_filler(five),
            "5-word utterance should not be filler");
    }

    #[test]
    fn empty_string_not_filler() {
        let _g = TEST_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        reset_state();
        assert!(!is_filler(""));
        assert!(!is_filler("   "));
    }

    #[test]
    fn list_all_sorted_by_count_desc() {
        let _g = TEST_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        reset_state();
        for _ in 0..5 { register_rejection("aaa"); }
        for _ in 0..3 { register_rejection("bbb"); }
        for _ in 0..3 { register_rejection("ccc"); }
        let entries = list_all();
        // Most-rejected first, then alphabetic.
        assert_eq!(entries[0].0, "aaa");
        assert_eq!(entries[0].1, 5);
        assert_eq!(entries[1].0, "bbb");
        assert_eq!(entries[2].0, "ccc");
    }

    #[test]
    fn remove_clears_entry() {
        let _g = TEST_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        reset_state();
        for _ in 0..5 { register_rejection("oops"); }
        assert!(is_filler("oops"));
        assert!(remove("oops"));
        assert!(!is_filler("oops"));
        // Removing again returns false.
        assert!(!remove("oops"));
    }

    #[test]
    fn toml_roundtrip() {
        let mut state = HashMap::new();
        state.insert("mh mhmmm".to_string(), 4u32);
        state.insert("so um".to_string(), 2u32);
        // Manually round-trip the format.
        let mut content = String::from("[fillers]\n");
        let mut keys: Vec<&String> = state.keys().collect();
        keys.sort();
        for k in keys {
            content.push('"');
            content.push_str(k);
            content.push('"');
            content.push_str(" = ");
            content.push_str(&state[k].to_string());
            content.push('\n');
        }
        let parsed = parse_toml(&content);
        assert_eq!(parsed.len(), 2);
        assert_eq!(*parsed.get("mh mhmmm").unwrap(), 4);
        assert_eq!(*parsed.get("so um").unwrap(), 2);
    }

    #[test]
    fn toml_skips_malformed_lines() {
        let content = "[fillers]\n\"good\" = 5\nmalformed_no_equals\n\"bad\" = not_a_number\n\"another good\" = 3\n";
        let parsed = parse_toml(content);
        assert_eq!(parsed.len(), 2);
        assert_eq!(*parsed.get("good").unwrap(), 5);
        assert_eq!(*parsed.get("another good").unwrap(), 3);
    }

    #[test]
    fn toml_ignores_non_filler_section() {
        let content = "[other]\n\"x\" = 99\n[fillers]\n\"real\" = 4\n[third]\n\"y\" = 1\n";
        let parsed = parse_toml(content);
        assert_eq!(parsed.len(), 1);
        assert_eq!(*parsed.get("real").unwrap(), 4);
    }
}
