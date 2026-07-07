# Fog ↔ Vision ↔ Shader integration (r641/r642)

## The chain
1. **Shader (already present):** `chunk.frag` renders Beer-Lambert fog with a
   humidity-thickened density: `base · mix(0.6, 2.2, humidity^1.6)` (line ~1227),
   `exp(-σ_t·depth)` extinction. Base density `0.012` in the bin (NO_FOG → 0).
2. **CPU model (`sim::fog_model`):** same physics, plus `shader_fog_density(base,
   humidity)` which reproduces the frag curve *exactly*, so the CPU and GPU agree
   on how dense the fog is. `visibility_distance(σ_t, floor)` inverts Beer-Lambert.
3. **Vision (`sim::eyes`):** `EyeProfile::in_fog(σ_t)` clamps sighting range to the
   fog visibility distance (5% contrast floor). `sees_in_fog(eye, σ_t, ...)` is
   `sees` with that clamp. Result: **a creature sees exactly as far as the player
   can before fog closes in** — no shooting/aggroing through a white wall.

## Wired in `unendlich_fullgame.rs`
- `frame_fog_sigma` (fn scope, ~1600): fog extinction at the player column.
- Set each climate tick (~4122) from `chunk_manager.model_arc().humidity(player)`
  via `shader_fog_density(0.012, humid)`; `NO_FOG` zeros it. Refreshed on the
  existing `frame_count % 30` humidity sample — no extra cost. One-frame-stale is
  fine (fog drifts slowly).
- Perception site (~7560): `eyes::sees_in_fog(senses.eyes, frame_fog_sigma, ...)`.

## Still open (optional)
- `unendlich_world.rs:1866` has the other `eyes::sees(...)` call (secondary bin).
  Same one-line swap to `sees_in_fog` + a `frame_fog_sigma` there if that bin is
  used.
- **Per-creature fog** (vs player-column): `frame_fog_sigma` uses the player's
  humidity as the local fog value. For very large maps with strong humidity
  gradients you could sample humidity at each creature instead — but that's a
  `model_arc().humidity()` call per creature per frame; the shared value is the
  cheap, good-enough default.
- **Graceful contrast falloff:** `in_fog` is a hard range cap at the 5% floor.
  A softer model would also scale apparent contrast by `transmittance(σ_t, dist)`
  before the acuity test, so targets blur out approaching the limit rather than
  popping. `fog_model::transmittance` is ready if you want that refinement.

## Verified
`fog_model` 14 tests (incl. `shader_fog_density_matches_frag_curve`, analytic
airmass == numerical, phase fns integrate to unity). `eyes`+`fog_model` assembled
7 tests (incl. `target_visible_in_clear_air_is_lost_in_fog`). Bin edits
delimiter-checked; need your build to compile-confirm.
