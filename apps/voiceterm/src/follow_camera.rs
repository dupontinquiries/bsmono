//! follow_camera: a CINEMATIC "camera follows cursor" controller.
//!
//! v0.1.506 rewrite. The user's insight about the Pulsar plugin
//! impakt-cam was one phrase: "it's more cinematic." That names the
//! real principle and a more robust algorithm than impakt's accidental
//! version (which averaged SCREEN-coord cursor positions over a time
//! window — feeling cinematic but collapsing when reference frames
//! mixed during scroll; see docs/design/cinematic_camera.md).
//!
//! A basic camera is a SECURITY camera: points where the subject IS.
//! A cinematic camera:
//!   - LEADS a moving subject using its velocity (lead room),
//!   - holds still for tiny movements (DEADBAND — no jitter chase),
//!   - moves with CRITICALLY-DAMPED easing (smooth, never overshoots,
//!     velocity-continuous across re-targets), and
//!   - CUTS (snaps) rather than dollies when the subject leaves frame
//!     (a big jump), resetting smoothing state across the cut.
//!
//! Works in STABLE CONTENT COORDINATES (line-based), not DOM screen
//! rects — so there is NO reference-frame-mixing collapse mode.
//! Zero dependencies (impakt pulled ALL of GSAP + debounce to use one
//! tween). The caller ticks it like a native QML frame update; it
//! advances ONE scroll position — no per-frame JS.

use std::collections::VecDeque;

#[derive(Clone, Copy, Debug)]
pub struct CameraConfig {
    /// Spring stiffness (rad/s). Higher = snappier follow. ~12 is a
    /// smooth, film-like dolly at 60fps.
    pub stiffness: f32,
    /// Look-ahead seconds: target leads the cursor by velocity *
    /// this. 0 = no lead (center on cursor); ~0.15 = subtle lead room.
    pub lead_seconds: f32,
    /// Deadband in lines: if the framing error is within this many
    /// lines, hold (don't chase sub-line jitter).
    pub deadband_lines: f32,
    /// A cursor jump larger than this many lines is a CUT (snap),
    /// not a dolly. Resets velocity + history.
    pub cut_threshold_lines: usize,
    /// Window (ms) of (time, line) samples kept for velocity
    /// estimation.
    pub velocity_window_ms: u64,
    /// Max look-ahead in lines, so a fast scroll can't lead the camera
    /// off into space.
    pub max_lead_lines: f32,
}

impl Default for CameraConfig {
    fn default() -> Self {
        Self {
            stiffness: 12.0,
            lead_seconds: 0.15,
            deadband_lines: 0.75,
            cut_threshold_lines: 150,
            velocity_window_ms: 120,
            max_lead_lines: 8.0,
        }
    }
}

/// The camera's output each tick.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum CameraMove {
    /// Smoothly dolly: set scroll-top (px) to this. The value changes
    /// gradually tick-to-tick (the damped motion).
    Dolly(f32),
    /// Cut: snap scroll-top (px) here immediately (subject left frame).
    Cut(f32),
    /// Hold: within deadband, no move this tick.
    Hold,
}

/// Cinematic follow camera. One per view. All distances in CONTENT
/// coordinates (px), lines used only for velocity + jump detection.
pub struct FollowCamera {
    cfg: CameraConfig,
    /// Current camera scroll-top (content px). None until first frame.
    cam_px: Option<f32>,
    /// Camera velocity (px/s) — preserved across re-targets (the fix
    /// for impakt's restart-from-rest stutter).
    cam_vel: f32,
    /// Recent (time_ms, cursor_line) for velocity estimation.
    samples: VecDeque<(u64, f32)>,
    last_line: Option<usize>,
}

impl FollowCamera {
    pub fn new(cfg: CameraConfig) -> Self {
        Self { cfg, cam_px: None, cam_vel: 0.0, samples: VecDeque::new(), last_line: None }
    }

    fn evict(&mut self, now_ms: u64) {
        let cutoff = now_ms.saturating_sub(self.cfg.velocity_window_ms);
        while let Some(&(t, _)) = self.samples.front() {
            if t < cutoff { self.samples.pop_front(); } else { break; }
        }
    }

    /// Cursor velocity in lines/sec from the sample window (oldest to
    /// newest). 0 if fewer than 2 samples or zero time span.
    fn cursor_velocity_lps(&self) -> f32 {
        if self.samples.len() < 2 { return 0.0; }
        let (t0, l0) = *self.samples.front().unwrap();
        let (t1, l1) = *self.samples.back().unwrap();
        let dt = (t1.saturating_sub(t0)) as f32 / 1000.0;
        if dt <= 0.0 { return 0.0; }
        (l1 - l0) / dt
    }

    /// Advance the camera for a cursor at `cursor_line`, with the given
    /// viewport + line geometry, at time `now_ms` with frame step
    /// `dt_s` seconds. Returns the move to apply to the scroll-top.
    pub fn tick(
        &mut self,
        now_ms: u64,
        cursor_line: usize,
        line_height_px: f32,
        viewport_height_px: f32,
        dt_s: f32,
    ) -> CameraMove {
        let half = (viewport_height_px / 2.0).max(0.0);
        let cursor_px = cursor_line as f32 * line_height_px;

        // ── CUT: subject left the frame (big jump) ──────────────────
        if let Some(prev) = self.last_line {
            let dist = if cursor_line > prev { cursor_line - prev } else { prev - cursor_line };
            if dist > self.cfg.cut_threshold_lines {
                let centered = (cursor_px - half).max(0.0);
                self.cam_px = Some(centered);
                self.cam_vel = 0.0;
                self.samples.clear();
                self.samples.push_back((now_ms, cursor_line as f32));
                self.last_line = Some(cursor_line);
                return CameraMove::Cut(centered);
            }
        }

        // record sample for velocity, update history
        self.samples.push_back((now_ms, cursor_line as f32));
        self.evict(now_ms);
        self.last_line = Some(cursor_line);

        // ── LEAD ROOM: frame ahead of motion by velocity ────────────
        let vel_lps = self.cursor_velocity_lps();
        let lead_lines = (vel_lps * self.cfg.lead_seconds)
            .clamp(-self.cfg.max_lead_lines, self.cfg.max_lead_lines);
        let lead_px = lead_lines * line_height_px;

        // target scroll-top so (cursor + lead) sits at viewport center
        let target = (cursor_px + lead_px - half).max(0.0);

        // first frame: snap-initialize (no prior camera state)
        let cam = match self.cam_px {
            Some(c) => c,
            None => {
                self.cam_px = Some(target);
                self.cam_vel = 0.0;
                return CameraMove::Cut(target); // initial placement
            }
        };

        // ── DEADBAND: hold for sub-threshold framing error ──────────
        let err_lines = (target - cam).abs() / line_height_px.max(1.0);
        if err_lines <= self.cfg.deadband_lines {
            // bleed off residual velocity so we settle cleanly
            self.cam_vel *= 0.5;
            return CameraMove::Hold;
        }

        // ── CRITICALLY-DAMPED step toward the (moving) target ───────
        let (np, nv) = damped_step(cam, self.cam_vel, target, self.cfg.stiffness, dt_s);
        self.cam_px = Some(np);
        self.cam_vel = nv;
        CameraMove::Dolly(np)
    }

    /// Reset (view changed / follow disabled).
    pub fn reset(&mut self) {
        self.cam_px = None;
        self.cam_vel = 0.0;
        self.samples.clear();
        self.last_line = None;
    }

    /// Current camera position if initialized (for QML to read).
    pub fn position(&self) -> Option<f32> { self.cam_px }
}

/// One step of a critically-damped spring (damping ratio = 1):
/// converges to `target` smoothly and NEVER overshoots, while
/// preserving velocity (so a moving target is tracked, not chased
/// from rest). Stable implicit integrator (Ryan Juckett's form).
fn damped_step(pos: f32, vel: f32, target: f32, omega: f32, dt: f32) -> (f32, f32) {
    if dt <= 0.0 || omega <= 0.0 { return (pos, vel); }
    let f = 1.0 + 2.0 * dt * omega;
    let oo = omega * omega;
    let hoo = dt * oo;
    let hhoo = dt * hoo;
    let det_inv = 1.0 / (f + hhoo);
    let new_pos = (f * pos + dt * vel + hhoo * target) * det_inv;
    let new_vel = (vel + hoo * (target - pos)) * det_inv;
    (new_pos, new_vel)
}

/// v0.1.692: EVENT-DRIVEN framing target (preferred over the per-frame spring
/// tick). Instead of polling a spring every 16ms, callers compute the scroll-
/// top the viewport SHOULD settle at when the cursor moves, then let the UI
/// layer interpolate (QML Behavior/SpringAnimation) — which naturally retains
/// in-flight motion and re-targets with a constraint-satisfying lerp.
///
/// Returns the scroll-top (content px) framing `cursor_line` at `frac` down
/// the viewport (0.0=top, 0.5=center), clamped to [0, content-vh]. None when
/// within `deadband_px` of `current_top` (hold). is_cut = the move is a large
/// jump the UI should SNAP rather than animate.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FramingTarget {
    pub top: f32,
    pub is_cut: bool,
}

pub fn framing_target(cursor_line: usize, line_h: f32, viewport_h: f32,
                      content_h: f32, current_top: f32, frac: f32,
                      deadband_px: f32, cut_px: f32) -> Option<FramingTarget> {
    if line_h <= 0.0 || viewport_h <= 0.0 { return None; }
    let line_y = cursor_line as f32 * line_h;
    let raw = line_y - frac.clamp(0.0, 1.0) * viewport_h + line_h * 0.5;
    let max_top = (content_h - viewport_h).max(0.0);
    let target = raw.clamp(0.0, max_top);
    let delta = (target - current_top).abs();
    if delta < deadband_px {
        return None;
    }
    Some(FramingTarget { top: target, is_cut: delta > cut_px })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cam() -> FollowCamera { FollowCamera::new(CameraConfig::default()) }
    const LH: f32 = 20.0;
    const VH: f32 = 400.0;
    const DT: f32 = 1.0 / 60.0;

    #[test]
    fn first_frame_cuts_to_centered() {
        let mut c = cam();
        let m = c.tick(1000, 20, LH, VH, DT);
        assert_eq!(m, CameraMove::Cut(200.0));
        assert_eq!(c.position(), Some(200.0));
    }

    #[test]
    fn framing_centers_and_clamps() {
        // line 20 at 19px, viewport 400, content 2000, center (0.5)
        let t = framing_target(20, 19.0, 400.0, 2000.0, 0.0, 0.5, 2.0, 600.0).unwrap();
        // raw = 380 - 200 + 9.5 = 189.5
        assert!((t.top - 189.5).abs() < 0.6, "got {}", t.top);
        assert!(!t.is_cut);
        // near top clamps to 0
        let t0 = framing_target(1, 19.0, 400.0, 2000.0, 50.0, 0.5, 2.0, 600.0).unwrap();
        assert_eq!(t0.top, 0.0);
    }

    #[test]
    fn framing_deadband_holds() {
        // target ~189.5; current 189 → within 2px deadband → hold
        assert!(framing_target(20, 19.0, 400.0, 2000.0, 189.0, 0.5, 2.0, 600.0).is_none());
    }

    #[test]
    fn framing_detects_cut() {
        // jump from top 0 to a far line → is_cut
        let t = framing_target(300, 19.0, 400.0, 10000.0, 0.0, 0.5, 2.0, 600.0).unwrap();
        assert!(t.is_cut, "large jump should be a cut");
    }

    #[test]
    fn framing_clamps_to_max() {
        // last line; can't scroll past content-viewport
        let t = framing_target(100, 19.0, 400.0, 1000.0, 0.0, 0.5, 2.0, 600.0).unwrap();
        assert!(t.top <= 600.0 + 0.01);
    }

    #[test]
    fn damped_motion_never_overshoots() {
        let mut c = cam();
        c.tick(1000, 20, LH, VH, DT);
        let mut t = 1000u64;
        let mut maxpos = c.position().unwrap();
        for _ in 0..240 {
            t += 16;
            let m = c.tick(t, 40, LH, VH, DT);
            if let CameraMove::Dolly(p) | CameraMove::Cut(p) = m {
                if p > maxpos { maxpos = p; }
            }
        }
        let settled = c.position().unwrap();
        // Settles WITHIN the deadband of the target (0.75 lines = 15px),
        // not exactly on it — that's the deadband preventing a
        // jitter-chase to the exact pixel. Assert it's close AND short
        // (never past), which is the no-overshoot guarantee.
        assert!(settled > 600.0 - 16.0 && settled <= 600.5,
            "should settle just shy of 600 within deadband, got {}", settled);
        assert!(maxpos <= 600.5, "critically damped must not overshoot, peak {}", maxpos);
    }

    #[test]
    fn deadband_holds_for_tiny_movement() {
        let mut c = cam();
        c.tick(1000, 20, LH, VH, DT);
        let m = c.tick(1016, 20, LH, VH, DT);
        assert_eq!(m, CameraMove::Hold, "no movement within deadband");
    }

    #[test]
    fn big_jump_cuts_not_dollies() {
        let mut c = cam();
        c.tick(1000, 20, LH, VH, DT);
        let m = c.tick(1016, 520, LH, VH, DT);
        match m {
            CameraMove::Cut(p) => assert_eq!(p, 520.0 * LH - 200.0),
            other => panic!("expected Cut, got {:?}", other),
        }
        assert_eq!(c.cam_vel, 0.0, "cut zeroes velocity");
    }

    #[test]
    fn lead_room_anticipates_downward_motion() {
        let geom = |c: &mut FollowCamera| {
            let mut t = 1000u64;
            for line in (10..40).step_by(2) {
                t += 16;
                c.tick(t, line, LH, VH, DT);
            }
            c.position().unwrap()
        };
        let mut lead = FollowCamera::new(CameraConfig { lead_seconds: 0.15, ..Default::default() });
        let mut nolead = FollowCamera::new(CameraConfig { lead_seconds: 0.0, ..Default::default() });
        let lp = geom(&mut lead);
        let np = geom(&mut nolead);
        assert!(lp >= np, "lead camera should be at/ahead of no-lead (lead {} vs {})", lp, np);
    }

    #[test]
    fn reset_clears_state() {
        let mut c = cam();
        c.tick(1000, 20, LH, VH, DT);
        c.reset();
        assert_eq!(c.position(), None);
        let m = c.tick(2000, 5, LH, VH, DT);
        assert!(matches!(m, CameraMove::Cut(_)));
    }

    #[test]
    fn target_never_negative() {
        let mut c = cam();
        let m = c.tick(1000, 1, LH, VH, DT);
        assert_eq!(m, CameraMove::Cut(0.0));
    }

    #[test]
    fn damped_step_no_overshoot_unit() {
        let (mut p, mut v) = (0.0f32, 0.0f32);
        let mut peak = 0.0f32;
        for _ in 0..300 {
            let (np, nv) = damped_step(p, v, 100.0, 12.0, DT);
            p = np; v = nv;
            if p > peak { peak = p; }
        }
        assert!((p - 100.0).abs() < 0.5);
        assert!(peak <= 100.01, "no overshoot, peak {}", peak);
    }

    #[test]
    fn zero_dt_is_noop_step() {
        let (p, v) = damped_step(50.0, 3.0, 100.0, 12.0, 0.0);
        assert_eq!((p, v), (50.0, 3.0));
    }

    #[test]
    fn velocity_estimate_from_samples() {
        let mut c = cam();
        c.tick(1000, 10, LH, VH, DT);
        c.tick(1050, 15, LH, VH, DT);
        let v = c.cursor_velocity_lps();
        assert!((v - 100.0).abs() < 1.0, "expected ~100 lines/s, got {}", v);
    }
}
