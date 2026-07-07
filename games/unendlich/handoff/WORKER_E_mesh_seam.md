# Worker thread T05 (mesh) — close §5b light-leak seams in the mesher

Stream: **mesh**. Doc: `handoff/SESSION_HANDOFF.md §5b` — gaps between the
flat / bumpy / LOD mesh paths leak light and fog at chunk boundaries; fix in the
mesher (edge skirts or shared-edge vertex snap).

## Task
Read `crates/unendlich-engine/src/render/mesher.rs` (the flat-top, bumpy-top, and
LOD/downsample emit paths) and `SESSION_HANDOFF.md §5b`. Close the boundary gaps
with ONE approach (pick the lower-risk one for the existing code):
- **Edge skirts:** at a chunk's outer boundary, extend the boundary quads a small
  amount downward so a neighbour at a different LOD/mesh-path can't show a gap; or
- **Shared-edge vertex snap:** snap the finer mesh's boundary-edge vertices to the
  coarser neighbour's edge line so the seam closes.
Keep the CPU mesher **deterministic** (no float-order nondeterminism). Don't
change vertex format or break existing tests.

## Validate / return
- A CPU unit test: for two adjacent chunks meshed on different paths, assert the
  shared boundary has no vertical gap (boundary vertices align / skirt covers).
- Optional: a `vk-harness` `mesher_render` / `mesh_parity` pixel check on
  **lavapipe** showing no light bleed at the seam.
- `bash RETURN_KIT.sh start` (before) / `finish` (after, HARNESS GREEN) →
  `CHANGES-T05-R26.patch`. Off-limits: `froxel_fill.comp`, `chunk.frag`.
