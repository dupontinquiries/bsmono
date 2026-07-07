# r62-05 · greedy mesher — FINDINGS (clean negative + diagnosis)

**WID:** r62-05-greedy-mesh-improve · **Files changed: NONE** (no patch — see why).

## TL;DR
The proposed candidate ("full 2D quad-merge on each of the 6 face planes, merge
across both axes, drop degenerate quads") is **already implemented** in
`mesh_chunk_greedy_undecorated` (mesher.rs:1911). It is already optimal for
rectangle-greedy. Measuring shows the real surface-chunk cost is **intentional
decoration geometry**, not un-merged quads — and that cannot be merged without
changing the visible surface (which the brief forbids). So: no code change.

## Evidence 1 — the merge is already full-2D
`mesh_chunk_greedy_undecorated` builds a per-slice 2D `mask[N*N]` and, per cell,
extends width along `u` **and** height along `v` (the `'outer` loop), emits one
quad per maximal rectangle, then clears it. That is merge-across-both-axes, not
1D runs. It never emits zero-area quads, and per-plane rectangles are already
maximal (nothing left to coalesce; cross-slice merge is impossible — different
depths).

## Evidence 2 — mesh-bench BEFORE (and AFTER == BEFORE, no change)
`cargo run --release --features render --bin mesh-bench`:

```
chunk_y    naive_v    greedy_v   ratio   %saved
  -2        381264        6032    63.2x   +98.4%
  -1        388488        9912    39.2x   +97.4%
   0        460880     1185386     0.4x  -157.2%
   1        399368     1880684     0.2x  -370.9%
   2             0       65536      —       —
   4             0       53276      —       —
 TOTAL     1630000     3200826     0.5x   -96.4%
 time: naive 69437 µs, greedy 400332 µs (0.17x)
```

The greedy MERGE works exactly as intended on solid/underground chunks
(y=-2,-1): **63x / 39x fewer vertices** than naive. There are no un-merged
coplanar same-material rectangles left to win there.

## Evidence 3 — why surface chunks look like a "regression"
`mesh_chunk_naive` emits **plain cube faces only** — no decoration. The
production `mesh_chunk_greedy` path additionally emits, on the same chunk:
- jittered tops (`emit_jittered_top`) and weather-subdivided tops
  (`emit_subdivided_top`),
- soft foliage side faces (`emit_soft_side_face`),
- sub-voxel grass tufts & plants (`decorate_chunk` → `emit_subvoxel_grid` /
  stamped templates),
- fluid surfaces / clouds.

These are **per-cell / sub-voxel** by nature (each cell's geometry is unique), so
they bypass face merging *by design*. On surface chunks they dominate, which is
the entire 1.19M / 1.88M vertex figure at y=0 / y=1 — it is decoration, not
un-merged solid faces. naive simply doesn't draw any of it, so the comparison on
surface chunks is plain-cubes vs fully-decorated, not "bad greedy".

## Correctness gates
- **winding-check: clean** (exit 0, no flipped/hole/mismatch; merged unit case =
  24 verts / 12 tris, all faces outward).
- **No source files changed** vs the pristine snapshot → worldgen determinism
  (`world-bench` all-match), octree round-trip (`rt_fail=0`), and `harness/run.sh`
  are unaffected by definition; the meshing-relevant gate (winding) is clean.
- `mesh-bench` / `winding-check` built clean under
  `cargo build --release --features render` (Rust 1.96; Cargo.lock v4 builds
  directly — no lock-dance needed on the 1.96 toolchain).

## Why no patch
"More 2D merging" cannot reduce the surface count — the merge is already
maximal-rectangle and already wins where merging is possible (solid chunks). The
remaining surface geometry is intentional decoration that can only shrink by
changing the visible surface, which this candidate explicitly disallows. A
rewrite would risk winding/holes for zero quad-count benefit. Honest negative.

## The real lever (separate, design-gated candidate — NOT this one)
The surface meshing cost center is **decoration density**, not quad merging.
Genuine wins live there and need their own candidate + sign-off because they
alter the rendered surface or its LOD policy:
- distance/LOD-gate jitter & sub-voxel tufts (full detail near, plain faces far);
- instance the tuft/plant sub-grids instead of baking them into the chunk mesh
  (huge vertex-buffer saving — note the bench's "+35.95 MB" is mostly tufts);
- budget `emit_subdivided_top` weathering by LOD.
Each of those is measurable on `mesh-bench` / `unendlich-spin` but is out of
scope for a "same-surface, merge-more" change.
