# Batch-as-Field LOD — Grass & Fire (experimental)

> See also `dynamic_world_architecture.md` — batch-as-field is the tier-D shared substrate in the dynamic-world architecture.

> Part of the perf work — see `PERF_PROFILING.md`, `mesh_vision_scheduling.md`.
> Grass specifics: `grass_field_batch.md`.

A shared pattern: instead of culling dense distant dynamic stuff, **batch it
into an aggregate field** and split the work by distance band — near = high-res
per-element, far = one **field-propagation** pass that fast-forwards the skipped
ticks. You pay for what's resolvable on screen, and the far stuff stays alive
(advancing to a plausible present) rather than frozen, so it doesn't pop when
the player looks.

Both modules reuse the engine's existing tick bands
(`world::scheduler::band_for_chunk` → Render / Simulation / CatchUp) so they
line up with the rest of streaming, and both mirror
`world::leaf_decay::advance_n_ticks`'s saturating catch-up. Pure, always built,
unit-tested; live hookups gated.

## Grass — `sim::grass_field` (feature `grass-field-batch`, r238)
Field of `{density, growth, sway}`. Growth & sway are linear per-cell →
`fast_forward(n)` is **closed-form and exactly equals n front ticks**; density
spreads by capped diffusion. Detail: `grass_field_batch.md`.

## Fire — `sim::fire_field` (feature `fire-field-batch`, r239)
Field of `{fuel, intensity, heat}` — a reaction-diffusion front:
- burning cells consume `fuel` (monotonically) and radiate `heat` to
  neighbours;
- a fuelled cell whose `heat ≥ ignite_heat` ignites (intensity jumps);
- `heat` decays when unfed (keeps fronts thin); a cell out of fuel goes out.

`tick_front(p)` = one exact step (near fires). `fast_forward(n, p)` advances the
back band: because combustion is **nonlinear/coupled** (unlike grass's linear
channels), the honest catch-up *iterates the aggregate step n times* — still
O(n·w·h) « O(n·N cells) for a dense blaze, on a coarse field. `n` is **capped**
to `~4/burn_per_tick`: past that the region has consumed its fuel and
converged (fuel is monotonic), so a long absence can't spin forever — the far
fire is simply found burnt-out, which is correct.

`band_plan(band, gap, params) -> FireWork{ HighResFront | FieldFastForward{
catch_up_ticks } | Dormant }`, same shape as grass.

Tests (6): spreads from a source into fuel; fuel monotonically consumed;
`fast_forward(n)` == n front ticks (shared step); a fire burns itself out (cap
converges); no fuel ⇒ no fire; the band split.

### Fire integration recipe (gated)
Same shape as grass (`grass_field_batch.md` steps 1–4):
1. Keep a coarse `FireField` per fire-bearing chunk, seeded from the burning
   blocks (`fuel` from flammable mass, `intensity`/`heat` from currently-lit
   cells). The per-cell exact path stays `sim::combustion_spread`.
2. Drive from the fire tick handler via `band_for_chunk` + the `fire_tracker`
   gap: `tick_front` / `fast_forward(gap)` / skip.
3. Push the field back to visuals: flame intensity & ember emission
   (`world::ember_field`) from `intensity`, scorch/char from spent `fuel`,
   light/heat sources from `heat`. Far fires render from the field.
4. All under `#[cfg(feature = "fire-field-batch")]`; the per-cell path is
   default until validated.

### How to test
Build with `fire-field-batch`, start a large wildfire (F9), walk away and look
back: the distant fire front should keep advancing & glowing (not frozen),
fuel visibly depleting behind it; `UNENDLICH_FRAME_PROFILE=1` `tick_sim` p99 should
drop versus per-cell ticking the same blaze. If the far fire freezes or snaps,
the field isn't being fast-forwarded from the tracker gap (step 2) or pushed to
visuals (step 3).

## Not done (deliberately)
Only the pure models + band splits are wired and tested for both. The per-chunk
fields, handler hookups, and pushing fields to visuals/mesh are the gated
integrations and want a running build. Nothing is on the default path.
