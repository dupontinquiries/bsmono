# SECTION — Bending (Resonance)

## 1. What this section is
Resonance Bending is the player's core verb set. Per `resonance_bending_spec.docx`
v2.0 it **replaces the action-hotbar model**: no chi bar, no cooldowns, no slotted
moves, no unlock tiers. What a player can do is gated entirely by seven measured
environmental properties and a *resonance* attunement value; skill is finding or
creating the physical conditions for a technique, then committing the right
gesture. The player-facing goal: an emergent, physics-legible bending system where
"what's possible right now" is read from the world, not a menu.

## 2. Already built (merged modules — real APIs, read in-tree)
All under `crates/unendlich-engine/src/sim/`. All pure (no IO/clock/unseeded-RNG;
no panics), deterministic.

- **`affinity_access.rs`** — the environmental gate. `Fields` = the 7 measured
  properties `h,r,w,p,l,s,b` (humidity, radiant heat, wind, pressure, luminosity,
  seismic index, biotic density); `access(&Fields) -> Access`; `DISCIPLINES = 7`,
  `THRESHOLDS = [0.45,0.55,0.40,0.60,0.70,0.50,0.35]`; `can_access(&Fields, disc)`.
- **`gesture_resolve.rs`** — the gesture grammar. `Stance{Open,Rooted,Flow,Surge,
  Aerial}`, `Gesture`, `Directional`, `Element{Earth,Water,Fire,Air}` →
  `resolve(...) -> TechniqueShape{kind,scale,range,radial,signature}` (signature
  cast amp ×1.5).
- **`bending_charge.rs`** — the resonance resource (replaces chi/cooldown).
  `Resonance{charge01,overdraw01}`; `build(on_rhythm01,dt)`, `spend(cost01)->power01`
  (over-budget fires at reduced power + adds overdraw debt), `decay(dt)`,
  `is_overdrawn()`. Rates: BUILD_RATE 0.5/s, CHARGE_BLEED 0.05/s, OVERDRAW_RECOVERY
  0.10/s, OVERDRAW_THRESHOLD 0.05.
- **`player_bending.rs`** — the orchestrator. `Discipline{Stormstrike,HeavyBrine,
  ThermalPhase,Information,Elemental(Element)}`; `BendInput{stance,gesture,dir,
  element,aim}`; `discipline_for(&BendInput)`; `governing_field_index(disc)` maps a
  discipline to its gating field; `PlayerBending::step(...) -> EffectIntent{
  discipline,shape,power01,aim,committed}`. **This emits intent only.**
- **`bending_targeting.rs`** — spatial resolution. `Target`,`Hit`; `ray_hits`,
  `cone_hits`, `area_hits`, `chain_hits`, `resolve_targets(...)`, `falloff`,
  `chain_default_links`.
- **`bending_feedback.rs`** — "being wrong is costly". `Backlash`; `mispredict(
  committed_aim,actual)`, `overdraw_backlash(overdraw01)`, `discipline_backlash(
  disc,mispredict01,power01)`, `combine`.
- **`bending_stormstrike.rs`** — ⚡ routing discipline. `Conductor`,`StrikePlan`;
  `route(origin,aim,conductors,max_links,max_hop,aim_cos)`, `redirect_cost`.
- **`bending_heavy_brine.rs`** — 🟣 conductive-medium discipline. `density(salinity01)`
  (1000→1240), `BrineState`, `BrineTechnique`, `BrineEffect`, `inertia(&BrineState)`,
  `effect(tech,&state,power01)`.
- **`bending_thermal_phase.rs`** — 🔥 pressure/phase discipline. `Phase`,`Material`;
  `phase_of`, `heat_to_phase`, `apply_heat`, `technique_cost01`.
- **`bending_information.rs`** — ✨ perception-routing discipline. `Signature`;
  `sense(local,others,falloff)`, `disrupt(&mut,power01)`, `mask(&mut,power01)`,
  `coherence_to_power`.

**Coverage vs spec:** the resonance model, the gesture grammar, the env gate, and
all four *architectural* disciplines (Stormstrike/Heavy Brine/Thermal Phase/
Information) exist as decision cores. The **standard elemental** disciplines exist
only as the `Elemental(Element)` enum arm + gating — no per-element effect cores yet.

## 3. The big sub-sections to build

### A. World-effect application — *the keystone gap*
**Scope:** the entire pipeline currently stops at `EffectIntent` + `Hit`s; nothing
turns intent into block/fluid/mob change. Build a pure resolver
`EffectIntent + targets + world-sample -> Vec<WorldEdit>` (block set/remove, fluid
delta, entity impulse/heat/charge), leaving the *mutation* to the bin/world layer.
**Derives from:** spec §"Technique cost"/§ discipline effect tables.
**Seams:** consumes `player_bending::EffectIntent`, `bending_targeting::Hit`;
emits edits the world/fluid/entity systems apply. **Deps:** none new (sits on built
cores). **Purity:** the decision core is pure; the apply step is bin (compile-on-end).

### B. Standard elemental effect cores (Earth/Water/Fire/Air)
**Scope:** per-element technique effects for `Discipline::Elemental(e)` (earth
shaping, water shaping, fire jet, air gust) analogous to the four architectural
cores. **Derives from:** spec §2.2 element affinities + base discipline tables.
**Seams:** feed sub-section A. **Deps:** A (shared `WorldEdit` vocabulary). **Pure.**

### C. Resonance rhythm/recovery input
**Scope:** compute `on_rhythm01` (and recovery gating) that `bending_charge::build`
consumes — from player stillness + environmental alignment, per spec "recovery
requires specific physical behaviours (stillness, environmental alignment)".
**Seams:** input to `bending_charge`; reads player motion + `Fields`. **Pure.**

### D. Attunement / resonance ceiling (the real "progression")
**Scope:** spec is explicit that there are **no move unlocks**; "progression" is the
resonance *ceiling/precision* rising with sustained attunement to conditions. Build
a per-discipline familiarity model raising the ceiling/precision used by
`PlayerBending`. **Derives from:** §1.2 "Resonance State sets scale/precision
ceiling". **Seams:** modifies the charge ceiling / shape scale. **Pure.**

### E. Cross-discipline interaction (Brine medium × Stormstrike)
**Scope:** Heavy Brine suspends a conductive medium that Stormstrike routes through
(upgrade spec). A core that injects brine-`Conductor`s / conductance modifiers into
`stormstrike::route`. **Seams:** `bending_heavy_brine` → `bending_stormstrike`.
**Pure.**

### F. NPC benders + Ryujin Triad faction
**Scope:** AI that drives `BendInput` like a player (situation → stance/gesture/
element/aim), reusing the whole pipeline; plus Ryujin Triad faction behaviour from
the upgrade spec. **Seams:** entity/fauna AI → `player_bending`. **Deps:** A.
**Purity:** decision core pure; perception/entity wiring is bin.

### G. Information-bending observer model
**Scope:** `disrupt`/`mask` change what *observers* are allowed to see; needs an
observer-perception application (per-observer signature visibility). **Seams:**
`bending_information` → AI perception + render. **Deps:** A, F. **Mixed** (pure
decision; render/AI apply).

### H. VFX / audio hooks
**Scope:** map `EffectIntent.shape` + discipline to particle/light/sound cues.
**Seams:** render + audio. **Render/bin — compile-on-end.**

## 4. Prioritized roadmap (each ≈ one worker thread)
1. **A · World-effect application core** *(pure)* — keystone; unblocks everything
   visible. Defines the `WorldEdit` vocabulary the rest target.
2. **C · Resonance rhythm input** *(pure)* — closes the build/recover loop so charge
   is earned, not assumed. Independent of A; do in parallel.
3. **B · Standard elemental effect cores** *(pure)* — needs A's `WorldEdit`.
4. **E · Brine×Stormstrike medium coupling** *(pure)* — needs the built storm/brine
   cores (have them); small, high-flavour.
5. **D · Attunement ceiling** *(pure)* — needs C (rhythm feeds attunement).
6. **F · NPC bender AI core** *(pure decision; bin perception)* — needs A.
7. **G · Observer perception model** *(mixed)* — needs A + F.
8. **H · Bending VFX/audio hooks** *(render/bin, compile-on-end)* — last; needs A.

Unblock chain: **A → {B, F}**, **C → D**, **F → G**, all visuals → **H**.

## 5. Open questions / decisions (operator)
- **`WorldEdit` ownership:** should the apply step live in the existing world/fluid/
  entity systems, or a new `bending_apply` bin module? (Decides where A's pure core
  hands off.)
- **Fields source:** what populates `affinity_access::Fields` per tick — a sampler
  over biome/weather/depth/entity-density? That sampler is a prerequisite for A/C
  and may be its own (bin) task.
- **"Progression" confirmation:** confirm the no-unlock stance — D raises ceiling/
  precision only, never gates new techniques. (Spec says so; flagging because it
  contradicts the usual RPG expectation.)
- **Information bending vs render:** does observer-visibility (G) drive actual
  render culling/illusions, or only AI perception? Scope differs a lot.
- **Ryujin Triad:** how much faction content (spawns, territory, dialogue) is in
  scope for F vs a separate faction section.
