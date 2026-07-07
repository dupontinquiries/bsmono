//! stamina — a small reserve a creature spends to **sprint**, used sparingly:
//! only when something matters (a threat, a startle, a high-priority goal). It
//! drains fast while sprinting, refills slowly at rest, and won't let a creature
//! sprint on empty — so a panicked bolt is a burst, not a permanent speed. A
//! hysteresis floor means you can't chain sprints: once spent, stamina has to
//! recover past the floor before the next one. Pure and deterministic.

/// Speed multiplier while actually sprinting.
pub const SPRINT_MULT: f32 = 1.8;
/// Stamina spent per second while sprinting (full → empty in ~10 s). Lowered
/// 5× from 0.5 so stamina lasts roughly 5× longer (operator: stamina felt low).
pub const DRAIN_PER_SEC: f32 = 0.1;
/// Stamina recovered per second at rest (slower than it drains).
pub const REGEN_PER_SEC: f32 = 0.18;
/// A sprint can only *start* at or above this reserve (so it stays sparing).
pub const SPRINT_FLOOR: f32 = 0.25;

/// A creature's sprint reserve in `[0, 1]` plus whether it is mid-sprint.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Stamina {
    pub level: f32,
    pub sprinting: bool,
}

impl Default for Stamina {
    fn default() -> Stamina {
        Stamina::full()
    }
}

impl Stamina {
    pub fn full() -> Stamina {
        Stamina { level: 1.0, sprinting: false }
    }
    pub fn new(level: f32) -> Stamina {
        Stamina { level: level.clamp(0.0, 1.0), sprinting: false }
    }

    /// Could a *fresh* sprint begin right now?
    pub fn can_sprint(&self) -> bool {
        self.level >= SPRINT_FLOOR
    }

    /// Advance one tick. `urge` is the high-level call to sprint — set it only for
    /// danger / startle / high-priority pursuit, never for ordinary wandering.
    /// Returns the speed multiplier to apply this tick: [`SPRINT_MULT`] while
    /// sprinting, else `1.0`. Sprinting needs both the urge and enough reserve to
    /// start; once started it continues until the urge passes or the reserve hits
    /// zero, draining meanwhile and regenerating otherwise.
    pub fn tick(&mut self, urge: bool, dt: f32) -> f32 {
        if self.sprinting {
            if !urge || self.level <= 0.0 {
                self.sprinting = false;
            }
        } else if urge && self.level >= SPRINT_FLOOR {
            self.sprinting = true;
        }

        if self.sprinting {
            self.level = (self.level - DRAIN_PER_SEC * dt).max(0.0);
            SPRINT_MULT
        } else {
            self.level = (self.level + REGEN_PER_SEC * dt).min(1.0);
            1.0
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_startle_sprints_and_drains() {
        let mut s = Stamina::full();
        let m = s.tick(true, 0.1);
        assert_eq!(m, SPRINT_MULT);
        assert!(s.sprinting && s.level < 1.0);
    }

    #[test]
    fn no_sprint_without_the_urge() {
        let mut s = Stamina::full();
        assert_eq!(s.tick(false, 0.1), 1.0);
        assert!(!s.sprinting);
        assert!(s.level >= 1.0 - 1e-6, "rested stamina stays full");
    }

    #[test]
    fn sprint_cannot_outrun_an_empty_reserve() {
        let mut s = Stamina::new(0.1); // already below the floor
        // Urge present, but too little to *start*.
        assert_eq!(s.tick(true, 0.1), 1.0);
        assert!(!s.sprinting);
    }

    #[test]
    fn a_held_urge_empties_then_stops() {
        let mut s = Stamina::full();
        let (mut sprint_ticks, mut ever, mut stopped_after) = (0, false, false);
        // Long enough to fully drain a burst at the current drain rate; we assert
        // the invariant (sprints, then stops when dry) rather than an exact count,
        // so this survives stamina retuning.
        for _ in 0..200 {
            if s.tick(true, 0.1) > 1.0 {
                sprint_ticks += 1;
                ever = true;
            } else if ever {
                stopped_after = true; // ran dry despite the urge still being on
            }
        }
        assert!(ever && sprint_ticks > 0, "should sprint while it has reserve");
        assert!(stopped_after, "burst isn't permanent — must stop when the reserve runs dry");
    }

    #[test]
    fn a_spent_reserve_recovers_to_full() {
        let mut s = Stamina::full();
        // sprint until the reserve is spent
        while s.tick(true, 0.1) > 1.0 {}
        assert!(!s.can_sprint(), "a spent reserve can't immediately re-sprint");
        // at rest it refills back to full in bounded time (recovery now generous —
        // drain was lowered 5× for longer sprints, so recovery is no longer the
        // slower of the two; it just has to terminate and restore sprinting).
        let mut recover = 0;
        while s.level < 1.0 - 1e-3 {
            s.tick(false, 0.1);
            recover += 1;
            assert!(recover < 10_000, "recovery must terminate");
        }
        assert!(s.can_sprint(), "fully rested ⇒ can sprint again");
    }
}

// ── service registration (co-located form) ──────────────────────────────────
// A worker adds their service to the live tick set by dropping this block in
// their own file — no edit to App or any central list. linkme links it in even
// though nothing else references `STAMINA_SERVICE`.
use crate::sim::service_registry::{Service, ServiceCtx, ServiceEntry, SERVICES};
use linkme::distributed_slice;

struct StaminaService(Stamina);
impl Service for StaminaService {
    fn name(&self) -> &'static str {
        "stamina"
    }
    fn tick(&mut self, ctx: &ServiceCtx) {
        let _ = self.0.tick(false, ctx.dt); // no urge until wired to player input
    }
}
#[distributed_slice(SERVICES)]
static STAMINA_SERVICE: ServiceEntry = ServiceEntry {
    name: "stamina",
    make: || Box::new(StaminaService(Stamina::new(1.0))),
};
