//! `sim::mob_forage_memory` — WID r102-M1-mobs.
//!
//! Deepens mob behaviour on top of [`crate::sim::fauna_mind`] (object-permanence
//! cognition) and the `physical_mob_engine` / `mob_brain_object_permanence`
//! design docs. It closes the loop those leave open: a creature's **internal
//! vitality** (hunger/fatigue), a **decaying memory of where food was last
//! seen** (object permanence for food, not just threat), and a small
//! **forage→flee→rest state machine** that picks a mode from vitality + the
//! mind's threat signal.
//!
//! What this adds over what already exists:
//! - `fauna_mind` composes perceive→threat→place-memory→flee→herd, but its place
//!   memory is threat-only and it has no hunger/fatigue or mode FSM.
//! - `behavior_arbiter` scores drives → behaviour, and `fauna_behavior` applies a
//!   temperament FSM, but neither *maintains* the drives over time or remembers
//!   food. This module owns that state: it advances hunger/fatigue, remembers and
//!   forgets food places, and feeds a clean `MobMode` out.
//!
//! Design contract proven by tests:
//! - **Object permanence for food:** a creature that has seen food still recalls
//!   its place after the food leaves view, and **forgets it after `T` seconds**
//!   (salience decays below recall threshold).
//! - **Vitality FSM:** hungry + safe → `Forage`; any real threat → `Flee`
//!   (overrides hunger); rested-and-fed + safe → `Rest`.
//! - Hunger rises over time and falls when feeding; fatigue rises while active
//!   and falls while resting. All scalars clamped `[0,1]`, finite, deterministic.
//!
//! Pure: data in → data out. No IO/RNG/clock/render, no `unwrap`/`expect`/`panic!`
//! in non-test code. Registered as a `service_registry` service (neutral tick).

use crate::sim::fauna_mind::{FaunaMind, WorldSense};
use crate::sim::mob_memory::MobMemory;

// ── Tunables (documented; per-species overrides via SpeciesParams) ──────────────

/// Landmark id used for "food was here" memories (distinct from fauna_mind's
/// threat landmark id 1).
const FOOD_LANDMARK: u32 = 2;

/// Recall radius (world units) for "is there remembered food near me".
const FOOD_RECALL_R: f32 = 64.0;

/// Salience at or below which a food memory is considered forgotten.
const FORGET_EPS: f32 = 0.05;

/// Threat (from the mind, `[0,1]`) at/above which the creature flees regardless
/// of hunger.
const FLEE_THREAT: f32 = 0.45;

/// Hunger at/above which foraging becomes the priority (when safe).
const HUNGER_FORAGE: f32 = 0.45;

/// Fatigue at/above which the creature wants to rest (when safe and not starving).
const FATIGUE_REST: f32 = 0.6;

// ── Per-species parameters ──────────────────────────────────────────────────────

/// Per-species behaviour tuning. Defaults model a mid herbivore.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SpeciesParams {
    /// Hunger gained per second while not feeding.
    pub hunger_rate: f32,
    /// Hunger removed per second while feeding.
    pub feed_rate: f32,
    /// Fatigue gained per second while active (Forage/Flee).
    pub fatigue_rate: f32,
    /// Fatigue removed per second while resting.
    pub rest_rate: f32,
    /// Food memory salience lost per second once out of view (controls forget-T).
    pub food_forget_rate: f32,
    /// Memory capacity (number of remembered places).
    pub memory_slots: usize,
}

impl SpeciesParams {
    /// A balanced grazer.
    pub fn grazer() -> SpeciesParams {
        SpeciesParams {
            hunger_rate: 0.02,
            feed_rate: 0.35,
            fatigue_rate: 0.03,
            rest_rate: 0.12,
            food_forget_rate: 0.05, // food fades below recall in ~60s @ 60fps (see forget_secs)
            memory_slots: 8,
        }
    }

    /// A restless predator: hungrier faster, tires faster, sharper memory.
    pub fn predator() -> SpeciesParams {
        SpeciesParams {
            hunger_rate: 0.035,
            feed_rate: 0.5,
            fatigue_rate: 0.05,
            rest_rate: 0.1,
            food_forget_rate: 0.025, // ~120s @ 60fps — stronger object permanence
            memory_slots: 12,
        }
    }

    /// The seconds it takes a fresh (salience 1.0) food memory to fade to the
    /// forget threshold with no re-sighting, at a given tick `dt`.
    ///
    /// `MobMemory::decay(r)` multiplies salience by `(1 - r)` each call, and this
    /// module calls it with `r = food_forget_rate * dt` per tick — so decay is
    /// **exponential**: after `t` seconds (`t/dt` ticks) salience is
    /// `(1 - food_forget_rate*dt)^(t/dt)`. Solving `= FORGET_EPS` gives the
    /// closed form below. (As `dt → 0` this approaches the continuous
    /// `ln(EPS) / -food_forget_rate`.)
    pub fn forget_secs(&self, dt: f32) -> f32 {
        let r = self.food_forget_rate;
        if r <= 0.0 || dt <= 0.0 {
            return f32::MAX;
        }
        let per = 1.0 - r * dt;
        if per <= 0.0 {
            return dt; // one tick wipes it
        }
        // t = dt * ln(EPS) / ln(per)
        (dt * (FORGET_EPS.ln() / per.ln())).max(0.0)
    }
}

impl Default for SpeciesParams {
    fn default() -> Self {
        SpeciesParams::grazer()
    }
}

// ── Per-tick sense additions (food) layered on fauna_mind's WorldSense ──────────

/// The food-related senses this layer needs, alongside the [`WorldSense`] the
/// underlying mind consumes. Kept separate so we don't restructure `fauna_mind`.
#[derive(Clone, Copy, Debug)]
pub struct ForageSense {
    /// Food is visible this tick at this position.
    pub food_visible: bool,
    pub food_pos: [f32; 3],
    /// The creature is in contact with food and can eat it this tick.
    pub eating: bool,
}

impl ForageSense {
    /// No food in view, not eating.
    pub fn none() -> ForageSense {
        ForageSense { food_visible: false, food_pos: [0.0; 3], eating: false }
    }
}

// ── Mode ────────────────────────────────────────────────────────────────────────

/// The behavioural mode chosen this tick.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MobMode {
    /// Calm, low needs, safe — idle/rest.
    Rest,
    /// Hungry and safe — seek food (toward remembered food if any).
    Forage,
    /// Under threat — flee (overrides everything).
    Flee,
}

/// The full decision this tick.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ForageOutput {
    pub mode: MobMode,
    pub hunger: f32,
    pub fatigue: f32,
    pub threat: f32,
    /// Where to head: remembered/seen food in Forage; away handled by caller in
    /// Flee (we surface the mind's threat); `None` if nothing to seek.
    pub goal: Option<[f32; 3]>,
}

fn clamp01(v: f32) -> f32 {
    crate::util::mathx::saturate(v) // r343 dedup
}

// ── The behaviour ─────────────────────────────────────────────────────────────

/// A creature's vitality + food memory + mode, layered over its [`FaunaMind`].
#[derive(Clone, Debug)]
pub struct MobForager {
    pub mind: FaunaMind,
    pub params: SpeciesParams,
    /// `[0,1]` — 0 = full, 1 = starving.
    pub hunger: f32,
    /// `[0,1]` — 0 = fresh, 1 = exhausted.
    pub fatigue: f32,
    /// Remembered food places (object permanence for food).
    pub food_memory: MobMemory,
    tick: u64,
}

impl MobForager {
    pub fn new(pos: [f32; 3], hp: f32, courage: f32, params: SpeciesParams) -> MobForager {
        MobForager {
            mind: FaunaMind::new(pos, hp, courage),
            params,
            hunger: 0.0,
            fatigue: 0.0,
            food_memory: MobMemory::new(params.memory_slots.max(1)),
            tick: 0,
        }
    }

    /// Is there a remembered food place within recall radius right now?
    ///
    /// `MobMemory::recall` returns any entry within radius regardless of how far
    /// its salience has decayed, so we apply the forget threshold here: a food
    /// memory whose salience has fallen to/below [`FORGET_EPS`] is treated as
    /// forgotten (object permanence has a finite horizon `T`).
    pub fn recalled_food(&self) -> Option<[f32; 3]> {
        self.food_memory
            .recall(self.mind.pos, FOOD_RECALL_R)
            .filter(|m| m.salience > FORGET_EPS)
            .map(|m| m.pos)
    }

    /// Advance one tick. Updates the mind, vitality, and food memory, then
    /// chooses a [`MobMode`].
    pub fn step(&mut self, sense: &WorldSense, forage: &ForageSense, dt: f32) -> ForageOutput {
        let dt = if dt.is_finite() { dt.max(0.0) } else { 0.0 };

        // 1) Underlying cognition (threat, flee, perception).
        let mind_out = self.mind.step(sense, dt);
        let threat = clamp01(mind_out.threat);

        // 2) Food object-permanence: commit a memory when food is seen, and
        //    decay all food memories over time (forgetting after T).
        if forage.food_visible {
            self.food_memory.remember(FOOD_LANDMARK, forage.food_pos, 1.0, self.tick);
        }
        self.food_memory.decay(dt * self.params.food_forget_rate);

        // 3) Vitality update. Feeding reduces hunger; otherwise it grows.
        if forage.eating {
            self.hunger = clamp01(self.hunger - dt * self.params.feed_rate);
        } else {
            self.hunger = clamp01(self.hunger + dt * self.params.hunger_rate);
        }

        // 4) Choose mode (decide before applying fatigue so the mode reflects
        //    this tick's intent), then apply fatigue per the chosen activity.
        let mode = self.choose_mode(threat);

        match mode {
            MobMode::Rest => {
                self.fatigue = clamp01(self.fatigue - dt * self.params.rest_rate);
            }
            MobMode::Forage | MobMode::Flee => {
                self.fatigue = clamp01(self.fatigue + dt * self.params.fatigue_rate);
            }
        }

        // 5) Goal: remembered/seen food when foraging.
        let goal = match mode {
            MobMode::Forage => {
                if forage.food_visible {
                    Some(forage.food_pos)
                } else {
                    self.recalled_food()
                }
            }
            _ => None,
        };

        self.tick = self.tick.wrapping_add(1);
        ForageOutput { mode, hunger: self.hunger, fatigue: self.fatigue, threat, goal }
    }

    /// Mode arbitration: threat dominates; otherwise hunger drives foraging,
    /// then fatigue drives rest, else rest by default.
    fn choose_mode(&self, threat: f32) -> MobMode {
        if threat >= FLEE_THREAT {
            return MobMode::Flee;
        }
        // Safe. Starving overrides tiredness — eat before you sleep.
        if self.hunger >= HUNGER_FORAGE {
            return MobMode::Forage;
        }
        if self.fatigue >= FATIGUE_REST {
            return MobMode::Rest;
        }
        // Fed and not tired: rest/idle.
        MobMode::Rest
    }
}

// ── service registration (smoke test ticks the whole composition) ───────────────

struct MobForagerService(MobForager);
impl crate::sim::service_registry::Service for MobForagerService {
    fn name(&self) -> &'static str {
        "mob_forage_memory"
    }
    fn tick(&mut self, ctx: &crate::sim::service_registry::ServiceCtx) {
        // Neutral senses prove the call path links + runs panic-free.
        let s = WorldSense::neutral();
        let f = ForageSense::none();
        let _ = self.0.step(&s, &f, ctx.dt);
    }
}
#[linkme::distributed_slice(crate::sim::service_registry::SERVICES)]
static MOB_FORAGE_MEMORY_SERVICE: crate::sim::service_registry::ServiceEntry =
    crate::sim::service_registry::ServiceEntry {
        name: "mob_forage_memory",
        make: || {
            Box::new(MobForagerService(MobForager::new(
                [0.0, 64.0, 0.0],
                1.0,
                0.5,
                SpeciesParams::grazer(),
            )))
        },
    };

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn calm() -> WorldSense {
        WorldSense::neutral()
    }

    fn threat_sense(player_xz: [f32; 2]) -> WorldSense {
        WorldSense {
            player_pos: [player_xz[0], 64.0, player_xz[1]],
            player_yaw: 0.0,
            noise: 0.9,
            has_los: true,
            roar_power: 0.0,
            roar_dist: 1.0e6,
            roar_range: 1.0,
            herd_centroid: [0.0, 0.0],
            nearest_neighbor: [1.0e6, 1.0e6],
        }
    }

    fn food_at(pos: [f32; 3]) -> ForageSense {
        ForageSense { food_visible: true, food_pos: pos, eating: false }
    }

    // ── Object permanence for food: remember after it leaves view ──

    #[test]
    fn remembers_food_after_it_leaves_view() {
        let mut m = MobForager::new([0.0, 64.0, 0.0], 1.0, 0.5, SpeciesParams::grazer());
        // See food a few ticks at (20,64,5).
        let seen = food_at([20.0, 64.0, 5.0]);
        for _ in 0..10 {
            m.step(&calm(), &seen, 1.0 / 60.0);
        }
        assert!(m.recalled_food().is_some(), "should have a food memory while seeing it");
        // Food leaves view; shortly after, it's still remembered.
        let gone = ForageSense::none();
        for _ in 0..30 {
            m.step(&calm(), &gone, 1.0 / 60.0);
        }
        let recalled = m.recalled_food().expect("food still remembered shortly after leaving view");
        assert!((recalled[0] - 20.0).abs() < 1e-3 && (recalled[2] - 5.0).abs() < 1e-3);
    }

    #[test]
    fn forgets_food_after_T() {
        let p = SpeciesParams::grazer();
        let mut m = MobForager::new([0.0, 64.0, 0.0], 1.0, 0.5, p);
        let seen = food_at([15.0, 64.0, 0.0]);
        // one sighting
        m.step(&calm(), &seen, 1.0 / 60.0);
        assert!(m.recalled_food().is_some());
        // Run well past forget_secs with no re-sighting.
        let gone = ForageSense::none();
        let dt = 1.0 / 60.0;
        let t = p.forget_secs(dt);
        let steps = ((t * 1.5) * 60.0) as usize + 1;
        for _ in 0..steps {
            m.step(&calm(), &gone, dt);
        }
        assert!(m.recalled_food().is_none(), "food should be forgotten after T={t:.1}s");
    }

    #[test]
    fn predator_remembers_food_longer_than_grazer() {
        // Stronger object permanence ⇒ longer forget time.
        let dt = 1.0 / 60.0;
        assert!(
            SpeciesParams::predator().forget_secs(dt) > SpeciesParams::grazer().forget_secs(dt),
            "predator should remember food longer"
        );
    }

    #[test]
    fn re_sighting_refreshes_memory() {
        let mut m = MobForager::new([0.0, 64.0, 0.0], 1.0, 0.5, SpeciesParams::grazer());
        let seen = food_at([10.0, 64.0, 0.0]);
        m.step(&calm(), &seen, 1.0 / 60.0);
        let gone = ForageSense::none();
        // decay a while (but not past T)
        for _ in 0..120 {
            m.step(&calm(), &gone, 1.0 / 60.0);
        }
        assert!(m.recalled_food().is_some(), "still remembered mid-decay");
        // re-sight: salience back to full
        for _ in 0..5 {
            m.step(&calm(), &seen, 1.0 / 60.0);
        }
        // now decay the SAME duration again — should still be remembered because refreshed
        for _ in 0..120 {
            m.step(&calm(), &gone, 1.0 / 60.0);
        }
        assert!(m.recalled_food().is_some(), "re-sighting should refresh and extend memory");
    }

    // ── Vitality FSM: forage / flee / rest ──

    #[test]
    fn hungry_and_safe_forages() {
        let mut m = MobForager::new([0.0, 64.0, 0.0], 1.0, 0.9, SpeciesParams::grazer());
        m.hunger = 0.8; // hungry
        let out = m.step(&calm(), &ForageSense::none(), 1.0 / 60.0);
        assert_eq!(out.mode, MobMode::Forage, "hungry + safe ⇒ forage");
    }

    #[test]
    fn threat_overrides_hunger_and_flees() {
        // Frail + timid so the mind's threat climbs fast and flee triggers.
        let mut m = MobForager::new([0.0, 64.0, 0.0], 0.15, 0.1, SpeciesParams::grazer());
        m.hunger = 1.0; // starving — but threat must still win
        let near = threat_sense([1.5, 0.0]);
        let mut out = m.step(&near, &ForageSense::none(), 1.0 / 60.0);
        for _ in 0..200 {
            out = m.step(&near, &ForageSense::none(), 1.0 / 60.0);
        }
        assert_eq!(out.mode, MobMode::Flee, "sustained threat overrides hunger");
        assert!(out.threat >= FLEE_THREAT);
    }

    #[test]
    fn fed_and_tired_and_safe_rests() {
        let mut m = MobForager::new([0.0, 64.0, 0.0], 1.0, 0.9, SpeciesParams::grazer());
        m.hunger = 0.0; // fed
        m.fatigue = 0.9; // tired
        let out = m.step(&calm(), &ForageSense::none(), 1.0 / 60.0);
        assert_eq!(out.mode, MobMode::Rest, "fed + tired + safe ⇒ rest");
    }

    #[test]
    fn forage_goal_is_remembered_food_when_out_of_view() {
        let mut m = MobForager::new([0.0, 64.0, 0.0], 1.0, 0.9, SpeciesParams::grazer());
        // learn a food place
        let seen = food_at([12.0, 64.0, 3.0]);
        for _ in 0..5 {
            m.step(&calm(), &seen, 1.0 / 60.0);
        }
        // now hungry, food out of view → forage goal should be the remembered place
        m.hunger = 0.9;
        let out = m.step(&calm(), &ForageSense::none(), 1.0 / 60.0);
        assert_eq!(out.mode, MobMode::Forage);
        let g = out.goal.expect("forage goal should be the remembered food");
        assert!((g[0] - 12.0).abs() < 1e-3 && (g[2] - 3.0).abs() < 1e-3);
    }

    // ── Vitality dynamics ──

    #[test]
    fn hunger_rises_over_time_when_not_feeding() {
        let mut m = MobForager::new([0.0, 64.0, 0.0], 1.0, 0.9, SpeciesParams::grazer());
        let h0 = m.hunger;
        for _ in 0..600 {
            m.step(&calm(), &ForageSense::none(), 1.0 / 60.0);
        }
        assert!(m.hunger > h0, "hunger should rise when not feeding");
    }

    #[test]
    fn feeding_reduces_hunger() {
        let mut m = MobForager::new([0.0, 64.0, 0.0], 1.0, 0.9, SpeciesParams::grazer());
        m.hunger = 0.9;
        let eating = ForageSense { food_visible: true, food_pos: [0.0, 64.0, 0.0], eating: true };
        for _ in 0..120 {
            m.step(&calm(), &eating, 1.0 / 60.0);
        }
        assert!(m.hunger < 0.9, "feeding should reduce hunger, got {}", m.hunger);
    }

    #[test]
    fn resting_reduces_fatigue() {
        let mut m = MobForager::new([0.0, 64.0, 0.0], 1.0, 0.9, SpeciesParams::grazer());
        m.fatigue = 0.9;
        m.hunger = 0.0; // ensure Rest is chosen (safe, fed, tired)
        for _ in 0..120 {
            m.step(&calm(), &ForageSense::none(), 1.0 / 60.0);
        }
        assert!(m.fatigue < 0.9, "resting should reduce fatigue, got {}", m.fatigue);
    }

    #[test]
    fn vitality_stays_in_range() {
        let mut m = MobForager::new([0.0, 64.0, 0.0], 0.5, 0.5, SpeciesParams::predator());
        let near = threat_sense([3.0, 0.0]);
        for i in 0..2000 {
            let s = if i % 3 == 0 { near } else { calm() };
            let f = if i % 5 == 0 {
                food_at([5.0, 64.0, 0.0])
            } else {
                ForageSense::none()
            };
            let out = m.step(&s, &f, 1.0 / 60.0);
            assert!(out.hunger.is_finite() && (0.0..=1.0).contains(&out.hunger));
            assert!(out.fatigue.is_finite() && (0.0..=1.0).contains(&out.fatigue));
            assert!(out.threat.is_finite() && (0.0..=1.0).contains(&out.threat));
        }
    }

    // ── Determinism ──

    #[test]
    fn deterministic() {
        let run = || {
            let mut m = MobForager::new([0.0, 64.0, 0.0], 0.7, 0.4, SpeciesParams::grazer());
            let mut outs = Vec::new();
            for i in 0..300 {
                let s = if i % 4 == 0 { threat_sense([3.0, 1.0]) } else { calm() };
                let f = if i % 6 == 0 { food_at([8.0, 64.0, 2.0]) } else { ForageSense::none() };
                outs.push(m.step(&s, &f, 1.0 / 60.0));
            }
            outs
        };
        assert_eq!(run(), run(), "same inputs ⇒ same outputs");
    }

    #[test]
    fn nan_dt_is_safe() {
        let mut m = MobForager::new([0.0, 64.0, 0.0], 1.0, 0.5, SpeciesParams::grazer());
        let out = m.step(&calm(), &ForageSense::none(), f32::NAN);
        assert!(out.hunger.is_finite() && out.fatigue.is_finite());
    }

    // ── service smoke ──

    #[test]
    fn service_adapter_ticks_panic_free() {
        use crate::sim::service_registry::{Service, ServiceCtx};
        let mut svc =
            MobForagerService(MobForager::new([0.0, 64.0, 0.0], 1.0, 0.5, SpeciesParams::grazer()));
        let ctx = ServiceCtx { dt: 1.0 / 60.0 };
        for _ in 0..240 {
            svc.tick(&ctx);
        }
        assert_eq!(svc.name(), "mob_forage_memory");
    }
}
