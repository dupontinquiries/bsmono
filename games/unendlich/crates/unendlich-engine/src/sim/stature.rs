//! Stature — height as the master physical scalar (see `docs/STATURE.md`).
//!
//! A character's height (in blocks) drives their whole *physical* identity:
//! collider, eye height, mass, health, hunger, strength, knockback, locomotion
//! (step / jump / speed / turn), ground-shake, and wind/water-current resistance.
//! Two genes — `bulk` (girth) and `density` (vs water) — fold in, plus a
//! `strength` gene. Everything is anchored at the **1.8-block person** so a
//! default character derives back to today's tuning and nothing changes for them.
//!
//! Pure: numbers in → a [`StatureProfile`] out. No RNG, I/O, GPU, wall-clock; no
//! `unwrap`/`expect`/`panic!`; clamped inputs keep every output finite.

/// The physical attributes derived from a character's stature. All consumers
/// (physics, vitality, movement, combat, the footfall→shake/erosion path) read
/// from this one struct so the scaling laws live in a single place.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StatureProfile {
    /// Height in blocks (clamped to [0.9, 15.0]).
    pub height: f32,
    /// Collider half-extents (X, Y, Z); Y = height/2.
    pub half_extents: [f32; 3],
    /// Eye height above the feet (camera placement; ~0.9·height).
    pub eye_height: f32,
    /// Mass (kg-ish, reference person = 70).
    pub mass: f32,
    /// Material density vs water (1.0). < 1 floats, > 1 sinks.
    pub density: f32,
    /// Net buoyancy = 1 − density (signed; > 0 floats).
    pub buoyancy: f32,
    /// Max health (reference person = 100).
    pub max_health: f32,
    /// Strength multiplier (reach/leverage; reference = 1.0).
    pub strength: f32,
    /// Hunger-drain multiplier (metabolic; reference = 1.0).
    pub hunger_rate: f32,
    /// Multiplier on knockback this body *deals* on a hit.
    pub knockback_dealt: f32,
    /// Fraction of incoming knockback *negated* (0..1).
    pub knockback_resist01: f32,
    /// Auto-step-up height in blocks (reference = 1.1).
    pub step_height: f32,
    /// Jump take-off velocity in blocks/s (reference = 10.0).
    pub jump_velocity: f32,
    /// Walk speed in blocks/s (reference = 4.5).
    pub move_speed: f32,
    /// Yaw turn rate in rad/s (lower = wider radius; reference = 6.0).
    pub turn_rate: f32,
    /// Per-footfall shake intensity (camera shake + earthquake + erosion).
    /// ≈ 0 at the reference person, grows with mass.
    pub ground_shake: f32,
    /// Minimum wind magnitude needed to start pushing this body.
    pub wind_resist_threshold: f32,
    /// Multiplier on how much (above-threshold) wind pushes this body
    /// (lower = more resistant).
    pub wind_resist_mult: f32,
    /// Minimum water-current magnitude needed to start pushing this body.
    pub water_resist_threshold: f32,
    /// Multiplier on how much (above-threshold) current pushes this body.
    pub water_resist_mult: f32,
}

/// The reference person — every law is anchored here so `derive(1.8, 1, 1, 1)`
/// reproduces today's physics tuning.
pub const BASE_HEIGHT: f32 = 1.8;
const BASE_HALF_WIDTH: f32 = 0.30;
const BASE_EYE: f32 = 1.62;
const BASE_MASS: f32 = 70.0;
/// Reference person's max health (the normaliser for stature damage-resist).
pub const BASE_HEALTH: f32 = 100.0;
const BASE_JUMP: f32 = 10.0;
const BASE_WALK: f32 = 4.5;
const BASE_STEP: f32 = 1.1;
const BASE_TURN: f32 = 6.0;

/// Clamp bounds for stature + genes (kept here so callers and tests agree).
pub const MIN_HEIGHT: f32 = 0.1;
pub const MAX_HEIGHT: f32 = 50.0;

/// Derive the full physical profile from a character's height (blocks) and the
/// `bulk` (girth), `density` (vs water), and `strength` genes. See module docs /
/// `docs/STATURE.md` for the laws. Inputs are clamped; outputs are finite.
pub fn derive(height: f32, bulk: f32, density: f32, strength_gene: f32) -> StatureProfile {
    let h = clampf(height, MIN_HEIGHT, MAX_HEIGHT);
    let bulk = clampf(bulk, 0.5, 3.0);
    let density = clampf(density, 0.3, 3.0);
    let str_g = clampf(strength_gene, 0.5, 3.0);
    let r = h / BASE_HEIGHT; // size ratio vs the reference person

    // Collider: width grows with height × √bulk; height is the stature itself.
    let half_w = BASE_HALF_WIDTH * r * bulk.sqrt();
    let half_extents = [half_w, h * 0.5, half_w];
    let eye_height = BASE_EYE * r;

    // Mass ∝ volume = h³ · bulk · density.
    let mass = BASE_MASS * r * r * r * bulk * density;
    let buoyancy = 1.0 - density;

    let strength = r * str_g;
    let max_health = BASE_HEALTH * r.powf(2.2) * str_g.sqrt();
    let hunger_rate = (mass / BASE_MASS).powf(0.75);

    let knockback_dealt = strength;
    let knockback_resist01 = mass / (mass + BASE_MASS);

    let step_height = BASE_STEP * r;
    let mut jump_velocity = BASE_JUMP * r.powf(0.5);
    // r575: SMALL-CHARACTER HOP (operator) — characters under ~1.4 blocks jump about
    // 75% HIGHER. Apex height ∝ v², so +75% height = v × √1.75 ≈ ×1.323. Ramped over
    // [1.35, 1.4] so there's no step at the threshold; full boost at/under 1.35, none
    // at/above 1.4 (so the 1.8 reference person is untouched).
    {
        let t = ((1.4 - h) / 0.05).clamp(0.0, 1.0); // 1 for h ≤ 1.35, 0 for h ≥ 1.4
        jump_velocity *= 1.0 + 0.323 * t;
    }
    // r573: curb the giant speed scaling ~15% (operator) — exponent 0.7 → 0.65.
    // At r≈28 (50 blocks) this drops move_speed ~15% (≈47→40 m/s); the base 1.8
    // person is unchanged (r=1 ⇒ 4.5 either way).
    let move_speed = BASE_WALK * r.powf(0.65);
    let turn_rate = BASE_TURN * (BASE_HEIGHT / h).powf(0.5);

    // r580 FIX: ground_shake is driven by HEIGHT, not mass. The old `√(mass/70)−1`
    // leaked bulk×density into the shake, so after density started tracking the
    // muscle slider (r567) a *muscular* but normal-HEIGHT (1.8) character got a
    // non-zero shake and "felt like a giant" stomping. Keyed on the height ratio it
    // is exactly 0 at 1.8 for ANY build, and matches the old curve at default bulk
    // (r^1.5 = √(r³)). Only true tallness shakes the ground now.
    let ground_shake = (r.powf(1.5) - 1.0).max(0.0);

    let inv_r = BASE_HEIGHT / h;
    let mass_ratio = mass / BASE_MASS;

    StatureProfile {
        height: h,
        half_extents,
        eye_height,
        mass,
        density,
        buoyancy,
        max_health,
        strength,
        hunger_rate,
        knockback_dealt,
        knockback_resist01,
        step_height,
        jump_velocity,
        move_speed,
        turn_rate,
        ground_shake,
        wind_resist_threshold: mass_ratio,
        wind_resist_mult: inv_r,
        water_resist_threshold: mass_ratio,
        water_resist_mult: inv_r,
    }
}

#[inline]
fn clampf(v: f32, lo: f32, hi: f32) -> f32 {
    let v = if v.is_finite() { v } else { lo };
    if v < lo {
        lo
    } else if v > hi {
        hi
    } else {
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn near(a: f32, b: f32, eps: f32) -> bool {
        (a - b).abs() <= eps
    }

    #[test]
    fn reference_person_reproduces_today_tuning() {
        let p = derive(1.8, 1.0, 1.0, 1.0);
        assert!(near(p.half_extents[0], 0.30, 1e-3), "half_w {}", p.half_extents[0]);
        assert!(near(p.half_extents[1], 0.90, 1e-3), "half_h {}", p.half_extents[1]);
        assert!(near(p.eye_height, 1.62, 1e-3));
        assert!(near(p.mass, 70.0, 1e-2));
        assert!(near(p.max_health, 100.0, 1e-1));
        assert!(near(p.jump_velocity, 10.0, 1e-3));
        assert!(near(p.move_speed, 4.5, 1e-3));
        assert!(near(p.step_height, 1.1, 1e-3));
        assert!(near(p.turn_rate, 6.0, 1e-3));
        assert!(near(p.ground_shake, 0.0, 1e-3), "shake {}", p.ground_shake);
        assert!(near(p.knockback_resist01, 0.5, 1e-3));
        assert!(near(p.hunger_rate, 1.0, 1e-3));
    }

    #[test]
    fn taller_is_heavier_stronger_slower_to_turn_and_shakes_more() {
        let s = derive(1.8, 1.0, 1.0, 1.0);
        let big = derive(9.0, 1.0, 1.0, 1.0);
        assert!(big.mass > s.mass * 50.0); // (9/1.8)^3 = 125x
        assert!(big.max_health > s.max_health);
        assert!(big.strength > s.strength);
        assert!(big.step_height > s.step_height);
        assert!(big.jump_velocity > s.jump_velocity); // absolute jump higher
        assert!(big.move_speed > s.move_speed);
        assert!(big.turn_rate < s.turn_rate); // wider radius
        assert!(big.ground_shake > s.ground_shake + 1.0);
        assert!(big.knockback_resist01 > s.knockback_resist01);
        assert!(big.hunger_rate > s.hunger_rate);
        assert!(big.eye_height > s.eye_height);
        assert!(big.half_extents[1] > s.half_extents[1]);
    }

    #[test]
    fn density_sets_buoyancy_sign() {
        assert!(derive(1.8, 1.0, 0.6, 1.0).buoyancy > 0.0); // cork floats
        assert!(derive(1.8, 1.0, 1.5, 1.0).buoyancy < 0.0); // stone sinks
        assert!(near(derive(1.8, 1.0, 1.0, 1.0).buoyancy, 0.0, 1e-6));
    }

    #[test]
    fn extremes_stay_finite_and_clamped() {
        for &(h, b, d, s) in &[
            (0.9f32, 0.5f32, 0.3f32, 0.5f32),
            (15.0, 3.0, 3.0, 3.0),
            (f32::NAN, f32::INFINITY, -1.0, 0.0),
            (1000.0, 1000.0, 1000.0, 1000.0),
        ] {
            let p = derive(h, b, d, s);
            for v in [
                p.height, p.mass, p.max_health, p.strength, p.hunger_rate,
                p.knockback_dealt, p.knockback_resist01, p.step_height,
                p.jump_velocity, p.move_speed, p.turn_rate, p.ground_shake,
                p.wind_resist_threshold, p.wind_resist_mult,
                p.water_resist_threshold, p.water_resist_mult, p.eye_height,
            ] {
                assert!(v.is_finite(), "non-finite output");
            }
            assert!(p.height >= MIN_HEIGHT && p.height <= MAX_HEIGHT);
            assert!(p.knockback_resist01 >= 0.0 && p.knockback_resist01 < 1.0);
        }
    }
}
