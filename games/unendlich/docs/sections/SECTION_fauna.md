# SECTION: Fauna & Animals

*Plan of record for the fauna domain. Derived from `docs/specs/fauna_behaviors.docx`,
`fauna_locomotion.docx`, and `animal_voice_lauscher.docx`, plus the merged modules read in
the tree (r74-03 survey).*

## 1. What this section is

Fauna is the living animal layer of the voxel world: a roster of ~20 species (passive,
neutral, hostile, and ecological specialists) that each run a coherent behavioural FSM,
move with species-appropriate gaits, perceive the player and each other, and occupy
ecological roles (predator/prey, grazing pressure, taming). The player-facing goal is a
world that feels *inhabited and reactive* — a deer that pronks and flees, a wolf pack that
targets it, a parrot that learns your voice, and a blind Lauscher that hunts you by sound —
rather than texture-mapped stat blocks.

## 2. Already built

Pure-sim cores (all deterministic, no IO/clock/unseeded-RNG):

- **`sim/fauna_behavior.rs`** — temperament-driven behaviour FSM. `step(prev, Temperament, &Stimulus, dt) -> FaunaState`, composing `behavior_arbiter`; temperament filter (Passive never Attacks, Neutral attacks only when provoked, Hostile/Specialist eager) + dwell-gate hysteresis (`MIN_DWELL_S=0.8`, urgent bypass on threat>0.75 / injury>0.80). `FaunaBehavior {Idle, Graze, Wander, Flee, Approach, Threaten, Attack, Retreat}`.
- **`sim/fauna_gait.rs`** — body-plan proportions + gait selection for 20 plans. `BodyPlan`, `proportions(plan) -> Proportions` (ratios vs torso=1.0, from locomotion §D), `gait_for(plan, speed01) -> GaitKind`, `stride_phase(prev, speed, stride_len, dt)`.
- **`sim/perception.rs`** — awareness + "am I being looked at." `perceive(...)`, `being_watched(player_pos, player_yaw, mob_pos, &PerceptParams)`, `in_blind_spot(...)`, `blind_half(size, p)` (rear/under blind cone widens with body size — the crouch-to-hide / blind-dragon mechanic).
- **`sim/predator_prey.rs`** — reactive read-only choice `-> Act {flee|hunt|carry on}` from perceived neighbours, fixed priority.
- **`sim/predation.rs`** — perception-gated hunting: needs eyes-or-(ears+sound) to locate prey, jaws to eat. The rule the dragon/predator brain consults.
- **`sim/popdyn.rs`** / **`popdyn_step.rs`** / **`popgen.rs`** — deterministic colony dynamics (fed colony breeds, starved dies, grazing draws down grass) — the ecosystem-shape test harness.
- **`sim/taming.rs`** — `progress`: feed-driven taming meter `0..1` (gain = affinity × feed_quality, clamped).
- **`sim/bond_tame.rs`** — `advance(Bond, Event, trust) -> (stage, trust)`: the slow trust-climb that gates a creature's first taming.
- **`sim/lauscher.rs`** — the blind echo-hunter creature core (near-zero `vision_coverage`, hunts by sound).
- **`sim/mob_spawn_table.rs`** — `pick(biome, light, &[(species,weight)], roll) -> Option<species>`: weighted contextual spawn selection.
- **`sim/ai_budget.rs`** — deterministic brain-tick curfew (cap/curfew distant roamer ticks; the headline fauna cost control).
- **`sim/mob.rs`** — the `Mob` brain: a `Character` body + kind + tiny integer-deterministic AI driven through the *same* `step_character` kernel as the player (splitmix64-seeded). Self-contained; integration is a scoped follow-up.
- **`sim/mob_memory.rs`**, **`sim/forage_sim.rs`/`forage_memory.rs`/`forage_target.rs`** — memory + foraging cores.

World/bin-side (not pure):

- **`world/entity.rs`** — `EntityStore`/`EntityKind` substrate (flat global store, f32 continuous position). Defined but **UNWIRED** (no spawning/render/tick from real code yet).
- **`world/spawn_streaming.rs`** — `SpawnStreamer::update(center, budget) -> StreamDelta` (chunks to populate / despawn, Chebyshev radius, budgeted release). Pure chunk bookkeeping; holds no creatures.
- **`world/ecosystem.rs`** / **`ecosystem_census.rs`** / **`mob_persistence.rs`** — `habitat_weight` placement, census, `SpawnLedger` persistence.

Specs in tree: `fauna_behaviors.docx` (roster + FSMs + ecological roles + drops), `fauna_locomotion.docx` (§D body proportions, joint maps, gaits, air/water), `animal_voice_lauscher.docx` (parrot phrase-learning + Lauscher echo-hunter + degradation loop), plus `dragon_voice.docx` (shared voice dep).

## 3. The big sub-sections to build

### 3a. Species roster data (pure)
Scope: encode the ~20 catalogued species as data — temperament, `BodyPlan`, home-range, detection radii, drop table, habitat weights — wiring `fauna_behaviors.docx §2–5` numbers into one table the FSM/gait/spawn cores read. Seams: `fauna_behavior`, `fauna_gait`, `mob_spawn_table`, `ecosystem::habitat_weight`. Deps: none (pure data module).

### 3b. Per-species FSM specializations (pure)
Scope: the spec FSMs go beyond the generic temperament filter — deer RUT/seasonal, rabbit warren-bound IDLE + freeze-on-alert, pack coordination for wolves. A `species_fsm` layer that refines `FaunaBehavior` per species. Derives from `fauna_behaviors.docx §2–4`. Seams: `fauna_behavior`, `perception`. Deps: 3a.

### 3c. Locomotion rig consumption (render/bin)
Scope: feed `fauna_gait::proportions` + `stride_phase` into the forge rig to actually animate joints; spine flex, ear-tracking, pronk, landing lurch from `fauna_locomotion.docx §D`. Seams: forge/creature rig, render. Deps: 3a; **render — compile-on-end.**

### 3d. Animal voice reactions (pure core + audio backend)
Scope: universal acoustic-event response table (`animal_voice_lauscher.docx §2`) — amplitude/freq/duration thresholds → reaction per species. Pure core maps acoustic event → behaviour nudge; the audio capture/playback is backend. Seams: `fauna_behavior`, voice echo system (shared with `dragon_voice`). Deps: voice/audio backend — **not pure (audio).**

### 3e. Parrot phrase-learning (pure core + audio)
Scope: rotating 6-slot phrase database, learning-priority rules (player voice > other parrots @80% fidelity), replay. `animal_voice_lauscher.docx §3`. Pure core = database + priority + fidelity math; audio = capture/replay. Deps: 3d.

### 3f. Lauscher echo-hunter + degradation loop (pure core + audio)
Scope: Lauscher stores/replays heard phrases corrupted; the parrot↔Lauscher degradation loop (`animal_voice_lauscher.docx §1` "most important interaction"). `sim/lauscher.rs` exists as the creature; this adds the phrase-corruption + loop. Deps: 3e, `lauscher.rs`.

### 3g. Ecology integration (pure + bin wiring)
Scope: tie `predator_prey`/`predation`/`popdyn` into live spawned mobs via `spawn_streaming` + `EntityStore`; grazing pressure feeds back into biome B (deer keep clearings trimmed). Seams: `world/entity.rs` (needs wiring), `spawn_streaming`, `ecosystem`. Deps: entity store wiring — **bin/compile-on-end.**

### 3h. Taming progression (pure)
Scope: connect `taming::progress` + `bond_tame::advance` into a per-species taming flow (which species tameable, feed items, bond events), incl. parrot "NEUTRAL if mishandled." Seams: `taming`, `bond_tame`, 3a. Deps: 3a.

## 4. Prioritized roadmap

1. **Species roster data table** (3a) — *pure.* Unblocks 3b, 3c, 3h and gives spawn/gait/FSM real numbers. Do first.
2. **Per-species FSM specializations** (3b) — *pure.* Deer rut/seasonal, rabbit warren, pack logic. Needs 1.
3. **Taming progression per species** (3h) — *pure.* Small; needs 1. Independent of render.
4. **Animal voice reaction core** (3d, pure half) — *pure.* The acoustic-event→reaction table; defer audio backend.
5. **Parrot phrase-learning core** (3e, pure half) — *pure.* Database + priority + fidelity; needs 4.
6. **Lauscher degradation loop core** (3f, pure half) — *pure.* Needs 5 + `lauscher.rs`.
7. **Ecology integration** (3g) — *bin/compile-on-end.* Wire `EntityStore` + `spawn_streaming` + predator/prey/popdyn into live mobs. Unblocks everything visible in-world.
8. **Locomotion rig consumption** (3c) — *render/compile-on-end.* Animate the gaits. Needs 1 + rig.
9. **Audio backend for voice** (3d/3e/3f audio halves) — *not pure (audio dep, shares `dragon_voice`).* Last; biggest external dependency.

Pure-sim items (1–6) can run as standard single-module worker threads now; 7–9 are compile-on-end render/bin/audio.

## 5. Open questions / decisions

- **Audio backend**: animal voice (3d–f) shares `dragon_voice`'s audio dependency. Is the audio backend in scope this cycle, or do we ship the pure cores + stub the audio and defer? This gates items 4–6's "audio half."
- **EntityStore wiring owner**: `world/entity.rs` is defined but UNWIRED. Ecology (3g) and locomotion (3c) both need live entities. Should entity-store wiring be its own dispatch round before 3g/3c, or folded into 3g?
- **Species count**: spec catalogues ~20; confirm the v1 roster subset (which species ship first) so 3a's table isn't over-scoped.
- **Seasonal/time-of-day source**: deer RUT and parrot dawn/dusk forage need a season/time signal — confirm the canonical source (atmos_params time_of_day vs a world-calendar module).
