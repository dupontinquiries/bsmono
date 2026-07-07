# LOD · SHADOW · ENTITY integration kit

The plan for the GPU-side work you asked to start: close the LOD chunk gap, cut the
main + shadow raster cost at distance, lay down the shader-pairing structure, and wire
mobs / entity bodies / items into the passes. Sandbox can't run Vulkan, so this is a
sequenced integration spec against your real `world_renderer` + flags; the pure cores it
leans on are already in the base and harness-green.

## Where the time actually goes (RD=24, pool 2048, the 433 s run)
- **main p50 = 101 ms**, **shadow p50 = 73 ms**, **total p50 = 192 ms**, all ~99% over an
  8.3 ms budget. **cull** p50 2.5 ms but **p90 24 ms** (spiking). CPU **record p50 = 27 ms**
  (recording the draws). Memory is fine (2048 MB, no OOM fallbacks).
- Verdict: you are **raster + draw-recording bound at distance**, not memory bound. So the
  levers, in priority order: (B) cut far main-pass geometry (SurfaceRun far-LOD), (C) cut
  shadow cost (bucketing + LOD-paired casters), (D/E) cut draw count via the pairing table +
  instanced entities. (A) closes the visible gap first because it's cheap and you can see it.

---

## A. The chunk gap + LOD fidelity (cheap, visible — do first)
The dark seam across the cliff in your screenshot is a **LOD-transition crack**: neighbour
chunks at different LOD factors (1,2,4,8) don't share edge vertices, so a gap opens at the
boundary.
- **Fix = LOD skirts.** At each chunk boundary, emit a thin vertical apron dropping to the
  coarser neighbour's floor level. It occludes the crack without T-junction stitching — the
  standard cheap fix. Then apply `mesh_smooth` (R5) on the seam rings for normal + colour
  blending so lighting doesn't betray it. (Stitching exact T-junctions is the expensive
  alternative; skirts first.)
- **Fidelity:** `LOD_FACTORS=1,2,4,8` downsamples far chunks 8×, which is most of the washed-
  out look on the far wall. Three knobs: push `LOD_BANDS=12,24,48`→`16,32,64` (more chunks
  stay fine), soften the ramp `1,2,4,8`→`1,2,3,4` (more detail, more verts), raise
  `LOD_FIDELITY` 0.8→0.9. **But raising mesh fidelity costs the raster you're already short
  on** — which is the case for the runs path below: Mid-as-runs keeps full per-voxel surface
  detail at low memory and fewer draws, so it buys fidelity *and* perf where downsampled mesh
  trades them off.

## B. SurfaceRun far-LOD — the main-pass lever (extends SURFACERUN_KIT Steps 1–3)
`upload_repr::classify` routes Near→octree, Mid→runs, Far/VFar→procedural-from-seed; the
projection showed the octree pool stays ~77 MB at every radius. The raster win on top of the
memory win: runs render fewer, cheaper draws than downsampled mesh, and procedural far skips
upload entirely.
1. **Telemetry (go/no-go).** Drop `handoff/surfacerun/surfacerun_telemetry.rs`; log
   `project_residency` on the live loaded set at RD=24. Confirms the routing before any GPU
   change.
2. **Mid → runs.** Branch the upload/draw path on `classify`; Mid (12–24 band) meshes via
   `chunk_mesh_runs.comp` (already in the tree) into a runs pipeline. At RD=24 the 12–24 band
   is the bulk of the far wall, so this is the biggest single cut to **main = 101 ms**.
3. **Far → procedural.** Far/VFar regen on the GPU from seed (no upload, no octree, a cheap
   proc pipeline). Re-baseline `perf_check.py` after (the stored baseline predates the fog
   march).

## C. Shadow pass — 73 ms, paired to the LOD bands
Shadow is 98.8% over budget and almost entirely far-caster cost.
- **Turn on `SHADOW_BUCKETS`** (currently unset). Bucket casters by distance into cascades;
  far casters render at coarse res / lower update frequency. This is the cheap flag win.
- Push `SHADOW_CHUNK_STRIDE` 2→3–4 for the far buckets only (near stays dense).
- **LOD-paired shadow geometry:** Near octree casts full depth; Mid runs cast a coarse
  runs-proxy; Far procedural casts from a **low-res heightfield**, not full geometry. A small
  far caster doesn't need per-voxel shadow.
- `SHADOW_CACHE=1` is on — make sure static far cascades actually reuse the cache and don't
  re-render when nothing moved.

## D. The shader-pairing table (LOD × pass → pipeline) — the structure to "prepare"
Lay this down as the routing skeleton; it's what the **typed pipeline builder** (backlog ★)
should eventually make type-safe so a descriptor/binding mismatch won't compile.

| Band / kind            | Main pass            | Shadow pass            | Geometry source            |
|------------------------|----------------------|------------------------|----------------------------|
| Near (octree)          | `chunk.frag` (full)  | depth full             | octree residency pool      |
| Mid (runs)             | `runs.frag`          | depth runs-proxy       | `chunk_mesh_runs.comp`     |
| Far (procedural)       | `proc.frag`          | depth heightfield      | seed (GPU regen)           |
| Entities (mob/body/item)| `entity.frag` (inst)| depth instanced        | entity instance buffer     |
| Decals (footprints)    | `decal.frag`         | —                      | `footprint_decal` params   |

All passes share the **cull** output (one cull, many consumers — like the nav-field: compute
once, read many). Keep the UBO slot registry (R3/R9/R16) authoritative as you add `runs`/
`proc`/`entity` pipelines; regen `.spv` on any shader edit (R2).

## E. Wire mobs / entity bodies / items into the shader (the "still to do")
The flat splayed dragons in your screenshot are rendering outside a proper entity path.
- **Entity instance buffer.** Each frame, gather visible entities → one instance buffer:
  transform from **`large_body_grounding`** (contact-plane fit — this fixes the lying-flat /
  ground-clipping), detail from **`mob_lod`** (apparent-pixel LOD → which parts to keep), body
  mesh from `creature_to_vertices` / **`mob_instances`**. One instanced draw per archetype.
- **`entity.frag`** vertex-pulls the body mesh + per-instance transform; shares main-pass
  depth + lighting. The dragon body shape comes from genetics → `creature/body` params (the
  shapes you're choosing in `dragon_directions.html`).
- **Items**: same instance path, simpler meshes. **Footprints**: `footprint_decal` (R4) → a
  decal pass projected on terrain (impact-shape + weathering already computed).
- **LOD-pair entities into shadows too** (instanced depth), bucketed like terrain.

## F. Block break / place (you flagged it broken — fast + accurate)
- **Accurate pick = DDA voxel raycast** (Amanatides–Woo) from the camera through the grid →
  exact hit voxel + **hit face** (place on the adjacent cell, break the hit cell). This is a
  pure, testable core — `voxel_raycast` — I've queued it for the next sim round.
- **Fast = local synchronous remesh.** On edit, remesh only the touched chunk (and its
  boundary neighbour if the edit is on a chunk edge) *immediately*, not through the async
  upload queue, so the change shows that frame.
- "Broken" is most likely (1) face selection picking the wrong adjacent cell, (2) the edited
  chunk not being re-meshed (stale mesh), or (3) the winding/nonsolid-face bug making the new
  faces look missing. Sequence: **fix winding first**, then `voxel_raycast`, then local remesh.

---

## Suggested order
1. `bash fix_lerp3.sh` (so it builds) → **LOD skirts** (close the visible gap) + `mesh_smooth` on seams.
2. `SHADOW_BUCKETS` on + far-bucket stride (the cheap 73 ms shadow win).
3. SurfaceRun **Step 1 telemetry → Step 2 Mid→runs** (the 101 ms main-pass win).
4. **Entity instance buffer + `entity.frag`** + grounding transform (mobs/bodies/items into the pass).
5. Block edit: **winding fix → `voxel_raycast` → local remesh**.
6. SurfaceRun **Step 3 Far→procedural**; then raise LOD fidelity once raster has headroom.

Cull p90 (24 ms) and CPU record (27 ms) should both fall out of B+D (fewer/cheaper far draws
to cull and record). Re-measure after Step 2 before deciding whether cull needs its own pass.
