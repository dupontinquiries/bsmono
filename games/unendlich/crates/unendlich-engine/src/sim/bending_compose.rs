//! r332-bending — end-to-end bending composition (pure).
//!
//! `SECTION_bending.md` describes the full Resonance system, and the pure cores
//! all exist — `affinity_access` (env gate), `gesture_resolve` (grammar),
//! `bending_charge` (resonance), `player_bending` (emits an [`EffectIntent`]),
//! `bending_targeting`, `bending_feedback`, and the discipline cores
//! `bending_stormstrike` / `bending_heavy_brine` / `bending_thermal_phase`. The
//! gap this module closes: an `EffectIntent` was produced but nothing turned it
//! into **resolved targets + a discipline effect + feedback**. Here is that one
//! composition:
//!
//! ```text
//! EffectIntent
//!   → bending_targeting::resolve_targets        (who is hit)
//!   → discipline core by Discipline             (what happens)
//!   → bending_charge::Resonance::spend          (what it costs)
//!   → bending_feedback (mispredict + overdraw)  (what being wrong costs)
//!   = BendOutcome { hits, effect, backlash, resonance_after, power_realized01 }
//! ```
//!
//! **Composes existing APIs only** — no discipline is re-implemented. World/medium
//! inputs the disciplines need (conductors, brine medium, thermal material) are
//! *passed in* via [`BendWorld`]; nothing reads global/world state, so this stays
//! pure, deterministic, and panic-free (no IO/clock/RNG/`unwrap`/`expect`).
//! Per SHARED_RULES, any world mutation a discipline implies is returned as
//! outcome **data** — the bin applies it.
//!
//! Base note: this ships as a new `sim::bending_compose` module. On the r332 base
//! the dispatch targets, this content folds into `sim::bending_runtime`'s tick
//! (this tree's `bending_runtime` is an earlier `resonance_state`-based variant);
//! see HANDOFF_bending_compose.md.

use crate::sim::affinity_access::Fields;
use crate::sim::bending_charge::Resonance;
use crate::sim::bending_feedback::{self, Backlash};
use crate::sim::bending_focus::{focus, FocusItem};
use crate::sim::bending_heavy_brine::{self, BrineEffect, BrineState, BrineTechnique};
use crate::sim::bending_stormstrike::{self, Conductor, StrikePlan};
use crate::sim::bending_targeting::{self, Hit, Target};
use crate::sim::bending_thermal_phase::{self, Material, Phase};
use crate::sim::player_bending::{BendInput, Discipline, EffectIntent, PlayerBending};

/// The discipline-specific resolved effect. The three disciplines the brief names
/// route to their real cores; `Information` and `Elemental` have no effect core
/// this round and surface as [`DisciplineEffect::Unwired`] (honest scoping — they
/// are *not* re-implemented here).
#[derive(Clone, Debug, PartialEq)]
pub enum DisciplineEffect {
    /// `bending_stormstrike::route` — the chained bolt plan.
    Stormstrike(StrikePlan),
    /// `bending_heavy_brine::effect` — force / slow / anchor.
    HeavyBrine(BrineEffect),
    /// `bending_thermal_phase` — signed heat to reach `target` + its resonance cost.
    ThermalPhase { heat: f32, cost01: f32, target: Phase },
    /// A committed discipline with no wired effect core this round.
    Unwired(Discipline),
}

/// Everything one committed bend resolves to — the data the combat/render/UI
/// layer consumes (and the bin applies to the world).
#[derive(Clone, Debug, PartialEq)]
pub struct BendOutcome {
    /// Targets the technique affects (`bending_targeting`).
    pub hits: Vec<Hit>,
    /// The discipline's resolved effect.
    pub effect: DisciplineEffect,
    /// What being wrong / forcing it cost the bender (`bending_feedback`).
    pub backlash: Backlash,
    /// The resonance resource after this tick's spend (debt updated).
    pub resonance_after: Resonance,
    /// Power actually delivered after the spend (`<= intent.power01`; lower when
    /// the technique was forced past the available charge).
    pub power_realized01: f32,
}

/// World/medium context the caller supplies. **Pure**: passed in, never read from
/// globals. Only the field(s) matching the cast discipline are consulted; the
/// rest are harmless defaults (see [`BendWorld::for_targets`]).
#[derive(Clone, Debug)]
pub struct BendWorld<'a> {
    /// Cast origin (the bender).
    pub origin: [f32; 3],
    /// Candidate targets for `resolve_targets`.
    pub targets: &'a [Target],
    // --- Stormstrike ---
    /// Chainable conductors for `bending_stormstrike::route`.
    pub conductors: &'a [Conductor],
    pub strike_max_links: u8,
    pub strike_max_hop: f32,
    pub strike_aim_cos: f32,
    // --- Heavy Brine ---
    pub brine: BrineState,
    pub brine_tech: BrineTechnique,
    // --- Thermal Phase ---
    pub thermal: Material,
    pub thermal_target: Phase,
}

impl<'a> BendWorld<'a> {
    /// A context carrying just the targeting inputs, with sensible discipline
    /// defaults for the rest (water medium, room-temp material). Callers override
    /// the field(s) relevant to the discipline they expect.
    pub fn for_targets(origin: [f32; 3], targets: &'a [Target], conductors: &'a [Conductor]) -> Self {
        BendWorld {
            origin,
            targets,
            conductors,
            strike_max_links: 4,
            strike_max_hop: 12.0,
            strike_aim_cos: 0.5,
            brine: BrineState { salinity01: 0.5, mass_kg: 200.0 },
            brine_tech: BrineTechnique::Crush,
            thermal: Material { freeze_c: 0.0, boil_c: 100.0, temp_c: 20.0 },
            thermal_target: Phase::Gas,
        }
    }
}

/// Map a [`Discipline`] to the backlash-flavour id `bending_feedback`
/// understands (0 Stormstrike, 1 Thermal Phase, 2 Heavy Brine, else balanced).
#[inline]
fn backlash_discipline_id(d: Discipline) -> u8 {
    match d {
        Discipline::Stormstrike => 0,
        Discipline::ThermalPhase => 1,
        Discipline::HeavyBrine => 2,
        _ => 255, // balanced default arm in discipline_backlash
    }
}

#[inline]
fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

/// Direction from `origin` to the strongest resolved hit, or `[0,0,0]` when the
/// cast hit nothing (which `bending_feedback::mispredict` reads as "committed to
/// nothing" → maximum error).
fn actual_aim(world: &BendWorld, hits: &[Hit]) -> [f32; 3] {
    let mut best: Option<&Hit> = None;
    for h in hits {
        if best.map_or(true, |b| h.strength01 > b.strength01) {
            best = Some(h);
        }
    }
    match best {
        Some(h) => world
            .targets
            .get(h.index)
            .map(|t| sub(t.pos, world.origin))
            .unwrap_or([0.0; 3]),
        None => [0.0; 3],
    }
}

/// Resolve a committed [`EffectIntent`] end-to-end against the world context and
/// the bender's resonance. Pure: `resonance` is taken by value and the updated
/// copy is returned in [`BendOutcome::resonance_after`].
pub fn resolve_bend(intent: &EffectIntent, world: &BendWorld, resonance: Resonance) -> BendOutcome {
    // 1. Who is hit (universal targeting).
    let hits = bending_targeting::resolve_targets(
        world.origin,
        intent.aim,
        &intent.shape,
        world.targets,
        intent.power01,
    );

    // 2. Pay for it. spend() returns the realized power and updates charge/debt.
    let mut res = resonance;
    let granted = res.spend(intent.power01);

    // 3. What happens — route to the real discipline core (no re-implementation).
    let effect = match intent.discipline {
        Discipline::Stormstrike => DisciplineEffect::Stormstrike(bending_stormstrike::route(
            world.origin,
            intent.aim,
            world.conductors,
            world.strike_max_links,
            world.strike_max_hop,
            world.strike_aim_cos,
        )),
        Discipline::HeavyBrine => {
            DisciplineEffect::HeavyBrine(bending_heavy_brine::effect(world.brine_tech, &world.brine, granted))
        }
        Discipline::ThermalPhase => DisciplineEffect::ThermalPhase {
            heat: bending_thermal_phase::heat_to_phase(&world.thermal, world.thermal_target),
            cost01: bending_thermal_phase::technique_cost01(&world.thermal, world.thermal_target),
            target: world.thermal_target,
        },
        d => DisciplineEffect::Unwired(d),
    };

    // 4. What being wrong / forcing it costs: misprediction (committed aim vs the
    //    actual nearest target) plus any overdraw debt from spending past charge.
    let mis = bending_feedback::mispredict(intent.aim, actual_aim(world, &hits));
    let mut backlash =
        bending_feedback::discipline_backlash(backlash_discipline_id(intent.discipline), mis, intent.power01);
    if res.is_overdrawn() {
        backlash = bending_feedback::combine(backlash, bending_feedback::overdraw_backlash(res.overdraw01));
    }

    BendOutcome {
        hits,
        effect,
        backlash,
        resonance_after: res,
        power_realized01: granted,
    }
}

/// Convenience bridge: raw input → grammar + access gate (`player_bending::step`)
/// → [`resolve_bend`]. Returns `None` when the discipline is gated off by the
/// fields or the gesture is being held — so an env-gated-off discipline yields
/// **no outcome at all**, the gating staying the single source of truth in
/// `player_bending`.
pub fn tick(
    pb: &mut PlayerBending,
    input: &BendInput,
    fields: &Fields,
    dt: f32,
    world: &BendWorld,
    resonance: Resonance,
) -> Option<BendOutcome> {
    let intent = pb.step(input, fields, dt)?;
    Some(resolve_bend(&intent, world, resonance))
}

/// Build resonance for a tick with the bender's equipped focus items applied:
/// the aggregated focus `resonance_gain_mul` ([`bending_focus::focus`]) scales
/// how fast charge accrues this step. Empty items → identity (a plain build).
/// Pure; wires `bending_focus` into the charge path.
pub fn build_resonance_focused(
    mut resonance: Resonance,
    items: &[FocusItem],
    on_rhythm01: f32,
    dt: f32,
) -> Resonance {
    let mods = focus(items);
    resonance.build_scaled(on_rhythm01, dt, mods.resonance_gain_mul);
    resonance
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sim::gesture_resolve::{self, Directional, Element, Gesture, Stance};

    fn rich() -> Fields {
        Fields { h: 0.9, r: 0.9, w: 0.9, p: 0.9, l: 0.9, s: 0.9, b: 0.9 }
    }
    fn dry() -> Fields {
        Fields { h: 0.0, r: 0.0, w: 0.0, p: 0.0, l: 0.0, s: 0.0, b: 0.0 }
    }

    fn input(el: Element, aim: [f32; 3]) -> BendInput {
        BendInput {
            stance: Stance::Open,
            gesture: Gesture::Strike,
            dir: Directional::Push,
            element: el,
            aim,
        }
    }

    // Build an EffectIntent directly for the discipline under test.
    fn intent_for(disc: Discipline, el: Element, aim: [f32; 3], power01: f32) -> EffectIntent {
        let shape = gesture_resolve::resolve(Stance::Open, Gesture::Strike, Directional::Push, el, 1.0);
        EffectIntent { discipline: disc, shape, power01, aim, committed: true }
    }

    fn targets_ahead() -> Vec<Target> {
        // a target straight along +X within reach
        vec![Target { pos: [6.0, 0.0, 0.0], radius: 1.0 }]
    }

    // 1. A committed intent resolves to hits via targeting.
    #[test]
    fn committed_intent_yields_hits() {
        let tg = targets_ahead();
        let world = BendWorld::for_targets([0.0, 0.0, 0.0], &tg, &[]);
        let it = intent_for(Discipline::ThermalPhase, Element::Fire, [1.0, 0.0, 0.0], 1.0);
        let out = resolve_bend(&it, &world, Resonance::full());
        assert!(!out.hits.is_empty(), "a target in the cone should be hit");
    }

    // 2. Each discipline routes to its own effect core.
    #[test]
    fn disciplines_route_to_their_effects() {
        let tg = targets_ahead();
        let conds = [Conductor { pos: [6.0, 0.0, 0.0], conductance01: 0.9, is_self: false }];
        let mut world = BendWorld::for_targets([0.0, 0.0, 0.0], &tg, &conds);
        world.brine_tech = BrineTechnique::Crush;

        let storm = resolve_bend(&intent_for(Discipline::Stormstrike, Element::Air, [1.0, 0.0, 0.0], 1.0), &world, Resonance::full());
        assert!(matches!(storm.effect, DisciplineEffect::Stormstrike(_)));

        let brine = resolve_bend(&intent_for(Discipline::HeavyBrine, Element::Water, [1.0, 0.0, 0.0], 1.0), &world, Resonance::full());
        assert!(matches!(brine.effect, DisciplineEffect::HeavyBrine(_)));

        let thermal = resolve_bend(&intent_for(Discipline::ThermalPhase, Element::Fire, [1.0, 0.0, 0.0], 1.0), &world, Resonance::full());
        assert!(matches!(thermal.effect, DisciplineEffect::ThermalPhase { .. }));

        let info = resolve_bend(&intent_for(Discipline::Information, Element::Air, [1.0, 0.0, 0.0], 1.0), &world, Resonance::full());
        assert!(matches!(info.effect, DisciplineEffect::Unwired(Discipline::Information)));
    }

    // 3. Stormstrike actually routes through the conductor.
    #[test]
    fn stormstrike_routes_through_conductor() {
        let tg = targets_ahead();
        let conds = [Conductor { pos: [5.0, 0.0, 0.0], conductance01: 1.0, is_self: false }];
        let world = BendWorld::for_targets([0.0, 0.0, 0.0], &tg, &conds);
        let out = resolve_bend(&intent_for(Discipline::Stormstrike, Element::Air, [1.0, 0.0, 0.0], 1.0), &world, Resonance::full());
        if let DisciplineEffect::Stormstrike(plan) = out.effect {
            assert_eq!(plan.path, vec![0]);
            assert!(!plan.grounded_on_self);
        } else {
            panic!("expected Stormstrike");
        }
    }

    // 4. A wrong commit (aim away from the only target) produces backlash.
    #[test]
    fn mispredict_produces_backlash() {
        let tg = targets_ahead(); // target at +X
        let world = BendWorld::for_targets([0.0, 0.0, 0.0], &tg, &[]);
        // aim at -X: opposite the target → max misprediction
        let it = intent_for(Discipline::ThermalPhase, Element::Fire, [-1.0, 0.0, 0.0], 1.0);
        let out = resolve_bend(&it, &world, Resonance::full());
        assert!(out.backlash.self_damage01 > 0.0, "a misaimed cast should sting");
    }

    // 5. Forcing an uncharged technique overdraws → overdraw backlash folded in.
    #[test]
    fn overdraw_produces_backlash() {
        let tg = targets_ahead();
        let world = BendWorld::for_targets([0.0, 0.0, 0.0], &tg, &[]);
        // empty resonance, full-power cast → spend overdraws
        let it = intent_for(Discipline::ThermalPhase, Element::Fire, [1.0, 0.0, 0.0], 1.0);
        let out = resolve_bend(&it, &world, Resonance::new());
        assert!(out.resonance_after.overdraw01 > 0.0, "forcing should accrue debt");
        assert!(out.power_realized01 < 1.0, "forced cast fires at reduced power");
        assert!(out.backlash.self_damage01 > 0.0 || out.backlash.stagger01 > 0.0);
    }

    // 6. Resonance is spent consistently when within budget.
    #[test]
    fn resonance_spent_within_budget() {
        let tg = targets_ahead();
        let world = BendWorld::for_targets([0.0, 0.0, 0.0], &tg, &[]);
        let before = Resonance::full();
        let it = intent_for(Discipline::ThermalPhase, Element::Fire, [1.0, 0.0, 0.0], 0.4);
        let out = resolve_bend(&it, &world, before);
        assert!((out.power_realized01 - 0.4).abs() < 1e-6, "full charge delivers full power");
        assert!(out.resonance_after.charge01 < before.charge01, "charge should drop by the spend");
        assert_eq!(out.resonance_after.overdraw01, 0.0, "no debt within budget");
    }

    // 7. Env-gated-off discipline yields NO outcome (gate lives in player_bending).
    #[test]
    fn gated_off_discipline_yields_no_outcome() {
        let tg = targets_ahead();
        let world = BendWorld::for_targets([0.0, 0.0, 0.0], &tg, &[]);
        let mut pb = PlayerBending::new();
        // dry fields → no discipline accessible → step returns None → tick None
        let out = tick(&mut pb, &input(Element::Air, [1.0, 0.0, 0.0]), &dry(), 0.0, &world, Resonance::full());
        assert!(out.is_none(), "a gated-off discipline must produce no outcome");
    }

    // 7b. With rich fields the same cast DOES produce an outcome (gate opens).
    #[test]
    fn accessible_discipline_yields_outcome() {
        let tg = targets_ahead();
        let world = BendWorld::for_targets([0.0, 0.0, 0.0], &tg, &[]);
        let mut pb = PlayerBending::new();
        let out = tick(&mut pb, &input(Element::Air, [1.0, 0.0, 0.0]), &rich(), 0.0, &world, Resonance::full());
        assert!(out.is_some(), "an accessible discipline should resolve");
        assert!(matches!(out.unwrap().effect, DisciplineEffect::Stormstrike(_)));
    }

    // 8. Determinism.
    #[test]
    fn deterministic() {
        let tg = targets_ahead();
        let conds = [Conductor { pos: [5.0, 0.0, 0.0], conductance01: 1.0, is_self: false }];
        let world = BendWorld::for_targets([0.0, 0.0, 0.0], &tg, &conds);
        let it = intent_for(Discipline::Stormstrike, Element::Air, [1.0, 0.0, 0.0], 0.8);
        let a = resolve_bend(&it, &world, Resonance::full());
        let b = resolve_bend(&it, &world, Resonance::full());
        assert_eq!(a, b);
    }

    // 9. A clean miss still resolves to an outcome (empty hits, miss backlash).
    #[test]
    fn miss_still_resolves() {
        let tg: Vec<Target> = Vec::new(); // nothing to hit
        let world = BendWorld::for_targets([0.0, 0.0, 0.0], &tg, &[]);
        let it = intent_for(Discipline::HeavyBrine, Element::Water, [1.0, 0.0, 0.0], 1.0);
        let out = resolve_bend(&it, &world, Resonance::full());
        assert!(out.hits.is_empty());
        // committed at full power into nothing → max mispredict → backlash present
        assert!(out.backlash.self_damage01 > 0.0 || out.backlash.stagger01 > 0.0);
        assert!(matches!(out.effect, DisciplineEffect::HeavyBrine(_)));
    }
}
