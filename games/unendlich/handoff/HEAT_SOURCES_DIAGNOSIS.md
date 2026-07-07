# `fires: 269000` — diagnosis & fix (heat-source map)

## What "fires" actually is
The FPS-line `fires:` count is `heat_sources.len()` — the dynamic **HeatSourceMap** (lava, plus
player F9 fires). Not creatures, not a fire-spread sim.

## It is NOT a leak (good news)
- Lava registration is **deduped** by a `lava_registered: Set<ChunkCoord>` — a chunk registers once.
- Eviction removes by bbox; the added source positions (`origin+l+0.5`) fall inside the chunk's
  `[origin, origin+CHUNK_SIZE)` bbox, so `remove_within_bbox` matches them all. No orphans accumulate.
- **Nothing in the dragon / seasons / weather paths touches `heat_sources`.** Your hunch that they
  "shouldn't breed fires" is correct — they don't. 89k→269k just tracks loading ~2.4× more terrain
  at altitude (loaded 3784→8975); the count is **load-proportional**, ~30 sources / loaded chunk.

## The real bug — per-block sources × an O(n) field
1. `register_lava_heat_sources` emits **one HeatSource per surface-lava block**. With CHUNK_SIZE=32,
   a lava-rich chunk emits hundreds; summed over thousands of subterranean chunks → ~269k.
2. `HeatSourceMap::total_heat_at` is a **linear scan over ALL sources** (`v.iter().map(contribution_at).sum()`),
   no spatial index. It's called by `temperature_at_with_heat` in hot paths (convection sampling at
   y and y+16, plant lifecycle, ice checks). At n=269k every temperature query walks a quarter-million
   sources. The module comment literally says "fine for tens to a few hundred" — it's 1000× past that.
   This is a large, hidden slice of `tick_sim` / plant_lifecycle.

## Fix A — cluster lava sources (shipped: fix_lava_heat_clustering.patch)
Coarsen to one source per BIN³ region of surface lava (BIN=16 ⇒ ≤8/chunk) at each region's centroid,
radius 6 to cover. Centroids stay inside the chunk bbox, so eviction removal is unchanged. Expected
~30× fewer sources (269k → ~9k) ⇒ proportionally cheaper temperature queries, zero visible change
(the field is a smooth gaussian). Tunable: BIN=CHUNK_SIZE ⇒ one source per lava chunk (max cut).
Apply on your tree: `git apply fix_lava_heat_clustering.patch` (or `patch -p1 < …`).

## Fix B — spatial index — IMPLEMENTED & TESTED ✓ (outputs/heat_source.rs)
Drop `outputs/heat_source.rs` over `crates/unendlich-engine/src/heat_source.rs`. API-preserving
(verified: identical public surface; engine imports only HeatSource + HeatSourceMap). Internals:
id→source map + a spatial-hash grid (CELL=16). `total_heat_at` scans only the cell neighborhood
within reach of the query (reach widens automatically if a large-radius source is ever added), so
it is O(nearby) regardless of total count. `remove_within_bbox` (chunk eviction) and
`remove_nearest_within` (F9) are now O(nearby) too — both were O(n) before. 15 rustc tests pass,
including: 2000-source scatter where the indexed sum == brute-force sum at many points; a
5000-far-source test proving a local query is byte-identical regardless of distant count; bbox
max-exclusive; large-radius reach; plus all 9 original parity tests. Minor: max_radius only grows
(conservative over-scan if the biggest source is removed — harmless). With Fix A (clustering) the
heat system stops being a scaling concern at any count.

## Fix B — original spec (for reference)
Even clustered, the query stays O(n). Replace the flat `Vec<HeatSource>` with a **grid / spatial
hash** keyed by floor(pos / CELL) (CELL ≈ 8–16). `total_heat_at(p)` then sums only sources in the
3×3×3 cell neighborhood around `p` (sources past 2×radius already contribute ~0), making queries
O(nearby) regardless of total count. Keep the public API drop-in:
- `add` returns a stable id (monotonic counter) instead of a vec index; back it with `id→(source,cell)`
  + `cell→Vec<id>`. `remove(id)`, `remove_nearest_within`, `remove_within_bbox`, `add_many`, `snapshot`,
  `total_heat_at`, `len`, `is_empty` all preserved.
- The existing tests (round-trip add/remove, linear sum, gaussian falloff, bbox dereg) are the safety
  net; add: "query cost independent of far sources", "neighborhood sum == old global sum within eps".
This is a clean, pure, testable module change — a good worker core (T02/T03) when threads resume, or
I can implement + rustc-test it here on request.

## Bonus: this compounds with the remesh_gate no-op
`remesh_gate skipped=0` ⇒ near chunks remesh ~18× each. Each remesh re-meshes (CPU) but does NOT
re-register lava (dedup guard), so the gate doesn't worsen the heat count — but fixing the gate still
cuts the near-mesher tail (separate item, see MESH_TIER_PLAN.md / PERF-LEVERS).
