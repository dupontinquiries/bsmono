# Dynamic World Architecture — Computation vs. Vision, and the Tiered Mesher

> Forward-looking: `statistical_timebending_and_storage.md` extends this into deep-time scrub (statistical+residual long-time sim) and shows why the tier-D fast-forward is also what makes dynamic-world PERSISTENCE affordable (regenerate-on-load vs store-everything).

> Status: design / architecture (operator notes, r250). This is the unifying
> doc that ties together the pieces already designed separately:
> `tick_authority_regions.md` (regional async tick + render-last-state),
> `batch_field_lod.md` + `grass_field_batch.md` (batch-as-field LOD),
> `mesh_vision_scheduling.md` (vision-driven meshing),
> `PERF_FINDING_far_region_corrected.md` (compute-once-share, never skip).
> It adds the missing spine: the **separation of "compute" from "vision"**, the
> **three vision consumers**, and the **tiered mesher**.

---

## 1. The core principle — two questions, never conflated

Every chunk/region the engine touches answers two INDEPENDENT questions, and
the bug we keep almost making is answering them as if they were one:

- **Q1 — "compute this now, and mark what changed dirty."**
  The world *evolves* whether or not anyone is looking. Water flows downhill and
  pools in caves; rivers erode their banks; a cut forest regrows into its gap;
  grass reclaims a trodden path; a lightning fire eats fuel and the front
  advances; storms build from humidity and instability and drift on the wind;
  populations rise, fall, and migrate. This is **simulation authority**: it owns
  the truth of the world's state and produces *dirty marks* (this region's
  terrain/biome/field changed) as its output.

- **Q2 — "is this chunk needed for someone's vision right now?"**
  Rendering is a *consumer* of that state, on demand, at the fidelity the viewer
  needs. It does NOT own truth and must never be the reason a simulation result
  is or isn't computed.

**The rule:** computation cadence is driven by Q1 (the sim's needs +
dirty-propagation), and meshing/upload cadence is driven by Q2 (who needs to see
it). A chunk can be:
- computed but not meshed (far sim region nobody's looking at — keeps evolving),
- meshed but not re-computed (static terrain in view — just needs vertices),
- both (in view AND changing),
- neither (dormant, far, unchanged — costs nothing).

This is why `UNENDLICH_FAR_SKIP_CAVES` was wrong (r248, reverted r249): it tried
to answer Q1 ("compute caves?") with a Q2 answer ("you can't see them"). Caves
feed water/karst/erosion that propagates between regions; the computation is
*necessary*. The fix is never "skip the computation," it's "compute it once at
the right resolution, share it, and don't redundantly rebuild."

---

## 2. Three consumers of vision (not one)

`mesh_vision_scheduling.md` already meshes for "view cones (player + nearby
mobs)." Make that explicit as a first-class list, because each consumer wants a
DIFFERENT fidelity and the mesher must serve all three:

1. **Player vision** — the one human camera. Needs the highest fidelity, full
   detail, near the camera; LOD falls off with distance. This is what the
   current near/mid/far mesher targets.

2. **Mob vision** — every creature has eyes too. A dragon hunting fauna, a herd
   watching for predators, a mob pathing around a lake all "see" the world — but
   they do NOT need a render-quality mesh. A mob needs to answer gameplay
   queries: *is there prey/cover/water/a ledge here?* That can be answered from a
   **much cheaper representation** than the player's mesh (see §3, tier C).
   Critically, mob vision exists in places the player isn't looking — so it can't
   piggyback on the player's frustum.

3. **Simulation propagation** — the sim itself "consumes" terrain structure to
   evolve regional fields (water needs to know where the cave/valley is to pool;
   fire needs fuel density; erosion needs slope). This consumer wants the
   **coarse regional field** (§3, tier D), not a mesh at all.

The mistake is building one mesher for the player and then either (a) forcing
mobs and sim to use that expensive mesh, or (b) starving mobs/sim of terrain
they need because "the player can't see it." Each consumer gets its own tier.

---

## 3. The tiered mesher / tiered representation

One source of truth (the region's computed state), several *views* of it at
different cost, each serving a consumer. From most to least detailed:

| Tier | Representation | Consumer | Cost | Example |
|------|----------------|----------|------|---------|
| **A. Full mesh** | greedy/octree GPU mesh, per-voxel faces, climate tint | player vision, near | highest | the chunk you're standing in |
| **B. Downsampled mesh** | factored cells (factor 2/4/8), no greedy merge | player vision, mid/far | medium | hills on the horizon |
| **C. Proxy / fast-noise** | cheap analytic answer ("grass here? ledge? water?") from a fast noise or the coarse field — NO vertex buffer | **mob vision**, far gameplay | low | a far dragon deciding the meadow below has prey-cover grass |
| **D. Coarse regional field** | one aggregate value per coarse cell (solid/cave/surface-height/fuel/moisture) | **sim propagation** | lowest, shared | water-flow / erosion / population deciding what a region *is* |

Key design points:
- **Tier D is the shared substrate.** Compute the region's coarse terrain field
  ONCE (the batch-as-field pattern from `batch_field_lod.md`), and have tiers B,
  C, and the propagating sim all *read* it instead of re-sampling the generator.
  This is the structural perf fix from `PERF_FINDING_far_region_corrected.md`,
  and it makes the visual and the sim **consistent by construction** — the lake
  the sim fills is the lake the mesh shows, because they're the same field.
- **Tier C is what "grass = fast noise for mob eyes" means.** A far dragon
  hunting doesn't need grass *meshed*; it needs to know "is there grazing cover
  / are there prey here," which a fast noise lookup or a read of the grass field
  (`sim::grass_field`) answers in nanoseconds. So dragons keep hunting, growing,
  and building over distance without the far grass ever being meshed.
- **Tiers are chosen per (consumer, distance), not globally per chunk.** The same
  region can be tier-A-meshed for the player while answering tier-C queries for a
  mob on the far side and tier-D field-updates for the water sim — all at once,
  each paying only its own cost.

This generalises `mesh_vision_scheduling.md`: instead of "mesh or defer," it's
"serve each consumer at its tier." The player's frustum picks A/B; mob cones
pick C; the sim always has D.

---

## 4. How the dynamic-world behaviours map onto this

Each behaviour you described is "Q1 computes + marks dirty; the right consumer
sees it at the right tier." None of them require the player to be present.

- **Lakes settle into caves / rivers erode and reshape.** Water-flow + karst
  (`world::karst_sink`, water/moisture fields) evolve on tier D, mark the
  affected regions dirty as terrain changes. When the player (tier A/B) or a mob
  (tier C) next needs that region, it's rebuilt from the updated field. The river
  that changed shape while you were away is simply *found* in its new shape — no
  pop, because (per `tick_authority_regions.md`) we render the last-ready state
  until the new mesh is built.

- **Player pathways regrow grass.** Trampling writes low `density` into the grass
  field along the path; `sim::grass_field` regrows it (closed-form fast-forward
  over the time you were away). The regrowth is computed (Q1) regardless of
  vision; you see it (tier A/B) when you return, mobs read it (tier C) to decide
  the path is passable cover again.

- **Cut forests fill in ecologically.** Tree removal marks gaps; the plant-
  population sim (`world::plant_lifecycle`, generalised) regrows into them per
  ecological rules (seed dispersal from neighbours, light, moisture) on a coarse
  cadence. This is a Q1 process with a dirty-mark output; meshing happens only
  when someone looks.

- **Lightning storm burns a forest → animals migrate → smoke on the wind.** This
  is the showcase of cross-region propagation: storm intensity
  (`world::storm_intensity` = humidity × instability) builds on tier D and
  drifts on the wind field; a strike ignites the fire field (`sim::fire_field`,
  reaction-diffusion, fast-forwarded far); the fire's heat/loss of fuel marks
  terrain + population dirty; the population sim reads "fire here, fuel gone" and
  **migrates** mobs to neighbouring regions (population is a propagating field
  too); smoke is an advected field riding the same wind. Every step is Q1
  computation marking regions dirty; the player and mobs consume whatever tier
  they need, wherever they are. The forest can burn and the herd can flee in a
  region no one is currently rendering.

---

## 5. Where this meets the current code (wiring reality, not new architecture)

This doc is design; here's the honest mapping to what exists so it's buildable
incrementally without inventing new subsystems:

- **Tier D substrate** ← the batch-as-field modules already in `sim/`
  (`grass_field`, `fire_field`) + the regional fields (`storm_intensity`,
  `karst_sink`, moisture/water). The new work is a *shared coarse terrain field*
  per region that the far mesher (tier B) reads instead of calling
  `generator.density()`/`is_cave()` itself (the measured 11ms/far-chunk cost).
- **Tier C mob proxy** ← `sim::mesh_vision` already models view cones for "player
  + nearby mobs"; extend the mob branch to answer gameplay queries from the field
  / fast noise rather than requiring a mesh. Mob AI (the roamer loop) is the
  consumer.
- **Dirty propagation + render-last-state** ← `tick_authority_regions.md`'s
  async-tick-against-snapshot + apply-budget deferral is exactly the "compute on
  workers, mark dirty, render last-ready until new state lands" machinery. The
  per-region stride table proposed there is the cadence control for Q1.
- **Cadence decoupling** ← the scheduler bands (`world::scheduler`) already split
  Render / Simulation / CatchUp; this doc says: meshing rides the Render band
  (Q2), field/sim computation rides Simulation/CatchUp (Q1), and they don't
  block each other.

### Suggested build order (each step independently shippable + testable)
1. **Shared coarse regional terrain field** (tier D for terrain): compute
   density/cave/surface-height once per coarse region cell, cache it, have the
   far mesher read it. Removes the redundant generator sampling; first concrete
   perf win that respects the sim's need for the data.
2. **Far-mesh cache + dirty-rebuild**: rebuild a far mesh only when its region
   field changes (not every resubmit). Pace far-mesh bursts (a far submission
   budget). Kills the `far=11ms×77` spike without dropping computation.
3. **Mob-vision tier C**: route mob gameplay queries (prey/cover/water/ledge)
   through the field/fast-noise instead of any mesh. Validate dragons still hunt
   at distance with no far meshing.
4. **Populations as a propagating field**: generalise plant/mob population to a
   regional field that migrates across region boundaries (the fire→migration
   case). This is the largest piece and depends on 1–3 being solid.

Each of these is a wiring task against existing modules + the registries
(`sim/service_registry.rs`, `sim/kernel_registry.rs`), each gets a named kill
switch + a `log::info!`, and each needs a profile run to validate (no GPU in the
build sandbox).

---

## 6. Invariants (so future changes don't regress the principle)

- **Never gate a simulation computation on player visibility.** Vision picks the
  *tier you render*, never *whether the sim runs*.
- **One source of truth per region; many tiered views.** If two consumers
  disagree about a region (the mesh shows a hill the sim thinks is a lake), a
  tier is re-sampling the generator instead of reading the shared field — fix
  the sharing, don't reconcile after the fact.
- **Far ≠ frozen.** Far regions advance (fast-forwarded fields), they're just
  represented coarsely and rebuilt on demand. "Found in a plausible present" is
  the goal, not "paused until looked at."
- **Compute once, rebuild only on change, pace the bursts.** The three perf sins
  are: recomputing shared data per-consumer, rebuilding unchanged meshes, and
  letting a burst of rebuilds land in one frame.
