//! sim::moving_platform — deterministic platform paths + rider carry.
//!
//! A moving platform (a lift, a drifting stone, a raft) follows a fixed path and
//! must **carry** whatever stands on it, or riders skate off. Both halves are
//! pure kinematics: given waypoints, a speed, and a loop mode, [`Platform`]
//! evaluates its position at any time `t` — no per-frame state, so it's
//! rewind/replay-safe and identical on every peer — and [`Platform::delta`]
//! gives the per-tick displacement a rider inherits. The contact test (is the
//! body actually on top?) stays with the caller; [`carry`] and [`on_top`] are
//! the small helpers for applying the result. Std-only, deterministic.

/// How the platform treats the end of its waypoint path.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LoopMode {
    /// Travel to the last waypoint and stop there.
    Once,
    /// Wrap back to the first waypoint (a seam jump unless the path is closed).
    Loop,
    /// Bounce back and forth — no seam, so displacement stays continuous
    /// (the safe default for carrying riders).
    PingPong,
}

/// A platform that moves along `waypoints` at constant `speed` (blocks/sec).
#[derive(Clone, Debug)]
pub struct Platform {
    pub waypoints: Vec<[f32; 3]>,
    pub speed: f32,
    pub mode: LoopMode,
}

fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn len3(v: [f32; 3]) -> f32 {
    (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt()
}
fn lerp3(a: [f32; 3], b: [f32; 3], u: f32) -> [f32; 3] {
    [a[0] + (b[0] - a[0]) * u, a[1] + (b[1] - a[1]) * u, a[2] + (b[2] - a[2]) * u]
}

impl Platform {
    pub fn new(waypoints: Vec<[f32; 3]>, speed: f32, mode: LoopMode) -> Platform {
        Platform { waypoints, speed, mode }
    }

    /// Total arclength of the polyline (sum of segment lengths).
    pub fn path_length(&self) -> f32 {
        self.waypoints.windows(2).map(|w| len3(sub(w[1], w[0]))).sum()
    }

    /// Position at arclength `s` along the polyline (clamped to `[0, length]`).
    fn pos_at_arclen(&self, s: f32) -> [f32; 3] {
        let pts = &self.waypoints;
        if pts.is_empty() {
            return [0.0, 0.0, 0.0];
        }
        if pts.len() == 1 {
            return pts[0];
        }
        let total = self.path_length();
        if total <= 1e-9 {
            return pts[0];
        }
        let s = s.clamp(0.0, total);
        let mut acc = 0.0;
        for w in pts.windows(2) {
            let seg = len3(sub(w[1], w[0]));
            if seg <= 1e-9 {
                continue;
            }
            if s <= acc + seg {
                return lerp3(w[0], w[1], (s - acc) / seg);
            }
            acc += seg;
        }
        *pts.last().unwrap()
    }

    /// Position at time `t` (seconds), applying the loop mode.
    pub fn pos_at(&self, t: f32) -> [f32; 3] {
        let total = self.path_length();
        if self.waypoints.is_empty() {
            return [0.0, 0.0, 0.0];
        }
        if total <= 1e-9 || self.speed <= 0.0 || t <= 0.0 {
            // no motion (or before start): sit at the first waypoint
            return if t <= 0.0 { self.waypoints[0] } else { self.pos_at_arclen(self.speed.max(0.0) * t) };
        }
        let d = self.speed * t;
        let s = match self.mode {
            LoopMode::Once => d.min(total),
            LoopMode::Loop => d.rem_euclid(total),
            LoopMode::PingPong => {
                let p = d.rem_euclid(2.0 * total);
                if p > total {
                    2.0 * total - p
                } else {
                    p
                }
            }
        };
        self.pos_at_arclen(s)
    }

    /// Displacement between two times — the vector a rider standing on the
    /// platform should be moved by over `[t0, t1]`. Continuous for `Once`/
    /// `PingPong`; for `Loop` the wrap seam produces a one-tick jump the caller
    /// can detect by its large magnitude.
    pub fn delta(&self, t0: f32, t1: f32) -> [f32; 3] {
        sub(self.pos_at(t1), self.pos_at(t0))
    }

    /// Central-difference velocity estimate at `t` (blocks/sec).
    pub fn velocity_at(&self, t: f32, eps: f32) -> [f32; 3] {
        let e = eps.max(1e-4);
        let d = self.delta(t - e, t + e);
        [d[0] / (2.0 * e), d[1] / (2.0 * e), d[2] / (2.0 * e)]
    }
}

/// Move a rider by a platform displacement (add the delta). The caller applies
/// this only when the rider is [`on_top`].
pub fn carry(rider_pos: [f32; 3], platform_delta: [f32; 3]) -> [f32; 3] {
    [rider_pos[0] + platform_delta[0], rider_pos[1] + platform_delta[1], rider_pos[2] + platform_delta[2]]
}

/// Is a rider standing on the platform's top surface? `rider_foot_y` is the
/// bottom of the rider; `platform_top_y` the platform's surface; `tol` the
/// contact slop (a few cm). True when the foot is within `tol` above the top.
pub fn on_top(rider_foot_y: f32, platform_top_y: f32, tol: f32) -> bool {
    let gap = rider_foot_y - platform_top_y;
    gap >= -tol && gap <= tol
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line() -> Platform {
        // 0→10 along X at 2 blocks/s: full traverse takes 5 s.
        Platform::new(vec![[0.0, 0.0, 0.0], [10.0, 0.0, 0.0]], 2.0, LoopMode::Once)
    }

    #[test]
    fn once_reaches_the_end_and_stops() {
        let p = line();
        assert_eq!(p.pos_at(0.0), [0.0, 0.0, 0.0]);
        let mid = p.pos_at(2.5);
        assert!((mid[0] - 5.0).abs() < 1e-4, "halfway at t=2.5, got {mid:?}");
        let end = p.pos_at(5.0);
        assert!((end[0] - 10.0).abs() < 1e-4);
        let past = p.pos_at(100.0);
        assert!((past[0] - 10.0).abs() < 1e-4, "Once should clamp at the end");
    }

    #[test]
    fn loop_wraps_to_the_start() {
        let mut p = line();
        p.mode = LoopMode::Loop;
        // length 10, speed 2 ⇒ period 5 s; at t=5 it's back at the start.
        let a = p.pos_at(0.0);
        let b = p.pos_at(5.0);
        assert!((a[0] - b[0]).abs() < 1e-4, "loop returns to start after one period");
    }

    #[test]
    fn pingpong_bounces_back() {
        let mut p = line();
        p.mode = LoopMode::PingPong;
        // period is 2·L/speed = 10 s. At t=5 (one length) it's at the far end;
        // at t=7.5 it's heading back through the middle; at t=10 back to start.
        assert!((p.pos_at(5.0)[0] - 10.0).abs() < 1e-4);
        assert!((p.pos_at(7.5)[0] - 5.0).abs() < 1e-4);
        assert!((p.pos_at(10.0)[0] - 0.0).abs() < 1e-4);
    }

    #[test]
    fn delta_matches_speed_on_a_straight_run() {
        let p = line();
        let dt = 0.1;
        let d = p.delta(1.0, 1.0 + dt);
        assert!((len3(d) - p.speed * dt).abs() < 1e-4, "|delta| ≈ speed·dt, got {}", len3(d));
    }

    #[test]
    fn a_rider_is_carried_by_the_platform_delta() {
        let p = line();
        let d = p.delta(1.0, 1.1);
        let rider = [3.0, 1.0, 0.5];
        let moved = carry(rider, d);
        // rider keeps the same offset from the platform: it shifted by exactly d
        assert!((moved[0] - (rider[0] + d[0])).abs() < 1e-6);
        assert!((moved[1] - rider[1]).abs() < 1e-6, "no vertical drift on a flat run");
    }

    #[test]
    fn pingpong_delta_is_continuous_across_the_turn() {
        // No seam jump at the far end: the step straddling t=5 stays small.
        let mut p = line();
        p.mode = LoopMode::PingPong;
        let d = p.delta(4.95, 5.05);
        assert!(len3(d) < 0.5, "pingpong turn must not jump, got {}", len3(d));
    }

    #[test]
    fn position_stays_on_the_polyline() {
        // An L-shaped path: the midpoint of travel must lie on a segment.
        let p = Platform::new(vec![[0.0, 0.0, 0.0], [0.0, 0.0, 4.0], [3.0, 0.0, 4.0]], 1.0, LoopMode::Once);
        assert!((p.path_length() - 7.0).abs() < 1e-4);
        let at5 = p.pos_at(5.0); // 5 along: 4 up Z, then 1 along X
        assert!((at5[2] - 4.0).abs() < 1e-4 && (at5[0] - 1.0).abs() < 1e-4, "got {at5:?}");
    }

    #[test]
    fn degenerate_and_still_platforms_do_not_move() {
        let single = Platform::new(vec![[2.0, 3.0, 4.0]], 5.0, LoopMode::Loop);
        assert_eq!(single.pos_at(9.0), [2.0, 3.0, 4.0]);
        assert_eq!(single.delta(0.0, 1.0), [0.0, 0.0, 0.0]);
        let mut still = line();
        still.speed = 0.0;
        assert_eq!(still.pos_at(3.0), [0.0, 0.0, 0.0]);
    }

    #[test]
    fn on_top_contact_test() {
        assert!(on_top(4.02, 4.0, 0.05));
        assert!(!on_top(4.5, 4.0, 0.05), "floating well above ⇒ not on the platform");
        assert!(on_top(3.98, 4.0, 0.05), "slightly sunk within tolerance still counts");
    }

    #[test]
    fn evaluation_is_deterministic() {
        let mut p = line();
        p.mode = LoopMode::PingPong;
        for &t in &[0.0_f32, 1.3, 4.9, 5.1, 12.7] {
            assert_eq!(p.pos_at(t), p.pos_at(t));
        }
    }
}
