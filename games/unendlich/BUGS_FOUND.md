# B1 — Bughunt report (r102)

Pure-core correctness hunt across `sim/` and `world/`. One real defect fixed with
a fail-before/pass-after regression test; the rest of this file records the
modules audited and found robust (honest negatives), so the next hunt doesn't
re-walk them.

## BUG FIXED — 11 BlockId variants missing from TryFrom<u8>, and creative-mode weapons couldn't be held

Two related bugs found while chasing a user report: "inventory/hotbar doesn't load item models, blocks stay placeable that shouldn't be, scroll wheel is tied to some invisible list that doesn't reflect the creative tabs/menu, and weapons don't work at all — never seen one, can't use abilities."

**Bug 1 — `impl TryFrom<u8> for BlockId` (`world/blocks.rs`) was missing 11 consecutive variants**, discriminants 81..=91: `MossyCobblestone`, `PolishedStone`, `SmoothSandstone`, `Terracotta`, `ChiseledStoneBricks`, `DarkOakPlanks`, `PolishedBasalt`, `Carrot`, `Potato`, `Cabbage`, `Sprout`. All 11 are real, present in `BlockId::ALL`, but the match was hand-maintained and simply never got these arms added. Found by a round-trip test I wrote for the (unrelated) weapon fix below — `Block(MossyCobblestone).to_item_id()` came back as generic "Item" instead of its real name, which traced straight to `try_from` returning `Err` for id 81.

Blast radius (every production caller of `BlockId::try_from`, checked individually):
- `storage.rs` (save-game loading) — a saved chunk containing any of these 11 blocks would misread on load.
- `octree.rs`, `gpu_generator.rs`, `gpu_chunk_pool_vk.rs`, `surface_run.rs` — all treat `Err` as "unknown → Air", so these blocks silently vanish (render as empty air) on the GPU/octree paths.
- `forage_harvest.rs` — can't gather them (two of the eleven are literally the crops `Carrot`/`Potato`).
- `chunk_manager.rs` (edit-log replay), `unendlich_fullgame.rs`/`unendlich_server.rs` (network `Place` command) — silently drop the edit/placement entirely.
- `item.rs`'s `item_display` — shows generic "Item" name/color instead of the real one (this is what my test caught).

Fix: added the 11 missing match arms with their exact documented discriminants. Added `try_from_u8_covers_every_blockid_in_all`, a regression test that walks the entire `BlockId::ALL` and asserts every variant round-trips — so a future variant added without a `TryFrom` arm fails CI immediately instead of silently misbehaving in a release build.

**Bug 2 — creative-mode `hand_item` could never be a weapon (`bin/unendlich_fullgame.rs`, `item.rs`).** The creative UI panel already lets you pick any `ItemKind` (`Block`/`SpawnEgg`/`Ingot`/`Tool`/`Legendary` — confirmed by its type and click handlers, stored in `renderer.creative_place_item`), but the game loop's `hand_item` (what's actually held/used/rendered as the viewmodel) was derived by reinterpreting `selected_block: BlockId` as a raw `ItemId` (`selected_block as u32`) — which can structurally never represent a weapon. Any weapon/tool/egg pick from the creative catalog was silently dropped before it reached your hand; only `ItemKind::Block` picks ever got through (via the separate r657 sync into `selected_block`).

Fix:
- Added `ItemKind::to_item_id(self) -> u32` in `item.rs` — the correct, documented inverse of `item_display`'s numeric-id convention (`0..256` block via discriminant, `1000+idx` ingot, `4000+idx` legendary weapon by class — lossy on element/tier/seed, matching the convention's own existing lossiness, since the real `Inventory` already only ever stores bare `ItemId(u32)`). `Tool`/`SpawnEgg` have no established range in the convention, so they map to a documented sentinel (`ItemKind::UNMAPPED_ID`) rather than guessing one.
- `unendlich_fullgame.rs`'s every-frame `hand_item` sync (r487) now checks `renderer.creative_place_item` first (any `ItemKind`, via `to_item_id`), falling back to the old `selected_block`-based path only when nothing's been picked through the panel (i.e. G/V-cycle or PLACE_PALETTE scroll/number-keys, which are still `BlockId`-only — see the separate, larger, NOT-fixed-here issue below).
- Because `creative_place_item` is sticky (persists frame-to-frame) and now takes *priority* for `hand_item`, the scroll-wheel, number-key (1-9), and ALT+home-row hotbar handlers all needed to write their `BlockId` pick back into `renderer.creative_place_item` too — otherwise scrolling past a panel-picked weapon would change `selected_block` but leave the weapon stuck in your hand. Added that sync to all three sites (mirrors the pattern the G/V handler already used for exactly this reason).

**Follow-up (same session): both deferred items above are now fixed too.**

- **The hardcoded `PLACE_PALETTE: [BlockId; 9]` hotbar is gone from creative mode.** Turned out there was no UX design decision to make — `renderer.creative_hotbar: [Option<ItemKind>; 10]` already existed as the REAL, on-screen hotbar (the panel already lets you assign any item, including weapons, into its 10 slots; a comment in `world_renderer.rs` already calls it "the single source of truth" for the HUD row). `PLACE_PALETTE` was a second, entirely disconnected 9-block array that the scroll-wheel and number-key handlers walked instead — literally "the invisible list" from the report, since it never had any relationship to what the visible hotbar showed. Added `apply_creative_hotbar_slot()` and routed the scroll-wheel, number-keys (1-9), and ALT+home-row handlers through it in creative mode (survival is untouched — it already correctly reads the real `Inventory`). Scrolling in creative now cycles all 10 real hotbar slots instead of 9 hardcoded blocks.
- **Placement is now gated on what's actually held, in both modes.** Added `held_placeable_block()` — computed fresh at each of the two placement call sites from `creative_place_item` (creative) or the real inventory slot (survival), instead of the possibly-stale `selected_block` global. Holding a weapon (or an empty hotbar slot) now correctly means right-click does nothing, rather than placing whatever block happened to be selected before. Fixed at both placement sites (there were two near-identical copies of this logic, likely for a large-character branch vs. the normal one).
- Also had to fix a subtlety this uncovered: since `creative_place_item` is now authoritative for `hand_item` (previous entry), an *empty* hotbar slot needs to mean "empty hands," not "fall back to a stale block" — removed the `selected_block`-based fallback from the r487 sync entirely, since every creative selection path now writes through `creative_place_item` including the `None` (empty slot) case.

**Self-caught regression before it shipped:** `creative_hotbar` initializes as `[None; 10]` — completely empty. Before this fix, that didn't matter (scroll/keys used `PLACE_PALETTE`, which always had 9 real blocks). After wiring scroll/keys through `creative_hotbar`, a fresh creative session would start with **nothing** placeable until the player manually dragged items into the hotbar via the panel — a real usability regression I introduced and then caught by re-checking the initializer. Fixed: `creative_hotbar` now starts pre-populated with the same 9 blocks `PLACE_PALETTE` used to provide (Stone, Dirt, Cobblestone, OakLog, OakLeaves, Sand, Glass, OakPlanks, Water), in the real, visible hotbar this time instead of a disconnected shadow array. Slot 10 (unreachable by number keys 1-9) starts empty, same as any hotbar slot the player hasn't assigned yet.


- `cargo check --no-default-features --features "render window ui-overlay" --bin unendlich-fullgame` → clean, same 12 pre-existing warnings, no new ones (confirms `selected_block` is still used elsewhere, no dead code introduced).
- Full lib suite re-run after these changes: 8650 passed, same single pre-existing environment-only failure.
- **Still not verified: on-device.** Same gap as before — I can compile and type-check the real binary but can't run it. What I specifically can't confirm from here: that scrolling through the creative hotbar actually feels right, that an empty slot correctly shows empty hands in the viewmodel, and that placement is correctly blocked while holding a weapon. All three are now *wired* — none are *seen*.

**Verification:**
- `cargo test --no-default-features --features mesh-ref --lib blocks::` → 17/17 (16 pre-existing + 1 new coverage test).
- `cargo test --no-default-features --features mesh-ref --lib item::` → 26/26 (22 pre-existing + 4 new `to_item_id` tests).
- Full lib suite: 8650 passed, 1 pre-existing environment-only failure (unrelated `shader_artifacts` test, reproduces on unmodified `main`).
- `cargo check --no-default-features --features "render window ui-overlay" --bin unendlich-fullgame` → clean, no new warnings.
- **Not verified: on-device.** I have no display/GPU here — I can't confirm the viewmodel actually renders a weapon, that the ability/attack input path works once a weapon is truly held, or that the hotbar-sync fix feels right in practice. This needs your device.



**Where:** `world::chunk_manager` (`reset_with_generator`, the F4 `rebuild_for_seed`
path) + the async mesher/renderer that consumes mesh results.

**Root cause.** `reset_with_generator` swaps in a new `TerrainGenerator`, rebuilds
the chunk loader, and clears `chunks` / `octrees` / `chunks_with_transitions` so
the world regenerates from the new seed. Meshing is asynchronous, so mesh jobs
issued for the *previous* world can still be in flight when the reset lands. When
such a job completes, its result is geometry for a world that no longer exists —
but nothing distinguished a late, stale result from a fresh one. The renderer
could therefore briefly upload stale geometry into the freshly-regenerated world
(a flash of the old terrain at a chunk coordinate that now holds something else).
There was no epoch/generation concept anywhere in `chunk_manager` (confirmed by
grep) to tell the two apart.

**Fix (pure, in-core).**
- `ChunkManager` gains a monotonic `world_epoch: u64`, initialised to `0`,
  **bumped on every reset** (`reset_with_generator`, which `rebuild_for_seed`
  calls), exposed via `pub fn world_epoch(&self) -> u64`.
- New pure module `world::mesh_epoch` with the decision point the renderer/mesher
  uses: `mesh_result_is_current(result_epoch, current_epoch) -> bool` (accept iff
  equal) plus a `WorldEpoch` newtype with a **saturating** `bumped()` (so a
  practically-impossible overflow can never wrap a fresh world back onto an old
  epoch and silently re-accept stale geometry).

**Tests (fail-before / pass-after).**
- `world::chunk_manager::tests::reset_bumps_world_epoch` — epoch starts at 0 and
  strictly increases across both `reset_with_generator` and `rebuild_for_seed`,
  and the `mesh_epoch` gate rejects a pre-reset epoch while accepting the current
  one. *Fail-before:* `world_epoch()` did not exist and no bump occurred, so the
  staleness was unobservable — the test could not even be written against the old
  API; with the old behavior a late result was indistinguishable from fresh.
- `world::mesh_epoch::tests` (4): `accepts_current_rejects_stale`,
  `bumped_is_strictly_increasing_and_saturating`, `typed_gate_matches_raw`, and
  `models_the_defect_a_late_result_after_reset_is_dropped` (an issued-then-reset
  job's result is dropped; a post-reset job is accepted).

**Handoff (GPU side — cannot compile in the worker sandbox, propose only).**
The mesh pipeline should: (1) record `mgr.world_epoch()` when a mesh job is
*issued* (store it on the job/result), and (2) on completion, drop the result
when `!world::mesh_epoch::mesh_result_is_current(job_epoch, mgr.world_epoch())`
before it reaches the upload path in `render::world_renderer`. The epoch only
ever changes in `reset_with_generator`, so the check is a single integer compare
on the result-intake path. No descriptor/pass changes.

**Verification (run in-sandbox):**
- `cargo test -p unendlich-engine --features gpu --lib mesh_epoch` → 4 passed.
- `cargo test -p unendlich-engine --features gpu --lib reset_bumps_world_epoch` → 1 passed.
- `cargo test -p unendlich-engine --features gpu --lib reset_with_generator_can_round_trip_seeds`
  → 1 passed (no regression to the existing reset behaviour).
- `cargo check -p unendlich-engine --features render` → Finished (render gate; pre-existing warnings only).

---

## Feature: cross-chunk seam culling — mesher-side AND pool wiring both done

`get_local`/`block_at` have always treated any out-of-chunk read as Air (documented as "seam culling with neighbor chunks is a future optimization"), so every chunk boundary renders both sides' faces even when the neighbor is solid -- pure wasted geometry (perf-only defect: extra triangles, never a missing face, so it was safe to leave). Implemented both halves:

**Mesher side** (`render/mesher.rs`):
- `NeighborChunks { neg_x, pos_x, neg_y, pos_y, neg_z, pos_z: Option<Arc<Chunk>> }` -- one optional neighbor per face direction. `NeighborChunks::none()` is the all-`None` default.
- `block_at_with_neighbors(chunk, coord, neighbors)` -- like `block_at`, but when `coord` is exactly one block past this chunk's edge on exactly one axis (the only shape the greedy mesher's mask build ever produces), it wraps the coordinate into the matching neighbor's local space and reads that chunk's real block instead of assuming Air. Any other out-of-range shape, or a `None` (unloaded) neighbor slot, falls back to plain `block_at`'s Air-at-OOB -- identical to today's behavior.
- `mesh_chunk_greedy` is now a thin wrapper over a shared `mesh_chunk_greedy_impl(chunk, neighbors, ...)`, called with `NeighborChunks::none()` -- every existing call site (~35, including all pre-existing tests and `mesh_bench`/`winding_check`/`geom_probe`) is unaffected by construction.
- New `mesh_chunk_greedy_with_neighbors(chunk, &neighbors, sky, world_model, current_tick)` is the seam-aware entry point.

**Pool wiring** (`render/meshing_pool.rs`, `bin/unendlich_fullgame.rs`):
- `MeshRequest` gained a `neighbors: NeighborChunks` field (`NeighborChunks::none()` added to all 8 existing test literals -- mechanical, verified unchanged).
- The worker's Near-LOD dispatch (the CPU greedy-mesh path -- this is the *live* path today since `gpu_mesher_mode` is hardcoded to `0`) now calls `mesh_chunk_greedy_with_neighbors(&req.chunk, &req.neighbors, ...)` instead of the old `mesh_chunk_with_sky*` convenience wrappers. Those three wrapper fns are now unused from this call site (still `pub`, still used elsewhere -- `mesh_chunk_with_sky_and_model` is used by `chunk_renderer.rs`, `mesh_chunk_with_sky` by a `meshing_pool` test); their unused imports were removed from the worker fn's scope.
- `unendlich_fullgame.rs`'s submit loop now fetches all 6 face-neighbor `Arc<Chunk>` handles via `chunk_manager.get_chunk(...)` (cheap `DashMap` reads, mirrors the exact pattern the existing v0.24.32 octree-prewarm code next to it already uses for a different purpose) and populates `req.neighbors` before submitting. An unloaded neighbor naturally yields `None` in that slot, which the mesher already treats as Air -- so this cannot punch a hole at the edge of loaded terrain, by construction rather than by a runtime check.
- Neighbor-edit dirty propagation needed nothing new: `ChunkManager::set_block`'s `bump_neighbor_if_edge` already existed and is already tested (`set_block_at_chunk_boundary_bumps_neighbor_version`, `set_block_at_y_edge_bumps_y_neighbor`, `set_block_at_corner_bumps_three_neighbors`) -- before this change it caused a harmless redundant re-mesh; now it's load-bearing (it's what re-triggers a boundary chunk's re-mesh so a culled face reappears if the neighbor's edge block is removed).

**`unendlich-world` (the other binary) was left untouched** -- attempting to compile it (needed `cmake`/GLFW dev libs, which I had to `apt-get install` in this sandbox since they weren't present) surfaced that it **already fails to compile on unmodified `main`**: 7 pre-existing type errors (`WindowEvent`/`InputEvent` mismatches, `MouseButton`/`MouseBtn` mismatches, a missing `AtmosphereParams::wave_speed` field) unrelated to anything here. That binary has drifted independently and needs its own pass; I didn't touch its `MeshRequest` literal (which was also missing the `epoch` field from a separate, earlier change) since I have no way to verify edits to code that doesn't build in the first place.

**Tests added** (`render::mesher::tests`): `neighbor_none_matches_plain_greedy` (delegation is exact), `seam_culling_drops_boundary_face_against_solid_neighbor`, `seam_culling_keeps_boundary_face_against_air_neighbor`, `seam_culling_symmetric_across_the_boundary` (both sides of a shared face cull independently -- the actual win, a wall of loaded chunks stops rendering internal seams), `seam_culling_all_six_directions`, `seam_culling_diagonal_out_of_bounds_falls_back_to_air`.

**Verification:**
- `cargo test --no-default-features --features mesh-ref --lib mesher` -> 84/84 (78 pre-existing + 6 new), confirming the seam-culling logic is correct and `mesh_chunk_greedy`'s output is byte-for-byte unchanged.
- `cargo check --no-default-features --features render-core --lib` -> clean.
- `cargo check --no-default-features --features "render window ui-overlay" --bin unendlich-fullgame` -> clean (the one that matters: the real, live binary, with the actual Vulkan/ash types the pool wiring touches, not just the headless `mesh-ref` shim).
- Did **not** get a full `--features render-core` test-binary run to completion here -- ash/Vulkan linking was still going at the sandbox's timeout. The type-level checks above are solid evidence the wiring is correct, but I have not run `meshing_pool.rs`'s own concurrency tests against this change, and I have no device to confirm the actual on-screen behavior (no holes at the render-distance edge, seams actually disappearing). **Recommend running `cargo test --features render-core meshing_pool` and an on-device fly-to-the-edge check before calling this shipped.**

Expected win: eliminates one full quad's worth of triangles per solid-adjacent chunk-boundary face pair across the whole loaded world. No device here to measure the actual frame-time delta.

## Perf: self-audit caught a double bounds-check in block_at_with_neighbors

Right after landing the seam-culling change above, re-read my own hot path: `block_at_with_neighbors`'s in-bounds fast case computed the 3-axis range check, then called `block_at` (== `get_local`), which redoes the *identical* 3-axis check. This is exactly the redundant check r698 deliberately removed from `get_local`'s own fast path (its doc comment: "get_local is the mesher's hottest call ... dropping the redundant check is a real inner-loop win"), reintroduced one call site up by my new wrapper. Since this call site replaced `block_at` directly in the greedy mask build's per-face loop (~6M calls/chunk per that same comment), it was double-checking bounds on every single face check in the game's live meshing path.

Fixed: the fast path now indexes `chunk.blocks[Chunk::block_index(x, y, z)]` directly once bounds are proven, matching `get_local`'s own discipline, instead of routing through `block_at`. Verified: `cargo test --features mesh-ref --lib mesher` → 84/84, unchanged.

Flagging this one specifically because it's a regression *I* introduced in the same session, not a pre-existing issue — worth remembering that new hot-path code needs the same "prove bounds once" scrutiny as everything else in this file already has.

## Perf: far/very-far mesh regen — real, quantified, already diagnosed, NOT flipped (needs a device)

`docs/design/PERF_FINDING_meshing_and_steady.md` already found this and it's still live: Far/VeryFar chunks cost **11-13ms each** to mesh by default, because `mesh_chunk_far_virtual` re-samples the world generator's density+cave noise per grid cell instead of reading blocks already sitting in `req.chunk`. A later comment (`r-perf-far-regen`, in `meshing_pool.rs`) confirms the chunk is *always* already materialized by the time a request reaches this dispatch, so the original "avoid materializing a chunk you'll mostly discard" justification doesn't hold in the reachable code paths today.

There's already a working escape hatch: `UNENDLICH_FAR_MESH_FROM_CHUNK=1` switches Far/VeryFar to `mesh_chunk_downsampled` on the real chunk, defaults off. **I did not flip the default** — checked the two paths' actual behavior first (comments in `mesh_chunk_far_virtual`'s header + the `virtual_far_and_materialised_comparable_triangle_count` test, which explicitly only checks triangle counts agree within 10x, not visual equivalence) and confirmed they are NOT visually identical: the virtual path has no biome variation, no ore/water, and different terrain silhouettes at cell boundaries (centre-sample vs. majority-of-8 vote). Flipping the default is very likely a net visual improvement (real biome/water/ore at Far distance) as well as the performance win, but it's a visible change to every Far/VeryFar chunk in the game and needs eyes on a real render, not a sandbox guess.

**Recommended next step (zero code risk):** launch with `UNENDLICH_FAR_MESH_FROM_CHUNK=1` and A/B the frame-time chug and the look of distant terrain against default. If it looks right, flipping the default (or inverting the flag to opt-out) is a one-line, low-risk follow-up.



`sample_env_at_cached` memoised `sample_env_at` in a `thread_local! RefCell<HashMap<(BlockId, i32, i32, i32), EnvironmentSample>>`, hit up to 5× per block (top + side faces) per mesh pass. Same shape as the r695/r697 corner-cache HashMap that was already converted to a flat generation-tagged array — this one hadn't been.

Converted to the same pattern: flat `Vec<(EnvironmentSample, u32)>` sized `CHUNK_SIZE³`, indexed by chunk-local `(x, y, z)`, generation bump instead of `.clear()`. `block` was dropped from the key — within one pass a chunk-local position holds exactly one block, so it was redundant. Kept an uncached fallback for any position outside `0..CHUNK_SIZE` (defensive; both current call sites always pass an in-chunk block position, so this branch isn't exercised today). `EnvironmentSample` needed `Default` added (was already `Copy`).

**Verification:** `cargo test --no-default-features --features mesh-ref --lib mesher` → 78/78 passed (byte-identical output, same as the corner-cache precedent). Full `mesh-ref` lib suite: 8639 passed, 1 pre-existing failure (`shader_artifacts::committed_spirv_is_fresh_against_transitive_source`, unrelated file, reproduces on unmodified `main` too — looks environment-dependent, needs `glslang-tools` this sandbox doesn't have).

Not yet benchmarked on-device — this is a lookup-cost reduction (array index vs. hash of a 4-field key + one fewer field to hash), same class of win as r697's corner cache, not a new algorithm.

## Audited and found CORRECT (documented negatives)

These were checked against the bug families the brief named and found robust;
recording them so the hunt isn't repeated.

- **Noise lattice floors** (`world::ridge_noise`, `terrain_warp`,
  `batch_ridge_noise`, `batch_terrain_warp`, `scatter_cluster`): all compute
  `x0 = x.floor()` *before* `x0 as i32`, so the lattice cell and the fractional
  part are correct at negative coordinates (no trunc-toward-zero seam at x=0).
- **`Chunk::block_index(...) as u16`** (`world::plant_lifecycle`): safe —
  `CHUNK_VOLUME = 32³ = 32768 < 65536`, so the index never truncates.
- **World↔chunk/local coordinate math** (`world::chunk`): uses `div_euclid`/
  `rem_euclid` (and masks), not sign-preserving `/`/`%`; correct at negatives.
- **Determinism of gameplay iteration**: `sim::neighbor_grid` iterates fixed
  `dx/dy/dz` ranges + insertion-ordered `Vec`s (never `HashMap` order, per its own
  doc); `sim::world_authority` deliberately uses `BTreeMap` for deterministic
  iteration; `sim::lightning`'s Dijkstra expands a fixed `NEIGHBORS6` array.
- **LOD boundary math** (`world::lod::desired_lod_for`, `lod_bands_for_rd*`):
  Chebyshev distance with inclusive `<=` bands, clamped to render distance, bands
  strictly increasing — no off-by-one.
- **NaN/divide guards**: `sim::boids` *and* its SoA twin `sim::batch_boids` both
  guard separation with `if d > EPS`; `sim::herd_cohesion`, `scent_gradient`,
  `env_deform` normalise with zero-guards; `sim::fluid`'s surface normal has a
  constant `+1.0` y-term so its length is always ≥ 1 (never divides by zero).
- **`mob_memory`** (decay/remember/recall/eviction): salience stays finite and
  non-negative, capacity is bounded, recall returns the most-salient in radius —
  all covered by existing property tests.
- **Unsigned tick deltas**: `sim::ability_cooldown::cd_remaining` is `f32` and
  explicitly clamps clock-skew (`now < last_cast`); `sim::desync_monitor::annotate_frametime`
  guards `e.tick < start_tick` *and* bounds the index. No underflow.
- **`physics`** is fixed-point (`Fixed`), so the f32 NaN/overflow family does not
  apply to the character/world physics step.
