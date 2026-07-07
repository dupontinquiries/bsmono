# Feature batch — destructible buildings, map events, multi-level maps, hunt polish

## Building mesh deformation & destructibility (`src/destructible.rs`)
Added `DestructibleKind::Wall` — a large structural panel (220 HP) that **deforms progressively** from its live
damage fraction rather than snapping between three prop states. `wall_mesh()` builds the wall as a grid of
panel cells (3–8 cols × 2–6 rows); as damage rises, the weakest cells (centre + upper, per a deterministic
per-cell toughness) knock out first, leaving a growing ragged hole and rubble stubs at the base, while
surviving cells crack (more dither) and bow inward. Fully caved → a low jagged rubble line. Rendered across
0/35/65/90% damage; regression-tested that cell count drops monotonically with damage.

## Per-map live events (`src/game.rs`)
Added a generalized biome-keyed `MapEvent` system, complementary to the existing generic Fog/Blackout/Surge
events and Desert Town's bespoke quake+sandstorm. Each biome gets a signature ambient event that periodically
sweeps the map (a tense-quiet baseline broken by a spectacle, like the quake cadence):
- Volcanic → **Ember Storm** (rising embers + ash)
- Ice → **Blizzard** (driving snow + fog whiteout)
- Toxic → **Gas Surge** (acid mist wells up, fog thickens)
- Station → **Solar Flare** (static ion discharge off edges)
- Carrier → **Squall** (sea spray + rain, the deck heaves)
- Alien/Canopy/Mistfall → **Spore Bloom** (drifting biolume motes)
- Redwood/Meadow → **Wind Front** (rolling gust, leaves/pollen)
Events drive `fog_event` where appropriate, emit themed particles around the player, and push a HUD banner.
`map_event()` / `map_event_intensity()` expose the state. Tested: each theme maps to its event and the volcanic
ember storm actually fires over time.

## Sand banks on Desert Town (`src/level.rs`)
The quake-struck town now sits in drifted desert: an edge-weighted fbm dune field raises the perimeter terrain
(the central plaza/streets stay flat and walkable) plus five tall heaped sand banks against the outskirts.
Verified numerically (max height ~2.8m, centre 0.0, ~2700/3364 corners raised) and all 71 level tests pass.

## Multi-level map: Spire Foundry (`src/level.rs`)
A new authored map (`build_spire_foundry`, preset "Spire Foundry", authored id 7) built specifically for
verticality. A tall foundry hall with **three stacked walkable floors** — ground, mezzanine (4.2m), upper
gantry (8.4m) — wrapped around an open central atrium, joined by ramps and switchback stairs, with catwalks
bridging the void, standable machine housings, support columns through all levels, edge railings, and overhead
lamps. Uses the engine's `Platform`/`support_y` system so you stand on a deck and walk beneath it. 30 platforms
across three distinct heights; spawns on opposite ground-floor corners. Tested for multi-level structure
(platforms at ≥2 raised tiers, max height >7m) and full-wave population.

## Hunt enemy animation / modelling (`src/enemy.rs`)
Enriched the two exotic hunt creatures beyond their base silhouettes:
- **Storm Kraken**: a charge pulse now rides the walk phase — the mantle brightens, glowing ridge fins run up
  it, and alternate tentacle tips carry pulsing charged nodes; tentacles curl as well as sway.
- **Widow**: legs now scuttle in a staggered gait (alternate feet lift on the forward swing), an eight-eye
  cluster glints on the head, and two fangs flex with the gait.

## Better skyboxes (`src/app.rs`)
The horizon silhouette is now theme-shaped beyond the previous urban-vs-hills split: a clean hazy sea line for
the Carrier, sharp ember-lit jagged peaks for Volcanic maps, and low distant hull structures over flat void for
the Station. Existing gradient/stars/moon/sun/cloud layers unchanged.

## Test status
All new features tested. Full suite green except the long-standing pre-existing buoyancy failure
(`player::tests::a_body_dropped_into_deep_water_floats_instead_of_sinking`), which remains untouched and still
needs its own dedicated physics fix.

## Still open from the original request
The broader "asset pool" expansion and connected-texture work remain partially addressed (floors and skyboxes
are in good shape; the enemy bodies are richly modelled). A dedicated material/connected-texture pass on the
older procedural maps is the natural next step.
