// src/service/inbox.rs
//
// Watch the XDG inbox directory for new pin files.
//
//   $XDG_DATA_HOME/ambient-learner/inbox/pins/<id>.json
//   (default: ~/.local/share/ambient-learner/inbox/pins/<id>.json)
//
// Each file is parsed as a `MapPin`. Successful parses are upserted
// into the `PinStore`; failures are logged and the file is left
// alone so the publisher can fix and retry.
//
// Why a file-based inbox at all when we already have D-Bus?
//   * Many submitters are shell scripts, weather-data importers,
//     `cron` jobs, photo-organizer post-hooks. Asking those to link
//     a D-Bus library is overkill.
//   * Files survive a restart of *us*. If a publisher writes a pin
//     while we're not running, we'll pick it up next startup.
//   * Files give a publisher a clear audit trail and the option to
//     remove a pin by deleting the file.

#![cfg(feature = "service")]

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use notify::{event::EventKind, Event, RecommendedWatcher, RecursiveMode, Watcher};

use super::map_pin::MapPin;
use super::pin_store::PinStore;

/// Compute the canonical inbox path:
///   $XDG_DATA_HOME/ambient-learner/inbox/pins
/// Falls back to ~/.local/share/... when XDG_DATA_HOME isn't set.
pub fn default_inbox_dir() -> PathBuf {
    let base = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .filter(|p| !p.as_os_str().is_empty())
        .or_else(|| {
            std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share"))
        })
        .unwrap_or_else(|| PathBuf::from("/tmp"));
    base.join("ambient-learner").join("inbox").join("pins")
}

/// Make sure the inbox directory exists. Returns the path it created
/// or confirmed. Errors propagate up; callers should warn but continue.
pub fn ensure_inbox_dir(path: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(path)
}

/// Spawn the inbox watcher on a background thread.
///
/// The thread:
///   1. Imports every existing `*.json` in the inbox at startup.
///   2. Subscribes to inotify events for the directory and re-imports
///      any file that's created or modified.
///   3. Removes pins from the store when their file is deleted.
///
/// The handle is kept alive by the returned `WatcherHandle`. Drop it
/// to stop watching. (We don't expose a public stop method because
/// the watcher is process-lifetime in normal use.)
pub fn start(path: PathBuf, store: PinStore) -> std::io::Result<WatcherHandle> {
    // 1. First pass: import everything that's already there.
    if let Ok(entries) = std::fs::read_dir(&path) {
        for entry in entries.flatten() {
            let p = entry.path();
            if is_pin_file(&p) {
                import_file(&p, &store);
            }
        }
    }

    // 2. Now subscribe to changes.
    let store_for_events = store.clone();
    let path_for_events = path.clone();
    let mut watcher: RecommendedWatcher = notify::recommended_watcher(move |res| {
        match res {
            Ok(event) => handle_event(event, &path_for_events, &store_for_events),
            Err(e) => log::warn!("inbox watcher error: {e}"),
        }
    })
    .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, format!("notify: {e}")))?;

    watcher
        .watch(&path, RecursiveMode::NonRecursive)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, format!("notify watch: {e}")))?;

    log::info!("📂 inbox watcher live at {}", path.display());
    Ok(WatcherHandle {
        _watcher: Arc::new(watcher),
        path,
    })
}

/// Convenience: ensure the inbox dir exists, then start watching it.
pub fn ensure_and_start(store: PinStore) -> std::io::Result<WatcherHandle> {
    let dir = default_inbox_dir();
    ensure_inbox_dir(&dir)?;
    start(dir, store)
}

/// Returned by `start()`; dropping it stops the watcher.
pub struct WatcherHandle {
    _watcher: Arc<RecommendedWatcher>,
    pub path: PathBuf,
}

fn handle_event(event: Event, inbox_dir: &Path, store: &PinStore) {
    match event.kind {
        EventKind::Create(_) | EventKind::Modify(_) => {
            for p in event.paths {
                if is_pin_file(&p) && p.starts_with(inbox_dir) {
                    // notify can fire CREATE before the file is fully
                    // written; give the publisher a moment to flush.
                    std::thread::sleep(Duration::from_millis(50));
                    import_file(&p, store);
                }
            }
        }
        EventKind::Remove(_) => {
            for p in event.paths {
                if let Some(id) = pin_id_from_filename(&p) {
                    if store.remove(&id).is_some() {
                        log::info!("📂 inbox: removed pin {id}");
                    }
                }
            }
        }
        _ => {}
    }
}

fn is_pin_file(path: &Path) -> bool {
    path.extension().and_then(|s| s.to_str()) == Some("json")
}

fn pin_id_from_filename(path: &Path) -> Option<String> {
    path.file_stem().and_then(|s| s.to_str()).map(|s| s.to_owned())
}

/// Read a single JSON file and submit it as a pin. Logs and returns
/// silently on any error; the publisher will see the file untouched
/// in the inbox and can adjust.
fn import_file(path: &Path, store: &PinStore) {
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) => {
            log::warn!("inbox: cannot read {}: {e}", path.display());
            return;
        }
    };

    let mut pin: MapPin = match serde_json::from_slice(&bytes) {
        Ok(p) => p,
        Err(e) => {
            log::warn!("inbox: cannot parse {}: {e}", path.display());
            return;
        }
    };

    // If the pin's id field is empty, fall back to the filename — that
    // way `cp foo.json inbox/` works without the publisher having to
    // duplicate the id inside the file.
    if pin.id.is_empty() {
        if let Some(stem) = pin_id_from_filename(path) {
            pin.id = stem;
        }
    }

    // Tag the pin with its source so the audit log is meaningful.
    if pin.source.is_none() {
        pin.source = Some(format!("inbox:{}", path.display()));
    }

    let pin = match pin.validated() {
        Ok(p) => p,
        Err(e) => {
            log::warn!("inbox: rejecting {}: {e}", path.display());
            return;
        }
    };

    store.upsert(pin);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use tempfile::tempdir;

    fn pin_json(id: &str, lat: f64, lon: f64) -> String {
        serde_json::json!({
            "id": id, "kind": "photo",
            "latitude": lat, "longitude": lon
        }).to_string()
    }

    #[test]
    fn importing_an_existing_file() {
        let dir = tempdir().unwrap();
        let f = dir.path().join("hello.json");
        std::fs::write(&f, pin_json("hello", 1.0, 2.0)).unwrap();

        let store = PinStore::new();
        import_file(&f, &store);

        assert_eq!(store.len(), 1);
        let p = &store.list()[0];
        assert_eq!(p.id, "hello");
        assert_eq!(p.latitude, 1.0);
    }

    #[test]
    fn id_fallback_to_filename() {
        let dir = tempdir().unwrap();
        let f = dir.path().join("auto-id.json");
        // No id field at all
        std::fs::write(&f, r#"{"kind":"photo","latitude":1.0,"longitude":2.0}"#).unwrap();

        let store = PinStore::new();
        import_file(&f, &store);
        assert_eq!(store.list()[0].id, "auto-id");
    }

    #[test]
    fn malformed_json_logged_not_panicked() {
        let dir = tempdir().unwrap();
        let f = dir.path().join("bad.json");
        std::fs::write(&f, "this is not json").unwrap();

        let store = PinStore::new();
        import_file(&f, &store);
        assert!(store.is_empty());
    }

    #[test]
    fn rejects_invalid_pin() {
        let dir = tempdir().unwrap();
        let f = dir.path().join("bad-coords.json");
        // latitude out of range
        std::fs::write(&f, r#"{"id":"x","kind":"photo","latitude":99.9,"longitude":0.0}"#).unwrap();

        let store = PinStore::new();
        import_file(&f, &store);
        assert!(store.is_empty());
    }
}
