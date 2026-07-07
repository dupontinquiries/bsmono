# HANDOFF — r332-bending (end-to-end bending composition)

**WID:** `r332-bending` · **Status:** pure core VERIFIED in-sandbox (10/10 mine,
85/85 with the real mounted modules; rustc 1.96.0).
**Coordinator action:** fold into `sim::bending_runtime`'s tick on the r332 base
(this tree's `bending_runtime` is an earlier variant — see "Base note"), and supply
the `BendWorld` inputs from the bin's combat/world layer.

---

## What landed (verified, pure)
`crates/unendlich-engine/src/sim/bending_compose.rs` (+1 `pub mod` line in `sim/mod.rs`).

The single composition the spec was missing — `EffectIntent` → resolved targets →
discipline effect → feedback:

```
resolve_bend(intent, world, resonance) -> BendOutcome {
    hits:            bending_targeting::resolve_targets(origin, aim, &shape, targets, power01)
    effect:          match discipline {
        Stormstrike  => bending_stormstrike::route(..)        -> StrikePlan
        HeavyBrine   => bending_heavy_brine::effect(tech, &state, granted) -> BrineEffect
        ThermalPhase => heat_to_phase + technique_cost01      -> { heat, cost01, target }
        Information | Elemental(_) => Unwired(discipline)      // no effect core this round
    }
    backlash:        discipline_backlash(disc_id, mispredict(aim, actual), power01)
                     combined with overdraw_backlash(overdraw01) when forced
    resonance_after: bending_charge::Resonance after spend(power01)
    power_realized01: the realized power (< power01 when forced past charge)
}
tick(pb, input, fields, dt, world, resonance) -> Option<BendOutcome>
    = player_bending::step (grammar + affinity_access gate) → resolve_bend
```

**Composes existing APIs only** — no discipline re-implemented. Gating stays in
`player_bending::step` (returns `None` when the field gate is shut → `tick` yields
`None`), and the resonance spend uses `bending_charge::Resonance::spend`.

Gate: flatten harness mounting the **real** module sources under `mod sim {}` so
their `crate::sim::*` paths resolve →
`rustc --edition 2021 --test` → **10 passed** (mine) / **85 passed** (with the 75
real tests of the mounted modules — nothing regressed).

## Tests (all pass)
committed intent → hits; each discipline routes to its effect; stormstrike routes
through a conductor; misaimed cast → backlash; forced/uncharged cast → overdraw +
overdraw backlash + reduced power; charge spent consistently within budget;
**env-gated-off discipline → no outcome**; accessible discipline → outcome;
deterministic; a clean miss still resolves (empty hits + miss backlash).

---

## BendWorld — inputs the bin supplies (pure; passed in, never read from globals)
```
origin, targets: &[Target]                              // universal targeting
conductors: &[Conductor], strike_{max_links,max_hop,aim_cos}  // Stormstrike route
brine: BrineState, brine_tech: BrineTechnique           // Heavy Brine medium
thermal: Material, thermal_target: Phase                // Thermal Phase material
```
`BendWorld::for_targets(origin, targets, conductors)` fills sane discipline
defaults; the bin overrides the field(s) for the cast discipline:
- **Stormstrike**: build `conductors` from nearby conductive entities/terrain
  (pos + a `conductance01`; `is_self` for the bender) — the chain nodes.
- **Heavy Brine**: `brine` from the medium at the target (salinity/mass), and
  `brine_tech` from the gesture (Crush/Anchor/Drag/Wall).
- **Thermal Phase**: `thermal` = the target material's `{freeze_c, boil_c, temp_c}`,
  `thermal_target` = desired `Phase`.

`targets` for `resolve_targets` come from the combat broadphase (entities within
reach as `Target { pos, radius }`).

## Applying the outcome (bin)
`BendOutcome` is **data** — the bin applies world mutation per SHARED_RULES:
- `hits` → damage/impulse to those target indices, scaled by `strength01`.
- `effect`:
  - `Stormstrike(plan)` → arc VFX along `plan.path`; if `grounded_on_self`, the
    bolt hits the bender.
  - `HeavyBrine(eff)` → apply `force` / `slow01` / `anchor01` to the hit targets.
  - `ThermalPhase { heat, .. }` → `apply_heat(material, heat)` on the target block/entity.
  - `Unwired(d)` → no-op this round (wire Information/Elemental later).
- `backlash` → self-damage / stagger / resonance loss to the bender.
- `resonance_after` → store back as the bender's resonance for next tick.

## Where to call it
In `bending_runtime`'s per-frame tick, after the gesture resolves to an
`EffectIntent` (or directly via `tick(..)`). One `BendOutcome` per committed cast;
hold/no-gesture frames produce `None` (resonance still `build`/`decay`s separately).

---

## Base note (important)
The dispatch targets r332, where `bending_runtime` already returns an
`EffectIntent` to be composed. **This tree (r330b) has an earlier `bending_runtime`**
that composes `affinity_access + resonance_state + hybrid_form + gesture_resolve`
into a `BenderTick` and does **not** reference `player_bending`/`EffectIntent`/the
disciplines. So I shipped the composition as a **new file** `sim::bending_compose`
rather than editing that divergent `bending_runtime`. On the r332 base, move
`resolve_bend`/`tick`/`BendOutcome`/`BendWorld` into `bending_runtime` (or keep the
module and have `bending_runtime` call it) — the bodies are unchanged.

## Honesty
- Pure/deterministic/panic-free: no IO/clock/RNG/`unwrap`/`expect`; all the cores it
  calls are already pure.
- Information & Elemental disciplines surface as `Unwired` — the brief named only
  Stormstrike/HeavyBrine/ThermalPhase effect cores; I did not invent effects for
  the other two.
- The gate uses a service-registry-stripped copy of `player_bending` **in the
  harness only** (the shipped file is untouched). The stripped tail is the
  `service_registry` wiring block, unrelated to the bending API my module uses;
  removing it keeps the standalone gate from dragging in the registration hub.
- No edits to any existing module; one `pub mod` line added (coordinator reconciles).
- Full `cargo test -p unendlich-engine --features "render-core egui" --lib bending_compose`
  is the coordinator's on-merge confirmation; the flatten harness is the in-sandbox
  substitute and exercises the same real sources.
