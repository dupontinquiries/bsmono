//! `sim::survival` — gentle, PvP-safe survival drives.
//!
//! This core carries no damage and no death. It tracks three slow body
//! [`Vitals`] — `hunger`, `thirst`, `warmth`, each `0..1` — and turns them into
//! [`Drives`]: urgency scalars (`0..1`) the experience-brain arbitrates against
//! everything else it wants to do. A hungry creature *wants* to seek food; what
//! it does about that is the brain's call, not this module's.
//!
//! Reactive tier: [`tick`] is a pure step run every frame. Hunger and thirst
//! drift up with elapsed `dt` and reset when the creature eats or drinks;
//! warmth eases toward the ambient temperature with no overshoot. Everything is
//! clamped to `[0, 1]` so the state can never run away, no matter how large a
//! `dt` it is handed. No clock, no randomness — same inputs, same next state.

/// Per-tick drift rates and easing, in units of "fraction of the bar per second".
mod rate {
    /// How fast hunger fills with no food.
    pub const HUNGER_PER_SEC: f32 = 0.02;
    /// How fast thirst fills with no drink (a touch faster than hunger).
    pub const THIRST_PER_SEC: f32 = 0.03;
    /// How quickly warmth chases the ambient temperature.
    pub const WARMTH_EASE_PER_SEC: f32 = 0.5;
    /// Below this much vital headroom the drive starts to bite.
    pub const URGENCY_KNEE: f32 = 0.5;
}

/// The slowly-changing body state. Each field is a `0..1` bar.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Vitals {
    /// `0` = full, `1` = starving.
    pub hunger: f32,
    /// `0` = sated, `1` = parched.
    pub thirst: f32,
    /// `0` = freezing, `1` = warm.
    pub warmth: f32,
}

impl Vitals {
    /// A comfortable starting point: fed, watered, warm.
    pub fn comfortable() -> Self {
        Vitals {
            hunger: 0.0,
            thirst: 0.0,
            warmth: 1.0,
        }
    }
}

/// What the world is doing to the creature this tick.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EnvSample {
    /// Ambient temperature as a `0..1` warmth target.
    pub ambient_warmth: f32,
    /// True on a tick the creature eats.
    pub ate: bool,
    /// True on a tick the creature drinks.
    pub drank: bool,
}

/// Urgency scalars the brain reads. Each is `0..1`, rising as the matching
/// vital nears its dangerous limit.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Drives {
    /// Urge to find food.
    pub hunger: f32,
    /// Urge to find water.
    pub thirst: f32,
    /// Urge to find warmth.
    pub cold: f32,
}

fn clampf(x: f32, lo: f32, hi: f32) -> f32 {
    let x = if x > lo { x } else { lo };
    if x < hi {
        x
    } else {
        hi
    }
}

fn clamp01(x: f32) -> f32 {
    clampf(x, 0.0, 1.0)
}

/// Map a vital that is dangerous when *high* (hunger, thirst) to an urgency.
/// Flat until `URGENCY_KNEE`, then ramps to `1` at the limit.
fn urgency_high(v: f32) -> f32 {
    let v = clamp01(v);
    if v <= rate::URGENCY_KNEE {
        0.0
    } else {
        (v - rate::URGENCY_KNEE) / (1.0 - rate::URGENCY_KNEE)
    }
}

/// Advance the vitals one tick and derive the drives.
///
/// `dt` is seconds since the last tick; negative values are treated as `0`.
/// All outputs are clamped to `[0, 1]`.
pub fn tick(v: Vitals, env: EnvSample, dt: f32) -> (Vitals, Drives) {
    let dt = if dt > 0.0 { dt } else { 0.0 };

    // Hunger / thirst: rise with time, fully reset when fed / watered.
    let hunger = if env.ate {
        0.0
    } else {
        clamp01(v.hunger + rate::HUNGER_PER_SEC * dt)
    };
    let thirst = if env.drank {
        0.0
    } else {
        clamp01(v.thirst + rate::THIRST_PER_SEC * dt)
    };

    // Warmth eases toward ambient. Clamp the ease fraction to `[0,1]` so a huge
    // dt can reach ambient but never overshoot past it.
    let target = clamp01(env.ambient_warmth);
    let ease = clamp01(rate::WARMTH_EASE_PER_SEC * dt);
    let warmth = clamp01(v.warmth + (target - v.warmth) * ease);

    let vitals = Vitals {
        hunger,
        thirst,
        warmth,
    };
    let drives = Drives {
        hunger: urgency_high(hunger),
        thirst: urgency_high(thirst),
        // Cold is dangerous when warmth is *low*: urgency on the inverted bar.
        cold: urgency_high(1.0 - warmth),
    };
    (vitals, drives)
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
    fn rand_vitals(seed: &mut u32) -> Vitals {
        Vitals {
            hunger: unit(seed),
            thirst: unit(seed),
            warmth: unit(seed),
        }
    }
    fn rand_env(seed: &mut u32) -> EnvSample {
        EnvSample {
            ambient_warmth: unit(seed),
            ate: lcg(seed) & 1 == 0,
            drank: lcg(seed) & 1 == 0,
        }
    }

    fn in01(x: f32) -> bool {
        x >= 0.0 && x <= 1.0
    }

    #[test]
    fn prop_vitals_stay_in_unit_range() {
        let mut seed = 0xA11C_u32;
        for _ in 0..3000 {
            let v = rand_vitals(&mut seed);
            let env = rand_env(&mut seed);
            // wild dt, including absurdly large and negative
            let dt = unit(&mut seed) * 1000.0 - 100.0;
            let (nv, _) = tick(v, env, dt);
            assert!(in01(nv.hunger) && in01(nv.thirst) && in01(nv.warmth));
        }
    }

    #[test]
    fn prop_drives_in_unit_range() {
        let mut seed = 0xD00D_u32;
        for _ in 0..3000 {
            let v = rand_vitals(&mut seed);
            let env = rand_env(&mut seed);
            let dt = unit(&mut seed) * 10.0;
            let (_, d) = tick(v, env, dt);
            assert!(in01(d.hunger) && in01(d.thirst) && in01(d.cold));
        }
    }

    #[test]
    fn hunger_rises_monotonically_without_food() {
        let env = EnvSample {
            ambient_warmth: 0.5,
            ate: false,
            drank: false,
        };
        let mut v = Vitals::comfortable();
        let mut prev = v.hunger;
        let mut saw_increase = false;
        for _ in 0..120 {
            v = tick(v, env, 1.0).0;
            assert!(v.hunger >= prev, "hunger dipped: {} -> {}", prev, v.hunger);
            if v.hunger > prev {
                saw_increase = true;
            }
            prev = v.hunger;
        }
        assert!(saw_increase, "hunger never rose");
        assert!((v.hunger - 1.0).abs() < 1e-6, "should have capped at 1");
    }

    #[test]
    fn eating_lowers_hunger() {
        let start = Vitals {
            hunger: 0.7,
            thirst: 0.4,
            warmth: 0.5,
        };
        let env = EnvSample {
            ambient_warmth: 0.5,
            ate: true,
            drank: false,
        };
        let after = tick(start, env, 0.5).0;
        assert!(after.hunger < start.hunger);
        assert_eq!(after.hunger, 0.0);
    }

    #[test]
    fn drinking_lowers_thirst() {
        let start = Vitals {
            hunger: 0.4,
            thirst: 0.8,
            warmth: 0.5,
        };
        let env = EnvSample {
            ambient_warmth: 0.5,
            ate: false,
            drank: true,
        };
        let after = tick(start, env, 0.5).0;
        assert!(after.thirst < start.thirst);
        assert_eq!(after.thirst, 0.0);
    }

    #[test]
    fn warmth_tracks_ambient_without_overshoot() {
        let mut seed = 0x5EED_u32;
        for _ in 0..2000 {
            let v = rand_vitals(&mut seed);
            let env = rand_env(&mut seed);
            let dt = unit(&mut seed) * 5.0;
            let after = tick(v, env, dt).0.warmth;
            let target = clamp01(env.ambient_warmth);
            // moves toward target (or stays put), and never crosses it
            let before_gap = (target - v.warmth).abs();
            let after_gap = (target - after).abs();
            assert!(after_gap <= before_gap + 1e-6, "moved away from ambient");
            // sign check: didn't overshoot to the far side
            if v.warmth < target {
                assert!(after <= target + 1e-6);
            } else {
                assert!(after >= target - 1e-6);
            }
        }
    }

    #[test]
    fn deterministic() {
        let mut seed = 0xC0FE_u32;
        for _ in 0..1000 {
            let v = rand_vitals(&mut seed);
            let env = rand_env(&mut seed);
            let dt = unit(&mut seed) * 8.0;
            assert_eq!(tick(v, env, dt), tick(v, env, dt));
        }
    }
}
