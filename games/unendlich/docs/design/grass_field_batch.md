# Batch-as-Field Grass LOD (experimental)

> Part of the menu/sim/perf work — see `MENU_SYSTEM.md` and `PERF_PROFILING.md`.

A different take on distance LOD for dense grass. The usual move is *cull or
simplify the far stuff*. This instead **batches a patch into a field** and
splits the work by distance band: the near **front** is high-res per-blade,
the far **back** is fast-forwarded as an aggregate field. You pay for what's
actually resolvable on screen.

Status: the **model** (`crates/unendlich-engine/src/sim/grass_field.rs`) is
pure, always-built, and unit-tested (6 tests). The **live tick-path hookup**
is behind the `grass-field-batch` cargo feature (off by default) and needs a
running build to validate — the recipe is below.

## The model

A grass patch → a coarse `w × h` **field** of `FieldCell { density, growth,
sway }` tiles (aggregate, not per-blade).

- `GrassField::tick_front(wind, params)` — one high-res tick (what the player
  sees up close): growth += rate, sway advances with wind, density diffuses to
  neighbours (spreading). Exact, no caps.
- `GrassField::fast_forward(n, wind, params)` — **the field propagation that
  fast-forwards the back band by `n` skipped ticks in ONE pass.** Growth and
  sway are independent per-cell channels, so their `n`-tick advance is
  close-form and *exactly* equals `n` front ticks (proven by
  `fast_forward_matches_repeated_front_ticks`). Spreading is diffused once at a
  capped strength — the field analogue of `leaf_decay::advance_n_ticks`'s
  saturating catch-up: a long-skipped patch spreads to "mostly full", not
  beyond. Cost is O(w·h) « O(N blades).
- `band_plan(band, ticks_since_update, params) -> GrassWork` — the split,
  reusing the engine's existing `world::scheduler::band_for_chunk`:
  - `Render` band → `HighResFront` (per-blade);
  - `Simulation` band → `FieldFastForward { catch_up_ticks }`;
  - `CatchUp`/beyond → `Dormant` (real culling still applies out here).

So when the player turns to look at a far patch, its field has been advanced to
a plausible *present* (grown, rippling) rather than frozen or visibly catching
up — but it cost one field step, not N blade ticks.

## Why this is a win

Ticking N blades is O(N). At distance the eye can't resolve individual blades —
it reads the aggregate (how green, how tall, how it ripples), which the field
captures directly. A dense patch that would be thousands of blade updates
becomes a few hundred field-cell updates, and the *visible* front keeps full
detail. Net: detail where it counts, cheap aggregate where it doesn't —
without the popping that hard culling causes when a patch blinks between ticked
and frozen.

## Integration recipe (the `grass-field-batch`-gated part)

1. **Represent patches as fields.** Where dense grass is instanced per chunk
   (see `world::flora_blocks` / the grass instancing in the renderer), keep a
   coarse `GrassField` per grass-bearing chunk (e.g. one cell per 4×4 blocks).
   Seed `density`/`growth` from the existing instance data.
2. **Drive from the tick handler**, exactly like `leaf_decay::decay_handler`:
   for each grass chunk, classify with `band_for_chunk(coord, player, params)`,
   then `band_plan(band, gap, params)` where `gap` comes from the same
   `fire_tracker` leaf_decay uses (ticks since this chunk last fired). Apply
   `tick_front` / `fast_forward(catch_up_ticks, …)` / skip accordingly.
3. **Push the field to the mesh/instancing.** For the back band, derive the
   per-blade visual (height from `growth`, bend from `sway`, count from
   `density`) from the field cell when (re)building the far LOD instance buffer
   — so the far grass *renders* from the field rather than from individually
   ticked blades. The front band keeps its existing per-blade path.
4. **Gate it.** All of the above lives under `#[cfg(feature =
   "grass-field-batch")]`; the per-blade path stays the default until this
   proves out at runtime.

## How to test (needs a build + the feature)

Build with the feature, stand at the edge of a large grass field, and look
across it:
```
cargo run --release --bin unendlich-fullgame \
  --features "render,hot-shaders,window,gpu,ui-overlay,audio,grass-field-batch"
```
- The near grass should sway/grow at full detail (front band).
- The far grass should still ripple and look alive (field fast-forward), not
  frozen, and should *not* visibly snap when you turn toward it.
- Profile with `UNENDLICH_FRAME_PROFILE=1` (see PERF_PROFILING.md): `tick_sim`'s
  p99/max should drop versus per-blade ticking of the same field, with no new
  spike in `draw`.

If the far grass looks frozen or pops, the field isn't being pushed to the far
LOD instance buffer (step 3) or `gap` isn't being read from the tracker
(step 2).

## Not done (deliberately)
Only the pure model + the band-split decision are wired and tested. Steps 1–4
(the per-chunk field, the handler hookup, and pushing the field to the far LOD
mesh) are the gated integration and want a running build to validate — none of
it is on the default path, so the engine is unaffected until it's proven.
