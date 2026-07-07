# HANDOFF — world surface read (`r330-world`, §4a fix direction a)

**Worker:** `r330-world`  **Module shipped:** `sim::surface_scan` (pure, 10/10 standalone).
**Status of this doc:** *proposal only.* Per SHARED_RULES §2 I did **not** edit any
placement call-site or the bin. The switches below are for the coordinator/operator to
wire and validate on the real FP32 GPU path.

---

## 1. What the new core gives you

```rust
// Contiguous column, column[i] = world y (y_min + i). Brief signature.
sim::surface_scan::top_solid_y(&column, y_min) -> Option<i32>

// Predicate form — pass BlockId straight through, NO trait impl, NO shared-file edit:
sim::surface_scan::top_solid_y_by(&column, y_min, |b: &BlockId| b.is_solid()) -> Option<i32>

// Alloc-free: scans the flat chunk buffer in place (stride x + z*cs + y*cs*cs),
// Y-depth derived from buffer len. This is the form the hot paths should use.
sim::surface_scan::top_solid_y_in_chunk(&chunk.blocks, lx, lz, CHUNK_SIZE, chunk_world_y0,
                                        |b: &BlockId| b.is_solid()) -> Option<i32>
```

"Solid" = `BlockId::is_solid()` (excludes air, water, lava, and thin plant decoration —
flowers/grass/fern/vine/cobweb/mushroom/lichen). That is the right "can a body/plant base
rest here" notion: it lands placement on the dirt under shallow water and ignores a flower
poking up, instead of floating on the water surface or on the plant.

The core reads **voxels that already exist**; it never calls the generator and never
re-predicts. That is the whole point — it is structurally incapable of reproducing the
FP64-prediction divergence.

---

## 2. The mismatch, restated (so the wiring intent is unambiguous)

`world/chunk_manager.rs::surface_y_at` (line **1831**) returns
`self.generator.cpu().surface_y_at(...)` — the **CPU FP64** noise *prediction*. Terrain
voxels are generated **FP32** on the GPU path (`world/gpu_generator.rs`, wrapped by
`world/terrain_generator.rs` alongside the FP64 `cpu_generator.rs`). The two disagree by a
few blocks, so anything that places against the prediction floats / sinks
(`NEXT_SESSION_GUIDE.md §4a`). The fix is to place against the **generated chunk's** top
solid block instead.

> Anchor note: §4a cites `world/constants.rs:23` for `SEA_LEVEL = 64`; in this tree that
> constant actually lives at **`crates/unendlich-engine/src/constants.rs:23`**
> (`pub const SEA_LEVEL: i32 = 64;`). Value matches; path corrected for whoever greps.

---

## 3. Call-sites to switch (engine-side — coordinator-owned to apply)

These read the prediction for **flora placement / mob ground / spawn** and are the
intended switch targets. All are real `file:line` in this tree.

### a) Spawn ground (mob/entity placement)
- `sim/spawn.rs:72` — `gen.surface_y_at(wx, wz, -16, 320)` → entity drop column.
- `sim/spawn.rs:169` — `g.surface_y_at(...).expect("spawned over a surface")`
  **(also a latent panic — SHARED_RULES §1).** Switching to a voxel scan + `Option`
  fallback hardens this too.

**Switch:** resolve the chunk for `(wx,wz)`, then
`top_solid_y_in_chunk(&chunk.blocks, lx, lz, CHUNK_SIZE, chunk_world_y0, |b| b.is_solid())`,
falling back to the old prediction only if the chunk isn't generated yet (see §5).

### b) Spawn pick (`pick_spawn`)
- `world/chunk_manager.rs:1836` (`pick_spawn` → `surface_y_at(...).unwrap_or(y_min)`),
  consumed at e.g. `bin/unendlich_fullgame.rs:8852` (**bin — coordinator/operator**).

**Switch:** give `ChunkManager` a sibling `pick_spawn_voxel` that, when the spawn chunk is
loaded, scans `chunk.blocks`; otherwise keeps the prediction. Leaves `surface_y_at`
intact for callers that legitimately want the predicted height (biome/ocean classification
in `cpu_generator.rs:224`, generation-time slope estimation, headless collages).

### c) Flora placement during generation
- `world/cpu_generator.rs:763` (tree/flora pass; `surface_y_cached` memo @ 756, used by the
  trunk/canopy placement @ ~791/835).

This one is subtle: it runs **inside generation**, where the FP32 voxel column for the
chunk being built may not be finalized yet. Treat as operator's call — if the voxel pass
completes the solid column before the flora pass, switch it; otherwise this is the case
"fix direction (b), close CPU/GPU FP parity" is really for. Flag, don't force.

### d) Probe / diagnostics
- `sim/world_prober.rs:91` — surface probe; switching it makes the prober report the
  *real* surface, which is also what you want for the lavapipe divergence measurement (§6).

### Bin call-sites — **OFF-LIMITS to me, coordinator/operator owns** (listed for completeness)
`bin/unendlich_fullgame.rs` foliage scatter & mob placement (2084–2123, 2359, 2523–2634,
3843, 6818, 8852), `bin/unendlich_world.rs` (461, 1681–1795), `bin/unendlich_demo.rs:58`.
Same switch shape; same loaded-chunk precondition.

---

## 4. Perf note (the brief asked for this: "cache the scan per chunk")

`top_solid_y_in_chunk` is **O(air-thickness)** — it returns on the first solid from the top,
so over open sky it touches only the empty cells above ground, not the whole column. It is
alloc-free (no column copy) because it strides `chunk.blocks` in place.

Reuse the **existing** `sim::surface_cache::SurfaceCache` (closure-supplied scan, cleared
per tick) so a cluster of roamers/foliage probing nearby columns collapses to one scan per
distinct `(x,z)`:

```rust
let y = surface_cache.get_or(wx, wz, ||
    top_solid_y_in_chunk(&chunk.blocks, lx, lz, CHUNK_SIZE, chunk_world_y0,
                         |b| b.is_solid()));
```

For generation-time bulk placement, mirror the `surface_y_cached` HashMap memo already at
`cpu_generator.rs:756` — same pattern, voxel scan as the closure body.

---

## 5. The one precondition: the chunk must be generated/loaded

`top_solid_y*` reads voxels, so the chunk has to exist. Prediction-based `surface_y_at`
worked on *unloaded* columns (it just evaluates noise). So each switched call-site needs:

1. **chunk loaded** → scan voxels (`top_solid_y_in_chunk`). Authoritative.
2. **not loaded** → keep `surface_y_at` prediction as a *temporary* estimate, OR defer the
   placement until the chunk streams in (preferred for spawn — `pick_spawn` already has a
   first-request path via `first_request_pending`).

Initial player spawn is the sharp case: it can fire before the spawn chunk streams in.
Recommend deferring the final spawn snap to the first frame after the spawn chunk is
loaded, rather than trusting the prediction for the player.

Multi-chunk vertical span: scan the stack top chunk → down, first solid wins (each chunk is
32 tall; `top_solid_y_in_chunk` handles one chunk's `chunk_world_y0`).

---

## 6. Validation (operator)

- Real FP32 GPU path: switch (b)+(d) first (smallest blast radius), confirm flora/mobs sit
  on the surface and water no longer fills below `SEA_LEVEL=64`.
- **Measure the bug** (per §4a "NEW" + §2c lavapipe): generate a chunk headless on the FP32
  GPU path, compare `top_solid_y_in_chunk(real voxels)` vs `surface_y_at(prediction)` per
  column; the per-column delta histogram is the divergence this core eliminates. The unit
  test `divergence_harness_follows_voxels_not_the_fp64_prediction` is that comparison in
  miniature (true top 67 vs prediction 70).

---

## 7. Verify commands I ran (this module)

```
rustc --edition 2021 --test surface_scan.rs -o /tmp/t && /tmp/t   # 10 passed / 0 failed
rustc --edition 2021 --crate-type lib surface_scan.rs             # clean, 0 warnings
git apply --check -p1 CHANGES-r330-world.patch                    # CLEAN on this tree
```
Toolchain available in-sandbox was rustc **1.75** (brief specifies 1.96; this module uses
only long-stable APIs — `i32::try_from`, `saturating_add`, `checked_mul`, `enumerate().rev()`
— so the 1.96 result is identical. Flagging the version honestly per the honesty rule).
