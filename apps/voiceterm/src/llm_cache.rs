// src/llm_cache.rs
//
// Cache mapping prompt hash → LLM response. Avoids re-spending Ollama
// CPU on identical user requests. Most useful for:
//
//   - Voice fragment classification ("is this a complete utterance or
//     mid-thought") which gets called repeatedly with similar inputs
//   - Voice intent disambiguation prompts
//   - Anything where the same prompt structure recurs across sessions
//
// NOT useful for:
//   - One-shot LLM tasks ("rewrite this paragraph") — each prompt is
//     unique, cache hit rate ~0%
//   - Time-sensitive prompts ("what's the news today") — caller should
//     bypass cache for those
//
// The caller decides whether to consult the cache. We don't auto-cache
// every Ollama query because some prompts SHOULD return different
// results on subsequent calls (e.g. anything stochastic or context-
// dependent the prompt didn't fully encode).
//
// Capacity: 500 entries. At ~1KB average response that's 500KB max
// disk usage. Entries evict by LRU when at cap.
//
// Persistence: ~/.config/web2qml/llm_cache.bin

use std::sync::{Mutex, OnceLock};
use std::path::PathBuf;
use std::collections::HashMap;

const FILE_MAGIC: &[u8; 4] = b"LLMC";
const FILE_VERSION: u16 = 1;
const MAX_ENTRIES: usize = 500;

/// Hash a prompt to a stable u64 key. We use FNV-1a — fast, good
/// distribution, no dep needed. Prompt collisions are theoretically
/// possible but vanishingly rare (2^64 keyspace) and the cost of a
/// false hit is at most one wrong cached answer, which the caller
/// could detect via response sanity-checking.
fn fnv1a(s: &str) -> u64 {
    crate::util::fnv1a(s)
}

#[derive(Clone, Debug)]
struct Entry {
    response: String,
    last_used: u64,
}

struct Cache {
    entries: HashMap<u64, Entry>,
}

impl Cache {
    fn new() -> Self { Self { entries: HashMap::new() } }
}

static CACHE: OnceLock<Mutex<Cache>> = OnceLock::new();

fn cache() -> &'static Mutex<Cache> {
    CACHE.get_or_init(|| Mutex::new(Cache::new()))
}

fn now_secs() -> u64 {
    crate::util::epoch_secs()
}

/// Look up a cached response for `prompt`. Returns None if no entry
/// exists. Updates last_used to keep LRU eviction current.
pub fn lookup(prompt: &str) -> Option<String> {
    let key = fnv1a(prompt);
    let mut g = cache().lock().ok()?;
    let resp = g.entries.get_mut(&key).map(|e| {
        e.last_used = now_secs();
        e.response.clone()
    });
    resp
}

/// Store a (prompt, response) pair in the cache. Evicts oldest entry
/// if at cap. The in-memory insert is immediate (so lookups are
/// correct at once); the DISK save is debounced (see save_debounced)
/// — a multi-chunk page summary fires up to 7 stores in a burst, and
/// re-serializing the whole cache file 7 times in a row was wasteful
/// I/O exactly during the latency-sensitive summarize. Drop calls
/// flush() to guarantee the last burst lands on disk.
pub fn store(prompt: &str, response: &str) {
    if response.is_empty() { return; }
    // Don't cache error responses — caller might want to retry on
    // a later call when ollama is back up. The caller's responsibility
    // to not call store() on errors, but we double-check for the
    // [LLM error: ...] format that ollama_query_* uses internally.
    if response.starts_with("[LLM error:") { return; }
    let key = fnv1a(prompt);
    let mut g = match cache().lock() { Ok(g) => g, Err(_) => return };
    if g.entries.len() >= MAX_ENTRIES && !g.entries.contains_key(&key) {
        // Evict oldest by last_used.
        let evict_key: Option<u64> = g.entries.iter()
            .min_by_key(|(_, e)| e.last_used)
            .map(|(k, _)| *k);
        if let Some(k) = evict_key {
            g.entries.remove(&k);
        }
    }
    g.entries.insert(key, Entry {
        response: response.to_string(),
        last_used: now_secs(),
    });
    drop(g);
    save_debounced();
}

/// v0.1.443: debounced disk save. Writes at most once per
/// SAVE_DEBOUNCE_SECS; bursts of stores (e.g. a multi-chunk page
/// summary) coalesce into a single write. Marks the cache dirty
/// so a later flush() (or the next debounce window) persists. The
/// in-memory state is always current regardless.
const SAVE_DEBOUNCE_SECS: u64 = 5;

fn last_save_cell() -> &'static Mutex<(u64, bool)> {
    // (last_save_epoch, dirty). Initialized to (0, false): the
    // first store sees now - 0 >= DEBOUNCE (any real epoch is far
    // > 5), so the FIRST save is immediate — only subsequent
    // bursts within a 5s window coalesce. This preserves the old
    // "first write lands promptly" behavior while still collapsing
    // the 7-store summary burst.
    static LS: OnceLock<Mutex<(u64, bool)>> = OnceLock::new();
    LS.get_or_init(|| Mutex::new((0, false)))
}

/// Pure debounce decision: given the current time, the last-save
/// time, and the window, should we write now? Extracted so it's
/// unit-testable without touching disk. (now - last >= window)
fn should_save_now(now: u64, last: u64, window: u64) -> bool {
    now.saturating_sub(last) >= window
}

fn save_debounced() {
    let now = now_secs();
    let do_save = {
        match last_save_cell().lock() {
            Ok(mut g) => {
                g.1 = true; // dirty
                if should_save_now(now, g.0, SAVE_DEBOUNCE_SECS) {
                    g.0 = now;
                    g.1 = false; // about to write
                    true
                } else {
                    false
                }
            }
            Err(_) => true, // lock poisoned: just save
        }
    };
    if do_save {
        save_to_disk();
    }
}

/// Force any pending (dirty) state to disk now. Call on shutdown
/// (Backend::drop) so a debounced burst isn't lost. Cheap no-op if
/// nothing is dirty.
pub fn flush() {
    let dirty = last_save_cell().lock().map(|mut g| {
        let d = g.1;
        if d {
            g.0 = now_secs();
            g.1 = false;
        }
        d
    }).unwrap_or(true);
    if dirty {
        save_to_disk();
    }
}

pub fn len() -> usize {
    cache().lock().map(|g| g.entries.len()).unwrap_or(0)
}

fn db_path() -> Option<PathBuf> {
    let home = std::env::var("HOME").ok()?;
    Some(PathBuf::from(home).join(".config/web2qml/llm_cache.bin"))
}

/// Disk format:
///   [4 bytes magic = "LLMC"]
///   [u16 version = 1]
///   [u32 entry count]
///   for each entry:
///     [u64 key]
///     [u32 response_len][response_bytes]
///     [u64 last_used]
pub fn save_to_disk() {
    let path = match db_path() { Some(p) => p, None => return };
    let _ = std::fs::create_dir_all(path.parent().unwrap());
    let g = match cache().lock() { Ok(g) => g, Err(_) => return };

    let mut out: Vec<u8> = Vec::with_capacity(g.entries.len() * 256);
    out.extend_from_slice(FILE_MAGIC);
    out.extend_from_slice(&FILE_VERSION.to_le_bytes());
    out.extend_from_slice(&(g.entries.len() as u32).to_le_bytes());
    for (key, e) in &g.entries {
        out.extend_from_slice(&key.to_le_bytes());
        let rb = e.response.as_bytes();
        if rb.len() > u32::MAX as usize { continue; }
        out.extend_from_slice(&(rb.len() as u32).to_le_bytes());
        out.extend_from_slice(rb);
        out.extend_from_slice(&e.last_used.to_le_bytes());
    }
    drop(g);

    let tmp = path.with_extension("bin.tmp");
    if let Err(e) = std::fs::write(&tmp, &out) {
        crate::clog!("llm-cache", "save failed: {}", e);
        return;
    }
    let _ = std::fs::rename(tmp, path);
}

pub fn load_from_disk() {
    let path = match db_path() { Some(p) => p, None => return };
    let bytes = match std::fs::read(&path) { Ok(b) => b, Err(_) => return };
    if bytes.len() < 10 { return; }
    if &bytes[0..4] != FILE_MAGIC {
        crate::clog!("llm-cache", "file has wrong magic, skipping load");
        return;
    }
    let _version = u16::from_le_bytes([bytes[4], bytes[5]]);
    let n = u32::from_le_bytes([bytes[6], bytes[7], bytes[8], bytes[9]]) as usize;
    let mut pos = 10usize;
    let mut entries: HashMap<u64, Entry> = HashMap::with_capacity(n);
    for _ in 0..n {
        if pos + 8 > bytes.len() { break; }
        let key = u64::from_le_bytes([
            bytes[pos], bytes[pos+1], bytes[pos+2], bytes[pos+3],
            bytes[pos+4], bytes[pos+5], bytes[pos+6], bytes[pos+7],
        ]);
        pos += 8;
        if pos + 4 > bytes.len() { break; }
        let rl = u32::from_le_bytes([bytes[pos], bytes[pos+1], bytes[pos+2], bytes[pos+3]]) as usize;
        pos += 4;
        if pos + rl > bytes.len() { break; }
        let response = match String::from_utf8(bytes[pos..pos+rl].to_vec()) {
            Ok(s) => s, Err(_) => break,
        };
        pos += rl;
        if pos + 8 > bytes.len() { break; }
        let last_used = u64::from_le_bytes([
            bytes[pos], bytes[pos+1], bytes[pos+2], bytes[pos+3],
            bytes[pos+4], bytes[pos+5], bytes[pos+6], bytes[pos+7],
        ]);
        pos += 8;
        entries.insert(key, Entry { response, last_used });
    }
    if let Ok(mut g) = cache().lock() {
        g.entries = entries;
        eprintln!("LlmCache: loaded {} entries from {:?}", g.entries.len(), path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debounce_first_save_immediate() {
        // v0.1.443: with a realistic epoch and last=0, the first
        // store always clears the window (epoch >> 5s).
        let base = 1_748_000_000u64;
        assert!(should_save_now(base, 0, SAVE_DEBOUNCE_SECS),
            "first store (last=0) saves immediately at a real epoch");
    }

    #[test]
    fn debounce_coalesces_burst() {
        let base = 1_748_000_000u64;
        // Within the window → no save.
        assert!(!should_save_now(base + 1, base, SAVE_DEBOUNCE_SECS));
        assert!(!should_save_now(base + 4, base, SAVE_DEBOUNCE_SECS));
        // At/after the window → save.
        assert!(should_save_now(base + 5, base, SAVE_DEBOUNCE_SECS));
        assert!(should_save_now(base + 6, base, SAVE_DEBOUNCE_SECS));
    }

    #[test]
    fn debounce_saturating_no_underflow() {
        // now < last (clock skew) must not underflow-panic; treated
        // as "not yet" (0 >= window is false for window > 0).
        assert!(!should_save_now(10, 100, SAVE_DEBOUNCE_SECS));
    }
}
