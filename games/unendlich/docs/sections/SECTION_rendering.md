# SECTION · Rendering & Shaders

## 1. What this section is
The GPU path that turns the deterministic sim/world state into the image on
screen: terrain/entity rasterization, lighting (sun + shadows + volumetrics),
sky/atmosphere, water, post (bloom/tonemap/present), the egui overlay, and the
splitscreen plumbing. Player-facing goal: a cohesive, AAA-looking day/night world
— readable terrain, believable light shafts and fog, water that reflects sky —
running at frame budget on the Apple M2 / Asahi (FP32, Vulkan via `ash`). Render
code is **compile-on-end** (needs a device); the decision layers it consumes are
pure sim.

## 2. Already built (read in-tree)
- **`src/render/world_renderer.rs`** — the renderer. `draw_frame_with_view_and_atmos(&mut self, ctx: &VulkanContext, view: &View, atmos: &AtmosphereParams) -> Result<bool>` is the per-frame entry. Owns the shadow pre-pass (`ShadowMap`+`ShadowPipeline`), the froxel volume + fill compute (`FroxelVolume`, `FroxelFillPipeline`), a single light cascade (`last_cascade: Option<LightCascade>`, multi-cascade noted as later), and shadow-stride / shadow-cache temporal optimizations. The API is already "list of (camera, viewport) pairs" shaped but currently always renders one view (lines 11–12).
- **`CameraUbo`** (set 0, binding 0) — the shared uniform: `view_proj, camera_pos, sun_dir, sun_color, sky_color, ambient_color, atmos_params(density,humidity,temp,time), wind_params, viewport_rect, light_view_proj`. `viewport_rect.zw` already reserved for per-view pixel offsets (splitscreen-ready).
- **Shaders (32 sources in `src/shaders/`)** — terrain `chunk.{vert,frag}` + shadow `chunk_shadow.vert`; GPU mesher/cull/gen comp (`chunk_mesh*, chunk_cull, chunk_generation, octree_*`); `sky.{vert,frag}`; `froxel_fill.comp`; `cascade_fit.comp`; post `bloom_prefilter/blur.frag, tonemap.glsl, present.{vert,frag}`; `water_waves.glsl`; `entity.*`; `egui.*`; UI blur (`ui_blur/ui_downsample.comp`); `view_compute.comp`; `field_diffuse.comp`.
- **Merged AAA shaders (`handoff/SHADER_MERGE_r68.md`, 5 of 7):** r68-01 PBR terrain (Cook-Torrance GGX + hemispheric ambient + normal map, A/B via `PBR_QUALITY`); r68-03 volumetrics/god rays (`froxel_fill.comp`: dual-lobe HG, per-froxel shadowed shafts, valley fog, temporal hook off; descriptor-stable); r68-04 water (`chunk.frag` is_water + `water_waves.glsl`: Gerstner normals, Schlick fresnel, planar-sky reflect + Beer-Lambert refract, foam, caustics); r68-05 tonemap+bloom (ACES/AgX, Karis bloom, lens dirt/vignette/dither — also fixed the build.rs `SHADERS[]` stale-`.spv` gap); r68-07 sky (NEW `sky.{vert,frag}` analytic Rayleigh+Mie + `chunk.frag` aerial perspective). `chunk.frag` is a clean 3-way merge (lighting / is_water / distance-fog), spirv-val clean.
- **Pure decision layers (sim, built + tested):** `sim::player_views` (`ViewRegistry`, `compute_overlap → {per_view, union_count, shared, overlap_ratio}`), `sim::viewport_split` (rect layout in [0,1]), `sim::frustum_plane_extract` (`extract(viewproj) -> [Plane;6]`).
- **Headless GPU (`docs/HEADLESS_GPU.md`):** `VulkanContext::new_headless()` (feature `headless`) + `create_offscreen_target`, proven on lavapipe; `gpu-headless-probe` bin. Real GPU meshing measurable headless (~0.5–1.3 ms/chunk). Per-shader SPIR-V *module ingest* timed; real *pipeline-build* timing is not (see roadmap).
- **egui overlay** — `render::egui_overlay::EguiOverlay`, hand-rolled egui-0.34-on-ash, drawn end-of-main-pass under feature `ui-overlay`.

## 3. The big sub-sections to build
### A. Sky pipeline wiring  *(render / compile-on-end)*
`sky.{vert,frag}` compile + validate but **nothing calls them** (`SHADER_MERGE_r68.md` OWED #1). Add the render-side sky pass: fullscreen 3-vertex draw on the far plane (depth LEQ, no write), `CameraUbo` set0/0, before terrain. Layout is already blessed in `harness/shader_descriptor_baseline.txt`. Seam: `world_renderer` main pass setup. Deps: none — unblocks the merged sky shader actually appearing.

### B. Soft shadows  *(render / compile-on-end — r68-02 never returned)*
The shadow map + `light_view_proj` exist and `froxel_fill.comp` already PCFs them; the **main terrain `chunk.frag` shadow term** is the gap (world_renderer notes shadow consumption was still pending in the fragment path). Scope: sample `shadow_map` (set0/1) in `chunk.frag` with PCF/PCSS, wire the cascade. Seam: `chunk.frag` lighting region (descriptor already present → descriptor-stable). Deps: none.

### C. Ambient occlusion infrastructure  *(mixed — see `docs/shaders/AO_FINDINGS_r68.md`)*
A `chunk.frag`-only AO is impossible here: the fragment normal is flat-per-face (snapped to axis → silhouette halos), there is **no depth/normal G-buffer**, and **no per-vertex AO attribute** from the mesher. Two real paths: **(C1, pure-sim-ish)** a mesher vertex-AO attribute — compute per-vertex occlusion from the 8 neighbor voxels in the meshing pass (the classic 0–3 corner AO); the *occlusion calc* is a pure function; wiring it into the vertex format + `chunk.{vert,frag}` is compile-on-end. **(C2, render)** a depth(+normal) G-buffer + an SSAO/GTAO pass — new render targets, a new pass, new `chunk.frag` binding (descriptor drift → must update Rust layout + UBO parity + regenerate baseline; the doc warns silent drift breaks runtime, and there's no in-sandbox GPU to runtime-validate). C1 is the lower-risk first step. Deps: meshing section (vertex format).

### D. Splitscreen render integration  *(render / compile-on-end — `handoff/SPLITSCREEN_RENDER.md`)*
Pure layer done (`player_views`, `viewport_split`, `frustum_plane_extract`). Owed: a `ViewRegistry`-tagged per-view resource set; per-frame `reg.step(dt)` → set scissor/viewport from each `PlayerView.viewport`; extract per-view planes → `compute_overlap` → mesh/upload the **union once**, draw per-view indexing shared buffers; optional cross-fade on join/leave via the `from/target/viewport` triple. Seam: `world_renderer` (already (camera,viewport)-list shaped) + the chunk mesh pool. Deps: none (pure layer ready).

### E. Headless shader perf bench  *(bin / compile-on-end — `docs/HEADLESS_GPU.md`)*
Module-ingest timing exists; the useful metric is **compute pipeline-build** time. Blocker: building a pipeline with an empty/mismatched layout **segfaults lavapipe**. Need per-shader SPIR-V **descriptor reflection** → build the matching `DescriptorSetLayout` → time pipeline creation + a dispatch. Seam: `gpu-headless-probe` + a reflection helper. Deps: none; feeds the r62 perf matrix + `unendlich-analyze`.

### F. Multi-cascade shadows  *(render / compile-on-end)*
`last_cascade` is a single `Option<LightCascade>`; `cascade_fit.comp` exists. Scope: N-cascade split + selection in `chunk.frag`. Deps: B (soft shadows) first.

## 4. Prioritized roadmap (each ≈ one worker thread)
1. **Sky pipeline wiring** (A, render) — makes the already-merged sky shader visible; smallest, highest visible payoff. Unblocks nothing else but is owed.
2. **Main-pass soft shadows** (B, render) — descriptor-stable `chunk.frag` shadow term; the single biggest lighting-correctness win. Unblocks F.
3. **Mesher vertex-AO attribute — occlusion calc** (C1, **pure sim**: `sim::vertex_ao` — 8-neighbor corner AO, deterministic, in→out) + a follow-on compile-on-end thread to add it to the vertex format. Removes the AO blocker without a G-buffer.
4. **Splitscreen render wiring** (D, render) — pure layer is ready; turns it on.
5. **Headless shader pipeline-build bench** (E, bin) — per-shader descriptor reflection; gives CI shader-perf numbers (independent, can run in parallel).
6. **Multi-cascade shadows** (F, render) — after soft shadows land.
7. **SSAO/GTAO G-buffer** (C2, render) — only if vertex-AO proves insufficient; largest descriptor/risk surface, do last and validate on-device.

Pure-sim items (briefable as a new file + one `pub mod`): #3's occlusion calc, plus any future cascade-split-math or froxel-jitter-sequence helper. Everything else is render/bin compile-on-end.

## 5. Open questions / decisions
- **AO strategy:** vertex-AO (C1, cheap, no new bindings) vs SSAO/GTAO (C2, needs a G-buffer + descriptor change)? Recommend C1 first. Operator call on whether a G-buffer is wanted anyway (it would also unblock SSR for water, currently deferred).
- **Soft-shadow filter:** PCF (cheap, banded) vs PCSS (softer, costlier) for the main pass on M2 — pick after an on-device A/B.
- **Tunable promotion:** several merged shaders expose quality knobs as GLSL consts (e.g. r68-03 `SHAFT_INTENSITY`, PBR `PBR_QUALITY`). Decide which graduate to live `Settings`/UBO uniforms (spare `ambient_color.*` / `atmos_params.z` / `viewport_rect.*` lanes exist → no layout change) vs stay compile-time.
- **Temporal froxel reprojection:** r68-03 left a default-off hook; full history-blend needs a prev-froxel image + prev view-proj (new binding + baseline regen). Worth it for noise, or is the current no-temporal path good enough on M2?
- **On-device validation debt:** every r68 shader is compile/spirv-val/descriptor-gate clean but **visually unvalidated on M2** (`SHADER_MERGE_r68.md`). A dedicated operator visual+frame-time pass should gate before more shader work piles on.
