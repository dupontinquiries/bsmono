# Perf: far-region cost — the CORRECT framing (r249, supersedes r248's cave-skip)

> Implementation scoping: `STEP1_SCOPING_regiongrid.md` — the shared coarse field is an EXTENSION of the existing `RegionGrid` pattern (worldgen already caches sea_level/humidity/etc.), not new infra; the gap is that density/is_cave aren't RegionGrid-backed. The minimal perf-fix subset is "add a RegionGrid for the far-mesh solidity verdict, read it in mesh_chunk_far_virtual."

> See also `dynamic_world_architecture.md` — the compute-once-share principle here is the spine of that architecture (§3 tier D, §5 build step 1).

> Corrects a wrong turn. r248 added `UNENDLICH_FAR_SKIP_CAVES` to skip cave
> evaluation in far meshing "because caves are invisible at distance." That was
> WRONG and has been REVERTED. Operator's correction: far regions must still
> compute caves/density/terrain because those feed regional simulation —
> **water flow, wind, storms, clouds, population dynamics** — that PROPAGATES
> region-to-region. Skipping the computation would desync the sim, not just dim
> a distant visual. The cost is necessary, not waste.

## The distinction I missed
- **Rendering far** = `mesh_chunk_far_virtual` builds a downsampled visual mesh.
- **Simulating far** = regional fields (moisture, storm intensity, water flow,
  karst dissolution, population) evolve and PROPAGATE across regions whether or
  not you're looking. Terrain structure (caves, density) is an INPUT to that.

These are different consumers of the same underlying terrain. The right
optimization is therefore NOT "compute less" — it's **"compute the terrain
structure once at the right resolution and SHARE it"** between the mesh and the
regional sim, instead of either recomputing it per-consumer or dropping it.

## What the code actually shows (so we optimize the real thing)
- The regional sims that propagate are **pure aggregate-field functions**:
  `world/storm_intensity.rs` (intensity = humidity × instability),
  `world/karst_sink.rs` (dissolve = f(limestone, water_flow, time)), the
  moisture/water fields. They take COARSE inputs, not per-voxel generator
  samples — so they are already cheap and correctly designed. Good.
- The expensive `generator.density()` / `generator.is_cave()` calls (is_cave =
  three 3D noise layers) live in `world_model.rs`, `cpu_generator.rs`, and the
  far-mesher. The far-mesher samples them per grid cell to build the visual
  mesh — that's the measured ~11–13ms/far-chunk.
- So today the far-mesh terrain sampling and the regional sim are SEPARATE; the
  mesh sampling is real render cost, and the regional fields are cheap.

## Correct optimization directions (none break the propagating sim)
Ranked, all needing a run to validate:

1. **Share a coarse regional terrain field.** If both the far mesh AND any
   region-propagating system need "is this coarse cell solid / cave / surface
   height," compute that ONCE per region into a cached coarse field (the
   batch-as-field pattern we already use for grass/fire) and have both read it.
   The generator's density/cave noise is evaluated once per coarse cell per
   region-refresh, not per-mesh-rebuild. This is the structural win and it makes
   the propagating sim and the visual CONSISTENT by construction (same field).
2. **Cache the far-mesh result; rebuild only on region change.** Far terrain is
   static unless edited or the region's coarse state changes. The 11ms is paid
   every time a far chunk (re)submits; if far meshes are cached and only rebuilt
   when their coarse region field changes, the per-frame burst disappears
   without dropping any computation the sim needs.
3. **Throttle far-mesh submission bursts.** `mesher: far=11ms×77` means 77 far
   chunks meshed in one window; the completed-install lands on the main thread.
   A far-tier submission budget (analogue of the upload budget) spreads the
   burst so it can't stack into a frame spike — again, computes the SAME total,
   just paced.
4. **Decouple sim-tick from mesh-rebuild.** If the regional sim needs terrain
   every coarse tick but the MESH only needs rebuilding when the visible result
   changes, separate the two: the sim reads the shared field on its cadence; the
   mesh rebuilds only on visible change. Neither skips work the other needs.

## The principle
Far regions are a LOD problem for BOTH render and sim, but "LOD" means *right
resolution + shared computation + right cadence*, not "skip it." The propagating
systems (water/wind/storms/clouds/population) are first-class and must keep
their inputs. Optimize by removing DUPLICATION and REDUNDANT REBUILDS, never by
removing the terrain computation the sim depends on.

## Status
- r248's cave-skip lever is REVERTED (lib compiles clean). No behavior change
  remains from it.
- The directions above are design, not yet implemented — #1 (shared coarse
  regional terrain field) is the real structural fix and the natural next step,
  but it's a meaningful change to how far terrain is computed and must be
  designed against the actual regional-sim consumers + validated on a build.
- The `roamer_tick` zone (r248) stays — it's measurement for the SEPARATE
  steady-16ms question and doesn't touch any of this.
