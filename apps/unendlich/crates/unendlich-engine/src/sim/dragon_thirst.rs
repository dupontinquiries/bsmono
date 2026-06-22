//! `sim::dragon_thirst` — the dragon's hydration vital and its water-seeking urge.
//!
//! Spec: `docs/specs/river_systems.docx` (Dragon Thirst). A dragon loses water over
//! time — faster when exerting (flight/sprint) or hot (basking, fire-breathing) —
//! and must drink at a river/lake to recover. Thirst is more *urgent* than hunger
//! over the short term (a parched dragon will break off foraging to drink) but
//! hunger wins once it is actually starving.
//!
//! Mirrors `sim::feeding::Vitality` in shape so the two vitals compose cleanly; the
//! brain (or `dragon_activity`) consults [`drink_urge`] / [`prefers_water_over_food`]
//! to decide whether to route toward water. Pure: dt + exertion passed in, no clock.

use crate::sim::feeding::Vitality;

/// Baseline water loss per second when idle. Higher than food decay — animals
/// dehydrate faster than they starve.
pub const THIRST_DECAY_PER_SEC: f32 = 0.02;
/// Extra loss multiplier at full exertion (sprinting, flying, fire-breathing, heat).
pub const EXERTION_MULT: f32 = 2.5;
/// Below this hydration the dragon is thirsty (will seek water when convenient).
pub const THIRSTY_THRESHOLD: f32 = 0.5;
/// Below this it is parched (will break off other activities to drink).
pub const PARCHED_THRESHOLD: f32 = 0.2;
/// Hydration restored per second of drinking.
pub const DRINK_RATE: f32 = 0.6;

fn clamp01(v: f32) -> f32 {
    crate::util::mathx::saturate(v) // r343 dedup
}

/// The dragon's hydration, 1.0 = fully watered, 0.0 = parched.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Hydration {
    pub level: f32,
}

impl Default for Hydration {
    fn default() -> Self {
        Hydration::full()
    }
}

impl Hydration {
    pub fn new(level: f32) -> Hydration {
        Hydration { level: clamp01(level) }
    }
    pub fn full() -> Hydration {
        Hydration { level: 1.0 }
    }

    /// Lose water over `dt`. `exertion01` (0..1) is how hard the dragon is working
    /// (sprint/flight/heat/fire-breath) — it dehydrates faster under load.
    pub fn tick(&mut self, dt: f32, exertion01: f32) {
        let dt = if dt.is_finite() { dt.max(0.0) } else { 0.0 };
        let ex = clamp01(exertion01);
        let rate = THIRST_DECAY_PER_SEC * (1.0 + EXERTION_MULT * ex);
        self.level = clamp01(self.level - rate * dt);
    }

    /// Drink for `dt` seconds at a water source.
    pub fn drink(&mut self, dt: f32) {
        let dt = if dt.is_finite() { dt.max(0.0) } else { 0.0 };
        self.level = clamp01(self.level + DRINK_RATE * dt);
    }

    pub fn is_thirsty(&self) -> bool {
        self.level < THIRSTY_THRESHOLD
    }
    pub fn is_parched(&self) -> bool {
        self.level < PARCHED_THRESHOLD
    }
}

/// The urge (0..1) to go drink right now. Rises as hydration falls; being at/near a
/// water source boosts it (top up while you can, like grazing on grass underfoot),
/// but a parched dragon wants water even with none in sight.
pub fn drink_urge(h: Hydration, near_water: bool) -> f32 {
    // deficit grows the urge; squared so it ramps up near parched
    let deficit = 1.0 - h.level;
    let base = deficit * deficit;
    let convenience = if near_water { 0.25 } else { 0.0 };
    // parched floor: ensure a strong urge regardless of proximity
    let floor = if h.is_parched() { 0.7 } else { 0.0 };
    clamp01(base + convenience).max(floor)
}

/// Short-term priority: should the dragon seek water before food? Thirst is more
/// urgent than hunger UNTIL the dragon's energy is critically low, at which point
/// food wins (you can ride out thirst a little longer than an empty tank here).
pub fn prefers_water_over_food(h: Hydration, v: &Vitality) -> bool {
    /// Energy at/below which eating overrides everything.
    const CRIT_ENERGY: f32 = 0.1;
    if v.energy <= CRIT_ENERGY {
        return false; // critically low energy overrides — eat first
    }
    // parched always wins; otherwise thirst wins when thirstier than hungry
    h.is_parched() || (h.is_thirsty() && h.level <= v.energy)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sim::feeding::Vitality;

    #[test]
    fn dehydrates_over_time_faster_under_exertion() {
        let mut idle = Hydration::full();
        let mut working = Hydration::full();
        for _ in 0..100 {
            idle.tick(0.5, 0.0);
            working.tick(0.5, 1.0);
        }
        assert!(idle.level < 1.0, "loses water while idle");
        assert!(working.level < idle.level, "exertion dehydrates faster");
    }

    #[test]
    fn crosses_thirsty_then_parched() {
        let mut h = Hydration::full();
        let mut saw_thirsty = false;
        for _ in 0..2000 {
            h.tick(0.5, 0.5);
            if h.is_thirsty() && !h.is_parched() {
                saw_thirsty = true;
            }
            if h.is_parched() {
                assert!(saw_thirsty, "passes through thirsty before parched");
                break;
            }
        }
        assert!(h.is_parched(), "eventually parches without drinking");
    }

    #[test]
    fn drinking_restores_and_clamps() {
        let mut h = Hydration::new(0.1);
        h.drink(1.0);
        assert!(h.level > 0.1 && h.level <= 1.0);
        for _ in 0..10 {
            h.drink(1.0);
        }
        assert_eq!(h.level, 1.0, "drinking clamps at full");
    }

    #[test]
    fn urge_rises_as_it_dries_and_with_water_nearby() {
        let full = drink_urge(Hydration::full(), false);
        let low = drink_urge(Hydration::new(0.3), false);
        let low_near = drink_urge(Hydration::new(0.3), true);
        assert!(full < 0.05, "no urge when full");
        assert!(low > full, "urge rises as hydration falls");
        assert!(low_near > low, "water nearby nudges it to top up");
        assert!(drink_urge(Hydration::new(0.1), false) >= 0.7, "parched is a strong urge even with no water in sight");
    }

    #[test]
    fn thirst_beats_hunger_until_starving() {
        let fed = Vitality::full();
        // parched but fed → drink first
        assert!(prefers_water_over_food(Hydration::new(0.15), &fed));
        // thirstier than hungry → drink first
        let mildly_hungry = Vitality::new(0.45);
        assert!(prefers_water_over_food(Hydration::new(0.3), &mildly_hungry));
        // critically-low energy overrides thirst → eat first
        let starving = Vitality::new(0.05);
        assert!(!prefers_water_over_food(Hydration::new(0.15), &starving));
        // hydrated → no water preference
        assert!(!prefers_water_over_food(Hydration::full(), &fed));
    }

    #[test]
    fn nan_and_negative_dt_are_safe() {
        let mut h = Hydration::full();
        h.tick(f32::NAN, 2.0);
        h.tick(-1.0, f32::NAN);
        assert!(h.level.is_finite() && (0.0..=1.0).contains(&h.level));
    }
}
