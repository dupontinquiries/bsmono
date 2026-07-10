//! ring.rs — a shrinking **safe zone** (the storm/dome), shared across modes.
//!
//! A circle centred somewhere on the map whose radius contracts from `r0` to `r1`
//! over `shrink` seconds. Outside the ring you take damage per second; this is the
//! battle-royale pressure the Prowl mode uses to force the action together. Kept
//! deterministic and free of any renderer so both the sim and the app share it.

#[derive(Clone, Copy)]
pub struct Ring {
    pub cx: f32, pub cz: f32,
    pub r0: f32, pub r1: f32,
    pub shrink: f32,     // seconds to go from r0 to r1
    pub dps: f32,        // damage per second while outside
    pub t: f32,          // elapsed time
}

impl Ring {
    pub fn new(cx: f32, cz: f32, r0: f32, r1: f32, shrink: f32, dps: f32) -> Self {
        Ring { cx, cz, r0, r1: r1.min(r0), shrink: shrink.max(0.001), dps, t: 0.0 }
    }

    pub fn tick(&mut self, dt: f32) { self.t += dt; }

    /// Current radius, easing linearly from r0 to r1 and then holding at r1.
    pub fn radius(&self) -> f32 {
        let k = (self.t / self.shrink).clamp(0.0, 1.0);
        self.r0 + (self.r1 - self.r0) * k
    }

    /// Distance a point sits *outside* the ring (0 if safely inside).
    pub fn outside_by(&self, x: f32, z: f32) -> f32 {
        let d = ((x - self.cx).powi(2) + (z - self.cz).powi(2)).sqrt();
        (d - self.radius()).max(0.0)
    }

    pub fn contains(&self, x: f32, z: f32) -> bool { self.outside_by(x, z) <= 0.0 }

    /// Damage a point should take this frame for being outside the ring.
    pub fn damage(&self, x: f32, z: f32, dt: f32) -> f32 {
        if self.outside_by(x, z) > 0.0 { self.dps * dt } else { 0.0 }
    }

    /// Unit vector pointing back toward safety (the centre), for AI to flee the storm.
    pub fn toward_safety(&self, x: f32, z: f32) -> (f32, f32) {
        let (dx, dz) = (self.cx - x, self.cz - z);
        let l = (dx * dx + dz * dz).sqrt();
        if l > 1e-5 { (dx / l, dz / l) } else { (0.0, 0.0) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn radius_shrinks_then_holds() {
        let mut r = Ring::new(0.0, 0.0, 100.0, 20.0, 10.0, 5.0);
        assert!((r.radius() - 100.0).abs() < 1e-4);
        for _ in 0..(5 * 30) { r.tick(1.0 / 30.0); }
        let mid = r.radius();
        assert!(mid > 20.0 && mid < 100.0, "radius should be mid-shrink, got {mid}");
        for _ in 0..(10 * 30) { r.tick(1.0 / 30.0); }
        assert!((r.radius() - 20.0).abs() < 1e-3, "radius should settle at r1");
    }

    #[test]
    fn outside_takes_damage_inside_is_safe() {
        let r = Ring::new(0.0, 0.0, 50.0, 10.0, 10.0, 6.0);
        assert!(r.contains(10.0, 0.0));
        assert_eq!(r.damage(10.0, 0.0, 0.1), 0.0);
        assert!(!r.contains(80.0, 0.0));
        assert!(r.damage(80.0, 0.0, 0.1) > 0.0);
        // safety points back toward the centre
        let (sx, _sz) = r.toward_safety(80.0, 0.0);
        assert!(sx < 0.0, "from +x outside, safety is toward -x");
    }
}
