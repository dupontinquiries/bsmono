//! voice_confusion.rs — a self-adapting table of systematic whisper mistranscriptions, ported from
//! voiceterm's confusion_table. Two ways corrections get in:
//!   1. Seeded: a small list of known command-vocab mishears ("brunch" → "branch") that apply immediately.
//!   2. Learned: `record(misheard, intended)` — call this wherever a correction is confirmed (e.g. a future
//!      "no, I meant branch" follow-up). A correction only APPLIES once it's been recorded twice
//!      (MIN_CONFIRMATIONS), so a one-off transcription slip doesn't silently start rewriting words.
//! `apply_corrections(text)` runs as a pre-pass in `voice_intent::parse`, before command matching, so a
//! systematically-misheard command word gets fixed before the parser ever sees it. Persisted to
//! `~/.config/logscope/confusions.bin` so learning survives restarts. Pure logic (levenshtein, the seed
//! gate) is unit-tested; the table itself is process-global mutable state (matches the original design).

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

const FILE_MAGIC: &[u8] = b"LSC1";
const MAX_ENTRIES: usize = 500;
const MIN_CONFIRMATIONS: u32 = 2; // require the same correction to be recorded twice before it applies
const SEED_COUNT: u32 = MIN_CONFIRMATIONS + 1; // seeds apply immediately; a disagreeing user correction can still overtake one with enough confirmations

#[derive(Default)]
struct ConfusionTable {
    /// misheard → [(intended, count)], sorted by count desc after each insert.
    map: HashMap<String, Vec<(String, u32)>>,
}

static TABLE: OnceLock<Mutex<ConfusionTable>> = OnceLock::new();
fn table() -> &'static Mutex<ConfusionTable> { TABLE.get_or_init(|| Mutex::new(ConfusionTable::default())) }

/// Record that `misheard` was probably meant to be `intended`. Rejects no-op/empty/overlong pairs and
/// corrections too different to be a plausible mishear (>60% of characters changed).
pub fn record(misheard: &str, intended: &str) {
    if misheard.is_empty() || intended.is_empty() { return; }
    if misheard.eq_ignore_ascii_case(intended) { return; }
    if misheard.len() > 30 || intended.len() > 30 { return; }
    let dist = levenshtein(&misheard.to_lowercase(), &intended.to_lowercase());
    let max_len = misheard.len().max(intended.len());
    if dist as f32 / max_len as f32 > 0.6 { return; }

    let Ok(mut g) = table().lock() else { return };
    let (mh, intd) = (misheard.to_lowercase(), intended.to_lowercase());
    let entries = g.map.entry(mh).or_default();
    match entries.iter_mut().find(|(t, _)| *t == intd) {
        Some((_, cnt)) => *cnt += 1,
        None => entries.push((intd, 1)),
    }
    if entries.len() > 5 { entries.sort_by_key(|(_, c)| std::cmp::Reverse(*c)); entries.truncate(5); }
    if g.map.len() > MAX_ENTRIES {
        let mut kv: Vec<(String, u32)> = g.map.iter().map(|(k, v)| (k.clone(), v.first().map(|(_, c)| *c).unwrap_or(0))).collect();
        kv.sort_by_key(|(_, c)| *c);
        for (k, _) in kv.iter().take(g.map.len() - MAX_ENTRIES) { g.map.remove(k); }
    }
    drop(g);
    save_to_disk();
}

/// The most likely correction for a misheard token, if it's crossed the confirmation threshold.
pub fn correct(misheard: &str) -> Option<String> {
    let g = table().lock().ok()?;
    let entries = g.map.get(&misheard.to_lowercase())?;
    let (intended, cnt) = entries.first()?;
    (*cnt >= MIN_CONFIRMATIONS).then(|| intended.clone())
}

/// Apply learned corrections token-by-token. Non-alphanumeric runs (spaces, punctuation) pass through
/// unchanged, so this is safe to run on a whole utterance before intent parsing.
pub fn apply_corrections(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut cur = String::new();
    let flush = |buf: &mut String, dst: &mut String| {
        if buf.is_empty() { return; }
        match correct(buf) { Some(c) => dst.push_str(&c), None => dst.push_str(buf) }
        buf.clear();
    };
    for c in text.chars() {
        if c.is_alphanumeric() || c == '\'' { cur.push(c); } else { flush(&mut cur, &mut out); out.push(c); }
    }
    flush(&mut cur, &mut out);
    out
}

/// Pure Levenshtein (character edit distance) — used to gate `record` against implausible "corrections".
fn levenshtein(a: &str, b: &str) -> usize {
    let (av, bv): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
    let (m, n) = (av.len(), bv.len());
    if m == 0 { return n; }
    if n == 0 { return m; }
    let mut prev: Vec<usize> = (0..=n).collect();
    let mut curr: Vec<usize> = vec![0; n + 1];
    for i in 1..=m {
        curr[0] = i;
        for j in 1..=n {
            let cost = if av[i - 1] == bv[j - 1] { 0 } else { 1 };
            curr[j] = (curr[j - 1] + 1).min(prev[j] + 1).min(prev[j - 1] + cost);
        }
        std::mem::swap(&mut prev, &mut curr);
    }
    prev[n]
}

fn db_path() -> Option<std::path::PathBuf> {
    let home = std::env::var("HOME").ok()?;
    Some(std::path::PathBuf::from(home).join(".config/logscope/confusions.bin"))
}

/// Persist the table (misheard-len-prefixed strings + counts). Best-effort; never panics.
pub fn save_to_disk() {
    let Some(path) = db_path() else { return };
    if let Some(parent) = path.parent() { let _ = std::fs::create_dir_all(parent); }
    let Ok(g) = table().lock() else { return };
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

/// Load a previously-saved table. No-op if the file doesn't exist or is malformed.
pub fn load_from_disk() {
    let Some(path) = db_path() else { return };
    if !path.exists() { return; }
    let Ok(bytes) = std::fs::read(&path) else { return };
    if bytes.len() < 8 || &bytes[0..4] != FILE_MAGIC { return; }
    let mut p = 4usize;
    fn read_u8(b: &[u8], p: &mut usize) -> Option<u8> { let v = *b.get(*p)?; *p += 1; Some(v) }
    fn read_u32(b: &[u8], p: &mut usize) -> Option<u32> {
        if *p + 4 > b.len() { return None; }
        let v = u32::from_le_bytes([b[*p], b[*p + 1], b[*p + 2], b[*p + 3]]); *p += 4; Some(v)
    }
    fn read_str(b: &[u8], p: &mut usize) -> Option<String> {
        let n = read_u8(b, p)? as usize;
        if *p + n > b.len() { return None; }
        let s = String::from_utf8_lossy(&b[*p..*p + n]).to_string(); *p += n; Some(s)
    }
    let Some(n_entries) = read_u32(&bytes, &mut p) else { return };
    let mut new_table = ConfusionTable::default();
    for _ in 0..n_entries {
        let Some(mh) = read_str(&bytes, &mut p) else { return };
        let Some(n_corr) = read_u8(&bytes, &mut p) else { return };
        let mut entries = Vec::with_capacity(n_corr as usize);
        for _ in 0..n_corr {
            let (Some(intd), Some(cnt)) = (read_str(&bytes, &mut p), read_u32(&bytes, &mut p)) else { return };
            entries.push((intd, cnt));
        }
        new_table.map.insert(mh, entries);
    }
    if let Ok(mut g) = table().lock() {
        let n = new_table.map.len();
        *g = new_table;
        crate::clog!("voice", "confusion table: loaded {n} entries");
    }
}

/// Seed a small set of command-vocab mishears that apply immediately (count = SEED_COUNT). Idempotent —
/// skipped per-word if the user already has any correction recorded for it (user data always wins).
/// Bias: only pairs heavily aligned to OUR command vocabulary — generic English homophones are left alone
/// since "fixing" them would hurt ordinary dictation.
pub fn seed_universal_confusions() {
    let pairs: &[(&str, &str)] = &[
        // graph navigation / voiceterm-proven universal command mishears
        ("brunch", "branch"), ("branched", "branch"), ("brunches", "branches"),
        ("oppen", "open"), ("overwatch", "overview"), ("overview'd", "overview"),
        ("prewn", "prune"), ("proon", "prune"), ("unprewn", "unprune"),
        ("free camp", "freecam"), ("free camb", "freecam"), ("free cams", "freecam"),
        ("tore", "tour"),
        ("bess", "best"),
        // coder-dictation symbol names that commonly run together as one whisper token
        ("colin", "colon"), ("commas", "comma"),
    ];
    let Ok(mut g) = table().lock() else { return };
    let mut added = 0u32;
    for &(misheard, intended) in pairs {
        if misheard == intended { continue; }
        let (mh, intd) = (misheard.to_lowercase(), intended.to_lowercase());
        let entries = g.map.entry(mh).or_default();
        if entries.iter().any(|(i, _)| *i == intd) { continue; } // already seeded
        if !entries.is_empty() { continue; } // user data present for this word — user wins, skip seeding
        entries.push((intd, SEED_COUNT));
        added += 1;
    }
    if added > 0 { crate::clog!("voice", "confusion table: seeded {added} universal command-vocab corrections"); }
}

/// Entry count, for diagnostics.
pub fn entry_count() -> usize { table().lock().map(|g| g.map.len()).unwrap_or(0) }

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn levenshtein_basic() {
        assert_eq!(levenshtein("branch", "branch"), 0);
        assert_eq!(levenshtein("brunch", "branch"), 1);
        assert_eq!(levenshtein("", "abc"), 3);
    }

    #[test]
    fn record_requires_two_confirmations_then_applies() {
        record("zqtestwrd", "zqtestword"); // one-char insertion — a plausible mishear
        assert_eq!(correct("zqtestwrd"), None, "one confirmation is not enough");
        record("zqtestwrd", "zqtestword");
        assert_eq!(correct("zqtestwrd"), Some("zqtestword".to_string()), "two confirmations should apply");
    }

    #[test]
    fn record_rejects_implausible_corrections() {
        // "xyz" -> "completely different word" is far too different an edit to be a plausible mishear
        record("zzzxyz2", "zzzcompletelydifferentword2");
        record("zzzxyz2", "zzzcompletelydifferentword2");
        assert_eq!(correct("zzzxyz2"), None, "corrections >60% different should be rejected");
    }

    #[test]
    fn apply_corrections_is_token_safe() {
        record("zqfoox", "zqfooy"); // one-char swap — plausible mishear
        record("zqfoox", "zqfooy");
        assert_eq!(apply_corrections("go zqfoox now"), "go zqfooy now");
        assert_eq!(apply_corrections("nothing to correct here"), "nothing to correct here");
    }

    #[test]
    fn seeded_command_words_apply_immediately() {
        seed_universal_confusions();
        assert_eq!(correct("brunch"), Some("branch".to_string()), "seeds should apply without user confirmation");
    }
}
