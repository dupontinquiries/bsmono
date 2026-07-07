# unendlich — Session Handoff (the "AAA shaders" thread)

Date: 2026-06-03. Written **to the next Claude** who picks up this tree.

---

## 0. How to work in this repo (read this first)

- **You cannot compile or run.** You write Rust/GLSL **blind**; the user builds,
  runs, and A/Bs on **Linux / Asahi (Apple Silicon, Honeykrisp Vulkan)**. FP32,
  Vulkan ~1.2. macOS/MoltenVK is **de-prioritized** — optimize for the Asahi path.
- **This tree is the ground truth.** It is divergent and *ahead* of any canonical
  repo: creatures, genetics, foraging, seasons, textures, GPU-decorate, the user's
  own SSR, and metal→vk WIP all live here. Don't assume an upstream layout — read
  files from this repo (everything is under the `unendlich/` prefix).
- **Delivery workflow the user prefers:** they refresh their *entire workspace*
  from a FULL zip (root = `unendlich/`). Always provide the FULL zip **plus** a
  small overlay zip (changed files only) for when they have uncommitted local work
  the full zip would clobber.
- **Validate before you ship.** You have no compiler, so the static checkers in
  `handoff/` are your safety net. Run `check_delimiters.py` on every edited
  `.rs`/`.comp`, and the shader + winding checks when relevant.

---

## 1. THE biggest gotcha: the `.spv` recompile trap

Editing a shader **source** does nothing if its compiled `.spv` doesn't
regenerate. This silently ate **two rounds** of a *correct* winding fix that
"did nothing" — the running pipeline kept the stale `.spv`.

- GPU shaders compile to `assets/shaders/<name>.spv` via `build.rs` (the `SHADERS`
  list + content-hash sidecars `.spv.hash`).
- `build.rs` recompiles a listed shader only if glslang/glslc is present and the
  hash is stale; `UNENDLICH_STRICT_SHADERS=1` panics on a stale *listed* shader.
  A cached binary run without `cargo build`, or an asset-path override, can still
  serve a stale `.spv`.
- **Protocol after editing any shader:** force a clean rebuild —
  `handoff/force_recompile_shader.sh <name>` (deletes `.spv` + `.spv.hash`), then
  rebuild. `handoff/check_shaders.py` lists `SHADERS` ↔ `.spv` and flags staleness.

---

## 2. The missing-tops bug — DIAGNOSED + FIXED (awaiting user confirm)

**Symptom:** flat ground tops missing on near chunks; `UNENDLICH_NO_CULL=1` made
them reappear (so geometry was present, just back-faced). Grass/dirt (weatherable)
tops rendered fine.

**Root cause:** in the GPU octree mesher (`src/shaders/chunk_mesh_octree.comp`),
the **flat `+Y` top** `emit_quad` was wound **opposite** to the side faces and the
**bumpy** (weatherable) tops. Under the default pipeline (`front_face=CW`,
`cull=BACK`) the flat tops were back-faces → culled → every **non-weatherable**
block (stone, placed blocks) had no top. Weatherable tops used the bumpy path
(correct winding) and showed.

**How it was proven (don't redo blind winding math — use the tool):** the debug
panel has a live **winding sweep** that builds all six `front_face × cull` chunk
pipelines and binds the selected one (no re-mesh, no recreation). The user
reported: *only variants 2 & 5 (cull=NONE) are complete; the culling variants only
trade block types between visible/invisible.* That means **two opposite winding
groups** → no single front-face fixes both → mesher inconsistency, not a global
flip.

**Fix applied:** reversed the flat `+Y` corner order `q0,q1,q2,q3 → q0,q3,q2,q1`
(AO follows: `ao.x,ao.w,ao.z,ao.y`) so it matches the sides + bumpy. Then deleted
the stale `.spv` to force regen.

**Verify:** leave the winding slider at **0** (CW+BACK, normal default), rebuild;
flat/stone tops should now render — variant 0 becomes complete like 2 & 5 but with
culling on. If they're STILL missing → the `.spv` isn't regenerating (see §1):
check the build log for `chunk_mesh_octree` compiling and that STRICT_SHADERS
didn't panic.

**Invariant:** every face emit in the octree mesher must wind consistently
(CW+BACK = front). New face/emit paths must match; verify with the panel sweep.
`handoff/check_octree_winding.py` guards the flat `+Y` order against regression.

---

## 3. New UI — the debug / shaderpack panel (Esc)

Files: `src/debug_settings.rs` (global lock-free `DEBUG`), panel UI inside the
`run_ui` closure in `render/world_renderer.rs`, ESC wiring in
`bin/unendlich_world.rs`.

- `DEBUG` is a process-global `static` of atomics. Read it anywhere with
  `crate::debug_settings::DEBUG.<field>.load(Relaxed)` — no Arc, no plumbing.
- **Esc** toggles `renderer.settings_open` (frees the cursor when opening). The
  panel's **Quit** button sets `DEBUG.request_quit`; the bin polls it.
- Panel contents:
  - **Look picker** — 12 atmosphere presets, fully wired via `apply_look(&mut atmos)`
    each frame (no shader recompile). Retunes sun/sky/ambient/fog. "Light shafts"
    = thick fog + strong sun so the froxel volume in-scatters into shafts.
  - **Toggles** — `disable_fog` is live (via `apply_look`); the rest are labeled
    **(hook)**: they store state but need a one-line read at their behavior site
    to do anything (see §5c).
  - **Winding sweep slider (0..5)** — the §2 diagnostic. Leave at 0 for play.
- **To add a toggle:** add an `AtomicBool` to `DebugSettings`, init it in `new()`,
  add a `tog!(field,"label")` line in the panel, and read it at the behavior site.

---

## 4. Input model — frame-pacing decoupling (don't regress)

GLFW. Events are drained **twice per frame**: top-of-loop (where consumers act) and
a late pre-draw drain (freshest camera).

- **Bug fixed:** discrete press-edges (esc/jump/flight/inventory) and UI clicks that
  landed in the *late* drain were latched then wiped by `end_frame` before any
  consumer ran → "eaten keys." And egui's button level (`ui_mouse_down`) was set
  only in the top drain, so a release in the late drain left egui stuck "pressed"
  → clicks turned into window drags.
- **Fix:** `InputCache::handle_event_late` stashes late press-edges in `late_*`;
  `end_frame` *carries* them into the next frame's just-pressed set. The late drain
  now calls `handle_event_late`. Esc uses `is_key_just_pressed` (carry-aware). The
  egui pointer level is sourced from `InputCache::is_mouse_button_pressed` (updated
  by *both* drains), plus collapsed-click pairs for low-fps frames.
- **Invariant:** route ALL discrete actions through `is_key_just_pressed` /
  `bindings.is_action_just_pressed` (carry-aware). Never gate an action on a local
  flag set only in the top-drain match. Unit test in `input.rs::tests`.

---

## 5. Deferred / open items

### 5a. Mobs in the fog pass — **post-merge (agreed)**
Entities/mobs draw via `entity_pipeline` and are **not** sampled into the froxel
fog (only `chunk.frag` samples binding 2, the 3D froxel volume), so they pop
against fogged terrain. Defer: the froxel plumbing is touched by branch work and
doing it now risks conflicts. Later: give `entity.frag` the froxel sampler + camera
UBO (mirror `chunk.frag` bindings 0/2), reconstruct view depth, sample
transmittance/in-scatter, composite identically. ~1 binding + ~15 lines of frag.

**IMPLEMENTED 2026-06-04 (shader-only — no Rust/descriptor change needed).**
The entity pipeline already reuses the chunk pipeline's set-0 layout (froxel is
binding 2) and binds the same per-frame descriptor set the chunk pass uses (the
renderer writes the froxel image into binding 2 of that set). So `entity.frag`
only had to *declare and sample* it: it now reads the camera UBO (binding 0, for
`camera_pos` + `viewport_rect`) and the froxel `sampler3D` (binding 2), and
composites identically to `chunk.frag` — log-distributed W from `dist`, single
trilinear sample, `color = color*trans + scatter` — with the same sentinel probe
falling back to *un-fogged* (not the per-fragment march) when the volume is
unfilled. Verified: `glslangValidator` compile + `spirv-val` on the regenerated
`entity.frag.spv`, delimiter check, and `cargo build -p unendlich-engine` clean.
Open follow-up (optional): tonemap/contrast/vignette parity with `chunk.frag` so
close-up entities match the terrain's color grade exactly, not just its fog.

### 5b. Disconnected meshes between block types → light-leak gaps
Seams between mesh paths (flat vs bumpy tops, soft-side sag, LOD/material edges)
don't share edge vertices → thin gaps → light/fog leaks through. Options to blend
for now: **(A)** edge "skirts" — extend face edges down by an epsilon so neighbors
overlap; **(B)** snap shared-edge vertices to a common world-Y (the bumpy path
already does this via a pure function of world coords — extend to flat↔bumpy
borders); **(C)** a small normal/AO blend across the seam to soften it visually.
Recommend after the winding fix lands and the branch merges.

### 5c. (hook) toggles to wire
- `disable_ssr` / `disable_bloom` / `disable_shadows` → gate where each effect's
  intensity is set.
- `freeze_lod` / `freeze_time` / `pause_sim` → gate the LOD update / time-of-day
  advance / sim tick in the bin loop.
- `mesher_mid_gpu` → OR into the `UNENDLICH_GPU_MESHER_MID` read in
  `render/meshing_pool.rs` (~line 547).
- `neutral_tint` → OR into the `UNENDLICH_NEUTRAL_TINT` read in the bin.
- `show_chunk_borders` → needs a debug line/AABB draw (no hook yet).

### 5d. `chunk.frag` debug uniform (`shader_mode`) — not wired
`DEBUG.shader_mode` + `ShaderMode` exist but do nothing. To make front/back-face
viz etc. work, plumb an int into `chunk.frag` (push-constant or a spare UBO field)
and branch (`gl_FrontFacing ? green : red`, normals→rgb, …). **Do not reorder the
CameraUbo fields** — append or use a push constant.

---

## 6. Good merging points (order + conflict risk)

- **Independent / low conflict (merge anytime):**
  - `input.rs` (only adds methods + carry; self-contained).
  - `debug_settings.rs` (new file) + the one `pub mod debug_settings;` in `lib.rs`.
  - The **winding fix** (`chunk_mesh_octree.comp` flat `+Y` + deleted `.spv`) — this
    is the **priority correctness merge**, independent of everything else.
- **Medium (edit large, branch-active files — merge after the above):**
  - `render/chunk_pipeline.rs` (6-variant pipeline; isolated to `ChunkPipeline`).
  - `render/world_renderer.rs` (`settings_open` field/init, the run_ui panel block,
    pointer-level fix, bind `self.pipeline.current()`).
  - `bin/unendlich_world.rs` (late drain, esc→settings, `apply_look` call, pointer
    fields).
- **Suggested order:** winding fix → input.rs → debug_settings+lib → chunk_pipeline
  → world_renderer → bin. Validate each step with `handoff/check_delimiters.py`.
- **Conflict-prone:** `world_renderer.rs` and `bin/unendlich_world.rs` are large and
  actively changing. My edits anchor on unique strings; if line numbers moved,
  re-anchor on those strings. The overlay zip holds the exact final file contents.

---

## 7. Test harnesses (`handoff/`)

Built for a blind Claude — run these in the sandbox before shipping.

- `check_delimiters.py <files…>` — comment/string/lifetime-aware brace/paren/bracket
  balancer (`.rs`, and `.comp`/`.glsl`). The structural error a compiler would catch
  but you can't.
- `check_shaders.py` — lists `build.rs` `SHADERS`, flags missing/stale `.spv` (§1).
- `force_recompile_shader.sh <stem>` — deletes a shader's `.spv` + `.hash` to force
  a clean rebuild.
- `check_octree_winding.py` — regression guard for the §2 fix (asserts flat `+Y`
  uses the corrected `q0,q3,q2,q1` / `ao.x,ao.w,ao.z,ao.y` order, and that the bumpy
  top keeps its known-visible winding).
- Rust unit tests the **user** runs: `cargo test -p unendlich-engine` exercises
  `input::tests` (edge carry) and `debug_settings::tests` (look-preset math).

---

## 8. File inventory (this session)

- **NEW:** `src/debug_settings.rs`, `handoff/*`.
- **EDITED:** `src/input.rs`, `src/lib.rs`, `src/render/chunk_pipeline.rs`,
  `src/render/world_renderer.rs`, `src/bin/unendlich_world.rs`,
  `src/shaders/chunk_mesh_octree.comp`.
- **DELETED:** `assets/shaders/chunk_mesh_octree.comp.spv` + `.spv.hash` (force regen).

## 9. Status

| Item | State |
|---|---|
| Winding fix (flat +Y) | applied; **awaiting user confirm** at variant 0 |
| Input eating / click-drag | fixed; unit-tested |
| Debug panel + Esc + Looks | working (user confirmed panel + clicks) |
| Winding sweep slider | working (used to diagnose) |
| (hook) toggles | stored, **not wired** |
| shader_mode / chunk.frag uniform | **not wired** |
| Mobs in fog | **implemented** (entity.frag froxel composite; shader-only) |
| lerp3 default-feature build break | **fixed** (ungated in debug_settings.rs) |
| Light-leak seam blend | **documented, not implemented** |
