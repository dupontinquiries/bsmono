//! `sim::breath_attack` — a dragon's breath weapon, composing `breath_cone`
//! (angular falloff), `breath_damage` (range falloff) and `breath_recharge`
//! (fuel) into one damage-at-a-target query plus fuel management. Element, reach
//! and damage come from the genome. Pure, deterministic, panic-free.

use crate::sim::breath_cone::intensity;
use crate::sim::breath_damage::damage;
use crate::sim::breath_recharge::recharge as recharge_primitive;
use crate::sim::dragon_archetype::{Genome, NUM_ELEMENTS};

#[derive(Clone, Copy, Debug)]
pub struct BreathParams {
    pub cone_half_angle: f32,
    pub max_range: f32,
    pub base_damage: f32,
    /// Dominant element index (0=Wood..5=Storm), per `dragon_archetype`.
    pub element: u8,
}

/// Dominant element channel of a genome.
pub fn dominant_element(g: &Genome) -> u8 {
    let mut idx = 0u8;
    let mut best = f32::MIN;
    for (i, &w) in g.element.iter().enumerate().take(NUM_ELEMENTS) {
        if w > best {
            best = w;
            idx = i as u8;
        }
    }
    idx
}

/// Extra punch by element: Fire hits hardest, Storm/Water next, others modest.
fn element_power(elem: u8) -> f32 {
    match elem {
        4 => 6.0, // Fire
        5 => 4.0, // Storm
        1 => 3.0, // Water (scald/pressure)
        _ => 2.0, // Wood / Air / Dust
    }
}

/// Breath parameters from a genome: wider gape ⇒ wider cone, bigger dragon ⇒
/// longer reach, more teeth + element ⇒ more damage.
pub fn params_from(g: &Genome) -> BreathParams {
    let elem = dominant_element(g);
    BreathParams {
        cone_half_angle: 0.25 + g.jaw_depth.clamp(0.0, 1.0) * 0.35, // ~0.25..0.60 rad
        max_range: 6.0 + (0.5 + g.bulk.clamp(0.0, 1.0)) * 14.0, // ~13..27
        base_damage: 8.0 + g.teeth.clamp(0.0, 1.0) * 10.0 + element_power(elem),
        element: elem,
    }
}

/// Damage delivered to a target at `angle_to_target` (rad off the breath axis)
/// and `distance`: angular cone falloff × range falloff. 0 outside the cone or
/// beyond range; greatest on-axis at point-blank.
pub fn damage_at(p: &BreathParams, angle_to_target: f32, distance: f32) -> f32 {
    intensity(angle_to_target, p.cone_half_angle) * damage(distance, p.max_range, p.base_damage)
}

/// Recharge breath fuel toward full (wraps `breath_recharge`).
pub fn recharge(fuel: f32, rate: f32, dt: f32) -> f32 {
    recharge_primitive(fuel, rate, dt)
}

/// Whether there's enough fuel to breathe.
pub fn can_breathe(fuel: f32, cost: f32) -> bool {
    cost >= 0.0 && fuel >= cost
}

/// Spend `cost` fuel if available; returns whether the breath fired.
pub fn try_breathe(fuel: &mut f32, cost: f32) -> bool {
    if can_breathe(*fuel, cost) {
        *fuel = (*fuel - cost).max(0.0);
        true
    } else {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p() -> BreathParams { params_from(&Genome::from_seed_element(2, 4)) } // Fire

    #[test]
    fn strongest_on_axis_point_blank() {
        let bp = p();
        let center = damage_at(&bp, 0.0, 0.0);
        assert!(center > 0.0);
        assert!(center >= damage_at(&bp, 0.1, 0.0), "on-axis ≥ off-axis");
        assert!(center >= damage_at(&bp, 0.0, bp.max_range * 0.5), "point-blank ≥ far");
    }

    #[test]
    fn zero_outside_cone_and_beyond_range() {
        let bp = p();
        assert_eq!(damage_at(&bp, bp.cone_half_angle + 0.1, 1.0), 0.0, "outside cone");
        assert_eq!(damage_at(&bp, 0.0, bp.max_range + 1.0), 0.0, "beyond range");
    }

    #[test]
    fn decreasing_with_distance_and_angle() {
        let bp = p();
        assert!(damage_at(&bp, 0.0, 2.0) > damage_at(&bp, 0.0, 8.0));
        assert!(damage_at(&bp, 0.05, 4.0) > damage_at(&bp, bp.cone_half_angle * 0.8, 4.0));
    }

    #[test]
    fn fire_hits_harder_than_plain() {
        let fire = params_from(&Genome::from_seed_element(2, 4)).base_damage;
        let air = params_from(&Genome::from_seed_element(2, 2)).base_damage;
        assert!(fire > air, "fire breath does more base damage");
    }

    #[test]
    fn fuel_recharges_and_spends() {
        assert!(recharge(0.2, 0.5, 0.5) > 0.2);
        assert_eq!(recharge(0.9, 1.0, 5.0), 1.0);
        let mut fuel = 0.5;
        assert!(try_breathe(&mut fuel, 0.3));
        assert!((fuel - 0.2).abs() < 1e-6);
        assert!(!try_breathe(&mut fuel, 0.5), "not enough fuel");
        assert!((fuel - 0.2).abs() < 1e-6, "rejected breath spends nothing");
    }

    #[test]
    fn deterministic() {
        let bp = p();
        assert_eq!(damage_at(&bp, 0.1, 3.0), damage_at(&bp, 0.1, 3.0));
    }
}
