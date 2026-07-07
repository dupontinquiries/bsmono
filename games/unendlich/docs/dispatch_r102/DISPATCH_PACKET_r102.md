# DISPATCH PACKET r102 — 6 threads

Read `SHARED_RULES.md` first. Each thread is independent; deliver pure core + a headless
proof. State of the world you're building on (r102 merged tree):
- The live dragon IS the forge (`sim::dragon_forge`): `world::entity::forge_to_vertices`
  builds the mesh via `skin(model, &ForgeAnim{gait_phase, anim_time, head_yaw, head_pitch})`.
  Forge gait/idle works in isolation (see `examples/sim_collage.rs`). The throat "black box"
  was removed. Wings are still under-modelled (sparse boxes, no membrane).
- Creatures from images: `sim::creature_from_image` (surface traits → `place_surface_glow`).
- Shader-from-image: `sim::shader_from_image` (PBR material estimate + `synth_pattern_glsl`
  image→.frag codegen, glslang/spirv-val clean).
- Mob cognition: `sim::fauna_mind` (object-permanence brain, registered in the service registry).
- Perf reference (RD=5, ~382 chunks): FPS 40, cpu draw=79.6ms (HIGH), tick_sim=14.7ms,
  gpu main p99=52ms. draw-zone cost is the standout.

Reference docs (in `docs/specs/`): `physical_mob_engine.docx`,
`elemental_spiritualism_reference.docx`, `mob_brain_object_permanence.docx`,
and `docs/incoming-r94/new-docs/shader_synthesis_guide.docx`.

---
## B1 — BUGHUNTING
GOAL: find and FIX real correctness bugs in pure engine core; each fix lands with a test
that fails before / passes after. Don't chase the bin.
LOOK AT: integer/float overflow & NaN paths in worldgen and physics; off-by-one in chunk/
LOD boundary math; ordering/`%`-wrap bugs (e.g. phase accumulators); `HashMap` iteration
nondeterminism leaking into gameplay; any `as` cast that can truncate world coords; eviction/
cache bookkeeping in `render::world_renderer` reachable without GPU. The in-flight mesher
results after a world reset can briefly upload stale geometry — propose a "world epoch" stamp
(discard mesh results from a prior epoch) as a pure change + test if you can isolate it.
DELIVER: `sim`/`world` fixes + regression tests. A short `BUGS_FOUND.md` listing each
bug, root cause, and the test that pins it.
VERIFY: `cargo test --features gpu --lib` for the modules you touch; render gate green.

## S1 — SHADERS: dirt-on-dragons + dynamic lights
GOAL (two pure-core deliverables, GLSL must validate):
1. DIRT/GRIME accumulation on creatures: a pure `sim::creature_grime` that, given a
   creature's recent biome/activity history, produces a grime factor + tint that the forge
   skin or material can consume (data only — no live wiring). Plus a `.frag` snippet (via the
   `shader_from_image` codegen style or hand-authored) that darkens/roughens lower surfaces
   ("dirt sticks to the belly/feet"). Headless proof: a swatch PNG of clean→dirty.
2. DYNAMIC POINT LIGHTS from emissive sources (forge glow boxes, eyes, fire): a pure
   `render`-side data structure + math for gathering N nearest point lights per fragment
   region and a `.frag` that applies them (attenuation + clamp). NO descriptor/pass wiring —
   codegen + a CPU reference that the test checks against. Validate all GLSL.
REFERENCE: `shader_synthesis_guide.docx`, `sim::shader_from_image`. Keep emitted floats
fixed-point (no inf/nan/exponent literals) per the existing codegen.

## I1 — ITEMS: model-in-hand (first-person held item)
GOAL: pure geometry for a first-person held-item/tool model — given an item kind + the
camera basis, produce the held model's vertices/transform in view space (a viewmodel pose:
slight bob from a walk phase, sway from look delta). Data + math only; the bin will feed the
camera + draw it. Headless proof: render the viewmodel pose at a few walk phases to a PNG
collage. Look at the existing `item` module for item kinds. Don't touch the bin's input/draw.
DELIVER: `sim::viewmodel` (or `item::viewmodel`) + tests (bob/sway bounded, deterministic) +
a handoff `.md` describing exactly how the bin should call it.

## M1 — MOBS
GOAL: deepen mob behaviour on top of `sim::fauna_mind` (object permanence) and the
`physical_mob_engine` / `mob_brain_object_permanence` docs. Candidates (pick a coherent
slice, pure + tested): memory of last-seen food/threat positions decaying over time; simple
flocking/spacing so mobs don't stack; state machine (forage→flee→rest) driven by vitality +
senses; per-species behaviour params. Register any new service in the service registry and
keep the smoke test green.
DELIVER: `sim::<mob_behaviour>` + tests asserting the behaviour (e.g. "remembers food after
it leaves view, forgets after T"); a numeric proof, no bin.

## WG1 — WORLDGEN: caves
GOAL: a real multi-scale cave system in pure worldgen. `UNENDLICH_MULTI_SCALE_CAVES` +
`UNENDLICH_CAVE_THRESHOLD`/`_FBM` flags exist but caves default off. Build a deterministic
cave carve (worm/tunnel + chamber noise) in the generator that's seed-stable and bounded
(no runaway carving, surface integrity preserved). Headless proof: a 2D vertical-slice PNG of
a seed's caves + assertions (connected void fraction in range, no carving above surface,
seed-varying). Look at `world::pressure_sampler` / the generator's density function.
DELIVER: cave carve in the generator (feature/flag-gated to stay off by default) + the slice
example + tests. Don't regress the existing surface tests.

## WG2 — WORLDGEN: plants
GOAL: richer flora distribution in pure worldgen, building on `world::plant_lifecycle` /
`world::stem`. Biome-appropriate plant placement (density, species mix, clustering),
deterministic + seed-stable, with the elemental/biome reference in mind. Headless proof: a
top-down placement map PNG (like the seed-map preview) showing plant distribution per biome +
assertions (right species in right biome, density in range, seed-varying, none in ocean/rock).
DELIVER: `world::<flora_placement>` + the map example + tests. Coordinate file-sharing with
`plant_lifecycle` via a WID header; one `pub mod` line only.

---
RETURN: per SHARED_RULES §6. Coordinator merges, gates, and the operator runs on M2.
