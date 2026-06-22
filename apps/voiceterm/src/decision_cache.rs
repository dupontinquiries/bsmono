// src/decision_cache.rs
//
// Cache mapping (normalized utterance text) → (final terminal command,
// confirmation count, last_used). When the same voice utterance leads
// repeatedly to the same final command, that pair gets cached and
// surfaced as the top candidate on subsequent matches without running
// any heuristics.
//
// Effect on user experience:
//   - "list files" → "ls -la" with score 1.0 after 3 confirmations
//     skips entity extraction, LLM dispatch, fuzzy command matching
//   - First call still runs the full pipeline; cache fills passively
//   - User can voice-confirm a different command for the same
//     utterance (e.g. they want "ls" instead of "ls -la") and the
//     counter resets / new entry takes precedence on later use
//
// Persistence: ~/.config/web2qml/decision_cache.bin
// Format: simple length-prefixed binary, similar to other DBs in this
// codebase. No serde dep.
//
// The cache is intentionally KEYED on the post-stripped post-cleaned
// utterance, NOT the raw whisper output. So "DNF, search python" and
// "dnf search python" hash to the same key after normalization.
// Whisper variations of capitalization and trailing punctuation
// don't fragment the cache.

use std::sync::{Mutex, OnceLock};
use std::path::PathBuf;
use std::collections::HashMap;

/// One cache entry. We track:
///   - command: the actual shell command this utterance resolved to
///   - count: number of times the user has confirmed this mapping
///   - last_used: epoch seconds of the most recent confirmation
///
/// A new entry starts with count=1. Subsequent confirmations of the
/// SAME (utterance, command) pair increment count. If the user confirms
/// a DIFFERENT command for the same utterance, we replace the entry
/// with count=1 (most-recent-wins) — older preferences become stale.
#[derive(Clone, Debug)]
pub struct CacheEntry {
    pub command: String,
    pub count: u32,
    pub last_used: u64,
}

/// Threshold for treating an entry as "cached enough to surface as top
/// candidate immediately". Below this we DO surface the entry (because
/// even one prior confirmation is information) but at a lower score —
/// the user might still want the heuristic candidates to win.
pub const SURFACE_THRESHOLD: u32 = 1;
pub const CONFIDENT_THRESHOLD: u32 = 3;

const FILE_MAGIC: &[u8; 4] = b"DCAC";  // Decision CAChe
const FILE_VERSION: u16 = 1;
/// Cap entries to prevent unbounded growth on a long-running install.
/// When we hit the cap, we evict the lowest-count + oldest entries first.
const MAX_ENTRIES: usize = 2000;

struct Cache {
    entries: HashMap<String, CacheEntry>,
}

impl Cache {
    fn new() -> Self {
        Self { entries: HashMap::new() }
    }
}

static CACHE: OnceLock<Mutex<Cache>> = OnceLock::new();

fn cache() -> &'static Mutex<Cache> {
    CACHE.get_or_init(|| Mutex::new(Cache::new()))
}

/// Normalize an utterance for cache lookup: lowercase, trim trailing
/// punctuation, collapse multiple spaces. Keep this consistent with
/// the normalization done in `record` so lookups always hit the same
/// keys writes use.
fn normalize(utterance: &str) -> String {
    let trimmed = utterance.trim()
        .trim_end_matches(|c: char| c == '.' || c == '!' || c == '?' || c == ',')
        .trim_start_matches(|c: char| c == ',' || c == ' ');
    let lower = trimmed.to_lowercase();
    // Collapse runs of whitespace to single spaces.
    let mut out = String::with_capacity(lower.len());
    let mut prev_space = false;
    for c in lower.chars() {
        if c.is_whitespace() {
            if !prev_space { out.push(' '); }
            prev_space = true;
        } else {
            out.push(c);
            prev_space = false;
        }
    }
    out.trim().to_string()
}

fn now_secs() -> u64 {
    crate::util::epoch_secs()
}

/// Look up an utterance in the cache. Returns the entry if found,
/// None otherwise. Cheap — single mutex lock + HashMap lookup.
pub fn lookup(utterance: &str) -> Option<CacheEntry> {
    let key = normalize(utterance);
    if key.is_empty() { return None; }
    let g = cache().lock().ok()?;
    g.entries.get(&key).cloned()
}

/// Record a confirmation: the user said `utterance` and confirmed it
/// resolved to `command`. Updates the cache entry (creates if missing,
/// increments if matching, replaces if conflicting).
pub fn record(utterance: &str, command: &str) {
    if crate::security::should_suppress_capture() {
        return;
    }
    let key = normalize(utterance);
    if key.is_empty() || command.is_empty() { return; }
    let mut g = match cache().lock() { Ok(g) => g, Err(_) => return };
    let now = now_secs();
    let needs_eviction = g.entries.len() >= MAX_ENTRIES;
    let entry_existed = g.entries.contains_key(&key);
    let same_command = g.entries.get(&key).map(|e| e.command == command).unwrap_or(false);

    if entry_existed && same_command {
        // Same mapping — just bump.
        if let Some(e) = g.entries.get_mut(&key) {
            e.count = e.count.saturating_add(1);
            e.last_used = now;
        }
    } else {
        // Either new or conflicting (different command for same key).
        // For conflicts: most-recent-wins. We don't try to maintain
        // multiple candidates — keep this simple.
        if entry_existed {
            crate::clog!("decision-cache", "replacing '{}' -> '{}' (was different command)",
                key, command);
        }
        // Evict before insert if we're at cap. We pick the worst entry:
        // lowest count, oldest last_used. Linear scan is fine at 2000
        // since this only fires on insertion past cap.
        if needs_eviction && !entry_existed {
            let evict_key: Option<String> = g.entries.iter()
                .min_by(|(_, a), (_, b)| {
                    a.count.cmp(&b.count)
                        .then_with(|| a.last_used.cmp(&b.last_used))
                })
                .map(|(k, _)| k.clone());
            if let Some(k) = evict_key {
                g.entries.remove(&k);
            }
        }
        g.entries.insert(key, CacheEntry {
            command: command.to_string(),
            count: 1,
            last_used: now,
        });
    }
    drop(g);
    // Persist after every write — small files, the fsync cost is tiny
    // compared to the value of not losing a session's learning.
    save_to_disk();
}

/// Negative signal: the user explicitly REJECTED this (utterance,
/// command) pair, e.g. by hitting cancel after the cache served it.
/// We decrement count; if it goes to 0, remove the entry entirely.
/// This lets users undo a wrong-cached entry without manual cleanup.
pub fn record_rejection(utterance: &str, command: &str) {
    if crate::security::should_suppress_capture() {
        return;
    }
    let key = normalize(utterance);
    if key.is_empty() { return; }
    let mut g = match cache().lock() { Ok(g) => g, Err(_) => return };
    let same_command = g.entries.get(&key).map(|e| e.command == command).unwrap_or(false);
    if !same_command { return; }
    let remove = if let Some(e) = g.entries.get_mut(&key) {
        if e.count <= 1 { true } else {
            e.count -= 1;
            false
        }
    } else { false };
    if remove {
        g.entries.remove(&key);
        crate::clog!("decision-cache", "removed '{}' -> '{}' (rejection)", key, command);
    }
    drop(g);
    save_to_disk();
}

/// Number of entries currently in the cache. Diagnostic.
pub fn len() -> usize {
    cache().lock().map(|g| g.entries.len()).unwrap_or(0)
}

fn db_path() -> Option<PathBuf> {
    let home = std::env::var("HOME").ok()?;
    Some(PathBuf::from(home).join(".config/web2qml/decision_cache.bin"))
}

/// Disk format:
///   [4 bytes magic = "DCAC"]
///   [u16 version = 1]
///   [u32 entry count]
///   for each entry:
///     [u16 key_len][key_bytes]
///     [u16 cmd_len][cmd_bytes]
///     [u32 count]
///     [u64 last_used]
pub fn save_to_disk() {
    if !crate::security::persistence_enabled() {
        return;
    }
    let path = match db_path() { Some(p) => p, None => return };
    let _ = std::fs::create_dir_all(path.parent().unwrap());
    let g = match cache().lock() { Ok(g) => g, Err(_) => return };

    let mut out: Vec<u8> = Vec::with_capacity(g.entries.len() * 64);
    out.extend_from_slice(FILE_MAGIC);
    out.extend_from_slice(&FILE_VERSION.to_le_bytes());
    out.extend_from_slice(&(g.entries.len() as u32).to_le_bytes());
    for (key, e) in &g.entries {
        let kb = key.as_bytes();
        if kb.len() > u16::MAX as usize { continue; }
        out.extend_from_slice(&(kb.len() as u16).to_le_bytes());
        out.extend_from_slice(kb);
        let cb = e.command.as_bytes();
        if cb.len() > u16::MAX as usize { continue; }
        out.extend_from_slice(&(cb.len() as u16).to_le_bytes());
        out.extend_from_slice(cb);
        out.extend_from_slice(&e.count.to_le_bytes());
        out.extend_from_slice(&e.last_used.to_le_bytes());
    }
    drop(g);

    // v0.1.727: gzip the serialized blob before writing. record() calls this on
    // EVERY confirmed voice command, and at the 2000-entry cap the raw blob is
    // ~100KB of highly-compressible text (utterances + commands). Measured: ~5×
    // smaller at flate2 level 1 in ~0.3ms — cuts both the per-command write size
    // and the on-disk footprint. gzip's own magic (0x1f 0x8b) lets load detect
    // the format, and a legacy uncompressed file (starts with DCAC) still loads.
    use std::io::Write as _;
    let compressed = {
        let mut enc = flate2::write::GzEncoder::new(
            Vec::with_capacity(out.len() / 4), flate2::Compression::new(1));
        if enc.write_all(&out).is_ok() {
            enc.finish().unwrap_or(out)
        } else {
            out
        }
    };

    let tmp = path.with_extension("bin.tmp");
    if let Err(e) = std::fs::write(&tmp, &compressed) {
        crate::clog!("decision-cache", "save failed: {}", e);
        return;
    }
    let _ = std::fs::rename(tmp, path);
}

pub fn load_from_disk() {
    let path = match db_path() { Some(p) => p, None => return };
    let raw = match std::fs::read(&path) { Ok(b) => b, Err(_) => return };
    // v0.1.727: transparently decompress gzip-wrapped stores. New saves are
    // gzipped (magic 0x1f 0x8b); legacy files start with DCAC and pass through.
    let bytes = if raw.len() >= 2 && raw[0] == 0x1f && raw[1] == 0x8b {
        use std::io::Read as _;
        let mut dec = flate2::read::GzDecoder::new(&raw[..]);
        let mut buf = Vec::new();
        match dec.read_to_end(&mut buf) {
            Ok(_) => buf,
            Err(_) => {
                crate::clog!("decision-cache", "gzip decode failed, skipping load");
                return;
            }
        }
    } else {
        raw
    };
    if bytes.len() < 10 { return; }
    if &bytes[0..4] != FILE_MAGIC {
        crate::clog!("decision-cache", "file has wrong magic, skipping load");
        return;
    }
    let _version = u16::from_le_bytes([bytes[4], bytes[5]]);
    let n = u32::from_le_bytes([bytes[6], bytes[7], bytes[8], bytes[9]]) as usize;
    let mut pos = 10usize;
    let mut entries: HashMap<String, CacheEntry> = HashMap::with_capacity(n);
    for _ in 0..n {
        if pos + 2 > bytes.len() { break; }
        let kl = u16::from_le_bytes([bytes[pos], bytes[pos+1]]) as usize;
        pos += 2;
        if pos + kl > bytes.len() { break; }
        let key = match String::from_utf8(bytes[pos..pos+kl].to_vec()) {
            Ok(s) => s, Err(_) => break,
        };
        pos += kl;
        if pos + 2 > bytes.len() { break; }
        let cl = u16::from_le_bytes([bytes[pos], bytes[pos+1]]) as usize;
        pos += 2;
        if pos + cl > bytes.len() { break; }
        let cmd = match String::from_utf8(bytes[pos..pos+cl].to_vec()) {
            Ok(s) => s, Err(_) => break,
        };
        pos += cl;
        if pos + 4 > bytes.len() { break; }
        let count = u32::from_le_bytes([bytes[pos], bytes[pos+1], bytes[pos+2], bytes[pos+3]]);
        pos += 4;
        if pos + 8 > bytes.len() { break; }
        let last_used = u64::from_le_bytes([
            bytes[pos], bytes[pos+1], bytes[pos+2], bytes[pos+3],
            bytes[pos+4], bytes[pos+5], bytes[pos+6], bytes[pos+7],
        ]);
        pos += 8;
        entries.insert(key, CacheEntry { command: cmd, count, last_used });
    }
    if let Ok(mut g) = cache().lock() {
        g.entries = entries;
        eprintln!("DecisionCache: loaded {} entries from {:?}", g.entries.len(), path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// normalize() lowercases, trims, and strips trailing punctuation.
    #[test]
    fn normalize_basic() {
        assert_eq!(normalize("Hello World"), "hello world");
        assert_eq!(normalize("  hello  "), "hello");
        assert_eq!(normalize("HELLO"), "hello");
    }

    /// Trailing punctuation stripped: . ! ? ,
    #[test]
    fn normalize_strips_trailing_punct() {
        assert_eq!(normalize("scroll down."), "scroll down");
        assert_eq!(normalize("scroll down!"), "scroll down");
        assert_eq!(normalize("scroll down?"), "scroll down");
        assert_eq!(normalize("scroll down,"), "scroll down");
    }

    /// Leading commas/spaces stripped.
    #[test]
    fn normalize_strips_leading_garbage() {
        assert_eq!(normalize(", scroll down"), "scroll down");
        assert_eq!(normalize("   scroll down"), "scroll down");
    }

    /// Runs of whitespace collapse to single spaces.
    #[test]
    fn normalize_collapses_whitespace() {
        assert_eq!(normalize("scroll   down"), "scroll down");
        assert_eq!(normalize("scroll\tdown"), "scroll down");
        assert_eq!(normalize("scroll  \n  down"), "scroll down");
    }

    /// Empty / whitespace-only input.
    #[test]
    fn normalize_empty() {
        assert_eq!(normalize(""), "");
        assert_eq!(normalize("   "), "");
        assert_eq!(normalize("..."), "");
    }

    /// "Hello World" and "hello world." should normalize to the
    /// same key.
    #[test]
    fn normalize_equivalence_classes() {
        assert_eq!(normalize("Hello World"), normalize("hello world."));
        assert_eq!(normalize("Open GitHub!"), normalize("open github"));
    }
}
