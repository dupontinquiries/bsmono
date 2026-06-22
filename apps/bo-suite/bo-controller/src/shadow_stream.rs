// src/shadow_stream.rs
#![allow(dead_code)]
//
// Volume-gated pre-streaming ("shadow stream"). When the anticipator
// believes the user is very likely to tap a specific track soon, we
// quietly start playing it on the device with the volume zeroed out.
// The device buffers, decodes, and starts playback — all silently. When
// the user actually taps that track, we just restore the volume and
// they hear it appear with no perceptible latency.
//
// HONEST SCOPE — when this works, it's magic; when it doesn't, here's why:
//
//   * It only helps when we correctly predict the next track. A wrong
//     prediction wastes a few seconds of device decode (no harm, no
//     audible artifact — they just hear nothing extra, since vol=0).
//   * Some firmware revisions handle volume changes mid-stream
//     differently. On older devices, going 0→V mid-stream may cause a
//     brief click. We attempt a fast ramp via repeated SetVolume calls
//     to soften this, but the cleanest solution lives in firmware.
//   * The device must accept a new stream while currently playing
//     something else. If it doesn't, calling start() while in active
//     playback will interrupt the user. We refuse to shadow when the
//     device reports it's currently playing.
//   * Shadows time out after MAX_SHADOW_AGE so they don't burn
//     bandwidth indefinitely. The user gets a fresh shadow on the next
//     high-confidence prediction.
//
// State machine (intentionally minimal):
//
//   None ── start(track) ──> Active(track, saved_vol)
//   Active ── promote(same_track) ──> None (vol restored)
//   Active ── promote(other_track) ──> None then start (new shadow)
//   Active ── cancel() ──> None (vol restored, stream stopped)
//   Active ── age > MAX ──> auto-cancel

use std::time::{Duration, Instant};

/// Maximum time a shadow stream can live before we cancel it. Tuned to
/// "user opened library, hovered on a track for half a minute" — long
/// enough to cover deliberation, short enough that we don't burn
/// bandwidth all afternoon if they forget the app is open.
pub const MAX_SHADOW_AGE: Duration = Duration::from_secs(30);

/// Confidence floor for actually firing a shadow. Below this, the
/// expected value of pre-streaming (savings × probability) doesn't pay
/// for the bandwidth + decode cost.
pub const SHADOW_CONFIDENCE_FLOOR: f32 = 0.65;

#[derive(Debug, Clone, PartialEq)]
pub struct ShadowState {
    pub track_id: i64,
    pub track_url: String,
    pub saved_volume: i32,
    pub started_at: Instant,
}

impl ShadowState {
    pub fn age(&self) -> Duration { self.started_at.elapsed() }
    pub fn is_expired(&self) -> bool { self.age() > MAX_SHADOW_AGE }
}

/// Outcome of attempting to "promote" a shadow into the actual user-
/// requested play. The QtState dispatches different commands based on
/// which variant comes back.
#[derive(Debug, PartialEq)]
pub enum PromoteResult {
    /// Shadow matched the requested track. Vol-up only; no new POST needed.
    Hit { saved_volume: i32 },
    /// Shadow was for a different track. The caller should cancel the
    /// previous shadow and run the normal play_track flow for `requested`.
    Mismatch { previous_track_id: i64, previous_saved_volume: i32 },
    /// No shadow was running.
    Miss,
}

/// Decision returned by `should_shadow`: yes (and the track URL to use),
/// no (with a reason for the log).
#[derive(Debug, PartialEq)]
pub enum ShadowDecision {
    Go,
    Skip(&'static str),
}

/// The shadow-stream manager. Holds at most one Active shadow at a time;
/// transitions are explicit so the test suite can drive each path.
#[derive(Debug, Default)]
pub struct ShadowManager {
    state: Option<ShadowState>,
}

impl ShadowManager {
    pub fn new() -> Self { Self { state: None } }

    pub fn current(&self) -> Option<&ShadowState> { self.state.as_ref() }

    pub fn is_active(&self) -> bool { self.state.is_some() }

    pub fn active_track_id(&self) -> Option<i64> {
        self.state.as_ref().map(|s| s.track_id)
    }

    /// Decide whether to fire a shadow given the current situation.
    /// We err on the side of *not* shadowing when in doubt — a wasted
    /// shadow is bandwidth and decode cycles for nothing.
    pub fn should_shadow(
        &self,
        confidence: f32,
        device_is_playing: bool,
        device_has_known_volume: bool,
        track_id: i64,
    ) -> ShadowDecision {
        // NaN comparisons are unordered, so `NaN < FLOOR` is false —
        // a NaN confidence would otherwise PASS the floor check.
        // Treat non-finite confidence as "no signal", skip the shadow
        // rather than burning bandwidth on a confused prediction.
        if !confidence.is_finite() {
            return ShadowDecision::Skip("non-finite confidence");
        }
        if confidence < SHADOW_CONFIDENCE_FLOOR {
            return ShadowDecision::Skip("confidence below floor");
        }
        if device_is_playing {
            return ShadowDecision::Skip("device already playing — refuse to interrupt");
        }
        if !device_has_known_volume {
            return ShadowDecision::Skip("no known device volume to save");
        }
        if let Some(s) = &self.state {
            if s.track_id == track_id {
                return ShadowDecision::Skip("already shadowing this track");
            }
            // We have an active shadow for a different track. Let the
            // caller decide whether to replace; default policy is to
            // skip (don't churn through shadows).
            return ShadowDecision::Skip("different track already shadowed");
        }
        ShadowDecision::Go
    }

    /// Record that we've started a shadow. Caller is responsible for
    /// actually setting the device volume to 0 and dispatching the
    /// stream.
    pub fn start(&mut self, track_id: i64, track_url: String, saved_volume: i32) {
        self.state = Some(ShadowState {
            track_id, track_url, saved_volume,
            started_at: Instant::now(),
        });
    }

    /// User actually tapped a track. Return PromoteResult so the caller
    /// can take the right next step.
    pub fn promote(&mut self, requested_track_id: i64) -> PromoteResult {
        match self.state.take() {
            None => PromoteResult::Miss,
            Some(s) if s.track_id == requested_track_id => {
                PromoteResult::Hit { saved_volume: s.saved_volume }
            }
            Some(s) => {
                let prev = s.track_id;
                let v = s.saved_volume;
                PromoteResult::Mismatch {
                    previous_track_id: prev,
                    previous_saved_volume: v,
                }
            }
        }
    }

    /// Clear the shadow without promoting. Returns the saved volume so
    /// the caller can restore it before the next action. Returns None
    /// if there was no shadow.
    pub fn cancel(&mut self) -> Option<i32> {
        self.state.take().map(|s| s.saved_volume)
    }

    /// Check expiry — call periodically (e.g. from tick_anticipator).
    /// Returns the saved volume if a shadow was reaped.
    pub fn reap_expired(&mut self) -> Option<i32> {
        if let Some(s) = &self.state {
            if s.is_expired() { return self.cancel(); }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fresh_manager_is_inactive() {
        let m = ShadowManager::new();
        assert!(!m.is_active());
        assert!(m.current().is_none());
    }

    #[test]
    fn should_shadow_rejects_low_confidence() {
        let m = ShadowManager::new();
        let d = m.should_shadow(0.40, false, true, 1);
        assert_eq!(d, ShadowDecision::Skip("confidence below floor"));
    }

    #[test]
    fn shadow_stream__rejects_nan_confidence() {
        // NaN < FLOOR is false in IEEE 754, so without the explicit
        // is_finite check a NaN confidence would slip past the floor
        // gate and fire a shadow stream on garbage signal. The bug
        // would burn bandwidth and decode cycles whenever the
        // anticipator hits a numerical edge.
        let m = ShadowManager::new();
        for bad in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            let d = m.should_shadow(bad, false, true, 1);
            assert_eq!(d, ShadowDecision::Skip("non-finite confidence"),
                       "{} should be rejected", bad);
        }
    }

    #[test]
    fn should_shadow_rejects_when_device_playing() {
        let m = ShadowManager::new();
        let d = m.should_shadow(0.90, true, true, 1);
        assert!(matches!(d, ShadowDecision::Skip(_)));
    }

    #[test]
    fn should_shadow_rejects_without_known_volume() {
        let m = ShadowManager::new();
        let d = m.should_shadow(0.90, false, false, 1);
        assert!(matches!(d, ShadowDecision::Skip(_)));
    }

    #[test]
    fn should_shadow_accepts_good_conditions() {
        let m = ShadowManager::new();
        assert_eq!(m.should_shadow(0.90, false, true, 1), ShadowDecision::Go);
    }

    #[test]
    fn start_transitions_to_active() {
        let mut m = ShadowManager::new();
        m.start(42, "http://x/t/abc".to_string(), 25);
        assert!(m.is_active());
        assert_eq!(m.active_track_id(), Some(42));
        let s = m.current().unwrap();
        assert_eq!(s.saved_volume, 25);
    }

    #[test]
    fn should_shadow_rejects_when_same_track_already_shadowed() {
        let mut m = ShadowManager::new();
        m.start(42, "url".to_string(), 25);
        let d = m.should_shadow(0.90, false, true, 42);
        assert!(matches!(d, ShadowDecision::Skip(_)));
    }

    #[test]
    fn promote_matching_returns_hit_with_saved_volume() {
        let mut m = ShadowManager::new();
        m.start(42, "url".to_string(), 30);
        let r = m.promote(42);
        assert_eq!(r, PromoteResult::Hit { saved_volume: 30 });
        // After promote, manager is back to inactive.
        assert!(!m.is_active());
    }

    #[test]
    fn promote_different_track_returns_mismatch() {
        let mut m = ShadowManager::new();
        m.start(42, "url".to_string(), 30);
        let r = m.promote(99);
        assert!(matches!(r, PromoteResult::Mismatch { previous_track_id: 42,
                                                       previous_saved_volume: 30 }));
        // Manager is also cleared so caller can replace.
        assert!(!m.is_active());
    }

    #[test]
    fn promote_when_idle_returns_miss() {
        let mut m = ShadowManager::new();
        assert_eq!(m.promote(42), PromoteResult::Miss);
    }

    #[test]
    fn cancel_returns_saved_volume() {
        let mut m = ShadowManager::new();
        m.start(42, "url".to_string(), 35);
        let v = m.cancel();
        assert_eq!(v, Some(35));
        assert!(!m.is_active());
    }

    #[test]
    fn cancel_when_idle_returns_none() {
        let mut m = ShadowManager::new();
        assert_eq!(m.cancel(), None);
    }

    #[test]
    fn reap_expired_keeps_fresh_shadow() {
        let mut m = ShadowManager::new();
        m.start(42, "url".to_string(), 30);
        assert!(m.reap_expired().is_none());
        assert!(m.is_active());
    }

    #[test]
    fn confidence_floor_is_sane() {
        // Documenting intent: floor should be high enough that random
        // taps don't waste bandwidth.
        assert!(SHADOW_CONFIDENCE_FLOOR >= 0.5);
        assert!(SHADOW_CONFIDENCE_FLOOR <= 0.9);
    }

    #[test]
    fn shadow_age_grows_with_time() {
        let s = ShadowState {
            track_id: 1,
            track_url: "".into(),
            saved_volume: 0,
            started_at: Instant::now() - Duration::from_secs(10),
        };
        let a = s.age();
        assert!(a >= Duration::from_secs(10));
        assert!(a < Duration::from_secs(11));
    }

    #[test]
    fn shadow_expires_after_max_age() {
        let s = ShadowState {
            track_id: 1,
            track_url: "".into(),
            saved_volume: 0,
            started_at: Instant::now() - (MAX_SHADOW_AGE + Duration::from_secs(1)),
        };
        assert!(s.is_expired());
    }
}
