# Batch a bending shape as one remesh transaction (r649/r650)

`world::brick_plan` turns a shape's edits into the minimal set of dirty mesh
bricks in a single pass, instead of routing each of ~1340 voxels through the
per-edit dirty path.

## The flow (in the edit-application path)
```text
1. shape generator → Vec<BlockEdit { world_pos, block_id }>   (already produced)
2. apply each edit to its chunk slab via block_edit::apply;
   collect the world positions that ACTUALLY changed (apply returns `changed`)
3. let sets = brick_plan::dirty_bitsets_for_cells(&changed_positions);
4. for (chunk, mask) in sets { chunk.dirty_bricks.0 |= mask; }   // one OR per chunk
5. mesh_brick::select_bricks / sim::mesh_budget drains them across frames
```

Two things this gets right that the naive path doesn't:
- **Hollow-aware.** `dirty_bitsets_for_cells` marks only bricks that contain a
  changed voxel (or its ±1 face-neighbour), so a dome shell / ring / cage leaves
  its interior bricks clean. The bounding-box planner (`dirty_brick_bitsets`)
  would re-bake the interior too. (Test: `hollow_shell_leaves_interior_bricks_clean`.)
- **One pass.** You compute the dirty set once and OR it in, rather than paying
  coalesce + dirty bookkeeping 1340 times.

## Only feed it CHANGED cells
`block_edit::apply` returns `changed: bool`. Skip no-op edits (placing stone where
stone already is) — they shouldn't dirty a brick. Passing only changed cells keeps
the remesh set truly minimal (e.g. re-casting the same dome twice → second cast
dirties nothing).

## Pre-check for coarse-to-fine
`brick_plan::dirty_brick_total(&changed)` (or `brick_count` on the AABB) is a cheap
count. If it exceeds a threshold you can choose to remesh the dirty bricks at a
coarse LOD first and refine within the mesh budget over the next frames, so even a
world-sized shape never stalls a frame.

## Not done here (needs your build)
Steps 2 and 4 touch the real chunk-slab storage and `mesh_brick::DirtyBricks`
(engine state), so they're the integration edit to make on your side. The pure
planning (steps 3, and the brick math) is `brick_plan`, verified in-sandbox.
The remaining octree win — a `fill_region(aabb, block)` that writes/collapses whole
subtrees instead of 1340 per-voxel walks — is a separate change to `octree.rs`.
