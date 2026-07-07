//! `timebend_scrub` — the player-facing camera mode for **timebending**.
//!
//! The sim already models branching timelines ([`crate::sim::timeline`]); this
//! is the *viewfinder* for them. While the player roams, we record a rolling
//! ring of "where I was and what the world looked like" snapshots — camera
//! pose + world clock. Entering scrub mode freezes the player, fades the
//! screen, and lets them scroll back through those vantages: the camera flies
//! to each past position and the sky is wound to the time-of-day it was then,
//! so the *live* world re-renders exactly as it appeared from that spot at that
//! moment. Every scrub step triggers a short fade so the jump reads as a
//! deliberate cut rather than a teleport.
//!
//! Pure logic — no rendering, no I/O — so it unit-tests on the dev box even
//! though the on-screen result can't be verified here. The bin drives it:
//! `maybe_capture` each live frame, `enter`/`exit` on the hotkey, `scrub` on
//! scroll, and reads `current()` + `fade` to override the camera and veil.

use nalgebra_glm as glm;
use std::collections::VecDeque;
use std::time::Instant;

/// One recorded vantage: where the camera was and what the world clock read.
#[derive(Clone, Debug)]
pub struct WorldSnapshot {
    pub pos: glm::Vec3,
    pub yaw: f32,
    pub pitch: f32,
    /// `sky_state.time_seconds` at capture — replayed so lighting matches.
    pub sky_time: f64,
    /// Seconds of real wall-time since session start at capture (for the HUD).
    pub stamp_s: f32,
}

/// Saved live state to restore when the player exits scrub mode.
#[derive(Clone, Copy)]
struct LiveState {
    pos: glm::Vec3,
    yaw: f32,
    pitch: f32,
    sky_time: f64,
}

pub struct TimebendScrub {
    snaps: VecDeque<WorldSnapshot>,
    cap: usize,
    interval_s: f32,
    last_capture: Instant,
    session_start: Instant,

    /// True while the player is scrubbing (camera + sim frozen).
    pub active: bool,
    /// Index into `snaps` of the vantage currently shown.
    pub index: usize,
    /// Transition veil, 0..1. Pulsed to 1 on each jump, decays each frame.
    pub fade: f32,

    saved: Option<LiveState>,
}

impl TimebendScrub {
    pub fn new() -> Self {
        // Defaults overridable via env so the operator can tune the window
        // depth without a rebuild (e.g. a longer history for a flythrough).
        let cap = std::env::var("UNENDLICH_TIMEBEND_SNAPSHOTS")
            .ok().and_then(|s| s.parse().ok()).unwrap_or(120usize).clamp(8, 4096);
        let interval_s = std::env::var("UNENDLICH_TIMEBEND_INTERVAL_S")
            .ok().and_then(|s| s.parse().ok()).unwrap_or(1.5f32).clamp(0.1, 30.0);
        let now = Instant::now();
        Self {
            snaps: VecDeque::with_capacity(cap),
            cap,
            interval_s,
            last_capture: now - std::time::Duration::from_secs(3600),
            session_start: now,
            active: false,
            index: 0,
            fade: 0.0,
            saved: None,
        }
    }

    pub fn len(&self) -> usize { self.snaps.len() }
    pub fn is_empty(&self) -> bool { self.snaps.is_empty() }

    /// Record the live vantage if the capture interval has elapsed. No-op while
    /// scrubbing (we don't want to record the frozen replay camera).
    pub fn maybe_capture(&mut self, pos: glm::Vec3, yaw: f32, pitch: f32, sky_time: f64) {
        if self.active { return; }
        if self.last_capture.elapsed().as_secs_f32() < self.interval_s { return; }
        self.last_capture = Instant::now();
        let stamp_s = self.session_start.elapsed().as_secs_f32();
        if self.snaps.len() >= self.cap {
            self.snaps.pop_front();
        }
        self.snaps.push_back(WorldSnapshot { pos, yaw, pitch, sky_time, stamp_s });
    }

    /// Enter scrub mode: stash the live vantage, jump to the most recent
    /// snapshot, and kick a fade. Returns false (no-op) if nothing's recorded.
    pub fn enter(&mut self, pos: glm::Vec3, yaw: f32, pitch: f32, sky_time: f64) -> bool {
        if self.snaps.is_empty() { return false; }
        self.saved = Some(LiveState { pos, yaw, pitch, sky_time });
        self.active = true;
        self.index = self.snaps.len() - 1;
        self.fade = 1.0;
        true
    }

    /// Leave scrub mode. Returns the live vantage to restore (pos, yaw, pitch,
    /// sky_time), and kicks a final fade.
    pub fn exit(&mut self) -> Option<(glm::Vec3, f32, f32, f64)> {
        self.active = false;
        self.fade = 1.0;
        self.saved.take().map(|s| (s.pos, s.yaw, s.pitch, s.sky_time))
    }

    /// Move the scrub cursor by `delta` snapshots (e.g. scroll wheel). Clamps to
    /// the ends and fades on any actual move.
    pub fn scrub(&mut self, delta: i32) {
        if !self.active || self.snaps.is_empty() { return; }
        let max = self.snaps.len() as i32 - 1;
        let next = (self.index as i32 + delta).clamp(0, max) as usize;
        if next != self.index {
            self.index = next;
            self.fade = (self.fade + 0.85).min(1.0);
        }
    }

    /// The vantage currently being shown.
    pub fn current(&self) -> Option<&WorldSnapshot> {
        self.snaps.get(self.index)
    }

    /// Decay the transition veil. Call once per frame with the frame dt.
    pub fn tick_fade(&mut self, dt: f32) {
        if self.fade > 0.0 {
            // ~0.4s to clear from full — quick, cinematic cut.
            self.fade = (self.fade - dt * 2.5).max(0.0);
        }
    }

    /// Fraction 0..1 of where the cursor sits in the timeline (for a HUD bar).
    pub fn timeline_fraction(&self) -> f32 {
        if self.snaps.len() <= 1 { return 1.0; }
        self.index as f32 / (self.snaps.len() as f32 - 1.0)
    }

    /// One-line HUD label: position in the ring + how long ago it was captured.
    pub fn hud_label(&self) -> String {
        match self.current() {
            Some(s) => {
                let ago = (self.session_start.elapsed().as_secs_f32() - s.stamp_s).max(0.0);
                format!(
                    "TIMEBEND  ·  vantage {}/{}  ·  {:.0}s ago  ·  scroll to scrub, F8 to return",
                    self.index + 1, self.snaps.len(), ago
                )
            }
            None => "TIMEBEND  ·  (no history yet)".to_string(),
        }
    }
}

impl Default for TimebendScrub {
    fn default() -> Self { Self::new() }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn v(x: f32) -> glm::Vec3 { glm::vec3(x, 0.0, 0.0) }

    #[test]
    fn capture_respects_interval_and_cap() {
        let mut t = TimebendScrub::new();
        t.interval_s = 0.0; // capture every call
        t.cap = 3;
        for i in 0..5 {
            t.maybe_capture(v(i as f32), 0.0, 0.0, i as f64);
        }
        assert_eq!(t.len(), 3, "ring buffer capped");
        // Oldest two dropped; first remaining is the 3rd capture (x=2).
        assert!((t.snaps.front().unwrap().pos.x - 2.0).abs() < 1e-6);
    }

    #[test]
    fn enter_jumps_to_latest_and_scrub_clamps() {
        let mut t = TimebendScrub::new();
        t.interval_s = 0.0;
        for i in 0..4 { t.maybe_capture(v(i as f32), 0.0, 0.0, i as f64); }
        assert!(t.enter(v(99.0), 1.0, 0.5, 9.0));
        assert!(t.active);
        assert_eq!(t.index, 3, "starts at most recent");
        t.scrub(-10);
        assert_eq!(t.index, 0, "clamps to oldest");
        t.scrub(100);
        assert_eq!(t.index, 3, "clamps to newest");
    }

    #[test]
    fn exit_restores_saved_live_state() {
        let mut t = TimebendScrub::new();
        t.interval_s = 0.0;
        t.maybe_capture(v(1.0), 0.0, 0.0, 1.0);
        t.enter(v(42.0), 1.23, -0.5, 7.0);
        let (p, y, pi, sk) = t.exit().expect("saved live state");
        assert!((p.x - 42.0).abs() < 1e-6);
        assert!((y - 1.23).abs() < 1e-6);
        assert!((pi + 0.5).abs() < 1e-6);
        assert!((sk - 7.0).abs() < 1e-9);
        assert!(!t.active);
    }

    #[test]
    fn enter_noop_without_history() {
        let mut t = TimebendScrub::new();
        assert!(!t.enter(v(0.0), 0.0, 0.0, 0.0));
        assert!(!t.active);
    }
}
