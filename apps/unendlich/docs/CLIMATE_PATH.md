# CLIMATE_PATH.md — weather, seasons & water roadmap

The north star (from DESIGN_NOTES "ecology-driven atmosphere"): the
world should *derive* its weather, seasons, and water from simulated
fields rather than authored presets. The engine already simulates the
inputs — temperature, humidity, pressure are real queryable fields on
`WorldModel`. This doc tracks the path from "real fields" to "living
world" and records where each increment lands.

---

## The architecture: field is authoritative, block is presentation

Established for moisture (clouds) and water in prior sessions: the
simulation owns a **field**; blocks are just markers/presentation of it.
`BlockId::Water` is a worldgen-time marker that (eventually) just seeds
field cells; the field is what flows, freezes, and evaporates.

## The climate triangle

Three coupled fields generate weather/seasons/phase transitions with no
per-feature scripting:

```
        temperature (scalar)
            /          \
   convection            phase change
   (warm rises)          (vapor↔liquid↔ice)
          /                  \
   wind (vector) —— transport —— moisture (scalar)
```

- **temperature → wind:** warm air rises, cool sinks → convection cells.
- **temperature × moisture → phase:** >100°C vapor, <0°C ice, cold
  high-altitude cloud → rain.
- **wind × moisture → transport:** fog/clouds/rain-curtains advect.
- **temperature → friction/effects:** ice slippery, lava ignites.

Today the engine has temperature, humidity (≈moisture), and pressure as
spatial fields. Wind-as-vector-field and the couplings are still ahead.

---

## Seasons (temporal axis) — IN PROGRESS

Seasons is temperature/moisture modulated over a year cycle. The hook
was reserved long ago: `WorldModel::temperature(x,y,z,_t)` has an unused
time parameter, and the module doc says "Tomorrow: fields can become
time-evolving `f(seed, t, region_coord)`."

**Shipped (v0.24.NEXT+4-seasons):** `world/season.rs` — a pure, tested
year-phase model (`year_phase`, `temperature_delta`, `humidity_delta`,
`Season`). Wired into the GPU mesher's per-chunk climate-tint sample as
a *render-only* additive delta, behind `UNENDLICH_SEASONS=1`. Because
the renderer's climate-tint already consumes temperature/humidity, this
produces seasonal color shifts with zero shader changes.

**Crucially worldgen-safe:** the season delta is applied at the tint
*consumption* point, NOT baked into `temperature()`/`humidity()` —
because those feed `sea_level`→`density` (terrain). A world must not
re-grow its mountains every winter. The base fields stay byte-identical;
only the tint shifts.

**Next increments for seasons:**
1. **CPU-mesher tint parity.** This ship covers the GPU path
   (Near/Mid). Far-LOD chunks (CPU-meshed via `sample_env_at`) don't yet
   get the seasonal delta — a subtle tint mismatch at the Mid/Far
   boundary. Thread the tick into `sample_env_at` and apply the same
   delta. (Deferred: needs tick threading through several emit helpers.)
2. **Live transition.** Static already-meshed chunks won't visibly
   change season until re-meshed. Add a slow periodic re-mesh trigger
   (or a season-version bump on the chunk) so the world transitions
   live, not just on load. Throttle hard — re-meshing the whole view
   every few minutes, not every tick.
3. **Latitude / hemisphere.** `temperature_delta` is currently global;
   make it phase-shift with world Z (or a latitude field) so hemispheres
   are opposite and equator barely swings.
4. **Snow line.** Couple winter temperature to a surface snow block /
   subvoxel frost layer where `base_temp + winter_delta` drops below a
   threshold. First real season→world (not just season→tint) effect.

---

## Water — ROADMAP (field-driven fluid)

Current state: `BlockId::Water` with CPU-mesher surface passes
(`emit_water_surface`, top faces only). v0.24.NEXT+2 brought those to the
GPU path via the decorate post-pass. Water is still static, block-grid.

The 5-step path to fluid (from prior design sessions):

1. **Continuous surface heights.** Per-cell water *level* field (f32),
   so partial fills and visible water heights are possible. Mesher emits
   the surface at the field height, not the block top.
2. **Settle / flow.** Each tick, water flows downhill + laterally toward
   its level. This is when "water" becomes "fluid" — pour it on a hill,
   watch it find level.
3. **Field-driven mesh.** Mesher reads the field, not the block;
   `BlockId::Water` decays to a worldgen seed marker.
4. **Erosion.** High-flow cells weather adjacent stone (stone → gravel →
   sand → silt). Rivers carve valleys over time.
5. **Evaporation + rain feedback.** Water → moisture field → cloud →
   rain → ground → water. Closes the hydrologic cycle and ties water
   directly into the climate triangle.

**Started (v0.24.NEXT+6-water-field):** `world/water_field.rs` — the
step-1+2 foundation. A per-column depth heightfield (`WaterField`, 4 KB/
chunk) plus a volume-conserving, deterministic `settle_step` (step 2's
flow math: water moves toward equal surface heights, never sends more
than it holds, closed chunk boundaries). Pure and unit-tested (6 tests:
empty, volume conservation, spreading, downhill flow, non-negativity,
determinism). **No rendering or worldgen coupling yet** — it's state
only, so it's byte-identical until wired.

**Chunk↔field bridge added (v0.24.NEXT+7-water-bridge):**
`WaterField::seed_from_chunk(chunk, fluid)` (per-column depth = fluid
block count; `total_volume` == fluid block count by construction) and
`ground_heights_from_chunk(chunk, fluid)` (per-column floor height; a
documented Air/fluid-only approximation since `BlockId` solidity
predicates aren't in this crate slice). With these, the full
seed→settle loop works from a real chunk and is tested end-to-end
(`seed_then_settle_conserves_seeded_volume`). Still pure/inert — nothing
constructs a field yet.

**Absorbent block ported from the mod (sponge → `absorb_at`):**
`WaterField::absorb_at(sx, sz, max_radius, capacity)` is the field-driven
port of the mod's `SuperSponge` BFS water-absorber. The original removes
connected water *blocks* in 3D (flood depth 6, ~128-cell capacity); the port
removes connected water *column depth* via a 2D BFS over the heightfield
(`SPONGE_RADIUS`/`SPONGE_CAPACITY` keep the 6/128 limits), the vertical
extent absorbed implicitly by draining the column. It conserves accounting
exactly (volume drops by precisely the returned amount) and — crucially for
"field is authoritative" — composes with `settle_step`: a sponge in a lake
clears only a transient pocket that the surrounding water flows back into.
Pure/inert until a block-behavior hook calls it (the integration step, like
`seed_from_chunk`). Six tests (radius disk, capacity cap, fractional last
column, dry-gap connectivity, exact accounting, absorb-then-settle refill).

**Remaining water increments:**
1. **Persistent field storage + tick.** A per-chunk `WaterField` that
   lives somewhere (a side-map in the chunk manager, or a new `Chunk`
   field — the latter touches the procedural/explicit storage model, so
   the side-map is safer) and is settled each sim tick. This is the
   integration that's currently **blocked**: `Chunk` has no field
   side-state, `leaf_decay.rs`/the sim scheduler aren't in this crate
   slice, so the tick-driver pattern can't be safely followed blind.
   Needs the chunk-manager/scheduler files.
2. **Field-driven mesh (step 3).** Once a persistent settled field
   exists, the mesher emits the water top at `field.at(x,z)` surface
   height (partial fills, smooth settled surfaces) instead of the
   per-block top. Flag `UNENDLICH_WATER_FIELD=1`. (Mesh-time-derived
   fields were ruled out: without settling they equal the block heights —
   no visible change — and settling per-chunk-only creates cross-chunk
   seams. Real payoff needs the persistent field from (1).)
3. **Cross-chunk flow**, then **erosion (step 4)**, then
   **evaporation/rain cycle (step 5)** — step 5 connects water to the
   moisture field weather rides.

---

## Weather (emergent, later)

**Atmospheric layer + phase physics started (v0.24.NEXT+5, +8):**
`world/weather.rs` — pure, tested state. `precipitation` from
humidity-gated storm noise; `snow_fraction` from temperature (cold →
snow); `lightning_flash` from stochastic flicker-pulse strikes during
*liquid* rain. The renderer consumes it: rain darkens sky/fog, snow
brightens/whites-out, lightning flashes, storms raise wind shimmer — all
on uniforms the shader already reads. Flag `UNENDLICH_WEATHER`.

Rain↔snow ties the climate fields together: cold biomes/altitude snow,
warm lowlands rain, emergent from temperature. **Open:** snow uses base
climate temperature, not the seasonal tint — sharing the seasonal delta
into weather temperature (so winter makes temperate storms snow) needs a
common world-tick between renderer and season clock. Snow *accumulation*
(`Snow`/`SnowBlock` on the ground) is a worldgen/sim step; particles,
bolt geometry, thunder, wetness are render-side consumers of this state.

Full **emergent** weather — moving fronts from moisture mass +
temperature gradient + wind transport — still needs wind-as-vector-field
+ couplings + froxel fog (DESIGN_NOTES Phase 3). The current model is
per-location stochastic; built so the front model can replace `sample`'s
internals without changing consumers. Tracked here so seasons/water increments compose into it (the
moisture field water step 5 feeds is the same field fronts ride).

---

**Varied weather (cloud cover / fog / wind) added (v0.24.NEXT+5b):**
`WeatherState` gained `cloud_cover`, `fog`, `wind_dir`, `wind_speed` — each
a `value_noise` layering on a distinct period, gated by the same authoritative
humidity/temperature fields. The renderer applies them behind `UNENDLICH_WEATHER`
(overcast greying of the part of cloud cover not already from rain, fog density,
directed wind). Verified in a Python port before shipping (six distinct kinds,
all fields bounded, rain⇒overcast holds).

**Field-is-authoritative for the categorical view, too.** Per the same rule
that makes `Biome` a derived compatibility surface over the field model,
`WeatherKind` (Clear/Overcast/Fog/Rain/Snow/Storm) is *not* stored state: it
is produced at call time by `classify_weather(precip, snow_fraction,
cloud_cover, fog)` — the weather analogue of `biomes::determine_biome` — and
exists only so enum-oriented callers/developers from other engines get an
intuitive label. The continuous fields stay authoritative; the renderer reads
them directly, never the kind. If the biome→field-range table ever moves to
data (`biomes.json`), this cascade is the natural next thing to externalize.

## Flags introduced on this path

| Flag                       | Default | Effect                              |
| -------------------------- | ------- | ----------------------------------- |
| `UNENDLICH_SEASONS`        | off     | seasonal render-tint modulation     |
| `UNENDLICH_TICKS_PER_YEAR` | 72000   | year length in ticks                |
| `UNENDLICH_WEATHER`        | off     | rain/lightning atmosphere modulation|
| `UNENDLICH_WATER_FIELD`    | (future)| field-driven water (when step 3 lands)|

All default-off / byte-identical so the simulation and worldgen are
unchanged until explicitly opted into for A/B.
