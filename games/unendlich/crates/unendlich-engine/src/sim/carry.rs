//! Carried-item effects — what an item does while it rides in your pack or hand,
//! before you ever *use* it. Carrying becomes a choice with consequences:
//!
//! - **Volatile** components build toward a hazard unless you keep their
//!   condition satisfied — the unstable tome that detonates if you dawdle
//!   instead of pushing its quest along. Meeting the calming condition (quest
//!   progress, the right place) bleeds the instability back down.
//! - **Auras** pulse a passive effect over a radius each tick — the Ring of
//!   Regrowth quickening the plants around you, or a stranger relic nudging the
//!   world through the physics systems.
//!
//! Pure: the volatility timer is integer/deterministic (it drives real events
//! and rolls back); the aura's falloff is an `f32` multiplier the plant/effect
//! layer consumes.

/// What a carried item does passively.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CarryKind {
    /// No passive effect.
    Inert,
    /// Instability climbs by `build` each tick; the calming condition bleeds off
    /// `calm`. Reaching `threshold` triggers the hazard (detonation).
    Volatile { build: u16, calm: u16, threshold: u16 },
    /// A passive aura of `radius` blocks that pulses each tick.
    Aura { radius: u8 },
}

/// What carrying produced this tick.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CarryEvent {
    Nothing,
    /// The volatile item just went off — the caller applies the blast.
    Detonate,
    /// Emit the aura effect over `radius` (caller applies regrowth, etc.).
    Pulse { radius: u8 },
}

/// The live carry state of one item.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Carried {
    pub kind: CarryKind,
    pub instability: u16,
    pub ticks: u32,
}

impl Carried {
    pub fn new(kind: CarryKind) -> Carried {
        Carried { kind, instability: 0, ticks: 0 }
    }

    /// One tick of carrying. `stabilizing` = is the calming condition met this
    /// tick (quest advancing, in the right biome…)? Returns what happened.
    pub fn tick(&mut self, stabilizing: bool) -> CarryEvent {
        self.ticks = self.ticks.saturating_add(1);
        match self.kind {
            CarryKind::Inert => CarryEvent::Nothing,
            CarryKind::Aura { radius } => CarryEvent::Pulse { radius },
            CarryKind::Volatile { build, calm, threshold } => {
                if stabilizing {
                    self.instability = self.instability.saturating_sub(calm);
                } else {
                    self.instability = self.instability.saturating_add(build);
                }
                if self.instability >= threshold {
                    self.instability = 0; // spent in the blast
                    CarryEvent::Detonate
                } else {
                    CarryEvent::Nothing
                }
            }
        }
    }

    /// How close a volatile item is to going off, `[0,1]` — drive a warning glow
    /// / hum off this. Non-volatile items read 0.
    pub fn danger(&self) -> f32 {
        match self.kind {
            CarryKind::Volatile { threshold, .. } if threshold > 0 => {
                (self.instability as f32 / threshold as f32).min(1.0)
            }
            _ => 0.0,
        }
    }
}

/// Extra plant-growth multiplier the Ring of Regrowth grants a plant `dist`
/// blocks from the wearer, given the aura `radius`. `1.0` = normal growth
/// (outside the aura), rising to `1 + MAX_REGROWTH_BONUS` at the centre and
/// fading linearly to the edge. Feed this into the plant lifecycle's growth step.
pub const MAX_REGROWTH_BONUS: f32 = 1.5;
pub fn regrowth_multiplier(dist: f32, radius: u8) -> f32 {
    let r = radius as f32;
    if r <= 0.0 || dist >= r {
        1.0
    } else {
        1.0 + MAX_REGROWTH_BONUS * (1.0 - dist / r)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn volatile_detonates_if_neglected() {
        let mut c = Carried::new(CarryKind::Volatile { build: 10, calm: 30, threshold: 100 });
        let mut detonated = false;
        for _ in 0..20 {
            if c.tick(false) == CarryEvent::Detonate {
                detonated = true;
                break;
            }
        }
        assert!(detonated, "left to climb, it goes off");
    }

    #[test]
    fn keeping_the_quest_moving_keeps_it_safe() {
        let mut c = Carried::new(CarryKind::Volatile { build: 10, calm: 30, threshold: 100 });
        // Calm bleeds (30) outpaces build (10): stabilizing every tick stays safe.
        for _ in 0..100 {
            assert_eq!(c.tick(true), CarryEvent::Nothing, "stabilized → never detonates");
        }
        assert_eq!(c.instability, 0);
    }

    #[test]
    fn danger_rises_toward_detonation() {
        let mut c = Carried::new(CarryKind::Volatile { build: 25, calm: 0, threshold: 100 });
        c.tick(false); // 25
        let early = c.danger();
        c.tick(false); // 50
        c.tick(false); // 75
        let late = c.danger();
        assert!(early > 0.0 && late > early && late < 1.0, "{early} -> {late}");
    }

    #[test]
    fn aura_pulses_each_tick() {
        let mut ring = Carried::new(CarryKind::Aura { radius: 6 });
        assert_eq!(ring.tick(false), CarryEvent::Pulse { radius: 6 });
        assert_eq!(ring.danger(), 0.0, "an aura is never dangerous");
        assert_eq!(Carried::new(CarryKind::Inert).tick(false), CarryEvent::Nothing);
    }

    #[test]
    fn regrowth_is_strongest_at_the_centre_and_normal_outside() {
        let centre = regrowth_multiplier(0.0, 6);
        let mid = regrowth_multiplier(3.0, 6);
        let edge = regrowth_multiplier(6.0, 6);
        assert!((centre - (1.0 + MAX_REGROWTH_BONUS)).abs() < 1e-6, "max boost at the wearer");
        assert!(centre > mid && mid > 1.0, "fades with distance: {centre} > {mid} > 1");
        assert_eq!(edge, 1.0, "normal growth at/outside the edge");
        assert_eq!(regrowth_multiplier(100.0, 6), 1.0, "far away → unaffected");
    }
}

// ── service_registry wiring (S2) ───────────────────────────────────
// Wires a carried object's state machine. Neutral: an Inert payload that
// is not being stabilized, so tick() returns Nothing — proves wiring.
use crate::sim::service_registry::{Service, ServiceCtx, ServiceEntry, SERVICES};
use linkme::distributed_slice;

struct CarryService(Carried);
impl Service for CarryService {
    fn name(&self) -> &'static str {
        "carry"
    }
    fn tick(&mut self, _ctx: &ServiceCtx) {
        let _ = self.0.tick(false);
    }
}
#[distributed_slice(SERVICES)]
static CARRY_SERVICE: ServiceEntry = ServiceEntry {
    name: "carry",
    make: || Box::new(CarryService(Carried::new(CarryKind::Inert))),
};
