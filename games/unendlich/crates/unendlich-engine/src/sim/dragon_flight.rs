//! `sim::dragon_flight` — flight dynamics for a dragon, composing the existing
//! aero primitives (`lift_force::lift`, `glide_ratio`) into a per-tick state:
//! lift vs weight, glide ratio, stall, and the climb/glide/level mode. Wing area
//! and mass come from the genome. Pairs with `stamina` (flapping drains it) and
//! `movement_tune` (the vertical accel feeds the integrator). Pure, deterministic.

use crate::sim::dragon_archetype::Genome;
use crate::sim::glide_ratio::glide_ratio;
use crate::sim::lift_force::lift;

pub const GRAVITY: f32 = 9.81;

#[derive(Clone, Copy, Debug)]
pub struct FlightParams {
    pub wing_area: f32,
    pub mass: f32,
    pub lift_coeff: f32,
    pub drag_coeff: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FlightMode { Stalled, Gliding, Level, Climbing }

#[derive(Clone, Copy, Debug)]
pub struct FlightStep {
    pub lift: f32,
    pub drag: f32,
    pub glide_ratio: f32,
    pub vertical_accel: f32,
    pub mode: FlightMode,
}

/// Total lifting wing area (m², both wings) from the genome: span × chord × size.
pub fn wing_area(g: &Genome) -> f32 {
    let span = 0.5 + g.wing_span.clamp(0.0, 1.0) * 2.5; // 0.5..3.0
    let chord = 0.3 + g.wing_spread.clamp(0.0, 1.0) * 0.9; // 0.3..1.2
    let size = 0.7 + g.bulk.clamp(0.0, 1.0) * 0.9; // bigger dragon → bigger wing
    (span * chord * size * 2.0).max(0.5)
}

/// Flight parameters from a genome (mass mirrors `spawn::dragon_mass`).
pub fn params_from(g: &Genome) -> FlightParams {
    FlightParams {
        wing_area: wing_area(g),
        mass: 28.0 + g.bulk.clamp(0.0, 1.0) * 47.0,
        lift_coeff: 1.2,
        drag_coeff: 0.06,
    }
}

/// Minimum airspeed that supports the dragon's weight (lift == weight). Larger
/// wings ⇒ lower stall speed.
pub fn stall_speed(p: &FlightParams) -> f32 {
    let denom = (0.6125 * p.wing_area.max(0.0) * p.lift_coeff.max(0.0)).max(1e-3);
    (p.mass.max(0.0) * GRAVITY / denom).sqrt()
}

/// One flight tick at `airspeed` (m/s) with wing-beat effort `flap01` (0 = glide).
/// Below stall the wing sheds lift; flapping adds thrust-lift. Finite everywhere.
pub fn flight_step(p: &FlightParams, airspeed: f32, flap01: f32, _dt: f32) -> FlightStep {
    let v = airspeed.max(0.0);
    let flap = flap01.clamp(0.0, 1.0);
    let stall = stall_speed(p);

    let mut wing_lift = lift(v, p.wing_area, p.lift_coeff);
    if v < stall {
        // smooth lift collapse below stall speed
        let t = (v / stall.max(1e-3)).clamp(0.0, 1.0);
        wing_lift *= t * t;
    }
    let flap_lift = flap * p.wing_area.max(0.0) * 0.5 * GRAVITY * 6.0;
    let total_lift = wing_lift + flap_lift;

    let drag = (p.drag_coeff.max(0.0) * v * v) + 1.0; // parasitic + a floor to avoid div0
    let gr = glide_ratio(wing_lift, drag);
    let weight = p.mass.max(0.0) * GRAVITY;
    let vertical_accel = (total_lift - weight) / p.mass.max(1e-3);

    let mode = if v < stall && flap < 0.1 {
        FlightMode::Stalled
    } else if vertical_accel > 0.5 {
        FlightMode::Climbing
    } else if vertical_accel < -0.5 {
        FlightMode::Gliding
    } else {
        FlightMode::Level
    };

    FlightStep { lift: total_lift, drag, glide_ratio: gr, vertical_accel, mode }
}

/// Stamina drained by flapping this tick (feed to `stamina`/`mount_stamina`).
pub fn flap_stamina_cost(flap01: f32, dt: f32) -> f32 {
    flap01.clamp(0.0, 1.0) * 0.4 * dt.max(0.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params() -> FlightParams { params_from(&Genome::from_seed(3)) }

    #[test]
    fn lift_rises_with_airspeed_above_stall() {
        let p = params();
        let s = stall_speed(&p);
        let lo = flight_step(&p, s * 1.5, 0.0, 0.016).lift;
        let hi = flight_step(&p, s * 3.0, 0.0, 0.016).lift;
        assert!(hi > lo, "faster ⇒ more lift");
    }

    #[test]
    fn bigger_wings_lower_stall_speed() {
        let small = FlightParams { wing_area: 2.0, mass: 50.0, lift_coeff: 1.2, drag_coeff: 0.06 };
        let big = FlightParams { wing_area: 8.0, ..small };
        assert!(stall_speed(&big) < stall_speed(&small));
        // and a broader-winged genome has more area
        let mut narrow = Genome::from_seed(1); narrow.wing_span = 0.1; narrow.wing_spread = 0.1;
        let mut broad = Genome::from_seed(1); broad.wing_span = 1.0; broad.wing_spread = 1.0;
        assert!(wing_area(&broad) > wing_area(&narrow));
    }

    #[test]
    fn stalls_at_low_speed_climbs_when_fast() {
        let p = params();
        assert_eq!(flight_step(&p, 0.0, 0.0, 0.016).mode, FlightMode::Stalled);
        let fast = flight_step(&p, stall_speed(&p) * 3.0, 0.0, 0.016);
        assert!(matches!(fast.mode, FlightMode::Climbing | FlightMode::Level));
        assert!(fast.vertical_accel.is_finite());
    }

    #[test]
    fn flapping_adds_lift() {
        let p = params();
        let glide = flight_step(&p, 4.0, 0.0, 0.016).lift;
        let beat = flight_step(&p, 4.0, 1.0, 0.016).lift;
        assert!(beat > glide, "wing-beat adds lift");
    }

    #[test]
    fn glide_ratio_finite_and_positive() {
        let p = params();
        let gr = flight_step(&p, 12.0, 0.0, 0.016).glide_ratio;
        assert!(gr.is_finite() && gr > 0.0);
    }

    #[test]
    fn all_finite_and_deterministic() {
        let p = params();
        for v in [0.0, 1.0, 12.0, 1e4] {
            let s = flight_step(&p, v, 0.5, 0.016);
            assert!(s.lift.is_finite() && s.drag.is_finite() && s.vertical_accel.is_finite());
        }
        let a = flight_step(&p, 10.0, 0.3, 0.016);
        let b = flight_step(&p, 10.0, 0.3, 0.016);
        assert_eq!(a.vertical_accel, b.vertical_accel);
        assert!(flap_stamina_cost(1.0, 0.016) > flap_stamina_cost(0.0, 0.016));
    }
}
