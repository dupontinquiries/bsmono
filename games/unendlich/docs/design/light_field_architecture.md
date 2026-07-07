# Light Field Architecture — one stored, queryable gameplay light, many consumers

> Status: design (r298). The piece several systems already need separately and
> currently fake or hand-roll: a **stored, queryable per-voxel gameplay light
> field** with sky + block channels, fractional transmittance through non-cube
> blocks (leaves/grass), and angle-aware exposure for slow systems. Consumers:
> cave/forest **ecology** (what grows where), **solar armor** (light → gravity
> float), **mob perception** ("eyes" / visibility in shade), **lumen pearls**
> (`lumen.rs` bioluminescence). Sits behind a narrow read seam so it plugs into
> `sim::world_prober` like surface/biome do. Extends the environment-field-stack
> idea introduced when `WorldProber` was built (r296). Cross-links:
> `perception_and_population_behavior.md` (mob perception is local/event-driven),
> `acoustic_perception_and_poi.md` (sibling sense — hearing; light is sight's
> field), `genome_loci_for_behavior.md` (species light tolerances are dispositional
> loci), `dynamic_world_architecture.md` (where the propagation/edit pass lives).

---

## 0. Why this is the keystone (and what already exists)

Light kept surfacing as a dependency across unrelated systems, each solving it in
isolation:

- `world/light.rs` already has the propagation math — `propagate_block_light`
  (per-channel RGB BFS, attenuating, opaque-stopped) and `skylight_columns`
  (top-down sky exposure + overhang bleed). **But it bakes the result into mesh
  vertex colours for rendering** — it is not stored as a field gameplay can query.
- `sim/armor.rs` (solar armor) computes repair from `daylight × sky-exposure ×
  skylight` as a `0..=255` the **caller hand-assembles** from scattered pieces.
- `world/lumen.rs` + `world/cave_flora.rs` each precompute per-cell **sky exposure**
  (via `sampling::exposure`) to charge bioluminescent pearls.
- `sampling.rs` already has `exposure(origin, dirs, max_dist, blocked)` +
  `hemisphere_dirs` / `upward_hemisphere` — **blue-noise hemispherical sky probing**,
  the angle-aware primitive.
- `world/block_predicates.rs` already treats a leaf as **solid-but-not-opaque** —
  a transparency axis separate from collision (`is_opaque`).

So this is **not greenfield**. The propagation, the hemisphere probe, and the
leaf-transparency axis exist. What's missing is (a) **storing** light as a queryable
per-voxel field, (b) keeping it correct under edits, (c) a **read seam** so every
consumer reads one thing, and (d) two small fidelity extensions (fractional
transmittance, baked exposure). This doc unifies the scattered pieces into one
performant foundation.

---

## 1. The one distinction that decides whether this scales

Everything `WorldProber` holds today — surface height, biome — is a **pure function
of the seed**. That is why its memo works by *closure-fill*: a miss recomputes from
the generator, and clearing the cache loses nothing.

**Light is state, not a pure function.** It depends on the *current* blocks,
including player edits (dig a shaft → skylight floods in; place a torch → block light
spreads; seal a roof → a lit cave goes dark). You cannot compute light at a point in
isolation — it is a flood-fill from sources — and you must **not** cache it the way
surface is cached, because a stale value is a *wrong* value (solar armor charging in
a now-dark cave, fungi surviving where the roof just closed).

**The rule that keeps systems from breaking:** `WorldProber` (and every consumer)
**reads** light, never **computes** it. Surface/biome fill-on-miss from the generator
(pure); light is **fetched** from where it is stored and propagated (state). Same
uniform sampling interface to consumers; two different fill mechanisms underneath.

Get this wrong — model light like surface, recompute-on-miss — and you reproduce the
deep-scan freeze (r294–r297) *and* add staleness bugs. Get it right and light reads
stay O(1) forever.

---

## 2. Three layers

### Layer 1 — storage + propagation (the source of truth)

Promote `light.rs` from *bake-into-vertices* to *store-per-voxel*. `Chunk` already
carries `blocks: Vec<BlockId>` (32³); add a parallel light store:

- **Per-voxel, two channels:** skylight + block light. One byte/voxel (nibble each)
  is the Minecraft-style baseline (~32 MB at 1000 loaded chunks — acceptable). See
  §6 on resolution if that's too much.
- **Propagate at chunk load** — amortised, ideally on the mesh worker thread that
  already runs, so it never stalls a frame. Reuse `skylight_columns` (seed open-sky
  cells) + `propagate_block_light` (BFS from emissive blocks), writing the *result
  into the store* instead of into vertices.
- **Re-propagate incrementally on edits** — add/remove BFS, **budgeted** exactly like
  `UNENDLICH_SUBMIT_BUDGET_MS` / upload / plant-build-spread. A bounded amount of
  light work per frame; overflow resumes next frame.

> **The genuinely hard part is the removal BFS.** Adding light (place a torch, open
> the sky) is a simple flood. *Removing* it (mine the torch, place a block that seals
> a shaft) requires un-propagating: tear down the now-unsupported light, then re-flood
> from any remaining sources that bordered the torn region. This is the famously
> fiddly part of every voxel-lighting implementation — more work than the initial
> fill. Plan for it; it's where the engineering actually is.

### Layer 2 — the `LightSampler` read seam (the decoupler)

A narrow trait, implemented by the chunk manager, depended on by `WorldProber`:

```rust
pub trait LightSampler {
    /// (skylight, block_light), each 0..=MAX, at a world voxel. O(1) array read.
    fn light_at(&self, x: i32, y: i32, z: i32) -> (u8, u8);
    /// Baked integrated sky exposure 0..=255 (see §4) — None if not baked here.
    fn sky_exposure(&self, x: i32, y: i32, z: i32) -> Option<u8> { None }
}
```

This is the **"won't break later" decision.** `WorldProber` and every consumer depend
on the *trait*, not on how light is stored. You can change the storage (per-voxel →
octree node → SDF, coarse-in-rock → fine-near-surface) without touching `WorldProber`
or a single consumer. Keep the trait **scalar-valued**; if real-time directional sun
(§5) is ever wanted, it is a *new method*, not a break.

### Layer 3 — `WorldProber` as the uniform read/interpolation front-end

`WorldProber` graduates from "cache surface + biome" into the sampling front-end for
the whole environment-field stack. Add:

- `light_at(light_src, x, y, z) -> (u8, u8)` — read-through to the trait.
- `light_smooth(light_src, fx, fy, fz) -> (f32, f32)` — **trilinear** over the eight
  corner voxels, for the smooth solar-armor float (see §3 + §7).

Consumers — ecology, solar armor, mob perception, lumen — only ever call
`WorldProber`. It already takes `gen` for the pure fields; it takes the
`LightSampler` for the state field. One object, all environment reads.

---

## 3. Two channels, separated

| Channel | Source | Propagation | Day/night |
|---|---|---|---|
| **Skylight** | the open sky | top-down + overhang bleed (`skylight_columns`) | modulated **at read** by sun elevation / `season::daylight_fraction` |
| **Block light** | emissive blocks (torch, lava, glow-fungi, lumen pearls) | BFS from sources (`propagate_block_light`), radius-limited | constant |

Stored separately because **day/night modulates sky but not block** (a torch-lit cave
stays lit at night; a sunlit meadow goes dark) and because consumers want different
blends:

- **Solar armor** charges off a **weighted blend, daylight-dominant** — it pulls from
  torchlight too, just weaker:
  `charge = w_sky · skylight · sun_elevation + w_block · block_light`, with
  `w_sky > w_block`. Pitch-dark cave → 0 (normal jump); torch nearby → small lift;
  open sun → full launch. (Corrects an earlier "sky only" sketch — armor *does* read
  block light, weighted down.)
- **Mob perception / cave-spawn** reads `max(sky, block)` (any light reveals).
- **Sun-photosynthesis** reads `skylight · daylight` alone.

One field, queried per channel.

---

## 4. Fractional transmittance + baked exposure (the canopy/angle fidelity)

The forest requirement ("dim under leaves, but not fully; angle matters for
understory growth") lands as two small, cheap extensions — **not** sub-voxel light
transport.

**4.1 Fractional transmittance per material.** Today a block is opaque or not; light
steps a fixed amount or passes whole. Give each material a **transmittance** (a new
field alongside the `emissive` that `block_material.rs` already carries): leaves
~0.5–0.7 per layer, grass ~0.9, water ~0.8, rock 0. The BFS *multiplies* by
transmittance instead of a binary step. This is the load-bearing piece for "forest
dims but not fully" — the canopy fraction *emerges* from leaves attenuating, and it
stacks naturally (two leaf layers darker than one). No sub-voxel anything. Generalises
the existing `is_opaque` boolean into a `[0,1]` coefficient.

**4.2 Baked hemispherical / daily sun exposure (angle, for slow systems).** Angle
matters for **growth** — a spot that catches morning sun but is canopy-shaded at noon
gets moderate *daily* light. Growth is slow and static, so it needs the *integrated*
exposure, not real-time directional transport. Reuse `sampling::exposure` +
`upward_hemisphere`: cast a dozen blue-noise rays up (or along the sun's daily arc,
weighted), count what escapes to sky (transmittance-attenuated), bake per cell **once**
(or rarely). This is the same probe `lumen.rs` / `cave_flora.rs` already run, promoted
to a stored `sky_exposure` channel on the trait (Layer 2). The sun *angle* shapes
where understory light lands via which rays escape — captured at bake time, read for
free.

---

## 5. The split that prevents scope-creep: gameplay scalar vs visual dapple

The requirements split into two fidelities with very different costs. Route each to
where it is cheap:

- **Gameplay (ecology, solar-armor float, mob eyes)** wants a **scalar** — "how much
  light reaches this cell," as a *fraction*. A fractional per-voxel scalar (§4.1) with
  trilinear smoothing (§3, §7) covers all of it. **No sub-voxel detail needed.**
- **Visual dapple — shafts, leaf-shaped shadows, sub-block detail** — is a *render*
  phenomenon and belongs in the render path that **already exists**: the shadow map
  with alpha-tested leaf geometry produces dappled shadows for free; froxel
  volumetrics give the shafts. Gameplay does not store sub-voxel directional light to
  make the forest *look* dappled — the renderer does that per pixel.

**The trap to avoid:** building sub-voxel directional light transport (that is global
illumination) into the *gameplay* field. None of the listed outcomes need it.

**The one genuinely expensive option, deferred:** real-time per-frame directional sun
for *gameplay* (armor float changing with sun azimuth as it rakes under the canopy
through the day) means re-probing as the sun moves. Daily-integrated exposure (§4.2) ×
daylight-intensity gets ~90% of that feel for a fraction of the cost. Treat real-time
angled gameplay sun as a conscious opt-in **if testing shows the need** — not the
baseline.

---

## 6. Performance principle + open decisions

**Cache the expensive, read through the cheap.** Surface/biome are deep noise scans →
memoise them (current `WorldProber`). A per-voxel light read is an O(1) array index →
**do not** memoise it; hashing an array read is slower than the read. So `WorldProber`'s
light methods are read-through + interpolation, not a memo. The expense moves entirely
to **propagation, which is amortised (load, worker) and incremental (edit, budgeted) —
never per-query.** This is the inversion of the freeze (r294–r297): the deep scans
recomputed per query; light computes once, stores, and every consumer reads it free.

Decisions to settle before code (testing will inform these):

- **Resolution.** Per-voxel (crisp float, exact fungus gating, more memory + full BFS)
  vs coarse per-cell deep in solid rock (light is trivially 0 there) with fine
  near-surface/near-source. Leaning **per-voxel near surfaces/sources, untracked in
  deep solid** — interacts with octree storage.
- **Where propagation runs.** The mesh worker thread is the natural home (it already
  walks chunks at load); incremental edits need a bounded budget on the main thread or
  a worker hand-off.
- **Channels storage.** Sky + block as two nibbles in one byte; `sky_exposure` (baked)
  as a separate coarse byte array (it changes rarely).
- **Render reuse.** `light.rs`'s vertex bake can read the *stored* field instead of
  recomputing, unifying gameplay + render light — but they stay separate systems
  (gameplay reads the field; render bakes/froxels the look). Confirm the bake can
  source from the store without a perf regression.

---

## 7. Consumers (all read through `WorldProber`)

| Consumer | Reads | Produces |
|---|---|---|
| **Solar armor** | `light_smooth` (sky+block, weighted §3) × sun elevation, **temporally smoothed** (lerp gravity ~0.2–0.3 s) | gravity-float scalar → jump apex / hang-time; zero in dark caves, full in open sun, partial under canopy |
| **Forest/cave ecology** | `sky_exposure` (§4.2) + `light_at` + temp + moisture | per-candidate viability: sun-grass dies underground, fungi thrive there, shade-tolerant understory under canopy — emergent zonation |
| **Mob perception ("eyes")** | `max(sky, block)` at the target | visibility / contrast term for awareness (a player in shade or tall grass is harder to see) — feeds the perception model |
| **Lumen pearls** (`lumen.rs`) | `sky_exposure` to set day-charge | night-discharge light waves; sealed caves never charge (already designed) |
| **Render** (separate) | the stored field, optionally | vertex bake / froxel volumetrics — owns the *dapple* (§5) |

Solar armor needs **two smoothings** to feel like floating rather than stair-stepping:
trilinear interpolation across voxels (§3) **and** temporal smoothing of the gravity
scalar — without them the quantised light steps read as jank.

---

## 8. Build order

0. **(Separate, immediate)** Heightmap-seed the surface sweep — kills the streaming
   freeze; independent of this work.
1. **Per-voxel store + load-time propagation + `LightSampler` trait + `WorldProber`
   read methods.** Sky + block, binary transmittance first. Wire **one** consumer
   (solar-armor float) end-to-end as the proof.
2. **Fractional transmittance** — material attribute + BFS multiply (§4.1). Canopy
   dimming appears.
3. **Incremental invalidation** — add/remove BFS, budgeted (§1, Layer 1). *The hard
   part.* Light now stays correct under edits (torches, digging).
4. **Baked hemispherical exposure** channel (§4.2) — angle-aware growth; wire ecology
   viability.
5. **Mob perception + lumen** through the same reads.
- **Deferred:** real-time angled gameplay sun (§5), pending testing.

Each step compiles green (`cargo check` gate; no GPU here) and is kill-switched per
project convention. Light reads stay O(1) throughout; the cost is paid once per change
and spread across frames — the opposite of the freeze this whole arc started from.


> See also: `neural_shading_integration.md` (r301) — the render-side consumer of
> this substrate. Its 6 hint rays are occlusion queries (the same query as mob LOS /
> cliff-pathing), its G-buffer worldpos/normal is what a sampler reads against, and it
> reuses the same temporal-accumulation base. Surfaces are the neural net's job;
> volumes/reflections stay froxel/SSR (its own findings, BUGWATCH H1).
