//! swim — 3D aquatic locomotion. A swimmer is pushed by its own thrust along a
//! 3D heading (yaw + pitch), floated by buoyancy, and slowed by quadratic drag.
//! Integrate `SwimState` per tick to get motion that glides, coasts to a terminal
//! speed, and rises/sinks only when buoyancy is off-balance. Pure, std-only,
//! deterministic (tick `dt` only).

use std::f32::consts::FRAC_PI_2;

/// Tuning for the water medium and the body.
#[derive(Clone, Copy, Debug)]
pub struct SwimParams {
    /// Net vertical acceleration from buoyancy − gravity (blocks/s²): >0 floats,
    /// <0 sinks, 0 = neutrally buoyant.
    pub buoyancy: f32,
    /// Quadratic drag coefficient (per (blocks/s), bounds terminal speed).
    pub drag: f32,
    /// Thrust acceleration at full throttle (blocks/s²).
    pub thrust: f32,
    /// Pitch clamp magnitude (radians); keeps the nose within ±this of level.
    pub max_pitch: f32,
}

impl Default for SwimParams {
    fn default() -> SwimParams {
        SwimParams { buoyancy: 0.0, drag: 0.4, thrust: 6.0, max_pitch: FRAC_PI_2 * 0.9 }
    }
}

/// A swimmer's kinematic state.
#[derive(Clone, Copy, Debug)]
pub struct SwimState {
    pub pos: [f32; 3],
    pub vel: [f32; 3],
    pub yaw: f32,
    pub pitch: f32,
}

fn heading(yaw: f32, pitch: f32) -> [f32; 3] {
    let cp = pitch.cos();
    [yaw.cos() * cp, pitch.sin(), yaw.sin() * cp]
}

impl SwimState {
    pub fn new(pos: [f32; 3]) -> SwimState {
        SwimState { pos, vel: [0.0; 3], yaw: 0.0, pitch: 0.0 }
    }

    pub fn speed(&self) -> f32 {
        (self.vel[0] * self.vel[0] + self.vel[1] * self.vel[1] + self.vel[2] * self.vel[2]).sqrt()
    }

    /// Advance one tick. `throttle` in `[0, 1]` drives thrust along the (clamped)
    /// heading; buoyancy acts on +Y; quadratic drag opposes velocity. `dt` seconds.
    pub fn step(&mut self, throttle: f32, dt: f32, p: &SwimParams) {
        let dt = dt.max(0.0);
        self.pitch = self.pitch.clamp(-p.max_pitch, p.max_pitch);
        let dir = heading(self.yaw, self.pitch);
        let thr = throttle.clamp(0.0, 1.0) * p.thrust;

        let spd = self.speed();
        // quadratic drag: a = -drag * |v| * v  (opposes motion, ~v² magnitude)
        let drag_a = [
            -p.drag * spd * self.vel[0],
            -p.drag * spd * self.vel[1],
            -p.drag * spd * self.vel[2],
        ];
        let accel = [
            dir[0] * thr + drag_a[0],
            dir[1] * thr + drag_a[1] + p.buoyancy,
            dir[2] * thr + drag_a[2],
        ];
        for i in 0..3 {
            self.vel[i] += accel[i] * dt;
            self.pos[i] += self.vel[i] * dt;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lcg(seed: &mut u32) -> f32 {
        *seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
        ((*seed >> 8) as f32) / ((1u32 << 24) as f32)
    }

    #[test]
    fn neutral_buoyancy_means_no_vertical_drift() {
        let p = SwimParams { buoyancy: 0.0, ..SwimParams::default() };
        let mut s = SwimState::new([0.0, 10.0, 0.0]); // level, no throttle
        for _ in 0..600 {
            s.step(0.0, 1.0 / 60.0, &p);
        }
        assert!((s.pos[1] - 10.0).abs() < 1e-3, "stays at depth when neutral: {}", s.pos[1]);
    }

    #[test]
    fn drag_bounds_terminal_speed() {
        let p = SwimParams { buoyancy: 0.0, drag: 0.4, thrust: 6.0, ..SwimParams::default() };
        let mut s = SwimState::new([0.0; 3]);
        for _ in 0..2000 {
            s.step(1.0, 1.0 / 60.0, &p);
        }
        // terminal: drag*v² = thrust → v = sqrt(thrust/drag) = sqrt(15) ≈ 3.87
        let terminal = (p.thrust / p.drag).sqrt();
        assert!(s.speed() <= terminal + 0.2, "speed near terminal {terminal}, got {}", s.speed());
    }

    #[test]
    fn positive_buoyancy_rises_negative_sinks() {
        let mut up = SwimState::new([0.0; 3]);
        let mut down = SwimState::new([0.0; 3]);
        let pu = SwimParams { buoyancy: 1.0, ..SwimParams::default() };
        let pd = SwimParams { buoyancy: -1.0, ..SwimParams::default() };
        for _ in 0..120 {
            up.step(0.0, 1.0 / 60.0, &pu);
            down.step(0.0, 1.0 / 60.0, &pd);
        }
        assert!(up.pos[1] > 0.0 && down.pos[1] < 0.0, "rise vs sink");
    }

    #[test]
    fn prop_stays_finite_and_pitch_clamped() {
        let p = SwimParams::default();
        let mut seed = 0x5EA0_F00Du32;
        for _ in 0..2000 {
            let mut s = SwimState::new([lcg(&mut seed) * 10.0, lcg(&mut seed) * 10.0, lcg(&mut seed) * 10.0]);
            s.yaw = lcg(&mut seed) * 12.0 - 6.0;
            s.pitch = lcg(&mut seed) * 12.0 - 6.0; // wild pitch — must clamp
            for _ in 0..30 {
                s.step(lcg(&mut seed), 1.0 / 60.0, &p);
                assert!(s.pos[0].is_finite() && s.vel[0].is_finite(), "finite");
                assert!(s.pitch.abs() <= p.max_pitch + 1e-6, "pitch clamped: {}", s.pitch);
            }
        }
    }
}

// ── S1 (r94): register as a sim::service_registry service (co-located) ───────
// Adapter ticks the swimmer with NEUTRAL zero throttle and default SwimParams;
// real throttle/heading control is a later wiring pass.
struct SwimService(SwimState, SwimParams);
impl crate::sim::service_registry::Service for SwimService {
    fn name(&self) -> &'static str { "swim" }
    fn tick(&mut self, ctx: &crate::sim::service_registry::ServiceCtx) {
        self.0.step(0.0, ctx.dt, &self.1);
    }
}
#[linkme::distributed_slice(crate::sim::service_registry::SERVICES)]
static SWIM_SERVICE: crate::sim::service_registry::ServiceEntry =
    crate::sim::service_registry::ServiceEntry {
        name: "swim",
        make: || Box::new(SwimService(SwimState::new([0.0, 0.0, 0.0]), SwimParams::default())),
    };

#[cfg(test)]
mod s1_wiring_tests {
    use crate::sim::service_registry::{Service, ServiceCtx};
    #[test]
    fn swim_service_ticks_panic_free() {
        let mut s = super::SwimService(super::SwimState::new([0.0, 0.0, 0.0]), super::SwimParams::default());
        let ctx = ServiceCtx { dt: 1.0 / 60.0 };
        for _ in 0..240 { s.tick(&ctx); }
        assert_eq!(s.name(), "swim");
    }
}
