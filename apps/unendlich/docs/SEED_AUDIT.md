# SEED AUDIT — "two seeds in one world" (r346)

## Root cause
There was no single live world-seed. At startup the terrain generator + a field-model + a
colony + weather were each seeded independently from `WORLD_SEED_DEFAULT` / hardcoded constants
(the menu-backdrop world). A world launch called only `chunk_manager.rebuild_for_seed(seed)` —
which rebuilds the TERRAIN GENERATOR ONLY. Every other seed consumer kept the old/default seed,
so a launched world had terrain from the picked seed stitched onto overlays/creatures/weather
from a different seed. That is the "two seeds in the same world" symptom (esp. from the title →
Create World path: you preview/create with seed A but the overlays + spawns are seed B).

## Divergence vectors found (bin/unendlich_fullgame.rs)
| # | consumer | was | now |
|---|---|---|---|
| 1 | **field overlays** (moisture/biome/wind) — `field_registry` over a SEPARATE `WorldModel` | built once at `WORLD_SEED_DEFAULT` (l.568), Arc-shared, **never rebuilt on launch** | rebuilt on launch via new engine API `chunk_manager.set_fields(..)` with a model at the launched seed |
| 2 | **creature/dragon spawns** (4 `wseed` sites + 3 arg sites) | hardcoded `WORLD_SEED_DEFAULT as u64 ^ …` | read the live `fg_world_seed` |
| 3 | **popdyn Colony** (launch reseed) | `Colony::seeded(24, 0xFA1A)` (fixed) | `Colony::seeded(24, fg_world_seed ^ 0xFA1A)` |
| 4 | **weather** | fixed env/default seed shared by ALL worlds (l.1139) | reseeded per-world on launch: `Weather::new(fg_world_seed ^ 0x5EED…)` |

## Fix
- Engine: NEW `ChunkManager::set_fields(&mut self, Arc<FieldRegistry>)` — runtime swap of the
  field registry (clears cached per-chunk field grids so they recompute for the new model).
  Mirrors `with_fields` but post-construction. chunk_manager brace-balanced.
- Bin: NEW single live `let mut fg_world_seed: u64 = WORLD_SEED_DEFAULT as u64;` (the backdrop
  default before launch, the picked seed after). All in-game seed consumers route through it.
  Launch `Ok` block now: `fg_world_seed = seed;` + rebuild field overlays + reseed weather +
  reseed colony. Startup constructions (l.519 TerrainGenerator, l.569 field model) stay
  `WORLD_SEED_DEFAULT` (the backdrop) — fg_world_seed starts equal, so no behavior change pre-launch.
- Verified: bin brace/paren depth 0 (raw-string/comment/char aware), engine setter balanced.
  Cannot full-build here (bin) — operator M2 build is the gate.

## Other seed paths checked & deemed OK (not divergences)
- `seed: fg_world_seed ^ (frame_count)` ephemeral effect spawns — now world-keyed too (was default).
- `model_for_fields` was intentionally a *separate* model from chunk_manager's; the fix keeps them
  in sync by rebuilding it on launch (rather than coupling them) — minimal change.
- Per-world persistence (r163/r291) already keys saves by name+seed correctly.

## Possible-bug surface for future (where this class recurs)
Any NEW seed consumer added to the event loop MUST read `fg_world_seed`, not `WORLD_SEED_DEFAULT`.
Candidates to watch when wired: client/server world join (the JOINER must adopt the HOST's seed via
the world-join handshake, not generate its own), and the title-screen preview (already adopts the
rolled seed via `fg_last_preview_seed` → Create World).
