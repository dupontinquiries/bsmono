# Map analysis & authoring systems

Three scoring lenses now judge a map, each catching a failure the others miss. Together they turn "does this
map look/play good?" from a matter of taste into something measurable and iterable.

## 1. Geometric gameplay score — `src/mapeval.rs`
The original scorer. Coarse aggregate metrics from the grid + obstacles: **openness** (sky/air exposure),
**flow** (short/medium/long sightline mix), **cover** density, **verticality**, **connectivity**, scale, and
whether the map has a landmark. Produces a letter grade (S–F). Fast, runs on every generated map.

Use it for: "is this map connected, standable, and roughly balanced?"

## 2. Sightline / chokepoint score — `src/sightline.rs`  *(new)*
Gameplay **geometry** analysis built on line-of-sight ("gun-arc") calculation. Where mapeval sees aggregates,
this builds a per-tile visibility model and derives tactical structure:

- **exposure(tile)** — the fraction of the map that can see & shoot a tile. Computed by casting supercover DDA
  rays between a subsample of walkable tiles and testing each against the eye-height cover mask (the same
  "tall obstacle blocks a standing sightline" rule mapeval's flow uses). Works on authored maps whose walls
  are freestanding boxes, not grid cells.
- **exposure_spread** — the stdev of exposure. High = the map mixes exposed perches and safe pockets
  (memorable, tactical); low = a uniform gallery or a uniform maze.
- **chokepoints** — walkable throats: tiles open along one axis but pinched across the other, i.e. doorways
  and alley mouths between open areas. Adjacent throat tiles collapse into one chokepoint.
- **safe_fraction** — share of tiles with low exposure (cover to break to).
- **structure_score** (0..1) — the headline number. It rewards *internal variety* rather than a fixed
  exposure target: relative spread (coefficient of variation, so a dense town and an open arena are each
  judged against their own baseline), absolute spread (a floor so a uniform map still scores low), a healthy
  safe-fraction band (15–70% — neither a death gallery nor a wall-maze), and a sane chokepoint density
  (~1 per 40 sampled tiles). This archetype-fair weighting is deliberate: an earlier version keyed off a
  single "ideal" 35% mean exposure and unfairly punished dense urban maps that legitimately sit near 10%.

`suggest_cover(level, budget)` closes the loop: it ranks tiles by exposure and returns *widely-spaced* world
positions + sizes where an authoring pass should drop cover. `Level::scatter_exposure_cover()` applies them as
weathered crates. **Caveat, learned the hard way:** blanket cover placement at every exposed tile *lowers* the
structure score — it flattens the exposure spread that makes a map interesting. Cover must punctuate long
lanes, not erase them, which is why suggest_cover spaces its picks ~8 tiles apart and caps the budget. It's a
scalpel for specific over-open lanes, not a fix to spray everywhere.

Run `cargo run --example sightline_report` for the table. Observed: Pyre Garage & Flood Zone score highest on
tactical structure (~0.75); Reference Town is flagged as chokepoint-*heavy* (126 throats — a corridor crawl),
which correctly drags its structure score down despite a decent geometric grade.

Use it for: "does this map play as a series of meaningful fights, or one flat shooting gallery? where are the
pinch points? where is cover missing?"

## 3. Scenic / variety score — `tools/scenic.py`  *(new, OpenCV)*
Judges the *look* of a map from the player's vantage, and **negatively scores repetition and blandness** — the
two failure modes of box-built procedural levels. Consumes the multi-angle renders from
`cargo run --example scenic_shots --release` (→ `/tmp/scenic/<map>/angle*.png`) and reports per map:

- **texture** — local detail (local stddev + Laplacian energy). Flat painted boxes score low; furrowed bark,
  mottled plaster, rubble score high.
- **colour** — hue-histogram entropy over saturated pixels. A monotone sand/smoke map scores low.
- **silhouette** — Canny edge density + gradient-orientation entropy. A flat plane low; varied rooflines high.
- **depth** — how much tonal range the frame uses (contrast / real lighting).
- **variety** — the **repetition penalty**: luminance autocorrelation at tile-sized offsets. A tiled /
  checkerboard / repeated-prefab frame self-aligns strongly → low variety → it *multiplicatively gates the
  whole scenic score down* (repetition can cost up to 45%).

Run `python3 tools/scenic.py`. This is what flagged the Desert Town "Roblox" problem and confirmed the fix:
after adding per-building clay-hue variation, protruding viga roof beams, recessed window voids with timber
lintels, and ground scatter, Desert Town went from blandest to *most* scenic (silhouette 0.86, full depth).

Use it for: "does this map look hand-built and rich, or flat and repetitive?"

## The authoring loop
1. Generate / author a map.
2. `mapeval` gates it (connected, standable, balanced).
3. `sightline` finds over-exposed lanes → `suggest_cover` proposes where to break them.
4. `scenic.py` flags flat/repetitive surfaces → add material relief, colour variation, silhouette detail.
5. Re-score, repeat.

## `mapqa` — the one-command health dashboard
`cargo run --example mapqa --release` rolls every lens into a single pass over all authored presets: mapeval
grade, sightline structure, liveliness score, triangle count, connectivity, wave-spawn count, and — the part
that catches real breakage — a set of **hard checks** with pass/fail flags:

- `connected` — a room is unreachable from spawn.
- `spawns` — a full enemy wave can't be placed (the wave-spam bug).
- `standable` — a spawned enemy sits over the void / off any walkable surface.
- `geometry` — degenerate triangles or bad normals (holes & shading glitches).

Filter to a subset with `-- <substr>` (e.g. `-- redwood`), or gate CI with `-- --fail` (non-zero exit if any
map fails a hard check). This is what surfaced a fleet-wide degenerate-triangle bug: the LOD impostor pass laid
a flat ground quad over thin-vertical chunks (poles/antennas/reeds), collapsing it to zero-area tris on six
maps at once. Fixing it in `build_impostor` cleared all six. Two regression tests now lock it, so it can't
come back silently. Prefer `mapqa` as the first and last step of any map change — it's the fastest way to see,
across the whole roster, whether an edit helped one map while quietly breaking another.

## `composition` — the anti-gaming "is it designed?" metric (`src/composition.rs`)
The `mapeval` grade rewards proxies that random clutter satisfies by accident: openness is a free point for
open-sky maps, cover is just density near a target, flow rewards a *statistical* sightline-length mix that
scattered boxes produce with zero design intent. So procedurally-cluttered maps grade A/B next to genuinely
hand-composed ones — the metric can't tell **designed space** from **plausible noise**, and a human ranks them
very differently. (This was the correct critique that motivated the module.)

`composition::analyze` measures the thing the old metrics miss — is the arrangement *composed* or *noise-like*?
Its four sub-scores are anti-gaming by construction (none is improved by "add more objects near a density"):
- **clustering** — excess occupancy variance over what a *uniform-random scatter of the same count* would give
  (Poisson baseline). Random → ~0; deliberate masses (buildings, cover nests) → high.
- **void structure** — fraction of open area in a few large connected regions (legible rooms/lanes) vs. shards.
- **landmark hierarchy** — Gini inequality of mesh masses: a readable skyline of a few dominant structures vs.
  a field of identical props.
- **multi-scale** — structure present at both a coarse (district) and fine (detail) grid, as design has and
  noise doesn't.

`honest_quality` / `honest_grade` blend composition with the (already honest) sightline structure score and
hard-penalise noise-like layouts, tuned so few maps earn an A. `examples/score_compare` puts the old grade next
to the new one: nearly every procedural map drops (Foundry Night A→C, Crossfire Mesa A→C, Forward Outpost B→D)
while genuinely-composed authored maps hold or rise (Desert Town → A). The enhancer's objective now includes
composition too, so the recursive builder optimises for *designed-ness*, not the gameable proxies.

The lesson: **every metric that becomes a target stops measuring quality (Goodhart).** The fix isn't a cleverer
single number — it's a metric that scores *arrangement* (which is hard to fake) instead of *counts* (which are
trivial to inflate), plus always validating against an independent signal (here, a stricter human eye).

## The recursive cinematic enhancer + tuning slider (`src/enhance.rs`)
The scorers *measure*; the enhancer *acts*. `enhance(&mut level, tuning, seed, iters)` closes the loop: it
repeatedly proposes a small mutation (add exposure-guided cover, plant a swaying foliage clump, drop an
emissive focal landmark, scatter scenic detail), re-scores a weighted **cinematic objective**
(`sightline` structure + a motion-potential proxy + a landmark bonus), and keeps the change only if it
improves — hill-climbing with exact rollback, so enhancement is monotonic and never vandalises a base map.

`CinematicTuning(0.0..1.0)` is the single dial the map-selection UI exposes as a slider (Off → Subtle →
Balanced → Cinematic → Spectacle). 0 leaves the map exactly as authored; higher pushes harder toward a denser,
livelier layout. The game constructor `Game::new_match_tuned(..)` applies it between map generation and play,
so one base map yields a whole family of varied-but-fun layouts.

**Validated against the independent full-sim harness, not just its own objective.** `examples/enhance_report`
compares the static objective against the real `liveliness` score (a full Game simulation) across tuning
levels. That validation caught a real failure mode: an early version sprinkled *fire* sources to boost
motion, which the static proxy loved but which **tanked** the real liveliness — because the liveliness harness
(correctly) penalises constant motion as churn the eye tunes out. The fix was to (a) make the objective's
motion term peak at a moderate density and fall off past it, (b) prefer *intermittent* motion (wind-bowed
foliage that mostly rests) over *constant* motion (fire/large water), (c) detect maps that are **already**
motion-saturated (a sandstorm, an ocean, a dense forest) and leave them alone, and (d) hard-cap added foliage.
Result: sparse maps climb (Concrete Slab real liveliness 0.47 → 0.56) while already-rich maps hold steady
(Malecón stays 0.74 at every tuning) instead of being wrecked. Six tests lock this in, including one that
checks the *real* score doesn't regress and one asserting ocean/sandstorm maps are recognised as saturated.

The lesson worth remembering: **a recursive builder is only as good as its objective, and a static objective
can confidently optimise the wrong thing.** Always validate the closed loop against the independent arbiter.

## Design specs & generative recipes (`docs/design/`)
- `map-specs.txt` — a large library of themed map briefs (alpine, underwater/swimming, ruins, sci-fi, …) with
  per-POI spatial logic. Source these when picking the next map to build.
- `generative-fsms.txt` — FSM "DNA sequencer" recipes that grow a rich POI from a single seed primitive; they
  map onto the archetypes in `src/fsm.rs` (`SentinelNode`, `LogisticsSpine`, `KineticWreckage`, …).
- `abstraction-methodology.txt` — the volumetric-decomposition method behind the FSM kit.

## Reusable authoring helpers (in `src/level.rs`)
Patterns extracted so new maps vary quickly without re-deriving them:
- `clay_wall_run` / `clay_building_ext` — adobe building kit: material-relief walls, recessed window voids +
  timber frames, weathering gradients, per-building hue variation, protruding viga roof beams, external
  stairs to a walkable roof with a split parapet.
- ground-scatter blocks (sand drifts / half-buried rocks / dry scrub; boulder clusters; rubble fields) — drop
  these into any open floor to kill the flat-plane look.
- `tree_styled(TreeKind::…)` + bark/moss materials — instant forest dressing.
- the `crate::fsm` POI kit — parametric bunker/tower/wreck/shelter builders + `Dressing` detail helpers.
- material palette (24 atlas tiles incl. new **Bark**, **Moss**) — assign per-surface for texture variety the
  scenic scorer rewards.

## The recursive noise-driven generator (`src/procgen.rs`) — proof of concept
A feed-forward map generator built from the design idea "noise → diff → tiny net → FSM archetype → recurse to
character scale". The pipeline:
1. **Noise substrate** — a multi-octave fBm field over the map (the raw 3D profile).
2. **Diff it** — per-region gradient (slope) and variance (roughness) become the macro-topography signals.
3. **Tiny net → activations** — a 6→7 perceptron (`ActivationNet`) maps a region's noise features
   `[height, slope, roughness, openness, depth, bias]` to an activation per FSM archetype. It's small and
   inspectable on purpose: a *swappable taste function*. Hand-set the weights, or learn/evolve them.
4. **Fire the winning FSM** — the chosen archetype (SightlineBreaker / Vantage / Sentinel / Spine / Wreckage /
   Shelter / Void) grows a POI. Selection is soft-sampled from the activations (not pure argmax) so the mix
   stays varied instead of one archetype swamping the map.
5. **Recurse** — every region still bigger than character scale and still "interesting" (high local variance)
   is subdivided into quadrants and the whole process runs again at the finer scale. The first couple of
   levels always split so a map is never a single blob. Stop at ≈character size — finer is just clutter.

Because the same subdivision operator runs at every scale, structure appears at *all* scales — exactly the
multi-scale organisation the composition metric rewards. `examples/procgen_demo` shows it working: swapping the
net's weights on a fixed seed visibly reshapes the archetype mix (a "dense" net floods cover; a "sparse" net
leaves mostly open voids) while the noise keeps the layout spatially coherent.

**Honest status:** this is a proof of concept, not yet in the shipping roster. It generates coherent, varied,
scoreable levels, but raw recursive output currently grades C/D on the honest composition metric (while the old
gameable metric grades it A — the two systems doing exactly their jobs). The clear next steps: (a) let the
leaf FSMs call the real `fsm::*.grow(&Site)` archetypes for hand-authored micro-quality instead of the
self-contained stand-in geometry; (b) **evolve the net's weights** with `composition::honest_quality` as the
fitness function (a genetic/swarm search over taste vectors, blacklisting low-scorers) — now viable precisely
because the fitness function is honest; and (c) feed generated maps through the `enhance` pass for cinematic
polish. The architecture is sound; the quality gap is a tuning problem, and the honest metric is what makes
closing it tractable.

### procgen v2 — dense contextual layer, terrain zones, and evolved taste
Three upgrades closed the gap between "recursive scatter" and "coherent composition":

1. **Dense layer + downward context (the anti-scatter fix).** The net is now two layers (10→8 ReLU→7), and a
   `DownwardContext` is threaded through the recursion: a child's decision is conditioned on its parent's zone
   and archetype and on how much cover the branch has already committed (cover pressure accumulates across
   siblings). Independent per-leaf choices became a coherent hierarchy — a built district stays built, a placed
   landmark suppresses its neighbours, a busy quadrant quiets the next one.
2. **Terrain zones (the "specifics" tier).** A coarse, low-frequency noise band assigns each macro-cell a
   `TerrainZone` (Field / Built / Water / Rubble / Elevated) *before* any detail is placed — the regional
   commitments (a water body, a built-up block) that can't be grown one leaf at a time. The zone then gates
   which archetypes the fine recursion may place, so there's no bunker in a lake or wreck on a pristine field.
3. **Per-instance variation (bigger palette).** Placed props sample the noise for material, proportion and
   weathering, so a field of cover reads as varied crates/plates/blocks instead of identical boxes.

**The evolutionary payoff.** `procgen::evolve_taste` runs a small genetic search over the net's genome
(`to_genome`/`from_genome`) with `composition::honest_quality` as fitness across several seeds. Because that
fitness can't be gamed by adding clutter, evolution *can't* win by filling space — it has to find genuinely
composed layouts. In practice it lifts mean honest-quality from ~0.37 (hand-tuned default) to ~0.51 and, on its
own, discovers *restraint*: the evolved net places far fewer POIs in far more open space (e.g. 40 obstacles
with a void-heavy mix), generalising to held-out seeds. This is the concrete demonstration that an honest,
un-gameable objective is what makes the swarm/genetic idea actually work — the metric and the search are two
halves of the same tool.

### procgen v3 — real FSM archetypes + a content-aware fitness
The leaf placer now calls the **real hand-authored `fsm` archetypes** (`SentinelNode`, `LogisticsSpine`,
`KineticWreckage`, `VerticalRiskReward`, `AdHocShelter`, `SightlineBreaker`) instead of flat stand-in boxes.
`grow_fsm` builds a `fsm::Site` from the region's noise-derived features (sightline from openness, high-point
from height, slope, wet from a Water zone, a stable danger bearing from the field) and calls the machine's
`grow`; if the FSM's preconditions aren't met it declines and procgen falls back to the lightweight stand-in.
The result: generated maps now carry **~8–15k triangles of authored micro-quality** — L-shaped concrete
bunkers with sandbags, braced water-tower landmarks with warning lights, three-section sheared wrecks with
shattered glass — while still passing the same hard bar as authored maps (connected, spawns a full wave, zero
degenerate geometry; all regression-tested).

This cleanly separated two problems that were tangled before: **micro-quality** (how good each POI looks — now
solved by the FSMs) and **macro-composition** (how POIs are arranged — still the domain of the net + recursion
+ composition metric). With rich geometry in place, the evolutionary loop's job became purely about placement.

The evolution fitness gained a **one-sided content floor**: `honest_quality` alone rewards emptiness (an open
field is maximally un-noisy), so a pure-composition objective drove the net toward near-empty maps. Adding a
floor that penalises *too-bare* maps — without rewarding density past the floor (the composition term already
punishes over-fill as noise) — keeps evolution in the "well-composed AND populated" sweet spot. Evolved nets
now settle around ~100 well-placed authored POIs and grade a solid **C** (vs. authored maps' B/A), which for
fully-automatic output is a real result. The remaining gap to authored quality is macro-layout intent
(deliberate lanes, a designed spawn-to-spawn flow) — the natural next target, and one the composition metric
already gives an honest compass for.

## The Rosetta stone — text spec → themed map, by guided search (`src/rosetta.rs`)
The procgen maps were structurally sound but thematically blank (grey bunkers on empty ground). The design doc
`docs/design/map-specs.txt` holds 80 richly-themed map briefs, so the Rosetta module bridges them to procgen —
**not** by parsing prose into geometry (brittle), but as a **guided-search transfer**: mine each spec into a
target *signature*, then steer the procgen search toward it.

`extract(spec) -> ThemeTarget` keyword-mines a spec into: a **palette** (colour words → RGB; surface colours by
frequency, but the *sky* colour is taken from words appearing near atmosphere terms like "sky/fog/light", so a
"stormy purple sky" wins over an incidental "white ceramic" panel), an **archetype mix** (POI-role words like
"landmark", "flank cover", "wreck", "bunker" → archetype fractions), a **density** (open/dense/cramped words), a
**water** and **verticality** push (flooded/sunken/underwater and elevated/multi-tiered/tower words), and a
**mood** string. The keyword tables are small and auditable; extend them to cover more specs.

`generate_themed(seed, net, params, target)` applies the transfer to geometry: the palette restyles the whole
level; the target's water/verticality bias the **zone distribution** (`water_bias`/`vert_bias` widen the water
and elevated bands in `zone_at`); a `zone_lean` shifts the built-vs-rubble split from the theme's dominant
archetypes (wreck-heavy → more Rubble zones); and an **archetype-selection bias** (from the target mix) nudges
each leaf's choice directly, so placement follows the theme instead of relying on net weights alone.

`evolve_toward(target, …)` is the guided search: it evolves a net whose themed maps maximise
`match_score(target, mix, level)` (archetype-mix + density + water + verticality match) blended with a floor of
honest composition (so it can't match the theme by making junk). Across the specs, guided search measurably
improves the match (e.g. Elevated Spine 0.46→0.60, Iron Reef 0.55→0.63, Karst 0.75→0.85 held-out), water scales
with the theme (dry specs ~3 bodies, flooded specs ~10), and palettes come out visibly distinct (Archivist
warm-pink, Elevated Spine stormy-purple, Gondola cold-white).

**Honest limits.** The transfer captures *composition and mood*, not literal props — it builds "an alpine
station", not the bright-yellow gondola with slashed red seats the prose describes. The realised archetype mix
is constrained by zone plausibility (a "flooded wreckage" theme can't put the wreck FSM underwater — water zones
forbid it), so some mixes compromise. And match scores sit in the 0.5–0.9 band, not 1.0. But the pipeline is
real and end-to-end: prose reference → mined signature → steered procedural map, with the honest metrics keeping
the search from gaming its way to a match.
