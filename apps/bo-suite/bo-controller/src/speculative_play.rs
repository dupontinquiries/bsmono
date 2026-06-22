// src/speculative_play.rs
#![allow(dead_code)]
//
// Speculative loadUri-with-pause: the strongest form of predictive
// pre-staging.
//
// Compared to the shadow stream (which plays the track at vol=0 and
// promotes via a volume restore), speculative-pause loads the URI
// into the device's audio pipeline but ASKS THE DEVICE TO START PAUSED.
// When the user actually taps play, we send a single Transport=play
// command — no volume manipulation, no audible click, no risk of the
// shadow's silent stream leaking even a millisecond of audio.
//
// Trade-offs:
//   * Requires firmware that honors a `pause` flag on loadUri.
//     Modern Mozart firmware does; older B&O network products don't.
//   * No fallback within this module — the worker tries it, and if
//     the device rejects it (4xx response), we surface the failure
//     and the existing shadow_stream module remains the fallback.
//
// This module owns the STATE: which track is currently pre-loaded on
// the device, when it was loaded (for staleness check), what the
// device URL was (for cross-device invalidation). The actual network
// POST happens in the worker thread; the state here gates whether a
// new speculation is even worth issuing.

use std::collections::HashMap;
use std::time::{Duration, Instant};

/// How long a pre-load is considered "fresh". After this, we'd rather
/// re-issue than rely on stale state — devices can purge their loaded
/// URI silently (memory pressure, source switch, etc.).
pub const FRESHNESS_WINDOW: Duration = Duration::from_secs(120);

/// Minimum interval between speculations targeting the same device.
/// Prevents thrashing when the anticipator's top prediction oscillates.
pub const PER_DEVICE_COOLDOWN: Duration = Duration::from_secs(8);

/// Tracks speculative pre-loads per device URL. One device can have
/// at most one track speculatively loaded at a time (the device's own
/// playback pipeline can hold one queued URI).
#[derive(Debug, Default)]
pub struct SpeculativeStager {
    per_device: HashMap<String, DeviceState>,
}

#[derive(Debug, Clone)]
struct DeviceState {
    /// Catalog track ID currently loaded on this device.
    loaded_track_id: Option<i64>,
    /// When the pre-load was issued.
    loaded_at: Instant,
    /// Last speculation timestamp regardless of success. Used for
    /// cooldown so we don't hammer a device that keeps rejecting.
    last_attempt_at: Option<Instant>,
}

#[derive(Debug, PartialEq)]
pub enum SpeculateDecision {
    /// OK to issue the speculative load now. Pass the returned token
    /// to the worker; on success the caller updates this state via
    /// `mark_loaded`.
    Proceed,
    /// This track is already loaded on this device. The "promotion"
    /// path (sending Transport=play) is all that's needed.
    AlreadyLoaded { loaded_at: Instant },
    /// Too soon since the last attempt to this device.
    Cooldown { last_attempt_at: Instant },
    /// Same device has a DIFFERENT track loaded. We can replace it,
    /// but the caller may want to be conservative and skip — e.g.
    /// if the existing pre-load was very recent.
    Mismatch { current_track_id: i64, loaded_at: Instant },
}

#[derive(Debug, PartialEq)]
pub enum PromoteResult {
    /// The device has this track pre-loaded and fresh. Send a single
    /// Transport=play to start it; no fetch required.
    Hit,
    /// The device has a different track loaded. The caller must do a
    /// full play_track flow.
    Miss,
    /// Pre-load is too old; better to play afresh than gamble on the
    /// device still having it ready.
    Stale,
}

impl SpeculativeStager {
    pub fn new() -> Self { Self::default() }

    /// Should we issue a speculative load for `track_id` on `device_url`?
    pub fn check(&self, device_url: &str, track_id: i64, now: Instant) -> SpeculateDecision {
        match self.per_device.get(device_url) {
            None => SpeculateDecision::Proceed,
            Some(state) => {
                // Cooldown gate first — even if we'd want a new load,
                // we honor the cooldown window.
                if let Some(last) = state.last_attempt_at {
                    if now.duration_since(last) < PER_DEVICE_COOLDOWN {
                        return SpeculateDecision::Cooldown { last_attempt_at: last };
                    }
                }
                match state.loaded_track_id {
                    Some(t) if t == track_id => {
                        if now.duration_since(state.loaded_at) < FRESHNESS_WINDOW {
                            SpeculateDecision::AlreadyLoaded { loaded_at: state.loaded_at }
                        } else {
                            SpeculateDecision::Proceed
                        }
                    }
                    Some(t) => SpeculateDecision::Mismatch {
                        current_track_id: t,
                        loaded_at: state.loaded_at,
                    },
                    None => SpeculateDecision::Proceed,
                }
            }
        }
    }

    /// Record that a speculation was just issued (whatever the outcome).
    /// Establishes the per-device cooldown.
    pub fn note_attempt(&mut self, device_url: &str, now: Instant) {
        let entry = self.per_device.entry(device_url.to_string())
            .or_insert(DeviceState {
                loaded_track_id: None,
                loaded_at: now,
                last_attempt_at: None,
            });
        entry.last_attempt_at = Some(now);
    }

    /// Record that the device confirmed a successful pre-load.
    pub fn mark_loaded(&mut self, device_url: &str, track_id: i64, now: Instant) {
        self.per_device.insert(device_url.to_string(), DeviceState {
            loaded_track_id: Some(track_id),
            loaded_at: now,
            last_attempt_at: Some(now),
        });
    }

    /// Check whether a user's play tap on `track_id` can be promoted
    /// via the speculative path (no fetch, just unpause).
    pub fn try_promote(&self, device_url: &str, track_id: i64, now: Instant) -> PromoteResult {
        match self.per_device.get(device_url) {
            Some(state) => match state.loaded_track_id {
                Some(t) if t == track_id => {
                    if now.duration_since(state.loaded_at) < FRESHNESS_WINDOW {
                        PromoteResult::Hit
                    } else {
                        PromoteResult::Stale
                    }
                }
                _ => PromoteResult::Miss,
            }
            None => PromoteResult::Miss,
        }
    }

    /// Invalidate state for a device — call on device switch or when
    /// we know the load has been displaced (e.g. user played
    /// something else through another path).
    pub fn invalidate(&mut self, device_url: &str) {
        self.per_device.remove(device_url);
    }

    /// Invalidate ALL state — useful when the active device list
    /// changes substantially or on shutdown.
    pub fn clear(&mut self) {
        self.per_device.clear();
    }

    /// Diagnostic: how many devices have speculative state?
    pub fn tracked_devices(&self) -> usize { self.per_device.len() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fresh_stager_says_proceed() {
        let s = SpeculativeStager::new();
        assert_eq!(s.check("http://x", 42, Instant::now()), SpeculateDecision::Proceed);
    }

    #[test]
    fn after_load_same_track_is_already_loaded() {
        let mut s = SpeculativeStager::new();
        let t = Instant::now();
        s.note_attempt("http://x", t);
        s.mark_loaded("http://x", 42, t);
        let later = t + Duration::from_secs(30);
        match s.check("http://x", 42, later) {
            SpeculateDecision::AlreadyLoaded { .. } => {},
            other => panic!("expected AlreadyLoaded, got {:?}", other),
        }
    }

    #[test]
    fn after_load_different_track_is_mismatch() {
        let mut s = SpeculativeStager::new();
        let t = Instant::now();
        s.note_attempt("http://x", t);
        s.mark_loaded("http://x", 42, t);
        // Cooldown will trip first since we just attempted.
        // Past cooldown, different track is Mismatch.
        let later = t + PER_DEVICE_COOLDOWN + Duration::from_secs(1);
        match s.check("http://x", 99, later) {
            SpeculateDecision::Mismatch { current_track_id: 42, .. } => {},
            other => panic!("expected Mismatch(42, ..), got {:?}", other),
        }
    }

    #[test]
    fn cooldown_gates_immediate_resend() {
        let mut s = SpeculativeStager::new();
        let t = Instant::now();
        s.note_attempt("http://x", t);
        // Any check within cooldown returns Cooldown — even for "Proceed" cases.
        let soon = t + Duration::from_secs(1);
        match s.check("http://x", 42, soon) {
            SpeculateDecision::Cooldown { .. } => {},
            other => panic!("expected Cooldown, got {:?}", other),
        }
    }

    #[test]
    fn stale_load_becomes_proceed_again() {
        let mut s = SpeculativeStager::new();
        let t = Instant::now();
        s.note_attempt("http://x", t);
        s.mark_loaded("http://x", 42, t);
        let stale = t + FRESHNESS_WINDOW + Duration::from_secs(1);
        // After freshness window expires, even same track says Proceed.
        assert_eq!(s.check("http://x", 42, stale), SpeculateDecision::Proceed);
    }

    #[test]
    fn try_promote_hit() {
        let mut s = SpeculativeStager::new();
        let t = Instant::now();
        s.mark_loaded("http://x", 42, t);
        let later = t + Duration::from_secs(30);
        assert_eq!(s.try_promote("http://x", 42, later), PromoteResult::Hit);
    }

    #[test]
    fn try_promote_miss_on_different_track() {
        let mut s = SpeculativeStager::new();
        let t = Instant::now();
        s.mark_loaded("http://x", 42, t);
        assert_eq!(s.try_promote("http://x", 99, t), PromoteResult::Miss);
    }

    #[test]
    fn try_promote_stale_after_freshness() {
        let mut s = SpeculativeStager::new();
        let t = Instant::now();
        s.mark_loaded("http://x", 42, t);
        let stale = t + FRESHNESS_WINDOW + Duration::from_secs(1);
        assert_eq!(s.try_promote("http://x", 42, stale), PromoteResult::Stale);
    }

    #[test]
    fn invalidate_clears_one_device() {
        let mut s = SpeculativeStager::new();
        let t = Instant::now();
        s.mark_loaded("http://x", 42, t);
        s.mark_loaded("http://y", 99, t);
        s.invalidate("http://x");
        assert_eq!(s.try_promote("http://x", 42, t), PromoteResult::Miss);
        assert_eq!(s.try_promote("http://y", 99, t), PromoteResult::Hit);
    }

    #[test]
    fn clear_drops_all() {
        let mut s = SpeculativeStager::new();
        let t = Instant::now();
        s.mark_loaded("http://x", 42, t);
        s.mark_loaded("http://y", 99, t);
        assert_eq!(s.tracked_devices(), 2);
        s.clear();
        assert_eq!(s.tracked_devices(), 0);
    }

    #[test]
    fn per_device_state_independent() {
        let mut s = SpeculativeStager::new();
        let t = Instant::now();
        s.mark_loaded("http://x", 42, t);
        // Different device → no cooldown, no load info → Proceed.
        assert_eq!(s.check("http://y", 99, t), SpeculateDecision::Proceed);
    }
}
