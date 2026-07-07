//! 3D flight — steering behaviors + flight dynamics for flying creatures.
//!
//! Groundwork for eventual dragons (and reusable for any flier — birds, and
//! the planned fish move the same way in water). Pure vector math, no world
//! or entity dependency: the steering functions produce a desired force, and
//! [`Flier::step`] integrates it under three limits — acceleration
//! (`max_force`), turning (`max_turn_force`, the centripetal cap that gives a
//! big flier its wide banking arcs), and top speed (`max_speed`). The
//! creature/AI layer picks a behavior each tick, sums the forces, and steps.
//!
//! Behaviors follow the standard Reynolds steering model: each returns
//! `desired_velocity - current_velocity`, which the integrator then clamps.
//!
//! ## A note on [`orbit`]
//!
//! Orbit settles into a *stable* circle at the requested altitude, but the
//! equilibrium radius runs a little wider than asked when the creature is
//! fast relative to its turn limit — the same reason a fast aircraft has a
//! wide turn radius. Tighten it by lowering the orbit `cruise` speed or
//! raising `max_turn_force`. This is intended: a lazily-circling dragon and a
//! tight-banking one fall out of the same model.

pub type Vec3 = [f64; 3];

#[inline]
pub fn v_add(a: Vec3, b: Vec3) -> Vec3 {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}
#[inline]
pub fn v_sub(a: Vec3, b: Vec3) -> Vec3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
#[inline]
pub fn v_scale(a: Vec3, s: f64) -> Vec3 {
    [a[0] * s, a[1] * s, a[2] * s]
}
#[inline]
pub fn v_dot(a: Vec3, b: Vec3) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
#[inline]
pub fn v_len(a: Vec3) -> f64 {
    v_dot(a, a).sqrt()
}
#[inline]
pub fn v_norm(a: Vec3) -> Vec3 {
    let l = v_len(a);
    if l > 1e-12 {
        v_scale(a, 1.0 / l)
    } else {
        [0.0, 0.0, 0.0]
    }
}
#[inline]
pub fn v_cross(a: Vec3, b: Vec3) -> Vec3 {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}
/// Clamp a vector's magnitude to at most `m`.
#[inline]
pub fn v_clamp_len(a: Vec3, m: f64) -> Vec3 {
    let l = v_len(a);
    if l > m && l > 1e-12 {
        v_scale(a, m / l)
    } else {
        a
    }
}

/// Flight limits for a creature. `max_turn_force` is the centripetal
/// (lateral) acceleration cap: smaller → wider turns / larger minimum turn
/// radius (`r_min ≈ speed² / max_turn_force`).
#[derive(Debug, Clone, Copy)]
pub struct FlightLimits {
    pub max_speed: f64,
    pub max_force: f64,
    pub max_turn_force: f64,
}

impl Default for FlightLimits {
    fn default() -> Self {
        Self { max_speed: 8.0, max_force: 4.0, max_turn_force: 5.0 }
    }
}

/// A flying body: position and velocity. Steered by applying forces via
/// [`Flier::step`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Flier {
    pub pos: Vec3,
    pub vel: Vec3,
}

impl Flier {
    pub fn new(pos: Vec3) -> Self {
        Self { pos, vel: [0.0, 0.0, 0.0] }
    }

    /// Integrate one step under `force`, applying (in order) the turn-force
    /// cap on the component perpendicular to current velocity, the overall
    /// acceleration cap, then the top-speed cap. Semi-implicit Euler.
    pub fn step(&mut self, force: Vec3, limits: &FlightLimits, dt: f64) {
        let mut f = force;
        let speed = v_len(self.vel);
        if speed > 1e-6 {
            // Split into thrust (along velocity) and turn (perpendicular);
            // cap the turning part to bound how sharply we can bank.
            let dir = v_scale(self.vel, 1.0 / speed);
            let parallel = v_scale(dir, v_dot(f, dir));
            let lateral = v_clamp_len(v_sub(f, parallel), limits.max_turn_force);
            f = v_add(parallel, lateral);
        }
        f = v_clamp_len(f, limits.max_force);
        let mut v = v_add(self.vel, v_scale(f, dt));
        let s = v_len(v);
        if s > limits.max_speed {
            v = v_scale(v, limits.max_speed / s);
        }
        self.vel = v;
        self.pos = v_add(self.pos, v_scale(v, dt));
    }
}

/// Steer toward `target` at full speed (a dive / pursuit). Returns a steering
/// force (`desired_velocity - velocity`).
pub fn seek(pos: Vec3, vel: Vec3, target: Vec3, max_speed: f64) -> Vec3 {
    let desired = v_scale(v_norm(v_sub(target, pos)), max_speed);
    v_sub(desired, vel)
}

/// Steer directly away from `threat` at full speed (retreat).
pub fn flee(pos: Vec3, vel: Vec3, threat: Vec3, max_speed: f64) -> Vec3 {
    let desired = v_scale(v_norm(v_sub(pos, threat)), max_speed);
    v_sub(desired, vel)
}

/// Seek `target` but decelerate inside `slow_radius` so the creature settles
/// onto it instead of overshooting (approach / perch / land).
pub fn arrive(pos: Vec3, vel: Vec3, target: Vec3, max_speed: f64, slow_radius: f64) -> Vec3 {
    let off = v_sub(target, pos);
    let d = v_len(off);
    if d < 1e-6 {
        return v_scale(vel, -1.0); // on target → brake
    }
    let ramped = if d < slow_radius && slow_radius > 1e-6 {
        max_speed * d / slow_radius
    } else {
        max_speed
    };
    let desired = v_scale(off, ramped / d);
    v_sub(desired, vel)
}

/// Circle `center` at `radius` (in the horizontal plane) at `cruise` speed,
/// `clockwise` seen from above. The desired velocity is tangential plus a
/// proportional radial correction toward the target radius. See the module
/// note about the emergent equilibrium radius.
pub fn orbit(
    pos: Vec3,
    vel: Vec3,
    center: Vec3,
    radius: f64,
    cruise: f64,
    clockwise: bool,
) -> Vec3 {
    let radial = [pos[0] - center[0], 0.0, pos[2] - center[2]];
    let dist = v_len(radial);
    let rdir = if dist > 1e-6 {
        v_scale(radial, 1.0 / dist)
    } else {
        [1.0, 0.0, 0.0]
    };
    let mut tangent = v_cross([0.0, 1.0, 0.0], rdir);
    if clockwise {
        tangent = v_scale(tangent, -1.0);
    }
    // Radial correction: move inward when too far, outward when too close.
    let radial_vel = (-(dist - radius) * 0.5).clamp(-cruise, cruise);
    let desired = v_add(v_scale(tangent, cruise), v_scale(rdir, radial_vel));
    v_sub(desired, vel)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run<F: Fn(&Flier) -> Vec3>(start: Vec3, steps: usize, lim: &FlightLimits, force: F) -> Flier {
        let mut f = Flier::new(start);
        for _ in 0..steps {
            let s = force(&f);
            f.step(s, lim, 0.1);
        }
        f
    }

    #[test]
    fn seek_closes_distance_and_respects_speed() {
        let lim = FlightLimits::default();
        let target = [20.0, 5.0, 0.0];
        let start = [0.0, 0.0, 0.0];
        let d0 = v_len(v_sub(target, start));
        let mut f = Flier::new(start);
        let mut max_speed_seen = 0.0f64;
        for _ in 0..400 {
            let s = seek(f.pos, f.vel, target, lim.max_speed);
            f.step(s, &lim, 0.1);
            max_speed_seen = max_speed_seen.max(v_len(f.vel));
        }
        let d1 = v_len(v_sub(target, f.pos));
        assert!(d1 < d0 * 0.3, "seek should close most of the gap: {} -> {}", d0, d1);
        assert!(max_speed_seen <= lim.max_speed + 1e-9, "speed cap held: {}", max_speed_seen);
    }

    #[test]
    fn arrive_settles_on_target() {
        let lim = FlightLimits::default();
        let target = [20.0, 5.0, 0.0];
        let f = run([0.0, 0.0, 0.0], 600, &lim, |f| arrive(f.pos, f.vel, target, lim.max_speed, 6.0));
        assert!(v_len(v_sub(target, f.pos)) < 0.5, "should arrive near target");
        assert!(v_len(f.vel) < 0.5, "should decelerate to rest");
    }

    #[test]
    fn flee_opens_distance() {
        let lim = FlightLimits::default();
        let threat = [1.0, 0.0, 0.0];
        let start = [0.0, 0.0, 0.0];
        let d0 = v_len(v_sub(start, threat));
        let f = run(start, 50, &lim, |f| flee(f.pos, f.vel, threat, lim.max_speed));
        assert!(v_len(v_sub(f.pos, threat)) > d0 + 5.0, "flee should open distance");
    }

    #[test]
    fn orbit_circles_at_stable_radius_and_altitude() {
        let lim = FlightLimits::default();
        let center = [0.0, 10.0, 0.0];
        let radius = 10.0;
        let mut f = Flier::new([18.0, 10.0, 0.0]);
        let mut tail: Vec<f64> = Vec::new();
        let mut swept = 0.0f64;
        let mut prev = (f.pos[2] - center[2]).atan2(f.pos[0] - center[0]);
        for i in 0..3000 {
            let s = orbit(f.pos, f.vel, center, radius, 5.0, false);
            f.step(s, &lim, 0.1);
            let a = (f.pos[2] - center[2]).atan2(f.pos[0] - center[0]);
            let mut d = a - prev;
            if d > std::f64::consts::PI { d -= std::f64::consts::TAU; }
            if d < -std::f64::consts::PI { d += std::f64::consts::TAU; }
            swept += d;
            prev = a;
            if i >= 2500 {
                tail.push((f.pos[0] - center[0]).hypot(f.pos[2] - center[2]));
            }
        }
        let mean = tail.iter().sum::<f64>() / tail.len() as f64;
        let var = tail.iter().map(|r| (r - mean).powi(2)).sum::<f64>() / tail.len() as f64;
        assert!(var.sqrt() < 0.5, "radius should stabilize, stdev {}", var.sqrt());
        assert!(mean > 0.5 * radius && mean < 2.0 * radius, "radius bounded near target: {}", mean);
        assert!((f.pos[1] - center[1]).abs() < 0.5, "altitude held: {}", f.pos[1]);
        assert!(swept.abs() > std::f64::consts::TAU, "should complete at least one lap: {}", swept);
    }

    #[test]
    fn turn_force_cap_limits_lateral_change() {
        // A flier cruising +x, asked to turn hard +z: lateral velocity gained
        // in one step can't exceed max_turn_force * dt.
        let lim = FlightLimits { max_speed: 10.0, max_force: 100.0, max_turn_force: 3.0 };
        let mut f = Flier { pos: [0.0, 0.0, 0.0], vel: [8.0, 0.0, 0.0] };
        f.step([0.0, 0.0, 100.0], &lim, 0.1); // huge sideways force
        assert!(f.vel[2] <= 3.0 * 0.1 + 1e-9, "lateral gain capped: {}", f.vel[2]);
    }
}
