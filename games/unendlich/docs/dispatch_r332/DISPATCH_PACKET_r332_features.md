# DISPATCH PACKET — r332 FEATURES (5 threads)

Five feature/fix threads. All read `docs/dispatch_r330/SHARED_RULES.md` FIRST (pure cores,
no bin/glfw, in-sandbox-validatable, return changed `.rs` + MANIFEST + any PNG proof).
Base = latest `unendlich-r332-*-CHECKED.zip`. Each carries a FILE ALLOW-LIST; all five edit
disjoint files (one `pub mod` line each where needed — coordinator reconciles).

**Merge order on return:** `swords-creative → sound → structures → bending → dragon`
(additive new-file cores first; dragon edits the shared `dragon_forge` skin path, gated last).

---

## T-swords-creative — give the legendary swords a creative slot so they're testable
**WID:** `r332-swords-creative`
**Why (operator blocker):** r332 merged `ItemKind::Legendary` + `legendary_loot`, but the creative
picker only enumerates `ITEMS: &[BlockId]` and `spawn_egg_palette()` — there's **no creative entry
for legendaries**, so they can't be put in hand and tested. Close the pure half.
**Build:** in `item.rs` (additive, mirror `spawn_egg_palette`@300):
- `pub fn legendary_palette() -> Vec<ItemKind>` — a curated sample set: each `WeaponClass` ×
  the 6 elements (`element_idx 0..6`) × a representative `tier` (e.g. 0 and a high tier), as
  `ItemKind::Legendary(LegendarySpec{ seed: <stable per entry>, class, element_idx, tier })`.
  Stable deterministic seeds so the palette is reproducible. Keep it a sensible size (≤ ~48).
**Anchors:** `item.rs` (`spawn_egg_palette`@300, `blocks_palette`@305, `ItemKind::Legendary`,
`LegendarySpec`), `sim/legendary_weapon.rs` (`WeaponClass::ALL`), `docs/CREATIVE-PICKER.md`,
`render/world_renderer.rs` (`Tab` enum @~3008, the `ITEMS`/palette match — READ ONLY).
**Verify:** ≥6 tests (palette non-empty + bounded; every entry is `ItemKind::Legendary` with a
valid element_idx 0..6 and `display_name`/`display_color` non-trivial; `spec.weapon()` forges for
each; deterministic; no dup specs). `cargo test --features "render-core egui" --lib` (touches
`item.rs`) or flatten with a `forge` stub.
**Bin handoff (`HANDOFF_swords_creative.md`):** propose a `Tab::Legendaries` arm in
`render/world_renderer.rs` (mirror `Tab::SpawnEggs => spawn_egg_palette()` →
`Tab::Legendaries => item::legendary_palette()`), the tab button + title string, and the
in-hand/place path (a legendary in hand → equip, not place — ties to `spawn.rs::resolve_use`'s
`Legendary => Nothing` arm). Propose, don't wire.

---

## T-sound — occlusion-aware spatial audio (sound-engine improvement)
**WID:** `r332-sound`
**Why:** the pure spatial mixer `sim::spatial_audio` (`distance_gain`, `mix_gains`,
`menu_stem_layout`) attenuates by **distance only** — sound passes through walls undimmed. Add
**occlusion**: solid voxels between listener and source muffle the sound (low-pass + gain drop),
the next step toward a physically-legible mixer (`AUDIO_SPATIAL_MIXER.md`,
`docs/design/acoustic_perception_and_poi.md`).
**Build:** extend `sim::spatial_audio` (additive, under a `// r332-sound` header) and/or a new
`sim::audio_occlusion`:
- `occlusion_gain(solid_blocks_between: u32, per_block_db: f32) -> f32` — gain multiplier from the
  count of solid voxels on the listener→source segment (passed in; the bin raymarches the voxel
  grid — that's glue). Pure dB→linear with a floor.
- `occluded_mix_gains(listener, stems, falloff, occlusion: &[u32]) -> Vec<f32>` — `mix_gains` ×
  per-stem `occlusion_gain`. Keeps the existing API; the new one layers occlusion on top.
- Optional: a muffle/low-pass *cutoff* hint per stem (`muffle_cutoff_hz(solid_blocks) -> f32`) the
  bin can feed a filter, since occlusion is spectral not just level.
**Anchors:** `sim/spatial_audio.rs` (`distance_gain`@24, `mix_gains`@31, `menu_stem_layout`@40),
`sim/acoustics.rs` (`effective_hearing`@142, `spatial_impression`@162), `handoff/AUDIO_SPATIAL_MIXER.md`,
`docs/design/acoustic_perception_and_poi.md`, `docs/sound_genome.md`.
**Verify:** ≥6 tests (0 blocks → gain 1.0; gain decreases monotonically with block count; floor
respected; `occluded_mix_gains` == `mix_gains` when occlusion all-zero; dB math finite/clamped;
deterministic). Self-contained → `rustc --test`. No audio device needed (pure math).
**Watch:** the raymarch (counting solid blocks on the segment) is **bin glue** — propose it in a
handoff; keep this module pure (counts passed in).

---

## T-structures — rare-structure placement selector (wire the rare structures in)
**WID:** `r332-structures`
**Why:** the structure infra exists — `world::structure_place` (`SiteFeatures`, `StructurePrefs`,
`suitability`, `rank_sites`, `Candidate`) and `world::structure_grammar` (`expand`, `to_placements`)
and `structure_template` — but there's **no rarity-gated selector** deciding *which* rare structures
appear and *how rarely*, so rare structures don't actually populate the world.
**Build:** new `world::rare_structure` (`crates/unendlich-engine/src/world/rare_structure.rs`) — a
pure, deterministic per-region selector:
- `enum RareKind { … }` — a small set of rare structure types (e.g. Obelisk, Shrine, Ruin,
  Monument) keyed to grammar axioms/templates.
- `struct RarityTable` — per-kind spawn weight + min spacing (regions between occurrences).
- `select_for_region(region: [i32;2], world_seed: u64, biome: u8, table: &RarityTable)
  -> Option<RarePlacement>` — deterministic: hash `(region, seed)`, roll against the rarity weight,
  pick a kind allowed in `biome`, and a within-region offset + rotation. Honors min-spacing so two
  rares don't cluster. Pure (no IO/clock; seed-driven splitmix like `legendary_weapon`/`gpu`).
- `RarePlacement { kind, origin, rotation, axiom }` — feeds `structure_grammar::to_placements` /
  `structure_place::rank_sites` (the bin/worldgen calls those; propose the wiring).
**Anchors:** `world/structure_place.rs` (`rank_sites`@73, `suitability`@51, `Candidate`@64),
`world/structure_grammar.rs` (`expand`@18, `to_placements`@61), `world/structure_template.rs`,
the deterministic generator + `assets/biomes/`.
**Verify:** ≥6 tests (selection deterministic for a `(region,seed)`; rarity weight ~holds over a
grid of regions (low frequency); min-spacing respected; biome gating; different seeds → different
placement; rotation in range). Self-contained → `rustc --test`.
**Bin handoff (`HANDOFF_rare_structures.md`):** propose where worldgen calls `select_for_region`
during chunk/region generation and feeds the placement to the grammar/template stamper.

---

## T-bending — wire the designed bending mechanics into the runtime (from the docs)
**WID:** `r332-bending`
**Why:** `SECTION_bending.md` describes the full Resonance system and the pure modules all exist —
`affinity_access` (env gate), `gesture_resolve` (gesture grammar), `bending_charge` (resonance
resource), `player_bending` (orchestrator — **emits intent only**), `bending_targeting`,
`bending_feedback`, `bending_stormstrike`/`heavy_brine`/`thermal_phase`/`information`. The gap:
the intent isn't composed end-to-end — `player_bending::step` returns an `EffectIntent` that
nothing turns into resolved targets + a discipline effect + feedback.
**Build:** in `sim::bending_runtime` (extend `BenderTick`/`update`, under a `// r332-bending`
header) — the pure composition that ties the existing modules into one tick:
`EffectIntent → bending_targeting::resolve_targets → the discipline effect
(bending_stormstrike/heavy_brine/thermal_phase per `Discipline`) → bending_feedback::backlash`,
producing a pure `BendOutcome { hits, effect, backlash, resonance_after }`. **Compose existing
APIs only — do not re-implement the disciplines.** Respect `affinity_access` gating + `bending_charge`
spend already in `player_bending`.
**Anchors:** `docs/sections/SECTION_bending.md` (the full map of built modules + APIs),
`sim/player_bending.rs` (`EffectIntent`, `Discipline`, `step`), `sim/bending_targeting.rs`
(`resolve_targets`, `cone_hits`, `chain_hits`), `sim/bending_stormstrike.rs` (`Conductor`,
`StrikePlan`), `sim/bending_heavy_brine.rs`, `sim/bending_thermal_phase.rs`,
`sim/bending_feedback.rs` (`Backlash`, `combine`), `sim/bending_runtime.rs` (`BenderState`,
`BenderTick`, `update`@71/`update_from_input`@117).
**Verify:** ≥6 tests (a committed intent yields resolved hits via targeting; each `Discipline`
routes to its effect; overdraw/mispredict produces backlash via `bending_feedback`; an
env-gated-off discipline yields no effect; resonance is spent/decayed consistently; deterministic).
`cargo test --features "render-core egui" --lib bending_runtime` or flatten the composed modules.
**Watch:** pure composition, no new mechanics; if a discipline's apply needs world mutation, return
it as **intent/outcome data** (the bin applies) — don't reach into world state here.

---

## T-dragon — fix the body-part bulge/balloon in the forge animation
**WID:** `r332-dragon`
**Why (§4c):** head/wings **balloon then shrink** during animation. The forge skin path
`sim::dragon_forge::skin(model, anim) -> Vec<ForgeBox>` builds `OBox` solids whose `half: [f32;3]`
extents are scaled by per-part factors (`b2`, `ml`, `jd`, `jw`, …); something in the anim path
modulates those so box extents grow beyond their rest size over the cycle. Not yet diagnosed.
**Build (diagnose then fix, in `sim/dragon_forge.rs`, scoped to the skin/anim scale path under a
`// r332-dragon` header):**
- First **reproduce numerically**: a test that runs `skin` across a full `ForgeAnim` cycle and
  asserts each box's max extent never exceeds its rest extent by more than a small tolerance
  (e.g. 15%) — this both proves the bug and becomes the regression guard.
- Then find the compounding scale (likely an anim factor multiplied into `half` that should apply
  to the *pose/transform*, not the *extent*, or a breathing/`appendage_ripple` term applied
  multiplicatively without a clamp) and fix so extents stay bounded while the pose still animates.
- If a clamp is the right fix, factor a tiny pure helper `clamp_extent(rest, animated, max_grow)`
  (reuse `util::mathx::clamp` from r332) rather than an inline magic number.
**Anchors:** `sim/dragon_forge.rs` (`skin`@855, `OBox`@61, the head/jaw boxes @255-270),
`sim/body_scale.rs`, `sim/mob_limb_girth.rs`, `sim/appendage_ripple.rs`/`batch_appendage_ripple.rs`,
`sim/breathe.rs`, §4c, `handoff/ENTITY_BODY_SEGMENTATION.md`, `handoff/BODY_PART_MESHING.md`.
**Verify:** the extent-bound test above + ≥5 more (rest pose extents unchanged; a static anim frame
== rest; wings/head specifically bounded; symmetry preserved L/R; deterministic). **Ship a headless
PNG** (CPU rasteriser collage of the dragon at several anim phases, per SHARED_RULES §3 "failures as
pictures") showing the silhouette no longer balloons. `cargo test --features "render-core egui"
--lib dragon_forge` or flatten `skin` + its deps.
**Watch:** preserve the intended motion (breathing, wing flap) — bound the *extent ballooning*, not
the animation itself. No render/bin edits; the mesher consumes `ForgeBox` unchanged.

---

### Coordinator checklist
- [ ] Cut `worker-<WID>-r332.zip` = base + `WORKER_ID` + brief + `SHARED_RULES.md`.
- [ ] On return merge `swords-creative → sound → structures → bending → dragon`, gate each @1.96,
      reconcile the mod lines, wire the proposed bin call-sites on-device (creative tab, occlusion
      raymarch, worldgen structure call, bending apply), roll `unendlich-r333-merged.zip`.
