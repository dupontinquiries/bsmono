# GLOSSARY / QUICK REFERENCE (unendlich coordinator)

The token-cheap front door. For anything not here, use `tools/xref.sh` (don't broad-grep):
`xref.sh sym <Name>` · `xref.sh env <FLAG>` · `xref.sh doc <kw>` · `xref.sh find <kw>` ·
`xref.sh file <frag>`. Full env list: `xref.sh env -l` (70 flags).

## Build / test cheat sheet (sandbox)
Cargo.lock is v4, cargo is 1.75 → always:
```
cp Cargo.lock /tmp/lock.v4; rm -f Cargo.lock; cargo <cmd>; cp /tmp/lock.v4 Cargo.lock
```
- Fast compile gate (render-side): `cargo check -p unendlich-engine --features headless`  (~30s)
- World/sim only:                  `cargo check -p unendlich-engine --features gpu`
- Tests (slow ~4min, link may exceed call limit): `cargo test ... --features headless --no-run`
  then run `target/debug/deps/unendlich_engine-*` directly with:
  `VK_ICD_FILENAMES=/usr/share/vulkan/icd.d/lvp_icd.json <bin> <filter> --test-threads=1`
- Green gate: `bash harness/run.sh` (from repo root) → `HARNESS GREEN` + shaders 25/25
- lavapipe install (if missing): `apt-get install -y mesa-vulkan-drivers`

## Feature gates (what compiles where)
`render → render-core → gpu`; `window = [glfw, vk-sys]`; `headless = [render-core]`.
- `gpu`: world/sim/dragon (chunk_manager, mesh_buffer_pool, creature_*). NOT render module.
- `headless`: + render module via glfw-free Vulkan (`new_headless`, offscreen). Validatable here.
- `window`: pulls glfw (C/cmake/X11) → **fails in sandbox**. **All `bin`s need `window` → M2-compile-only.**
- ash (Vulkan, pure Rust) builds; glfw-sys does not.

## Perf / stall env flags

**Doc convention (how flags are organized in the overview docs):** lead with the *recommended*
value, not the shipped default. The engine is **beta and still moving**, so older code paths are
NOT "legacy" or "deprecated" — they're **fallbacks / earlier paths**, kept on purpose for A/B and
safety, and they live in their own section below. Inline code comments keep the full history of
*why* a path exists and what it superseded; these overview docs only point at the current best.
Where a flag's shipped default differs from the recommended value, the default is shown in
*(parens)*.

**R27 default change:** the proven perf cluster is now **ON by default** — you no longer need the
flag barrage to get the fast path. Each is a **kill-switch**: pass `=0` to fall back. So a bare
`cargo run …` already runs prefetch + octree GPU mesher + device-local + the scheduler/upload
toggles. `UNENDLICH_PROFILE=rd24` is now mostly about pinning the *numeric* knobs (batch widths,
LOD bands, pool sizes) rather than flipping switches.

On by default now (R27): PREFETCH, GPU_MESHER=2, DEVICE_LOCAL_MESH, SHADOW_CACHE, REMESH_GATE,
SKY_CULL(margin 1), ADAPTIVE_UPLOADS, SUBMIT_SKIP, SLACK_SCHEDULER, BIASED_SHELL_WALK,
UPLOAD_PRIORITY. Still default-OFF / unchanged: SHADOW_CHUNK_STRIDE (=1, changes shadow output),
GPU_MESHER_MID, GPU_GEN_BATCH/MESH_BATCH (numeric, set by profile).

| flag | value | what it does |
|---|---|---|
| UNENDLICH_PREFETCH (+_COUNT/_LOOKAHEAD) | **on** / 16 / 16 | predictive chunk LOAD + mesh-ahead — removes the on-demand near-mesh stall |
| UNENDLICH_GPU_MESHER / _MID | **2** / 2 *(MID still opt-in)* | octree GPU mesher for near+mid (=0 CPU; construction failure degrades to CPU) |
| UNENDLICH_DEVICE_LOCAL_MESH | **on** | device-local mesh buffers |
| UNENDLICH_GPU_GEN_BATCH / _MESH_BATCH | 256 / 256 *(default 16 / 64; profile sets 256)* | gen + mesh batch width (world-fill rate + parallel mesh) |
| UNENDLICH_SHADOW_CHUNK_STRIDE / _CACHE | 2 / **on** | cheap-stride shadows *(stride still opt-in: changes output)* + cache reuse |
| _REMESH_GATE, _SKY_CULL, _ADAPTIVE_UPLOADS, _SUBMIT_SKIP, _SLACK_SCHEDULER, _BIASED_SHELL_WALK, _UPLOAD_PRIORITY | **on** | the proven perf toggles, now default-on (=0 disables each) |

**Per-run knobs (vary these):** UNENDLICH_RENDER_DISTANCE, _LOD_BANDS (match RD with a tight near
band, e.g. `4,10,24`), _LOD_FIDELITY, _OCTREE_POOL_MB, _MAX_UPLOADS_PER_FRAME / _UPLOAD_BUDGET_MS,
_APPLY_BUDGET / _EVICT_BUDGET, _SPAWN_ARCHE, eye-candy (_POSTFX/_BLOOM/_SEASONS/_WEATHER/_GPU_DECORATE).

**Diagnostics (off by default; turn on to measure):** UNENDLICH_CULL_DIAG, _SHADOW_BUCKETS,
_THREAD_PROBE(_INTERVAL_MS), _OCTREE_VERIFY, _WARN_SNIPE.

**Fallback / earlier paths (still valid — kept for A/B or if the recommended path misbehaves):**
reach these by passing the kill-switch `=0` (the fast path is now the default). UNENDLICH_INDIRECT=0
(per-chunk draw loop instead of GPU-driven indirect), UNENDLICH_GPU_MESHER=0 (CPU mesher) or =1
(sparse GPU mesher), UNENDLICH_PREFETCH=0 (on-demand meshing at shell entry), UNENDLICH_DEVICE_LOCAL_MESH=0,
and the other `=0` kill-switches. These select older/simpler paths retained deliberately, not on their way out.

(full list: `xref.sh env -l`)

## Key symbols → file (the R27 stall-fix surface)
- `mesh_buffer_pool.rs`: `write_at`, `write_at_deferrable`, `flush_pending_copies`,
  `count_within_budget` (pure, unit-tested)
- `world/chunk_manager.rs`: `request_prefetch`, `predicted_mesh_ahead`, `last_vel_chunks`,
  `update`, `get_chunk`, `loaded_coords`, `install_chunk`
- `world/prefetch_predict.rs`: `prefetch(player[i32;3], vel[f32;3] chunk-units, lookahead, n)`
- `world_renderer.rs`: `upload_prebuilt_mesh`, `upload_prebuilt_mesh_at_lod[_deferrable]`,
  `evict_stale_lods` (T10), `chunk_visible`, `stale_lod_key` (pred), `mesh_or_empty_version_for_lod`
- `world/lod.rs`: `LodLevel{Near,Mid,Far,VeryFar}`, `LodLevel::iter`, `MeshSlotKey::at`,
  `desired_lod_for(coord, player_chunk, rd)`
- bin `unendlich_world.rs`: frame loop; mesh-candidate shell walk (~2740) → near/mid/far/vfar
  cands; mesh-shell-extension fold-in (after shell walk); result-drain + upload (~3140)

## Invariants (do not violate)
- Never reorder `CameraUbo` (canonical in `chunk.vert`) — append or push-constant.
- After a shader edit: regen committed `assets/shaders/<n>.spv` + `.hash` (glslangValidator -V +
  spirv-val, or delete `.hash` → build.rs regens). The ".spv recompile trap".
- Exclude `target/` from every zip.
- OFF-LIMITS to workers (coordinator owns): `src/shaders/froxel_fill.comp`, `src/shaders/chunk.frag`.
- Don't cache once-per-frame runtime-tunable env flags (defeats tuning + tests) — see held T5.

## Map of the map (where to look)
- This file + `COORDINATOR_BOOTSTRAP.md` → start here.
- `docs/SESSION_R27_CHANGES.md` → what changed last round + open items.
- `handoff/COORDINATOR_TODO.md` → live TODO + running stall diagnosis + merge log.
- `docs/PREDICTIVE_UPLOAD_QUEUE.md` → the stall fix design.
- `docs/GLFW_DECOUPLING.md` → headless build.
- `handoff/MERGE_PLAYBOOK.md`, `WORKER_PIPELINE.md`, `RETURN_KIT.sh` → dispatch/merge mechanics.
- `tools/THREAD_TOOL.md` → the extractor/merger.
- Task sources to portion from: `docs/{PERF-LEVERS,FOLLOWUPS,TODO-BACKLOG,SHADER_ROADMAP}.md`.
