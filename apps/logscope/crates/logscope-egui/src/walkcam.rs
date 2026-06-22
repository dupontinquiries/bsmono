//! walkcam.rs — a critically-damped "follow camera" for a prezi-style walkthrough of the conversation
//! graph. Ported from voiceterm's follow_camera (the "cinematic camera" design): the camera eases
//! toward a focus target with NO overshoot while preserving velocity (so stepping node→node glides
//! instead of snapping from rest), HOLDS within a deadband (no jitter for tiny moves), and CUTS
//! (snaps, resetting the smoothing) when the focus jumps far. Pure math — the UI feeds dt + the focus
//! index and reads back the scroll position. No egui, no I/O.

/// One step of a critically-damped spring (damping ratio = 1): converges to `target` smoothly, never
/// overshoots, and preserves velocity (a moving target is tracked, not chased from rest). Stable
/// implicit integrator (Ryan Juckett's form). `omega` is the stiffness in rad/s.
pub fn damped_step(pos: f32, vel: f32, target: f32, omega: f32, dt: f32) -> (f32, f32) {
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

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FramingTarget { pub top: f32, pub is_cut: bool }

/// Scroll-top (content px) that frames item `index` at `frac` down the viewport (0=top, 0.5=center),
/// clamped to [0, content−viewport]. None when within `deadband_px` of `current_top` (hold).
/// is_cut = the move is large enough that the UI should SNAP rather than animate.
pub fn framing_target(index: usize, item_h: f32, viewport_h: f32, content_h: f32,
                      current_top: f32, frac: f32, deadband_px: f32, cut_px: f32) -> Option<FramingTarget> {
    if item_h <= 0.0 || viewport_h <= 0.0 { return None; }
    let item_y = index as f32 * item_h;
    let raw = item_y - frac.clamp(0.0, 1.0) * viewport_h + item_h * 0.5;
    let max_top = (content_h - viewport_h).max(0.0);
    let target = raw.clamp(0.0, max_top);
    let delta = (target - current_top).abs();
    if delta < deadband_px { return None; }
    Some(FramingTarget { top: target, is_cut: delta > cut_px })
}

/// A walkthrough camera over a vertical item layout. Holds a scroll position + velocity; `focus()`
/// re-aims it (snapping on a cut, easing otherwise); `tick()` advances the spring one frame.
#[derive(Clone, Debug)]
pub struct WalkCam {
    pub top: f32, // current scroll-top (content px)
    vel: f32,
    target: f32,
    omega: f32,   // stiffness (rad/s)
}

impl WalkCam {
    pub fn new(stiffness: f32) -> Self { Self { top: 0.0, vel: 0.0, target: 0.0, omega: stiffness.max(0.1) } }

    /// Re-aim at item `index`, centering it in the viewport. Returns true if it was a CUT (snap): on a
    /// cut the spring resets to rest at the new position so it doesn't dolly across a huge jump. A move
    /// within the deadband is ignored (hold).
    pub fn focus(&mut self, index: usize, item_h: f32, viewport_h: f32, content_h: f32, deadband_px: f32, cut_px: f32) -> bool {
        match framing_target(index, item_h, viewport_h, content_h, self.top, 0.5, deadband_px, cut_px) {
            None => false,
            Some(ft) => {
                self.target = ft.top;
                if ft.is_cut { self.top = ft.top; self.vel = 0.0; true } else { false }
            }
        }
    }

    /// Advance the damped spring by `dt` seconds; returns the new scroll-top.
    pub fn tick(&mut self, dt: f32) -> f32 {
        let (p, v) = damped_step(self.top, self.vel, self.target, self.omega, dt);
        self.top = p; self.vel = v;
        self.top
    }

    /// Settled = close to target with little velocity, so the UI can stop requesting repaints.
    pub fn settled(&self) -> bool { (self.target - self.top).abs() < 0.5 && self.vel.abs() < 0.5 }
}

#[cfg(test)]
mod tests {
    use super::*;
    const DT: f32 = 1.0 / 60.0;

    #[test]
    fn damped_step_converges_without_overshoot() {
        let (mut p, mut v) = (0.0f32, 0.0f32);
        let target = 100.0;
        let mut max = 0.0f32;
        for _ in 0..600 {
            let (np, nv) = damped_step(p, v, target, 12.0, DT);
            p = np; v = nv;
            if p > max { max = p; }
        }
        assert!((p - target).abs() < 0.5, "converges to target, got {p}");
        assert!(max <= target + 0.01, "critically damped: never overshoots (max {max} <= {target})");
    }

    #[test]
    fn damped_step_preserves_velocity_midflight() {
        let (p, v) = damped_step(0.0, 0.0, 100.0, 12.0, DT);
        assert!(p > 0.0 && v > 0.0, "from rest it starts moving toward target with nonzero velocity");
    }

    #[test]
    fn framing_centers_and_honors_cut() {
        // item 10 at 20px, centered in a 400px viewport ⇒ top ≈ 10*20 - 200 + 10 = 10
        let ft = framing_target(10, 20.0, 400.0, 4000.0, 0.0, 0.5, 4.0, 200.0).unwrap();
        assert!((ft.top - 10.0).abs() < 0.01, "centers the item, got {}", ft.top);
        assert!(!ft.is_cut, "delta 10 < cut 200 ⇒ a dolly, not a cut");
        let far = framing_target(100, 20.0, 400.0, 8000.0, 0.0, 0.5, 4.0, 200.0).unwrap();
        assert!(far.is_cut, "a far jump (delta ≫ 200) ⇒ cut");
    }

    #[test]
    fn framing_holds_within_deadband() {
        // current_top already framing item 10 ⇒ within deadband ⇒ None (hold)
        let target = framing_target(10, 20.0, 400.0, 4000.0, 10.0, 0.5, 4.0, 200.0);
        assert!(target.is_none(), "no re-frame within the deadband");
    }

    #[test]
    fn cut_snaps_then_holds_dolly_eases() {
        let mut c = WalkCam::new(12.0);
        // large jump ⇒ cut ⇒ snaps
        let cut = c.focus(100, 20.0, 400.0, 8000.0, 4.0, 200.0);
        assert!(cut, "far focus is a cut");
        let after_cut = c.top;
        // a nearby focus ⇒ eases (not a cut), converges over ticks
        c.focus(102, 20.0, 400.0, 8000.0, 4.0, 200.0);
        for _ in 0..600 { c.tick(DT); }
        assert!(c.settled(), "eases to the nearby target and settles");
        assert!(c.top > after_cut, "moved toward the later (lower) node");
    }
}
