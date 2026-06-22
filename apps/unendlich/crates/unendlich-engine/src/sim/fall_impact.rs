//! `sim::fall_impact` — landing speed + body → the **consequence** of a fall.
//!
//! The fall model elsewhere decides *how fast* a creature hits the ground; this
//! core is the pure mapping from that impact speed (plus a [`Body`]'s mass and
//! resilience) to an [`Impact`]: how much `damage`, how much `stagger`, and
//! whether it is knocked down. It is reactive-tier — a cheap deterministic
//! function with no clock and no randomness — so the same landing always reads
//! the same on every peer.
//!
//! Shape of the mapping:
//! - Below a **safe speed** there is no consequence at all (you can hop).
//! - Above it, damage grows with the *excess* speed, so it is monotonic in
//!   speed for a fixed body.
//! - Heavier bodies take more; **resilience** divides damage down.
//! - A **knockdown** only triggers once damage passes a cap, and `stagger` is a
//!   bounded `0..1` read of how rattled the landing was.
//!
//! Tuning lives in the [`tuning`] module so a designer has one place to look.

/// Hand-tuned constants for the fall→consequence mapping.
pub mod tuning {
    /// Impact speed (world units/s) at or below which a landing is free.
    pub const SAFE_SPEED: f32 = 8.0;
    /// Damage per unit of speed above [`SAFE_SPEED`], before body scaling.
    pub const DAMAGE_PER_EXCESS: f32 = 0.04;
    /// Damage at or above which the creature is knocked off its feet.
    pub const KNOCKDOWN_DAMAGE: f32 = 0.6;
    /// Damage that maps to full (`1.0`) stagger.
    pub const STAGGER_FULL_AT: f32 = 0.5;
}

/// The body taking the fall.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Body {
    /// Mass; heavier bodies take more damage. Treated as `~0.1..4`.
    pub mass: f32,
    /// Toughness; higher resilience divides damage down. Treated as `>= 0`.
    pub resilience: f32,
}

impl Body {
    /// A roughly humanoid default body.
    pub fn humanoid() -> Self {
        Body {
            mass: 1.0,
            resilience: 1.0,
        }
    }
}

/// The consequence of a landing.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Impact {
    /// Damage dealt by the landing. `>= 0`.
    pub damage: f32,
    /// How rattled the creature is, `0..1`.
    pub stagger: f32,
    /// Whether the landing knocks the creature down.
    pub knockdown: bool,
}

impl Impact {
    /// A clean landing: nothing happened.
    const NONE: Impact = Impact {
        damage: 0.0,
        stagger: 0.0,
        knockdown: false,
    };
}

fn clampf(x: f32, lo: f32, hi: f32) -> f32 {
    let x = if x > lo { x } else { lo };
    if x < hi {
        x
    } else {
        hi
    }
}

/// Map an impact speed and body to the landing consequence.
///
/// Returns [`Impact::NONE`] at or below [`tuning::SAFE_SPEED`]. Above it, damage
/// is `excess * DAMAGE_PER_EXCESS * mass_factor / (1 + resilience)`, so it rises
/// with speed and mass and falls with resilience, and is always `>= 0`.
pub fn impact_for(speed: f32, b: Body) -> Impact {
    let excess = speed - tuning::SAFE_SPEED;
    if excess <= 0.0 {
        return Impact::NONE;
    }

    // Heavier bodies hit harder, within sane bounds.
    let mass_factor = clampf(b.mass, 0.1, 4.0);
    // Resilience only ever softens; never below zero so it can't amplify.
    let resil = if b.resilience > 0.0 { b.resilience } else { 0.0 };

    let damage = excess * tuning::DAMAGE_PER_EXCESS * mass_factor / (1.0 + resil);
    let damage = if damage > 0.0 { damage } else { 0.0 };

    Impact {
        damage,
        stagger: clampf(damage / tuning::STAGGER_FULL_AT, 0.0, 1.0),
        knockdown: damage >= tuning::KNOCKDOWN_DAMAGE,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lcg(seed: &mut u32) -> u32 {
        *seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
        *seed >> 8
    }
    fn unit(seed: &mut u32) -> f32 {
        (lcg(seed) % 100_001) as f32 / 100_000.0
    }
    fn rand_body(seed: &mut u32) -> Body {
        Body {
            mass: 0.1 + unit(seed) * 4.0,
            resilience: unit(seed) * 4.0,
        }
    }

    #[test]
    fn zero_below_safe_threshold() {
        let mut seed = 0x0A11_u32;
        for _ in 0..1000 {
            let b = rand_body(&mut seed);
            // any speed up to and including the safe speed
            let speed = unit(&mut seed) * tuning::SAFE_SPEED;
            let imp = impact_for(speed, b);
            assert_eq!(imp.damage, 0.0);
            assert_eq!(imp.stagger, 0.0);
            assert!(!imp.knockdown);
        }
        // exactly at the threshold is still free
        assert_eq!(impact_for(tuning::SAFE_SPEED, Body::humanoid()), Impact::NONE);
    }

    #[test]
    fn prop_damage_non_negative() {
        let mut seed = 0xD4A6_u32;
        for _ in 0..3000 {
            let b = rand_body(&mut seed);
            let speed = unit(&mut seed) * 80.0;
            assert!(impact_for(speed, b).damage >= 0.0);
        }
    }

    #[test]
    fn prop_monotonic_in_speed() {
        let mut seed = 0x3303_u32;
        for _ in 0..3000 {
            let b = rand_body(&mut seed);
            let s1 = unit(&mut seed) * 80.0;
            let s2 = unit(&mut seed) * 80.0;
            let lo = s1.min(s2);
            let hi = s1.max(s2);
            let d_lo = impact_for(lo, b).damage;
            let d_hi = impact_for(hi, b).damage;
            assert!(d_hi >= d_lo, "damage fell as speed rose: {} -> {}", d_lo, d_hi);
        }
    }

    #[test]
    fn prop_higher_resilience_reduces_damage() {
        let mut seed = 0x9911_u32;
        for _ in 0..3000 {
            // a speed comfortably above threshold so damage is positive
            let speed = tuning::SAFE_SPEED + 1.0 + unit(&mut seed) * 60.0;
            let mass = 0.1 + unit(&mut seed) * 4.0;
            let r1 = unit(&mut seed) * 4.0;
            let r2 = unit(&mut seed) * 4.0;
            let lo = r1.min(r2);
            let hi = r1.max(r2);
            let d_lo = impact_for(speed, Body { mass, resilience: lo }).damage;
            let d_hi = impact_for(speed, Body { mass, resilience: hi }).damage;
            assert!(d_hi <= d_lo, "more resilience raised damage");
        }
    }

    #[test]
    fn knockdown_only_past_cap() {
        let mut seed = 0xCA99_u32;
        for _ in 0..3000 {
            let b = rand_body(&mut seed);
            let speed = unit(&mut seed) * 120.0;
            let imp = impact_for(speed, b);
            if imp.knockdown {
                assert!(imp.damage >= tuning::KNOCKDOWN_DAMAGE);
            } else {
                assert!(imp.damage < tuning::KNOCKDOWN_DAMAGE);
            }
        }
        // a gentle landing never knocks down; a violent one does
        assert!(!impact_for(tuning::SAFE_SPEED + 1.0, Body::humanoid()).knockdown);
        assert!(impact_for(200.0, Body::humanoid()).knockdown);
    }

    #[test]
    fn prop_stagger_in_unit_range() {
        let mut seed = 0x57A6_u32;
        for _ in 0..3000 {
            let b = rand_body(&mut seed);
            let speed = unit(&mut seed) * 200.0;
            let s = impact_for(speed, b).stagger;
            assert!(s >= 0.0 && s <= 1.0, "stagger out of range: {}", s);
        }
    }

    #[test]
    fn deterministic() {
        let mut seed = 0xFA77_u32;
        for _ in 0..2000 {
            let b = rand_body(&mut seed);
            let speed = unit(&mut seed) * 120.0;
            assert_eq!(impact_for(speed, b), impact_for(speed, b));
        }
    }
}
