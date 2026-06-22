// src/confusion_table.rs
//
// Tracks systematic whisper mistranscriptions per user.
//
// When a user confirms an intent, we know:
// - The recognized text (what whisper produced)
// - The canonical intent (what we did)
//
// For Navigate intents to known sites, we have a "canonical phrase" — e.g. the
// resolved domain or the fingerprint label. By comparing recognized words to
// canonical words via Levenshtein distance, we can identify likely substitutions
// and learn them.
//
// Example:
//   recognized: "open the verge"
//   resolved:   Navigate(theverge.com)
//   canonical:  "open the verge"  → identical, no learning
//
//   recognized: "open hacker noose"
//   resolved:   Navigate(news.ycombinator.com), known_site = "hacker news"
//   canonical:  "open hacker news"
//   mismatched: "noose" → "news"  (edit distance 2, learn it)
//
// The table is then used during PARSING:
//   "open hacker noose" → tokenize → check confusion table for substitutions →
//     try with "noose" replaced by "news" → resolve_known_site succeeds → Navigate.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

const FILE_MAGIC: &[u8] = b"CT01";
const MAX_ENTRIES: usize = 500;
const MIN_CONFIRMATIONS: u32 = 2; // require seeing the same correction at least twice

#[derive(Default)]
pub struct ConfusionTable {
    /// misheard → [(intended, count)]
    pub map: HashMap<String, Vec<(String, u32)>>,
}

static TABLE: OnceLock<Mutex<ConfusionTable>> = OnceLock::new();

fn table() -> &'static Mutex<ConfusionTable> {
    TABLE.get_or_init(|| Mutex::new(ConfusionTable::default()))
}

/// Record that `misheard` was probably meant to be `intended`. This increments
/// count, or adds the entry if new.
pub fn record(misheard: &str, intended: &str) {
    if misheard.is_empty() || intended.is_empty() { return; }
    if misheard.eq_ignore_ascii_case(intended) { return; }
    if misheard.len() > 30 || intended.len() > 30 { return; }
    // Don't learn corrections that are too different (more than half changed)
    let dist = levenshtein(&misheard.to_lowercase(), &intended.to_lowercase());
    let max_len = misheard.len().max(intended.len());
    if dist as f32 / max_len as f32 > 0.6 { return; }

    let mut g = match table().lock() { Ok(g) => g, Err(_) => return };

    let mh = misheard.to_lowercase();
    let intd = intended.to_lowercase();

    let entries = g.map.entry(mh).or_default();
    if let Some((_, cnt)) = entries.iter_mut().find(|(t, _)| *t == intd) {
        *cnt += 1;
    } else {
        entries.push((intd, 1));
    }

    // Cap entries per misheard
    if entries.len() > 5 {
        entries.sort_by_key(|(_, c)| std::cmp::Reverse(*c));
        entries.truncate(5);
    }

    // Cap total entries
    if g.map.len() > MAX_ENTRIES {
        // Drop entries whose top correction has lowest count
        let mut kv: Vec<(String, u32)> = g.map.iter()
            .map(|(k, v)| (k.clone(), v.first().map(|(_, c)| *c).unwrap_or(0)))
            .collect();
        kv.sort_by_key(|(_, c)| *c);
        let drop_n = g.map.len() - MAX_ENTRIES;
        for (k, _) in kv.iter().take(drop_n) {
            g.map.remove(k);
        }
    }

    drop(g);
    save_to_disk();
}

/// Look up the most likely correction for a misheard token.
/// Returns the corrected word if confidence threshold is met (count >= MIN_CONFIRMATIONS).
pub fn correct(misheard: &str) -> Option<String> {
    let g = table().lock().ok()?;
    let entries = g.map.get(&misheard.to_lowercase())?;
    let (intended, cnt) = entries.first()?;
    if *cnt >= MIN_CONFIRMATIONS {
        Some(intended.clone())
    } else {
        None
    }
}

/// Apply learned corrections to a string. Token-by-token replacement.
pub fn apply_corrections(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut current = String::new();

    let flush = |buf: &mut String, dst: &mut String| {
        if buf.is_empty() { return; }
        if let Some(corr) = correct(buf) {
            dst.push_str(&corr);
        } else {
            dst.push_str(buf);
        }
        buf.clear();
    };

    for c in text.chars() {
        if c.is_alphanumeric() || c == '\'' {
            current.push(c);
        } else {
            flush(&mut current, &mut out);
            out.push(c);
        }
    }
    flush(&mut current, &mut out);
    out
}

/// Compare recognized text to canonical phrase, recording any substitutions
/// that look like systematic mistranscriptions.
pub fn learn_from_confirmation(recognized: &str, canonical: &str) {
    if recognized.is_empty() || canonical.is_empty() { return; }
    if recognized.eq_ignore_ascii_case(canonical) { return; }

    let r_words: Vec<String> = tokenize(recognized);
    let c_words: Vec<String> = tokenize(canonical);

    // If lengths differ wildly, don't try (likely deletion/insertion not substitution)
    if r_words.len().abs_diff(c_words.len()) > 1 { return; }

    // Simple position-by-position comparison; works when length matches
    if r_words.len() == c_words.len() {
        for (r, c) in r_words.iter().zip(c_words.iter()) {
            if r != c {
                // Only learn if word similarity is high (likely typo, not unrelated word)
                let dist = levenshtein(r, c);
                let max_len = r.len().max(c.len());
                if max_len >= 3 && dist <= max_len / 2 {
                    record(r, c);
                }
            }
        }
    }
}

fn tokenize(s: &str) -> Vec<String> {
    s.to_lowercase()
        .split(|c: char| !c.is_alphanumeric() && c != '\'')
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .collect()
}

/// Iterative two-row Levenshtein distance.
fn levenshtein(a: &str, b: &str) -> usize {
    let av: Vec<char> = a.chars().collect();
    let bv: Vec<char> = b.chars().collect();
    let (m, n) = (av.len(), bv.len());
    if m == 0 { return n; }
    if n == 0 { return m; }

    let mut prev: Vec<usize> = (0..=n).collect();
    let mut curr: Vec<usize> = vec![0; n + 1];

    for i in 1..=m {
        curr[0] = i;
        for j in 1..=n {
            let cost = if av[i-1] == bv[j-1] { 0 } else { 1 };
            curr[j] = (curr[j-1] + 1).min(prev[j] + 1).min(prev[j-1] + cost);
        }
        std::mem::swap(&mut prev, &mut curr);
    }
    prev[n]
}

pub fn save_to_disk() {
    let path = match db_path() { Some(p) => p, None => return };
    let _ = std::fs::create_dir_all(path.parent().unwrap());

    let g = match table().lock() { Ok(g) => g, Err(_) => return };

    let mut out: Vec<u8> = Vec::with_capacity(2048);
    out.extend_from_slice(FILE_MAGIC);
    out.extend_from_slice(&(g.map.len() as u32).to_le_bytes());

    for (mh, entries) in &g.map {
        let mb = mh.as_bytes();
        if mb.len() > 255 { continue; }
        out.push(mb.len() as u8);
        out.extend_from_slice(mb);
        out.push(entries.len().min(255) as u8);
        for (intd, cnt) in entries.iter().take(255) {
            let ib = intd.as_bytes();
            if ib.len() > 255 { continue; }
            out.push(ib.len() as u8);
            out.extend_from_slice(ib);
            out.extend_from_slice(&cnt.to_le_bytes());
        }
    }

    let _ = std::fs::write(&path, &out);
}

pub fn load_from_disk() {
    let path = match db_path() { Some(p) => p, None => return };
    if !path.exists() { return; }
    let bytes = match std::fs::read(&path) { Ok(b) => b, Err(_) => return };
    if bytes.len() < 8 || &bytes[0..4] != FILE_MAGIC { return; }

    let mut p = 4;

    fn read_u8(b: &[u8], p: &mut usize) -> Option<u8> {
        let v = *b.get(*p)?; *p += 1; Some(v)
    }
    fn read_u32(b: &[u8], p: &mut usize) -> Option<u32> {
        if *p + 4 > b.len() { return None; }
        let v = u32::from_le_bytes([b[*p], b[*p+1], b[*p+2], b[*p+3]]);
        *p += 4; Some(v)
    }
    fn read_str(b: &[u8], p: &mut usize) -> Option<String> {
        let n = read_u8(b, p)? as usize;
        if *p + n > b.len() { return None; }
        let s = String::from_utf8_lossy(&b[*p..*p+n]).to_string();
        *p += n; Some(s)
    }

    let n_entries = match read_u32(&bytes, &mut p) { Some(v) => v, None => return };
    let mut new_table = ConfusionTable::default();

    for _ in 0..n_entries {
        let mh = match read_str(&bytes, &mut p) { Some(s) => s, None => return };
        let n_corrections = match read_u8(&bytes, &mut p) { Some(v) => v as usize, None => return };
        let mut entries = Vec::with_capacity(n_corrections);
        for _ in 0..n_corrections {
            let intd = match read_str(&bytes, &mut p) { Some(s) => s, None => return };
            let cnt = match read_u32(&bytes, &mut p) { Some(v) => v, None => return };
            entries.push((intd, cnt));
        }
        new_table.map.insert(mh, entries);
    }

    if let Ok(mut g) = table().lock() {
        *g = new_table;
        eprintln!("ConfusionTable: loaded {} entries", g.map.len());
    }
}

fn db_path() -> Option<std::path::PathBuf> {
    let home = std::env::var("HOME").ok()?;
    Some(std::path::PathBuf::from(home).join(".config/web2qml/confusions.bin"))
}

pub fn entry_count() -> usize {
    table().lock().map(|g| g.map.len()).unwrap_or(0)
}

/// v0.1.281 (voice accuracy): seed the confusion table with
/// universally-known whisper.cpp mistranscriptions for the
/// command vocabulary. Called once at startup AFTER
/// load_from_disk, so user-learned corrections take precedence
/// (the count for user-learned > seed's count of 0).
///
/// These are hand-curated based on common whisper-cpp output
/// observations on tiny/base English models. Each entry is a
/// word that whisper often produces when the user clearly said
/// a different command-relevant word.
///
/// Bias: we ONLY seed pairs where the correction is HEAVILY
/// command-aligned. Generic English homophones (read/red,
/// to/too) stay unfixed because correcting them would hurt
/// dictation accuracy.
///
/// Idempotent — calling twice doesn't duplicate.
pub fn seed_universal_confusions() {
    // Seeds use a count of 0 so any user-recorded correction
    // (which starts at 1+) overrides this on the same word.
    // The threshold for applying corrections is
    // MIN_CONFIRMATIONS=2, so seeds wouldn't apply on their
    // own ... but `correct()` returns the highest-count
    // intended IF count >= MIN_CONFIRMATIONS. Bypass: seeds
    // use a special count value of CONFIRM_BYPASS that
    // correct() treats as "trust the seed."
    //
    // Easier: just use count = MIN_CONFIRMATIONS+1 = 3 for
    // seeds so they apply immediately. User-recorded
    // corrections that DISAGREE with the seed will accumulate
    // and overtake it eventually (count grows past the seed's
    // 3 with enough confirmations).
    const SEED_COUNT: u32 = MIN_CONFIRMATIONS + 1;

    let pairs: &[(&str, &str)] = &[
        // Common command-vocab mistranscriptions seen across
        // tiny.en / base.en runs:
        ("scroll", "scroll"),  // identity — placeholder; real pair below
        ("scrolldown", "scroll down"),
        ("scrollup", "scroll up"),
        // "open" gets transcribed as "opened" / "oppen" sometimes
        ("oppen", "open"),
        // "github" gets various forms
        ("git hub", "github"),
        ("gethub", "github"),
        // "homepage" → "home page" already handled by parser
        // splitting, but "hone page" appears
        ("hone", "home"),
        // Browser-vocab
        ("backwards", "back"),
        ("forwards", "forward"),
        // Numbers misheard for command words
        ("for", "for"),  // identity placeholder
        // "scrolled" → "scroll" when user says command form
        ("scrolled", "scroll"),
        // "click" common mishears
        ("clique", "click"),
        ("clic", "click"),
        // "search" common mishears
        ("surge", "search"),
        ("serge", "search"),
        // "settings"
        ("setting", "settings"),
        // "terminal"
        ("term knoll", "terminal"),
        ("term ninal", "terminal"),
        // "audit" — recent vocab from GITLOG arc
        ("ought it", "audit"),
        ("auto it", "audit"),
        // "branch"
        ("brunch", "branch"),
        ("branched", "branch"),
        ("brunches", "branches"),
        // "rollback" / "undo"
        ("roll back", "rollback"),
        ("rolled back", "rollback"),
        // "history"
        ("histories", "history"),
        // Common name mishears (single-token speakers handled
        // by speaker-claim fuzzy match already; these are
        // generic patterns that ALSO help speaker identity
        // when the same misheard word appears as a name).
        // -- intentionally none here; speaker disambiguation
        //    is a separate path.
    ];

    let mut g = match table().lock() { Ok(g) => g, Err(_) => return };
    let mut added = 0;
    for &(misheard, intended) in pairs {
        if misheard == intended { continue; }  // identity skip
        let mh = misheard.to_lowercase();
        let intd = intended.to_lowercase();
        let entries = g.map.entry(mh.clone()).or_default();
        // Skip if user already has any correction for this
        // misheard word — user data wins. Also skip if seed
        // already present (idempotent).
        if entries.iter().any(|(i, _)| i == &intd) { continue; }
        if !entries.is_empty() {
            // User data exists for this misheard word — seed
            // only if WE don't already have it AND user's
            // count is low enough that we won't conflict.
            // Conservative: skip entirely if user data is
            // present at all.
            continue;
        }
        entries.push((intd, SEED_COUNT));
        added += 1;
    }
    if added > 0 {
        eprintln!("ConfusionTable: seeded {} universal command-vocab corrections", added);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Pin the contract that apply_corrections is byte-for-byte
    /// identical when no corrections are configured. Catches
    /// regressions where the tokenizer accidentally mangles
    /// input (e.g. by dropping trailing punctuation it shouldn't).
    #[test]
    fn apply_no_op_when_empty() {
        // Note: TABLE is global so this can't be 100% isolated
        // from other tests. We test the IDENTITY case where the
        // corrections lookup won't find anything for these
        // unique tokens.
        let s = "xyzqwerty12345abc";
        let r = apply_corrections(s);
        assert_eq!(r, s);
    }

    /// Tokenizer should preserve punctuation and spaces verbatim,
    /// only transforming alphanumeric token contents.
    #[test]
    fn apply_preserves_punctuation_and_spaces() {
        // Use a token unlikely to be in the seed list.
        let s = "  hello, foo: bar; baz!  ";
        let r = apply_corrections(s);
        // Spacing and punctuation preserved exactly.
        assert_eq!(r.len(), s.len(),
            "punctuation/space length should be preserved");
        // Specific punctuation positions remain.
        assert!(r.contains("  "));
        assert!(r.contains(", "));
        assert!(r.contains(": "));
        assert!(r.contains("; "));
        assert!(r.contains("!  "));
    }

    /// Levenshtein basic sanity. The function is private to
    /// the module, so we test it through the public record path
    /// — high-distance pairs get rejected, low-distance pairs
    /// get accepted.
    #[test]
    fn record_rejects_overly_distant_pairs() {
        // record() rejects when dist/max_len > 0.6.
        // v0.1.466: the prior version of this test PADDED the words
        // ("zzcat99"/"zzdog99") which made them SIMILAR (shared
        // zz..99, only cat↔dog differs → dist 3 / max_len 7 ≈ 0.43,
        // BELOW the 0.6 threshold → correctly ACCEPTED). So the test
        // asserting rejection was wrong, not the code. Use genuinely
        // distant strings: "qx"→"mnbvc" share nothing, dist 5,
        // max_len 5, ratio 1.0 > 0.6 → rejected. Distinctive tokens
        // avoid colliding with other tests' global-table state.
        record("qx", "mnbvc");
        let g = table().lock().unwrap();
        let entries = g.map.get("qx");
        assert!(entries.is_none() || entries.unwrap().iter().all(|(i, _)| i != "mnbvc"),
            "qx→mnbvc should be rejected (distance ratio 1.0 > 0.6)");
    }

    /// Identity pairs are skipped without inserting empty
    /// entries.
    #[test]
    fn record_skips_identity() {
        let before = table().lock().unwrap().map.len();
        record("identical_word_x9", "identical_word_x9");
        let after = table().lock().unwrap().map.len();
        assert_eq!(before, after);
    }
}
