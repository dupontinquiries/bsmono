// src/service/pin_store.rs
//
// In-memory store of all the pins external apps have published. Every
// path that mutates this store goes through a single `Mutex<HashMap>`
// so we don't have to think about cross-thread visibility.
//
// Why not push pins straight into `WorldState`? Two reasons:
//   * Pins are part of the *public API*, not internal state. Keeping
//     them in their own module preserves the option of versioning
//     them independently.
//   * The store is owned by the service feature, and that feature
//     shouldn't widen the SystemState struct on builds that aren't
//     using the service.
//
// The store also runs a background expiry sweep every 60 seconds so
// that pins with `expires_at` set don't linger forever.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use parking_lot::Mutex;

use super::map_pin::MapPin;

/// Notification fired when the set of live pins changes. The store
/// itself is intentionally unaware of consumers; whoever runs the
/// service binds an `on_change` callback that emits the D-Bus signal,
/// updates QML, etc.
pub type ChangeNotifier = Arc<dyn Fn() + Send + Sync>;

#[derive(Clone, Default)]
pub struct PinStore {
    inner: Arc<Mutex<Inner>>,
}

#[derive(Default)]
struct Inner {
    pins: HashMap<String, MapPin>,
    on_change: Option<ChangeNotifier>,
}

impl PinStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register the change notifier. Replacing it removes the old one.
    pub fn on_change<F>(&self, callback: F)
    where
        F: Fn() + Send + Sync + 'static,
    {
        self.inner.lock().on_change = Some(Arc::new(callback));
    }

    /// Insert or replace a pin. Returns the previous pin (if any) and
    /// whether the store actually changed (a no-op replace by the same
    /// pin doesn't fire the change notifier).
    pub fn upsert(&self, pin: MapPin) -> UpsertResult {
        let mut g = self.inner.lock();
        let previous = g.pins.get(&pin.id).cloned();
        let unchanged = previous
            .as_ref()
            .map(|p| pins_equivalent(p, &pin))
            .unwrap_or(false);
        g.pins.insert(pin.id.clone(), pin);
        let cb = g.on_change.clone();
        drop(g);

        if !unchanged {
            if let Some(cb) = cb {
                cb();
            }
        }

        UpsertResult { previous, was_new: !unchanged }
    }

    /// Remove a pin by id. Returns the removed pin (if any).
    pub fn remove(&self, id: &str) -> Option<MapPin> {
        let mut g = self.inner.lock();
        let removed = g.pins.remove(id);
        let cb = g.on_change.clone();
        drop(g);

        if removed.is_some() {
            if let Some(cb) = cb {
                cb();
            }
        }
        removed
    }

    /// Snapshot every currently-live pin.
    pub fn list(&self) -> Vec<MapPin> {
        self.inner.lock().pins.values().cloned().collect()
    }

    pub fn len(&self) -> usize {
        self.inner.lock().pins.len()
    }

    pub fn is_empty(&self) -> bool {
        self.inner.lock().pins.is_empty()
    }

    /// Drop pins whose `expires_at` is in the past. Returns the number
    /// of pins removed. Cheap when nothing expires.
    pub fn sweep_expired(&self) -> usize {
        let now = unix_now();
        let mut g = self.inner.lock();
        let before = g.pins.len();
        g.pins.retain(|_, pin| match pin.expires_at {
            Some(t) => t > now,
            None    => true,
        });
        let removed = before - g.pins.len();
        let cb = g.on_change.clone();
        drop(g);

        if removed > 0 {
            if let Some(cb) = cb {
                cb();
            }
        }
        removed
    }

    /// Spawn a background thread that calls `sweep_expired()` every
    /// `interval`. Returns immediately. The thread runs forever; we
    /// don't bother with a shutdown signal because this is a process-
    /// lifetime worker.
    pub fn start_expiry_sweeper(&self, interval: Duration) {
        let store = self.clone();
        std::thread::Builder::new()
            .name("ambient-pin-sweeper".into())
            .spawn(move || {
                // First sweep happens after one interval, not at startup,
                // so the inbox importer has a chance to populate first.
                loop {
                    std::thread::sleep(interval);
                    let _ = store.sweep_expired();
                }
            })
            .ok();
    }
}

#[derive(Debug)]
pub struct UpsertResult {
    /// The previous version of the pin, or None if it didn't exist.
    pub previous: Option<MapPin>,
    /// True if the pin actually changed (so consumers should re-render).
    pub was_new: bool,
}

fn pins_equivalent(a: &MapPin, b: &MapPin) -> bool {
    // We deliberately ignore `submitted_at` when checking equivalence:
    // the same logical pin re-submitted shouldn't fire a "changed"
    // signal just because the timestamp ticked forward.
    a.id == b.id
        && a.kind == b.kind
        && a.latitude == b.latitude
        && a.longitude == b.longitude
        && a.title == b.title
        && a.description == b.description
        && a.uri == b.uri
        && a.payload == b.payload
        && a.expires_at == b.expires_at
}

fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Used internally by tests.
#[allow(dead_code)]
fn freshly_seeded_store() -> PinStore {
    let _ = Instant::now();
    PinStore::new()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    fn pin(id: &str, lat: f64, lon: f64) -> MapPin {
        MapPin {
            id: id.into(),
            kind: "photo".into(),
            latitude: lat,
            longitude: lon,
            title: None, description: None, uri: None,
            payload: HashMap::new(), source: None,
            submitted_at: 0, expires_at: None,
        }
    }

    #[test]
    fn upsert_and_list() {
        let s = PinStore::new();
        s.upsert(pin("a", 1.0, 2.0));
        s.upsert(pin("b", 3.0, 4.0));
        assert_eq!(s.len(), 2);
        let all = s.list();
        assert!(all.iter().any(|p| p.id == "a"));
        assert!(all.iter().any(|p| p.id == "b"));
    }

    #[test]
    fn upsert_replaces_same_id() {
        let s = PinStore::new();
        s.upsert(pin("a", 1.0, 2.0));
        let r = s.upsert(pin("a", 9.9, 9.9));
        assert!(r.previous.is_some());
        assert!(r.was_new);   // values changed, so it's effectively new
        assert_eq!(s.len(), 1);
    }

    #[test]
    fn equivalent_upsert_doesnt_fire_change() {
        let s = PinStore::new();
        let counter = Arc::new(AtomicU32::new(0));
        let c2 = counter.clone();
        s.on_change(move || { c2.fetch_add(1, Ordering::SeqCst); });

        // First insert fires.
        s.upsert(pin("a", 1.0, 2.0));
        assert_eq!(counter.load(Ordering::SeqCst), 1);

        // Re-inserting an equivalent pin doesn't.
        s.upsert(pin("a", 1.0, 2.0));
        assert_eq!(counter.load(Ordering::SeqCst), 1);

        // Modifying the location does.
        s.upsert(pin("a", 5.0, 6.0));
        assert_eq!(counter.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn sweep_drops_expired() {
        let s = PinStore::new();
        let mut expired = pin("e", 0.0, 0.0);
        expired.expires_at = Some(0); // 1970, definitely in the past
        s.upsert(expired);

        let mut alive = pin("a", 0.0, 0.0);
        alive.expires_at = Some(i64::MAX);
        s.upsert(alive);

        let removed = s.sweep_expired();
        assert_eq!(removed, 1);
        assert_eq!(s.len(), 1);
        assert_eq!(s.list()[0].id, "a");
    }

    #[test]
    fn remove_fires_change() {
        let s = PinStore::new();
        let counter = Arc::new(AtomicU32::new(0));
        let c2 = counter.clone();
        s.on_change(move || { c2.fetch_add(1, Ordering::SeqCst); });

        s.upsert(pin("a", 1.0, 2.0));
        let baseline = counter.load(Ordering::SeqCst);

        s.remove("a");
        assert_eq!(counter.load(Ordering::SeqCst), baseline + 1);
    }
}
