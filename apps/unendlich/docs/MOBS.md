# Entities & mobs: three concepts → one render path

**Status (2026-05-31):** first *hardware-verified* render of the full entity
stack. A test dragon shows its body plan — torso, two wings, two eyes, jaw —
and a sim Seeker renders red, on the M2. Everything below the bin is pure +
pixel-proven in the headless graphics harness; the bin wiring is compile-checked
under `render`. **Not yet done: movement, and spawning from eggs / the ambient
spawner.** The things on screen are *static test spawns* placed near the player.

## The problem this arc solved

The engine had grown three unrelated notions of "a thing in the world", none of
which could be drawn:

| Concept | Lives in | What it is | Came from |
|---|---|---|---|
| `Entity` | `world/entity.rs` | renderable cube-ish thing in an `EntityStore` (`Critter`, `DroppedItem`, `Particle`) | the render side |
| `Creature` | `creature.rs` + `spawn.rs` | an **anatomy** — torso + parts (jaw/eye/wing/foot/tail), grown from a spawn egg | the gameplay side |
| `Mob` | `sim/mob.rs` | a **deterministic sim agent** (`Walker`/`Seeker`/`Flyer`/`Grazer`) with fixed-point body + AI | the simulation side |

Three structs, three modules, zero pixels. The arc's job was to make all three
reach the screen **without** merging them — each keeps its own reason to exist,
they just all *bake* to one renderable form.

## The one form they all bake to

Everything funnels into `EntityVertex { position: [f32;3], color: [f32;3] }`
(24 bytes, interleaved) and a `u32` index buffer. One vertex layout, one
pipeline, one draw. The bakers are all pure functions in `world/entity.rs`:

| Baker | Input | Geometry produced |
|---|---|---|
| `snapshot_to_vertices(&snap, color_fn)` | an `EntityRenderSnapshot` of the `EntityStore` | one box per entity (its AABB) |
| `creature_to_vertices(&Creature)` | a `Creature` with optional `Anatomy` | torso box + one box per **intact** part at its body-local offset |
| `mobs_to_vertices(&[Mob])` | the sim's mobs | one box per mob, sized + coloured by `MobKind` |

Shared helper `push_box(center, half, color)` emits the 8 corners + 36 indices
with a single fixed winding, so every baker agrees on face orientation. Colours
come from small exhaustive tables: `part_color(Part)` (eyes near-black, jaw bone,
wings light, feet dark, fin blue, tail body-ish) and `mob_appearance(MobKind)`
(Seeker predator = larger + red, Grazer prey = small + tan, Flyer bluish, Walker
neutral). Destroyed parts (`hp == 0`) are simply skipped — tear off a wing and
its box disappears.

To draw N things you bake each, then concatenate, rebasing the later index
buffers by the running vertex count. The bin does exactly that per frame:
entities → creatures → mobs → one `upload_entities`.

## The pipeline

`render/entity_pipeline.rs` — `EntityPipeline` is a thin sibling of the chunk
pipeline: float `pos[3]@0 / color[3]@12` stride-24 vertices, depth `LESS` (so
mobs occlude correctly against terrain), cull `NONE`, and it **reuses the chunk
camera descriptor-set layout** (it only needs binding 0, the camera UBO). Host
buffers grow on demand; `record(cmd, camera_set)` is one `draw_indexed`.

`render/world_renderer.rs` owns the pipeline (created right after the chunk
pipeline, reusing its descriptor-set layout + the device memory props) and draws
it inside the main pass, after the chunk draws and before the egui overlay.
`upload_entities(ctx, verts, indices)` is infallible host-buffer growth.

## How it's verified (and what isn't)

**Verified in-sandbox** (pure unit tests + the lavapipe offscreen graphics
harness, which renders to RGBA8 and reads pixels back):

- `creature_to_vertices`: a dragon bakes 1 + 8 boxes (72 v / 324 i, indices in
  bounds); its extent — especially the Z **wingspan** — is >3× the Lauscher's;
  destroying a wing removes exactly 36 indices.
- Rendered through the *real* `entity.vert` / `entity.frag`: a dragon covers >3×
  the lit pixels of a Lauscher (front-on, ortho camera); a Seeker mob is reddish
  at frame centre. So the bake output is provably renderable, not just plausible.
- The pipeline, renderer hook, and bin wiring all **compile** under `--features
  render`.

**Needs the machine:** the actual on-screen result (confirmed 2026-05-31), real
GPU perf, and anything `ui-overlay`-gated (the egui-side held-item selection).

## The brick sequence

| Patch | Brick | What it added |
|---|---|---|
| 211 | `test/entity-render` | harness proof that `entity.vert/frag` draw a mob in its colour (added UBO/descriptor support to the harness) |
| 212 | `render/entity-pipeline` | the `EntityPipeline` itself (compile-checked) |
| 213 | `render/entity-wire` | renderer owns + draws it; bin spawns 3 test Critters and bakes them each frame |
| 214 | `render/creature-geometry` | `creature_to_vertices` + `part_color` + `push_box`; multi-part bake, pixel-proven |
| 215 | `render/creature-in-bin` | bin spawns + bakes a test dragon |
| 216 | `render/mob-bridge` | `mob_appearance` + `mobs_to_vertices`; sim `Mob` → renderable, pixel-proven |
| 217 | `render/mob-in-bin` | bin spawns + bakes a Seeker + Grazer via the bridge |

## What's left

- **Movement.** The test spawns are static. Mobs already run the same
  `step_character` kernel as the player (see `MOVEMENT.md`), so driving them is a
  matter of ticking the sim + feeding AI intents in the bin — best done after the
  static render was confirmed (it now is).
- **Spawning.** `spawn::resolve_use` already turns a held spawn egg into a
  `Creature` (pure, tested), but the bin's place path reads the held item through
  the `ui-overlay`/egui layer, which only builds on the full toolchain — so that
  wiring lands on the real machine, not in the sandbox.
- **Ambient population.** Replace the hand-placed test mobs with
  `sim::spawn::spawn_region` output.
- **Part orientation.** Parts currently sit at body-local offsets with no facing;
  rotate them by the body's yaw so a creature turns to face its heading.
