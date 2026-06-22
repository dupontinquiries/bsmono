// src/volume_safety_ack.rs
//
// Persistent "we've already safety-checked this device" registry.
//
// The startup volume-safety clamp exists so a first connection to a
// hot-volume device doesn't surprise the listener. It's an
// important safety net for a NEW device — say, the user just got a
// loaner Beosound A9 from a friend who likes it cranked. But it's
// actively wrong for devices the user has been listening to for
// weeks at a level they explicitly set. On every app startup the
// clamp would yank their volume back down to ~18% (≈80 dB on an
// unknown product) and the user has to manually nudge it back up.
//
// The fix: store an acknowledgement once the user has actively used
// a device above the safety ceiling. From then on, that device is
// trusted: subsequent startups don't clamp.
//
// We store as a JSON file under $XDG_DATA_HOME/bo-controller/. JSON
// not SQLite because (a) the data is tiny (one bool per device URL),
// (b) the controller's media worker owns the only SQLite handle and
// going through it would require a roundtrip on every volume change,
// and (c) the file is human-inspectable / -editable for an admin
// who wants to reset the ack.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;

fn ack_path() -> PathBuf {
    let base = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))
        .unwrap_or_else(|| PathBuf::from("."));
    let dir = base.join("bo-controller");
    let _ = std::fs::create_dir_all(&dir);
    dir.join("volume_safety_ack.json")
}

/// In-memory cache of the acked device URLs. Loaded lazily on first
/// access; mutations write through to disk. Mutex is fine — the
/// access pattern is "rare write on user action, occasional read on
/// startup", not a hot path.
struct AckCache {
    loaded: bool,
    acks: HashMap<String, bool>,
}

fn cache() -> &'static Mutex<AckCache> {
    static CACHE: std::sync::OnceLock<Mutex<AckCache>> = std::sync::OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(AckCache {
        loaded: false,
        acks: HashMap::new(),
    }))
}

fn load_into(cache: &mut AckCache) {
    if cache.loaded { return; }
    cache.loaded = true;
    let path = ack_path();
    let Ok(s) = std::fs::read_to_string(&path) else { return; };
    if let Ok(map) = serde_json::from_str::<HashMap<String, bool>>(&s) {
        cache.acks = map;
    }
}

fn save(cache: &AckCache) {
    let path = ack_path();
    if let Ok(s) = serde_json::to_string_pretty(&cache.acks) {
        let _ = std::fs::write(path, s);
    }
}

/// Has the user acknowledged this device's "loud" output before?
/// Returns false on read errors (safe default — clamp on startup).
pub fn is_acked(url: &str) -> bool {
    if url.is_empty() { return false; }
    let Ok(mut cache) = cache().lock() else { return false; };
    load_into(&mut cache);
    *cache.acks.get(url).unwrap_or(&false)
}

/// Mark this device as acked. Called when the user actively sets a
/// volume above the safety ceiling — that's their explicit
/// consent that this device is allowed to be loud.
pub fn ack(url: &str) {
    if url.is_empty() { return; }
    let Ok(mut cache) = cache().lock() else { return; };
    load_into(&mut cache);
    if cache.acks.get(url).copied() == Some(true) { return; }  // no-op if already acked
    cache.acks.insert(url.to_string(), true);
    save(&cache);
}

/// Clear the ack for a device (used by the UI's "reset safety" action
/// — currently dead code but the API matches what the eventual UI
/// will call).
#[allow(dead_code)]
pub fn unack(url: &str) {
    if url.is_empty() { return; }
    let Ok(mut cache) = cache().lock() else { return; };
    load_into(&mut cache);
    if cache.acks.remove(url).is_some() { save(&cache); }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Override the path discovery for tests so we don't trample the
    /// real user's ack file. Each test gets its own temp dir via
    /// XDG_DATA_HOME, and the static cache is reset between tests.
    fn with_isolated_state<F: FnOnce()>(label: &str, f: F) {
        let tmp = std::env::temp_dir().join(
            format!("bo-vol-ack-test-{}-{}", label,
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        std::fs::create_dir_all(&tmp).unwrap();
        let prev = std::env::var_os("XDG_DATA_HOME");
        // SAFETY: tests run single-threaded with --test-threads=1 by
        // default for the env-mutating tests, and the path discovery
        // is the only consumer of this env var here.
        unsafe { std::env::set_var("XDG_DATA_HOME", &tmp); }
        // Reset the global cache so the env change takes effect.
        if let Ok(mut c) = cache().lock() {
            c.loaded = false;
            c.acks.clear();
        }
        f();
        if let Some(p) = prev {
            unsafe { std::env::set_var("XDG_DATA_HOME", p); }
        } else {
            unsafe { std::env::remove_var("XDG_DATA_HOME"); }
        }
        // Reset cache again so subsequent tests start clean.
        if let Ok(mut c) = cache().lock() {
            c.loaded = false;
            c.acks.clear();
        }
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn empty_url_never_acks() {
        with_isolated_state("empty_url", || {
            assert!(!is_acked(""));
            ack("");
            assert!(!is_acked(""));
        });
    }

    #[test]
    fn ack_persists_in_memory_and_round_trips_to_disk() {
        with_isolated_state("persist", || {
            assert!(!is_acked("http://192.168.2.12"));
            ack("http://192.168.2.12");
            assert!(is_acked("http://192.168.2.12"));
            // Force a reload by clearing the cache; should re-load
            // the value from disk.
            if let Ok(mut c) = cache().lock() { c.loaded = false; c.acks.clear(); }
            assert!(is_acked("http://192.168.2.12"));
        });
    }

    #[test]
    fn unack_clears_the_state() {
        with_isolated_state("unack", || {
            ack("http://x");
            assert!(is_acked("http://x"));
            unack("http://x");
            assert!(!is_acked("http://x"));
        });
    }

    #[test]
    fn multiple_devices_tracked_independently() {
        with_isolated_state("multi", || {
            ack("http://a");
            assert!(is_acked("http://a"));
            assert!(!is_acked("http://b"));
            ack("http://b");
            assert!(is_acked("http://b"));
            unack("http://a");
            assert!(!is_acked("http://a"));
            assert!(is_acked("http://b"));
        });
    }
}
