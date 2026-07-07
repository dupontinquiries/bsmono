//! head_steer — decouple where a creature LOOKS from where it MOVES.
//!
//! The moonwalk: movement was driven by the gaze/vision heading, so a creature
//! glancing to the side slid sideways. Real animals look with the head over a
//! limited arc, keep walking along the body, and only swing the body around once
//! they commit to a direction. This models a head yaw **offset** relative to the
//! body that
//!   1. tracks a look target, slew-limited and clamped to a maximum arc,
//!   2. springs back toward neutral when there's nothing to look at (a *detent*),
//!   3. drags the body around only when the look is *held* past a commit
//!      threshold — and unwinds itself as the body catches up, so the gaze stays
//!      locked on the world target.
//!
//! Locomotion always uses the **body** yaw, never the gaze, so a glance can never
//! translate the body. Pure and deterministic; the bin maps `Head::offset` onto
//! the head limb's local yaw and `pull_body` onto `Creature::yaw`.

/// Wrap an angle to `(-π, π]`.
fn wrap_pi(a: f32) -> f32 {
    let twopi = std::f32::consts::TAU;
    let mut a = a % twopi;
    if a > std::f32::consts::PI {
        a -= twopi;
    } else if a <= -std::f32::consts::PI {
        a += twopi;
    }
    a
}

/// Move `cur` toward `target` by at most `max_step`, landing exactly (no overshoot).
fn approach(cur: f32, target: f32, max_step: f32) -> f32 {
    if max_step <= 0.0 {
        return cur;
    }
    let d = target - cur;
    if d.abs() <= max_step {
        target
    } else {
        cur + max_step * d.signum()
    }
}

/// Tuning for head/neck motion. Angles in radians, rates in rad/s.
#[derive(Clone, Copy, Debug)]
pub struct GazeLimits {
    /// Hard limit on how far the head can turn from the body (e.g. ~70°).
    pub max_offset: f32,
    /// How fast the head tracks a look target.
    pub slew: f32,
    /// How fast the head re-centres toward neutral when idle (the detent spring).
    pub detent: f32,
    /// Deadzone: below this the body ignores the head (a steady micro-look
    /// doesn't jitter the body). Above it the body eases toward the gaze.
    pub commit: f32,
    /// How fast the body turns to follow a held, committed gaze.
    pub follow: f32,
}

impl Default for GazeLimits {
    fn default() -> GazeLimits {
        GazeLimits {
            max_offset: 1.2,
            slew: 3.0,
            detent: 2.0,
            commit: 0.05,
            follow: 2.5,
        }
    }
}

/// A head's yaw offset relative to its body.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Head {
    /// Signed yaw relative to the body, within `±max_offset`.
    pub offset: f32,
}

impl Head {
    pub fn new() -> Head {
        Head { offset: 0.0 }
    }

    /// Track a world-space look target for one step: ease the offset toward the
    /// shortest angle to `target_yaw` from `body_yaw`, clamped to the arc and
    /// slew-limited.
    pub fn track(&mut self, body_yaw: f32, target_yaw: f32, dt: f32, lim: &GazeLimits) {
        let want = wrap_pi(target_yaw - body_yaw).clamp(-lim.max_offset, lim.max_offset);
        self.offset = approach(self.offset, want, lim.slew * dt);
    }

    /// No target: spring the head back toward neutral.
    pub fn relax(&mut self, dt: f32, lim: &GazeLimits) {
        self.offset = approach(self.offset, 0.0, lim.detent * dt);
    }

    /// The world yaw the head is pointing.
    pub fn world_yaw(&self, body_yaw: f32) -> f32 {
        body_yaw + self.offset
    }

    /// If the gaze is held past `commit`, return the body-yaw delta to apply this
    /// step (turning the body toward the head) and unwind the offset by the same
    /// amount so the head stays locked on its world target. A *glance* (within
    /// `commit`) returns 0 — the body holds course, so it never slides sideways.
    pub fn pull_body(&mut self, dt: f32, lim: &GazeLimits) -> f32 {
        if self.offset.abs() <= lim.commit {
            return 0.0;
        }
        let over = self.offset.abs() - lim.commit;
        let turn = over.min(lim.follow * dt) * self.offset.signum();
        self.offset -= turn;
        turn
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::PI;

    #[test]
    fn head_cannot_turn_past_its_limit() {
        let lim = GazeLimits::default();
        let mut h = Head::new();
        // Try to look 90° to the side; the arc caps at max_offset (~69°).
        for _ in 0..200 {
            h.track(0.0, PI / 2.0, 0.05, &lim);
        }
        assert!((h.offset - lim.max_offset).abs() < 1e-3, "offset {} should clamp to {}", h.offset, lim.max_offset);
    }

    #[test]
    fn tracking_is_slew_limited() {
        let lim = GazeLimits::default();
        let mut h = Head::new();
        h.track(0.0, 1.0, 0.05, &lim);
        assert!(h.offset <= lim.slew * 0.05 + 1e-6, "one step moved too far: {}", h.offset);
        assert!(h.offset > 0.0);
    }

    #[test]
    fn idle_head_recentres_via_detent() {
        let lim = GazeLimits::default();
        let mut h = Head { offset: 1.0 };
        for _ in 0..200 {
            h.relax(0.05, &lim);
        }
        assert!(h.offset.abs() < 1e-3, "should return to neutral, got {}", h.offset);
    }

    #[test]
    fn a_steady_micro_look_does_not_jitter_the_body() {
        // The anti-moonwalk core: locomotion uses body yaw, never gaze. A tiny
        // steady offset (within the deadzone) leaves the body untouched.
        let lim = GazeLimits::default();
        let mut h = Head { offset: 0.03 }; // below commit (0.05)
        assert_eq!(h.pull_body(0.05, &lim), 0.0);
        assert_eq!(h.offset, 0.03);
    }

    #[test]
    fn an_off_center_gaze_pulls_the_body_toward_it() {
        let lim = GazeLimits::default();
        let mut h = Head { offset: 0.6 };
        let turn = h.pull_body(0.05, &lim);
        assert!(turn > 0.0, "an off-centre look should pull the body");
        assert!(turn <= lim.follow * 0.05 + 1e-6, "but no faster than the follow rate");
    }

    #[test]
    fn a_held_gaze_drags_the_body_and_stays_locked_on_target() {
        let lim = GazeLimits::default();
        let body = 0.2_f32;
        let mut h = Head { offset: 1.0 }; // committed look
        let world_before = h.world_yaw(body);
        let turn = h.pull_body(0.05, &lim);
        assert!(turn > 0.0, "committed gaze should pull the body");
        // The head unwinds by exactly the body's turn, so the world gaze is held.
        let world_after = h.world_yaw(body + turn);
        assert!((world_after - world_before).abs() < 1e-5, "gaze should stay on target: {world_before} vs {world_after}");
    }

    #[test]
    fn body_converges_to_face_a_held_target() {
        // Look at something 1 rad to the side and keep looking: over time the body
        // comes around to face it and the head returns to neutral.
        let lim = GazeLimits::default();
        let mut body = 0.0_f32;
        let mut h = Head::new();
        let target = 1.0_f32;
        for _ in 0..400 {
            h.track(body, target, 0.05, &lim);
            body += h.pull_body(0.05, &lim);
        }
        assert!((body - target).abs() < lim.commit + 0.03, "body should face the target, got {body}");
        assert!(h.offset.abs() < lim.commit + 0.03, "head should settle to near-neutral once aligned, got {}", h.offset);
    }
}
