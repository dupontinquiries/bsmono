//! `sim::camera_collision` — third-person boom pull-in + near-camera occlusion
//! fade. Applied AFTER `sim::camera_follow` (which places the desired eye) and
//! composes with `sim::camera_tremble`.
//!
//! When a wall sits between the player and the camera, pull the camera in along
//! the boom so it doesn't clip through; report how much to fade geometry that
//! ends up very close to the lens. Smoothing is **asymmetric** — snap in fast
//! (don't pop through walls) but ease out slowly (no jitter when the wall
//! clears). The bin raycasts target→desired-eye and passes the hit distance;
//! this module is the pure math + smoothing.
//!
//! Pure and deterministic: data in, data out. No GPU, IO, clock, or RNG; no
//! `.unwrap()`/`.expect()`. Inputs sanitised; never panics.

/// Tunable boom-collision parameters.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CollisionParams {
    /// Closest the camera may ever sit to the target (metres).
    pub min_distance: f32,
    /// Gap kept in front of a blocker so the lens doesn't touch it.
    pub pad: f32,
    /// Smoothing rate when pulling IN toward a wall (fast).
    pub pull_in_speed: f32,
    /// Smoothing rate when pushing OUT to the desired boom (slow).
    pub push_out_speed: f32,
    /// Boom distance at/above which near-camera fade is 0; fade reaches 1 at
    /// `min_distance`.
    pub fade_start: f32,
}

impl Default for CollisionParams {
    fn default() -> Self {
        CollisionParams {
            min_distance: 0.4,
            pad: 0.3,
            pull_in_speed: 30.0, // snap in
            push_out_speed: 4.0, // ease out
            fade_start: 1.2,
        }
    }
}

/// Persisted smoothing state — the current (smoothed) boom distance.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CollisionState {
    pub distance: f32,
}

impl CollisionState {
    /// Start at the desired boom distance (floored at 0).
    pub fn new(desired_distance: f32) -> CollisionState {
        CollisionState { distance: san(desired_distance).max(0.0) }
    }

    /// Resolve the boom for this frame. See module docs for the formula.
    pub fn resolve(
        &mut self,
        target: [f32; 3],
        desired_eye: [f32; 3],
        blocker_dist: Option<f32>,
        dt: f32,
        p: &CollisionParams,
    ) -> Collision {
        let tgt = [san(target[0]), san(target[1]), san(target[2])];
        let de = [san(desired_eye[0]), san(desired_eye[1]), san(desired_eye[2])];
        let min_d = san(p.min_distance).max(0.0);
        let pad = san(p.pad).max(0.0);

        // desired boom length + unit direction target -> desired eye
        let dx = de[0] - tgt[0];
        let dy = de[1] - tgt[1];
        let dz = de[2] - tgt[2];
        let desired = (dx * dx + dy * dy + dz * dz).sqrt();
        let dir = if desired > 1e-6 {
            [dx / desired, dy / desired, dz / desired]
        } else {
            [0.0, 0.0, 0.0]
        };

        // allowed boom: pull to (blocker - pad) when blocked, never past the
        // desired boom, never below min_distance.
        let allowed_raw = match blocker_dist {
            Some(b) if b.is_finite() => san(b) - pad,
            _ => desired,
        };
        let allowed = allowed_raw.min(desired).max(min_d);

        // advance the smoothed distance toward `allowed` (unless dt invalid).
        if dt.is_finite() && dt > 0.0 {
            let cur = san(self.distance);
            // pulling in (target closer) is fast; pushing out is slow.
            let speed = if allowed < cur {
                san(p.pull_in_speed).max(0.0)
            } else {
                san(p.push_out_speed).max(0.0)
            };
            // frame-rate-independent exponential approach; factor in [0,1] so it
            // never overshoots.
            let factor = 1.0 - (-(speed * dt)).exp();
            let next = cur + (allowed - cur) * factor;
            self.distance = if next.is_finite() { next } else { allowed };
        }

        let distance = san(self.distance);
        let eye = [
            tgt[0] + dir[0] * distance,
            tgt[1] + dir[1] * distance,
            tgt[2] + dir[2] * distance,
        ];
        let fade01 = fade(distance, p.fade_start, min_d);
        Collision { eye, distance, fade01 }
    }
}

/// Result of one resolve: where to put the eye, the boom distance, and how much
/// to fade near-camera geometry (`0` = opaque, `1` = fully faded).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Collision {
    pub eye: [f32; 3],
    pub distance: f32,
    pub fade01: f32,
}

#[inline]
fn san(x: f32) -> f32 {
    if x.is_finite() { x } else { 0.0 }
}

/// Near-camera fade: `0` at/above `fade_start`, rising to `1` at `min_distance`.
fn fade(distance: f32, fade_start: f32, min_distance: f32) -> f32 {
    let fs = san(fade_start);
    let md = san(min_distance);
    let denom = fs - md;
    if denom <= 1e-6 {
        // degenerate band: opaque unless at/below the start.
        return if distance <= fs { 1.0 } else { 0.0 };
    }
    ((fs - distance) / denom).max(0.0).min(1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dist(a: [f32; 3], b: [f32; 3]) -> f32 {
        let d = [a[0] - b[0], a[1] - b[1], a[2] - b[2]];
        (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt()
    }
    // target at origin, desired eye 5 m down +x
    const TGT: [f32; 3] = [0.0, 0.0, 0.0];
    const EYE: [f32; 3] = [5.0, 0.0, 0.0];

    #[test]
    fn no_blocker_eases_to_desired_no_overshoot() {
        let p = CollisionParams::default();
        let mut s = CollisionState::new(1.0); // start pulled in
        let mut prev = s.distance;
        for _ in 0..600 {
            let c = s.resolve(TGT, EYE, None, 1.0 / 60.0, &p);
            assert!(c.distance >= prev - 1e-6, "should not move backward easing out");
            assert!(c.distance <= 5.0 + 1e-4, "overshot desired boom: {}", c.distance);
            prev = c.distance;
        }
        assert!((prev - 5.0).abs() < 1e-2, "did not reach desired: {}", prev);
    }

    #[test]
    fn near_blocker_clamps_to_blocker_minus_pad_and_min() {
        let p = CollisionParams::default();
        // wall at 2.0 -> allowed 2.0 - pad(0.3) = 1.7
        let mut s = CollisionState::new(5.0);
        for _ in 0..600 {
            s.resolve(TGT, EYE, Some(2.0), 1.0 / 60.0, &p);
        }
        assert!((s.distance - 1.7).abs() < 1e-2, "not at blocker-pad: {}", s.distance);

        // wall so close that blocker-pad < min_distance -> clamp to min_distance
        let mut s2 = CollisionState::new(5.0);
        for _ in 0..600 {
            s2.resolve(TGT, EYE, Some(0.5), 1.0 / 60.0, &p);
        }
        assert!((s2.distance - p.min_distance).abs() < 1e-2, "not clamped to min: {}", s2.distance);
        assert!(s2.distance >= p.min_distance - 1e-6);
    }

    #[test]
    fn pull_in_is_faster_than_push_out() {
        let p = CollisionParams::default();
        let dt = 1.0 / 60.0;
        // pull IN: start at 5, target allowed 2.0 (blocker 2.3, pad .3) -> gap 3.0
        let mut sin = CollisionState::new(5.0);
        sin.resolve(TGT, EYE, Some(2.3), dt, &p);
        let pulled = 5.0 - sin.distance; // distance moved inward in one step

        // push OUT: start at 2.0, no blocker, desired 5 -> gap 3.0
        let mut sout = CollisionState::new(2.0);
        sout.resolve(TGT, EYE, None, dt, &p);
        let pushed = sout.distance - 2.0; // distance moved outward in one step

        assert!(pulled > pushed, "pull-in {} not faster than push-out {}", pulled, pushed);
    }

    #[test]
    fn eye_lies_on_target_to_desired_ray() {
        let p = CollisionParams::default();
        let mut s = CollisionState::new(3.0);
        let de = [3.0, 4.0, 0.0]; // |de| = 5, dir = (0.6,0.8,0)
        let c = s.resolve(TGT, de, Some(2.0), 1.0 / 60.0, &p);
        // eye = dir * distance ; check colinear with de and |eye| == distance
        assert!((dist(c.eye, TGT) - c.distance).abs() < 1e-4, "|eye| != distance");
        // cross product of eye and de ~ 0 (colinear)
        let cross = [
            c.eye[1] * de[2] - c.eye[2] * de[1],
            c.eye[2] * de[0] - c.eye[0] * de[2],
            c.eye[0] * de[1] - c.eye[1] * de[0],
        ];
        assert!(cross.iter().all(|v| v.abs() < 1e-3), "eye not on ray: {:?}", cross);
    }

    #[test]
    fn fade_endpoints_and_middle() {
        let p = CollisionParams::default(); // fade_start 1.2, min 0.4
        let far = { let mut s = CollisionState { distance: 2.0 }; s.resolve(TGT, EYE, None, 0.0, &p).fade01 };
        assert_eq!(far, 0.0, "fade not 0 above fade_start");
        let at_start = fade(p.fade_start, p.fade_start, p.min_distance);
        assert_eq!(at_start, 0.0);
        let at_min = fade(p.min_distance, p.fade_start, p.min_distance);
        assert!((at_min - 1.0).abs() < 1e-6, "fade not ~1 at min");
        let mid = fade(0.8, p.fade_start, p.min_distance);
        assert!(mid > 0.0 && mid < 1.0, "mid fade {}", mid);
        // degenerate band guarded
        let deg = fade(0.5, 0.4, 0.4);
        assert!(deg.is_finite() && (0.0..=1.0).contains(&deg));
    }

    #[test]
    fn dt_nonpositive_unchanged_and_finite() {
        let p = CollisionParams::default();
        let mut s = CollisionState::new(3.0);
        for &dt in &[0.0f32, -1.0, f32::NAN, f32::INFINITY] {
            let before = s.distance;
            let c = s.resolve(TGT, EYE, Some(1.0), dt, &p);
            assert_eq!(s.distance, before, "distance changed on dt={}", dt);
            assert!(c.eye.iter().all(|v| v.is_finite()) && c.distance.is_finite() && c.fade01.is_finite());
        }
    }

    #[test]
    fn degenerate_inputs_no_panic() {
        let p = CollisionParams::default();
        let mut s = CollisionState::new(0.0);
        // desired eye == target (zero boom) -> dir 0, eye == target, finite
        let c = s.resolve(TGT, TGT, None, 1.0 / 60.0, &p);
        assert!(c.eye.iter().all(|v| v.is_finite()) && c.fade01.is_finite());
        // non-finite inputs sanitised
        let mut s2 = CollisionState::new(3.0);
        let c2 = s2.resolve([f32::NAN; 3], [f32::INFINITY, 0.0, 0.0], Some(f32::NAN), 0.016, &p);
        assert!(c2.eye.iter().all(|v| v.is_finite()) && c2.distance.is_finite());
    }

    #[test]
    fn deterministic_and_seeded_sweep() {
        let p = CollisionParams::default();
        let mut a = CollisionState::new(4.0);
        let mut b = CollisionState::new(4.0);
        let ca = a.resolve(TGT, EYE, Some(2.5), 0.016, &p);
        let cb = b.resolve(TGT, EYE, Some(2.5), 0.016, &p);
        assert_eq!(ca, cb);

        // deterministic seeded sweep (R20): always finite, distance >= min, eye on ray
        let mut x: u64 = 0xCA3E_0057_u64;
        let mut next = || { x = x.wrapping_mul(6364136223846793005).wrapping_add(1); (x >> 33) as u32 };
        let span = |v: u32, lo: f32, hi: f32| lo + (v % 1000) as f32 / 1000.0 * (hi - lo);
        let mut s = CollisionState::new(span(next(), 0.5, 8.0));
        for _ in 0..5000 {
            let de = [span(next(), -8.0, 8.0), span(next(), -8.0, 8.0), span(next(), -8.0, 8.0)];
            let blk = if next() % 2 == 0 { Some(span(next(), 0.0, 10.0)) } else { None };
            let dt = span(next(), 0.0, 0.1);
            let c = s.resolve([0.0; 3], de, blk, dt, &p);
            assert!(c.distance.is_finite() && c.distance >= p.min_distance - 1e-4);
            assert!(c.eye.iter().all(|v| v.is_finite()));
            assert!((0.0..=1.0).contains(&c.fade01));
        }
    }
}
